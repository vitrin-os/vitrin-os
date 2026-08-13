# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.5 (#211): **the switcher and launcher is an ordinary client** — the
shipped `vitrind`, a real C shim, real Wayland apps, and
`examples/shell/run_shell.py` as a separate host-side process on the other end
of a real socket.

This is the mock-free half of issue #211, and it is a property gate rather than
a milestone gate for the reason `test_layout.py` and `test_attention.py` are:
it closes a gap this repo publishes (`docs/book/src/limits.md`) rather than
adding to a closed milestone.

## The claim, and why it needs a gate at all

PRD §5.1 makes "window-management policy lives outside the core" a permanent
invariant, and D-021(4) records that a switcher *inside* the compositor was the
cheaper path and was refused. A sentence in a document cannot tell those two
worlds apart. What can is this: the program that moves the output is a separate
process, holding grants a human approved, that can be `SIGKILL`ed — and when it
is, the core keeps compositing and the last realm it bound keeps receiving the
human's input, because the *binding* is core state and the *policy* was never
in the core to begin with.

## What is asserted, and what each assertion is evidence from

`RealShellSwitchesRealms`:

1. **The shell launches real apps into new realms.** Two `realm.launch` grants,
   one per template (#211 decision 4), two `launch` calls, two realm ids the
   *core* minted — and two more real C shims with two more real `click-target`s
   under them, read from procfs ancestry rather than from the shell's stdout.
2. **Each focus is confirmed by `--capture-dump`, never by the shell's own
   belief about what it did.** After `focus <realm>`, the human's next physical
   click flips *that* realm's app, read out of that realm's own core-internal
   dump, while the realms left behind stay unflipped. Physical input follows the
   binding (WS-E.1.6), so this is the binding's own witness.
3. **A launch confers nothing over what it launched.** The shell has to raise a
   *second* petition, over the id the core just minted, before it can focus it —
   visible in the journal as a `petition_requested` naming that id.

`RealShellDeathAndDenial`:

4. **Killing the shell leaves both realms running and the last-bound realm
   still receiving input.** `SIGKILL`, then ancestry, then a physical click that
   still lands in the realm the dead shell had bound. This is D-021's own stated
   cost asserted as behaviour: what a human loses is the ability to *re-aim*,
   not the session.
5. **A restarted shell re-petitions and fresh prompts appear.** The consent
   channel's `raised` edges after the restart are new petition ids, and the
   journal carries a second `petition_requested` per realm.
6. **A denied `layout.focus` leaves `focus` raising the typed `NotGranted` and
   the bound realm unchanged.** The shell sends the request anyway — it does not
   answer for the core (`examples/shell/README.md`, rule 1) — so what comes back
   is the grant table's `refused(layout.focus, not_granted)`, and the consent
   channel's own `band` reply still names the realm that was bound before.

## What this gate deliberately does not assert

- **That a human sees any of it.** Runners have no display and headless is the
  only backend CI runs (D-019(4)); `shim/docs/nested-multi-realm.md` carries the
  nested runbook step.
- **That the shell is pleasant to use, or that a hotkey works.** #211 says in as
  many words that neither is a criterion: the first is not measurable and the
  second does not exist, because there is no `observe_input` verb and none is
  designed.
- **`preempted` handling end to end.** The shell is host-side, so its keystrokes
  never reach `vitrind` and it cannot reach the collision #232 was built for.
  That mechanism has its own mock-free gate (`test_attention.py`). This gate
  therefore checks **nothing** about `preempted`, and says so rather than
  implying otherwise: that the client has no retry timer is a fact about
  reading `run_shell.py`, not something any assertion here observes, and a
  gate table is the wrong place to launder code inspection into evidence.

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`), and the same
`physical-input-injector` / `consent-injector` argument as `test_attention.py`
and `test_real_consent.py`: CI has no input device and no human to click a card.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "sdk", "python", "src"))

from harness import (  # noqa: E402
    DEMO_IDENTITY,
    DEMO_TOKEN,
    REPO,
    IntegrationTest,
    capture_dump_path,
    children_of,
    comm_of,
    shims_of,
)

#: The shell under test. A *file*, run as its own process: the whole claim is
#: that this is a separate program and not core code, so importing it into the
#: test would quietly weaken the thing being proved.
SHELL = REPO / "examples" / "shell" / "run_shell.py"

REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: `click-target`'s own colours (`shim/tests/click_target.c`). It paints a
#: 160x160 green square on a black field and flips its WHOLE surface red, once
#: and irreversibly, on the first click that lands INSIDE the square. One-way
#: is why every realm below gets exactly one click: a second would prove
#: nothing. The square is centred, so at a 640x480 realm view its centre is
#: (320, 240) — the coordinate every physical click here uses.
TARGET = "00ff00"
HIT = "ff0000"
CLICK_AT = (320, 240)
#: A fifth of the square. Enough that a partly-composited frame does not pass,
#: small enough that the exact edge rounding is not this gate's business.
MIN_TARGET_PIXELS = 5000

#: `realm-0` sorts first, so it is the realm the output binds to at startup.
SECOND = "second"

RUN_MS = "70000"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: `crates/vitrin-core/src/input/mod.rs`'s `PHYSICAL_HOLD_WINDOW`, plus slack.
#: The tests below lapse it explicitly before every layout request that follows
#: physical input, and that placement is load-bearing evidence in itself: the
#: waiting is the TEST's, never the shell's. A client that slept and re-sent
#: would pass these assertions while breaking #211's decision 5, so the absence
#: of a timer in `run_shell.py` is what this constant's existence here records.
PHYSICAL_HOLD_LAPSE_S = 0.7


class ShellProcess:
    """`examples/shell/run_shell.py` as a subprocess, driven over pipes.

    The shell's output contract (`examples/shell/README.md`) is: explanatory
    lines indented two spaces, then exactly one status line matching
    ``^(ok|refused|error) ``; startup ends with a single ``ready`` line. This
    reads to the status line and never parses an explanation, so a reworded
    message is not a test failure and a changed *outcome* is.
    """

    STATUS_PREFIXES = ("ok ", "refused ", "error ", "ready ")

    def __init__(self, test: IntegrationTest, socket: str, argv: list[str]) -> None:
        self.test = test
        # `-u`: this shell flushes every print, but the interpreter's own
        # buffering is not its to fix and a block-buffered pipe would deadlock
        # the ping-pong below rather than fail.
        self.proc = subprocess.Popen(
            [sys.executable, "-u", str(SHELL), "--socket", socket,
             "--identity", DEMO_IDENTITY, "--token", DEMO_TOKEN, *argv],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env={**os.environ, "PYTHONPATH": str(REPO / "sdk" / "python" / "src")},
        )
        self.lines: list[str] = []
        test.addCleanup(self.kill)

    def readline(self) -> str:
        assert self.proc.stdout is not None
        line = self.proc.stdout.readline()
        if not line:
            raise AssertionError(
                "the shell's stdout ended before it said anything more. It died; its "
                f"output so far was:\n" + "\n".join(self.lines)
            )
        self.lines.append(line.rstrip("\n"))
        return line.rstrip("\n")

    def read_to_status(self) -> str:
        """Read explanations until the one status line, and return it."""
        while True:
            line = self.readline()
            if line.startswith(self.STATUS_PREFIXES):
                return line

    def send(self, line: str) -> str:
        assert self.proc.stdin is not None
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        return self.read_to_status()

    def start_plain(self) -> str:
        """Read startup through to `ready` (auto-approve: no cards to answer)."""
        return self.read_to_status()

    def start_answering(self, injector, decisions: dict[str, str]) -> list[tuple[str, str]]:
        """Read startup, answering each card as it is raised.

        The shell announces every petition *before* it sends it, precisely so a
        caller can be told which prompt is about to go up while the request is
        still blocked on an unbounded human delay. `decisions` is keyed by that
        announcement's tail (``"layout.focus second"``); anything not named is
        allowed at the rung the shell asked for. Returns the (petition_id,
        token) pairs seen, so a caller can assert a restart really did raise
        NEW petitions.

        The ack is asserted rather than discarded. The card's buttons are
        ``allow-once`` / ``allow-while-running`` / ``deny``, and a word that is
        none of them is answered ``no-such-button`` and leaves the card up —
        which presents as this method hanging until the channel gives up thirty
        seconds later, somewhere else entirely.
        """
        seen: list[tuple[str, str]] = []
        while True:
            line = self.readline()
            stripped = line.strip()
            if line.startswith(self.STATUS_PREFIXES):
                return seen
            if stripped.startswith("petition "):
                key = stripped[len("petition "):]
                petition, token = injector.await_raised()
                seen.append((petition, token))
                choice = decisions.get(key, "allow-while-running")
                ack = injector.decide(token, choice)
                self.test.assertEqual(
                    ack, "queued", f"the card refused the button {choice!r}: {ack}"
                )
                injector.await_lowered(petition)

    def kill(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(timeout=10)
        # Closed explicitly rather than left to the collector: a ResourceWarning
        # per pipe per shell is four lines of noise in a CI log whose whole job
        # is making an unexpected line visible.
        for pipe in (self.proc.stdin, self.proc.stdout):
            if pipe is not None and not pipe.closed:
                pipe.close()

    def dump(self) -> str:
        return "\n".join(self.lines)


class _ShellGate(IntegrationTest):
    """Shared fixture: the real-app ladder's env contract and a work dir."""

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
        if not SHELL.is_file():
            self.fail(
                f"{SHELL} is missing. This gate exists to drive that program; without it "
                "the suite would collect a green module proving nothing (issue #229)."
            )
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-shell-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.dump_base = self.work / "internal.rgba"

    # -- shared readers ----------------------------------------------------

    def _await_shims(self, core, count: int, timeout=30.0) -> list[int]:
        deadline = time.monotonic() + timeout
        shims: list[int] = []
        while time.monotonic() < deadline:
            shims = shims_of(core.pid)
            if len(shims) >= count:
                return shims
            time.sleep(0.05)
        self.fail(f"expected {count} C shims under the core; got {shims}")
        raise AssertionError("unreachable")  # pragma: no cover

    def _await_apps(self, core, count: int, timeout=30.0) -> list[int]:
        deadline = time.monotonic() + timeout
        apps: list[int] = []
        while time.monotonic() < deadline:
            apps = [
                kid
                for shim in shims_of(core.pid)
                for kid in children_of(shim)
                if comm_of(kid).startswith("click-target")
            ]
            if len(apps) >= count:
                return apps
            time.sleep(0.05)
        self.fail(f"expected {count} real click-targets under the shims; got {apps}")
        raise AssertionError("unreachable")  # pragma: no cover

    def _dump_hist(self, realm: str, timeout=20.0) -> dict[str, int]:
        """A 4-bit-per-channel histogram of `realm`'s **core-internal** dump.

        Bypasses every grant on purpose: the point of reading `--capture-dump`
        is that it is the core's own readback rather than a client's account of
        what it did — and this shell holds no `observe` grant at all, because a
        switcher that could read your screen would be asking for authority its
        job does not need.

        Written out here rather than borrowed from the harness's frame helpers
        because a dump is RGBA8888 and a captured frame is xrgb8888
        (`B,G,R,X`); running the frame analysis over these bytes would
        transpose red and blue, which is exactly the pair this gate
        distinguishes.
        """
        path = capture_dump_path(self.dump_base, realm)
        deadline = time.monotonic() + timeout
        expect = REALM_WH[0] * REALM_WH[1] * 4
        while time.monotonic() < deadline:
            try:
                raw = pathlib.Path(path).read_bytes()
            except OSError:
                time.sleep(0.05)
                continue
            if len(raw) == expect:
                counts: dict[str, int] = {}
                for i in range(0, len(raw), 4):
                    key = "".join(
                        f"{(raw[i + n] & 0xF0) | (raw[i + n] >> 4):02x}" for n in (0, 1, 2)
                    )
                    counts[key] = counts.get(key, 0) + 1
                return counts
            time.sleep(0.05)
        self.fail(f"{path} never became a full {REALM_WH[0]}x{REALM_WH[1]} RGBA dump")
        raise AssertionError("unreachable")  # pragma: no cover

    def _dump_colour(self, realm: str) -> str:
        hist = self._dump_hist(realm)
        return max(hist.items(), key=lambda kv: kv[1])[0]

    def _await_dump_colour(self, realm: str, want: str, timeout=20.0) -> str:
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            last = self._dump_colour(realm)
            if last == want:
                return last
            time.sleep(0.1)
        return last

    def _await_target(self, realm: str, timeout=30.0) -> None:
        """Block until `realm`'s app has painted its clickable target.

        `click-target` paints a black field with one 160x160 green square in
        the middle, so the *dominant* colour is black both before it has
        mapped and after — "the dump is not black" would be satisfied by an
        unmapped realm. The green square is the only thing that distinguishes
        them, and it is also the only place a click does anything, so this is
        the readiness check that matches what the click below needs.
        """
        deadline = time.monotonic() + timeout
        best = 0
        while time.monotonic() < deadline:
            best = max(best, self._dump_hist(realm).get(TARGET, 0))
            if best >= MIN_TARGET_PIXELS:
                return
            time.sleep(0.1)
        self.fail(
            f"{realm}'s app never painted a locatable target ({best} px of {TARGET}); "
            "a click there would prove nothing"
        )

    def _shell(self, core, argv: list[str]) -> ShellProcess:
        return ShellProcess(self, str(core.socket), argv)

    def _entries(self, core, kind):
        return [e for e in core.entries() if e["kind"] == kind]


class RealShellSwitchesRealms(_ShellGate):
    """The shell launches real apps and moves the output to each in turn."""

    #: Two templates: declared in `realm.toml`, `autostart = false`, so nothing
    #: runs them until the shell's `realm.launch` grant asks. `realm-0`
    #: autostarts because the core refuses a file in which every realm is a
    #: template — a session of nothing but templates has nothing on screen.
    LEFT = "left"
    RIGHT = "right"

    def test_the_shell_launches_two_realms_and_focuses_each_in_turn(self):
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            templates=(self.LEFT, self.RIGHT),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            physical_input=True,
            capture_dump=self.dump_base,
            log_file=str(self.work / "core.log"),
        )
        phys = core.physical
        self.assertIsNotNone(phys, "the harness must have wired --physical-input-fd")
        self.assertEqual(
            phys.await_banner(),
            "vitrin-physical-input 1",
            "the core must have ADOPTED the physical channel, not merely been handed a "
            "socket. Rebuild with `--features vitrin-core/physical-input-injector` "
            "(tests/integration/run.sh does).",
        )
        # One autostarted realm, and two templates that have not run.
        self._await_apps(core, 1)

        shell = self._shell(
            core,
            ["--realm", "realm-0", "--template", self.LEFT, "--template", self.RIGHT],
        )
        ready = shell.start_plain()
        self.assertTrue(
            ready.startswith("ready "),
            f"the shell must reach `ready`; it said {ready!r}\n{shell.dump()}",
        )

        # The two commands that send nothing and the one that sends
        # `set_fullscreen`, exercised here — before any physical input, so no
        # hold window is open — because an untested command path in a
        # five-command program is most of the program. `fullscreen` is asserted
        # only to be SERVED rather than refused: the two modes are
        # indistinguishable while the output and the realm are the same size,
        # which every headless run is, and the IDL says so in as many words.
        self.assertTrue(shell.send("list").startswith("ok list "), shell.dump())
        self.assertEqual(shell.send("fullscreen on"), "ok fullscreen on", shell.dump())
        self.assertEqual(shell.send("fullscreen off"), "ok fullscreen off", shell.dump())
        # ...and the journal is what makes those two more than "no exception
        # was raised": the shell prints `ok` when nothing came back, so a
        # `set_fullscreen` that was never sent would look identical from here.
        # `layout_arrange` reaching the enforcement chokepoint twice, allowed,
        # is the core's own record that the requests arrived.
        arranged = [
            e
            for e in self._entries(core, "use_decision")
            if e.get("verb") == "layout_arrange"
        ]
        self.assertEqual(
            [e.get("decision") for e in arranged],
            ["allowed", "allowed"],
            "two set_fullscreen requests must reach the chokepoint and be allowed; "
            f"the journal saw {arranged}",
        )
        self.assertEqual(
            shell.send("focus nonesuch"),
            "error focus nonesuch unknown-realm",
            "a name the shell was never told about must be its own error, not a "
            f"petition: the wire has no realm enumeration to check it against\n{shell.dump()}",
        )

        # (1) TWO LAUNCHES, TWO CORE-MINTED IDS, TWO REAL PROCESSES.
        status = shell.send(f"launch {self.LEFT}")
        self.assertTrue(
            status.startswith(f"ok launch {self.LEFT} "),
            f"launch {self.LEFT} did not succeed: {status!r}\n{shell.dump()}",
        )
        left_realm = status.split()[-1]
        status = shell.send(f"launch {self.RIGHT}")
        self.assertTrue(
            status.startswith(f"ok launch {self.RIGHT} "),
            f"launch {self.RIGHT} did not succeed: {status!r}\n{shell.dump()}",
        )
        right_realm = status.split()[-1]
        self.assertNotEqual(left_realm, right_realm)
        # The ids came from the core, not from the shell: nothing in
        # run_shell.py constructs a realm id, and the journal is where the
        # spawn is recorded with the principal that asked for it.
        spawned = [e["realm"] for e in self._entries(core, "realm_spawned")]
        self.assertIn(left_realm, spawned, f"realm_spawned must name it; got {spawned}")
        self.assertIn(right_realm, spawned, f"realm_spawned must name it; got {spawned}")

        self._await_shims(core, 3)
        self._await_apps(core, 3)

        # (3) THE SHELL PETITIONS AGAIN OVER A LAUNCHED ID -- which is a fact
        # about the shell, not proof of a core property. "A launch confers
        # nothing" is the core's rule and this gate does not observe it: it
        # never tries to use a launch grant for anything else and watch it be
        # refused. What it does pin is that the shell asks rather than assuming,
        # which is the client-side half and the half this issue owns. The core
        # half belongs to #207's own gate.
        requested = [e["realm_name"] for e in self._entries(core, "petition_requested")]
        self.assertIn(
            left_realm,
            requested,
            "focusing a launched realm must cost a SECOND petition over the minted id; "
            f"petitions were over {requested}",
        )
        self.assertIn(right_realm, requested)

        self._await_target(left_realm)
        self._await_target(right_realm)
        for realm in ("realm-0", left_realm, right_realm):
            self.assertNotEqual(
                self._dump_colour(realm), HIT, f"fixture: {realm} must start unflipped"
            )

        # (2) EACH FOCUS CONFIRMED BY --capture-dump. The witness is the
        # human's own click: physical input follows the binding, so a click
        # after `focus X` must flip X's app and nothing else's.
        status = shell.send(f"focus {left_realm}")
        self.assertEqual(status, f"ok focus {left_realm}", shell.dump())
        phys.click(*CLICK_AT)
        self.assertEqual(
            self._await_dump_colour(left_realm, HIT),
            HIT,
            "after `focus <left>` the human's own click must land in that realm, read "
            "out of that realm's own core-internal dump rather than from anything the "
            "shell said about itself",
        )
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "and the realm the output started on must be untouched: a flip here means "
            "the binding never moved",
        )
        self.assertNotEqual(self._dump_colour(right_realm), HIT)

        # The lapse is the TEST's, not the shell's: that click marked the
        # bound realm as the human's for PHYSICAL_HOLD_WINDOW, and a layout
        # request inside it is refused `preempted`. run_shell.py has no timer
        # and does not retry (#211 decision 5), so the waiting has to happen
        # here — which is itself the evidence that it does not happen there.
        time.sleep(PHYSICAL_HOLD_LAPSE_S)

        status = shell.send(f"focus {right_realm}")
        self.assertEqual(status, f"ok focus {right_realm}", shell.dump())
        phys.click(*CLICK_AT)
        self.assertEqual(
            self._await_dump_colour(right_realm, HIT),
            HIT,
            "the second focus must move the output again, confirmed the same way",
        )
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "realm-0 has now been left behind twice and must still be unflipped",
        )

        self.assertEqual(shell.send("quit"), "ok quit", shell.dump())


class RealShellDeathAndDenial(_ShellGate):
    """What a shell crash costs, and what a denial costs — both asserted."""

    def _core(self):
        return self.core(
            size=REALM_SIZE,
            consent="interactive",
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            realms=(SECOND,),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            consent_injector=True,
            physical_input=True,
            capture_dump=self.dump_base,
            log_file=str(self.work / "core.log"),
        )

    def test_killing_the_shell_keeps_the_session_and_a_denial_moves_nothing(self):
        core = self._core()
        phys, cards = core.physical, core.injector
        self.assertIsNotNone(phys)
        self.assertIsNotNone(cards)
        self.assertEqual(phys.await_banner(), "vitrin-physical-input 1")
        self.assertEqual(
            cards.await_banner(),
            "vitrin-consent-injector 1",
            "the core must have ADOPTED the consent channel; a plain build refuses "
            "`--consent-injector-fd` at startup and would never bind its socket",
        )
        self._await_apps(core, 2)
        self._await_target("realm-0")
        self._await_target(SECOND)
        self.assertEqual(
            cards.band()["realm"],
            "realm-0",
            "fixture: realm-0 sorts first, so it is the realm the output binds to",
        )

        # --- the first shell: petitions, prompts, and one real switch -----
        first = self._shell(core, ["--realm", "realm-0", "--realm", SECOND])
        first_cards = first.start_answering(cards, {})
        self.assertEqual(
            len(first_cards),
            3,
            "two realms is two layout.focus petitions plus one layout.arrange — "
            f"arrangement is single-holder per output (D-018(4)). Got {first_cards}\n"
            f"{first.dump()}",
        )
        self.assertEqual(first.lines[-1].split()[0], "ready", first.dump())

        self.assertEqual(first.send(f"focus {SECOND}"), f"ok focus {SECOND}", first.dump())
        self.assertEqual(
            cards.band()["realm"],
            SECOND,
            "the shell's focus must have moved the core's own output binding",
        )

        # --- (4) KILL IT ---------------------------------------------------
        first.proc.send_signal(signal.SIGKILL)
        self.assertEqual(
            first.proc.wait(timeout=10),
            -signal.SIGKILL,
            "the shell must really have died by signal, not exited on its own",
        )
        # The session is unharmed: the realms' shims and apps are children of
        # `vitrind`, never of the shell.
        self.assertEqual(len(self._await_shims(core, 2)), 2)
        self.assertEqual(len(self._await_apps(core, 2)), 2)
        self.assertEqual(
            cards.band()["realm"],
            SECOND,
            "a dead client must not move the output back: the binding is core state, "
            "and nothing revokes it when the principal that set it disappears",
        )
        # ...and the last-bound realm still receives the human's input. This is
        # the half that makes the loss precise: what a crash costs is the
        # ability to RE-AIM the session, not the session.
        phys.click(*CLICK_AT)
        self.assertEqual(
            self._await_dump_colour(SECOND, HIT),
            HIT,
            "after the shell was killed, the realm it last bound must still receive the "
            "human's physical input — read from that realm's own core-internal dump",
        )
        self.assertNotEqual(
            self._dump_colour("realm-0"),
            HIT,
            "and realm-0 must not have quietly taken the output back",
        )

        # --- (5) A RESTARTED SHELL RE-PETITIONS, AND (6) A DENIAL ----------
        before = len(self._entries(core, "petition_requested"))
        time.sleep(PHYSICAL_HOLD_LAPSE_S)  # the click above; see the constant.
        second = self._shell(core, ["--realm", "realm-0", "--realm", SECOND])
        second_cards = second.start_answering(
            cards, {"layout.focus realm-0": "deny"}
        )
        self.assertEqual(len(second_cards), 3, second.dump())
        self.assertFalse(
            set(p for p, _ in second_cards) & set(p for p, _ in first_cards),
            "a restarted shell must raise FRESH petitions, not reuse the dead one's: "
            f"{second_cards} overlaps {first_cards}",
        )
        after = len(self._entries(core, "petition_requested"))
        self.assertGreaterEqual(
            after - before,
            3,
            "the journal must carry the restart's petitions; `while_running` grants do "
            "not outlive the principal that held them and nothing here re-uses one",
        )

        refused_before = [
            e
            for e in self._entries(core, "use_decision")
            if e.get("verb") == "layout_focus" and e.get("decision") == "refused"
        ]
        status = second.send("focus realm-0")
        self.assertEqual(
            status,
            "refused focus realm-0 not_granted",
            "a denied petition must leave `focus` showing the CORE's refusal. The shell "
            "sends the request anyway — it does not answer for the core — so this is the "
            f"grant table's answer, not a cached memory of the prompt.\n{second.dump()}",
        )
        # ...and the journal is what makes that more than a string the shell
        # composed. This is the ONE property the whole deliverable is named for
        # — PRD §5.1's "policy lives outside the core", which only means
        # anything if the client refuses to answer on the core's behalf — and
        # asserting it on the shell's own stdout is exactly the shape that lets
        # a short-circuiting shell pass. The same hazard is already cross-checked
        # for `fullscreen` above; leaving it uncrossed here was asymmetric.
        #
        # A shell that answered locally would put NO byte on the wire, so the
        # chokepoint would see nothing and this delta would be zero.
        refused_after = [
            e
            for e in self._entries(core, "use_decision")
            if e.get("verb") == "layout_focus" and e.get("decision") == "refused"
        ]
        self.assertEqual(
            len(refused_after) - len(refused_before),
            1,
            "the refused `focus` must have REACHED the enforcement chokepoint: a shell "
            "that short-circuited on its own memory of the denied prompt would print the "
            "identical line and send nothing. The core's own record is the only thing "
            f"that can tell the two apart. Journal saw {refused_after}",
        )
        self.assertEqual(
            cards.band()["realm"],
            SECOND,
            "a refused focus must move nothing: the output is still where the DEAD "
            "shell put it, which is the assertion a fail-open enforcement path fails",
        )

        # The complement, so the denial above is not merely a broken shell:
        # the realm whose petition was ALLOWED is still focusable.
        self.assertEqual(second.send(f"focus {SECOND}"), f"ok focus {SECOND}", second.dump())
        self.assertEqual(second.send("quit"), "ok quit", second.dump())
