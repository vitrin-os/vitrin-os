# SPDX-License-Identifier: Apache-2.0
"""The Firefox rung of the real bring-up ladder, under the real core (P1.6.6).

This is the top of the M1.2 ladder (weston-terminal -> GTK app -> Firefox) and
the render half of "Shim runs Firefox", now proved against the REAL core
rather than the shim's standalone mock. `shim/tests/acceptance/firefox_bringup.sh`
drives the same real shim and the same pinned Firefox, but under
`shim/tests/mock_core.c` -- a hand-written core stand-in that reads a dominant
colour off every committed frame. That script is a valuable shim-in-isolation
smoke test and stays; it is not, however, a milestone integration proof,
because the core it runs against is a mock. This module is that proof: the
shipped `vitrind` execs the built C shim, which fork/execs the pinned Firefox
ESR, which renders a local `file://` page painting a KNOWN solid colour, and
the real Python SDK captures a real Firefox frame through the real capture
path and asserts its dominant colour is the served colour -- no mock on any
seam.

It discharges the M1.2 render proof plus the globals-contract proof:

1. **Spine.** `vitrind -> vitrin-shim -> firefox`, read from procfs: the shim
   is the core's one direct child, and Firefox is a descendant of the shim
   (Firefox's own launcher/content-process fan-out sits under the shim, so
   "descendant of the shim", not "direct child", is the honest ancestry
   claim).
2. **A real Firefox frame of a known colour.** `pages/solid.html` paints
   `#0000ff` and nothing else; the captured frame's dominant colour is that
   blue, over enough of the view to be the page and not Firefox's grey chrome
   -- distinct from the shim's opaque black background and from any mock
   animation.
3. **The globals contract held.** The shim runs with its probe catalogue
   armed and its globals ledger written to a file (both requested through the
   realm's environment -- the one route a realm's environment grows by --
   since the production core conveys only the app command in the shim's argv).
   Every interface Firefox *demanded* (bound, including the probe-only ones it
   cannot otherwise reveal) must already be in
   `shim/docs/firefox-refused-globals.txt`; a new demand turns this red, which
   is the point -- the v0 global set is a contract, and a change to what the
   app needs must reach a human.

Agent-driven injection INTO Firefox (pointer scroll, typing the URL bar) is
the actuation track's concern (issue #108), not this render gate.

# Skip-or-fail policy (matches firefox_bringup.sh's discipline)

Firefox is a ~20 s run and a 75 MB download, so this gate is HEAVY and its own
CI step, and it is opt-outable -- but it never skips silently on the machine
that is meant to hold it:

- `VITRIN_SKIP_FIREFOX_GATE=1` -> skip. The explicit opt-out (local dev, or a
  CI job that has not fetched Firefox).
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim is missing -> fail (CI misconfig).
- `VITRIN_C_SHIM_BIN` set and the pinned Firefox is absent -> **fail**, not
  skip: a real run was requested and the browser is a declared dependency
  (`shim/tests/firefox/fetch-esr.sh`). Set `VITRIN_SKIP_FIREFOX_GATE=1` to
  decline it instead.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

from harness import (
    IN_REALM_HOME,
    IntegrationTest,
    await_shims,
    children_of,
    comm_of,
    descendant_named,
    dominant_colour,
    exe_identity,
    file_identity,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

#: The repo root the shim tree lives under -- resolved from THIS file, not
#: `VITRIN_REPO` (which points at the workspace whose warm `target/` holds
#: `vitrind`; locally those can be different trees). The pages, the profile,
#: the refused-globals allowlist and the Firefox fetch script are all the
#: shim's, so they resolve against the checkout that contains this test.
REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
SHIM_TREE = REPO_ROOT / "shim"
FF_DIR = SHIM_TREE / "tests" / "firefox"
SOLID_PAGE = FF_DIR / "pages" / "solid.html"
PROFILE_TEMPLATE = FF_DIR / "profile.user.js"
REFUSED_GLOBALS = SHIM_TREE / "docs" / "firefox-refused-globals.txt"

#: The colour pages/solid.html paints, and the dominant colour the captured
#: frame must carry. Channels are multiples of 0x11 so the 4-bit dominant-colour
#: histogram (harness.dominant_colour, mock_core.c's quantisation) reads it back
#: exactly.
SERVED_COLOUR = "0000ff"

#: How much of the view the page colour must cover to count as "the page, not
#: the chrome". Firefox's tab strip + toolbar is ~12% of a 1024x768 view, so a
#: solid page fills the rest; 55% is firefox_bringup.sh's proven floor -- far
#: above any incidental colour, comfortably below the ~85% a solid page reaches.
MIN_DOMINANT_PCT = 55

#: The realm view size, hence the captured frame size. 1024x768 is
#: firefox_bringup.sh's, chosen so the chrome is a small fraction of the view.
REALM_SIZE = "1024x768"

#: The environment Firefox and the shim need, forwarded the one legal way a
#: realm's environment grows -- the realm's `env_allow`, copied from the core's
#: own environment (seeded here via `extra_env`) into the shim, which passes it
#: to the app. Split by who reads it:
#:   * WLR_* -- the shim's headless/software-render backend (CI has no GPU).
#:   * MOZ_*/GDK/LIBGL/GTK/NO_AT_BRIDGE -- Firefox's documented headless
#:     software-WebRender, Wayland, no-a11y-bus configuration
#:     (shim/docs/firefox.md; identical to firefox_bringup.sh).
#:   * HOME is NOT here any more (P2.6.2, #186). `realm.rs`'s RESERVED_ENV
#:     refuses it -- a host `$HOME` names a directory a confined realm's
#:     filesystem does not contain -- and the core injects the realm's own
#:     private storage instead. That storage is this run's Firefox profile, and
#:     it is fresh per run for the reason the old comment gave: a persistent
#:     profile accumulates session state and "deterministic" then means "until
#:     someone runs it twice".
#:   * VITRIN_SHIM_* -- the shim's diagnostic knobs (shim/src/main.c env
#:     fallbacks): write the globals ledger to a file, and arm the probe
#:     catalogue so a demand for a NON-provided interface becomes an observable
#:     bind. The production core does not pass shim argv flags, so these travel
#:     as environment.
#: The ledger path and the profile are filled in setUp (per-run paths).
BASE_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
    "MOZ_ENABLE_WAYLAND": "1",
    "GDK_BACKEND": "wayland",
    "MOZ_ACCELERATED": "0",
    "LIBGL_ALWAYS_SOFTWARE": "1",
    "MOZ_CRASHREPORTER_DISABLE": "1",
    "GTK_A11Y": "none",
    "NO_AT_BRIDGE": "1",
    "VITRIN_SHIM_PROBE_GLOBALS": "1",
}


def _read_refused_globals() -> set[str]:
    """The documented allowlist of interfaces Firefox may demand and not get.

    One interface per line; blank lines and `#` comments ignored -- the exact
    grammar firefox_bringup.sh check (D) reads.
    """
    names: set[str] = set()
    for line in REFUSED_GLOBALS.read_text().splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            names.add(stripped)
    return names


class RealFirefox(IntegrationTest):
    """Firefox rendering a known colour through the real chain, end to end."""

    def setUp(self) -> None:
        super().setUp()
        # Firefox on software WebRender is genuinely slow to first paint; give
        # this one heavy gate a far wider deadline than the suite default,
        # which is sized for the fast mock-shim tests.
        signal.alarm(0)
        signal.alarm(180)

        if os.environ.get("VITRIN_SKIP_FIREFOX_GATE") == "1":
            self.skipTest("VITRIN_SKIP_FIREFOX_GATE=1 (explicit opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
                "Build it (meson setup shim/build shim && meson compile -C shim/build) and "
                "point the variable at shim/build/vitrin-shim."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a "
                "real run was requested; refusing to skip a requested gate (CI misconfig)."
            )

        self.firefox_bin = self._resolve_firefox()

        # A fresh per-run scratch tree: the profile, the ledger file, and the
        # core's redirected log all live here and are removed when the test
        # ends, pass or fail.
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-ff-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.core_log = self.work / "core.log"

        # P2.6.2 (#186): the realm is CONFINED, so every host path this fixture
        # used to hand Firefox is a path that does not exist in there. Three
        # moves, none of them cosmetic:
        #
        #  1. The runtime tree is named here rather than minted by the harness,
        #     because the realm's private-storage path is derived from it and
        #     has to be known before the core boots.
        #  2. The profile IS the realm's private storage (`/vitrin/home`, which
        #     the core injects as HOME and binds read-write). It is the only
        #     writable, host-visible, never-purged directory a confined app
        #     has: the realm's runtime directory is deleted on shutdown and its
        #     `/tmp` is a private tmpfs. `user.js` is dropped in before the
        #     core starts, which is why the tree is pre-created here.
        #  3. `HOME` has left `env_allow` -- `realm.rs`'s RESERVED_ENV refuses
        #     it now, and the refusal is a startup failure, not a warning. The
        #     core decides it; this gate no longer gets a vote.
        self.rt = self.work / "rt"
        self.rt.mkdir()
        self.profile = self.rt / "data" / "vitrin" / "realms" / "realm-0"
        self.profile.mkdir(parents=True)
        # 0700 down the whole chain: `prepare_realm_storage` audits every
        # component it did not create itself.
        for part in (self.rt / "data", self.rt / "data" / "vitrin", self.profile.parent):
            part.chmod(0o700)
        self.profile.chmod(0o700)
        shutil.copy(PROFILE_TEMPLATE, self.profile / "user.js")
        self.ledger = self.profile / "globals.log"

        self.env = dict(BASE_ENV)
        self.env["VITRIN_SHIM_GLOBALS_LOG"] = f"{IN_REALM_HOME}/globals.log"

    def _resolve_firefox(self) -> str:
        """The pinned Firefox binary, verified offline, or a hard failure.

        The path comes from the fetch script (`--print`), and the pin is
        re-verified without touching the network (`--verify-only`) -- the same
        two calls firefox_bringup.sh makes, so an unverifiable browser can
        never make a result here unattributable to a version.

        `--verify-only` checks two different things (issue #298), and this
        gate needs both. The tarball's sha256 says what was DOWNLOADED; the
        unpacked tree's `application.ini` Version and BuildID say what is
        INSTALLED -- and those diverged in practice, because Firefox ships an
        updater that rewrites the unpacked tree in place while leaving the
        tarball alone. On the development machine it did so on 2026-07-22:
        the browser this method resolved was 140.13.0esr from that date until
        2026-08-16, against a pin of 140.12.0esr, and neither `--verify-only`
        nor anything in this file could tell. The frames were real and the
        colour assertion was real; the version they were filed under was not.
        """
        fetch = FF_DIR / "fetch-esr.sh"
        printed = subprocess.run(
            ["bash", str(fetch), "--print"], capture_output=True, text=True
        )
        binary = printed.stdout.strip()
        if printed.returncode != 0 or not binary:
            self.fail(f"fetch-esr.sh --print failed: {printed.stderr.strip()}")
        if not (os.path.isfile(binary) and os.access(binary, os.X_OK)):
            self.fail(
                f"the pinned Firefox is not present at {binary}, but VITRIN_C_SHIM_BIN is set so "
                "a real run was requested. Fetch it (bash shim/tests/firefox/fetch-esr.sh) or "
                "decline this gate with VITRIN_SKIP_FIREFOX_GATE=1."
            )
        verify = subprocess.run(
            ["bash", str(fetch), "--verify-only"], capture_output=True, text=True
        )
        if verify.returncode != 0:
            self.fail(
                "the pinned Firefox failed offline verification "
                f"(fetch-esr.sh --verify-only):\n{verify.stdout}\n{verify.stderr}"
            )
        return binary

    def real_core(self):
        """A core booting `vitrind -> vitrin-shim -> firefox` on solid.html.

        The core's combined output is redirected to a file (`log_file`): a
        probing shim under Firefox writes thousands of DEBUG lines and Firefox
        is chatty, which would overflow the harness's after-exit stdout pipe
        and wedge the run.
        """
        # The page is on the host and the realm's filesystem does not contain
        # it, so it is bound READ-ONLY at its own path -- which is what
        # `binds` is for -- and the URL is that same path. Its directory
        # rather than the file, because a bind source that is a file needs a
        # file target and a directory is the shape `realm.toml` documents.
        url = SOLID_PAGE.resolve().as_uri()
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=str(pathlib.Path(self.firefox_bin).resolve()),
            # `--profile /vitrin/home`: the realm's own private storage, where
            # setUp put `user.js`. A host profile path would not exist in the
            # realm, and a read-only bind of one would not be a profile.
            args=["--profile", IN_REALM_HOME, "--no-remote", url],
            env_allow=tuple(self.env),
            extra_env=self.env,
            binds=(str(SOLID_PAGE.resolve().parent),),
            runtime_dir=str(self.rt),
            log_file=str(self.core_log),
        )

    def _spine(self, core):
        """Wait out `vitrind -> vitrin-shim -> firefox` and return its pids."""
        # `shims_of`, not `children_of`: at --isolation=default (the
        # default since P2.6.2, #186) the core's direct child is the
        # `vitrin-realm-init` supervisor and the shim is ITS child, so a
        # direct-children walk finds no shim at all.
        found = await_shims(core.pid, timeout=20.0)
        shim_pid = found[0] if found else None
        self.assertIsNotNone(
            shim_pid, f"the core forked no shim; children were {children_of(core.pid)}"
        )
        # The mock-freeness check, by INODE rather than by name. A confined
        # shim is bound at `/vitrin/shim`, so its `comm` is `shim` whichever
        # binary it is (P2.6.2, #186) and a name test stopped telling the real
        # shim from `vitrin-mock-shim`. The running image's inode does, and
        # more sharply: a name says what a program is called, an inode says
        # which file is executing.
        self.assertEqual(
            exe_identity(shim_pid),
            file_identity(self.shim_bin),
            f"the realm's shim (pid {shim_pid}, comm {comm_of(shim_pid)!r}) is not "
            f"the C shim this gate named ({self.shim_bin}) -- vitrin-mock-shim must "
            "appear nowhere in this path",
        )
        # Firefox is a DESCENDANT of the shim (its launcher/content processes
        # sit under it), so search from the shim, not the core -- this both
        # finds the browser and proves it descends from THIS shim.
        ff_pid = descendant_named(shim_pid, "firefox", timeout=40.0)
        self.assertIsNotNone(
            ff_pid,
            f"the C shim never fork/exec'd firefox; core={core.pid} shim={shim_pid} "
            f"shim descendants' comms were "
            f"{[comm_of(k) for k in children_of(shim_pid)]}",
        )
        return shim_pid, ff_pid

    def _capture_served_colour(self, grant, timeout=90.0):
        """Observe until Firefox's rendered page colour dominates a real frame.

        Firefox on software WebRender takes several seconds to first paint, and
        early frames are the shim's black background, then Firefox's grey
        chrome/white content area before the page paints. This loops `observe`
        (D6, one fresh frame per call) until the dominant colour is the served
        blue over `MIN_DOMINANT_PCT` of the view, recording the de-duplicated
        sequence of dominant colours seen for diagnostics. `NoSurface` retries
        cost no rate budget (judged before the token bucket).
        """
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        best = (None, 0)
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.1)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.1))
                continue
            colour, pct = dominant_colour(frame)
            if not seen or seen[-1] != colour:
                seen.append(colour)
            if colour == SERVED_COLOUR and pct > best[1]:
                best = (frame, pct)
            if colour == SERVED_COLOUR and pct >= MIN_DOMINANT_PCT:
                return frame, pct, seen
            time.sleep(0.2)
        self.fail(
            f"no frame reached the agent whose dominant colour was the served #{SERVED_COLOUR} "
            f"over >= {MIN_DOMINANT_PCT}% within {timeout:.0f}s. Dominant-colour sequence seen: "
            f"{' -> '.join('#' + c for c in seen)}. Best #{SERVED_COLOUR} coverage: {best[1]}%. "
            "A real Firefox frame of the solid page must arrive, or the real chain is not "
            "delivering Firefox's content."
        )

    def _assert_globals_contract(self):
        """Every interface Firefox demanded is already in the refused allowlist.

        Read from the ledger the real-core run wrote (VITRIN_SHIM_GLOBALS_LOG).
        The probe banner proves the catalogue was armed (a subset test against
        an allowlist is vacuous if nothing was offered); each `globals-demand`
        interface must be in shim/docs/firefox-refused-globals.txt.
        """
        self.assertTrue(
            self.ledger.is_file(),
            f"the shim wrote no globals ledger at {self.ledger}: the VITRIN_SHIM_GLOBALS_LOG "
            "env fallback did not reach the shim, or the probe run never started.",
        )
        lines = self.ledger.read_text().splitlines()

        armed = None
        for line in lines:
            if line.startswith("globals-log: probes_armed="):
                armed = int(line.split("probes_armed=")[1].split()[0])
                break
        self.assertIsNotNone(
            armed,
            "the ledger recorded no probe banner; VITRIN_SHIM_PROBE_GLOBALS did not arm the "
            f"catalogue. Ledger head:\n{chr(10).join(lines[:5])}",
        )
        self.assertGreater(
            armed, 0, "the probe catalogue armed zero interfaces; the demand check is vacuous"
        )

        demanded: set[str] = set()
        for line in lines:
            # Anchored: a record is a record only if it STARTS the line
            # (ledger.h). libwayland quotes a client's own interface string in
            # error text, so an unanchored match could read the app's bytes as
            # a record.
            if line.startswith("globals-demand:") and "interface=" in line:
                demanded.add(line.split("interface=")[1].split()[0])

        refused = _read_refused_globals()
        unexpected = demanded - refused
        self.assertEqual(
            unexpected,
            set(),
            f"Firefox demanded interface(s) not in {REFUSED_GLOBALS.name}: {sorted(unexpected)}. "
            "This is the ledger doing its job: decide each one deliberately -- add it to the v0 "
            "set in shim/src/globals.c citing the demand line, or add it to the refused list with "
            "the reason in shim/docs/firefox.md. Do not silence it.",
        )
        return armed, sorted(demanded)

    def test_firefox_renders_a_known_colour_through_the_real_spine(self):
        core = self.real_core()
        shim_pid, ff_pid = self._spine(core)
        self.assertEqual(comm_of(core.pid), "vitrind")
        # The shim by inode, not by name: a confined shim runs from the bind
        # target `/vitrin/shim`, so its comm is `shim` whichever binary it is
        # (P2.6.2, #186).
        self.assertEqual(exe_identity(shim_pid), file_identity(self.shim_bin))
        self.assertTrue(comm_of(ff_pid).startswith("firefox"))
        # Recorded while the processes are alive: the summary line at the end
        # runs after the core has been torn down, and `comm_of` on a reaped pid
        # is the empty string.
        spine = (comm_of(core.pid), comm_of(shim_pid), comm_of(ff_pid))

        conn = core.connect()
        grant = whole_realm_grant(conn)
        frame, pct, seen = self._capture_served_colour(grant, timeout=90.0)

        self.assertEqual((frame.width, frame.height), (1024, 768))
        served, got = SERVED_COLOUR, dominant_colour(frame)
        self.assertEqual(
            got[0],
            served,
            f"the captured frame's dominant colour is #{got[0]} ({got[1]}%), not the served "
            f"#{served}: the real chain delivered a frame that is not Firefox's solid page.",
        )

        conn.close()
        core.terminate()

        armed, demanded = self._assert_globals_contract()

        allowed = {
            e.get("verb")
            for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "allowed"
        }
        self.assertIn(
            "observe",
            allowed,
            f"the real Firefox capture must be admitted at the enforcement chokepoint; saw {allowed}",
        )

        print(
            f"\n[real-firefox] spine {spine[0]} -> the C shim "
            f"(comm {spine[1]!r}, matched by inode) -> {spine[2]} "
            f"(pids {core.pid} -> {shim_pid} -> {ff_pid})\n"
            f"[real-firefox] served #{served}; captured dominant #{got[0]} at {pct}% of a "
            f"{frame.width}x{frame.height} frame\n"
            f"[real-firefox] dominant-colour sequence: {' -> '.join('#' + c for c in seen)}\n"
            f"[real-firefox] probe catalogue armed {armed} interfaces; Firefox demanded "
            f"{len(demanded)} refused interface(s), all in {REFUSED_GLOBALS.name}: {demanded}"
        )


if __name__ == "__main__":
    unittest.main()
