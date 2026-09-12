# SPDX-License-Identifier: Apache-2.0
"""P2.6.9 ★ (issue #193): a repo-authored ransomware payload runs in a real
realm, and its **measured write set equals exactly {designated fds} ∪ {realm
private storage}** -- no mock on any seam. This is M2.5's named gate (with
★P2.7.6), and it is PRD user story 6 turned into a test.

Everything below drives `target/debug/vitrind` over a real Unix socket, with a
real `vitrin-realm-init`, a real `vitrin-shim`, the real repo-authored
`ransom-payload` in the realm and the real Python SDK as the agent. The
payload connects to the per-realm `designation.sock` and receives descriptors
over `SCM_RIGHTS` -- the app-side, mock-free receipt that P2.6.6/P2.6.7 left
owed to this gate (`test_real_powerbox.py`'s `click-target` never connects, so
its shim logs `no app is connected`; here the disposition is `relayed`).

# A MEASURED SET EQUALITY, not an assertion that confinement is implemented

The gate reports the write set the payload actually reached and asserts it
equals exactly {the descriptors the agent designated} ∪ {the realm's own
private storage}. A single write outside that set fails the gate. "Confinement
is implemented" is precisely the class of claim D12 exists to refuse; this
measures an outcome and asserts an equality.

# THIS PAYLOAD IS REPO-AUTHORED (R2.7), and here is the reduction in independence

`shim/tests/ransom_payload.c` is ours: no third-party program will
cooperatively enumerate its own write attempts and report the errno each got,
and the property here is a set equality over *attempted* writes. This is the
`click-target`/`form-target`/`solid-client` precedent (M1.4/M1.5/P2.6.2). The
mitigations, in the same breath:

- **The third-party rungs stay green in the same CI run against the same shim,
  transport and chokepoint** -- `test_real_app.py` (weston-terminal),
  `test_real_gtk.py` (GTK), `test_real_firefox.py` (Firefox ESR).
- **Two independent witnesses of the designated writes in the same run** -- the
  payload's own `--out` report of every `RX`/write it attempted, and the
  core's designation journal (`designation_settled`, keyed by
  `(st_dev, st_ino)`). A payload that lied about which descriptor it got would
  have to contradict the journal.
- **A receipt-frozen breakage in the watched-failing list** (item 6, PR body):
  a payload faked to under-report its write set turns the journal cross-check
  red rather than passing.

**What this does NOT rule out** is a payload *bug* that under-reports its own
attempts against *undesignated* paths in a way the journal also misses: the
journal only ever sees designations, never the writes this payload aims at
paths it was never granted. The write-set-outside-the-grant half rests on the
payload's own report, mitigated by the positive control (each undesignated
target is shown *reachable* under `--isolation=off`, so an absence is never
satisfied by no path at all) and pinned per tier below.

# Per tier, and the gate says which tier it ran on

`vitrind --print-isolation` prints the kernel and the measured tier; the run
echoes both. The write-set claim is qualified where the P2.6.3 ladder reports a
missing right: below Landlock ABI 3 there is no `LANDLOCK_ACCESS_FS_TRUNCATE`,
so a `truncate(2)` on a *path* the domain grants READ can still empty it -- a
ransomware-relevant loss the create/write measurement here does not cover and
which `vitrin-realm-init`'s own rung-3 test owns. (A `pwrite`/`ftruncate` on a
read-only *descriptor* is `EBADF`/`EINVAL` at every rung, so this gate does not
repeat that vacuous framing -- Correction 1, plan §2.)

# Deviations from #193 as written, recorded in the decision log (D-045 precedent)

- The issue says the home-reach positive control runs under `--isolation=none`;
  the shipped selector is `--isolation=off` (`none` was retired before shipping,
  D-037(4)). Home is ENOENT inside a confined realm because the *mount
  namespace* never binds it -- not because Landlock denied it -- so the control
  that grows the write set toward the host home is `--isolation=off` (no
  namespaces), and dropping Landlock alone would leave home ENOENT at both
  settings.
- The path-race "racer runs against the live picker ... shown to WIN without
  RESOLVE_NO_SYMLINKS" is realised as the in-crate resolver test already
  proved: the confirm→open window is sub-millisecond and unsynchronisable from
  a test process. This gate's live half asserts the delivered fd's
  `(st_dev, st_ino)` equals the displayed row under a continuously-running
  racer, and an inside-pointing symlink row is refused `unresolvable`; the
  "racer wins" control is an in-process resolve THIS gate authors (opens the
  raced path by name, no `RESOLVE_NO_SYMLINKS`, and must win at least once),
  and "RESOLVE_NO_SYMLINKS dropped" is watched-failing item 4 (a source edit).
- The picker-spoofing rung's "the replica receives no input grab" is provable
  only on a backend that stacks the consent grab. Headless does not (its hook
  stack is `NoopHook`/attention-only, by construction), so a physical key while
  a picker is up would reach the realm there -- the opposite of the claim. That
  rung therefore runs on the **DRM** backend on an isolated VT
  (`VITRIN_RANSOM_DRM=1`) and SKIPS otherwise; the trust-band overdraw half is
  the `test_real_trust_band.py` technique.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import tempfile
import threading
import time
import unittest

from harness import (
    ConsentInjector,
    CoreFailed,
    DEMO_IDENTITY,
    IntegrationTest,
    comm_of,
    descendant_named,
    exe_identity,
    file_identity,
    shims_of,
)

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

REALM_SIZE = "720x640"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: The realm's own private-storage hierarchies, read from
#: `crates/vitrin-realm-init/src/landlock.rs`'s `grants` -- the ONLY authority
#: (`docs/book/src/limits.md` names the two-element "{designated fds} ∪ {realm
#: private storage}" phrasing wrong "in the flattering direction"). Full write
#: authority is granted on exactly these four hierarchies; a create into any of
#: them is IN the set the gate asserts equality against.
PRIVATE_STORAGE = ("/run/vitrin", "/vitrin/home", "/tmp", "/dev/shm")

#: The instrument's own report, written relative to `$XDG_RUNTIME_DIR` (the
#: realm runtime dir), read back by the gate via `core.realm_dir()` -- the
#: `solid_client.c --probe-out` convention, so one argv serves both isolation
#: settings.
REPORT_NAME = "ransom-report.txt"
READY_NAME = "ransom-ready.flag"

_WRONG_BUILD = (
    "the core did not come up as an instrumented, designation-serving session. This gate "
    "needs a build with `--features vitrin-core/consent-injector` and a configured picker "
    "root; run.sh builds it. If you ran it by hand, that feature and `--consent=interactive` "
    "are both required."
)


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
    the real-app ladder, e.g. `test_real_powerbox.py::_require_shim`)."""
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


class _Ask:
    """One `request_file`/`request_dir`, running while the harness drives the
    picker. `request_file` blocks for human time (its own docstring), so a
    single-threaded test cannot both make the ask and answer it; this runs the
    ask on a daemon thread and leaves the SDK connection to that thread for the
    ask's life. Cloned from `test_real_powerbox.py::_Ask`.
    """

    def __init__(self, conn, grant, *, want_dir: bool = False, **kwargs: object) -> None:
        self._out: dict[str, object] = {}
        self._thread = threading.Thread(
            target=self._run, args=(conn, grant, want_dir, kwargs), daemon=True
        )
        self._thread.start()

    def _run(self, conn, grant, want_dir, kwargs) -> None:
        try:
            if want_dir:
                self._out["value"] = conn.request_dir(grant)
            else:
                self._out["value"] = conn.request_file(grant, **kwargs)
        except BaseException as exc:  # noqa: BLE001 -- re-raised in `result`
            self._out["error"] = exc

    def result(self, test: unittest.TestCase, timeout: float = 30.0):
        self._thread.join(timeout)
        if self._thread.is_alive():
            test.fail(
                f"request_file did not return within {timeout:.0f}s of the picker being "
                "driven. The core owes exactly one terminal per admitted ask."
            )
        if "error" in self._out:
            raise self._out["error"]  # type: ignore[misc]
        return self._out["value"]


class RealRansomwareContainment(IntegrationTest):
    """Issue #193's mock-free gate: a payload's measured write set is exactly
    its designated fds plus its own private storage."""

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        payload = _resolve_sibling(self.shim_bin, "ransom-payload", "VITRIN_RANSOM_PAYLOAD_APP")
        if payload is None:
            self.fail(
                f"no ransom-payload beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "rebuild the shim, or set VITRIN_RANSOM_PAYLOAD_APP."
            )
        self.payload_bin = str(pathlib.Path(payload).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-ransomware-"))
        self.addCleanup(shutil.rmtree, self.work, True)
        self.core_log = self.work / "core.log"

        # The picker root and the two fixture files the agent will designate:
        # one read-only, one read-write. Inodes recorded BEFORE any core exists,
        # so no assertion re-resolves a path (the race the picker closes).
        self.root = self.work / "docs"
        self.root.mkdir()
        self.ro_name = b"read-only.txt"
        self.rw_name = b"read-write.txt"
        self.inode_of: dict[bytes, tuple[int, int]] = {}
        for name, body in ((self.ro_name, b"RO CONTENTS\n"), (self.rw_name, b"RW CONTENTS\n")):
            path = self.root / os.fsdecode(name)
            path.write_bytes(body)
            st = path.stat()
            self.inode_of[name] = (st.st_dev, st.st_ino)
        # An inside-pointing symlink: its target is designatable under its own
        # name, so a refusal is about the link being a link (RESOLVE_NO_SYMLINKS),
        # not about the target being unreachable.
        self.link_name = b"inner-link.txt"
        os.symlink(os.fsdecode(self.ro_name), self.root / os.fsdecode(self.link_name))

    # -- core boot ----------------------------------------------------------

    def _core(self, payload_args: list[str], *, isolation: str | None = None):
        """A real chain under `--consent=interactive`, the payload as the realm
        app, with the injector and a configured picker root."""
        try:
            return self.core(
                consent="interactive",
                size=REALM_SIZE,
                shim=str(self.shim_bin),
                command=self.payload_bin,
                args=payload_args,
                env_allow=tuple(WLR_ENV),
                extra_env=WLR_ENV,
                log_file=str(self.core_log),
                consent_injector=True,
                picker_root=str(self.root),
                isolation=isolation,
            )
        except CoreFailed as exc:
            self.fail(f"{_WRONG_BUILD}\n\nThe core's own words:\n{exc}")

    def _assert_instrumented(self, core) -> ConsentInjector:
        cmdline = pathlib.Path(f"/proc/{core.pid}/cmdline").read_bytes().split(b"\0")
        self.assertIn(b"--consent-injector-fd", cmdline)
        injector = core.injector
        assert isinstance(injector, ConsentInjector)
        try:
            banner = injector.await_banner()
        except Exception as exc:  # noqa: BLE001
            self.fail(f"{_WRONG_BUILD}\n\nThe channel said: {exc}")
        self.assertEqual(banner, "vitrin-consent-injector 1")
        log = self.core_log.read_text(errors="replace")
        self.assertIn("CONSENT INJECTOR IS WIRED", log)
        self.assertIn(f"picker root opened root={self.root}", log)
        return injector

    def _spine(self, core) -> int:
        """Wait out vitrind -> vitrin-shim -> ransom-payload, and prove the shim
        is the real one BY INODE (a confined shim's comm is the bind-target
        basename, so a name test cannot tell it from vitrin-mock-shim)."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            if core.proc.poll() is not None:
                self.fail(
                    f"the core exited {core.proc.returncode} instead of serving.\n"
                    f"{_WRONG_BUILD}\n{core.output()}"
                )
            found = shims_of(core.pid)
            if found:
                shim_pid = found[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        self.assertEqual(
            exe_identity(shim_pid),
            file_identity(self.shim_bin),
            f"the realm's shim (pid {shim_pid}, comm {comm_of(shim_pid)!r}) is not the C shim "
            f"this gate named ({self.shim_bin}) -- vitrin-mock-shim must appear nowhere here",
        )
        self.assertIsNotNone(
            descendant_named(core.pid, "ransom-payload", timeout=15.0),
            "the C shim never fork/exec'd ransom-payload",
        )
        return shim_pid

    # -- report reading -----------------------------------------------------

    def _await_report_line(self, core, needle: str, timeout: float = 20.0) -> str:
        """Poll the payload's own --out file (in the realm runtime dir) until a
        line containing `needle` appears; return the whole report."""
        path = core.realm_dir() / REPORT_NAME
        deadline = time.monotonic() + timeout
        text = ""
        while time.monotonic() < deadline:
            try:
                text = path.read_text(errors="replace")
            except OSError:
                text = ""
            if any(needle in line for line in text.splitlines()):
                return text
            if core.proc.poll() is not None:
                break
            time.sleep(0.05)
        self.fail(
            f"the payload's report at {path} never contained {needle!r} within {timeout:.0f}s; "
            f"report so far:\n{text}\ncore:\n{core.output()}"
        )

    @staticmethod
    def _report_lines(text: str, key: str) -> list[dict[str, str]]:
        """Parse `KEY field=value field=value` lines with the given leading key
        into dicts of their fields."""
        out: list[dict[str, str]] = []
        for line in text.splitlines():
            parts = line.split()
            if not parts or parts[0] != key:
                continue
            fields: dict[str, str] = {}
            for tok in parts[1:]:
                if "=" in tok:
                    k, v = tok.split("=", 1)
                    fields[k] = v
            out.append(fields)
        return out

    # ======================================================================
    # Rung 1: the write set outside the grant is empty, confined.
    # ======================================================================

    def _write_targets(self) -> list[tuple[str, str, bool]]:
        """(label, dir, expect_reachable_confined). The host home and the paths
        outside the realm's private storage must be UNREACHABLE confined; the
        private-storage hierarchies must be reachable (they are the realm's own
        and IN the set the equality asserts)."""
        host_home = os.path.expanduser("~")
        return [
            ("host_home", host_home, False),
            ("root", "/", False),
            ("etc", "/etc", False),
            ("run", "/run", False),
            ("core_sock_dir", "/run", False),  # dirname(/run/vitrin) -- ../core.sock's dir
            ("private_run_vitrin", "/run/vitrin", True),
            ("private_home", "/vitrin/home", True),
            ("private_tmp", "/tmp", True),
            ("private_shm", "/dev/shm", True),
        ]

    def test_the_write_set_outside_the_grant_is_empty_confined(self):
        targets = self._write_targets()
        args = ["--out", REPORT_NAME]
        for label, directory, _ in targets:
            args += ["--write-target", f"{label}={directory}"]
        core = self._core(args)
        self._assert_instrumented(core)
        self._spine(core)
        text = self._await_report_line(core, "WRITE-SET-END")
        creates = {
            f["target"]: f
            for f in self._report_lines(text, "WRITE")
            if f.get("op") == "create"
        }
        core.terminate()

        for label, directory, reachable in targets:
            self.assertIn(label, creates, f"the payload reported no create attempt for {label}")
            row = creates[label]
            if reachable:
                self.assertEqual(
                    row["rc"], "0",
                    f"{label} ({directory}) is the realm's own private storage and a create "
                    f"there must succeed; got errno={row.get('errno')}",
                )
            else:
                self.assertEqual(
                    row["rc"], "-1",
                    f"{label} ({directory}) is outside the granted set and a create there must "
                    f"be refused, not succeed -- this is the containment claim",
                )
                self.assertIn(
                    row["errno"], ("ENOENT", "EACCES", "EROFS", "EPERM"),
                    f"{label} refused with an unexpected errno {row.get('errno')}",
                )

    def test_the_host_home_is_reachable_uncontrolled_positive_control(self):
        """The non-vacuity counterweight: at `--isolation=off` (no namespaces),
        the host home the confined run could not reach IS reachable. Without
        this, "home unreachable" confined is satisfied by a home that was never
        reachable at all (the #138 lesson). The payload writes only a
        self-cleaning O_EXCL canary, so this never touches a real file."""
        host_home = os.path.expanduser("~")
        core = self._core(
            ["--out", REPORT_NAME, "--write-target", f"host_home={host_home}"],
            isolation="off",
        )
        self._assert_instrumented(core)
        self._spine(core)
        # At --isolation=off there is no mount namespace, so the report lands in
        # the off-mode runtime dir; read it wherever it is.
        text = self._await_report_line(core, "WRITE-SET-END")
        core.terminate()
        creates = {
            f["target"]: f
            for f in self._report_lines(text, "WRITE")
            if f.get("op") == "create"
        }
        self.assertIn("host_home", creates)
        self.assertEqual(
            creates["host_home"]["rc"], "0",
            "with no mount namespace the host home MUST be reachable; if it is not, the "
            "confined run's 'home unreachable' proves nothing. The payload created and "
            "unlinked a single O_EXCL canary there.",
        )

    # ======================================================================
    # Rung 2: the designated fds are the only writable authority, and the
    # journal is a second witness of them.
    # ======================================================================

    def _drive_grant(self, core, injector, conn):
        grant = conn.request_grant(
            verbs=("designate.file",),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        petition, token = injector.await_raised()
        self.assertEqual(injector.decide(token, "allow-while-running"), "queued")
        injector.await_lowered(petition)
        grant.await_consent()
        return grant

    def _designate_row(self, injector, want_selected: bytes, *, write: bool):
        """Drive the picker to the row whose selected bytes == want_selected and
        confirm; returns after `confirm` is queued."""
        snap = injector.await_picker(timeout=20.0)
        # Navigate down until the selected row matches.
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            snap = injector.picker()
            if snap["state"] == "shown" and snap["selected"] == want_selected:
                break
            injector.navigate("down")
            time.sleep(0.03)
        else:
            self.fail(f"the picker never selected {want_selected!r}; last reply {snap!r}")
        self.assertEqual(injector.confirm(), "queued")

    def test_designated_fds_are_the_only_writable_authority(self):
        core = self._core(
            ["--out", REPORT_NAME, "--ready", READY_NAME, "--expect", "2", "--designated-write"]
        )
        injector = self._assert_instrumented(core)
        self._spine(core)
        # The payload connects to designation.sock at startup and drops --ready
        # once connected; only then can the agent's ask land `relayed`.
        self._await_report_line(core, "READY")

        actor = core.connect()
        # Ask 1: the read-write file -- writing through it must succeed.
        rw_grant = self._drive_grant(core, injector, actor)
        rw_ask = _Ask(actor, rw_grant, write=True)
        self._designate_row(injector, self.rw_name, write=True)
        rw = rw_ask.result(self)
        self.assertIsInstance(rw, vitrin_os.messages.DesignatedEvent)
        os.close(rw.fd)

        # Ask 2: the read-only file -- a write through it must be the kernel's
        # EBADF, not the shim's or the core's.
        ro_grant = self._drive_grant(core, injector, actor)
        ro_ask = _Ask(actor, ro_grant, write=False)
        self._designate_row(injector, self.ro_name, write=False)
        ro = ro_ask.result(self)
        self.assertIsInstance(ro, vitrin_os.messages.DesignatedEvent)
        os.close(ro.fd)

        text = self._await_report_line(core, "RX-END")
        core.terminate()

        rx = self._report_lines(text, "RX")
        self.assertEqual(len(rx), 2, f"the payload should have received two designations: {rx}")
        by_mode = {r["mode"]: r for r in rx}
        self.assertEqual(sorted(by_mode), ["read", "read_write"])
        # The delivered inodes are the fixture files', recorded before boot.
        self.assertEqual(
            (int(by_mode["read_write"]["st_dev"]), int(by_mode["read_write"]["st_ino"])),
            self.inode_of[self.rw_name],
        )
        self.assertEqual(
            (int(by_mode["read"]["st_dev"]), int(by_mode["read"]["st_ino"])),
            self.inode_of[self.ro_name],
        )
        # The write through each: read_write succeeds, read-only is EBADF.
        writes = {r["n"]: r for r in self._report_lines(text, "DESIGNATED")}
        rw_write = writes[by_mode["read_write"]["n"]]
        self.assertEqual(rw_write["rc"], "0", f"a read_write designated fd must be writable: {rw_write}")
        ro_write = writes[by_mode["read"]["n"]]
        self.assertEqual(
            ro_write["errno"], "EBADF",
            f"a read-only designated fd is the kernel's EBADF on write, not a grant: {ro_write}",
        )

        # SECOND WITNESS: the core's designation journal. Every fd the payload
        # received is a `designation_settled` row delivered with a matching
        # (st_dev, st_ino), and the count of delivered rows equals the count of
        # fds the payload reported receiving.
        entries = core.entries()
        settled = [e for e in entries if e["kind"] == "designation_settled"]
        delivered = [e for e in settled if e.get("outcome") == "delivered" and e.get("delivered")]
        self.assertEqual(
            len(delivered), len(rx),
            f"the journal's delivered-designation count ({len(delivered)}) must equal the "
            f"payload's received-fd count ({len(rx)}): {settled}",
        )
        journal_inodes = {(e["st_dev"], e["st_ino"]) for e in delivered}
        payload_inodes = {(int(r["st_dev"]), int(r["st_ino"])) for r in rx}
        self.assertEqual(
            journal_inodes, payload_inodes,
            "the journal's (st_dev, st_ino) set must equal the inodes the payload reported "
            "receiving -- the two independent witnesses of the same descriptors",
        )
        # The shim relayed rather than closing `no_client` (the app connected).
        self.assertIn(
            "relayed",
            core.app_output(),
            "the shim must have RELAYED the designation to the connected payload -- the "
            "app-side mock-free receipt this gate owns; `no app is connected` would mean the "
            "payload never held designation.sock",
        )

    # ======================================================================
    # Rung 3: path race. The delivered fd is the displayed inode; an
    # inside-pointing symlink row is refused; and the in-process control shows
    # a symlink-following resolve WOULD have been raced (so the guarded green
    # is not merely "the racer was too slow").
    # ======================================================================

    def test_a_symlink_row_is_refused_not_followed(self):
        core = self._core(["--out", REPORT_NAME, "--expect", "1"])
        injector = self._assert_instrumented(core)
        self._spine(core)
        actor = core.connect()
        grant = self._drive_grant(core, injector, actor)
        ask = _Ask(actor, grant, write=False)
        self._designate_row(injector, self.link_name, write=False)
        result = ask.result(self)
        core.terminate()
        self.assertIsInstance(
            result, vitrin_os.messages.PowerboxRefusedEvent,
            "an inside-pointing symlink row must be refused (RESOLVE_NO_SYMLINKS -> ELOOP -> "
            f"unresolvable), not followed to its target: {result!r}",
        )
        self.assertEqual(result.code, vitrin_os.protocol.PowerboxRefusal.UNRESOLVABLE)

    def test_the_control_resolve_following_symlinks_would_be_raced(self):
        """The in-process control the issue's 'racer wins without
        RESOLVE_NO_SYMLINKS' reduces to (D-045 deviation): a symlink-following
        `open(path)` (no RESOLVE flags) against a component a thread is flipping
        DOES deliver the decoy at least once. Without this, the guarded green
        (the core's resolver never delivering the decoy) would prove only that
        the racer was too slow to matter. The guarded half is proved in-crate
        (`picker/resolve.rs`) and over the socket by the refusal above."""
        base = self.work / "race"
        base.mkdir()
        good = base / "good"
        good.mkdir()
        (good / "f").write_bytes(b"GOOD\n")
        decoy = base / "decoy"
        decoy.mkdir()
        (decoy / "f").write_bytes(b"DECOY\n")
        good_ino = (good / "f").stat().st_ino
        decoy_ino = (decoy / "f").stat().st_ino
        swap = base / "swap"
        os.symlink(good, swap)

        stop = threading.Event()

        def flipper():
            a = base / ".swap-a"
            b = base / ".swap-b"
            os.symlink(good, a)
            os.symlink(decoy, b)
            toggle = False
            while not stop.is_set():
                src = a if toggle else b
                os.rename(src, swap)
                # re-mint the consumed name
                (base / (".swap-a" if toggle else ".swap-b"))
                target = good if toggle else decoy
                try:
                    os.symlink(target, a if toggle else b)
                except FileExistsError:
                    pass
                toggle = not toggle

        t = threading.Thread(target=flipper, daemon=True)
        t.start()
        saw_decoy = False
        try:
            for _ in range(20000):
                try:
                    fd = os.open(str(swap / "f"), os.O_RDONLY)
                except OSError:
                    continue
                try:
                    if os.fstat(fd).st_ino == decoy_ino:
                        saw_decoy = True
                        break
                finally:
                    os.close(fd)
        finally:
            stop.set()
            t.join(2.0)
        self.assertTrue(
            saw_decoy,
            f"a symlink-following resolve never delivered the decoy inode ({decoy_ino}) though "
            f"the swap was flipped continuously (good={good_ino}); the racer is too slow, so "
            "the core's guarded resolve delivering only the displayed inode would prove "
            "nothing. The guarded half is proved in crates/vitrin-core/src/picker/resolve.rs.",
        )

    # ======================================================================
    # Rung 4: kernel and measured isolation tier, printed.
    # ======================================================================

    def test_the_kernel_and_measured_isolation_tier_are_stated(self):
        core_bin = os.environ.get("VITRIN_CORE_BIN", "target/debug/vitrind")
        proc = subprocess.run(
            [core_bin, "--print-isolation"], capture_output=True, text=True, timeout=30
        )
        self.assertEqual(proc.returncode, 0, f"--print-isolation failed: {proc.stderr}")
        out = proc.stdout
        self.assertIn("kernel.release=", out)
        self.assertIn("tier=", out)
        kernel = next(
            (ln.split("=", 1)[1] for ln in out.splitlines() if ln.startswith("kernel.release=")),
            "?",
        )
        tier = next(
            (ln.split("=", 1)[1] for ln in out.splitlines() if ln.strip().startswith("tier=")),
            "?",
        )
        abi = next(
            (ln.split("=", 1)[1] for ln in out.splitlines() if "landlock.abi=" in ln),
            "?",
        )
        print(
            f"\n[real-ransomware] measured on kernel {kernel}, isolation tier {tier}, "
            f"landlock ABI {abi}. The write-set claim is qualified at this tier: below "
            "Landlock ABI 3 a read-granted PATH can still be truncated (P2.6.3 Correction 2), "
            "which this gate's create/write measurement does not cover."
        )

    # ======================================================================
    # Rung 5 (DRM only): the replica picker reaches neither the trusted band
    # nor an input grab. SKIPPED off an isolated VT (headless does not stack
    # the consent grab, so a physical key there reaches the realm -- the
    # opposite of the claim). Runs under VITRIN_RANSOM_DRM=1 in a hardware run.
    # ======================================================================

    @unittest.skipUnless(
        os.environ.get("VITRIN_RANSOM_DRM") == "1",
        "the picker-spoofing rung needs a backend that stacks the consent grab (DRM/nested) "
        "on an isolated VT; headless does not, so 'the replica receives no input grab' is not "
        "provable there. Set VITRIN_RANSOM_DRM=1 on an isolated VT (a hardware run) to run it.",
    )
    def test_the_replica_picker_reaches_neither_the_band_nor_a_grab(self):
        self.fail(
            "DRM spoof rung not yet executed. The payload's --replica-picker mode and the "
            "trust-band witness technique (test_real_trust_band.py) are wired; this arm is "
            "run in a hardware-run session on an isolated VT, per the module docstring."
        )


if __name__ == "__main__":
    unittest.main()
