# SPDX-License-Identifier: Apache-2.0
"""The M1.3 exit gate (P1.8.5, issue #107): an agent observes a REAL app end
to end through the real enforcement chokepoint, and the capture path is proved
to add no distortion.

Every live `observe()` this suite has ever made read the mock shim's
hand-drawn animation (`harness.py`'s default `command = MOCK_SHIM --animate`).
`test_real_app.py` replaced the mock on both seams for the *spine* proof — real
`vitrind` → real `vitrin-shim` → real `weston-terminal`, a real frame reaching
the agent. This module holds the three remaining M1.3 criteria, against a real
app whose pixels are *known*, so the claims are exact rather than "not empty":

1. **A known dominant colour (criterion 2).** The agent's captured frame's
   dominant colour equals the colour the real app rendered, headless/pixman,
   GPU-free — the CI-runnable colour proof. The app is `solid-client`, a
   toolkit-free wl_shm client that fills the whole realm view with one colour
   whose channels are multiples of 0x11 (so the capture's 4-bit histogram reads
   it back with no tolerance).

2. **No distortion (criterion 3).** That same agent frame agrees, by SSIM, with
   the **core-internal capture** — the raw composited readback `vitrind
   --capture-dump` writes straight to a file, before `render_frame`, the memfd,
   the wire and the SDK decode ever run. Their agreement is the plan's "the
   grant path adds no distortion" proof, finally against a real app, scored with
   the P1.9.2 `vitrin-golden` harness (via the compiled `vitrin-golden-cmp`, so
   this dependency-free suite uses the *real* SSIM, not a second copy of it).

3. **Rate-limit + expiry on the capture path (criterion 4).** The same refusals
   the actuation gate proves for `actuate.*` against the mock (`test_actuation.py`),
   now for `observe` against a real app: a capture burst past the grant's rate
   is refused `rate_limited`, and an expired grant's next `observe` is refused
   `expired` — asserted both as the SDK's typed exception and in the flight
   recorder. (The issue frames the rate case as "100 captures/s vs a 5/s grant";
   the test proves that same over-rate → refusal mechanism with a rate chosen
   for a wall-time-independent ceiling — see the test for why.)

The other three M1.3 criteria live where they were first proved and are not
repeated here: the no-mock spine (criterion 1) and shim-crash → `no_surface`
(criterion 5) are `test_real_app.py`'s; the nested Firefox variant (criterion 6)
is `shim/docs/firefox.md` §8 — a workstation-only recipe (nested needs a real
host compositor + GPU) that CI never runs, by construction rather than by a
silent skip.

# Skip-or-fail policy (matches the #105 real-app gate)

- `VITRIN_SKIP_REAL_APP=1` → skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset → skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim, `solid-client`, or
  `vitrin-golden-cmp` is missing → **fail**. All three are co-built by the same
  CI steps that build the shim and the workspace, so their absence under a
  requested real run is a misconfiguration, never a silent skip.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import tempfile
import time
import unittest

from harness import (
    GOLDEN_CMP,
    IntegrationTest,
    capture_dump_path,
    children_of,
    comm_of,
    descendant_named,
    dominant_colour,
    golden_cmp,
    packed_xrgb,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

WHILE_RUNNING = vitrin_os.Persistence.WHILE_RUNNING

#: The solid-colour client the gate boots. `comm` is kernel-truncated to 15
#: bytes and "solid-client" fits, so the descendant match (a prefix test) is
#: exact.
APP_NAME = "solid-client"

#: The realm view size, hence the captured frame size. Same as the weston/GTK
#: rungs — the colour and SSIM proofs are size-independent, and matching keeps
#: the ladder uniform.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: The colour `solid-client` paints and the dominant colour the captured frame
#: must carry. Each channel is a multiple of 0x11, so the 4-bit dominant-colour
#: histogram reads it back exactly (the Firefox solid page's discipline).
SERVED_COLOUR = "0000ff"

#: How long the client stays up: it must outlive the capture loops below, well
#: inside the harness's 90 s SIGALRM budget.
RUN_MS = "40000"

#: The headless/software-render selectors the C shim's wlroots backend needs
#: (CI has no GPU), forwarded the one legal way a realm's environment grows —
#: `env_allow`, copied from the core's own environment (seeded via `extra_env`).
#: Identical to the weston rung.
WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: The SSIM floor for the no-distortion proof. A static solid scene captured
#: two ways that share a byte-identical `view_cache` should score ~1.0, so 0.98
#: is comfortably met when nothing is wrong and drops hard if the transport
#: corrupts the frame — the whole point of the check.
MIN_SSIM = 0.98


def _resolve_solid_app(shim_bin: pathlib.Path) -> str | None:
    """The `solid-client` binary, or None if this machine has none built.

    `VITRIN_SOLID_APP` names it explicitly; otherwise it is a sibling of the C
    shim, exactly as `gtk-entry-probe` is resolved — the shim's Meson build
    drops `vitrin-shim` and `solid-client` in the same directory. Unlike the
    GTK probe, `solid-client` has no optional dependency (it is a bare wl_shm
    client), so it is always co-built with the shim; its absence beside a built
    shim is a misconfiguration, which the caller turns into a failure.
    """
    explicit = os.environ.get("VITRIN_SOLID_APP")
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / APP_NAME
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


class RealCaptureFidelity(IntegrationTest):
    """The M1.3 render/no-distortion/refusal proofs, against a real app."""

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

        app = _resolve_solid_app(self.shim_bin)
        if app is None:
            # solid-client is co-built with the shim unconditionally, so its
            # absence beside a built shim is a misconfiguration to FAIL on, not
            # a real machine state to skip past (contrast gtk-entry-probe, whose
            # GTK dependency makes its absence legitimate).
            self.fail(
                f"no {APP_NAME} beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); its "
                "absence is a build misconfiguration. Rebuild the shim, or set VITRIN_SOLID_APP."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

        if not (GOLDEN_CMP.is_file() and os.access(GOLDEN_CMP, os.X_OK)):
            self.fail(
                f"the vitrin-golden compare tool is missing at {GOLDEN_CMP}, but a real run was "
                "requested (VITRIN_C_SHIM_BIN set). It is built by `cargo build --workspace`, "
                "which CI runs before this suite; its absence is a build misconfiguration."
            )

        # A per-test scratch tree for the capture-dump target and the frames the
        # SSIM tool reads, created before the core boots (the dump path is a
        # boot argument) and reaped whatever the outcome.
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-fidelity-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    def real_core(self, capture_dump=None):
        """A core booting `vitrind → vitrin-shim → solid-client`."""
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS, "--colour", SERVED_COLOUR],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            capture_dump=capture_dump,
        )

    def _spine(self, core):
        """Wait out `vitrind → vitrin-shim → solid-client` and return its pids."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(
            shim_pid, f"the core forked no shim; children were {children_of(core.pid)}"
        )
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r}",
        )
        app_pid = descendant_named(core.pid, APP_NAME, timeout=15.0)
        self.assertIsNotNone(
            app_pid,
            f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid} "
            f"shim children were {children_of(shim_pid)}",
        )
        return shim_pid, app_pid

    def _capture_served_colour(self, grant, timeout=20.0):
        """Observe until the solid client's known colour reaches the agent.

        Before the client paints, the C shim composites an opaque *black*
        background, so early frames are `#000000`. A solid client's frame is
        deliberately UNIFORM — the whole view is one colour — so the
        "not the shim's empty fill" test is not non-uniformity (that would
        reject the very frame we want) but the dominant colour BEING the served
        one and not the background. This loops `observe` (D6, one fresh frame
        per call) until that holds. `NoSurface`/`RateLimited` retries cost no
        rate budget / back off on the server's hint.
        """
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            colour, _pct = dominant_colour(frame)
            if not seen or seen[-1] != colour:
                seen.append(colour)
            if colour == SERVED_COLOUR:
                return frame, seen
            time.sleep(0.1)
        self.fail(
            f"no frame whose dominant colour was the served #{SERVED_COLOUR} reached the agent "
            f"within {timeout:.0f}s. Dominant-colour sequence seen: "
            f"{' -> '.join('#' + c for c in seen)}. The solid client's frame must arrive, or the "
            "real chain is not delivering its content."
        )

    # -- criteria 2 + 3: known colour + no distortion (one boot) -----------

    def test_agent_frame_matches_the_served_colour_and_the_core_internal_capture(self):
        # One boot serves both proofs: the frame whose dominant colour is
        # checked (criterion 2) is the very frame compared against the
        # core-internal capture (criterion 3).
        dump = self.scratch("internal.rgba")
        core = self.real_core(capture_dump=dump)
        self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)
        frame, seen = self._capture_served_colour(grant, timeout=20.0)

        # -- criterion 2: the captured frame's dominant colour is the served
        # colour, within the histogram's tolerance (channels are multiples of
        # 0x11, so it reads back exactly).
        self.assertEqual((frame.width, frame.height), REALM_WH)
        got, pct = dominant_colour(frame)
        self.assertEqual(
            got,
            SERVED_COLOUR,
            f"the captured frame's dominant colour is #{got} ({pct}%), not the served "
            f"#{SERVED_COLOUR}: the real chain delivered a frame that is not the solid app's. "
            f"Dominant-colour sequence: {' -> '.join('#' + c for c in seen)}",
        )
        self.assertGreater(
            pct,
            90,
            f"the served colour covers only {pct}% of the view; a full-surface solid client "
            "should fill essentially all of it (a low share means chrome or an empty fill leaked in)",
        )

        # -- criterion 3: that same frame agrees with the core-internal capture.
        # The dump is written whenever the core recomposes that realm's view
        # (issue #252 stopped it recomposing an unchanged scene, so "every
        # redraw" is no longer true). Either way a static solid scene makes
        # every composite byte-identical, so the dump reflects exactly this
        # frame: the skipped rounds are skipped precisely because they would
        # have reproduced the bytes already there.
        agent_path = self.scratch("agent.xrgb")
        pathlib.Path(agent_path).write_bytes(packed_xrgb(frame))
        dump_path = self._await_dump(capture_dump_path(dump))

        # The issue offers "SSIM / per-pixel tolerance"; the gate asserts BOTH,
        # because they are complementary on this scene. SSIM is the named proof
        # and the P1.9.2 harness's structural score, but a solid field has no
        # structure, so its SSIM is degenerate (a colour swap that preserved
        # luma could slip past). The per-pixel tolerance is what makes the
        # no-distortion claim airtight for a flat frame: every channel of every
        # pixel must match within a hair, which a byte-order or truncation bug
        # in `render_frame`/memfd/wire/decode could not survive. The chosen
        # blue (#0000ff) is asymmetric across channels, so a channel swap moves
        # both scores hard.
        ssim = golden_cmp(
            agent_path, dump_path, REALM_WH, f"ssim:{MIN_SSIM}", artifacts=self.scratch("ssim-art")
        )
        self.assertEqual(
            ssim.returncode,
            0,
            "the agent's captured frame must agree with the core-internal capture by SSIM "
            f">= {MIN_SSIM} (the grant path adds no distortion). Compare tool said:\n"
            f"{ssim.stdout.strip()}\n{ssim.stderr.strip()}",
        )
        # Per-pixel: no channel may differ by more than 1 on more than 0.1% of
        # pixels — effectively exact, the real "adds no distortion" bar.
        tol = golden_cmp(
            agent_path, dump_path, REALM_WH, "tol:1,0.001", artifacts=self.scratch("tol-art")
        )
        self.assertEqual(
            tol.returncode,
            0,
            "the agent frame and the core-internal capture must agree per-pixel within tolerance "
            f"(the flat-field no-distortion proof). Compare tool said:\n"
            f"{tol.stdout.strip()}\n{tol.stderr.strip()}\n"
            f"break-down PNGs (on failure) under {self.scratch('tol-art')}",
        )
        result = ssim

        conn.close()
        core.terminate()

        # The capture passed the real enforcement chokepoint, recorded there.
        allowed = {
            e.get("verb")
            for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "allowed"
        }
        self.assertIn(
            "observe",
            allowed,
            f"the real-app capture must be admitted at the enforcement chokepoint; saw {allowed}",
        )
        print(
            f"\n[real-fidelity] served #{SERVED_COLOUR}; captured dominant #{got} at {pct}% of a "
            f"{frame.width}x{frame.height} frame\n"
            f"[real-fidelity] agent frame vs core-internal capture: {result.stdout.strip()}"
        )

    # -- criterion 4: rate-limit on the real capture path ------------------

    def test_bursting_past_the_grant_rate_refuses_capture_with_rate_limited(self):
        core = self.real_core()
        self._spine(core)
        conn = core.connect()

        # Warm the surface FIRST on a SEPARATE default-rate grant: `no_surface`
        # is judged before the rate bucket, so without a live surface an
        # over-rate `observe` would be refused `no_surface` and never reach the
        # bucket. The warm-up rides its own grant because the token bucket is
        # per-grant (test_actuation.py's RateCeiling discipline, for capture).
        self._capture_served_colour(whole_realm_grant(conn), timeout=20.0)

        # A 1/s grant, chosen for a WALL-TIME-INDEPENDENT ceiling. The issue's
        # framing is "100 captures/s vs a 5/s grant"; its *mechanism* is simply
        # "capture faster than the grant permits → rate_limited", which any rate
        # proves. Rate 1 proves it without the timing coupling a rate of 5 would
        # carry: the bucket is capacity = max_event_rate, born full, and refills
        # one token per refill-interval (rate 5 → 200 ms, rate 1 → 1 s). At
        # rate 5 a burst only drains if each `observe` out-paces a 200 ms
        # refill, so a pathologically slow runner could refill fast enough to
        # never bind and false-fail. At rate 1 the interval is a full second —
        # orders of magnitude above any real `observe` round-trip — so the
        # second back-to-back capture is refused with certainty. This is exactly
        # why the sibling actuation RateCeiling test uses rate 1.
        grant = conn.request_grant(
            verbs=("observe",),
            persistence=WHILE_RUNNING,
            max_event_rate=1,
        ).await_consent()

        # Burst as fast as possible: the first capture spends the one born-full
        # token, and the next (well within the 1 s refill window) is refused.
        admitted = 0
        limited = None
        for _ in range(50):
            try:
                grant.observe()
                admitted += 1
            except errors.NoSurface:
                # The surface was warmed; a stray no_surface just means a redraw
                # race, not the bucket — retry without counting it.
                time.sleep(0.02)
            except errors.RateLimited as rl:
                limited = rl
                break
        self.assertIsNotNone(
            limited,
            f"a rapid capture burst against a 1/s grant was never rate-limited (admitted "
            f"{admitted}); the token bucket did not bind on the real capture path",
        )
        self.assertEqual(limited.grant_id, grant.id)
        self.assertGreater(
            limited.retry_after_ms,
            0,
            "RateLimited must carry the refill hint (never zero, rounded up)",
        )
        # The burst must have got *some* captures through before the ceiling —
        # a full bucket, then refusal, not refusal from the first call.
        self.assertGreaterEqual(
            admitted,
            1,
            "the grant's initial burst allowance admitted nothing; the bucket was not born full",
        )

        conn.close()
        core.terminate()
        self.assertTrue(
            _refused(core.entries(), "rate_limited"),
            "the over-rate capture must be refused at the chokepoint and recorded",
        )

    # -- criterion 4: expiry on the real capture path ----------------------

    def test_capturing_with_an_expired_grant_refuses_with_expired(self):
        core = self.real_core()
        self._spine(core)
        conn = core.connect()

        # Expiry is judged before the `no_surface` gate (enforcement.rs: the
        # row's death code precedes the use-context gates), so an expired
        # `observe` is refused `expired` whether or not a surface is live — no
        # warm-up needed, and the refusal is deterministic rather than a race.
        grant = conn.request_grant(
            verbs=("observe",),
            persistence=WHILE_RUNNING,
            expiry_ms=100,
        ).await_consent()

        time.sleep(0.3)  # comfortably past the 100 ms deadline

        with self.assertRaises(errors.GrantExpired) as caught:
            grant.observe()
        self.assertEqual(caught.exception.grant_id, grant.id)
        self.assertEqual(caught.exception.code, errors.GrantExpired.REFUSAL)

        conn.close()
        core.terminate()
        self.assertTrue(
            _refused(core.entries(), "expired"),
            "the expired capture must be refused at the chokepoint and recorded",
        )

    # -- helpers -----------------------------------------------------------

    def scratch(self, name: str) -> str:
        """A path under this test's throwaway scratch tree (reaped in cleanup)."""
        return str(self.work / name)

    def _await_dump(self, path, timeout=5.0) -> str:
        """Wait until the core-internal capture file exists and has content.

        `--capture-dump` writes atomically (temp + rename) every time the core
        composes that realm's view, so the file appears whole once one composite
        has run — which it has by the time a content-bearing `observe()`
        returned, because that frame came from the same cache entry the dump
        mirrors. Polled only to close the narrow window between the agent's
        capture and that write landing. Note that since issue #252 an unchanged
        scene is not recomposed, so this waits for the file to *exist*, never
        for it to be rewritten.
        """
        expected_len = REALM_WH[0] * REALM_WH[1] * 4
        deadline = time.monotonic() + timeout
        p = pathlib.Path(path)
        while time.monotonic() < deadline:
            if p.is_file() and p.stat().st_size == expected_len:
                return str(p)
            time.sleep(0.05)
        size = p.stat().st_size if p.is_file() else "absent"
        self.fail(
            f"the core-internal capture at {path} never reached {expected_len} bytes "
            f"(size: {size}); `--capture-dump` did not write the composited readback"
        )


def _refused(entries, code: str):
    """The `use_decision` entries the core recorded as refused with `code`."""
    return [
        e
        for e in entries
        if e["kind"] == "use_decision"
        and e.get("decision") == "refused"
        and e.get("refusal") == code
    ]


if __name__ == "__main__":
    unittest.main()
