"""Issue #109 (M1.4 exit gate, the hold-Esc half): a completed dead-man chord,
applied over a REAL app through the REAL core, really revokes every grant and
really stops the app receiving seat events -- no mock DoD, no mock shim.

`test_real_actuation.py` (P1.8.6) proved a grant's pointer/text authority
reaches a real `click-target` through the real chokepoint. This closes the
other side of M1.4: that the session's off-switch (P1.7.3's `DeadManSwitch`,
wired live over the nested backend since the M1.1 runtime landing) really
revokes that authority when it fires, against the real
`GrantTable`/`PetitionRegistry` a real app's realm lives in.

# Why this is a signal, not a held key (the test-gated injector, issue #109)

Headless has no physical input device, structurally -- there is nothing to
hold a chord on (`crates/vitrin-core/src/deadman.rs`'s module docs) -- so a
human hold cannot be reproduced on a GPU-less CI runner. CI's stand-in is a
`SIGUSR1` sent to the core process, which a `dead-man-injector`-feature build
of `vitrind` turns into a synthesized [`crate::deadman::Trigger`] applied
through the *exact same* `Runtime::apply_dead_man` entry point a completed
physical hold reaches over the nested backend
(`crate::backend::winit::DeadManHost::on_trigger`). It is not a second,
weaker revocation path: it is the one real entry point, fed by a signal
instead of a timer. What it cannot prove -- and only nested mode, with a
human at a keyboard, can -- is that a real held key reaches that entry point
in the first place; see `shim/docs/firefox.md` for the nested recipe that
holds that other half.

**The feature is never enabled in a deployment build** (`Cargo.toml`'s
`dead-man-injector`, same posture as `scripted-consent`): a plain build has
no SIGUSR1 handler installed at all, so this signal takes its *default*
disposition there -- terminate the process. `tests/integration/run.sh` builds
`vitrind` with the feature for exactly this suite; this test tells that
failure mode apart from a real revocation (see `_send_dead_man_signal`)
rather than reporting a bare hang or a misattributed crash.

# What this proves, end to end, against `click-target`

1. **Before the chord**: the agent's grant is live -- it can `observe()` the
   app's real surface (a real `click-target`, not a mock).
2. **The chord fires** (SIGUSR1 stands in for the hold, above).
3. **After the chord**: `grant.observe()` *and* `grant.pointer.click()`
   -- capture and actuation, the whole D-series verb set the enforcement
   chokepoint gates -- both refuse `Revoked`, on the very next check
   (`enforcement.rs`'s single chokepoint, no separate "revocation applied"
   round-trip to wait out).
4. **The real app never saw the click.** `click-target` repaints its whole
   surface from green to red the instant a click lands on it (D10, the same
   feature `test_real_actuation.py` clicks) -- a real, unfakeable side
   effect. A revoked click that never reached the shim's `wl_seat` leaves the
   target exactly as green as it was before, so this checks the **core's own
   internal capture** (`--capture-dump`, bypassing every grant, wire frame
   and SDK decode) rather than a grant-gated `observe()` that the revocation
   itself has now foreclosed.
5. **The journal says why.** After the core exits, its flight recorder holds
   a `dead_man_triggered` entry (the cause) followed by `grant_revoked`
   entries naming every row the chord actually revoked (`deadman::apply`'s
   documented write order).

# Skip-or-fail policy (matches the real-app ladder)

- `VITRIN_SKIP_REAL_APP=1` -> skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim or `click-target` is missing ->
  fail (both co-built by the shim's Meson build, exactly as
  `test_real_actuation.py` treats them).
- The core dying instead of revoking on `SIGUSR1` -> fail, naming the
  `dead-man-injector` feature as the likely cause (see module docs above),
  never a silent skip: a real run was requested by setting `VITRIN_C_SHIM_BIN`.
"""

from __future__ import annotations

import os
import pathlib
import signal
import tempfile
import time
import unittest

from harness import (
    IntegrationTest,
    children_of,
    comm_of,
    descendant_named,
    locate_colour,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

#: Same realm shape as the actuation gate, so a `click-target` centroid found
#: here means exactly what it means there.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: How much green must be on screen before the agent trusts it located the
#: target, and (post-revocation) before it trusts the target is UNCHANGED.
#: Same bar `test_real_actuation.py` uses for the same 160x160 square.
MIN_TARGET_PIXELS = 5000


def _resolve_sibling(shim_bin: pathlib.Path, name: str, env_override: str) -> str | None:
    """A tool built beside the C shim, or an explicit override, or None."""
    explicit = os.environ.get(env_override)
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / name
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


def _require_shim(test: IntegrationTest) -> pathlib.Path:
    """The shared shim resolution + skip-or-fail preamble (matches the rest of
    the real-app ladder, e.g. `test_real_actuation.py::_require_shim`)."""
    if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
        test.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")
    shim = os.environ.get("VITRIN_C_SHIM_BIN")
    if not shim:
        test.skipTest(
            "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
            "Build it (meson setup shim/build shim && meson compile -C shim/build) and point "
            "the variable at shim/build/vitrin-shim. CI sets it."
        )
    shim_bin = pathlib.Path(shim)
    if not (shim_bin.is_file() and os.access(shim_bin, os.X_OK)):
        test.fail(
            f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a "
            "real run was requested; refusing to skip a requested gate (CI misconfig)."
        )
    return shim_bin


# -- reading the core-internal capture (`--capture-dump`) directly ----------
#
# `--capture-dump` mirrors the retained REALM-VIEW readback (never the
# human-visible/overlay one -- `crates/vitrin-core/src/backend/headless.rs`'s
# module docs) as tightly packed RGBA8888 straight to a file, bypassing every
# grant, the wire, and the SDK's frame decode entirely. That is exactly what
# step 4 above needs: a way to ask "did the app's surface change?" that a
# revoked grant cannot foreclose, because it never goes through one.


class _DumpFrame:
    """A `--capture-dump` RGBA readback, repacked as the harness's BGRX
    (xrgb8888) `Frame` shape so `harness.locate_colour` -- written once,
    against the wire frame format -- scores the core-internal capture too,
    rather than a second colour-analysis implementation (plan risk R7;
    `test_real_capture_fidelity.py`'s SSIM bridge makes the same call).
    """

    def __init__(self, rgba: bytes, width: int, height: int) -> None:
        bgrx = bytearray(len(rgba))
        bgrx[0::4] = rgba[2::4]  # B <- RGBA's B
        bgrx[1::4] = rgba[1::4]  # G <- RGBA's G
        bgrx[2::4] = rgba[0::4]  # R <- RGBA's R
        bgrx[3::4] = b"\xff" * (len(rgba) // 4)  # X: never read; keep opaque
        self.raw = bytes(bgrx)
        self.width = width
        self.height = height
        self.stride = width * 4


def _read_dump(path: str, size: tuple[int, int]) -> _DumpFrame:
    """Block until `--capture-dump`'s atomic temp+rename has produced a
    whole frame, then return it. `test_real_capture_fidelity.py`'s
    `_await_dump` proved the same wait is needed the first time the file
    appears; this additionally tolerates the file existing but momentarily
    short mid-rename on a slow runner, by retrying rather than failing on the
    first read.
    """
    width, height = size
    expected = width * height * 4
    p = pathlib.Path(path)
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        if p.is_file():
            data = p.read_bytes()
            if len(data) == expected:
                return _DumpFrame(data, width, height)
        time.sleep(0.05)
    size_now = p.stat().st_size if p.is_file() else "absent"
    raise AssertionError(
        f"the core-internal capture at {path} never reached {expected} bytes "
        f"(size: {size_now}); `--capture-dump` did not write the composited readback"
    )


class RealDeadManRevocation(IntegrationTest):
    """The M1.4 dead-man exit gate: a completed chord over a real app really
    revokes, and the real app really stops receiving seat events."""

    #: The colours `click-target` paints (channels x0x11, exact histogram
    #: read). Green target on a black field; a landed click repaints red.
    TARGET = "00ff00"
    HIT = "ff0000"

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        app = _resolve_sibling(self.shim_bin, "click-target", "VITRIN_CLICK_TARGET_APP")
        if app is None:
            # click-target is co-built with the shim unconditionally (a bare
            # wl_shm client, no optional dep), so its absence beside a built
            # shim is a build misconfiguration to FAIL on, not a machine state
            # -- exactly `test_real_actuation.py`'s read of the same app.
            self.fail(
                f"no click-target beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "rebuild the shim, or set VITRIN_CLICK_TARGET_APP."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-deadman-"))
        self.addCleanup(_rmtree, self.work)

    def real_core(self, *, capture_dump: str):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", "40000"],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            log_file=str(self.work / "core.log"),
            capture_dump=capture_dump,
        )

    def _spine(self, core) -> None:
        """Wait out `vitrind -> vitrin-shim -> click-target`, matching
        `test_real_actuation.py::RealActuationPointer._spine`."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r}",
        )
        app_pid = descendant_named(core.pid, "click-target", timeout=15.0)
        self.assertIsNotNone(app_pid, "the C shim never fork/exec'd click-target")

    def _locate_target(self, grant, timeout=20.0):
        """Observe until the green target is on screen; return `(frame, cx, cy)`.
        Identical read to `test_real_actuation.py`'s helper of the same name."""
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
            cx, cy, count = locate_colour(frame, self.TARGET)
            if count >= MIN_TARGET_PIXELS:
                return frame, cx, cy
            time.sleep(0.1)
        self.fail(
            f"the green target never reached the agent within {timeout:.0f}s: click-target's "
            "frame did not arrive, so there is nothing to click"
        )

    def _send_dead_man_signal(self, core) -> None:
        """SIGUSR1 to the core: the test-gated stand-in for a completed hold
        (module docs). Tells "the core revoked" apart from "the core died"
        immediately, rather than letting a feature-less build surface as a
        bare `ConnectionClosed` several assertions later with no clue why.
        """
        os.kill(core.pid, signal.SIGUSR1)
        # A feature-less `vitrind` has no SIGUSR1 handler installed at all
        # (the handler only exists under `dead-man-injector`), so the signal
        # takes its default disposition -- terminate -- and the process
        # dies within this window rather than revoking anything.
        time.sleep(0.5)
        if core.proc.poll() is not None:
            self.fail(
                f"the core exited (code {core.proc.returncode}) instead of revoking after "
                "SIGUSR1. This is what a `vitrind` built WITHOUT the `dead-man-injector` cargo "
                "feature does -- SIGUSR1's default disposition is terminate -- not what a "
                "completed dead-man chord does. Rebuild with `cargo build -p vitrin-core "
                "--features dead-man-injector` (tests/integration/run.sh does this "
                f"automatically).\ncore output so far:\n{core.output()}"
            )

    def test_sigusr1_triggers_the_deadman_switch_and_revokes_a_real_grant(self):
        dump_path = str(self.work / "internal.rgba")
        core = self.real_core(capture_dump=dump_path)
        self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)

        # 1. Before the chord: the grant is live against the real app -- the
        # agent locates the green target in its own captured frame (D10's
        # addressing), and does NOT click it. Clicking here would itself
        # flip the target permanently, which would make step 4 below
        # (an unflipped target) indistinguishable from "the click was
        # refused" -- so the whole point of the ordering is that the ONLY
        # click this test ever issues is the one after the chord that must
        # be refused.
        _frame, cx, cy = self._locate_target(grant)

        # 2. The chord fires: SIGUSR1 stands in for a completed hold
        # (module docs), applied through the real `Runtime::apply_dead_man`.
        self._send_dead_man_signal(core)

        # 3. After the chord: both the capture verb and the actuation verb
        # -- the whole D-series set the enforcement chokepoint gates -- are
        # refused Revoked on the very next check. No "give it a moment":
        # `apply_dead_man` ran synchronously inside the signal handler's
        # dispatch turn, before this process's `kill()` call even returned.
        from vitrin_os.protocol import Verb

        with self.assertRaises(errors.Revoked) as observe_ctx:
            grant.observe()
        self.assertEqual(observe_ctx.exception.verb, Verb.OBSERVE)

        with self.assertRaises(errors.Revoked):
            grant.pointer.click(cx, cy)

        conn.close()
        core.terminate()
        entries = core.entries()

        # 4. The real app never saw the click: read the CORE-INTERNAL
        # capture -- no grant, no wire, no SDK decode -- and confirm the
        # target is still exactly as green as it was in step 1. A click that
        # reached click-target's real wl_seat would have repainted the whole
        # surface red permanently (the same D10 side effect
        # `test_real_actuation.py` exploits to prove a click DID land); an
        # unchanged target is what proves this one did not.
        after = _read_dump(dump_path, REALM_WH)
        after_cx, after_cy, after_count = locate_colour(after, self.TARGET)
        self.assertGreaterEqual(
            after_count,
            MIN_TARGET_PIXELS,
            f"the target must still be green after the revoked click (saw {after_count} green "
            "px); a real click reaching click-target's wl_seat would have repainted it red",
        )
        hit_count = locate_colour(after, self.HIT)[2]
        self.assertEqual(
            hit_count,
            0,
            f"the target must show NO hit-colour pixels after a revoked click (saw {hit_count}); "
            "the click must never have reached the real app",
        )

        # 5. The journal says why: the cause entry, then the revocations it
        # explains, in that order (`deadman::apply`'s documented write order).
        kinds = [e["kind"] for e in entries]
        self.assertIn(
            "dead_man_triggered", kinds, "the flight recorder must journal the completed chord"
        )
        triggered_at = kinds.index("dead_man_triggered")
        revoked_at = [i for i, k in enumerate(kinds) if k == "grant_revoked"]
        self.assertTrue(revoked_at, "the chord must have revoked at least the live grant row")
        self.assertTrue(
            all(i > triggered_at for i in revoked_at),
            "every grant_revoked entry must come AFTER the dead_man_triggered cause it explains",
        )

        print(
            f"\n[real-deadman] SIGUSR1 fired the dead-man switch; grant.observe() and "
            f"grant.pointer.click() both refused Revoked; click-target's target stayed green "
            f"({after_count} px, 0 hit px); journal recorded {len(revoked_at)} grant_revoked "
            "entr" + ("y" if len(revoked_at) == 1 else "ies") + " after dead_man_triggered"
        )


def _rmtree(path: pathlib.Path) -> None:
    import shutil

    shutil.rmtree(path, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
