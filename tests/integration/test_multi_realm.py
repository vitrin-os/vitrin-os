# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.2 (issue #208): the shipped `vitrind` serves more than one realm.

Why this belongs *here* and not only in `cargo test`: the in-process tests
in `crates/vitrin-core/src/session.rs` build the runtime by calling
`start_realm_in` directly, so they never execute `run_session` — the
function the shipped binary uses to order startup. The multi-realm change
put a **loop** inside that ordering (`install()` → fork realm 1 → fork realm
2 → …), and a regression that moved the loop above `install()` would wedge
every shim after the first on `configure` while leaving every unit test
green.

What is claimed, and what is not. Each realm really does get its own shim
process, its own `$XDG_RUNTIME_DIR/vitrin-0/<id>/` tree and its own
`wayland-0` address — that is the paths half of the claim
`crates/vitrin-core/src/realm.rs` audits, and it is the half that was
genuinely a deletion. Since WS-E.1.3 each realm also gets its own scene and
its own capture: an `observe` grant returns the pixels of the realm it
names, hidden or not. What multi-realm does *not* buy is a per-realm
**output** -- the core composites one output from the one realm bound to it,
the first to attach -- and nothing below asserts otherwise.

These are **component** tests: the realms run `vitrin-mock-shim`, not a real
app under the real C shim, so they are not evidence for any milestone. The
mock shim binds no `wayland-0`, which is why the socket assertion below is
about the address the core hands each realm rather than about a bound
socket file.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import tempfile
import time
import unittest

from harness import (
    IntegrationTest,
    capture_when_ready,
    require_binaries,
    whole_realm_grant,
)

require_binaries()


EXTRA_REALMS = ("editor", "browser")
ALL_REALMS = ("realm-0", *EXTRA_REALMS)


def _realm_dir(core, realm: str) -> pathlib.Path:
    return core.runtime / "vitrin-0" / realm


def _spawned(core) -> dict[str, int]:
    """`realm -> pid` for every `realm_spawned` the run journalled."""
    return {
        entry["realm"]: entry["pid"]
        for entry in core.entries()
        if entry["kind"] == "realm_spawned"
    }


def _alive(pid: int) -> bool:
    return pathlib.Path(f"/proc/{pid}").exists()


class ThreeRealms(IntegrationTest):
    """Three configured realms, three shims, three private trees."""

    def test_each_configured_realm_gets_its_own_shim_and_runtime_tree(self):
        with self.core(realms=EXTRA_REALMS) as core:
            # The socket is up, so `run_session` finished startup: the loop
            # forked all three realms *after* `install()` registered their
            # sources, or the second and third would still be blocked in
            # `configure` and the core would never have bound.
            for realm in ALL_REALMS:
                directory = _realm_dir(core, realm)
                self.assertTrue(
                    directory.is_dir(),
                    f"{realm} must have its own runtime tree; found "
                    f"{sorted(p.name for p in (core.runtime / 'vitrin-0').iterdir())}",
                )
                # A sibling `<id>.lock` per realm, which is what lets two
                # realms be purged and re-owned independently.
                self.assertTrue(
                    directory.with_name(realm + ".lock").exists(),
                    f"{realm} must hold its own realm lock",
                )

            # An agent still works against the well-known realm, unchanged.
            # This is the additive half: nothing a version-1 client does
            # cares how many other realms exist.
            conn = core.connect()
            grant = whole_realm_grant(conn)
            frame = capture_when_ready(grant)
            self.assertEqual((frame.width, frame.height), (320, 200))
            conn.close()

        # Three `realm_spawned` records, three distinct ids, three distinct
        # pids — read off the flight recorder, which is the artifact that has
        # to reconstruct the session afterwards.
        spawned = _spawned(core)
        self.assertEqual(sorted(spawned), sorted(ALL_REALMS))
        self.assertEqual(
            len(set(spawned.values())),
            3,
            f"three realms must be three processes: {spawned}",
        )

        # ...and the shutdown ladder ran once per realm rather than once.
        exited = sorted(
            entry["realm"] for entry in core.entries() if entry["kind"] == "realm_exited"
        )
        self.assertEqual(
            exited,
            sorted(ALL_REALMS),
            f"every realm must be torn down at shutdown; kinds={core.kinds()}",
        )

    def _refused_startup(self, realm_toml: str) -> str:
        """Boot a core against `realm_toml`, require it to abort, return its
        output.

        The tree is written by hand rather than by starting a core first:
        these two configurations are ones the loader must refuse, so there
        is never a running session to inherit a tree from.
        """
        from harness import DEMO_IDENTITY, DEMO_TOKEN, Core

        runtime = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-it-refuse-"))
        self.addCleanup(shutil.rmtree, runtime, ignore_errors=True)
        principals = runtime / "principals.toml"
        principals.write_text(
            f'[[principal]]\nidentity = "{DEMO_IDENTITY}"\ntoken = "{DEMO_TOKEN}"\n'
        )
        principals.chmod(0o600)
        (runtime / "realm.toml").write_text(realm_toml)

        core = Core(runtime_dir=runtime, wait=False, write_config=False)
        self._cores.append(core)
        self.assertNotEqual(
            core.await_exit(timeout=30),
            0,
            "this configuration must abort startup rather than come up",
        )
        return core.output()

    def test_a_realm_toml_over_the_cap_is_refused_at_startup(self):
        """The cap is enforced by the loader, naming the count and the cap.

        `MAX_REALMS` is 16, so seventeen tables is one too many. Asserted on
        the message an operator reads, because the refusal's whole job is to
        say which two numbers disagree — the same refusal shape the old
        `exactly one` message had.
        """
        from harness import MOCK_SHIM

        output = self._refused_startup(
            "".join(
                f'[[realm]]\nid = "realm-{n}"\ncommand = "{MOCK_SHIM}"\nargs = ["--serve"]\n'
                for n in range(17)
            )
        )
        self.assertIn("realm.toml", output, output)
        self.assertIn("17 [[realm]] tables", output, output)
        self.assertIn("at most 16", output, output)

    def test_a_realm_toml_without_realm_0_is_refused_at_startup(self):
        """`realm-0` stays a required member of every configuration.

        The id pin is gone; the reason it existed is not. A deployment
        without `realm-0` answers every conformant client's
        `get_realm("realm-0")` petition `unavailable` forever, and the
        refusal has to say so in the client's own terms.
        """
        from harness import MOCK_SHIM

        output = self._refused_startup(
            f'[[realm]]\nid = "kiosk"\ncommand = "{MOCK_SHIM}"\nargs = ["--serve"]\n'
        )
        self.assertIn("realm-0", output, output)
        self.assertIn("get_realm", output, output)

    def test_ids_that_collide_in_the_runtime_tree_are_refused_at_startup(self):
        """Two realms may not fight over one entry in the runtime tree.

        `$XDG_RUNTIME_DIR/vitrin-0/` is flat: a realm owns `<id>/` and, as a
        *sibling*, `<id>.lock`. Free-form ids made `foo` and `foo.lock` name
        the same entry — invisible to either path helper on its own, so the
        refusal has to be a property of the whole file, and the message has
        to name both.
        """
        from harness import MOCK_SHIM

        table = '[[realm]]\nid = "{}"\ncommand = "{}"\nargs = ["--serve"]\n'
        output = self._refused_startup(
            table.format("realm-0", MOCK_SHIM)
            + table.format("foo", MOCK_SHIM)
            + table.format("foo.lock", MOCK_SHIM)
        )
        self.assertIn("foo", output, output)
        self.assertIn("foo.lock", output, output)


class OneRealmDies(IntegrationTest):
    """Killing one realm's shim leaves the others running and petitionable."""

    def test_killing_one_realm_does_not_disturb_the_others(self):
        with self.core(realms=EXTRA_REALMS) as core:
            # Give every shim time to attach and paint, so the survivors are
            # doing something rather than merely existing.
            conn = core.connect()
            grant = whole_realm_grant(conn)
            capture_when_ready(grant)

            spawned = {
                entry["realm"]: entry["pid"]
                for entry in core.entries()
                if entry["kind"] == "realm_spawned"
            }
            self.assertEqual(sorted(spawned), sorted(ALL_REALMS), spawned)

            victim = spawned["editor"]
            survivors = [spawned["realm-0"], spawned["browser"]]

            # A real kill. Both death signals become available to the core:
            # EOF on the socketpair and a SIGCHLD it must poll EVERY realm
            # for — a reaper that stopped at the first would leave this pid
            # a zombie and the registry calling `editor` Running.
            os.kill(victim, signal.SIGKILL)

            deadline = time.monotonic() + 20
            while _alive(victim) and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertFalse(
                _alive(victim),
                "the killed shim must be reaped by the core's SIGCHLD handling, not "
                "left a zombie — which is what a reaper that stopped at the first "
                "live realm would leave behind",
            )

            for pid in survivors:
                self.assertTrue(
                    _alive(pid), "a sibling's death must not touch a survivor"
                )

            # The survivors still answer petitions: a fresh grant over
            # realm-0 resolves after the sibling died. `admits_petitions`
            # would have flipped for all three if the death funnel had been
            # keyed wrongly.
            whole_realm_grant(conn)
            conn.close()

        # Exactly one `realm_died` names the killed realm, before shutdown
        # buries the rest. The latch is per realm, so one death is one
        # record — and no other realm may appear in a `realm_died` written
        # before the run's shutdown.
        died = [entry for entry in core.entries() if entry["kind"] == "realm_died"]
        for_editor = [entry for entry in died if entry["realm"] == "editor"]
        self.assertEqual(
            len(for_editor),
            1,
            f"exactly one realm_died for the killed realm; got {died}",
        )
        self.assertEqual(
            for_editor[0]["cause"],
            "connection_closed",
            f"the killed shim's death is observed as its connection ending: {for_editor}",
        )

    def test_a_dead_realms_grant_cannot_capture_a_live_siblings_frame(self):
        """A grant over a dead realm refuses `no_surface`; the survivor's does not.

        Liveness is judged against the realm a grant row names, not against
        "is any realm live". The second reading is fail-**open** across
        realms: with two realms attached, a grant over the dead one clears
        the gate on the living one's account and is then served a frame at
        all.

        Since WS-E.1.3 the frame it would be served is that realm's own last
        composition rather than the sibling's — the scenes are per realm and
        the capture cache is keyed and pruned by realm id. So the failure this
        gate now stands between is a **stale** frame, which the protocol
        forbids in as many words (`no_surface` is documented as "never a stale
        frame"), rather than a sibling's pixels. One property milder, still
        refused here.

        The pair is the evidence. A fix that refused *everything* after any
        realm's death would satisfy the first assertion and lose the session.
        """
        from vitrin_os import errors

        with self.core(realms=EXTRA_REALMS) as core:
            conn = core.connect()
            doomed = whole_realm_grant(conn, realm="editor")
            survivor = whole_realm_grant(conn, realm="realm-0")
            # Both capture while both realms are live: the baseline the
            # refusal below is measured against. Each frame is its own realm's
            # composition; that the two happen to be byte-identical here is a
            # content coincidence, because `vitrin-mock-shim` paints a pure
            # function of the frame index and both realms run the same mock.
            # Two *different* pictures proving the selection is
            # `test_two_realms.py`'s job, with real apps — this test asserts
            # only which grant is refused.
            capture_when_ready(survivor)
            capture_when_ready(doomed)

            editor = _spawned(core)["editor"]
            os.kill(editor, signal.SIGKILL)
            deadline = time.monotonic() + 20
            while _alive(editor) and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertFalse(_alive(editor), "the victim must be reaped")

            # The survivor keeps painting, so *a* realm in this session still
            # has a committed surface — exactly the state a fail-open
            # ("is any realm live") check would have cleared the dead realm's
            # grant on.
            capture_when_ready(survivor)

            with self.assertRaises(errors.NoSurface):
                doomed.observe()
            # ...and refusing it cost the survivor nothing.
            capture_when_ready(survivor)
            conn.close()


class CrossRealmActuation(IntegrationTest):
    """An actuation is refused unless its grant names the realm the seat serves.

    The write-side twin of the capture test above, at the wire level and
    against the mock shim — so it runs everywhere, including on a machine
    with no built C shim. `test_real_actuation.py`'s `RealActuationCrossRealm`
    is the same claim proved by a real app's own receipt; this one proves
    what the *agent* is told and what the journal records.

    Note which realm the seat serves: `session::seat_target` takes the first
    still-serving realm in **id order**, and `browser` sorts before both
    `editor` and `realm-0`. So the well-known realm is *not* the delivery
    target here, which is exactly the configuration where the missing guard
    would have sent a `realm-0` grant's keystrokes into the browser.
    """

    def test_only_a_grant_over_the_served_realm_may_actuate(self):
        from vitrin_os import errors

        # `seat=True`: the mock shims mint their seat objects, so an admitted
        # actuation is really delivered rather than dropped for want of one —
        # without it the negative half would pass for the wrong reason.
        with self.core(realms=EXTRA_REALMS, seat=True) as core:
            conn = core.connect()
            served = whole_realm_grant(conn, realm="browser")
            unserved = whole_realm_grant(conn, realm="realm-0")
            # A committed surface, so neither actuation can refuse
            # `no_surface` — the refusal below has to be the routing guard
            # and nothing else.
            capture_when_ready(served)

            with self.assertRaises(errors.OperationFailed):
                unserved.text.type("into the wrong app")
            # Recoverable: the connection is still serving.
            conn.sync(unserved)

            served.text.type("into the right one")
            conn.sync(served)
            conn.close()

        # One delivery, and it names the realm that received it (the field
        # added by the second WS-E.1.2 review: without it the journal could
        # say a keystroke was delivered and not to whom).
        delivered = [e for e in core.entries() if e["kind"] == "seat_delivered"]
        self.assertEqual(
            [(e["realm"], e["event"]) for e in delivered],
            [("browser", "text")],
            f"exactly one text delivery, into the seat's realm; got {delivered}",
        )
        refused = [
            e
            for e in core.entries()
            if e["kind"] == "use_decision" and e.get("refusal") == "internal"
        ]
        self.assertEqual(
            len(refused),
            1,
            f"the cross-realm actuation must be journalled as refused internal; got {refused}",
        )


if __name__ == "__main__":
    unittest.main()
