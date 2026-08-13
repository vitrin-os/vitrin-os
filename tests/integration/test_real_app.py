# SPDX-License-Identifier: Apache-2.0
"""The M1.2 exit gate: the whole real system, end to end, no mock on any seam.

Every other integration test drives the shipped `vitrind` against
`vitrin-mock-shim` -- a synthetic peer written by the same hand, in the same
language, that animates a hand-drawn buffer and forks no app. Every shim
acceptance test (`shim/tests/acceptance/*.sh`) drives the real C shim against
`shim/tests/mock_core.c`. Each real half has therefore only ever met a mock on
the *other* seam. This module is the first time the two real halves meet: the
shipped `vitrind` execs the built C shim (issue #104), which fork/execs a real
`weston-terminal`, whose real frames flow shim -> core and are byte-captured by
the real Python SDK through the real enforcement and capture path. No
`vitrin-mock-shim`, no `mock_core.c`, nowhere in the path (issue #105).

Per the M1.2 definition-of-done (#111), the milestone is "done" only when this
runs green. It boots the real chain and discharges four assertions -- the
issue's fifth (that it runs green headless / GPU-free and *fails* rather than
skips on a machine that is meant to run it) is not a per-frame check but a
property of the whole module, enforced by the Skip-or-fail policy below:

1. **Ancestry.** The process spine is `vitrind -> vitrin-realm-init ->
   vitrin-shim -> weston-terminal`, read from procfs -- not the mock, and not a
   direct app exec by the core. The middle link arrived with P2.6.2 (#186):
   `unshare(CLONE_NEWPID)` does not move the caller, so the confinement helper
   has to fork the realm's PID 1 and stay behind as its supervisor. The shim is
   matched by the **inode of the file it is executing**, not by `comm`: a
   confined shim runs from the bind target `/vitrin/shim`, so `vitrin-shim` and
   `vitrin-mock-shim` answer the same name and only the inode still tells them
   apart.
2. **A real frame.** A frame `weston-terminal` actually rendered reaches the
   agent through the enforcement/capture path and is non-uniform (it carries
   the terminal's chrome), so it is neither the mock animation nor an
   all-zero buffer.
3. **Confinement.** The app's environment names only the three the core
   injects -- `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR` and, since P2.6.2, `HOME` --
   at their **in-realm** paths (`/run/vitrin/wayland-0`, `/run/vitrin`,
   `/vitrin/home`) plus the allow-listed names; each in-realm path is then
   resolved through `/proc/<app>/root` and matched **by inode** against this
   run's own realm tree, so three constants cannot pass for a directory nobody
   checked. No host session variable leaks in, and the shim's core-socket
   descriptor (fd 3) is absent from the app's fd table -- also checked by
   inode, so descriptor-number reuse cannot fool it.
4. **Graceful degradation (P1.5.2).** Killing the shim mid-run removes the
   surface; the core survives and the agent's next `observe()` returns
   `NoSurface`, not a stale real-app frame and not a crash.

# Skip-or-fail policy (matches the firefox_bringup / app_spawn gate discipline)

A gate that only ever reports SKIP on the machine that gates merges is a gate
nobody is holding. So:

- `VITRIN_SKIP_REAL_APP=1` -> skip. The explicit opt-out for local dev.
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim; the
  same variable the conformance job and `shim.rs`'s cross-track test use.
- `VITRIN_C_SHIM_BIN` **set** but the shim binary or `weston-terminal` is
  missing -> **fail**. That is a CI misconfiguration, and a gate that passed
  it silently would prove nothing while looking green.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import time
import unittest

from harness import (
    IN_REALM_HOME,
    IN_REALM_RUNTIME_DIR,
    IN_REALM_WAYLAND_SOCKET,
    IntegrationTest,
    await_shims,
    capture_when_ready,
    children_of,
    colour_bytes,
    comm_of,
    descendant_named,
    environ_of,
    exe_identity,
    fd_targets_of,
    file_identity,
    has_real_content as _has_real_content,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

#: The app the gate boots. A real Wayland client with visible chrome that
#: renders blind against a headless shim -- the same rung `app_spawn.sh` and
#: `firefox_bringup.sh`'s smaller sibling use. Resolved to an absolute path
#: (the core's spawn audit refuses a relative `command`).
APP_NAME = "weston-terminal"

#: The realm view the core configures, and therefore the captured frame size.
#: Larger than the mock's 320x200 so `weston-terminal` has room to lay out its
#: window decorations -- the chrome assertion (2) leans on that content.
REALM_SIZE = "640x480"

#: The headless / pure-software render selectors the C shim's wlroots backend
#: needs (CI has no GPU). They reach the shim only through the realm's
#: `env_allow` -- the one route a realm's environment may grow by -- seeded
#: into the core's own environment for the allowlist to copy from. This
#: mirrors the c_shim conformance test (crates/vitrin-core/src/shim.rs) and
#: `app_spawn.sh` exactly.
WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: Environment names a confined app must never carry with a host value: a
#: host session socket, an X display, or the session bus address would each be
#: an ambient authority the confinement exists to remove. The core builds the
#: child environment from nothing (`env_clear`) and injects only the private
#: WAYLAND_DISPLAY/XDG_RUNTIME_DIR, so none of these can appear -- assertion 3
#: proves the construction held rather than trusting it.
HOST_LEAK_NAMES = (
    "DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XAUTHORITY",
    "WAYLAND_SOCKET",
)


# `colour_bytes` and `_has_real_content` (imported above as `has_real_content`)
# moved to harness.py when the real-app ladder grew past one rung (issue #106):
# the GTK and Firefox gates share the exact same "not the shim's empty fill"
# analysis, and one definition cannot drift from another. The behaviour is
# unchanged -- this gate still rejects both the shim's opaque background and a
# toolkit's content-free initial fill on the frame's colour channels alone.


class RealApp(IntegrationTest):
    """The five-assertion M1.2 gate, against the real chain end to end."""

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (explicit local-dev opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
                "Build it (meson setup shim/build shim && meson compile -C shim/build) and "
                "point the variable at shim/build/vitrin-shim. CI sets it, so CI cannot reach "
                "this skip -- there a missing shim is a failure, below."
            )
        # From here the operator declared a real run by setting the variable, so
        # a missing binary is a misconfiguration to FAIL on, never to skip past.
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a "
                "real run was requested; refusing to skip a requested gate (CI misconfig)."
            )
        app = shutil.which(APP_NAME) or (
            f"/usr/bin/{APP_NAME}" if os.access(f"/usr/bin/{APP_NAME}", os.X_OK) else None
        )
        if app is None:
            self.fail(
                f"{APP_NAME} is not installed, but VITRIN_C_SHIM_BIN is set so a real run was "
                "requested. Install weston (shim/ci/install-deps.sh does), or set "
                "VITRIN_SKIP_REAL_APP=1 to opt out. A requested gate must not skip silently."
            )
        # Absolute: the core's spawn audit refuses a relative `command`
        # (crates/vitrin-core/src/spawn.rs).
        self.app_bin = str(pathlib.Path(app).resolve())

    def real_core(self):
        """A core booting the real chain: C shim + `weston-terminal` realm.

        Reaped when the test ends (`IntegrationTest.core`), pass or fail.
        """
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=[],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
        )

    def _spine(self, core):
        """Wait out the real spawn spine and return `(shim_pid, app_pid)`.

        The core execs the C shim as its one direct child; the C shim
        fork/execs the app as *its* child, so the app is a grandchild of the
        core. Both are asserted here as the gate's first criterion.
        """
        # `shims_of`, not `children_of`: at --isolation=default (the
        # default since P2.6.2, #186) the core's direct child is the
        # `vitrin-realm-init` supervisor and the shim is ITS child, so a
        # direct-children walk finds no shim at all.
        found = await_shims(core.pid, timeout=15.0)
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
        app_pid = descendant_named(core.pid, APP_NAME, timeout=15.0)
        self.assertIsNotNone(
            app_pid,
            f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid} "
            f"shim children were {children_of(shim_pid)}",
        )
        # Ancestry, not just descent: the app's parent is the shim, so the core
        # did not exec the app directly. procfs stat field 4 is the ppid.
        ppid = int(pathlib.Path(f"/proc/{app_pid}/stat").read_text().split()[3])
        self.assertEqual(
            ppid,
            shim_pid,
            f"{APP_NAME} (pid {app_pid}) parent is {ppid}, not the shim {shim_pid}: the core "
            "must exec the shim which execs the app, never the app directly",
        )
        return shim_pid, app_pid

    def _capture_real_content(self, grant, timeout=20.0):
        """Capture until weston-terminal's chrome actually reaches the agent.

        `capture_when_ready` retries only the `NoSurface` first-frame race, so
        it returns the app's *first* commit -- which, against the real chain,
        is frequently the shim's opaque background or weston's own pre-chrome
        fill, both content-free. This keeps observing until the colour channels
        carry real content (:func:`_has_real_content`), which is what makes
        assertion 2 a genuine "a real weston frame reached the agent" and not a
        pass on the shim's empty output. Bounded by `timeout` (well inside the
        SIGALRM budget); each admitted `observe` spends a token, but the loop
        paces itself far under the 20/s default ceiling, and `NoSurface`/
        `RateLimited` retries cost nothing / back off on the server's hint.
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
            last = frame
            if _has_real_content(frame):
                return frame
            time.sleep(0.1)
        distinct = "no frame captured" if last is None else len(set(colour_bytes(last)))
        self.fail(
            f"no content-bearing weston-terminal frame reached the agent within {timeout}s: "
            f"every captured frame was a uniform/empty fill (last frame distinct colour values: "
            f"{distinct}). The real chrome frame must arrive, or the real chain is not delivering "
            "real content -- a green here on the shim's opaque background would be a vacuous pass."
        )

    # -- assertion 1 + 2 ---------------------------------------------------

    def test_real_frame_flows_through_the_real_spine_to_the_agent(self):
        core = self.real_core()
        shim_pid, app_pid = self._spine(core)
        self.assertEqual(
            (comm_of(core.pid), exe_identity(shim_pid), comm_of(app_pid)),
            ("vitrind", file_identity(self.shim_bin), APP_NAME),
            "the spine must be exactly vitrind -> the real C shim -> weston-terminal. "
            "The middle link is matched by the executing file's inode, not by name: a "
            "confined shim runs from the bind target /vitrin/shim and its comm is "
            f"{comm_of(shim_pid)!r} for the real shim and the mock alike (P2.6.2, #186).",
        )

        conn = core.connect()
        grant = whole_realm_grant(conn)
        # Retry until weston-terminal's chrome truly reaches the agent, not
        # merely until a surface exists. A real app must fork, connect,
        # configure, and *draw its chrome* before a content-bearing frame
        # exists (where the mock draws synchronously), and the C shim
        # composites an opaque background that is present before -- and
        # independent of -- the client's first commit. `capture_when_ready`
        # would return that empty background (or weston's pre-chrome fill),
        # making the milestone assertion vacuous; `_capture_real_content` loops
        # past both to a frame that carries colour.
        frame = self._capture_real_content(grant, timeout=20.0)

        self.assertEqual((frame.width, frame.height), (640, 480))
        self.assertEqual(len(frame.raw), frame.stride * frame.height)
        # A real, non-synthetic frame: `weston-terminal`'s chrome makes the
        # buffer's *colour* channels non-uniform. Checking the raw bytes is not
        # enough -- the shim's opaque background has a constant 0xFF padding
        # plane, so an all-bytes check reads {0x00,0xFF} as "content" on a frame
        # that has none (see `colour_bytes`). So the invariants are asserted on
        # the colour channels alone: non-zero (some pixel is not black) and
        # non-uniform (more than one colour value), which the shim's empty
        # opaque background and weston's pre-chrome fill both fail.
        colour = colour_bytes(frame)
        self.assertTrue(
            any(colour),
            "the captured frame's colour channels are all zero: only the shim's opaque "
            "background reached the agent, no real weston-terminal content",
        )
        self.assertGreater(
            len(set(colour)),
            1,
            "the captured frame is a single uniform colour: weston-terminal renders chrome, so "
            "a uniform buffer means the agent received the shim's empty fill, not the live "
            "composite of a real app's frame",
        )

        conn.close()
        core.terminate()

        # The capture passed the real enforcement chokepoint, recorded there --
        # the same ledger every mock-driven test asserts on, now stamped by a
        # real-app frame.
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

    # -- assertion 3 -------------------------------------------------------

    def test_the_app_is_confined_to_its_shims_private_universe(self):
        core = self.real_core()
        shim_pid, app_pid = self._spine(core)

        env = environ_of(app_pid)
        self.assertTrue(env, f"could not read /proc/{app_pid}/environ")

        # The private socket the shim binds, the private runtime directory and
        # the realm's own private storage -- never the host session's.
        #
        # **The names changed with P2.6.2 (#186) and the claim got stronger,
        # not weaker.** At `--isolation=default` -- the default, and what this
        # gate runs at -- the app's filesystem is the realm's, so the core
        # injects the IN-REALM paths: `/run/vitrin`, `/run/vitrin/wayland-0`
        # and `/vitrin/home`. Asserting only those strings would be a weaker
        # test than the old one, because three constants say nothing about
        # *which* directory they name; so each is then resolved through
        # `/proc/<app>/root` and compared BY INODE against this run's own realm
        # tree. A realm that had somehow been given the host session's runtime
        # directory under the in-realm name fails the second check.
        realm_dir = core.runtime / "vitrin-0" / "realm-0"
        self.assertEqual(
            env.get("WAYLAND_DISPLAY"),
            IN_REALM_WAYLAND_SOCKET,
            "the app's WAYLAND_DISPLAY must name its shim's private socket at the in-realm "
            "path the core injects",
        )
        self.assertEqual(
            env.get("XDG_RUNTIME_DIR"),
            IN_REALM_RUNTIME_DIR,
            "the app's XDG_RUNTIME_DIR must be the realm's private directory",
        )
        self.assertEqual(
            env.get("HOME"),
            IN_REALM_HOME,
            "a confined app's HOME must be the realm's own private storage: the operator's "
            "home directory is not in the realm's filesystem at all, and passing it through "
            "would name a path that does not exist (realm.rs's RESERVED_ENV)",
        )
        for name, in_realm in (
            ("XDG_RUNTIME_DIR", IN_REALM_RUNTIME_DIR),
            ("WAYLAND_DISPLAY", IN_REALM_WAYLAND_SOCKET),
        ):
            through_realm = f"/proc/{app_pid}/root{in_realm}"
            host = realm_dir if name == "XDG_RUNTIME_DIR" else realm_dir / "wayland-0"
            self.assertEqual(
                file_identity(through_realm),
                file_identity(host),
                f"{in_realm} inside the realm is not this run's own {host}: the app's "
                f"{name} names an in-realm path, and the path has to resolve to the realm's "
                "own directory rather than to anything else mounted there",
            )
        # No host session variable crossed the env_clear the core builds the
        # child environment from.
        for name in HOST_LEAK_NAMES:
            self.assertNotIn(
                name, env, f"host session variable {name} leaked into the confined app"
            )
        # The environment is *only* what the core composed: the allow-listed
        # names plus the three injected confinement names. Anything else would
        # mean the scrub is porous. `LD_LIBRARY_PATH` is in the allowlist on a
        # machine whose C shim links a vendored wlroots -- see
        # `harness.shim_library_dirs` for why that is a workaround for a real
        # defect and not a fact of nature -- and absent on one whose shim links
        # only system libraries, so the expected set is computed rather than
        # written out.
        allowed = set(WLR_ENV)
        if core.shim_library_dirs:
            allowed.add("LD_LIBRARY_PATH")
        self.assertEqual(
            set(env),
            allowed | {"WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "HOME"},
            f"the app's environment must hold only the allow-listed names and the three "
            f"injected confinement names; it held {sorted(env)}",
        )

        # fd 3 is the shim's core link -- a live channel to the TCB. The check
        # is by inode, not descriptor number: number 3 in the app is its own
        # business (its Wayland connection to the shim legitimately lands
        # there), but the *inode* of the shim's core socket must appear
        # nowhere (matching shim/tests/acceptance/app_spawn.sh check (c)).
        shim_core_socket = os.readlink(f"/proc/{shim_pid}/fd/3")
        self.assertTrue(
            shim_core_socket.startswith("socket:"),
            f"the shim's fd 3 should be its core socket; got {shim_core_socket!r}",
        )
        self.assertNotIn(
            shim_core_socket,
            set(fd_targets_of(app_pid).values()),
            f"the app inherited the shim's core socket {shim_core_socket}: a live channel to the "
            "TCB in the confined app is the whole confinement gone",
        )

    # -- assertion 4 -------------------------------------------------------

    def test_killing_the_shim_degrades_gracefully_and_the_core_survives(self):
        core = self.real_core()
        shim_pid, _app_pid = self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)
        capture_when_ready(grant, timeout=20.0)  # a live surface exists first

        # Kill the shim out from under the running realm. SIGKILL, not SIGTERM:
        # this is the ungraceful loss P1.5.2 promises the core survives -- the
        # shim gets no chance to tear down, so the core's own reaper is what
        # must notice and remove the surface.
        os.kill(shim_pid, signal.SIGKILL)

        # The next observe must resolve to NoSurface -- the honest "the realm
        # has no surface" reply -- rather than a stale frame or an error the
        # agent could not have prevented. Polled because the core's reaper and
        # teardown race the agent's next call; a stale real-app frame in the
        # window is tolerated, a stale frame *after* NoSurface should be would
        # be the bug, and the loop asserts NoSurface arrives and stays.
        deadline = time.monotonic() + 15.0
        degraded = False
        while time.monotonic() < deadline:
            try:
                grant.observe()
            except errors.NoSurface:
                degraded = True
                break
            time.sleep(0.1)
        self.assertTrue(
            degraded,
            "after the shim was killed the agent's observe never returned NoSurface: the surface "
            "was not removed, so degradation is not graceful",
        )
        self.assertIsNone(
            core.proc.poll(),
            "the core must survive its shim being killed (P1.5.2 graceful degradation); "
            f"it exited {core.proc.returncode}",
        )

        conn.close()
        core.terminate()


if __name__ == "__main__":
    unittest.main()
