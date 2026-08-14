# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.3 (#209): **a capture returns the granted realm's pixels, never the
output's** — proved against two real apps in two real realms.

This is the mock-free half of issue #209's confidentiality property. The
in-crate tests (`session.rs`'s
`a_capture_returns_the_granted_realms_pixels_and_never_the_outputs`,
`scene/realms.rs`) pin the selection against the runtime and the scene set;
this drives the shipped `vitrind`, two real `vitrin-shim` processes and two
real Wayland clients over a real socket, and asks the two agents what they
actually received.

## What the property is, and why one realm could never express it

Until WS-E.1.2 a session held one realm, so one scene holding one committed
surface was a complete model and every capture was trivially "the realm's
own". Raising the cap to 16 left that model in place for one workstream task,
which made a **confidentiality** defect real: with one scene, the last realm
to commit owned the only committed surface, so an `observe` grant over realm A
returned realm B's pixels the instant B painted. Nothing in that code was
wrong — there was one realm — and nothing in it prevented the leak either.

So the fixture is two `solid-client`s painting **different** colours. Each
agent holds an `observe` grant over one realm, and:

1. the two `observe()` frames differ, and each one's dominant colour is the
   colour *its own* realm's app was told to paint;
2. each agent's frame agrees with **its own realm's** `--capture-dump`
   through the M1.3 gate's own comparator, rather than a second, driftable
   byte compare;
3. the frames are *cross-checked*: neither agent's frame matches the other
   realm's dump. That is the assertion a leak fails, and it is the reason (1)
   and (2) are not enough on their own — a core that served the output's
   frame to everyone would satisfy neither, but a core that served it to the
   *hidden* realm only would satisfy (1) for the bound realm and could still
   be caught only here.

`realm-0` sorts first, so it is the realm the output binds to and `second` is
the hidden one. The test deliberately does not assert *which* realm is on
screen — that is a binding placeholder WS-E.1.4 owns, and "a human sees the
right realm" needs an eye and a display (`shim/docs/nested-multi-realm.md`).
What it asserts is that the hidden realm's agent still gets the hidden realm's
pixels, which is the whole point.

## The second property: a hidden realm keeps painting

`RealTwoRealmsHiddenKeepsPainting` boots a **static** `solid-client` in the
bound realm and an **animating** `damage-client` in the hidden one. The
asymmetry is the whole design:

- `damage-client` draws *only on frame callbacks*, so it is the paced case. A
  realm that stopped receiving `frame_done` would publish its first frame and
  then freeze, and its capture would be a stale frame — which
  `refusal.no_surface` forbids in as many words ("never a stale frame"). The
  hidden realm's `observe()` frames must therefore change across a window.
- The bound realm is a solid colour that never animates, so its capture must
  *not* change. That is what makes the first assertion non-vacuous: if the
  hidden realm's agent were being served the output's frame, its capture would
  be the static one and could not have changed. Two realms running the same
  animating app would have failed to distinguish those cases — their frames
  coincide by construction on the first commit.

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
    dominant_colour,
    golden_cmp,
    packed_xrgb,
    shims_of,
    whole_realm_grant,
)

from vitrin_os import errors  # noqa: E402

#: The realm view size, hence every captured frame's size. Matches the M1.3
#: fidelity gate's, so the dump comparison below uses the same geometry the
#: comparator is already exercised at.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: The two apps' colours. Each channel is a multiple of 0x11 so the 4-bit
#: dominant-colour histogram reads them back exactly (the discipline
#: `test_real_capture_fidelity.py` sets), and the two are maximally far apart
#: so a frame carrying the wrong realm's pixels cannot be mistaken for noise.
BOUND_COLOUR = "0000ff"
HIDDEN_COLOUR = "ff0000"

#: The hidden realm's id. Sorts *after* `realm-0`, so `realm-0` is the realm
#: the output binds to (the first to attach, in id order) and this one is
#: hidden — which is the case the leak lived in.
HIDDEN = "second"

#: How long the clients stay up: they must outlive the capture loops below,
#: well inside the harness's 90 s SIGALRM budget.
RUN_MS = "40000"

#: The headless/software-render selectors the C shim's wlroots backend needs
#: (CI has no GPU), forwarded the one legal way a realm's environment grows.
WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


def _resolve_app(shim_bin: pathlib.Path, name: str, env_var: str) -> str | None:
    """A shim-sibling test client, or None if this machine has none built.

    Both `solid-client` and `damage-client` are co-built with the shim
    unconditionally (`shim/meson.build`), so absence beside a built shim is a
    build misconfiguration the caller turns into a failure rather than a skip.
    """
    explicit = os.environ.get(env_var)
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / name
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


class _TwoRealmBase(IntegrationTest):
    """Shared boot: the C-shim contract, a scratch tree, and two realms."""

    #: Subclass hook: the app binary's name and its `VITRIN_*_APP` override.
    APP_NAME = "solid-client"
    APP_ENV = "VITRIN_SOLID_APP"

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

        app = _resolve_app(self.shim_bin, self.APP_NAME, self.APP_ENV)
        if app is None:
            self.fail(
                f"no {self.APP_NAME} beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); its "
                f"absence is a build misconfiguration. Rebuild the shim, or set {self.APP_ENV}."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-two-realms-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    def _two_live_apps(self, core, name: str, expect: tuple[str, ...] | None = None) -> None:
        """Both realms really are running an app before anything is captured.

        Load-bearing: a capture assertion about "the other realm's pixels" is
        only evidence if there is another realm with pixels of its own.
        `expect`, when given, additionally requires the two apps to be those
        distinct binaries — the two-different-apps fixture is what makes the
        pacing test's counterweight mean anything.
        """
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
        wanted = expect if expect is not None else (name, name)
        seen: list[str] = []
        while time.monotonic() < deadline:
            seen = [
                comm_of(kid)
                for shim in shims
                for kid in children_of(shim)
                if any(comm_of(kid).startswith(w) for w in wanted)
            ]
            if len(seen) >= 2 and all(
                any(c.startswith(w) for c in seen) for w in set(wanted)
            ):
                break
            time.sleep(0.05)
        self.assertEqual(
            len(seen),
            2,
            f"each shim must have fork/exec'd its own app from {wanted}; got {seen}",
        )
        for want in set(wanted):
            self.assertTrue(
                any(c.startswith(want) for c in seen),
                f"no realm is running {want}; the fixture's two apps are {seen}",
            )

    def _observe_until_painted(self, grant, timeout=25.0):
        """`observe()` until the app has actually painted; return the frame.

        "Painted" means the frame's dominant colour is no longer the core's
        deterministic no-client background — polled rather than slept on,
        because a real app's first commit has no fixed latency.
        """
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
            colour, share = dominant_colour(frame)
            last = (colour, share)
            # `share` is an integer percent (harness.dominant_colour).
            if share >= 90 and colour in (BOUND_COLOUR, HIDDEN_COLOUR):
                return frame
            time.sleep(0.1)
        self.fail(f"no realm ever painted a solid frame; last dominant colour was {last}")

    def _await_dump(self, path, timeout=20.0) -> bytes:
        """Block until this realm's `--capture-dump` file is whole, and read it.

        The core writes one dump per realm at `PATH.<realm-id>` (WS-E.1.3) and
        nothing at all to the bare PATH; a reader that went to the bare path
        would wait forever, which is exactly the ambiguity the suffix removes.
        Writes are atomic (temp + rename), so a whole-size file is a whole
        frame.
        """
        expected = REALM_WH[0] * REALM_WH[1] * 4
        deadline = time.monotonic() + timeout
        p = pathlib.Path(path)
        while time.monotonic() < deadline:
            if p.is_file() and p.stat().st_size == expected:
                return p.read_bytes()
            time.sleep(0.05)
        size = p.stat().st_size if p.is_file() else "absent"
        self.fail(
            f"the core-internal capture at {path} never reached {expected} bytes (size: "
            f"{size}); `--capture-dump` did not write this realm's composited readback"
        )
        raise AssertionError("unreachable")  # pragma: no cover


class RealTwoRealmsCapture(_TwoRealmBase):
    """Each agent observes its own realm, byte-for-byte, and never a sibling's."""

    def test_each_realms_observe_is_its_own_realms_pixels(self):
        dump_base = str(self.work / "internal.rgba")
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS, "--colour", BOUND_COLOUR],
            realms=(HIDDEN,),
            # The one variable this test has: same app, same everything, two
            # colours. Without per-realm args both realms would paint the same
            # pixels and a leak would be invisible.
            realm_args={
                "realm-0": ["--run-ms", RUN_MS, "--colour", BOUND_COLOUR],
                HIDDEN: ["--run-ms", RUN_MS, "--colour", HIDDEN_COLOUR],
            },
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            capture_dump=dump_base,
            log_file=str(self.work / "core.log"),
        )
        self._two_live_apps(core, self.APP_NAME)

        conn = core.connect()
        bound = whole_realm_grant(conn, realm="realm-0")
        hidden = whole_realm_grant(conn, realm=HIDDEN)

        bound_frame = self._observe_until_painted(bound)
        hidden_frame = self._observe_until_painted(hidden)

        # (1) Two agents, two different pictures, each its own realm's colour.
        bound_colour, bound_share = dominant_colour(bound_frame)
        hidden_colour, hidden_share = dominant_colour(hidden_frame)
        self.assertEqual(
            bound_colour,
            BOUND_COLOUR,
            f"the grant over realm-0 must observe realm-0's app "
            f"(got #{bound_colour} over {bound_share}% of the frame)",
        )
        self.assertEqual(
            hidden_colour,
            HIDDEN_COLOUR,
            f"the grant over {HIDDEN} must observe {HIDDEN}'s app, not whatever is on the "
            f"output (got #{hidden_colour} over {hidden_share}% of the frame) -- this is "
            "the cross-realm capture leak WS-E.1.3 closes",
        )
        self.assertNotEqual(
            packed_xrgb(bound_frame),
            packed_xrgb(hidden_frame),
            "two realms running two different apps must not deliver one frame to both agents",
        )

        # (2) + (3) Each agent's frame agrees with ITS OWN realm's dump and
        # disagrees with the sibling's, through the M1.3 gate's comparator.
        dumps = {
            "realm-0": self._await_dump(capture_dump_path(dump_base, "realm-0")),
            HIDDEN: self._await_dump(capture_dump_path(dump_base, HIDDEN)),
        }
        self.assertNotEqual(
            dumps["realm-0"],
            dumps[HIDDEN],
            "the core wrote one realm's frame to both dump files; the dumps cannot "
            "distinguish anything and neither can this gate",
        )
        for realm, frame in (("realm-0", bound_frame), (HIDDEN, hidden_frame)):
            other = HIDDEN if realm == "realm-0" else "realm-0"
            agent_path = self.work / f"agent-{realm}.xrgb"
            agent_path.write_bytes(packed_xrgb(frame))
            own = self.work / f"dump-{realm}.rgba"
            own.write_bytes(dumps[realm])
            match = golden_cmp(
                str(agent_path),
                str(own),
                REALM_WH,
                "tol:1,0.001",
                artifacts=str(self.work / f"cmp-{realm}"),
            )
            self.assertEqual(
                match.returncode,
                0,
                f"the agent's frame for {realm} must agree with {realm}'s own "
                f"--capture-dump:\n{match.stdout.strip()}\n{match.stderr.strip()}",
            )
            # ...and the cross check, which is the one a leak fails.
            sibling = self.work / f"dump-{other}.rgba"
            sibling.write_bytes(dumps[other])
            cross = golden_cmp(
                str(agent_path),
                str(sibling),
                REALM_WH,
                "tol:1,0.001",
            )
            self.assertNotEqual(
                cross.returncode,
                0,
                f"the agent's frame for {realm} also matched {other}'s dump: the two realms "
                "are serving one picture, which is the leak this gate exists for",
            )


class RealTwoRealmsHiddenKeepsPainting(_TwoRealmBase):
    """A realm nobody is looking at keeps being paced, so its capture stays live."""

    def setUp(self) -> None:
        super().setUp()
        # The hidden realm's app, resolved beside the shim exactly as the
        # bound realm's is.
        animating = _resolve_app(self.shim_bin, "damage-client", "VITRIN_DAMAGE_APP")
        if animating is None:
            self.fail(
                f"no damage-client beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration. Rebuild the shim, or set "
                "VITRIN_DAMAGE_APP."
            )
        self.animating_bin = str(pathlib.Path(animating).resolve())

    def test_the_hidden_realms_capture_changes_across_a_window(self):
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS, "--colour", BOUND_COLOUR],
            realms=(HIDDEN,),
            # Static on the output, animating off it. No `--free-run` for the
            # animating one: it draws only on frame callbacks, so what is
            # measured below is pacing rather than a client that would
            # repaint regardless.
            realm_commands={HIDDEN: self.animating_bin},
            realm_args={
                "realm-0": ["--run-ms", RUN_MS, "--colour", BOUND_COLOUR],
                HIDDEN: ["--run-ms", RUN_MS],
            },
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            log_file=str(self.work / "core.log"),
        )
        # Two shims, and the two apps really are the two different binaries.
        self._two_live_apps(core, "", expect=(self.APP_NAME, "damage-client"))

        conn = core.connect()
        grants = {
            "realm-0": whole_realm_grant(conn, realm="realm-0"),
            HIDDEN: whole_realm_grant(conn, realm=HIDDEN),
        }

        def sample(grant, timeout=25.0):
            """One `observe()`, as the SDK frame — bytes and colour from one
            capture, so the two analyses can never be about two instants."""
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                try:
                    return grant.observe()
                except errors.NoSurface:
                    time.sleep(0.05)
                except errors.RateLimited as rl:
                    time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
            self.fail("a live realm never served a frame")

        # Wait until the bound realm is actually painting its solid colour,
        # so "the bound realm's capture never changed" below is a statement
        # about a live static app rather than about a realm that had not
        # started yet.
        self._observe_until_painted(grants["realm-0"])
        first = {realm: packed_xrgb(sample(grant)) for realm, grant in grants.items()}

        moved: set[str] = set()
        last_hidden = None
        deadline = time.monotonic() + 25.0
        while time.monotonic() < deadline and HIDDEN not in moved:
            for realm, grant in grants.items():
                frame = sample(grant)
                if realm == HIDDEN:
                    last_hidden = frame
                if packed_xrgb(frame) != first[realm]:
                    moved.add(realm)
            time.sleep(0.15)

        self.assertIn(
            HIDDEN,
            moved,
            f"the hidden realm ({HIDDEN}) never repainted: its shim paces on frame_done, so "
            "a capture of it is a stale frame -- which refusal.no_surface forbids outright, "
            "and which is exactly what WS-E.1.3 decision 2 refuses to ship",
        )
        # The non-vacuity counterweight: the realm ON the output is a static
        # solid colour and did not move. So the hidden realm's capture that
        # DID move cannot have been the output's frame -- which is the way
        # this assertion would otherwise pass while the leak was live.
        self.assertNotIn(
            "realm-0",
            moved,
            "the bound realm's static solid-client capture changed, so 'the hidden realm's "
            "capture changed' says nothing about whose frame either agent is being served",
        )
        # ...and the two agents are plainly looking at two different things:
        # the hidden realm's frame is the animating client's, not a wall of
        # the bound realm's solid colour.
        colour, share = dominant_colour(last_hidden if last_hidden else sample(grants[HIDDEN]))
        self.assertFalse(
            colour == BOUND_COLOUR and share >= 90,
            f"the hidden realm's agent is being served the bound realm's solid "
            f"#{BOUND_COLOUR} (#{colour} over {share}% of the frame)",
        )


if __name__ == "__main__":
    import unittest

    unittest.main()
