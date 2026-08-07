# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.6 (#212): **physical input follows the human's attention; an agent's
actuation follows its grant** — against two real apps, over a real socket,
through the shipped `vitrind`.

This is the mock-free half of issue #212. The in-crate tests
(`input/mod.rs`'s `a_binding_change_drains_the_humans_held_presses_and_only_the_humans`
and `a_prompt_consumes_physical_input_for_every_realm_not_only_the_bound_one`,
`principal.rs`'s `a_grant_over_a_hidden_realm_actuates_into_that_realm_and_no_other`
and `physical_presence_in_one_realm_preempts_an_agent_only_in_that_realm`,
`deadman.rs`'s `a_chord_held_in_one_realm_revokes_grants_over_every_realm`) pin
the two addressing rules in process; this drives the shipped binary, two real
`vitrin-shim` processes and two real Wayland clients, and asks each app what it
actually received.

## Why a test injector exists at all, stated rather than assumed

CI cannot produce physical input. `SeatInput::physical` is private to
`crate::input` by design, `intake_physical` is the nested backend's only
production caller, headless has no input device, and headless is the only
backend CI runs (D-019(4)). Issue #212 says so in as many words and asks for
the honest alternative rather than a criterion that could never go green: a
`physical-input-injector` cargo feature, feeding **the same entry point** the
nested backend's winit handler feeds — never a second, weaker path — over an
inherited socketpair named by `--physical-input-fd N`.

What that build weakens is named where the claim is made
(`crates/vitrin-core/src/input/mod.rs`'s module docs, and the feature's own
block in `Cargo.toml`): in a shipping build "nothing but a human's device
mints a physical tag" is a **compile-time** guarantee, and in this one it is a
runtime guarantee — the feature, plus the flag, plus possession of a
descriptor.

## The properties

1. **An agent's click reaches the realm its grant names, while another realm
   is the one on screen.** Proved by that app's own surface flipping colour,
   read back through the agent's own `observe()` of that realm.
2. **A human's click reaches the realm the output is bound to** — the *other*
   realm, in the same session, in the same round.
3. **After a `layout.focus` moves the binding, the human's click follows** and
   the agent's does not move at all.
4. **A key held across that switch is released into the realm being left**,
   which the flight recorder shows as a `key` delivery to the losing realm
   with `origin=physical` — the drain issue #212's decision 3 exists for. The
   app cannot tell that release from a real one, which is a published limit
   (`docs/book/src/limits.md`), not a defect.

What this deliberately does **not** assert is that a *human at real hardware*
sees any of it. That half is `shim/docs/nested-multi-realm.md` step 9, a
nested manual runbook step, exactly as issue #212 asks.

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
    children_of,
    comm_of,
    dominant_colour,
    locate_colour,
    whole_realm_grant,
)

import vitrin_os  # noqa: E402
from vitrin_os import errors  # noqa: E402

#: Matches the rest of the real-app ladder.
REALM_SIZE = "640x480"

#: `click-target`'s own colours (see `shim/tests/click_target.c`).
TARGET = "00ff00"
HIT = "ff0000"
MIN_TARGET_PIXELS = 5000

#: `realm-0` sorts first, so it is the realm the output binds to at startup
#: and `second` is the hidden one a grant has to reach on its own.
SECOND = "second"

RUN_MS = "60000"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

class RealInputSwitch(IntegrationTest):
    """Two addressing rules, one session, two real apps."""

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
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so "
                "a real run was requested; refusing to skip a requested gate (CI misconfig)."
            )
        app = os.environ.get("VITRIN_CLICK_TARGET_APP")
        if not app:
            sibling = self.shim_bin.resolve().parent / "click-target"
            if sibling.is_file() and os.access(sibling, os.X_OK):
                app = str(sibling)
        if not app:
            self.fail(
                f"no click-target beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-inputswitch-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    # -- fixtures ---------------------------------------------------------

    def _core(self):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            realms=(SECOND,),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            physical_input=True,
            log_file=str(self.work / "core.log"),
        )

    def _two_live_apps(self, core) -> None:
        deadline = time.monotonic() + 25.0
        shims: list[int] = []
        while time.monotonic() < deadline:
            shims = [p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]
            if len(shims) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(shims), 2, f"two realms must be two C shims; core's children: {shims}"
        )
        apps: list[int] = []
        while time.monotonic() < deadline:
            apps = [
                kid
                for shim in shims
                for kid in children_of(shim)
                if comm_of(kid).startswith("click-target")
            ]
            if len(apps) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(apps), 2, f"each shim must have fork/exec'd its own click-target; got {apps}"
        )

    def _locate_target(self, grant, timeout=25.0):
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
            cx, cy, count = locate_colour(frame, TARGET)
            if count >= MIN_TARGET_PIXELS:
                return cx, cy
            time.sleep(0.1)
        self.fail("no click-target ever painted a locatable target; there is nothing to click")

    def _await_flip(self, grant, timeout=15.0):
        """This realm's dominant-colour share once it is the hit colour, else `None`."""
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
            colour, share = dominant_colour(frame)
            if colour == HIT and share >= 90:
                return share
            time.sleep(0.1)
        return None

    def _dominant(self, grant, timeout=10.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                return dominant_colour(grant.observe())
            except errors.NoSurface:
                time.sleep(0.05)
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
        self.fail("could not observe this realm at all")

    # -- the gate ---------------------------------------------------------

    def test_physical_input_follows_the_binding_and_an_agent_follows_its_grant(self):
        core = self._core()
        self._two_live_apps(core)

        phys = core.physical
        self.assertIsNotNone(
            phys,
            "the harness must have wired --physical-input-fd; without it this gate proves "
            "nothing about physical input",
        )
        banner = phys.await_banner()
        self.assertEqual(
            banner,
            "vitrin-physical-input 1",
            "the core must have ADOPTED the channel, not merely been handed a socket. A "
            "plain build does not know `--physical-input-fd` at all and would have exited "
            "at argument parsing; rebuild with "
            "`--features vitrin-core/physical-input-injector` (tests/integration/run.sh does).",
        )

        conn = core.connect()
        # `observe` + `actuate.pointer` on each realm, and deliberately **no**
        # layout verb: D-018(4)'s single-holder rule resolves a second
        # `layout.arrange` petition `layout_held`, and nothing here needs to
        # move the binding. (The second test below does, and takes
        # `layout.focus` on one realm only for the same reason.)
        first = conn.request_grant(
            realm="realm-0",
            verbs=("observe", "actuate.pointer"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()
        second = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "actuate.pointer"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()

        # Both apps are the same program at the same view size, so one
        # location serves both.
        cx, cy = self._locate_target(first)

        # (1) THE AGENT, INTO THE HIDDEN REALM. `realm-0` is on screen; this
        # grant names `second`. Before #212 the core refused this `internal`
        # and no app was touched.
        second.pointer.click(cx, cy)
        conn.sync(second)
        flipped = self._await_flip(second)
        self.assertIsNotNone(
            flipped,
            "the hidden realm's app never flipped: an actuation under a grant over a realm "
            "the output is not showing must still reach that realm's app, which is the "
            "whole of WS-E.1.6",
        )
        # ...and the realm on screen was not touched by it.
        self.assertNotEqual(
            self._dominant(first)[0],
            HIT,
            "the realm ON SCREEN flipped when a grant over a DIFFERENT realm clicked: the "
            "agent's actuation followed the output instead of its own grant",
        )

        # (2) THE HUMAN, INTO THE BOUND REALM. Same coordinates, same round,
        # physical-tagged through the core's own intake.
        phys.click(cx, cy)
        self.assertIsNotNone(
            self._await_flip(first),
            "the human's click never reached the realm the output is bound to: physical "
            "input must follow the binding",
        )

        # Both realms have now been clicked, each by the rule that addresses
        # it, and the journal says which realm each delivery named.
        entries = core.entries()
        buttons = [
            e for e in entries if e["kind"] == "seat_delivered" and e["event"] == "button"
        ]
        self.assertEqual(
            {(e["realm"], e["origin"]) for e in buttons},
            {(SECOND, "emulated"), ("realm-0", "physical")},
            f"the two rules must have addressed two different realms, and each delivery must "
            f"carry the tag intake bound (B2). Got {buttons}",
        )

        core.terminate()
        out = core.output()
        hits = [ln for ln in out.splitlines() if ln.startswith("HIT ")]
        self.assertEqual(
            len(hits),
            2,
            f"each click-target must report exactly one HIT of its own — the agent's and the "
            f"human's. Got {hits}.",
        )

        print(
            f"\n[input-switch] realm-0 on screen: an agent's grant over {SECOND!r} clicked "
            f"({cx}, {cy}) and THAT app flipped; a physical click at the same point flipped "
            f"realm-0's"
        )

    def test_a_key_held_across_a_focus_change_is_released_into_the_realm_being_left(self):
        """Issue #212's decision 3, over the wire: the drain.

        A physical modifier goes down while `realm-0` is on screen; a
        `layout.focus` holder then moves the output to `second`. The core owes
        `realm-0`'s app a release, because the human's own release will now be
        addressed to `second` — and it pays it *to `realm-0`*, tagged
        `physical`, before anything else.

        The evidence is the flight recorder's `seat_delivered` entries, which
        name the realm and the origin and deliberately name **nothing else**:
        an entry carrying the keysym or the key state would be one field away
        from a keylogger (`crates/vitrin-core/src/recorder.rs`). So the claim
        is counted rather than read: `realm-0` saw **two** key deliveries (the
        press and the release it is owed) and `second` saw **none** — it never
        saw the press go down, so it is owed nothing.
        """
        core = self._core()
        self._two_live_apps(core)
        phys = core.physical
        phys.await_banner()

        conn = core.connect()
        holder = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "layout.focus"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()
        self.assertIn(
            "layout.focus",
            _verb_names(holder.effective_verbs),
            "the core must actually grant layout.focus, or nothing below moves the binding",
        )
        # `realm-0` has to be serving before the key goes down, or the press
        # is dropped for want of a seat and there is nothing to strand.
        watcher = whole_realm_grant(conn, realm="realm-0")
        self._locate_target(watcher)

        # The human holds Left Ctrl down. It is in the core's layout-invariant
        # scancode table, so it resolves with no host keymap — which is the
        # whole set a headless core can type (see `input::injector`).
        phys.key(phys.KEY_LEFTCTRL, True)

        def key_deliveries():
            return [
                (e["realm"], e["origin"])
                for e in core.entries()
                if e["kind"] == "seat_delivered" and e["event"] == "key"
            ]

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and not key_deliveries():
            time.sleep(0.05)
        self.assertEqual(
            key_deliveries(),
            [("realm-0", "physical")],
            "fixture check: the press must have been delivered to the realm on screen, or "
            "the focus change below has nothing to strand",
        )

        # The binding moves, with the key still down.
        #
        # The wait is load-bearing, not a settle-sleep: `layout.focus` is
        # attention-shaped, so the chokepoint refuses it `preempted` while
        # physical input owns the realm the human's input follows
        # (`enforcement.rs` step 5c, judged against `env.physical_realm` for a
        # layout verb). The press above IS that ownership, for
        # `PHYSICAL_HOLD_WINDOW` = 500ms. Focus immediately and the request is
        # refused, the binding never moves, and this test silently asserts
        # nothing -- which is exactly how it first went red.
        #
        # Waiting the window out is not a cheat, because presence and
        # held-key state are different facts: `PhysicalPresence` tracks
        # *recency of human action* plus held BUTTONS, never held keys
        # (`input/mod.rs`), while the drain is owed off the router's
        # `pressed_keys`. So after 500ms the human still physically holds
        # Ctrl and the core still owes realm-0 its release -- which is the
        # state decision 3 is actually about.
        time.sleep(0.7)
        holder.focus()
        conn.sync()

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and len(key_deliveries()) < 2:
            time.sleep(0.05)
        self.assertEqual(
            key_deliveries(),
            [("realm-0", "physical"), ("realm-0", "physical")],
            "the realm losing the binding must be paid the release it is owed, tagged with "
            "the origin its own press recorded — and the realm GAINING the binding must be "
            "sent nothing, because it never saw the press go down. A missing second entry "
            "is a modifier latched down in an app the human can no longer see.",
        )

        # And the human's real release, arriving after the switch, reaches
        # nobody: it pairs with no delivered press in the realm now bound.
        phys.key(phys.KEY_LEFTCTRL, False)
        conn.sync()
        time.sleep(0.5)
        self.assertEqual(
            key_deliveries(),
            [("realm-0", "physical"), ("realm-0", "physical")],
            "the human's own release must not reach the realm they moved to: that app never "
            "saw the press, and an unpaired release is exactly what the router drops",
        )

        conn.close()
        core.terminate()
        print(
            "\n[input-switch] a physically held Ctrl survived a layout.focus: realm-0 was "
            "paid its release and 'second' was sent nothing"
        )

    def test_after_a_focus_change_the_human_follows_and_the_agent_does_not(self):
        """Property 3: the two addressing rules part company on a switch.

        The first test proves both rules with the binding held *still*, which
        cannot tell "physical follows the binding" apart from "physical was
        delivered where it would have gone anyway". This one moves the binding
        underneath both rules: the human's click follows the output to
        `second`, and the agent's click stays with the realm its grant names --
        `realm-0`, which is by then the realm nobody is looking at.

        Counted through the flight recorder rather than read off the apps'
        pixels, because `realm-0` is clicked twice here and its app latches to
        the hit colour on the first one; after that, colour cannot tell a
        second delivery from the first. `seat_delivered` names the realm and
        the origin and deliberately nothing else, which is exactly the pair
        under test.
        """
        core = self._core()
        self._two_live_apps(core)
        phys = core.physical
        phys.await_banner()

        conn = core.connect()
        # One `layout.focus` holder, over the realm that is NOT on screen --
        # it is what moves the binding. Only one grant here carries a layout
        # verb, so D-018(4)'s single-holder rule never comes into it.
        holder = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "layout.focus"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()
        # The agent's grant names `realm-0` and never changes. That is the
        # point: its addressing must not care what the output is showing.
        actor = conn.request_grant(
            realm="realm-0",
            verbs=("observe", "actuate.pointer"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()

        # Both apps are the same program at the same view size, so one
        # location serves both realms.
        cx, cy = self._locate_target(actor)

        def buttons():
            return [
                (e["realm"], e["origin"])
                for e in core.entries()
                if e["kind"] == "seat_delivered" and e["event"] == "button"
            ]

        # A click is a press AND a release, so each one lands more than a
        # single `button` delivery. What is under test is *which realm and
        # which origin* each click produced, never how many events a click
        # decomposes into — so every assertion below reads the delta this
        # click added and asserts on its distinct pairs.
        def click_and_collect(do_click, what):
            # The baseline is taken BEFORE the click is issued. Taking it
            # after is a race the fast path loses: the deliveries can already
            # have landed, the baseline counts them, and the delta reads
            # empty -- which looks exactly like "input was never routed".
            before = len(buttons())
            do_click()
            deadline = time.monotonic() + 15.0
            while time.monotonic() < deadline and len(buttons()) <= before:
                time.sleep(0.05)
            delta = buttons()[before:]
            self.assertTrue(delta, f"{what}: no button delivery ever arrived")
            return set(delta)

        # (1) Before the switch, the human's click lands in `realm-0` -- the
        # realm the output is bound to. Fixture check: without this the
        # divergence below has no "before" to diverge from.
        self.assertEqual(
            click_and_collect(lambda: phys.click(cx, cy), "the human's first click"),
            {("realm-0", "physical")},
            "fixture check: the human's click must start out in the realm on screen",
        )

        # (2) The binding moves to `second` -- but not until the human's hand
        # goes quiet. The click above is physical activity in `realm-0`, and
        # `layout.focus` is attention-shaped, so the chokepoint refuses it
        # `preempted` for `PHYSICAL_HOLD_WINDOW` = 500ms afterwards
        # (`enforcement.rs` step 5c, judged against `env.physical_realm`).
        # Focus inside that window and the request is refused, the binding
        # never moves, and step (3) below reads a click still landing in
        # `realm-0` -- which is indistinguishable from the routing bug this
        # test exists to catch. A click leaves no held button behind (it is a
        # press AND a release), so only the activity window has to lapse.
        time.sleep(0.7)
        holder.focus()
        conn.sync()

        # (3) THE HUMAN FOLLOWS. Same coordinates, different realm now.
        self.assertEqual(
            click_and_collect(lambda: phys.click(cx, cy), "the human's click after the switch"),
            {(SECOND, "physical")},
            "the human's click did not follow the output: after a layout.focus moves the "
            "binding, physical input must be delivered to the realm now on screen. Landing "
            "in realm-0 again means physical input is pinned to whichever realm was bound "
            "first, which is the bug this issue exists to fix.",
        )

        # (4) THE AGENT DOES NOT. Its grant still names `realm-0`, which is
        # now hidden. Same coordinates, and the delivery must not follow the
        # output the way the human's just did.
        def agent_click():
            actor.pointer.click(cx, cy)
            conn.sync(actor)

        self.assertEqual(
            click_and_collect(agent_click, "the agent's click after the switch"),
            {("realm-0", "emulated")},
            "the agent's actuation followed the OUTPUT instead of its own grant: a grant "
            "over realm-0 must reach realm-0 whether or not the human is looking at it. "
            "Landing in 'second' would mean a layout.focus holder can silently redirect "
            "another principal's actuations into an app it never had a grant over.",
        )

        conn.close()
        core.terminate()
        print(
            "\n[input-switch] after a layout.focus to 'second': the human's click followed "
            "to 'second' and the agent's grant over realm-0 still reached realm-0"
        )


def _verb_names(bits) -> set[str]:
    """The dotted names in an effective verb bitfield."""
    from vitrin_os import protocol

    return {name for name, bit in protocol.VERB_BY_DOTTED_NAME.items() if int(bits) & int(bit)}
