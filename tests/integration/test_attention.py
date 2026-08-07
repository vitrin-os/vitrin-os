# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.7 (#232): **the human's attention key lifts `preempted` for exactly
one layout use** — against the shipped `vitrind`, a real C shim, two real
Wayland clients, over a real socket.

This is the mock-free half of issue #232. The in-crate tests
(`attention.rs`'s `an_emulated_super_opens_no_window_and_is_delivered_untouched`
and `the_window_is_claimed_exactly_once`, `session.rs`'s
`the_humans_attention_key_lifts_preemption_for_exactly_one_layout_use` and
`a_prompt_is_never_lifted_by_the_attention_key`, `deadman.rs`'s
`an_attention_press_in_the_same_round_changes_nothing_here`) pin the mechanism
in process; this drives the shipped binary and asks the apps and the
`--capture-dump` what actually happened.

## The loop this closes

A human at a shell running *inside* a realm cannot ask it to change the layout.
They type `focus editor`, press Enter, and that Enter marks `PhysicalPresence`
in the realm they are in — so `enforcement.rs` step 5c refuses the very request
the Enter just sent, `preempted`. Pressing Enter again re-arms the window, so it
is a deterministic loop rather than a race.

## What is asserted, and what each assertion is evidence *from*

1. **Without the key, a layout use under the human's hand is refused
   `preempted`.** From the client's typed exception at the sync barrier, and
   cross-checked against the flight recorder's `use_decision`.
2. **The `attention` event reaches a layout holder, exactly once**, and the
   journal records the press with `opened` and a `notified` count.
3. **Inside the window the same `focus()` is admitted** — and that is confirmed
   by **`--capture-dump`**, never by the client's own belief about what it did:
   a physical click after the switch flips the app in the realm the output moved
   *to*, read back out of that realm's own core-internal dump, while the realm
   left behind is untouched.
4. **Exactly one.** A second layout use, with the human's hand still on the
   keyboard, is refused `preempted` again.
5. **A press that reaches no layout holder tells nobody and opens nothing.** An
   unconditional event would be a free keystroke-timing oracle for every
   connected client — the same hazard consuming the key closes on the app side —
   so a connection with real, live `observe` authority and no layout verb hears
   silence, and the journal records `opened: false`.

   The filter is per **principal**, not per connection: it asks the grant table
   whether *this identity* holds a live layout grant. A principal with two
   connections is therefore told on both, which is why the negative half here is
   proved before any layout grant exists rather than by a second connection of
   the same identity. This harness's registry holds exactly one principal by
   construction (R6 forbids auto-approve against any other), so a
   different-identity case is not expressible here; `session.rs`'s
   `a_client_holding_no_layout_verb_is_never_told_the_human_pressed_the_key`
   covers it in process, with two real identities.

## Why a test injector exists at all

Identical to `test_input_switch.py`'s argument, and it is issue #232's own
"deliberately not a criterion": CI cannot produce physical input.
`SeatInput::physical` is private to `crate::input`, `intake_physical` is the
nested backend's only production caller, headless has no input device, and
headless is the only backend CI runs (D-019(4)). So the `attention` line on the
`physical-input-injector` channel presses **this run's configured chord**
through the same `input::physical_key` the nested backend's keyboard handler
calls — never a second path, and never a scancode the harness guessed.

What that build weakens is named where the claim is made
(`crates/vitrin-core/src/input/mod.rs`'s module docs): in a shipping build
"nothing but a human's device mints a physical tag" is a compile-time
guarantee, and in this one it is a runtime guarantee.

**That a real Super press on real hardware produces any of this is not asserted
here and cannot be.** It is step 10 of `shim/docs/nested-multi-realm.md`, a
nested manual runbook step, exactly as issue #232 asks.

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
    locate_colour,
)

import vitrin_os  # noqa: E402
from vitrin_os import errors  # noqa: E402

#: Matches the rest of the real-app ladder.
REALM_SIZE = "640x480"

#: `click-target`'s own colours (see `shim/tests/click_target.c`).
TARGET = "00ff00"
HIT = "ff0000"
MIN_TARGET_PIXELS = 5000

#: `realm-0` sorts first, so it is the realm the output binds to at startup;
#: `second` is the realm the attention key has to let the holder reach.
SECOND = "second"

RUN_MS = "60000"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


class RealAttentionKey(IntegrationTest):
    """The human's attention key, end to end, against two real apps."""

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
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-attention-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    # -- fixtures ---------------------------------------------------------

    def _core(self):
        self.dump_base = self.work / "internal.rgba"
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            realms=(SECOND,),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            physical_input=True,
            capture_dump=self.dump_base,
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

    def _dump_colour(self, realm: str, timeout=15.0) -> str:
        """The dominant colour of `realm`'s **core-internal** capture dump.

        Deliberately not `grant.observe()`: the whole point of reading a
        `--capture-dump` here is that it bypasses the client's own grant, and
        with it the client's own belief about what it did.

        The histogram is written out here rather than borrowed from the
        harness because `--capture-dump` writes **RGBA8888** while a captured
        frame is xrgb8888 (`B,G,R,X`); running the frame analysis over these
        bytes would silently transpose red and blue, which is exactly the pair
        this test distinguishes.
        """
        path = capture_dump_path(self.dump_base, realm)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                raw = pathlib.Path(path).read_bytes()
            except OSError:
                time.sleep(0.05)
                continue
            if len(raw) == 640 * 480 * 4:
                counts: dict[tuple[int, int, int], int] = {}
                for i in range(0, len(raw), 4):
                    key = (raw[i] & 0xF0, raw[i + 1] & 0xF0, raw[i + 2] & 0xF0)
                    counts[key] = counts.get(key, 0) + 1
                (r, g, b), _ = max(counts.items(), key=lambda kv: kv[1])
                return "".join(f"{c | (c >> 4):02x}" for c in (r, g, b))
            time.sleep(0.05)
        self.fail(f"{path} never became a full 640x480 RGBA dump")

    def _await_dump_colour(self, realm: str, want: str, timeout=15.0) -> str:
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            last = self._dump_colour(realm)
            if last == want:
                return last
            time.sleep(0.1)
        return last

    def _entries(self, core, kind):
        return [e for e in core.entries() if e["kind"] == kind]

    # -- the gate ---------------------------------------------------------

    def test_the_attention_key_admits_one_layout_use_and_nobody_elses(self):
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

        # (0) A PRESS THAT REACHES NOBODY. **Before any layout grant exists
        # anywhere in the session**, this connection holds real, live `observe`
        # authority and nothing else. If the delivery filter were "any bound
        # connection" it would be told the human pressed the key -- a free
        # keystroke-timing oracle for every client in the session. The
        # ordering is load-bearing: the filter is per PRINCIPAL, and this
        # harness's registry holds exactly one (R6), so the *only* way to
        # express "this identity holds no layout verb" is to ask before one is
        # granted.
        watcher_conn = core.connect()
        watcher = watcher_conn.request_grant(
            realm="realm-0",
            verbs=("observe",),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()
        cx, cy = self._locate_target(watcher)
        phys.attention()
        watcher_conn.sync(watcher)
        self.assertEqual(
            watcher_conn.attention_count,
            0,
            "a client holding no layout verb must never learn the human pressed the key",
        )
        silent = self._entries(core, "attention_pressed")
        self.assertEqual(len(silent), 1, f"the press is journaled even so; got {silent}")
        self.assertEqual(silent[0]["notified"], 0)
        self.assertFalse(
            silent[0]["opened"],
            "a window no principal may claim is not open, and recording it as one would "
            "overstate what the human's gesture did",
        )

        conn = core.connect()
        # The holder: `layout.focus` over the HIDDEN realm, which is the realm
        # the human wants to switch to. Plus `observe` on it, so the test can
        # see that app at all -- and `actuate.pointer` on neither realm,
        # because every click below must be the human's.
        holder = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "layout.focus", "layout.arrange"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()
        # Both apps are the same program at the same view size, so the location
        # found above serves both; asked here only to be sure `second`'s app
        # has painted before the switch.
        self._locate_target(holder)
        # `click-target` paints a small green square on a black field, so the
        # DOMINANT colour is the background until a click flips the whole
        # surface red. "Unflipped" is therefore "not HIT", not "TARGET".
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "fixture: realm-0's app must be unflipped before any click",
        )
        self.assertNotEqual(
            self._dump_colour(SECOND), HIT, "fixture: so must second's"
        )

        # (1) THE HUMAN TYPES INTO THE REALM ON SCREEN, and the layout request
        # that typing was for is refused. A key rather than a click, so nothing
        # is flipped and step (3)'s evidence stays unambiguous. `KEY_LEFTCTRL`
        # is in the core's layout-invariant table, which is the whole set a
        # headless core can type.
        phys.key(phys.KEY_LEFTCTRL, True)
        phys.key(phys.KEY_LEFTCTRL, False)
        holder.focus()
        with self.assertRaises(errors.Preempted):
            conn.sync(holder)
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "fixture: the refused focus must have moved nothing",
        )

        # (2) THE HUMAN PRESSES THE ATTENTION KEY. Through the same
        # `physical_key` intake the `key` line above used, on the run's
        # configured chord -- not a scancode this test guessed.
        phys.attention()

        # (3) THE SAME `focus()` IS ADMITTED. Sent immediately, with nothing
        # between it and the press: the window is one second, and an honest
        # client sends the request it had already staged rather than starting
        # work on the event.
        holder.focus()
        conn.sync(holder)

        pressed = self._entries(core, "attention_pressed")
        self.assertEqual(len(pressed), 2, f"two presses, two journal entries; got {pressed}")
        self.assertEqual(pressed[1]["chord"], "super")
        self.assertTrue(
            pressed[1]["opened"],
            "a layout holder is now connected, so this press must have opened a window -- "
            f"unlike the one in step (0). Got {pressed[1]}",
        )
        self.assertGreaterEqual(
            pressed[1]["notified"],
            1,
            "at least the connection holding a layout verb must have been notified; the "
            "filter is per PRINCIPAL, so this identity's other connection is told too "
            f"(see the module docstring). Got {pressed[1]}",
        )

        # ...confirmed by `--capture-dump`, never by the client's own belief
        # about what it did. The proof that the binding really moved is that
        # the HUMAN'S next click lands in the realm the output moved to:
        # physical input follows the binding (WS-E.1.6), so `second`'s app
        # flips and `realm-0`'s does not.
        phys.click(cx, cy)
        conn.sync()
        self.assertEqual(
            self._await_dump_colour(SECOND, HIT),
            HIT,
            "after an admitted focus the human's own click must reach the realm the output "
            "moved to. Read out of that realm's own core-internal capture dump, so this is "
            "the core's pixels rather than the client's account of them",
        )
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "and the realm left behind must be untouched: a flip here means the binding "
            "never moved and the human's click landed where it always did",
        )

        claimed = self._entries(core, "attention_claimed")
        self.assertEqual(len(claimed), 1, f"one press, at most one claim; got {claimed}")

        # (4) EXACTLY ONE. The human is now typing in the realm they moved to,
        # so a second layout use meets the same gate -- with the window spent.
        phys.key(phys.KEY_LEFTCTRL, True)
        phys.key(phys.KEY_LEFTCTRL, False)
        holder.set_fullscreen(False)
        with self.assertRaises(errors.Preempted):
            conn.sync(holder)
        self.assertEqual(
            len(self._entries(core, "attention_claimed")),
            1,
            "one press admits at most ONE layout use: it cannot authorise a burst, and a "
            "holder that could focus, then fullscreen, then focus again would be spending "
            "one human gesture several times",
        )

        # (5) ...and the layout holder really was told, so step (0)'s silence
        # is silence about a mechanism that works rather than about a press
        # that never happened.
        self.assertGreaterEqual(
            conn.attention_count,
            1,
            "the layout holder must have been told the human pressed the key",
        )

        core.terminate()
        out = core.output()
        hits = [ln for ln in out.splitlines() if ln.startswith("HIT ")]
        self.assertEqual(
            len(hits),
            1,
            f"exactly one click-target must report a HIT -- the human's one click, in the "
            f"realm the attention key let them reach. Got {hits}.",
        )

        print(
            "\n[attention] a hand on the keyboard refused the switch; one Super tap "
            f"admitted exactly one, and the human's next click landed in {SECOND!r}"
        )


if __name__ == "__main__":
    import unittest

    unittest.main()
