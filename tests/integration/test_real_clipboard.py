# SPDX-License-Identifier: Apache-2.0
"""WS-E.2.1 (#213): **text copied in one realm reaches another only after two
distinct human gestures** — against the shipped `vitrind`, two real C shims,
two real Wayland clients, over a real socket.

This is the mock-free half of issue #213. The in-crate tests
(`clipboard.rs`'s `an_oversized_selection_is_refused_and_the_slot_is_left_untouched`,
`a_mime_outside_the_allow_list_is_refused_and_the_slot_is_left_untouched`,
`an_unsolicited_or_stale_or_misaddressed_answer_claims_nothing`,
`no_debug_rendering_anywhere_contains_the_content`, and `chord.rs`'s
`the_two_chords_are_mutually_exclusive_by_exact_modifier_equality`) pin the
mechanism in process; this drives the shipped binary and asks the apps and the
flight recorder what actually happened.

## What is asserted, and what each assertion is evidence *from*

1. **One gesture transfers nothing.** The offer chord alone, against an empty
   slot, sends no `offer_selection` at all — evidence: the sink file the
   receiving app would have written does not exist, and the journal records
   `clipboard_refused(offer, empty_slot)`. Then the promote chord alone, which
   fills the slot, still leaves that sink absent: **promoting reaches no other
   realm**, which is the half a reviewer is likeliest to take on trust.
2. **Two gestures do transfer it**, and the receiving app has the bytes —
   evidence: the file that app wrote, byte for byte, produced by a real
   `wl_data_device` transfer inside a realm that never saw the string.
3. **Exactly one `clipboard_promoted` and one `clipboard_offered`**, naming the
   source and sink realms and agreeing on one BLAKE3 digest.
4. **The copied string appears NOWHERE in the recorder output**, grepped for the
   literal — the secrecy contract, checked against the artifact rather than
   against the type that is supposed to enforce it. The core's log is grepped
   too, because a `tracing` field is as much a leak as a journal entry.

## Why a purpose-built peer rather than alacritty and Firefox

Issue #213's criterion names them, and it names them for the right reason —
the rest of the real-app ladder runs on measured-working general-purpose apps.
It is not reachable here and the substitution is stated rather than quietly
made. Two things the gate must do have no reproducible route through a general
app in CI: put a **known** string on an app's own clipboard without a human
selecting text with a mouse, and get that app to report **byte for byte** what
it later received. A terminal can be coaxed into the first with enough
synthetic input and cannot be made to do the second at all, so a Firefox-based
version of this gate would be asserting "some pixels changed", which is not the
claim.

`shim/tests/clipboard_peer.c` is a real Wayland client — bare `wl_shm` +
xdg-shell + `wl_data_device`, no toolkit, no vitrin protocol, no knowledge that
it is confined — running under a real `vitrin-shim`. It is **not** a mock of
the seam under test: that seam is core↔shim, and this sits a whole Wayland
connection past the far side of it. It is the same instrument `click-target`
and `solid-client` already are for the actuation and fidelity gates.

## Why a test injector exists at all

`test_input_switch.py`'s and `test_attention.py`'s argument, and issue #213's
own "**CI cannot drive the chord**" criterion: GitHub runners have no physical
input device. `SeatInput::physical` is private to `crate::input`, headless has
no input device, and headless is the only backend CI runs (D-019(4)). So the
`clipboard` line on the `physical-input-injector` channel presses **this run's
configured chord** — modifiers and trigger, through the same
`input::physical_key` the nested backend's keyboard handler calls.

**That a real Ctrl-Shift-Insert on real hardware produces any of this is not
asserted here and cannot be.** It is a nested manual runbook step
(`shim/docs/nested-multi-realm.md`), exactly as issue #213 asks.

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
)

import vitrin_os  # noqa: E402

#: Matches the rest of the real-app ladder.
REALM_SIZE = "640x480"

#: `realm-0` sorts first, so it is the realm the output binds to at startup.
SECOND = "second"

#: The string the source realm's app owns. Distinctive enough that a grep of
#: the journal is meaningful, and not a password-shaped string: it ends up in
#: this file, in the process table and in a temp file, none of which is a place
#: for one.
SECRET = "vitrin-clipboard-canary-8f3a1c"

#: The one type that may cross (`clipboard::CLIPBOARD_MIME`).
MIME = "text/plain;charset=utf-8"

RUN_MS = "60000"

#: Comfortably past `input::PHYSICAL_HOLD_WINDOW` (500 ms), so a layout use
#: after a chord is not refused `preempted`. Waiting rather than reaching for
#: the attention key keeps this gate about the clipboard.
PRESENCE_LAPSE_S = 0.9

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


class RealCrossRealmClipboard(IntegrationTest):
    """Copy in realm A, paste in realm B, and only after two gestures."""

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
        peer = os.environ.get("VITRIN_CLIPBOARD_PEER_APP")
        if not peer:
            sibling = self.shim_bin.resolve().parent / "clipboard-peer"
            if sibling.is_file() and os.access(sibling, os.X_OK):
                peer = str(sibling)
        if not peer:
            self.fail(
                f"no clipboard-peer beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration."
            )
        self.peer_bin = str(pathlib.Path(peer).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-clipboard-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        #: What `second`'s app writes whatever selection it is offered into.
        #: Its ABSENCE is half this gate's evidence, so it must start absent.
        self.sink = self.work / "sink.bin"
        self.assertFalse(self.sink.exists())

    # -- fixtures ---------------------------------------------------------

    def _core(self):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.peer_bin,
            args=["--run-ms", RUN_MS],
            realms=(SECOND,),
            realm_args={
                # The SOURCE owns the selection; the SINK reports what it is
                # given. Different colours so a reader of the core log can tell
                # the two realms apart at a glance.
                "realm-0": ["--run-ms", RUN_MS, "--colour", "0000ff", "--offer", SECRET],
                SECOND: ["--run-ms", RUN_MS, "--colour", "00ff00", "--sink", str(self.sink)],
            },
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
                if comm_of(kid).startswith("clipboard-peer")
            ]
            if len(apps) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(apps), 2, f"each shim must have fork/exec'd its own clipboard-peer; got {apps}"
        )

    def _entries(self, core, kind):
        return [e for e in core.entries() if e["kind"] == kind]

    def _await_entry(self, core, kind, count=1, timeout=15.0):
        deadline = time.monotonic() + timeout
        seen: list[dict] = []
        while time.monotonic() < deadline:
            seen = self._entries(core, kind)
            if len(seen) >= count:
                return seen
            time.sleep(0.1)
        self.fail(f"the journal never recorded {count} `{kind}` entr(y|ies); got {seen}")

    def _sink_bytes(self, timeout=10.0) -> bytes:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                return self.sink.read_bytes()
            except OSError:
                time.sleep(0.1)
        self.fail(
            f"{self.sink} was never written: the receiving app was offered no selection it "
            "would take, so nothing crossed the realm boundary"
        )

    # -- the gate ---------------------------------------------------------

    def test_two_gestures_move_text_between_realms_and_one_gesture_moves_nothing(self):
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
        # `layout.focus` over `second`: the human has to be *looking at* the
        # realm they offer into, because both chords aim at the bound realm.
        # No `actuate.*` anywhere in this test -- every gesture below is the
        # human's, through the physical intake.
        holder = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "layout.focus"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()

        # (0) THE OFFER GESTURE ALONE, WITH AN EMPTY SLOT. Nothing has been
        # promoted, so the core sends no `offer_selection` at all -- not an
        # empty one. Evidence: the journal, and the sink that stays absent.
        phys.clipboard(promote=False)
        refused = self._await_entry(core, "clipboard_refused")
        self.assertEqual(refused[0]["gesture"], "offer")
        self.assertEqual(
            refused[0]["reason"],
            "empty_slot",
            f"an offer against an empty slot must refuse for that reason; got {refused[0]}",
        )
        self.assertFalse(
            self.sink.exists(),
            "an offer gesture against an empty slot must send nothing to any realm",
        )

        # (1) THE PROMOTE GESTURE, IN THE REALM THE OUTPUT IS BOUND TO.
        # `realm-0` sorts first, so it is the realm the output bound at
        # startup and the realm this chord aims at.
        phys.clipboard(promote=True)
        promoted = self._await_entry(core, "clipboard_promoted")
        self.assertEqual(len(promoted), 1, f"one gesture, one promotion; got {promoted}")
        self.assertEqual(
            promoted[0]["realm"],
            "realm-0",
            "the promote chord asks the realm the human is LOOKING AT; a promotion from a "
            "hidden realm would be a channel out of a realm nobody chose",
        )
        self.assertEqual(promoted[0]["mime"], MIME)
        self.assertEqual(
            promoted[0]["bytes"],
            len(SECRET.encode()),
            "the journal records a length, and it must be the real one -- a length that did "
            "not track the content would make the whole digest-only contract decorative",
        )
        self.assertEqual(promoted[0]["digest_alg"], "blake3")
        self.assertRegex(promoted[0]["digest"], r"\A[0-9a-f]{64}\Z")

        # ...AND STILL NOTHING HAS CROSSED. This is the assertion issue #213
        # asks for in as many words, and the one a reviewer is likeliest to
        # take on trust: promoting writes the core's slot and reaches NO other
        # realm. The receiving app is live, focused-when-bound, and has been
        # offered nothing.
        time.sleep(1.0)
        self.assertFalse(
            self.sink.exists(),
            "ONE GESTURE TRANSFERRED SOMETHING. Promoting must fill the core's slot and "
            "reach no other realm; the receiving app wrote a sink file, so it was offered "
            "a selection nobody made a second gesture for",
        )
        self.assertEqual(
            self._entries(core, "clipboard_offered"),
            [],
            "and no offer may be journaled either",
        )

        # (2) THE HUMAN MOVES THE OUTPUT. The chord above marked physical
        # presence in `realm-0`, so a layout use inside the 500 ms hold window
        # is refused `preempted` (D-022(6)). Waiting it out rather than
        # reaching for the attention key keeps this gate about the clipboard.
        time.sleep(PRESENCE_LAPSE_S)
        holder.focus()
        conn.sync(holder)

        # (3) THE OFFER GESTURE, IN THE REALM THE OUTPUT MOVED TO.
        phys.clipboard(promote=False)
        offered = self._await_entry(core, "clipboard_offered")
        self.assertEqual(len(offered), 1, f"one gesture, one offer; got {offered}")
        self.assertEqual(offered[0]["realm"], SECOND)
        self.assertEqual(
            offered[0]["source"],
            "realm-0",
            "the journal names which boundary was crossed, in one line",
        )
        self.assertEqual(offered[0]["mime"], MIME)
        self.assertEqual(offered[0]["bytes"], len(SECRET.encode()))
        self.assertEqual(
            offered[0]["digest"],
            promoted[0]["digest"],
            "what was offered must be what was promoted; two digests that differ mean the "
            "slot changed between the gestures",
        )

        # ...and the receiving app really has the bytes, through a real
        # `wl_data_device` transfer inside a realm that never saw the string.
        self.assertEqual(
            self._sink_bytes(),
            SECRET.encode(),
            "the app in the second realm must have received the string byte for byte",
        )

        # (4) EXACTLY ONE OF EACH, over the whole run.
        self.assertEqual(len(self._entries(core, "clipboard_promoted")), 1)
        self.assertEqual(len(self._entries(core, "clipboard_offered")), 1)

        # (5) THE SECRECY CONTRACT, CHECKED AGAINST THE ARTIFACT. The recorder
        # holds a length and a digest and has no field content could go in --
        # that is the type's job. This greps the bytes anyway, because a
        # contract asserted only by the type that enforces it is a contract
        # nobody would notice losing.
        journal = pathlib.Path(core.recorder).read_bytes()
        self.assertNotIn(
            SECRET.encode(),
            journal,
            "the copied string appears in the flight recorder. Length and digest only "
            "(recorder.rs's `credential_bytes` precedent): a clipboard carries passwords, "
            "and a journal that recorded one would be a credential store built by accident",
        )
        log = (self.work / "core.log").read_bytes()
        self.assertNotIn(
            SECRET.encode(),
            log,
            "the copied string appears in the core's LOG. A `tracing` field is as much a "
            "leak as a journal entry, and a `#[derive(Debug)]` on anything holding the slot "
            "is how it gets there",
        )
