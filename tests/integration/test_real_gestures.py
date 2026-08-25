# SPDX-License-Identifier: Apache-2.0
"""WS-E.4.2 (#222): **the touchpad classes the seat vocabulary grew — relative
motion, swipe and pinch — reach a real app with their origin intact, and no
app is ever left latched mid-gesture** — over a real socket, through the
shipped `vitrind`, a real `vitrin-shim` and a real Wayland client.

This is issue #222's acceptance criterion 4, the last one it had open. Criteria
1–3 are met in-crate (`crates/vitrin-core/src/input/mod.rs`, notably
`a_realm_switch_mid_gesture_ends_it_cancelled_and_owes_the_app_nothing_after`),
and per `CLAUDE.md` those component tests are **not** substitutable for this
one: they end at the core's own router, and everything after it — the encode,
the wire, the shim's replay, the three Wayland extension globals, and what a
client actually receives — is exactly the half a component test cannot see.

## Why an injector, and what it does NOT stand in for

CI has no touchpad, and neither does the machine this suite runs on when it
runs headless: `SeatInput::physical` is private to `crate::input`, the headless
backend has no input device, and headless is the only backend CI runs
(D-019(4)). The `physical-input-injector` feature is the precedent every
neighbouring gate follows (`test_input_switch.py` #212, `test_attention.py`
#232, `test_real_clipboard.py` #213, `test_screenshot.py` #216): a
socketpair named by `--physical-input-fd N`, whose lines become host events fed
to **`input::intake_physical`** — the nested backend's own entry point, never a
second, weaker path.

**What that cannot cover is libinput's own gesture detection.** A real touchpad
reports finger contacts; libinput is what decides those contacts are a
three-finger swipe or a two-finger pinch, and only then emits the events this
channel injects. That decision sits *upstream* of `intake_physical`, so nothing
reachable from here exercises it, and nothing here should be read as evidence
that a human's three fingers on real hardware produce a `gesture_begin`. That
half is **`docs/drm-bringup.md` step 13a** — a runbook rung against this
laptop's real touchpad, written and **not yet run**, on exactly the terms issue
#212 set for "a human sees it". It is the DRM runbook and not a nested one
because the nested backends never see a gesture at all: the host compositor
consumes it.

## The properties

1. **The classes reach the app, with every quantity where it belongs.** A
   relative delta arrives with its accelerated and unaccelerated halves
   distinct; a pinch update arrives with `scale` — which is absolute since the
   begin, where the three beside it are deltas — carried through rather than
   accumulated. The app is an independent witness on the far side of a real
   Wayland connection, so "what arrived" is its answer and not the core's.
2. **The journal names what was delivered and to which realm**, with
   `origin=physical` — and names `gesture_begin`/`gesture_end` while
   deliberately naming neither `relative_motion` nor the two updates, which
   arrive at raw device rate and are excluded from the recorder for `motion`'s
   own published reason. The journal is written on the *minting* side, so the
   origin tag is read a second time off the **shim's own decode trace**, on the
   far side of the wire. That is not belt-and-braces: the three Wayland
   extensions the app receives on carry no origin argument, so the app cannot
   be the witness for B2's tag and a shim that decoded it into the wrong field
   would otherwise be invisible to everything here.
3. **Begin/end pairing: a dropped or superseded end cannot latch the app.**
   This is issue #222's acceptance criterion 3 and the sharpest thing here. It
   is held twice: an *unpaired* end must reach nobody, and a realm switch
   *mid-gesture* must pay the losing realm's app a `cancelled` end before the
   binding moves — because after it moves, the human's own end is addressed
   somewhere else and the app would otherwise accumulate that pinch forever.
   Only the receiving client can settle it, which is why `gesture-probe`
   reports `in_flight=` at exit rather than the core reporting what it sent.

## Pointer constraints

The third served class is **not** injectable: a lock is *requested by the app*,
so there is no physical event that produces one. `gesture-probe --lock-pointer`
makes the ask a real client makes — `zwp_pointer_constraints_v1.lock_pointer`
on its first `wl_pointer.enter`, `persistent` lifetime — and reports the
`locked`/`unlocked` edges it is answered with. See
`test_a_pointer_lock_is_asked_for_by_the_app_and_answered_by_the_core` for what
that does and does not settle.

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`).
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "sdk", "python", "src"))

from harness import (  # noqa: E402
    IN_REALM_HOME,
    IntegrationTest,
    children_of,
    comm_of,
    shims_of,
    whole_realm_grant,
)

import vitrin_os  # noqa: E402
from vitrin_os import errors  # noqa: E402

#: Matches the rest of the real-app ladder.
REALM_SIZE = "640x480"

#: The centre of that view. `gesture-probe`'s surface is centred by the same
#: `ViewGeometry::place` the router hit-tests with, so the centre is inside it
#: under any surface size the shim configures — and the pointer must be over
#: the surface before a gesture is the app's at all (`InputRouter::route_into`).
#: Since #304 that centring is inside the USABLE rectangle, so the surface's
#: own centre sits `reserved_top() / 2` rows below this point; it stays inside
#: the surface for every size the shim configures, which is the usable view.
CENTRE = (320.0, 240.0)

#: `realm-0` sorts first, so it is the realm the output binds to at startup.
SECOND = "second"

#: Each realm's app is tagged, because both inherit the core's stdout and two
#: untagged `SUMMARY` lines cannot be told apart.
TAGS = {"realm-0": "first", SECOND: "second"}

RUN_MS = "60000"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


class RealGestures(IntegrationTest):
    """The touchpad classes, end to end, with no mock on any seam."""

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
        app = os.environ.get("VITRIN_GESTURE_PROBE_APP")
        if not app:
            sibling = self.shim_bin.resolve().parent / "gesture-probe"
            if sibling.is_file() and os.access(sibling, os.X_OK):
                app = str(sibling)
        if not app:
            self.fail(
                f"no gesture-probe beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration, never a reason to skip."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-gestures-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        # Named here rather than minted by the harness, because `_app_log`
        # has to name a path inside it before the core boots.
        self.rt = self.work / "rt"
        self.rt.mkdir()

    # -- fixtures ---------------------------------------------------------

    #: The app's report, named INSIDE the realm (P2.6.2, #186). Two host paths
    #: are wrong and one is right, and the wrong ones fail in different ways:
    #:
    #:  - the scratch `/tmp/vitrin-gestures-*` this used to pass **cannot be
    #:    created** -- a confined realm's `/tmp` is a private tmpfs -- so the
    #:    probe died at startup and the gate failed with "two realms must be
    #:    two C shims: 0 != 2", which reads like a spawn bug and is not one;
    #:  - the realm's runtime directory (`/run/vitrin`) is writable and
    #:    host-visible, but an orderly shutdown **deletes** it
    #:    (`lifecycle.rs::remove_runtime_dir`), and every assertion here reads
    #:    the report after the run.
    #:
    #: `/vitrin/home` is the realm's persistent private storage: writable,
    #: host-visible at `_app_log` below, and never purged.
    def _in_realm_out(self, tag: str) -> str:
        return f"{IN_REALM_HOME}/app-{tag}.txt"

    def _app_log(self, tag: str) -> pathlib.Path:
        """Where the app tagged `tag` writes its own account of the run.

        A file per app, and **not** the core's inherited stdout, which is what
        every neighbouring gate reads. The stream is shared by the core, both
        shims and both apps, and wlroots' `wlr_log` writes to an *unbuffered*
        stderr — so one shim log line is several `write()` calls and another
        writer can land between two of them. That is not a theory: a run of
        this gate produced

            seat-replay: seq=8 … dx=0.500 dy=-0.500 scaleIN swipe_begin fingers=3
            =0.750 rotation=10.000

        where the app's line is welded to the middle of the shim's with no
        separator. Every assertion here is about *what an app received*, so
        reading them out of a stream that can splice would make this gate fail
        for a reason that has nothing to do with the compositor — and, worse,
        would make a real regression indistinguishable from that noise.
        """
        return (
            self.rt / "data" / "vitrin" / "realms" / self._realm_of(tag) / f"app-{tag}.txt"
        )

    def _realm_of(self, tag: str) -> str:
        """The realm whose app carries `tag` -- `TAGS` read the other way."""
        for rid, value in TAGS.items():
            if value == tag:
                return rid
        raise KeyError(tag)

    def _app_output(self, tag: str) -> str:
        path = self._app_log(tag)
        return path.read_text(errors="replace") if path.exists() else ""

    def _core(self, lock_in: str | None = None):
        """A two-realm session, each realm a real shim running a real probe.

        Two realms even where one would do, because the pairing razor needs a
        realm to switch *to* and a second app to prove was never touched — and
        running the same fixture for both tests means the "nothing reached the
        other realm" claim is made against a realm that genuinely exists.

        `lock_in` names the realm that asks **the way a real app asks** — on
        its first `wl_pointer.enter`. Every *other* realm then asks with
        `--lock-on-map`, and that asymmetry is the point rather than a
        convenience: an app in a realm the output is not bound to never
        receives an `enter` at all, so under `--lock-pointer` it would never
        ask — and "that app was not told `locked`" would be a fact about the
        probe rather than about the core's per-realm scoping. Both ask; only
        one may be answered.
        """
        realm_args = {
            rid: ["--run-ms", RUN_MS, "--tag", tag, "--out", self._in_realm_out(tag)]
            + (
                []
                if lock_in is None
                else ["--lock-pointer"] if rid == lock_in else ["--lock-on-map"]
            )
            for rid, tag in TAGS.items()
        }
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            realm_args=realm_args,
            realms=(SECOND,),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            physical_input=True,
            runtime_dir=str(self.rt),
            log_file=str(self.work / "core.log"),
        )

    def _finish_apps_then_core(self, core, timeout: float = 15.0) -> None:
        """Let both probes exit cleanly, THEN stop the core.

        **The order was the other way round until P2.6.2 (#186), and it stopped
        working the day realms got a pid namespace.** The old comment here read
        "the apps outlive the core by however long it takes them to notice
        their Wayland connection died, and the `SUMMARY` line every assertion
        below reads is printed in that window". That window is now gone *by
        design*: the realm's PID 1 is the shim, and when PID 1 exits the kernel
        `SIGKILL`s every other process in its namespace at once -- which is
        precisely the orphan residual D-037(2) set out to close (before it,
        rung 3 killed the shim and left its app reparented to init). A
        `SIGKILL`ed probe writes no summary, and the gate failed with "no
        SUMMARY line from the 'first' app", which reads like a delivery bug and
        is not one.

        So the probes are asked to finish first. `SIGTERM` is the signal
        `gesture_probe.c` installs a handler for; it leaves the dispatch loop
        and prints `SUMMARY` on the way out. Sent from the host, to the host
        pid -- a pid namespace confines what the realm can address, not what
        its parent can.
        """
        for pid in self.app_pids:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass  # already gone: it hit --run-ms, or the realm died
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not any(os.path.exists(f"/proc/{pid}") for pid in self.app_pids):
                core.terminate()
                return
            time.sleep(0.05)
        core.terminate()
        self.fail(
            f"the probes {self.app_pids} did not exit within {timeout}s of SIGTERM; their "
            "SUMMARY lines cannot be trusted to have been written"
        )

    def _two_live_apps(self, core) -> None:
        deadline = time.monotonic() + 25.0
        shims: list[int] = []
        while time.monotonic() < deadline:
            shims = shims_of(core.pid)
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
                if comm_of(kid).startswith("gesture-probe")
            ]
            if len(apps) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(apps), 2, f"each shim must have fork/exec'd its own gesture-probe; got {apps}"
        )
        # Kept so the log can be read only once these have actually exited.
        self.app_pids = apps

    def _await_surface(self, grant, timeout=25.0) -> None:
        """Block until this realm's app has committed a surface.

        Load-bearing, not a settle-sleep: the router refuses a gesture whose
        realm has no committed surface for the pointer to be over, so injecting
        before the first commit produces a silent nothing that looks exactly
        like a routing bug.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                grant.observe()
                return
            except errors.NoSurface:
                time.sleep(0.05)
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
        self.fail("the app never committed a surface; there is nothing to gesture at")

    # -- reading the app back ---------------------------------------------

    @staticmethod
    def _fields_after(line: str, *marker: str) -> dict[str, str] | None:
        """The `k=v` run that follows `marker` in `line`, or `None`.

        Scanned rather than anchored with `startswith`, because the one place
        this is still used against a *shared* stream is the shim's replay trace
        (`_replays`), where a line can arrive with another writer's output
        welded onto its front. The run stops at the first token that is not
        `k=v`, so a splice on the far side cannot smuggle fields in either.
        """
        tokens = line.split()
        for i in range(len(tokens) - len(marker) + 1):
            if tuple(tokens[i : i + len(marker)]) != marker:
                continue
            fields: dict[str, str] = {}
            for token in tokens[i + len(marker) :]:
                if "=" not in token:
                    break
                key, value = token.split("=", 1)
                fields[key] = value
            return fields
        return None

    def _summaries(self, core) -> dict[str, dict[str, str]]:
        """Every app's `SUMMARY` line, by its `--tag`, as a field map.

        `core` is accepted and unused: it keeps the call sites reading as
        "what the run produced" rather than "what a file says", and it is what
        every failure message tails when a summary is missing.
        """
        found: dict[str, dict[str, str]] = {}
        for tag in TAGS.values():
            for line in self._app_output(tag).splitlines():
                fields = self._fields_after(line, "SUMMARY")
                if fields and "tag" in fields:
                    found[fields["tag"]] = fields
        return found

    def _require_summaries(self, core) -> dict[str, dict[str, str]]:
        got = self._summaries(core)
        for tag in TAGS.values():
            self.assertIn(
                tag,
                got,
                f"no SUMMARY line from the {tag!r} app ({self._app_log(tag)}). Either it "
                f"never ran, or it was killed before it could report — and an assertion "
                f"about what an app received cannot be made from an app that did not "
                f"speak.\n{_tail(core.output())}",
            )
        return got

    def _in_lines(self, core, event: str) -> list[dict[str, str]]:
        """Every `IN <event> ...` line the apps wrote, as field maps.

        Read from the apps' own files (`--out`), in tag order, so "which app
        received this" is a fact about which file it is in rather than about
        the order two processes reached a shared stream.
        """
        out = []
        for tag in TAGS.values():
            for line in self._app_output(tag).splitlines():
                fields = self._fields_after(line, "IN", event)
                if fields is not None:
                    out.append(fields)
        return out

    def _replays(self, core) -> list[dict[str, str]]:
        """Every `seat-replay:` line the C shims traced, as field maps.

        This is the **only** evidence in the system that `origin` survived the
        wire. The core's journal is written on the minting side, and the three
        Wayland extensions the app receives on
        (`zwp_relative_pointer_v1`, `zwp_pointer_gesture_swipe_v1`,
        `zwp_pointer_gesture_pinch_v1`) have **no origin argument at all** — so
        a core that encoded the tag correctly and a shim that decoded it into
        the wrong field would be invisible to both. The shim prints what it
        decoded, and it prints it unconditionally: `main.c` calls
        `wlr_log_init(WLR_DEBUG, NULL)` with no switch, so these lines are not
        contingent on a log level a runner might set differently.

        Read from `all_app_output()` rather than `core.output()` since P2.6.2
        (#186): a confined realm's shim writes to the realm's own log file
        instead of inheriting the core's descriptors, so the core's own stream
        carries none of these lines. Both realms', because which realm decoded
        what is the point of the trace.
        """
        out = []
        for line in core.all_app_output().splitlines():
            fields = self._fields_after(line, "seat-replay:")
            if fields:
                out.append(fields)
        return out

    def _deliveries(self, core, *events: str) -> list[tuple[str, str, str]]:
        """`(realm, event, origin)` for every journaled seat delivery named."""
        return [
            (e["realm"], e["event"], e["origin"])
            for e in core.entries()
            if e["kind"] == "seat_delivered" and e["event"] in events
        ]

    # -- the gate ---------------------------------------------------------

    def test_the_touchpad_classes_reach_the_app_and_the_journal_names_the_realm(self):
        core = self._core()
        self._two_live_apps(core)

        phys = core.physical
        self.assertIsNotNone(
            phys,
            "the harness must have wired --physical-input-fd; without it this gate proves "
            "nothing about physical input",
        )
        self.assertEqual(
            phys.await_banner(),
            "vitrin-physical-input 1",
            "the core must have ADOPTED the channel, not merely been handed a socket. A "
            "plain build does not know `--physical-input-fd` at all and would have exited "
            "at argument parsing; rebuild with "
            "`--features vitrin-core/physical-input-injector` (tests/integration/run.sh does).",
        )

        conn = core.connect()
        watcher = whole_realm_grant(conn, realm="realm-0")
        self._await_surface(watcher)

        # The pointer has to be over the app's surface before any of this is
        # the app's: a gesture over the letterbox matte belongs to nobody, and
        # a delta carries no position of its own so it hit-tests against the
        # stored pointer (`InputRouter::route_into`).
        phys.motion(*CENTRE)

        # (1) Relative motion. Four distinct numbers, so a shim or a core that
        # copied the accelerated delta into the unaccelerated field is a
        # failure here rather than a coincidence.
        phys.relative_motion(3.0, -4.0, 5.0, -6.0)

        # (2) A whole swipe, and (3) a whole pinch that the human CANCELS --
        # both spellings of `gesture_state`, because an inverted polarity is
        # indistinguishable from a correct one at any single call site, and an
        # app that commits a zoom the human abandoned is the visible cost.
        # Three fingers on one and two on the other, because the finger count
        # is what a toolkit dispatches on and a count flattened in transit
        # fires the wrong action.
        phys.gesture_begin("swipe", 3)
        phys.gesture_swipe(7.0, -8.0)
        phys.gesture_end("swipe")
        phys.gesture_begin("pinch", 2)
        # TWO updates inside one pinch, and that is load-bearing rather than
        # thorough: with a single update, a passthrough and a running product
        # seeded at 1.0 produce the identical number, so "carried through, not
        # accumulated" would be a claim no assertion could fail. The second
        # scale is 0.75 -- an accumulator reports 0.938 (1.25 * 0.75) and a
        # differencer 0.6, both distinguishable from the 0.75 that was sent.
        #
        # Every quantity in this file is a multiple of 1/256 on purpose: the
        # last hop is `wl_fixed_t`, which is 24.8 fixed point, so 0.8 would
        # arrive as 0.801 and the assertion would be about rounding rather
        # than about accumulation.
        phys.gesture_pinch(1.0, 2.0, 1.25, -30.0)
        phys.gesture_pinch(0.5, -0.5, 0.75, 10.0)
        phys.gesture_end("pinch", cancelled=True)

        # (4) An UNPAIRED end. The channel accepts it deliberately -- pairing
        # is the router's discipline and the shim's, and a channel that refused
        # one would be a third enforcement point that made this very case
        # untestable. What must happen is that it reaches nobody.
        phys.gesture_end("swipe")

        # (5) The vocabulary is closed, and a malformed line is ANSWERED. The
        # only surface an untrusted peer controls here is the parser, so a gate
        # that never sent it a bad line would leave that surface untested.
        phys.expect_refusal("gesture update swipe 1")
        phys.expect_refusal("gesture begin hold 3")

        # The journal, polled: `gesture_begin` and `gesture_end` are discrete
        # and bounded by the human's fingers, so they ARE journaled.
        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline:
            if len(self._deliveries(core, "gesture_begin", "gesture_end")) >= 4:
                break
            time.sleep(0.05)
        self.assertEqual(
            self._deliveries(core, "gesture_begin", "gesture_end"),
            [
                ("realm-0", "gesture_begin", "physical"),
                ("realm-0", "gesture_end", "physical"),
                ("realm-0", "gesture_begin", "physical"),
                ("realm-0", "gesture_end", "physical"),
            ],
            "the journal must name each gesture edge, the realm whose app received it and "
            "the origin intake bound (B2) — and exactly FOUR of them: the fifth `gesture "
            "end` was unpaired and must have been dropped by the router rather than "
            "delivered to an app that was not gesturing.",
        )
        # ...and deliberately names none of the high-rate events. This is a
        # design decision with a published reason (`record_seat_delivery`): all
        # three arrive at raw device rate with no token bucket in front of
        # them, and `relative_motion` arrives BESIDE the `motion` already
        # excluded, so journaling it would flood the recorder at double the
        # rate that argument rejects.
        self.assertEqual(
            self._deliveries(
                core, "relative_motion", "gesture_swipe_update", "gesture_pinch_update"
            ),
            [],
            "the recorder must not journal the raw-rate events; `record_seat_delivery` "
            "excludes them for `motion`'s own reason and this asserts the exclusion is "
            "real rather than merely documented",
        )

        conn.close()
        self._finish_apps_then_core(core)

        # The app's own account, which is the only one that can settle what
        # ARRIVED as opposed to what was sent.
        summaries = self._require_summaries(core)
        first, other = summaries["first"], summaries["second"]

        self.assertEqual(first["relative"], "1", "the relative delta never reached the app")
        self.assertEqual(
            (first["swipe_begin"], first["swipe_update"], first["swipe_end"]),
            ("1", "1", "1"),
            f"the swipe did not arrive whole: {first}",
        )
        self.assertEqual(
            (first["pinch_begin"], first["pinch_update"], first["pinch_end"]),
            ("1", "2", "1"),
            f"the pinch did not arrive whole: {first}",
        )
        self.assertEqual(
            first["in_flight"],
            "none",
            "the app is still holding a gesture: every begin it was told about must have "
            "been paired by an end it was told about (issue #222's acceptance criterion 3)",
        )
        self.assertEqual(
            first["unpaired"],
            "0",
            "the app received an event it could not pair — a second begin, an orphan "
            "update or an orphan end. Each is something the router and the shim both "
            "enforce against, so one arriving means both enforcement copies are gone.",
        )
        # The unpaired end again, from the receiving side: `swipe_end=1`, not
        # 2. The journal above says the core dropped it; this says no app saw
        # one either, which is the claim that actually matters.
        self.assertEqual(
            other,
            {
                "tag": "second",
                "enter": "0",
                "leave": "0",
                "motion": "0",
                "relative": "0",
                "swipe_begin": "0",
                "swipe_update": "0",
                "swipe_end": "0",
                "pinch_begin": "0",
                "pinch_update": "0",
                "pinch_end": "0",
                "locked": "0",
                "unlocked": "0",
                "in_flight": "none",
                "unpaired": "0",
            },
            "the realm the output is NOT bound to must have received nothing at all: "
            "physical input follows the human's attention (#212), and a gesture is "
            "physical input like any other",
        )

        # The quantities themselves. Only one app received anything, so these
        # lines are unambiguously the bound realm's.
        relative = self._in_lines(core, "relative_motion")
        self.assertEqual(len(relative), 1, f"expected one relative delta, got {relative}")
        self.assertEqual(
            relative[0],
            {"dx": "3.000", "dy": "-4.000", "udx": "5.000", "udy": "-6.000"},
            "every quantity must arrive where it belongs. Four distinct numbers went in; "
            "a shim or core that copied the accelerated delta into the unaccelerated field "
            "would show up here and nowhere else, because the two agree on real hardware "
            "often enough for a single-number test to pass by luck.",
        )
        self.assertEqual(
            self._in_lines(core, "pinch_update"),
            [
                {
                    "dx": "1.000",
                    "dy": "2.000",
                    # Absolute since the gesture's begin, where the three beside
                    # it are deltas -- carried through, never accumulated. The
                    # IDL names this as the one confusion an app cannot detect.
                    "scale": "1.250",
                    "rotation": "-30.000",
                    "paired": "1",
                },
                {
                    "dx": "0.500",
                    "dy": "-0.500",
                    # The assertion the first update cannot make. An
                    # accumulator would report 0.938 here (1.25 * 0.75) and a
                    # differencer 0.600; only a passthrough reports 0.750.
                    "scale": "0.750",
                    "rotation": "10.000",
                    "paired": "1",
                },
            ],
            "the pinch's four quantities must arrive unmixed, in order, and with `scale` "
            "carried through rather than folded into a running product",
        )
        swipe = self._in_lines(core, "swipe_update")
        self.assertEqual(
            swipe,
            [{"dx": "7.000", "dy": "-8.000", "paired": "1"}],
            "the swipe's delta must arrive as sent",
        )

        # The finger count, on both begins. Distinct numbers went in (3 and 2)
        # because a toolkit DISPATCHES on this -- three fingers is a workspace
        # switch where four is an overview -- so a count flattened in transit
        # fires the wrong action and nothing else in this run would notice.
        self.assertEqual(
            self._in_lines(core, "swipe_begin"),
            [{"fingers": "3"}],
            "the swipe's finger count must reach the app as sent",
        )
        self.assertEqual(
            self._in_lines(core, "pinch_begin"),
            [{"fingers": "2"}],
            "the pinch's finger count must reach the app as sent, and it must be the "
            "pinch's own 2 rather than the swipe's 3",
        )

        # BOTH spellings of `gesture_state`, at the app. The pinch was
        # cancelled and the swipe was finished, and asserting only one of them
        # would leave a shim that reported every end the same way green: a
        # polarity checked in one direction is a polarity not checked.
        self.assertEqual(
            self._in_lines(core, "pinch_end"),
            [{"cancelled": "1", "paired": "1"}],
            "the human cancelled that pinch, and the app has to be told so: a `completed` "
            "here is an app committing a zoom the human abandoned",
        )
        self.assertEqual(
            self._in_lines(core, "swipe_end"),
            [{"cancelled": "0", "paired": "1"}],
            "the human FINISHED that swipe, so a `cancelled` here is an app reverting a "
            "workspace switch that really happened. Exactly one line, too: the UNPAIRED "
            "swipe end injected afterwards must have reached no app at all.",
        )

        # And the origin tag on the far side of the wire. The app cannot
        # witness this -- the Wayland extensions it receives on carry no origin
        # argument -- so without these lines "with `origin` intact" would be a
        # claim about the core's own journal only, and a shim that decoded the
        # tag into the wrong field would pass everything above.
        self.assertEqual(
            [(t["event"], t["origin"], t["delivered"]) for t in self._replays(core)],
            [
                ("motion", "physical", "1"),
                ("relative_motion", "physical", "1"),
                ("gesture_begin", "physical", "1"),
                ("gesture_swipe_update", "physical", "1"),
                ("gesture_end", "physical", "1"),
                ("gesture_begin", "physical", "1"),
                ("gesture_pinch_update", "physical", "1"),
                ("gesture_pinch_update", "physical", "1"),
                ("gesture_end", "physical", "1"),
            ],
            "the shim must have DECODED `origin=physical` off each frame, and replayed each "
            "one exactly once. Nine lines and not ten: the unpaired end never reached a "
            "shim at all, so the router dropped it upstream of both enforcement copies.",
        )

        print(
            "\n[gestures] a relative delta, a whole swipe and a cancelled pinch reached a "
            "real app through a real shim; an unpaired end reached nobody, and the app "
            "ended holding nothing"
        )

    def test_a_realm_switch_mid_gesture_pays_the_losing_app_a_cancelled_end(self):
        """Issue #222's acceptance criterion 3, over the wire.

        A pinch is in flight in `realm-0` when a `layout.focus` holder moves
        the output to `second`. From that instant the human's own fingers are
        addressed to `second`, so `realm-0`'s app will never be told the pinch
        finished — unless the core pays it an end on the way out. It does, and
        the state it pays is **`cancelled`**, because the human did not finish
        it: something took the input away, which is the exact distinction
        `gesture_state` exists to carry. An app previewing a zoom puts it back
        rather than committing one nobody confirmed.

        The in-crate twin is
        `a_realm_switch_mid_gesture_ends_it_cancelled_and_owes_the_app_nothing_after`.
        What this adds is everything after the router: that the owed end is
        encoded, sent, replayed by the shim onto a real
        `zwp_pointer_gesture_pinch_v1`, and that the app on the far side ends
        the run holding nothing.
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
        watcher = whole_realm_grant(conn, realm="realm-0")
        self._await_surface(watcher)

        phys.motion(*CENTRE)
        phys.gesture_begin("pinch", 2)
        phys.gesture_pinch(0.0, 0.0, 1.5, 0.0)

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and not self._deliveries(core, "gesture_begin"):
            time.sleep(0.05)
        self.assertEqual(
            self._deliveries(core, "gesture_begin"),
            [("realm-0", "gesture_begin", "physical")],
            "fixture check: the begin must have been delivered to the realm on screen, or "
            "the switch below has nothing to strand",
        )

        # The wait is load-bearing, not a settle-sleep, and it is
        # `test_input_switch.py`'s reason word for word: `layout.focus` is
        # attention-shaped, so the chokepoint refuses it `preempted` while
        # physical input owns the realm the human's input follows, for
        # `PHYSICAL_HOLD_WINDOW` = 500ms after the last physical event. Focus
        # immediately and the request is refused, the binding never moves, and
        # this test silently asserts nothing. Waiting it out is not a cheat:
        # presence tracks recency of human action, while the gesture stays in
        # flight in the router's own record until something ends it -- so after
        # 700ms the human's fingers are still down and the core still owes
        # `realm-0` its end, which is precisely the state under test.
        time.sleep(0.7)
        holder.focus()
        conn.sync()

        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline and not self._deliveries(core, "gesture_end"):
            time.sleep(0.05)
        self.assertEqual(
            self._deliveries(core, "gesture_begin", "gesture_end"),
            [
                ("realm-0", "gesture_begin", "physical"),
                ("realm-0", "gesture_end", "physical"),
            ],
            "the realm losing the binding must be paid the end it is owed, tagged with the "
            "origin its own begin recorded — and the realm GAINING the binding must be sent "
            "nothing, because it never saw a begin. A missing second entry is an app "
            "accumulating a pinch forever in a realm the human can no longer see.",
        )

        # The human's fingers finally lift, and their own end lands after the
        # switch. It must reach nobody: `second`'s app saw no begin, so the end
        # pairs with nothing there; `realm-0`'s record was already drained.
        phys.gesture_pinch(0.0, 0.0, 2.0, 0.0)
        phys.gesture_end("pinch")
        conn.sync()
        time.sleep(0.5)
        self.assertEqual(
            self._deliveries(core, "gesture_begin", "gesture_end"),
            [
                ("realm-0", "gesture_begin", "physical"),
                ("realm-0", "gesture_end", "physical"),
            ],
            "the human's own end must not reach the realm they moved to: that app never saw "
            "the begin, and an unpaired end is exactly what the router drops",
        )

        conn.close()
        self._finish_apps_then_core(core)

        summaries = self._require_summaries(core)
        first, other = summaries["first"], summaries["second"]
        self.assertEqual(
            (first["pinch_begin"], first["pinch_end"], first["in_flight"], first["unpaired"]),
            ("1", "1", "none", "0"),
            f"the app losing the binding must have been paid exactly one end and be left "
            f"holding nothing: {first}",
        )
        self.assertEqual(
            self._in_lines(core, "pinch_end"),
            [{"cancelled": "1", "paired": "1"}],
            "the end the app was paid must be CANCELLED: the human never finished that "
            "pinch, and an app told `completed` commits a zoom nobody confirmed",
        )
        self.assertEqual(
            (
                other["pinch_begin"],
                other["pinch_update"],
                other["pinch_end"],
                other["in_flight"],
                other["unpaired"],
            ),
            ("0", "0", "0", "none", "0"),
            f"the realm gaining the binding must have received no part of a gesture it "
            f"never saw begin: {other}",
        )

        print(
            "\n[gestures] a layout.focus mid-pinch paid realm-0's app a CANCELLED end "
            "before the binding moved; the human's own end afterwards reached nobody"
        )

    def test_a_pointer_lock_is_asked_for_by_the_app_and_answered_by_the_core(self):
        """The third served class, driven the only way it can be.

        Relative motion and gestures happen *to* an app; a pointer lock is
        something the app **asks for**, so no injector line can produce one and
        no amount of physical input stands in for it. `gesture-probe
        --lock-pointer` makes the ask a real client makes —
        `zwp_pointer_constraints_v1.lock_pointer` on its first
        `wl_pointer.enter`, `persistent` lifetime, which is the lifetime whose
        reactivation rule the core's derived design exists for.

        **What this settles**: the whole ask path is real and connected — a
        real client's `lock_pointer` reaches a real shim, which asks the core
        over the wire, which decides and answers, and whose verdict the shim
        turns back into a `zwp_locked_pointer_v1.locked` the app receives.
        Before this there was no mock-free coverage of that loop at all. And it
        settles the **scoping**: *both* apps ask, and only the realm the output
        is bound to is answered. That is a property of `live_state`'s first
        path (`self.focused != Some(realm)`) rather than a property of which
        app happened to have a reason to ask — which is why the unbound app is
        made to ask with `--lock-on-map`. A session-wide lock flag would show
        up here as `second` reporting `locked=1`.

        **What it does not settle**: that a locked pointer stops *moving* on
        real hardware, and that the core's own cursor sprite is hidden while it
        is locked. The sprite exists only on bare metal
        (`crates/vitrin-core/src/input/constraint.rs` names this as the blast
        radius: every backend CI can run passes `human_cursor: None`), so that
        half is unreachable from here by construction and stays a hardware
        runbook step — `docs/drm-bringup.md` step 13a-vi, written and not yet
        run.
        """
        core = self._core(lock_in="realm-0")
        self._two_live_apps(core)
        phys = core.physical
        phys.await_banner()

        conn = core.connect()
        watcher = whole_realm_grant(conn, realm="realm-0")
        self._await_surface(watcher)

        # The ask is made on `wl_pointer.enter`, so the pointer has to arrive
        # over the surface first -- exactly as it does when a human moves onto
        # a 3D viewport before the app grabs the pointer.
        phys.motion(*CENTRE)

        # The app's stdout is only readable after the core exits (it is the
        # core's own inherited stream), so this cannot be polled the way the
        # journal is. The round trip is client -> shim -> core -> shim ->
        # client and the core reconciles constraints once per dispatch round,
        # so a fixed wait is what there is; it is generous rather than tight,
        # because a short one would turn a slow machine into a false failure.
        time.sleep(3.0)
        conn.close()
        self._finish_apps_then_core(core)

        summaries = self._require_summaries(core)
        first = summaries["first"]
        # BOTH asks, named by tag. Without the second one the assertion below
        # would be structurally unfailable -- an app that never created a
        # `zwp_locked_pointer_v1` cannot be told `locked` however
        # session-wide the core's decision was, so `locked=0` would be a fact
        # about the probe's arguments and not about the core.
        self.assertEqual(
            sorted(
                fields.get("tag") for fields in self._in_lines(core, "lock_requested")
            ),
            ["first", "second"],
            "both apps must have asked to lock the pointer: without the ask there is "
            "nothing for the core to answer, and this test would be asserting the absence "
            "of a reply to a question nobody put.\n" + _tail(core.output()),
        )
        self.assertEqual(
            first["locked"],
            "1",
            "the app asked for a pointer lock and was never told it was in force. The ask "
            "crosses a real client -> real shim -> real core -> back boundary, so a zero "
            "here is any one of: the shim did not relay it, the core did not decide it, or "
            "the verdict never came back.\n" + _tail(core.output()),
        )
        self.assertEqual(
            (summaries["second"]["locked"], summaries["second"]["unlocked"]),
            ("0", "0"),
            "the realm the output is NOT bound to asked for exactly the same lock and must "
            "not have been given one: a constraint is scoped to the realm the human is "
            "looking at, and a session-wide lock flag would report `locked=1` here.\n"
            + _tail(core.output()),
        )

        print(
            "\n[gestures] a real client's zwp_pointer_constraints_v1.lock_pointer was "
            "relayed by a real shim, decided by the core, and answered back as `locked`"
        )


def _verb_names(bits) -> set[str]:
    """The dotted names in an effective verb bitfield (test_input_switch.py's)."""
    from vitrin_os import protocol

    return {name for name, bit in protocol.VERB_BY_DOTTED_NAME.items() if int(bits) & int(bit)}


def _tail(text: str, lines: int = 40) -> str:
    """The end of the core log, for a failure message that can be acted on."""
    return "--- core log tail ---\n" + "\n".join(text.splitlines()[-lines:])


if __name__ == "__main__":
    import unittest

    unittest.main()
