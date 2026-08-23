# SPDX-License-Identifier: Apache-2.0
"""Boot a real `vitrind` and talk to it the way an agent does.

Everything in this suite goes through :class:`Core`, which starts the
**shipped binary** in its own throwaway `XDG_RUNTIME_DIR` and hands back the
socket, the pid and the flight-recorder log. No test constructs a runtime
in-process; that is what `crates/vitrin-core/src/session.rs`'s own tests do,
and the gap between the two is exactly what this suite exists to cover
(see ``run.sh``).
"""

from __future__ import annotations

import json
import os
import pathlib
import shlex
import shutil
import signal
import socket as socket_mod
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest

REPO = pathlib.Path(os.environ.get("VITRIN_REPO", pathlib.Path(__file__).resolve().parents[2]))


def _resolve_core() -> pathlib.Path:
    """The `vitrind` every core in this suite starts.

    Defaults to the build tree, which is what every existing caller wants.
    `VITRIN_CORE_BIN` moves it, and exists for exactly one reason (issue
    #286): AppArmor attaches a profile to a binary's **resolved absolute
    pathname**, so a profile written for an installed `vitrind` cannot be
    exercised against `target/debug/vitrind`. The `apparmor-profile` job in
    `.github/workflows/ci.yml` installs both binaries into `/usr/lib/vitrin/`
    and points this at them; nothing else in the repository sets it.

    **Set-but-wrong is a misconfiguration, not a machine state**, and it fails
    here rather than 90 times inside `setUp` — the same rule, and the same
    reasoning, as `VITRIN_C_SHIM_BIN` in `tests/integration/run.sh`. Silently
    falling back to the build tree would be worse than any error: the suite
    would pass, against the binary the caller was specifically trying not to
    test, and the log would look identical to a real run.
    """
    named = os.environ.get("VITRIN_CORE_BIN")
    if not named:
        return REPO / "target" / "debug" / "vitrind"
    path = pathlib.Path(named)
    if not path.is_file() or not os.access(path, os.X_OK):
        sys.exit(
            f"VITRIN_CORE_BIN={named} is not an executable file. It is SET, so a run against "
            "a specific vitrind was requested; refusing to fall back to target/debug/vitrind, "
            "which would pass while testing the wrong binary."
        )
    # The core resolves its helper as a sibling of its own `current_exe()`
    # (`default_realm_init_path` in crates/vitrin-core/src/main.rs), so a
    # relocated vitrind with no sibling helper starts fine and then fails
    # every confined spawn with a message about the helper — which reads like
    # a confinement bug and is not one. Say so here instead.
    helper = path.parent / "vitrin-realm-init"
    if not helper.is_file() or not os.access(helper, os.X_OK):
        sys.exit(
            f"VITRIN_CORE_BIN={named} has no executable `vitrin-realm-init` beside it "
            f"({helper}). vitrind resolves the helper as a sibling of its own resolved "
            "path, so every `--isolation=default` spawn would fail for a reason that has "
            "nothing to do with what this run is measuring. Install both binaries together."
        )
    return path


VITRIND = _resolve_core()
MOCK_SHIM = REPO / "target" / "debug" / "vitrin-mock-shim"
#: The P1.8.5 SSIM bridge (issue #107): the compiled `vitrin-golden` compare
#: tool the real-app fidelity gate shells out to, so the dependency-free Python
#: suite scores an agent frame against the core-internal capture with the *real*
#: harness rather than a second, driftable SSIM port. Built by the same
#: `cargo build --workspace` that builds `vitrind`.
GOLDEN_CMP = REPO / "target" / "debug" / "vitrin-golden-cmp"

#: The demo identity, matching `examples/principals.toml`. Auto-approve is
#: only permitted when the registry holds nothing but this principal (R6),
#: so the registries written here deliberately hold exactly one row.
DEMO_IDENTITY = "vitrin://local/agent/demo"

#: 64 hex chars. The core refuses tokens under 16 bytes, and a registry that
#: tripped that refusal would fail every test here for the wrong reason.
DEMO_TOKEN = "a" * 64

#: How long a core gets to bind its socket before a test gives up. Generous
#: because CI runners are slow and a flaky integration suite is worse than a
#: slow one; the wait polls, so the cost is only paid when something is
#: actually wrong.
BOOT_TIMEOUT_S = 30.0

#: Frames the mock shim animates for, per core. **This is a CPU budget, not
#: a duration.** Headless has no output clock, so a paced shim composes as
#: fast as the runtime loop will dispatch it — the count is the only thing
#: bounding how long it spins. It needs to outlive the one test that asserts
#: two captures differ (~1 s of wall clock) and no longer; an earlier draft
#: used 100_000 and pinned a core for the whole suite.
ANIMATE_FRAMES = 1200

#: What `comm` a realm's **supervisor** reports (P2.6.2, issue #186). At
#: `--isolation=default` the core's direct child is `vitrin-realm-init`, which
#: stays in the host pid namespace and forks the realm's PID 1; the shim is
#: therefore a grand child of the core rather than a child. The kernel
#: truncates `comm` to 15 bytes, so `vitrin-realm-init` reads back clipped --
#: spelled out here once rather than re-derived at each call site.
SUPERVISOR_COMM = "vitrin-realm-in"

#: `comm` prefixes a realm's fd-3 peer can have. Both, because `shims_of`
#: answers "what did this core fork as a realm", and a component test's realm
#: is the mock.
#:
#: **It is not the mock-freeness check, and it never was one.** It used to be
#: asserted as if it were -- nine gates read
#: `comm_of(shim).startswith("vitrin-shim")` -- and #186's confinement work
#: showed that check had already stopped checking: a confined shim takes its
#: `comm` from the bind target, so both binaries answered to the same name
#: whatever the gate believed. Mock-freeness is now
#: :func:`exe_identity` against :func:`file_identity` of the named binary,
#: which compares inodes rather than strings. Since #283 the bind target is
#: `/vitrin/vitrin-shim`, so a confined shim reads back as `vitrin-shim`
#: again -- that restores `ps` legibility and grants the name no more evidence
#: value than it had before. Measured, so nobody has to take it on trust: a
#: `--isolation=default` core started with `vitrin-mock-shim` yields comm
#: `vitrin-shim`, `/proc/<pid>/exe -> /vitrin/vitrin-shim`, and the MOCK's
#: inode. `vitrin-mock-shi` below is therefore the UNCONFINED spelling only.
SHIM_COMMS = ("vitrin-shim", "vitrin-mock-shi")

#: The paths a **confined** realm sees, mirroring the constants in
#: `crates/vitrin-realm-init/src/lib.rs` (P2.6.2, #186). At
#: `--isolation=default` the core injects these rather than the host paths, so
#: a gate asserting on the app's environment has to name them -- and a gate
#: that wants the older claim ("this is the realm's OWN socket, not a host
#: one") gets it by comparing inodes through `/proc/<pid>/root`, which is
#: strictly stronger than comparing the path string ever was.
IN_REALM_RUNTIME_DIR = "/run/vitrin"
IN_REALM_WAYLAND_SOCKET = "/run/vitrin/wayland-0"
IN_REALM_HOME = "/vitrin/home"
IN_REALM_SHIM = "/vitrin/vitrin-shim"

#: The highest Landlock ABI rung this build knows how to request, mirroring
#: `LANDLOCK_BUILD_MAX_RUNG` in `crates/vitrin-realm-init/src/lib.rs` (P2.6.3,
#: #187). A kernel reporting more is clamped down to it, and the realm's
#: journal entry says so -- which is only checkable from here if this side
#: knows the number. `vitrind --print-floor` prints it as
#: `build.landlock_max_rung`.
LANDLOCK_BUILD_MAX_RUNG = 9

#: The lowest Landlock ABI this build will start on, mirroring
#: `LANDLOCK_MIN_ABI` in `crates/vitrin-realm-init/src/lib.rs` (owner's
#: decision, 2026-08-15). A kernel below it is refused rather than degraded, so
#: a confined spawn that reached the point of writing a journal entry must have
#: obtained at least this rung -- which is the only mock-free check that the
#: declared floor is the one a real realm actually got. `vitrind --print-floor`
#: prints it as `build.landlock_min_abi`.
#:
#: **Held to the crate by `cargo xtask isolation-matrix`**, which reads both
#: numbers out of `vitrin-realm-init` and refuses to render if this file
#: disagrees. Before that check existed this constant sat at 7 for a day after
#: commit e7b5514 lowered the crate's floor to 6, which would have taken
#: `test_real_confinement.py`'s `obtained_rung >= LANDLOCK_MIN_ABI` assertion
#: RED on any ABI-6 kernel -- that is, on the exact machine class the floor was
#: lowered *for*. A mirrored constant nothing checks is a gate that fails on
#: the deployment target and nowhere else.
LANDLOCK_MIN_ABI = 6

#: A suite-wide `--landlock` argument for every core that does not name one.
#:
#: **This exists to make a published measurement repeatable, not to make a run
#: pass.** `docs/book/src/limits.md` once cited this repo's GTK gates as
#: failing at the shipped default and passing at `--landlock=off`, and the
#: second half of that sentence was unreachable from the repository -- no
#: module in the real-app ladder took a Landlock setting, so the only way to
#: obtain it was to edit a test. A control nobody can re-run is not a control.
#: Since K9b (2026-08-15) the first half is no longer true either: those gates
#: pass at the shipped default, and the page has deleted the `--landlock=off`
#: column rather than carrying a figure it never re-ran. The knob stays,
#: because every remaining `--landlock=off` measurement on that page -- and
#: `tests/integration/landlock-denials.sh` -- needs a control run to be
#: reachable from here rather than only from the machine it was first taken on.
#:
#: Two properties keep it from quietly becoming the thing under test:
#:
#: * an explicit `landlock=` argument always wins, so `test_real_confinement.py`
#:   keeps naming both settings in one run, and
#: * the confinement gate asserts its confined half journals
#:   `requested == "highest"`, so setting this to `off` turns that gate **red**
#:   rather than letting a whole-suite `off` run report as an ordinary pass.
#:   That is the intended behaviour: such a run is a control run, and
#:   `tests/integration/run.sh` announces it as one.
LANDLOCK_OVERRIDE = os.environ.get("VITRIN_LANDLOCK") or None

#: A command prefix placed in front of every `vitrind` this suite starts.
#:
#: **A diagnostic, and only ever a diagnostic.** It exists for
#: `tests/integration/landlock-denials.sh`, which needs to observe what the
#: kernel refuses a realm *from outside the realm* -- and on a host whose audit
#: subsystem is switched off (`audit_enabled=0`, which is the default on at
#: least Arch and every machine measured for this repo so far) the kernel's own
#: Landlock records do not exist to be read, so the only remaining observer is
#: a tracer on the outside. Nothing in the suite sets this; a run with it unset
#: builds exactly the argv it always built.
#:
#: `strace -D` (**--daemonize=grandchild**) is the reason this is safe to hand
#: a tracer: with `-D` the tracer re-parents itself and `vitrind` stays a
#: *direct* child of this process, so `Core.pid` is still the core's pid and
#: `shims_of`'s deliberate two-level walk (core -> helper -> shim) still counts
#: the same two levels. A wrapper without that property shifts the process tree
#: by one and silently breaks every gate that reads it -- which is a real trap,
#: and the reason this docstring names the flag rather than leaving the choice
#: of tracer open.
#:
#: Split with `shlex`, so quoting works the way a shell's does.
CORE_WRAPPER = shlex.split(os.environ.get("VITRIN_CORE_WRAPPER", ""))

#: Hard per-test ceiling. The failure this exists for is issue #77's trap T1:
#: a regression that registers the shim socketpair's source after the fork
#: leaves the shim blocked on `configure` forever, and `observe()` then never
#: returns. Without this the suite would hang until CI's 10-minute cap and
#: report a timeout rather than a named failing test — the worst possible
#: reporting for the exact bug this suite exists to catch.
TEST_TIMEOUT_S = 90


def _toml_string(value: str) -> str:
    r"""A TOML basic string literal for `value`.

    The realm loader (`crates/vitrin-core/src/toml_subset.rs`) reads a small
    TOML subset; a program path or env name never contains a quote or
    backslash, but escaping them keeps this honest rather than assuming it.
    """
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _toml_string_array(values: list[str]) -> str:
    """A TOML inline array of basic strings (`[]` when empty)."""
    return "[" + ", ".join(_toml_string(v) for v in values) + "]"


class CoreFailed(Exception):
    """The core exited when a test expected it to be serving."""


class InjectorFailed(Exception):
    """The consent-injector channel said something the harness cannot use."""


class PhysicalInputInjector:
    """The `physical-input-injector` channel (issue #212), from the harness
    side.

    A `physical-input-injector`-feature `vitrind` invoked with
    `--physical-input-fd N` speaks these request lines on the inherited
    socketpair (`crates/vitrin-core/src/input/injector.rs`)::

        motion <x> <y>                  (realm-view pixels)
        button <evdev-code> press|release
        scroll vertical|horizontal <v120>
        key <evdev-scancode> press|release
        attention                       (one whole tap of the configured chord)
        clipboard promote|offer          (one whole modifier chord)
        screenshot                       (one whole modifier chord)
        relative_motion <dx> <dy> <dx-unaccel> <dy-unaccel>
        gesture begin swipe|pinch <fingers>
        gesture update swipe <dx> <dy>
        gesture update pinch <dx> <dy> <scale> <rotation>
        gesture end swipe|pinch completed|cancelled

    Each becomes a host event handed to the core's **production** intake
    (`input::intake_physical`, or `input::physical_key` for keys), tagged
    `origin=physical` by the one constructor that can tag it, and routed by the
    same `session::route_physical_turn` the nested backend's winit handler
    tails into.

    The reply is ``ack <n>`` where `n` is the number of `SeatInput`s the intake
    produced -- **not** the number delivered. Whether an event reaches an app
    is what a gate here is measuring, so the channel deliberately does not
    report it: the evidence is the app's own behaviour and the flight
    recorder's `seat_delivered` realm.

    Keys are limited to the core's layout-invariant scancode table (Escape,
    Enter, Tab, arrows, modifiers, F-keys, space) because a headless core has
    no host keymap to ask and the core is forbidden to grow one.

    `screenshot` (WS-E.2.4, issue #216) is one whole chord of this run's
    `--screenshot-chord`, on the same terms as `clipboard`.

    `relative_motion` and `gesture` (WS-E.4.2, issue #222) are the classes the
    seat vocabulary grew for the touchpad, and they are the first lines here
    that name a wire event rather than a chord. **They do not stand in for
    libinput's own gesture detection**: a real touchpad reports finger
    contacts and libinput is what decides those contacts are a three-finger
    swipe. That decision is upstream of `intake_physical` and nothing reachable
    from this channel exercises it.

    `attention` (WS-E.1.7, issue #232) is one whole tap -- press then release --
    of **this run's configured attention chord**, resolved core-side and fed to
    the same `input::physical_key` a `key` line uses. It is a name for a
    *gesture*, never a second way into the core: a harness that sent only the
    press would leave the core believing the key is held, and one that
    hard-coded a scancode would press the wrong key the moment a run passed
    `--attention-chord rsuper`.
    """

    #: `KEY_LEFTCTRL` -- a modifier, in the layout-invariant table, and the
    #: key a latch is worst for.
    KEY_LEFTCTRL = 29
    #: `BTN_LEFT`, the evdev code `click-target` watches for.
    BTN_LEFT = 0x110

    VERBS = ("vitrin-physical-input", "ack", "err")

    def __init__(self, sock: "socket_mod.socket") -> None:
        self.sock = sock
        self._buf = b""
        self._replies: list[tuple[str, ...]] = []
        self.banner: str | None = None

    def _pump(self, deadline: float) -> None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise InjectorFailed("the physical-input channel went quiet")
        self.sock.settimeout(remaining)
        try:
            data = self.sock.recv(4096)
        except TimeoutError as exc:
            raise InjectorFailed("the physical-input channel went quiet") from exc
        if not data:
            raise InjectorFailed(
                "the core closed the physical-input channel (it exited, or never adopted it)"
            )
        self._buf += data
        while b"\n" in self._buf:
            raw, self._buf = self._buf.split(b"\n", 1)
            fields = tuple(raw.decode("utf-8", "replace").split(" "))
            if fields[0] not in self.VERBS:
                raise InjectorFailed(f"unknown physical-input line: {fields!r}")
            if fields[0] == "vitrin-physical-input":
                self.banner = " ".join(fields)
            else:
                self._replies.append(fields)

    def await_banner(self, timeout: float = 20.0) -> str:
        """Block for the core's ``vitrin-physical-input 1`` greeting.

        Its arrival is what distinguishes an instrumented core from one that
        merely opened a socket, which is the same guarantee the consent
        channel's banner carries.
        """
        deadline = time.monotonic() + timeout
        while self.banner is None:
            self._pump(deadline)
        return self.banner

    def _send(self, line: str, timeout: float = 10.0) -> tuple[str, ...]:
        self.sock.sendall((line + "\n").encode("utf-8"))
        deadline = time.monotonic() + timeout
        while not self._replies:
            self._pump(deadline)
        return self._replies.pop(0)

    def _expect_ack(self, line: str, produced: int | None = None) -> int:
        fields = self._send(line)
        if fields[0] != "ack":
            raise InjectorFailed(f"{line!r} was refused: {' '.join(fields)}")
        got = int(fields[1])
        if produced is not None and got != produced:
            raise InjectorFailed(
                f"{line!r} produced {got} intake events, expected {produced}: the core's own "
                "intake made something different of this request than the harness assumed"
            )
        return got

    def motion(self, x: float, y: float) -> None:
        self._expect_ack(f"motion {x} {y}", 1)

    def button(self, code: int, pressed: bool) -> None:
        self._expect_ack(f"button {code} {'press' if pressed else 'release'}", 1)

    def key(self, evdev: int, pressed: bool) -> None:
        self._expect_ack(f"key {evdev} {'press' if pressed else 'release'}", 1)

    def attention(self) -> None:
        """Tap the core's attention key: press and release, `ack 2`.

        Two intake events, because the chord is a tap. The core consumes both,
        so no app in any realm sees it -- which is why the evidence that it
        happened is the `attention` wire event and the flight recorder, never
        an app's own output.
        """
        self._expect_ack("attention", 2)

    def clipboard(self, promote: bool) -> None:
        """Chord the core's cross-realm clipboard gesture (WS-E.2.1, #213).

        `promote=True` is ctrl+shift+KEY (ask the focused realm for its
        selection); `promote=False` is shift+KEY (offer the core's slot to the
        focused realm). One line is the whole chord -- modifiers down, trigger
        down, trigger up, modifiers up -- because half a chord is not a gesture.

        The ack count is the number of intake events, so it is exactly the
        number of key transitions: 6 for the three-key chord, 4 for the
        two-key one. Asserting it is what makes `ack` evidence rather than
        decoration -- a core that stopped delivering a modifier would still
        answer `ack`, but not with this number.
        """
        self._expect_ack(
            f"clipboard {'promote' if promote else 'offer'}", 6 if promote else 4
        )

    def screenshot(self) -> None:
        """Chord the core's screenshot key (WS-E.2.4, #216).

        One line is the whole configured chord -- modifiers down, trigger
        down, trigger up, modifiers up. The ack count is the number of intake
        events, so `ctrl+print` is 4; asserting it is what makes `ack` evidence
        rather than decoration.

        This is the line that makes issue #216's "CI cannot press a key"
        criterion stale: it presses a REAL chord on real scancodes through the
        same `input::physical_key` the nested backend's keyboard handler calls,
        so the gate covers detection as well as effect.
        """
        self._expect_ack("screenshot", 4)

    def relative_motion(
        self, dx: float, dy: float, dx_unaccel: float, dy_unaccel: float
    ) -> None:
        """One `relative_motion` (WS-E.4.2, #222): a pointer DELTA, not a position.

        All four quantities are named by the caller and none is defaulted from
        another, because on a real device the accelerated and unaccelerated
        deltas are different numbers and the wire carries both. A harness that
        passed one twice could not tell a correct shim from one that copied a
        field.

        It does not move the pointer. The absolute `motion` that accompanies a
        real movement is a separate line, because the absolute half is minted
        by the backend that owns the accumulated position and this channel owns
        none.
        """
        self._expect_ack(
            f"relative_motion {dx} {dy} {dx_unaccel} {dy_unaccel}", 1
        )

    def gesture_begin(self, kind: str, fingers: int) -> None:
        """Begin a swipe or a pinch. `kind` is the wire's own spelling."""
        self._expect_ack(f"gesture begin {kind} {fingers}", 1)

    def gesture_swipe(self, dx: float, dy: float) -> None:
        """One `gesture_swipe_update`: the finger group's delta since the last."""
        self._expect_ack(f"gesture update swipe {dx} {dy}", 1)

    def gesture_pinch(self, dx: float, dy: float, scale: float, rotation: float) -> None:
        """One `gesture_pinch_update`.

        `scale` is **absolute since this gesture's begin** while the other
        three are deltas since its previous event -- the asymmetry the IDL
        spells out because reading one as the other is undetectable app-side.
        Nothing here accumulates it.
        """
        self._expect_ack(f"gesture update pinch {dx} {dy} {scale} {rotation}", 1)

    def gesture_end(self, kind: str, cancelled: bool = False) -> None:
        """End a gesture, `completed` or `cancelled`.

        Spelled out rather than defaulted: the two are what let an app commit
        rather than guess -- a completed pinch keeps its zoom, a cancelled one
        puts it back.
        """
        state = "cancelled" if cancelled else "completed"
        self._expect_ack(f"gesture end {kind} {state}", 1)

    def expect_refusal(self, line: str) -> str:
        """Send a line the core must REFUSE, and return the reason it gave.

        The vocabulary is closed on purpose, and a malformed line is answered
        `err` rather than dropped, panicked on, or silently accepted as a
        neighbouring message. A gate that never exercised that would leave the
        one surface an untrusted peer controls untested.
        """
        fields = self._send(line)
        if fields[0] != "err":
            raise InjectorFailed(
                f"{line!r} was ACCEPTED ({' '.join(fields)}); the injector vocabulary is "
                "closed and a malformed line must be answered `err`"
            )
        return " ".join(fields[1:])

    def click(self, x: float, y: float, code: int | None = None) -> None:
        """Move, press, release -- the human's version of `grant.pointer.click`."""
        code = self.BTN_LEFT if code is None else code
        self.motion(x, y)
        self.button(code, True)
        self.button(code, False)

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


class ConsentInjector:
    """The `consent-injector` channel (issue #138), from the harness side.

    A `consent-injector`-feature `vitrind` invoked with
    `--consent-injector-fd N` speaks a small line vocabulary on the inherited
    socketpair (`crates/vitrin-core/src/consent/injector.rs`):

    * unsolicited **edges** -- ``raised <petition> <token>`` and
      ``lowered <petition>``, emitted from inside the core's own consent round
      *after* `ConsentGrab::raise` has already marked the petition shown. So a
      `raised` line is a guarantee, in the core's ordering, that
      `consent_held` is in force for that principal: a caller blocks on the
      line and then acts, with no sleep and no pixel poll;
    * a synchronous **snapshot** -- ``describe`` replies with the consent
      surface's geometry and, when a card is up, the card's own footprint of
      the human-visible framebuffer as a sealed memfd over ``SCM_RIGHTS``;
    * a synchronous **trusted-band witness** (issue #139) -- ``band`` replies
      with ten scalar fields and no descriptor. It carries no pixels and,
      more strongly, nothing whose value moves with the session's indicator
      colour; `crates/vitrin-core/src/backend/band_witness.rs` argues why the
      weaker "no pixels" rule would not have been enough;
    * **message acceptance** -- ``decide <token> <button>`` replies
      ``decided-ack <queued|...>``.

    The channel never reports an authority *outcome*. Whether the decision
    conferred anything is read where it always is: the agent's `resolved`
    event on the wire and the flight recorder.
    """

    #: Everything the core may say, so an unknown verb is a loud failure
    #: rather than a silently ignored line.
    VERBS = (
        "vitrin-consent-injector",
        "raised",
        "lowered",
        "prompt",
        "band",
        "decided-ack",
    )

    def __init__(self, sock: "socket_mod.socket") -> None:
        self.sock = sock
        self._buf = b""
        self._edges: list[tuple[str, ...]] = []
        self._replies: list[tuple[str, ...]] = []
        self._fds: list[int] = []
        self.banner: str | None = None

    # -- transport ---------------------------------------------------------

    def _pump(self, deadline: float) -> None:
        """Read one message, classify its lines, and keep any descriptor.

        `recvmsg` on a stream socket stops at the boundary of a segment
        carrying ancillary data, so a descriptor is always delivered with the
        chunk that starts the line it belongs to -- which is what lets the
        pixels be matched to the `prompt` line positionally.
        """
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise InjectorFailed("the consent injector channel went quiet")
        self.sock.settimeout(remaining)
        try:
            data, fds, _flags, _addr = socket_mod.recv_fds(self.sock, 4096, 1)
        except TimeoutError as exc:
            raise InjectorFailed("the consent injector channel went quiet") from exc
        self._fds.extend(fds)
        if not data:
            raise InjectorFailed(
                "the core closed the consent injector channel (it exited, or never adopted it)"
            )
        self._buf += data
        while b"\n" in self._buf:
            raw, self._buf = self._buf.split(b"\n", 1)
            fields = tuple(raw.decode("utf-8", "replace").split(" "))
            if fields[0] not in self.VERBS:
                raise InjectorFailed(f"unknown consent-injector line: {fields!r}")
            if fields[0] == "vitrin-consent-injector":
                self.banner = " ".join(fields)
            elif fields[0] in ("raised", "lowered"):
                self._edges.append(fields)
            else:
                self._replies.append(fields)

    def await_banner(self, timeout: float = 20.0) -> str:
        """Block for the core's ``vitrin-consent-injector 1`` greeting."""
        deadline = time.monotonic() + timeout
        while self.banner is None:
            self._pump(deadline)
        return self.banner

    def _next_reply(self, verb: str, timeout: float) -> tuple[str, ...]:
        deadline = time.monotonic() + timeout
        while True:
            for i, fields in enumerate(self._replies):
                if fields[0] == verb:
                    return self._replies.pop(i)
            self._pump(deadline)

    # -- edges -------------------------------------------------------------

    def await_raised(self, timeout: float = 30.0) -> tuple[str, str]:
        """Block until a card goes up; return ``(petition_id, token)``."""
        deadline = time.monotonic() + timeout
        while True:
            for i, fields in enumerate(self._edges):
                if fields[0] == "raised":
                    self._edges.pop(i)
                    return fields[1], fields[2]
            self._pump(deadline)

    def await_lowered(self, petition: str | None = None, timeout: float = 30.0) -> str:
        """Block until a card comes down; return the petition it named."""
        deadline = time.monotonic() + timeout
        while True:
            for i, fields in enumerate(self._edges):
                if fields[0] == "lowered" and (petition is None or fields[1] == petition):
                    self._edges.pop(i)
                    return fields[1]
            self._pump(deadline)

    # -- requests ----------------------------------------------------------

    def describe(self, timeout: float = 30.0) -> tuple[dict[str, object], bytes | None]:
        """Snapshot the consent surface: geometry, and the card's pixels.

        Returns ``(fields, rgba_or_none)``. The pixels are the **card's
        footprint only** -- never a whole human-visible frame, and never the
        trust band or the trusted ring, which are painted in this session's
        secret indicator colour and are therefore not read back at all
        (issue #85; `crates/vitrin-core/src/consent/mod.rs`'s `card_rect`).
        """
        self.sock.sendall(b"describe\n")
        fields = self._next_reply("prompt", timeout)
        keys = (
            "state",
            "token",
            "card_x",
            "card_y",
            "card_w",
            "card_h",
            "view_w",
            "view_h",
            "band_h",
            "win_x",
            "win_y",
            "win_w",
            "win_h",
            "bytes",
        )
        if len(fields) != len(keys) + 1:
            raise InjectorFailed(f"malformed `prompt` reply: {fields!r}")
        out: dict[str, object] = {}
        for key, value in zip(keys, fields[1:]):
            out[key] = value if key in ("state", "token") else int(value)
        size = int(out["bytes"])  # type: ignore[arg-type]
        if size == 0:
            return out, None
        if not self._fds:
            raise InjectorFailed(
                f"the core reported {size} bytes of occlusion window but sent no descriptor"
            )
        fd = self._fds.pop(0)
        try:
            pixels = os.pread(fd, size, 0)
        finally:
            os.close(fd)
        if len(pixels) != size:
            raise InjectorFailed(f"short occlusion window: {len(pixels)} of {size} bytes")
        return out, pixels

    def band(self, timeout: float = 30.0) -> dict[str, object]:
        """The trusted-band witness (issue #139), as twelve numbers plus the
        bound realm's id.

        Deliberately returns **no pixels and no descriptor**, and that is not
        a convenience of this method -- it is the shape of the reply. The
        session's trusted-indicator colour never reaches a file or a
        descriptor in any build, so a harness cannot be handed the band to
        look at; what it can be handed is a set of counters and booleans whose
        values are the same whatever the colour is. See
        `crates/vitrin-core/src/backend/band_witness.rs` for why an apparently
        safer field ("do the band's rows equal the client's?") is a
        brute-force oracle, and `tests/integration/test_real_trust_band.py`
        for the gate that reads this.

        The `realm` key names **which realm** the pixel-derived fields are
        about (WS-E.1.3, issue #209): `tracks_view`, `probe_changes` and
        `probe_fnv` are statements about the realm bound to the output, and a
        caller comparing them against another realm's frame would be checking
        a number against the wrong picture. `None` when nothing is bound (the
        core sends `-`). Configuration, not a secret, so it does not weaken
        the report's secret-independence rule.

        `strip_h` and `strip_changes` (WS-E.2.3, issue #215) are the status
        strip's height in rows and how many composites repainted it. Both are
        `0` in every session that did not pass `--status`, which is the default
        and what this suite runs with -- so `tracks_view` compares exactly the
        rows it always did. They are appended AFTER the realm id rather than
        inserted among the counters, so every field position an older reader
        indexed is unchanged.

        Raises `InjectorFailed` if the core sent a descriptor with the reply:
        that would mean pixels travelled, which nothing about this request may
        ever cause.
        """
        fds_before = len(self._fds)
        self.sock.sendall(b"band\n")
        fields = self._next_reply("band", timeout)
        keys = (
            "composites",
            "band_changes",
            "probe_changes",
            "tracks_view",
            "band_uniform",
            "refusals",
            "band_h",
            "view_w",
            "view_h",
        )
        # verb + scalars + the hex digest + the bound realm's id + the two
        # WS-E.2.3 strip fields
        if len(fields) != len(keys) + 5:
            raise InjectorFailed(f"malformed `band` reply: {fields!r}")
        if len(self._fds) != fds_before:
            raise InjectorFailed(
                "the core sent a descriptor with a `band` reply; this request must never "
                "move pixels (issue #85: the indicator reaches no descriptor in any build)"
            )
        out: dict[str, object] = {
            key: int(value) for key, value in zip(keys, fields[1 : 1 + len(keys)])
        }
        # Indexed positionally rather than from the end: the realm id was
        # appended after the digest, so `fields[-1]` is no longer the digest.
        out["probe_fnv"] = int(fields[1 + len(keys)], 16)
        realm = fields[2 + len(keys)]
        out["realm"] = None if realm == "-" else realm
        out["strip_h"] = int(fields[3 + len(keys)])
        out["strip_changes"] = int(fields[4 + len(keys)])
        return out

    def decide(self, token: str, choice: str, timeout: float = 30.0) -> str:
        """Press a button on the prompt `token` names; return the ack word."""
        self.sock.sendall(f"decide {token} {choice}\n".encode())
        return self._next_reply("decided-ack", timeout)[1]

    def send_raw(self, raw: bytes) -> None:
        """Write arbitrary bytes -- for the fail-closed component tests."""
        self.sock.sendall(raw)

    def next_ack(self, timeout: float = 30.0) -> str:
        """The next ``decided-ack`` word, for callers that pipelined lines.

        Sending several `decide` lines in **one** write is how the component
        test pins the spent-token rule deterministically: the core drains the
        whole batch inside one `service_injector` call, before `post_dispatch`
        gets a chance to lower the card, so the second line is judged against
        a prompt that is provably still up.
        """
        return self._next_reply("decided-ack", timeout)[1]

    def close(self) -> None:
        for fd in self._fds:
            try:
                os.close(fd)
            except OSError:
                pass
        self._fds.clear()
        try:
            self.sock.close()
        except OSError:
            pass


class Core:
    """One `vitrind --headless` process, its runtime tree, and its log.

    Use as a context manager::

        with Core() as core:
            conn = core.connect()

    The runtime tree is a fresh temp directory per instance, which is what
    lets the two-cores test point a second core at the *same* tree
    deliberately rather than by accident.
    """

    def __init__(
        self,
        *,
        consent: str = "auto-approve",
        size: str = "320x200",
        animate: int = ANIMATE_FRAMES,
        seat: bool = False,
        runtime_dir: str | os.PathLike[str] | None = None,
        realms: tuple[str, ...] = (),
        templates: tuple[str, ...] = (),
        wait: bool = True,
        write_config: bool = True,
        shim: str | os.PathLike[str] | None = None,
        command: str | os.PathLike[str] | None = None,
        args: list[str] | None = None,
        realm_args: dict[str, list[str]] | None = None,
        realm_commands: dict[str, str | os.PathLike[str]] | None = None,
        env_allow: tuple[str, ...] = (),
        extra_env: dict[str, str] | None = None,
        log_file: str | os.PathLike[str] | None = None,
        capture_dump: str | os.PathLike[str] | None = None,
        consent_injector: bool = False,
        physical_input: bool = False,
        screenshot_dir: str | os.PathLike[str] | None = None,
        isolation: str | None = None,
        landlock: str | None = None,
        binds: tuple[str, ...] = (),
        realm_init: str | os.PathLike[str] | None = None,
    ) -> None:
        self.runtime = pathlib.Path(runtime_dir or tempfile.mkdtemp(prefix="vitrin-it-"))
        self._owns_runtime = runtime_dir is None
        self.recorder = self.runtime / "flight.jsonl"
        self.principals = self.runtime / "principals.toml"
        self.realm = self.runtime / "realm.toml"
        self.proc: subprocess.Popen[str] | None = None
        self._output = ""
        self._entries: list[dict] | None = None
        # Per-realm logs, cached at teardown for the same reason `_entries`
        # is: the files live inside the tree this object deletes.
        self._realm_logs: dict[str, str] = {}
        # A verbose realm (a probing shim under Firefox emits thousands of
        # DEBUG lines; the browser is chatty on its own) can write more to the
        # core's inherited stdout/stderr than a pipe holds -- and this harness
        # reads that pipe only *after* the process exits, so a run that fills
        # ~64 KiB before then would wedge the writer forever. `log_file`
        # redirects the child's combined output to a file instead of a pipe,
        # which cannot back-pressure; `output()` reads it back the same way.
        # Default (None) keeps the pipe every existing caller relies on.
        self._logf = open(log_file, "w+") if log_file is not None else None

        # The shim binary the core execs to hold fd 3 (issue #103). Default
        # is `vitrin-mock-shim`, which every existing test uses and which is
        # BOTH the fd-3 peer and the app stand-in it ignores. The real-app
        # gate (test_real_app.py) overrides it with the built C shim, whose
        # `command` names a genuine Wayland app the C shim fork/execs.
        shim_bin = pathlib.Path(shim) if shim is not None else MOCK_SHIM

        # The realm's read-only bind list (P2.6.2, #186): exactly what the
        # caller asked for, and nothing this harness added on its behalf.
        #
        # It held a second contribution until #283 -- the shim's own `ldd`
        # closure, bound read-only with `LD_LIBRARY_PATH` forwarded to make it
        # findable -- because a Meson build-tree shim linked against the
        # VENDORED wlroots carried the RUNPATH `$ORIGIN/subprojects/wlroots`
        # and died before `main` inside every realm. **That was a workaround
        # in the test harness for a defect in the shipped build, and it is
        # gone because the defect is:** shim/meson.build now links vendored
        # libraries statically, so the shim needs nothing outside the realm's
        # stock mount table. `meson test loader-independence` is what keeps it
        # that way, and the real-app gates below are what prove it end to end
        # -- if the RUNPATH ever comes back, they fail with `Broken pipe`
        # instead of passing over a harness that quietly widened the realm.
        realm_binds = tuple(binds)
        env_allow = tuple(env_allow)

        # `write_config=False` reuses whatever config is already in the tree.
        # The R6 test needs it: it deliberately relaxes the registry's mode
        # and then starts a core against it, which a rewrite-and-chmod on
        # every construction would silently undo — leaving a test that
        # asserts a refusal against a file that no longer deserves one.
        if write_config:
            self.principals.write_text(
                f'[[principal]]\nidentity = "{DEMO_IDENTITY}"\ntoken = "{DEMO_TOKEN}"\n'
            )
            # 0600 or the core refuses to read it at all: the registry holds
            # bearer tokens, and the R6 audit fails closed on any wider mode.
            self.principals.chmod(0o600)
            if command is None:
                # The mock path, unchanged: the mock shim is the realm's
                # `command` app stand-in (which it ignores) as well as the
                # `--shim` fd-3 peer. `--seat` (opt-in) mints the shim's
                # input-delivery object so routed seat events actually land
                # rather than dropping undelivered — what the #43 demo needs
                # to exercise the seat path. Default off, so every existing
                # caller's argv is unchanged.
                #
                # `templates` names realms declared `autostart = false`
                # (WS-E.1.1, issue #207): present in the registry, addressable
                # and petitionable, but never forked at startup. They exist to
                # be the subject of a `realm.launch` grant, and the core
                # refuses a file in which EVERY realm is one -- so `realm-0`
                # (written unconditionally, and never a template) is also what
                # keeps such a file loadable.
                #
                # `realms` names EXTRA realms beside `realm-0` (WS-E.1.2):
                # each becomes another `[[realm]]` table running the same
                # mock shim. Default empty, so every existing caller still
                # writes the single-table file it always did — which is the
                # test that this change was additive rather than merely
                # compatible-looking. `realm-0` is written unconditionally
                # because the core requires it in every configuration.
                seat_arg = ', "--seat"' if seat else ""
                tables = "".join(
                    "[[realm]]\n"
                    f'id = "{rid}"\n'
                    f'command = "{MOCK_SHIM}"\n'
                    f'args = ["--serve"{seat_arg}, "--animate", "{animate}"]\n'
                    f"binds = {_toml_string_array(list(realm_binds))}\n"
                    + ("autostart = false\n" if rid in templates else "")
                    for rid in ("realm-0", *realms, *templates)
                )
                self.realm.write_text(tables)
            else:
                # The real-app path: `command` names a genuine app the C shim
                # fork/execs after the `--` tail, and `env_allow` is the only
                # route the app's (and the shim's) environment is allowed to
                # grow by — the headless/software-render WLR_* names travel
                # this way, exactly as the c_shim conformance test threads
                # them (crates/vitrin-core/src/shim.rs).
                #
                # `realms` names EXTRA realms here too, each running the same
                # app under its own shim — what a real-app multi-realm gate
                # needs (the cross-realm actuation guard, WS-E.1.2). Default
                # empty, so every existing real-app caller still writes the
                # single table it always did.
                # `realm_args`/`realm_commands` override `args`/`command`
                # for the realms they name, so two realms can run the same
                # app with different arguments (two `solid-client`s painting
                # two colours) or two different apps entirely (a static one
                # beside an animating one). Both are what a cross-realm
                # capture test needs to tell one realm's pixels from
                # another's (WS-E.1.3, issue #209): with identical realms a
                # leak is invisible, because there is nothing to leak.
                # Unnamed realms take `command`/`args` unchanged, so every
                # existing caller is untouched.
                per_args = realm_args or {}
                per_command = realm_commands or {}
                self.realm.write_text(
                    "".join(
                        "[[realm]]\n"
                        f'id = "{rid}"\n'
                        "command = "
                        f"{_toml_string(os.fspath(per_command.get(rid, command)))}\n"
                        f"args = {_toml_string_array(per_args.get(rid, args or []))}\n"
                        f"env_allow = {_toml_string_array(list(env_allow))}\n"
                        f"binds = {_toml_string_array(list(realm_binds))}\n"
                        + ("autostart = false\n" if rid in templates else "")
                        for rid in ("realm-0", *realms, *templates)
                    )
                )

        argv = [
            # Empty unless `VITRIN_CORE_WRAPPER` is set; see
            # :data:`CORE_WRAPPER` for what may legally go in it.
            *CORE_WRAPPER,
            str(VITRIND),
            "--headless",
            "--size",
            size,
            f"--consent={consent}",
            "--principals",
            str(self.principals),
            "--realm",
            str(self.realm),
            "--recorder",
            str(self.recorder),
            # Since #103 the core execs a `--shim` binary (fd-3 holder) that
            # conveys the realm's `command` app in argv after `--`. The mock
            # shim is that binary by default; the real-app gate points it at
            # the built C shim, which in turn fork/execs the `command` app.
            "--shim",
            str(shim_bin),
        ]
        # `--isolation` (P2.6.2, #186, D-037(4)). Deliberately NOT passed
        # unless a caller names it: the flag's whole point is that omitting it
        # confines, so a harness that always spelled it out would stop testing
        # the path every deployment takes. `test_real_confinement.py` is the
        # one module that names both settings, and it names them in the same
        # run.
        if isolation is not None:
            argv += [f"--isolation={isolation}"]
        # `--landlock` (P2.6.3, #187), on the same rule as `--isolation`: NOT
        # passed unless a caller names it, so the path every deployment takes
        # (the default, `highest`) is the one the suite exercises. The only
        # caller that names it is `test_real_confinement.py`, which spends it
        # on `off` -- the positive control that turns "the ruleset denied
        # this" from an assertion into a measurement.
        #
        # `VITRIN_LANDLOCK` supplies that argument for every test that does
        # NOT name one (:data:`LANDLOCK_OVERRIDE`), which is what makes the
        # `--landlock=off` half of a published claim reproducible from this
        # repo rather than only from the machine it was first measured on.
        # `docs/book/src/limits.md` once cited this suite's GTK gates as
        # failing at the shipped default and passing at `--landlock=off`;
        # before this existed no gate in the real-app ladder could be run the
        # second way at all, so the citation named a run nobody could repeat.
        # That pair of claims is gone from the page (K9b, 2026-08-15: the gates
        # pass at the shipped default and the stale control column was deleted
        # rather than carried), and the knob outlives it as the instrument the
        # page's other control measurements are taken with.
        landlock = landlock if landlock is not None else LANDLOCK_OVERRIDE
        if landlock is not None:
            argv += [f"--landlock={landlock}"]
        # A substituted confinement helper, for the gate that proves a realm
        # the core cannot verify is REFUSED (`crates/vitrin-realm-init-fixtures`).
        if realm_init is not None:
            argv += ["--realm-init", str(realm_init)]
        # The P1.8.5 core-internal capture (issue #107): when set, the core
        # mirrors every live realm's composited realm-view readback to
        # `PATH.<realm-id>` as raw RGBA, the frame the fidelity gate compares
        # an `observe()` against. Readers must go through
        # `capture_dump_path()`; nothing is written to the bare PATH itself
        # (WS-E.1.3, issue #209 -- an unqualified dump would name *a* view the
        # moment a second realm exists, and a gate whose ground truth is a
        # guess is worse than no gate).
        if capture_dump is not None:
            argv += ["--capture-dump", str(capture_dump)]
        # The human's screenshot key (WS-E.2.4, issue #216). Absent = the chord
        # is still consumed and writes nothing; present = the core audits the
        # directory at startup and refuses to run if it is missing, a file, a
        # symlink, or writable by group or other.
        if screenshot_dir is not None:
            argv += ["--screenshot-dir", str(screenshot_dir)]
        # The consent-injector channel (issue #138): an inherited AF_UNIX
        # SOCK_STREAM socketpair, one end passed to the core by NUMBER on the
        # command line. Deliberately not a bound path plus a nonce -- the
        # confined realm runs as the core's own uid, so it could read both out
        # of `/proc`; a socketpair has no name to connect to and `spawn.rs`'s
        # post-fork `close_range(..., CLOSE_RANGE_CLOEXEC)` means neither the
        # shim nor the app holds it past `execve`. Authentication is
        # descriptor possession (crates/vitrin-core/src/consent/injector.rs).
        pass_fds: tuple[int, ...] = ()
        theirs: "socket_mod.socket | None" = None
        if consent_injector:
            ours, theirs = socket_mod.socketpair(socket_mod.AF_UNIX, socket_mod.SOCK_STREAM)
            # `pass_fds` keeps the descriptor number in the child, and the
            # pipes above are allocated first, so this is always >= 3 (which
            # the core also checks and refuses at startup).
            pass_fds = (theirs.fileno(),)
            argv += ["--consent-injector-fd", str(theirs.fileno())]
            self.injector: ConsentInjector | None = ConsentInjector(ours)
        else:
            self.injector = None
        self.injector_fd = pass_fds[0] if pass_fds else None
        # The physical-input channel (issue #212): a second inherited
        # socketpair, on exactly the terms of the first. Independent of it --
        # a gate wants one or the other, and combining them would make each
        # test carry machinery it does not use.
        physical_theirs: "socket_mod.socket | None" = None
        if physical_input:
            phys_ours, physical_theirs = socket_mod.socketpair(
                socket_mod.AF_UNIX, socket_mod.SOCK_STREAM
            )
            pass_fds = pass_fds + (physical_theirs.fileno(),)
            argv += ["--physical-input-fd", str(physical_theirs.fileno())]
            self.physical: PhysicalInputInjector | None = PhysicalInputInjector(phys_ours)
        else:
            self.physical = None
        env = {
            **os.environ,
            "XDG_RUNTIME_DIR": str(self.runtime),
            # Every confined realm gets private storage under
            # `$XDG_DATA_HOME/vitrin/realms/<id>`, injected as the app's `HOME`
            # (P2.6.2, #186). Pointed at this instance's throwaway tree, or the
            # suite would write into the developer's real
            # `~/.local/share/vitrin/realms`, carry one realm's files into the
            # next test that happens to use the same realm id, and journal
            # `storage_reused: true` on the second run of anything -- state
            # shared between tests through the operator's home directory is
            # exactly the kind of hidden coupling this suite exists to notice.
            "XDG_DATA_HOME": str(self.runtime / "data"),
            "RUST_LOG": "info",
        }
        # The core's own environment is the source `env_allow` copies from,
        # so the real-app gate seeds WLR_* here for the allowlist to forward.
        # `LD_LIBRARY_PATH` used to be seeded here too, to make #283's
        # workaround binds findable; it is not, and no realm's environment
        # carries it any more.
        if extra_env:
            env.update(extra_env)
        self.proc = subprocess.Popen(
            argv,
            env=env,
            stdout=self._logf if self._logf is not None else subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            pass_fds=pass_fds,
        )
        # Our copy of the core's end goes away immediately: with it open, a
        # core that died would leave the channel readable-but-never-EOF and a
        # waiting harness would hang instead of failing.
        if theirs is not None:
            theirs.close()
        if physical_theirs is not None:
            physical_theirs.close()
        if wait:
            self.await_socket()
            self.await_realms()

    # -- lifecycle ---------------------------------------------------------

    @property
    def socket(self) -> pathlib.Path:
        return self.runtime / "vitrin-0" / "core.sock"

    @property
    def pid(self) -> int:
        assert self.proc is not None
        return self.proc.pid

    def await_socket(self, timeout: float = BOOT_TIMEOUT_S) -> None:
        """Block until the core is serving, or explain why it never will be.

        Polls for the socket rather than sleeping a fixed interval: the boot
        path does an R6 registry audit, a realm load, a `flock`, a recorder
        create and a fork, and any of them can be slow on a loaded runner.
        A dead core short-circuits with its own stderr, because "connection
        refused" three lines later tells you nothing about a refused
        registry mode.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc is not None and self.proc.poll() is not None:
                raise CoreFailed(
                    f"core exited {self.proc.returncode} during boot:\n{self.output()}"
                )
            if self.socket.exists():
                return
            time.sleep(0.05)
        raise CoreFailed(f"core never bound {self.socket} within {timeout}s:\n{self.output()}")

    def autostart_realms(self) -> tuple[str, ...]:
        """The realm ids this session's config says startup must fork.

        Read back out of the `realm.toml` the core is pointed at rather than
        recomposed from this object's constructor arguments: it is the same
        file the core's loader reads, so the two cannot drift, and a caller
        that wrote its own tree (`write_config=False`) is covered by the same
        code path as one that let the harness write it.

        Templates (`autostart = false`, WS-E.1.1) are excluded, because
        `start_realm_in` skips them — waiting on one would wait forever.
        Returns `()` for a config this cannot read, which degrades
        :meth:`await_realms` to a no-op rather than to a spurious failure.
        """
        try:
            parsed = tomllib.loads(self.realm.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            return ()
        return tuple(
            table["id"]
            for table in parsed.get("realm", [])
            if "id" in table and table.get("autostart", True)
        )

    def await_realms(self, timeout: float = BOOT_TIMEOUT_S) -> None:
        """Block until every autostarting realm has been forked and journalled.

        **This is not redundant with :meth:`await_socket`, and the gap between
        them is a real one that cost a ~30%-per-run flake** (issue #292).
        `run_session` binds the core socket long before the backend exists;
        the realms are forked much later, one at a time, by
        `session::start_realm_in` running *between* `install()` and
        `event_loop.run()`. So the socket file appears first and the realm
        trees follow it over the next few milliseconds — measured here at
        +1.9 ms for the first realm and +8.4 ms (median) for the last of
        three, against `await_socket`'s 50 ms poll. A test that walked
        `$XDG_RUNTIME_DIR/vitrin-0/` the instant `await_socket` returned saw
        a half-populated tree roughly one run in four.

        **No conformant client can see that window**, which is why the fix is
        here and not in the core: the loop is not running yet, so the bound
        listener accepts nothing and connections merely queue in the backlog.
        Only a filesystem observer outside the protocol — this harness — can
        catch the session mid-startup. Moving the bind after the fork loop
        would trade this test-side race for a real one (a second core could
        take the socket meanwhile) and would break issue #77's trap T1, which
        requires `install()` before the fork.

        Waits on the **flight recorder**, deliberately, rather than on the
        realm directories: `spawn_realm` creates the tree and its `<id>.lock`
        and *then* journals `realm_spawned`, synchronously, in the parent. So
        the entry is strictly later than the tree, and a test can still assert
        on the tree afterwards without that assertion being a restatement of
        what was waited for. A realm that was journalled but got no directory
        still fails the test it should fail.
        """
        expected = set(self.autostart_realms())
        if not expected:
            return
        deadline = time.monotonic() + timeout
        seen: set[str] = set()
        while time.monotonic() < deadline:
            if self.proc is not None and self.proc.poll() is not None:
                raise CoreFailed(
                    f"core exited {self.proc.returncode} while forking its realms "
                    f"(journalled {sorted(seen)} of {sorted(expected)}):\n{self.output()}"
                )
            seen = {
                entry["realm"]
                for entry in self.entries()
                if entry["kind"] == "realm_spawned"
            }
            if expected <= seen:
                return
            time.sleep(0.02)
        raise CoreFailed(
            f"core never journalled a realm_spawned for {sorted(expected - seen)} "
            f"within {timeout}s:\n{self.output()}"
        )

    def await_exit(self, timeout: float = 30.0) -> int:
        assert self.proc is not None
        return self.proc.wait(timeout=timeout)

    def terminate(self) -> int:
        """Stop the core with SIGTERM — the ordinary way a session ends.

        Deliberately not SIGKILL: `SIGTERM` is the path that runs the
        shutdown ladder, and the teardown test below asserts on what that
        ladder writes to the log.
        """
        assert self.proc is not None
        # Before the signal, never after: an orderly shutdown deletes each
        # realm's runtime tree, and the realm's log lives in it.
        self._snapshot_realm_logs()
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=10)
        return self.proc.returncode

    def output(self) -> str:
        """Everything the core wrote, cached so it can be read more than once.

        Reads from the redirect file when one was given (`log_file`), else from
        the stdout pipe. Either way only after the core has exited — but note
        that "the core" is not every writer: the shims and the apps under them
        inherit this same stream and outlive the core by the moment it takes
        them to notice their connection died and print their own last lines.

        **Read through a SECOND, independent handle, never by seeking this
        one.** `self._logf` is the very file description the core, both shims
        and both apps write through, so a `seek(0)` on it moves the offset
        those still-running writers share: the next line an app printed landed
        at the top of the file and overwrote whatever was there. The symptom is
        as bad as it sounds and nothing like a truncation — the file keeps its
        length, the `SUMMARY` at the end is present, and one line from the
        MIDDLE of the run is simply gone. It was found that way, from a
        `test_real_gestures.py` run whose app counted a `swipe_update` it had
        printed and whose log did not contain the line. A fresh `open()` has
        its own file description and its own offset, so reading cannot touch
        what the writers are doing.
        """
        if not self._output and self.proc is not None and self.proc.poll() is not None:
            if self._logf is not None:
                self._logf.flush()
                with open(self._logf.name, errors="replace") as handle:
                    self._output = handle.read() or ""
            elif self.proc.stdout is not None:
                self._output = self.proc.stdout.read() or ""
        return self._output

    def realm_dir(self, realm: str = "realm-0") -> pathlib.Path:
        """A realm's private runtime directory, **on the host**.

        The same directory the realm sees at :data:`IN_REALM_RUNTIME_DIR` --
        it is bound there read-write -- so it is the one place a confined app
        can write a file this suite can then read. A gate that used to hand its
        app a path in a scratch `/tmp` directory now hands it
        `/run/vitrin/<name>` and reads `realm_dir(...)/<name>`: the realm's
        `/tmp` is a private tmpfs and a host scratch path simply does not
        exist in there.

        The core **purges** this directory at spawn, so a file a test wrote
        here beforehand is gone by the time the app runs -- which is what makes
        "the sink file is absent" evidence about the run rather than about the
        fixture.
        """
        return self.runtime / "vitrin-0" / realm

    def storage_dir(self, realm: str = "realm-0") -> pathlib.Path:
        """A realm's **persistent** private storage, on the host.

        The directory bound at :data:`IN_REALM_HOME` and injected as the app's
        `HOME` (P2.6.2, #186). Unlike :meth:`realm_dir`, it is *never purged*:
        the core removes the runtime tree on an orderly shutdown, so a file an
        app wrote in there is gone by the time a test that terminated the core
        goes looking for it. An app artefact a gate reads **after** the run
        belongs here; one it reads **during** the run may live in either.
        """
        return self.runtime / "data" / "vitrin" / "realms" / realm

    def realm_log_path(self, realm: str = "realm-0") -> pathlib.Path:
        """Where a **confined** realm's stdout and stderr go.

        At `--isolation=default` (P2.6.2, #186) the core gives each realm its
        own log file instead of letting the shim and the app inherit its
        descriptors: on a real session those would be the operator's tty, and
        no mount flag can revoke a descriptor. So an app's own diagnostics stop
        appearing in :meth:`output` the moment a realm is confined.
        """
        return self.realm_dir(realm) / "realm.log"

    def realm_output(self, realm: str = "realm-0") -> str:
        """A confined realm's own log, cached the way :meth:`output` is.

        Cached because the file lives *inside* the runtime tree this object
        deletes on cleanup, and the natural way to write these tests is to
        assert on an app's output after the core has been terminated.

        Empty string when there is no such file -- which is the
        `--isolation=off` case, where the realm inherits the core's stream and
        :meth:`output` is where its lines are.
        """
        cached = self._realm_logs.get(realm)
        if cached is not None:
            return cached
        try:
            # A fresh handle, never a seek on a descriptor the writers share:
            # the same lesson `output()` records at length.
            return self.realm_log_path(realm).read_text(errors="replace")
        except OSError:
            return ""

    def _snapshot_realm_logs(self) -> None:
        """Copy every realm's log into memory **before** the core stops.

        Not an optimisation -- the file will not be there afterwards. An
        orderly shutdown removes each realm's runtime tree
        (`lifecycle.rs::remove_runtime_dir`), which was invisible while a
        realm's diagnostics went to the core's inherited stdout and is not
        invisible now that they go to a file inside that tree. Called from
        :meth:`terminate`, so any caller that stops the core the ordinary way
        keeps its apps' lines whatever it reads them for.
        """
        tree = self.runtime / "vitrin-0"
        if not tree.is_dir():
            return
        for child in sorted(tree.iterdir()):
            if not child.is_dir() or child.name in self._realm_logs:
                continue
            try:
                self._realm_logs[child.name] = (child / "realm.log").read_text(errors="replace")
            except OSError:
                continue

    def app_output(self, realm: str = "realm-0") -> str:
        """Wherever **this** run's realm wrote its diagnostics.

        The per-realm log when the realm is confined, the core's own inherited
        stream when it is not. A gate reading an app's own lines wants this
        rather than :meth:`output`, and wants it rather than choosing for
        itself, because which of the two carries the app is a function of
        `--isolation` and a gate that hardcoded one would silently read an
        empty string at the other setting.

        Deliberately **not** folded into `output()`: a gate that greps the
        core's log for a string the realm must never have leaked (the
        clipboard canary, `test_real_clipboard.py`) would then be grepping the
        app's own log for the string the app itself was given, and would fail
        for the opposite of the reason it exists.
        """
        return self.realm_output(realm) or self.output()

    def realm_ids(self) -> list[str]:
        """Every realm id this run's runtime tree holds a directory for."""
        if self._realm_logs:
            return sorted(self._realm_logs)
        tree = self.runtime / "vitrin-0"
        if not tree.is_dir():
            return []
        return sorted(child.name for child in tree.iterdir() if child.is_dir())

    def all_app_output(self) -> str:
        """:meth:`app_output` over **every** realm, concatenated.

        What a multi-realm gate counting "one HIT per app" wants. Confinement
        split one stream into several files, and a gate that kept reading the
        core's own stream would count zero -- while a gate that read only
        `realm-0`'s would count one and call it two-realm evidence.
        """
        joined = "".join(self.realm_output(realm) for realm in self.realm_ids())
        return joined or self.output()

    # -- observation -------------------------------------------------------

    def entries(self) -> list[dict]:
        """The flight recorder's entries, parsed.

        Call after the core has exited: `run_ended` is written at shutdown,
        and a test reading mid-run sees a footer-less file.

        Cached on first read after exit, because the log lives *inside* the
        runtime tree this object deletes on cleanup — and the natural way to
        write these tests is to assert on the log after the `with` block, by
        which point the file is gone. Without the cache every such assertion
        would silently see an empty list and pass or fail for the wrong
        reason.
        """
        if self._entries is None:
            if not self.recorder.exists():
                return []
            parsed = [
                json.loads(line)
                for line in self.recorder.read_text().splitlines()
                if line.strip()
            ]
            # Only freeze once the run is over; a mid-run read is a
            # legitimate snapshot but must not become the cached answer.
            if self.proc is not None and self.proc.poll() is not None:
                self._entries = parsed
            return parsed
        return self._entries

    def kinds(self) -> list[str]:
        return [entry["kind"] for entry in self.entries()]

    def connect(self):
        """An SDK connection, bound as the demo principal."""
        import vitrin_os

        return vitrin_os.connect(
            str(self.socket), identity=DEMO_IDENTITY, credential=DEMO_TOKEN
        )

    # -- context manager ---------------------------------------------------

    def __enter__(self) -> "Core":
        return self

    def __exit__(self, *_exc: object) -> None:
        if self.injector is not None:
            self.injector.close()
            self.injector = None
        if self.physical is not None:
            self.physical.close()
            self.physical = None
        self.terminate()
        # Read the log before the tree it lives in goes away, so assertions
        # after the `with` block see the run rather than an empty list.
        self.entries()
        self.output()
        # Belt and braces: `terminate()` above already snapshotted, and this
        # catches a core that was never terminated at all. Skipping realms
        # already cached, so it can never overwrite a real log with the empty
        # string a deleted tree reads back as.
        self._snapshot_realm_logs()
        if self._logf is not None:
            self._logf.close()
            self._logf = None
        if self._owns_runtime:
            shutil.rmtree(self.runtime, ignore_errors=True)


class IntegrationTest(unittest.TestCase):
    """Base class: every test gets a hard deadline and leaves nothing running.

    Two failure modes this suite must not have, both learned rather than
    anticipated:

    - **Hangs.** A wedged shim makes `observe()` block forever. `SIGALRM`
      converts that into a named test failure in
      :data:`TEST_TIMEOUT_S` seconds instead of a nameless CI timeout ten
      minutes later.
    - **Leaks.** A test that fails between spawning a core and cleaning it
      up used to orphan a `vitrind` and its shim, which then kept composing
      — the next test would run alongside them. Registered cores are reaped
      in `tearDown` whatever the outcome.
    """

    def setUp(self) -> None:
        self._cores: list[Core] = []
        signal.signal(signal.SIGALRM, self._timed_out)
        signal.alarm(TEST_TIMEOUT_S)

    def tearDown(self) -> None:
        signal.alarm(0)
        for core in reversed(self._cores):
            try:
                core.__exit__()
            except Exception:  # cleanup must never mask the real failure
                pass

    def _timed_out(self, _signum: int, _frame: object) -> None:
        raise AssertionError(
            f"test exceeded {TEST_TIMEOUT_S}s. If this is `observe()` never returning, "
            "suspect issue #77's trap T1: the shim socketpair's event source registered "
            "after the fork leaves the shim blocked on `configure` forever."
        )

    def core(self, **kwargs: object) -> Core:
        """A core that is reaped when this test ends, pass or fail."""
        started = Core(**kwargs)  # type: ignore[arg-type]
        self._cores.append(started)
        return started


def children_of(pid: int) -> list[int]:
    """Direct children of `pid`, from procfs.

    `pstree` is not installed on every runner; `/proc/<pid>/task/*/children`
    is always there on Linux and is what `pstree` reads anyway.
    """
    kids: list[int] = []
    for task in pathlib.Path(f"/proc/{pid}/task").glob("*"):
        try:
            kids.extend(int(p) for p in (task / "children").read_text().split())
        except OSError:
            continue
    return kids


def comm_of(pid: int) -> str:
    """A pid's executable name, or '' if it is gone."""
    try:
        return pathlib.Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return ""


def shims_of(pid: int) -> list[int]:
    """Every realm shim the core `pid` owns, at **either** isolation setting.

    Until P2.6.2 (#186) this was one comprehension over `children_of(core.pid)`,
    written out at sixteen call sites, and every one of them silently returned
    `[]` the day `--isolation` began defaulting to `default`: the core's direct
    child became `vitrin-realm-init`, the host-pid-namespace supervisor, and
    the shim moved one level down as that supervisor's child (D-037(2) --
    `unshare(CLONE_NEWPID)` never moves the caller, so a fork is forced and the
    forker has to stay somewhere).

    Two levels, never a recursive walk. A realm's app is the shim's child and
    an app may fork children of its own; a DFS for "anything called
    vitrin-shim" would be no more correct and would stop distinguishing
    "the core forked a shim" from "something under the realm did".

    **A confined shim is matched by position, not by name, and that is not a
    shortcut.** `vitrin-realm-init` binds the shim at a core-chosen in-realm
    path, so the kernel derives its `comm` from *that* basename and every
    confined shim -- real C shim and `vitrin-mock-shim` alike -- reads back as
    the same name (`shim` until #283, `vitrin-shim` since). A name test
    there would either miss both or match anything; a supervisor has exactly
    one child and that child is the realm's PID 1, so its identity is
    unambiguous. What the *program* is stays checkable, and more sharply than
    a name ever was: :func:`exe_identity` compares the running image's inode
    against the shim binary the gate named.

    Returns host pids, in `children_of` order, so a caller that wants "one
    shim per realm, in realm order" must still sort or match by realm rather
    than by position -- exactly as before.
    """
    found: list[int] = []
    for kid in children_of(pid):
        name = comm_of(kid)
        if name.startswith(SHIM_COMMS):
            found.append(kid)  # --isolation=off: the core's own direct child
        elif name.startswith(SUPERVISOR_COMM):
            # The supervisor's one child, **once it has exec'd**. Between the
            # fork and the second `execve` that child is still running
            # `vitrin-realm-init` itself -- it is building the mount table --
            # and returning it then hands the caller a pid whose
            # `/proc/<pid>/exe` is the helper, not the shim. That race is not
            # theoretical: it turned up as `test_real_deadman.py` failing its
            # inode check with `comm 'vitrin-realm-in'` on one run in a
            # hundred. Filtering on the name the child sheds at `execve` makes
            # `await_shims` wait for the exec instead of racing it.
            found.extend(
                grandkid
                for grandkid in children_of(kid)
                if not comm_of(grandkid).startswith(SUPERVISOR_COMM)
            )
    return found


def exe_identity(pid: int) -> tuple[int, int] | None:
    """`(st_dev, st_ino)` of the program `pid` is running, or `None`.

    `/proc/<pid>/exe` is a magic link the kernel resolves from the process's
    own `mm->exe_file`, so a `stat` through it names the real inode even when
    the process lives in a mount namespace this reader cannot walk -- which is
    what makes it usable on a confined realm's shim, where `readlink` answers
    the in-realm path `/vitrin/vitrin-shim` and the *name* has stopped being
    evidence of anything.

    Comparing inodes is strictly stronger than the `comm` prefix test it
    replaced: a name says what a program is called, an inode says which file
    is executing. `vitrin-mock-shim` bound at the same in-realm path answers
    the same `comm` and a different inode.
    """
    try:
        st = os.stat(f"/proc/{pid}/exe")
    except OSError:
        return None
    return (st.st_dev, st.st_ino)


def file_identity(path: str | os.PathLike[str]) -> tuple[int, int] | None:
    """`(st_dev, st_ino)` of `path`, or `None` if it cannot be stat'ed."""
    try:
        st = os.stat(path)
    except OSError:
        return None
    return (st.st_dev, st.st_ino)


def await_shims(pid: int, count: int = 1, timeout: float = 15.0) -> list[int]:
    """Block until `pid` owns at least `count` shims; return what it owns.

    The spawn is asynchronous with respect to the socket appearing, so every
    gate that reads the spine used to open-code this poll. Returns whatever it
    has when the deadline passes, so the caller's own assertion produces the
    failure message rather than this helper inventing one.
    """
    deadline = time.monotonic() + timeout
    while True:
        found = shims_of(pid)
        if len(found) >= count or time.monotonic() >= deadline:
            return found
        time.sleep(0.05)


def descendant_named(pid: int, name: str, timeout: float = 15.0) -> int | None:
    """Wait for a descendant of `pid` whose `comm` matches `name`, DFS.

    The real spawn spine is core → C shim → app, so the app the test cares
    about is a *grand*child of the core, not a direct child. `comm` is
    truncated to 15 bytes by the kernel, so the match is a prefix test —
    `weston-terminal` arrives as `weston-terminal` (exactly 15) but a longer
    name would be clipped, and matching a prefix is what survives that.
    """
    deadline = time.monotonic() + timeout
    prefix = name[:15]
    while True:
        stack = list(children_of(pid))
        while stack:
            candidate = stack.pop()
            if comm_of(candidate).startswith(prefix):
                return candidate
            stack.extend(children_of(candidate))
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.05)


def environ_of(pid: int) -> dict[str, str]:
    """A pid's environment at exec time, parsed from `/proc/<pid>/environ`.

    NUL-separated `NAME=VALUE` records; a record without `=` (there is
    normally none) is skipped rather than guessed at.
    """
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
    except OSError:
        return {}
    env: dict[str, str] = {}
    for record in raw.split(b"\0"):
        if not record:
            continue
        name, sep, value = record.partition(b"=")
        if sep:
            env[name.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
    return env


def fd_targets_of(pid: int) -> dict[int, str]:
    """Every open descriptor of `pid` mapped to its `readlink` target.

    Used to prove the confined app holds no descriptor number 3 that the
    core's socketpair would occupy — the fd-3 leak is the whole confinement
    gone (`crates/vitrin-core/src/spawn.rs`).
    """
    targets: dict[int, str] = {}
    for entry in pathlib.Path(f"/proc/{pid}/fd").glob("*"):
        try:
            targets[int(entry.name)] = os.readlink(entry)
        except (OSError, ValueError):
            continue
    return targets


def require_binaries() -> None:
    missing = [str(p) for p in (VITRIND, MOCK_SHIM) if not p.is_file() or not os.access(p, os.X_OK)]
    if missing:
        sys.exit(
            "integration suite needs built binaries; missing: "
            + ", ".join(missing)
            + "\nrun `cargo build --workspace` (run.sh does this for you)"
        )


# -- shared agent-side helpers (used by more than one test module) ----------

ALL_VERBS = ("observe", "actuate.pointer", "actuate.text")


def capture_dump_path(base, realm: str = "realm-0"):
    """Where the core writes `realm`'s `--capture-dump` frame.

    `vitrind --capture-dump PATH` writes one file **per live realm**, at
    `PATH.<realm-id>`, and never PATH itself (WS-E.1.3, issue #209): while a
    session held one realm, PATH unambiguously named the one view the M1.3
    fidelity gate compares an agent's `observe()` against; with several it
    would name whichever realm the writer happened to mean. Every reader in
    this suite goes through here so the gate's ground truth stays a named
    realm rather than an assumption.
    """
    return pathlib.Path(f"{base}.{realm}")


def whole_realm_grant(conn, verbs=ALL_VERBS, realm="realm-0"):
    """Petition for the MVP's one grant shape and wait it out.

    `resource` is left empty deliberately: version 0 serves whole-realm grants
    only and refuses any finer granularity as `Unsupported` — an honest refusal
    rather than accepted-and-unenforced.

    `realm` defaults to the well-known one, so every existing caller is
    unchanged; a multi-realm test names another (WS-E.1.2). The realm the
    grant row carries is what the core judges that grant's *liveness* against,
    so "which realm" is not cosmetic here.
    """
    import vitrin_os

    return conn.request_grant(
        realm=realm, verbs=verbs, persistence=vitrin_os.Persistence.WHILE_RUNNING
    ).await_consent()


def capture_when_ready(grant, timeout=5.0, poll=0.02):
    """The first capture of a freshly-served realm, tolerating the startup race.

    A realm that has not yet committed and composited its first buffer has no
    surface, and the core answers `observe` with `NoSurface` — the honest reply
    the protocol prescribes, not a fault the agent could have prevented. So an
    agent's *first* `observe()` can lose a race with the realm's first frame:
    `await_consent()` returns the instant the grant resolves, which on a loaded
    machine is before the shim has drawn (seen on CI as a `NoSurface`). The poll
    model (D6, one fresh frame per call) is to retry, so this loops `observe()`
    until a frame lands or the deadline passes.

    A `NoSurface` refusal is judged *before* the token bucket (enforcement.rs),
    so a retry costs no rate budget; only the one admitted capture that returns
    consumes a token. That is why this doubles as the surface-ready barrier the
    actuation tests warm the realm with before a rate-sensitive burst.
    """
    from vitrin_os import errors

    deadline = time.monotonic() + timeout
    while True:
        try:
            return grant.observe()
        except errors.NoSurface:
            if time.monotonic() >= deadline:
                raise
            time.sleep(poll)


# -- captured-frame colour analysis (shared by every real-app gate) ---------
#
# The M1.2 render proof, in three layers used across the real-app ladder
# (test_real_app.py weston rung, test_real_gtk.py, test_real_firefox.py):
# `packed_pixels` turns a stride-generic wire frame into tight BGRX pixels,
# `has_real_content` is the weston/GTK "not the shim's empty fill" check, and
# `dominant_colour` is the Firefox "a known solid colour is on screen" check.


def _packed_pixels(frame) -> bytes:
    """`frame.raw` as tightly-packed 4-byte ``B,G,R,X`` pixels.

    Stride-generic per the IDL (row ``r`` begins at ``r * stride`` and carries
    ``width * 4`` payload bytes); a frame with ``stride > width * 4`` padding
    rows is repacked, the tight ``stride == width*4`` case (v1's pin) passes
    through. This is the one place the wire's row addressing is undone, so the
    two analyses below never re-derive it.
    """
    raw = frame.raw
    row_len = frame.width * 4
    if frame.stride == row_len:
        return raw
    return b"".join(
        raw[r * frame.stride : r * frame.stride + row_len] for r in range(frame.height)
    )


def colour_bytes(frame) -> bytes:
    """The colour channels of an xrgb8888 frame, padding byte stripped.

    Each 4-byte pixel is ``B, G, R, X`` (little-endian xrgb8888) with the
    ``X`` padding byte last. Dropping the padding plane is load-bearing: the C
    shim composites an **opaque** background whose padding plane is a constant
    ``0xFF`` even with no client buffer committed, so a check over *all* bytes
    reads ``{0x00, 0xFF}`` as "content" on an empty frame. The three colour
    planes concatenated carry only what a client actually painted.
    """
    packed = _packed_pixels(frame)
    return packed[0::4] + packed[1::4] + packed[2::4]


def has_real_content(frame) -> bool:
    """True once a frame carries real, non-uniform content, not an empty fill.

    Content-bearing iff the colour channels are both non-zero (some pixel is
    not black) and non-uniform (more than one colour value). The shim's opaque
    background and a toolkit's pre-chrome fill are each a single value and both
    fail this -- which is what makes "a real app frame reached the agent" a
    genuine claim rather than a pass on the shim's empty output.
    """
    colour = colour_bytes(frame)
    return bool(any(colour)) and len(set(colour)) > 1


#: Keep only a byte's top nibble -- a 4-bit-per-channel quantisation. It
#: matches mock_core.c's dominant-colour histogram, which is why the Firefox
#: acceptance pages use channel values that are multiples of 0x11 (they
#: survive it exactly, ``0xff -> 0xf0`` and back to ``0xff``).
_TOP_NIBBLE = bytes(i & 0xF0 for i in range(256))


def dominant_colour(frame) -> tuple[str, int]:
    """The most common colour of a captured frame as ``("rrggbb", percent)``.

    A 4-bit-per-channel histogram (mock_core.c's quantisation, so a page whose
    CSS colour has channels in multiples of 0x11 reads back as that literal
    colour with no tolerance): each pixel's ``R,G,B`` is reduced to its top
    nibble, the modal quantised colour wins, and its nibbles are expanded
    ``n -> n * 0x11`` into a ``rrggbb`` string. ``percent`` is that colour's
    share of the frame, floored -- the same "dominant over enough of the view
    to mean it" bar firefox_bringup.sh applies. This is the real-core render
    proof for Firefox: a local solid-colour page's colour dominating a real
    captured frame, distinct from the shim's black background and Firefox's
    grey chrome.
    """
    packed = _packed_pixels(frame)
    # Quantise each colour plane to its top nibble in one C-speed translate;
    # the padding plane (packed[3::4]) is never read.
    b = packed[0::4].translate(_TOP_NIBBLE)
    g = packed[1::4].translate(_TOP_NIBBLE)
    r = packed[2::4].translate(_TOP_NIBBLE)
    from collections import Counter

    counts = Counter(zip(r, g, b))
    (rq, gq, bq), top = counts.most_common(1)[0]
    total = len(r)
    # Expand each quantised channel (a multiple of 0x10) by nibble replication
    # (n -> n*0x11), the inverse the acceptance pages are authored against.
    hex6 = "".join(f"{(v >> 4) * 0x11:02x}" for v in (rq, gq, bq))
    return hex6, top * 100 // total


# -- the P1.8.5 SSIM bridge (issue #107) ------------------------------------


def packed_xrgb(frame) -> bytes:
    """A captured frame's pixels as tight ``width*height*4`` xrgb bytes.

    Undoes any stride padding (:func:`_packed_pixels`) so the bytes handed to
    the compare tool are the well-formed frame it expects — the same repack the
    colour analyses use, kept in one place so the fidelity gate's frame and the
    dominant-colour check can never disagree about what the frame *is*.
    """
    return _packed_pixels(frame)


def golden_cmp(
    actual_path,
    expected_path,
    size,
    policy,
    *,
    actual_format="xrgb",
    expected_format="rgba",
    artifacts=None,
):
    """Score two raw frame files with the compiled ``vitrin-golden-cmp`` tool.

    The P1.8.5 fidelity gate (issue #107) proves an agent's ``observe()`` frame
    agrees with the core-internal capture (``vitrind --capture-dump``) by SSIM,
    and the SSIM lives in the Rust ``vitrin-golden`` harness — the one the whole
    repo scores goldens with. Rather than re-port it into this dependency-free
    suite (a second definition that could silently drift, plan risk R7), the
    gate shells out to the real thing here.

    `size` is `(width, height)`; `policy` is the tool's policy string
    (`"exact"`, `"ssim:0.98"`, `"tol:2,0.01"`). Returns the completed process so
    a caller can assert on its exit code (0 passed, 1 did not, 2 error) and read
    the printed report. `artifacts`, when given, is a directory the tool drops
    actual/expected/diff PNGs into on a failing comparison.
    """
    w, h = size
    cmd = [
        str(GOLDEN_CMP),
        str(actual_path),
        str(expected_path),
        f"{w}x{h}",
        "--actual-format",
        actual_format,
        "--expected-format",
        expected_format,
        "--policy",
        policy,
    ]
    if artifacts is not None:
        cmd += ["--artifacts", str(artifacts)]
    return subprocess.run(cmd, capture_output=True, text=True)


# -- the P1.8.6 actuation gate (issue #108) ---------------------------------


def locate_colour(frame, hex6: str):
    """Centroid ``(cx, cy)`` and pixel count of a quantised colour in a frame.

    Quantises each pixel to 4 bits/channel (:func:`dominant_colour`'s histogram)
    and returns the centroid of every pixel whose quantised colour equals
    ``hex6`` — an ``"rrggbb"`` whose channels are multiples of ``0x11`` — plus
    how many matched. ``(None, None, 0)`` when the colour is absent. This is
    what the D10 pointer gate clicks: the agent locates a feature by its known
    colour in its *own* captured frame, and clicks the middle of it.
    """
    packed = _packed_pixels(frame)
    b = packed[0::4].translate(_TOP_NIBBLE)
    g = packed[1::4].translate(_TOP_NIBBLE)
    r = packed[2::4].translate(_TOP_NIBBLE)
    tr = int(hex6[0:2], 16) & 0xF0
    tg = int(hex6[2:4], 16) & 0xF0
    tb = int(hex6[4:6], 16) & 0xF0
    width = frame.width
    sum_x = sum_y = count = 0
    for i in range(len(r)):
        if r[i] == tr and g[i] == tg and b[i] == tb:
            sum_x += i % width
            sum_y += i // width
            count += 1
    if count == 0:
        return None, None, 0
    return sum_x // count, sum_y // count, count


# -- the P1.7.6 trusted-band witness (issue #139) ---------------------------

#: FNV-1a-64, the digest `crates/vitrin-core/src/backend/band_witness.rs`
#: reports its probe strip under. Reimplemented here rather than shared,
#: because this suite takes no dependency (D8) and the Rust side takes none
#: either -- so the check that the two agree is a check that two independent
#: readers of the same bytes reach the same number, which is exactly what the
#: gate wants it for. Nothing about it is a security property: it covers
#: realm-view pixels an observe grant may capture anyway.
_FNV_OFFSET = 0xCBF29CE484222325
_FNV_PRIME = 0x00000100000001B3
_U64 = (1 << 64) - 1


def fnv1a64(data: bytes) -> int:
    """FNV-1a-64 over `data`, matching `band_witness.rs`'s `fnv1a64`."""
    hashed = _FNV_OFFSET
    for byte in data:
        hashed = ((hashed ^ byte) * _FNV_PRIME) & _U64
    return hashed


def count_changed_pixels(before, after) -> int:
    """How many pixels differ in colour between two same-size frames.

    Compares the three colour channels (padding ignored) pixel by pixel. Used
    by the D7 text gate to assert an *observable* change — a typed string
    rendered into a real text field moves hundreds to thousands of glyph
    pixels, far above the handful a focus caret alone would, so a threshold on
    this count distinguishes "the text landed and was drawn" from incidental
    noise. Raises if the frames are not the same size (a mismatch is a bug to
    surface, not to paper over).
    """
    pa = _packed_pixels(before)
    pb = _packed_pixels(after)
    if len(pa) != len(pb):
        raise ValueError(f"frame size mismatch: {len(pa)} vs {len(pb)} bytes")
    changed = 0
    for i in range(0, len(pa), 4):
        if pa[i : i + 3] != pb[i : i + 3]:
            changed += 1
    return changed
