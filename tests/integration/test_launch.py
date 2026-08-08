# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.1 (#207): **a wire request makes the trusted core fork a process** —
against the shipped `vitrind`, the real `vitrin-shim`, and a real app.

This is the mock-free half of issue #207. `vitrin-mock-shim` appears nowhere in
this path: the core execs the built C shim, which fork/execs a real
`weston-terminal`, and the launched realm's pixels come back to the agent
through the real enforcement and capture path.

## What this gate is actually about

Until this issue, the only thing that could make `vitrind` create a process was
startup reading a file the operator had hardened. That property is gone, on
purpose, and what replaces it is weaker than "impossible": a consented, capped,
rate-limited, revocable grant. So the assertions here are not "launch works" —
they are the *bounds*, each one a thing that would silently not be true if the
verb had been wired straight to `spawn.rs`:

1. **A launch happens at all, and the id comes from the core.** A principal
   holding `realm.launch` over a template calls `launch()`, receives an id it
   never chose, and a *separate* `observe` petition over that id captures the
   new app's real pixels. Ancestry is read from procfs, so what forked is the
   real C shim with a real app under it.
2. **Launching confers nothing over what was launched.** The observe grant in
   (1) is a second petition; the launch grant alone captures nothing.
3. **A principal without the verb is refused `not_granted`** — recoverably,
   with the socket still usable afterwards.
4. **A human's `deny` is honoured**, and the card they were shown really did
   grow the extra field the launch verb adds.
5. **The rate ceiling binds**: launching faster than `max_event_rate` is
   refused `rate_limited` with a nonzero `retry_after_ms`.
6. **The journal names who asked** — asserted on the `realm_spawned` record's
   `principal` and `grant_id`, never inferred from timing.

## What this test deliberately does not assert

Two things, both stated rather than left as holes a reader has to notice:

- **The realm cap (`capacity`).** Reaching it needs sixteen live realms —
  sixteen shims and sixteen real apps on a CI runner — a resource claim this
  ladder should not make for one refusal code. It is proved in-crate
  (`session.rs::a_launch_past_the_realm_cap_refuses_capacity_and_forks_nothing`).
- **The words on the consent card.** The injector reports the card's geometry
  and pixels, not its text, so nothing here can read the program's path off
  the screen; assertion (4) checks only that the launch card is *taller* than
  a one-verb `observe` card in the same session, which is the extra field
  having reached a real rasterization. *Which* field it is, and that it holds
  the template's `command`, is proved in-crate
  (`consent/render.rs::a_launch_prompt_names_the_program_and_only_a_launch_prompt_does`
  and `petitions.rs::only_a_launch_petition_carries_its_templates_command_to_the_prompt`).

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`).
"""

from __future__ import annotations

import os
import pathlib
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "sdk", "python", "src"))

from harness import (  # noqa: E402
    IntegrationTest,
    children_of,
    comm_of,
    descendant_named,
    has_real_content,
    whole_realm_grant,
)

import vitrin_os  # noqa: E402
from vitrin_os import errors  # noqa: E402

#: The app the launched realm runs. A real Wayland client with visible chrome,
#: the same rung `test_real_app.py` uses.
APP_NAME = "weston-terminal"

#: The template's id. Never `realm-0`: the point of the fixture is that this
#: realm does **not** run until somebody with authority asks for it.
TEMPLATE = "kiosk"

#: The instance id the core is expected to mint for the template's first
#: launch. Asserted once, in the first test, because the *shape* is the core's
#: business and a client must treat it as opaque — but a gate that accepted any
#: string could not tell a minted id from an echoed one.
FIRST_INSTANCE = f"{TEMPLATE}.1"

REALM_SIZE = "640x480"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


class RealLaunch(IntegrationTest):
    """The six-assertion WS-E.1.1 gate, against the real chain end to end."""

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
        app = shutil.which(APP_NAME) or (
            f"/usr/bin/{APP_NAME}" if os.access(f"/usr/bin/{APP_NAME}", os.X_OK) else None
        )
        if app is None:
            self.fail(
                f"{APP_NAME} is not installed, but VITRIN_C_SHIM_BIN is set so a real run was "
                "requested. Install weston (shim/ci/install-deps.sh does), or set "
                "VITRIN_SKIP_REAL_APP=1 to opt out."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

    # -- fixtures ----------------------------------------------------------

    def launch_core(self, **kwargs):
        """A core whose `realm.toml` holds `realm-0` plus one **template**.

        `realm-0` autostarts (the core refuses a configuration in which every
        realm is a template, and a session with no app is not a session);
        `kiosk` does not, so nothing runs it until a launch grant does.
        """
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=[],
            templates=(TEMPLATE,),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            **kwargs,
        )

    def _await_startup(self, core, expected: int = 1) -> list[int]:
        """Wait until the core has exactly `expected` C shims and return them."""
        deadline = time.monotonic() + 25.0
        shims: list[int] = []
        while time.monotonic() < deadline:
            shims = [p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]
            if len(shims) >= expected:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(shims),
            expected,
            f"expected {expected} C shim(s); core's children were {children_of(core.pid)}",
        )
        return shims

    def _observe_until_painted(self, grant, timeout=30.0):
        """`observe()` until the launched app has actually painted."""
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
        self.fail(f"the launched app never painted real content; last frame was {last!r}")

    # -- 1, 2, 6 -----------------------------------------------------------

    def test_a_launch_forks_a_real_app_the_agent_can_then_petition_to_watch(self) -> None:
        with self.launch_core() as core:
            # Exactly one shim before the launch: the template is in the
            # registry and is NOT running. A core that forked templates would
            # fail here rather than quietly running an app nobody asked for.
            self._await_startup(core, expected=1)

            conn = core.connect()
            launcher = whole_realm_grant(conn, verbs=("realm.launch",), realm=TEMPLATE)
            self.assertIn(vitrin_os.Verb.REALM_LAUNCH, launcher.effective_verbs)

            instance = launcher.launch()
            self.assertEqual(
                instance,
                FIRST_INSTANCE,
                "the instance id is minted by the core as <template>.<n>; `launch` carries no "
                "arguments, so the client cannot have named it",
            )

            # A second shim, with a second real app under it: the launch
            # really created a process, and the process is the real chain.
            self._await_startup(core, expected=2)
            app_pid = descendant_named(core.pid, APP_NAME, timeout=25.0)
            self.assertIsNotNone(app_pid, "the launched shim never fork/exec'd the app")

            # **Launching confers nothing over what was launched.** The launch
            # grant holds only `realm.launch`, so it has no view facet to
            # capture through; watching the new realm is a SECOND petition,
            # over the id the core just minted.
            watcher = whole_realm_grant(conn, verbs=("observe",), realm=instance)
            frame = self._observe_until_painted(watcher)
            self.assertEqual((frame.width, frame.height), (640, 480))

            conn.close()
            core.terminate()

            # 6. The journal names who asked -- asserted on the record, not
            # inferred from timing. Two spawns: startup's `realm-0`, with no
            # principal, and the launch, with one.
            spawns = [e for e in core.entries() if e.get("kind") == "realm_spawned"]
            by_realm = {e["realm"]: e for e in spawns}
            self.assertIn(instance, by_realm, f"no realm_spawned for {instance}: {spawns}")
            launched = by_realm[instance]
            self.assertEqual(launched["spawned_by"], "realm_launch")
            self.assertEqual(launched["principal"], "vitrin://local/agent/demo")
            self.assertTrue(
                launched["grant_id"],
                "the entry must name the grant row the launch was judged against",
            )
            self.assertEqual(launched["command"], self.app_bin)
            self.assertEqual(by_realm["realm-0"]["spawned_by"], "startup")
            self.assertIsNone(by_realm["realm-0"]["principal"])

    # -- 3 -----------------------------------------------------------------

    def test_a_principal_without_the_verb_is_refused_not_granted(self) -> None:
        with self.launch_core() as core:
            self._await_startup(core, expected=1)
            conn = core.connect()
            # A grant over the same template, carrying everything BUT the
            # launch verb. The facet still mints -- refusing the mint would
            # make it an oracle for what the grant holds -- and the use is
            # what refuses.
            grant = whole_realm_grant(conn, verbs=("observe",), realm=TEMPLATE)
            with self.assertRaises(errors.NotGranted) as caught:
                grant.launch()
            self.assertEqual(caught.exception.verb, int(vitrin_os.Verb.REALM_LAUNCH))
            # Recoverable: the socket is still usable, proved by a real round
            # trip rather than by `not conn.closed` alone.
            conn.sync()
            self.assertFalse(conn.closed)

            # Nothing forked.
            time.sleep(0.5)
            self.assertEqual(
                len([p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]),
                1,
                "a refused launch must create no process",
            )
            conn.close()
            core.terminate()
            self.assertNotIn(
                FIRST_INSTANCE,
                [e.get("realm") for e in core.entries() if e.get("kind") == "realm_spawned"],
            )

    # -- 4 -----------------------------------------------------------------

    def test_a_denied_launch_petition_resolves_denied_and_forks_nothing(self) -> None:
        with self.launch_core(consent="interactive", consent_injector=True) as core:
            self._await_startup(core, expected=1)
            core.injector.await_banner()
            conn = core.connect()

            # A one-verb `observe` petition first, for the height baseline.
            # Approved rather than denied so the two prompts cannot overlap:
            # a pending petition would hold the second behind it.
            baseline_grant = conn.request_grant(
                realm=TEMPLATE,
                verbs=("observe",),
                persistence=vitrin_os.Persistence.WHILE_RUNNING,
            )
            petition, token = core.injector.await_raised()
            baseline, _pixels = core.injector.describe()
            self.assertEqual(core.injector.decide(token, "allow-while-running"), "queued")
            core.injector.await_lowered(petition)
            baseline_grant.await_consent()

            grant = conn.request_grant(
                realm=TEMPLATE,
                verbs=("realm.launch",),
                persistence=vitrin_os.Persistence.WHILE_RUNNING,
            )
            petition, token = core.injector.await_raised()
            # The card the human is deciding on carries the extra field the
            # launch verb adds -- the program they would be authorizing the
            # launching of. The injector reports geometry, not text, so what
            # is asserted here is that the field reached a real rasterization;
            # its contents are pinned in-crate (see the module docstring).
            described, _pixels = core.injector.describe()
            self.assertGreater(
                described["card_h"],
                baseline["card_h"],
                "a realm.launch card must be taller than a one-verb observe card: it carries "
                f"the Launches field as well. observe={baseline} launch={described}",
            )
            self.assertEqual(core.injector.decide(token, "deny"), "queued")
            with self.assertRaises(errors.GrantDenied):
                grant.await_consent()
            core.injector.await_lowered(petition)

            time.sleep(0.5)
            self.assertEqual(
                len([p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]),
                1,
                "a denied petition must create no process",
            )
            conn.close()
            core.terminate()
            kinds = [e.get("kind") for e in core.entries()]
            self.assertIn("petition_resolved", kinds, f"the denial must be journaled: {kinds}")

    # -- 5 -----------------------------------------------------------------

    def test_launching_faster_than_the_grant_allows_is_rate_limited(self) -> None:
        with self.launch_core() as core:
            self._await_startup(core, expected=1)
            conn = core.connect()
            # One launch per second: the first is admitted, the second in the
            # same second finds an empty bucket.
            grant = conn.request_grant(
                realm=TEMPLATE,
                verbs=("realm.launch",),
                max_event_rate=1,
                persistence=vitrin_os.Persistence.WHILE_RUNNING,
            ).await_consent()
            first = grant.launch()
            self.assertEqual(first, FIRST_INSTANCE)
            with self.assertRaises(errors.RateLimited) as caught:
                grant.launch()
            self.assertGreater(
                caught.exception.retry_after_ms,
                0,
                "a rate_limited refusal must carry a nonzero retry hint (IDL)",
            )
            # The bucket is the LAST gate, so exactly one realm was created:
            # a refused use must not have forked.
            self._await_startup(core, expected=2)
            time.sleep(0.5)
            self.assertEqual(
                len([p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]),
                2,
                "a rate-limited launch must create no second process",
            )
            conn.close()
            core.terminate()
