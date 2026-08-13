# SPDX-License-Identifier: Apache-2.0
"""The **realm-confinement property gate** (P2.6.2, issue #186, D-037).

Mock-free, real-app, and deliberately **not** a milestone gate, for the reason
`test_two_realms.py` and `test_layout.py` are not: `docs/plan/00-roadmap.md`
schedules no milestone here, and this closes a gap the project *publishes*
(`docs/book/src/limits.md`, `README.md`, `SECURITY.md`) rather than adding to a
closed milestone.

The whole chain is real: the shipped `vitrind`, the shipped
`vitrin-realm-init`, the built C `vitrin-shim`, and a real Wayland client
(`solid-client`) that connects over a real socket and paints. Nothing here
constructs a runtime in-process and `vitrin-mock-shim` appears nowhere.

## The criterion, and why it is two runs and not one

`docs/plan/02-phase-2-semantic-epochs.md` asks for exactly this shape:

> from inside the realm, `open()` on a canary file in `$HOME` fails `ENOENT`
> **and the same run proves that canary is reachable under `--isolation=off`**

The second half is the whole point. An absence over a path nothing proved
reachable is satisfied by no path at all -- a canary that had been deleted, or
mistyped, or created in a directory the app was never going to look in, is
"unreachable" from inside the realm and from everywhere else too, and a gate
that only checked the negative would print its success line either way. So the
positive control is not a nicety here; it is what makes the negative mean
something.

Everything about the two runs is held identical except the one flag:

* the **same** canary file, in the operator's real `$HOME`, created before
  either core boots and removed in teardown (owner's call, 2026-08-13);
* the **same** app, at the same path, with **byte-identical argv** -- asserted,
  by comparing the two `realm.toml` files the harness wrote;
* the same shim, the same size, the same everything the harness composes.

Only `--isolation` differs, and D-037(4) renamed its off-switch: the selector's
values are `default` and `off`, never `none`, because `Tier::None` is also what
an *unmeasured* probe yields and one token cannot mean both "the operator chose
no confinement" and "nothing was measurable".

## Reachability is decided by inode, never by name

The realm's mount table **creates** `$HOME` inside the realm. It has to:
`vitrin-realm-init` binds the app's own directory at its host path, and making
that target `mkdir -p`s every ancestor onto the realm's root tmpfs -- so for
any app under a development checkout, `$HOME` exists in there as an empty stub
directory. A presence test would therefore call a correct realm a breach, and
`crates/vitrin-core/src/spawn.rs`'s parent-side canary check says so in its own
comment, having been written the other way first.

`(st_dev, st_ino)` separates the two cases exactly, and it *narrows* the check
at the same time: a mount table that shadowed the canary with a same-named
decoy no longer passes by accident. So the app reports the identity of what it
opened, and this gate compares it against the host's.

## The positive control inside the realm

`/dev/null` is probed in **both** runs and must be `open=ok` in both. Without
it, a probe that had failed to run, or that returned `open=fail` for every path
because the report was written by a crashed process, would satisfy every
negative assertion here. That is the same class of vacuity the README's own
"mock-freeness is not discriminating power" note warns about, one layer down.

## What is asserted

`RealConfinementCanary` -- one test method, both directions, one run:

1. The host canary is readable **before and after** both runs (non-vacuity,
   checked at both moments rather than assumed to persist).
2. Under `--isolation=default` the app's `open()` on it fails **ENOENT**.
3. Under `--isolation=off` the same `open()` succeeds and the descriptor's
   `(st_dev, st_ino)` is the host file's.
4. `$HOME` itself: present-but-a-different-inode or absent under `default`
   (either is confinement; the stub is what the mount table had to create),
   and the host's own inode under `off`.
5. `/dev/null` opens in both.
6. The two runs' `realm.toml` are byte-identical.

`RealConfinementJournal` -- the flight recorder carries what the **parent**
observed, not what the child claimed:

7. `realm_spawned.isolation` under `default`: `applied_profile` is
   `namespaces-only` (never a tier name -- `intra-user` means namespaces *plus*
   Landlock *plus* seccomp and this build applies the first third),
   `parent_observed.namespaces_verified` is the five the supervisor is checked
   on, `root_dev_differs` and `canaries_unreachable` are true, `writable` is a
   measured list, and `landlock`/`seccomp` are the explicit `not-applied`
   strings rather than absent keys.
8. `shim_host_pid` and `supervisor_pid` agree with what **this process** read
   out of procfs -- the journal's account of the spine checked against an
   independent one.
9. Under `off`, every parent-observed field is null/empty and
   `writable_source` says why rather than printing a hopeful default.

`RealConfinementDevices` -- the published residual, measured at the app:

10. The realm still holds **every one** of the operator's supplementary groups
    (D-037(5): `setgroups(0, NULL)` is impossible for an unprivileged realm in
    either window, so `setgroups=deny` blocks the *call* and drops nothing) --
    set-equal to this process's own `os.getgroups()`, and equal in count to the
    number the journal reports.
11. `/dev/input` is nonetheless unreachable, because the mount table never
    binds it. That is the sharp reading of 10: the realm carries the group that
    would open those devices and cannot reach them, so **the mount table is the
    only barrier**, exactly as `SECURITY.md` and `limits.md` now say.

`RealConfinementRefusesTheUnverifiable` -- the checkpoint fires:

12. A substituted `--realm-init` that unshares all six namespaces and mounts
    **nothing** (`crates/vitrin-realm-init-fixtures`) is **refused**: the core
    exits non-zero, binds no socket, and names the root-view check. Six
    differing namespace inodes prove a helper unshared; they prove nothing
    about what it mounted, and this is the assertion that knows the difference.
    Without it, the gate above would stay green if `verify_root_view`'s
    `st_dev` comparison were deleted -- a correct helper never trips it.

## What this gate deliberately does not prove

* **Landlock and seccomp.** Neither is applied by this build (P2.6.3, P2.6.4),
  and the journal says `not-applied` in as many words. Nothing here should be
  read as evidence of a filesystem or syscall policy.
* **That the realm cannot reach the network.** `CLONE_NEWNET` is verified by
  inode above, and a loopback interface is brought up inside; whether anything
  routable remains is a separate measurement this gate does not make.
* **The GPU render node.** It is bound read-write, with its ioctl surface and
  cross-realm GPU-memory side channels intact -- a published, unmitigated cost
  of binding it at all (D-037). This gate asserts nothing about it.
* **That a realm cannot reach the session bus by other means.** `/run/user/N`
  is not in the mount table, which is a different and stronger statement than
  "the address is not advertised", but a full escape survey is not this.

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`); no new CI wiring, no cargo
feature, no injector channel.
"""

from __future__ import annotations

import errno
import os
import pathlib
import time

from harness import (
    IN_REALM_HOME,
    IN_REALM_RUNTIME_DIR,
    SUPERVISOR_COMM,
    VITRIND,
    IntegrationTest,
    await_shims,
    children_of,
    comm_of,
    exe_identity,
    fd_targets_of,
    file_identity,
    require_binaries,
)

require_binaries()

#: The real Wayland client this gate runs. `solid-client` grew `--probe` /
#: `--probe-out` for P2.6.2 rather than a fourth copy of the same wl_shm +
#: xdg-shell boilerplate being added beside it: the Wayland behaviour a
#: confinement gate needs is exactly this client's, and the three existing
#: copies justify themselves by speaking to the shim *differently*.
APP_NAME = "solid-client"

#: Where the app writes its report. Relative, so it resolves against
#: `$XDG_RUNTIME_DIR` **inside** the app -- which is `/run/vitrin` in a confined
#: realm and the host path at `--isolation=off`, and is the same host file
#: either way. That is what lets one argv, byte for byte, serve both runs.
PROBE_OUT = "confinement-probe.txt"

REALM_SIZE = "320x200"
#: Long enough for the report to land and the spine to be read; the harness's
#: own `TEST_TIMEOUT_S` is the real ceiling.
RUN_MS = "25000"
COLOUR = "0000ff"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: A device the mount table **does** bind, probed in every run. It is the
#: positive control *inside* the realm: without it, a report written by a
#: process that could open nothing at all would satisfy every negative
#: assertion in this file.
DEV_PRESENT = "/dev/null"

#: A device tree the mount table does **not** bind, though the realm still
#: holds the group that opens it (D-037(5)).
DEV_INPUT = "/dev/input"

#: The five namespace kinds the core verifies on the **supervisor**
#: (`SUPERVISOR_NAMESPACES` in `crates/vitrin-core/src/spawn.rs`). `pid` is
#: absent on purpose and is not an omission: `unshare(CLONE_NEWPID)` does not
#: move the caller, so the supervisor's own `ns/pid` is the core's forever; the
#: pid namespace is proved from the PID-1 child instead, and refusing the spawn
#: is what that proof does rather than adding a sixth name here.
SUPERVISOR_NAMESPACES = ["user", "mnt", "ipc", "uts", "net"]


def _resolve_app(shim_bin: pathlib.Path) -> str | None:
    """`solid-client`, resolved as every other rung of this ladder resolves it.

    `VITRIN_SOLID_APP` names it explicitly; otherwise it is a sibling of the C
    shim, which is where `shim/meson.build` puts it. It is co-built with the
    shim unconditionally (a bare wl_shm client, no optional dependency), so its
    absence beside a built shim is a build misconfiguration and the caller
    fails rather than skips.
    """
    explicit = os.environ.get("VITRIN_SOLID_APP")
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / APP_NAME
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


class _Probe:
    """One parsed `--probe-out` report."""

    def __init__(self, text: str) -> None:
        self.raw = text
        self.rows: dict[str, dict[str, str]] = {}
        self.groups: set[int] = set()
        self.group_count: int | None = None
        self.complete = False
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            if fields[0] == "PROBE-END":
                self.complete = True
            elif fields[0] == "PROBE-GROUPS":
                kv = dict(f.split("=", 1) for f in fields[1:] if "=" in f)
                self.group_count = int(kv.get("n", "0"))
                raw = kv.get("gids", "")
                self.groups = {int(g) for g in raw.split(",") if g}
            elif fields[0] == "PROBE":
                kv = dict(f.split("=", 1) for f in fields[1:] if "=" in f)
                self.rows[kv["path"]] = kv

    def outcome(self, path: str) -> str:
        return self.rows[path]["open"]

    def err(self, path: str) -> int:
        return int(self.rows[path]["errno"])

    def identity(self, path: str) -> tuple[int, int]:
        row = self.rows[path]
        return (int(row["dev"]), int(row["ino"]))


class _RealChain(IntegrationTest):
    """Shared setup: the same skip-or-fail policy as the rest of the ladder."""

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
        app = _resolve_app(self.shim_bin)
        if app is None:
            self.fail(
                f"no {APP_NAME} beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration. Rebuild the shim, or set "
                "VITRIN_SOLID_APP."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

        home = os.environ.get("HOME")
        if not home:
            self.fail(
                "$HOME is unset, so this gate has nowhere to put the canary the plan's "
                "criterion names. A machine without a home directory is a misconfiguration "
                "here, not a state to skip past: the core itself puts $HOME in its per-spawn "
                "canary set and would have nothing to check either."
            )
        self.home = pathlib.Path(home)

        # The canary. In the operator's real `$HOME` (owner's decision,
        # 2026-08-13) because that is the directory the criterion names and the
        # one a human means by "confined". Unique per run so two suites in
        # parallel cannot delete each other's, and removed whatever the
        # outcome.
        self.canary = self.home / f".vitrin-confinement-canary-{os.getpid()}-{time.time_ns()}"
        self.canary.write_bytes(b"vitrin confinement canary\n")
        self.addCleanup(self.canary.unlink, missing_ok=True)
        self.canary_identity = file_identity(self.canary)
        self.assertIsNotNone(self.canary_identity, "the canary was not created")

    # -- the run -----------------------------------------------------------

    def probe_argv(self) -> list[str]:
        """The app's argv. **Identical in both isolation modes, deliberately.**

        Every path here is absolute except `--probe-out`, which is resolved
        against the app's own `$XDG_RUNTIME_DIR` and therefore lands in the
        same host file whichever mode the run is in.
        """
        return [
            "--run-ms",
            RUN_MS,
            "--colour",
            COLOUR,
            "--probe",
            str(self.canary),
            "--probe",
            str(self.home),
            "--probe",
            DEV_PRESENT,
            "--probe",
            DEV_INPUT,
            "--probe-out",
            PROBE_OUT,
        ]

    def real_core(self, isolation: str):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=self.probe_argv(),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            isolation=isolation,
        )

    def report_path(self, core, realm: str = "realm-0") -> pathlib.Path:
        return core.runtime / "vitrin-0" / realm / PROBE_OUT

    def await_report(self, core, timeout: float = 30.0) -> _Probe:
        """Block until the app's report is on disk and complete.

        Completeness is `PROBE-END`, written last: a report read mid-write
        would be a partial file whose missing rows read as missing probes.
        """
        path = self.report_path(core)
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            if core.proc.poll() is not None:
                self.fail(
                    f"the core exited {core.proc.returncode} before the app reported:\n"
                    f"{core.output()}"
                )
            try:
                last = path.read_text()
            except OSError:
                last = ""
            probe = _Probe(last)
            if probe.complete:
                return probe
            time.sleep(0.05)
        self.fail(
            f"the real app never wrote a complete probe report at {path} within {timeout}s. "
            f"What was there: {last!r}. The realm's log: "
            f"{(core.runtime / 'vitrin-0' / 'realm-0' / 'realm.log').read_text()[-2000:]!r}"
        )

    def spawn_entry(self, core, realm: str = "realm-0") -> dict:
        for entry in core.entries():
            if entry["kind"] == "realm_spawned" and entry.get("realm") == realm:
                return entry
        self.fail(
            f"the flight recorder holds no realm_spawned for {realm}; kinds were {core.kinds()}"
        )


class RealConfinementCanary(_RealChain):
    """The plan's criterion, both directions, in one run."""

    def test_the_home_canary_is_unreachable_confined_and_reachable_unconfined(self):
        # (1) Non-vacuity, first and unconditionally. A canary that is not
        #     there is "unreachable" from inside the realm and from everywhere
        #     else, and a gate that skipped this would print its success line
        #     for a deleted file.
        self.assertEqual(
            self.canary.read_bytes(),
            b"vitrin confinement canary\n",
            "the canary must be readable on the host, or its absence inside the realm proves "
            "nothing at all",
        )

        confined_core = self.real_core("default")
        confined = self.await_report(confined_core)
        confined_realm_toml = confined_core.realm.read_text()
        confined_core.terminate()

        unconfined_core = self.real_core("off")
        unconfined = self.await_report(unconfined_core)
        unconfined_realm_toml = unconfined_core.realm.read_text()
        unconfined_core.terminate()

        # (6) The two runs differ in the flag and in nothing else. Asserted
        #     from the files the harness actually wrote, so a future
        #     convenience that quietly varied one realm's `args` or `binds`
        #     between the modes turns this gate red instead of weakening it.
        self.assertEqual(
            confined_realm_toml,
            unconfined_realm_toml,
            "the confined and unconfined runs must differ ONLY in --isolation; their realm "
            "configurations differ, so the comparison below would be between two different "
            "experiments",
        )

        # (5) The positive control inside the realm, before any negative is
        #     read out of the same report.
        for label, probe in (("confined", confined), ("unconfined", unconfined)):
            self.assertEqual(
                probe.outcome(DEV_PRESENT),
                "ok",
                f"the {label} app could not open {DEV_PRESENT} (errno "
                f"{probe.err(DEV_PRESENT)}). Every 'unreachable' below would then be "
                f"satisfied by an app that can open nothing; report was:\n{probe.raw}",
            )

        # (2) The criterion's negative half.
        self.assertEqual(
            confined.outcome(str(self.canary)),
            "fail",
            f"the confined app OPENED the $HOME canary {self.canary}. Report:\n{confined.raw}",
        )
        self.assertEqual(
            confined.err(str(self.canary)),
            errno.ENOENT,
            f"the confined open() must fail ENOENT ({errno.ENOENT}) -- the path is not in the "
            f"realm's filesystem at all -- not merely EACCES; report:\n{confined.raw}",
        )

        # (3) The criterion's positive half, by IDENTITY. `open=ok` alone would
        #     be satisfied by a same-named file somewhere else.
        self.assertEqual(
            unconfined.outcome(str(self.canary)),
            "ok",
            f"at --isolation=off the app must reach the canary; report:\n{unconfined.raw}",
        )
        self.assertEqual(
            unconfined.identity(str(self.canary)),
            self.canary_identity,
            "the unconfined app opened SOMETHING at the canary's path but not the canary: "
            f"(st_dev, st_ino) {unconfined.identity(str(self.canary))} is not the host's "
            f"{self.canary_identity}",
        )

        # (4) $HOME itself. Absent, or present as the mount table's own stub --
        #     never the host's inode. This is the assertion a name-based check
        #     could not state: the stub is real, and it is not a breach.
        home = str(self.home)
        host_home = file_identity(self.home)
        if confined.outcome(home) == "ok":
            self.assertNotEqual(
                confined.identity(home),
                host_home,
                "the confined app opened the operator's OWN $HOME inode. A same-named stub "
                "directory is expected (the mount table has to create it to bind the app's "
                "own directory at its host path); the host's inode is a breach",
            )
        else:
            self.assertEqual(
                confined.err(home),
                errno.ENOENT,
                f"$HOME was unopenable from inside the realm for a reason other than absence; "
                f"report:\n{confined.raw}",
            )
        self.assertEqual(
            unconfined.identity(home),
            host_home,
            "at --isolation=off the app must see the operator's own $HOME, or the positive "
            "control is not controlling for anything",
        )

        # (1, again) The canary survived both runs. A teardown that deleted it
        #     mid-test would make the second half's success meaningless.
        self.assertEqual(file_identity(self.canary), self.canary_identity)


class RealConfinementJournal(_RealChain):
    """The flight recorder carries what the **parent** read, not what the
    child claimed -- and the two isolation modes journal different shapes."""

    def test_the_recorder_carries_parent_observed_confinement_facts(self):
        core = self.real_core("default")
        self.await_report(core)

        # Read the spine independently, while the realm is alive, so the
        # journal's account of it can be checked against something other than
        # itself.
        supervisors = [
            pid for pid in children_of(core.pid) if comm_of(pid).startswith(SUPERVISOR_COMM)
        ]
        self.assertEqual(
            len(supervisors),
            1,
            f"expected exactly one realm supervisor; the core's children were "
            f"{ {pid: comm_of(pid) for pid in children_of(core.pid)} }",
        )
        observed_supervisor = supervisors[0]
        shims = await_shims(core.pid)
        self.assertEqual(len(shims), 1, f"expected one shim under the supervisor; got {shims}")
        observed_shim = shims[0]
        self.assertEqual(
            exe_identity(observed_shim),
            file_identity(self.shim_bin),
            "the confined realm's PID 1 is not the C shim this gate named -- matched by the "
            "executing file's inode, because a confined shim runs from the bind target "
            "/vitrin/shim and its comm is `shim` whichever binary it is",
        )
        core.terminate()

        iso = self.spawn_entry(core)["isolation"]
        observed = iso["parent_observed"]

        # (7) The shape of a confined spawn.
        self.assertEqual(
            iso["applied_profile"],
            "namespaces-only",
            "the profile must never be a tier name: `intra-user` is defined as namespaces "
            "PLUS Landlock PLUS seccomp, and this build applies the first third",
        )
        self.assertEqual(iso["landlock"], "not-applied (P2.6.3)")
        self.assertEqual(iso["seccomp"], "not-applied (P2.6.4)")
        self.assertEqual(
            observed["namespaces_verified"],
            SUPERVISOR_NAMESPACES,
            "the core journals the namespace kinds whose /proc/<pid>/ns inode it READ and "
            "found different from its own",
        )
        self.assertIs(observed["root_dev_differs"], True)
        self.assertIs(observed["canaries_unreachable"], True)
        self.assertGreaterEqual(
            observed["canaries_probed"],
            3,
            "the core probes its own socket, its recorder and the operator's $HOME on every "
            "single spawn; a shrinking list must be visible in the log",
        )
        self.assertIs(observed["setgroups_denied"], True)
        self.assertEqual(observed["uid_map"].split(), [str(os.geteuid()), str(os.geteuid()), "1"])
        self.assertEqual(observed["gid_map"].split(), [str(os.getegid()), str(os.getegid()), "1"])
        self.assertEqual(observed["stdio"], "per-realm log file")

        # (8) The journal's spine against procfs, read independently above.
        self.assertEqual(
            observed["supervisor_pid"],
            observed_supervisor,
            "the process the core's Child names must be the supervisor this test found in "
            "procfs, not the shim",
        )
        self.assertEqual(
            observed["shim_host_pid"],
            observed_shim,
            "the journaled shim pid must be the PID-1 child this test found under the "
            "supervisor",
        )
        self.assertNotEqual(
            observed["supervisor_pid"],
            observed["shim_host_pid"],
            "supervisor and shim are two processes; one number for both would mean the "
            "PID-namespace fork never happened",
        )

        # `writable` is measured from the child's own mountinfo, so it is a
        # list rather than a sentence. Asserted as "measured, and these are in
        # it" rather than as an exact set: the set is P2.6.9's business and
        # pinning it here would make this gate fail on a machine with a
        # different number of render nodes.
        self.assertTrue(
            observed["writable_source"].startswith("measured from"),
            f"the writable set must be measured, not described; got "
            f"{observed['writable_source']!r}",
        )
        writable = set(observed["writable"])
        self.assertLessEqual(
            {IN_REALM_RUNTIME_DIR, IN_REALM_HOME, "/tmp", "/dev/shm"},
            writable,
            f"the realm's published writable set must be present in what was measured; "
            f"measured {sorted(writable)}",
        )

        # The child-asserted half is present and labelled as such. Nothing here
        # licenses the spawn, which is exactly why the journal keeps it below
        # its own line.
        self.assertGreater(iso["child_asserted"]["mount_count"], 0)
        self.assertTrue(
            iso["child_asserted"]["mount_fingerprint"].startswith("fnv1a-64:"),
            "the fingerprint must name its algorithm, so a reader never mistakes it for one "
            "of the recorder's blake3 digests",
        )

    def test_an_unconfined_spawn_journals_absence_rather_than_a_hopeful_default(self):
        core = self.real_core("off")
        self.await_report(core)
        # Read live, before the teardown that empties `children_of`.
        direct_children = children_of(core.pid)
        core.terminate()

        iso = self.spawn_entry(core)["isolation"]
        observed = iso["parent_observed"]
        self.assertEqual(iso["applied_profile"], "none")
        self.assertEqual(observed["namespaces_verified"], [])
        self.assertIsNone(observed["root_dev_differs"])
        self.assertIsNone(observed["canaries_unreachable"])
        self.assertEqual(observed["canaries_probed"], 0)
        self.assertIsNone(observed["setgroups_denied"])
        self.assertIsNone(observed["uid_map"])
        self.assertIsNone(observed["shim_host_pid"])
        self.assertIsNone(observed["writable"])
        self.assertIn(
            "--isolation=off",
            observed["writable_source"],
            "an unmeasured writable set must say why it is unmeasured. `null` with no reason "
            "reads as `nothing is writable`, which is the opposite of the truth here",
        )
        # (9)'s sharp end: at `off` the core's own child IS the shim, so there
        # is no supervisor level and the pid the journal calls `supervisor_pid`
        # is the shim's own -- the process the core's `Child` names, which is
        # what the field has always meant. Checked against procfs rather than
        # against the journal's other half, which is null here.
        self.assertEqual(
            direct_children,
            [observed["supervisor_pid"]],
            "at --isolation=off the core forks the shim directly, with no supervisor between "
            "them, and the pid the journal names must be that one child",
        )


class RealConfinementDevices(_RealChain):
    """The published residual, measured at the app rather than described."""

    def test_the_realm_keeps_the_operators_groups_and_still_cannot_reach_dev_input(self):
        host_groups = set(os.getgroups())
        host_input = file_identity(DEV_INPUT)

        core = self.real_core("default")
        confined = self.await_report(core)
        core.terminate()

        unconfined_core = self.real_core("off")
        unconfined = self.await_report(unconfined_core)
        unconfined_core.terminate()

        # (10) The residual. `setgroups=deny` blocks the CALL; it drops
        #      nothing, and an unprivileged realm has no window in which to
        #      drop them itself (D-037(5)).
        #
        #      **The count is retained; the NAMES are unrenderable, and the
        #      difference is worth stating rather than eliding.** Inside a
        #      single-id `gid_map` every other gid is unmapped, so `getgroups()`
        #      renders each one as this kernel's `overflowgid` -- the same
        #      "nobody" rendering that made an earlier version of
        #      `vitrin-realm-init` refuse `/tmp` for being owned by root.
        #      Unmapped is not dropped: the kgids are still on the process, they
        #      are what the kernel checks against a host inode's group, and the
        #      parent reads them by their real numbers out of
        #      `/proc/<pid>/status`. So the retention is asserted three ways --
        #      the count at the app, the numbers at the parent, and an
        #      `--isolation=off` control that shows the same probe reporting the
        #      operator's real gids when there is no map to render them through.
        self.assertEqual(
            unconfined.groups,
            host_groups,
            "at --isolation=off the probe must report the operator's own supplementary "
            "groups. Without this control, the confined reading below would be a fact about "
            "the probe rather than about the realm",
        )
        self.assertEqual(
            confined.group_count,
            len(host_groups),
            "the confined realm must still hold exactly as many supplementary groups as the "
            "operator. This is a residual the project publishes, not a goal -- but if the "
            "number ever changes, `limits.md`, `SECURITY.md` and D-037(5) all need editing, "
            "and a test that tolerated the change would let all three go stale silently",
        )
        overflow = int(pathlib.Path("/proc/sys/kernel/overflowgid").read_text().strip())
        self.assertLessEqual(
            confined.groups,
            {overflow, os.getegid()},
            f"inside a single-id gid_map every retained group renders as overflowgid "
            f"({overflow}); a real gid appearing here would mean the map is wider than the "
            f"one identity the core writes. Got {sorted(confined.groups)}",
        )
        observed = self.spawn_entry(core)["isolation"]["parent_observed"]
        self.assertEqual(
            observed["supplementary_groups_retained"],
            len(host_groups),
            "the number the parent read out of /proc/<pid>/status must be the number the app "
            "itself reports from getgroups() -- the parent sees the real gids, the app sees "
            "them rendered as `nobody`, and they must agree on how many there are",
        )

        if host_input is None:
            self.skipTest(
                f"this machine has no {DEV_INPUT}, so its absence inside the realm proves "
                "nothing (an absence over a path nothing proved reachable is satisfied by no "
                "path at all). An honest machine state, not a misconfiguration: a container "
                "runner legitimately has no input devices."
            )

        # (11) The sharp reading: the group is retained AND the device tree is
        #      unreachable, so the mount table is the only barrier.
        self.assertEqual(
            confined.outcome(DEV_INPUT),
            "fail",
            f"the confined app reached {DEV_INPUT}. The realm holds the operator's groups, so "
            f"nothing but the mount table is stopping it; report:\n{confined.raw}",
        )
        self.assertEqual(confined.err(DEV_INPUT), errno.ENOENT)


class RealConfinementRefusesTheUnverifiable(_RealChain):
    """A realm the core cannot verify is refused, and the refusal is what makes
    the gate above about mounting rather than about unsharing."""

    #: The deliberately broken helper. It unshares all six namespaces, mounts
    #: nothing, and reports a well-formed handshake -- so it passes every
    #: namespace-inode check and must be caught by the root-view one.
    FIXTURE = "unshare-only-init"

    def fixture(self) -> pathlib.Path:
        path = VITRIND.parent / self.FIXTURE
        if not (path.is_file() and os.access(path, os.X_OK)):
            self.fail(
                f"{path} is missing. It is a workspace binary "
                "(crates/vitrin-realm-init-fixtures) built by the same `cargo build "
                "--workspace` that builds vitrind, which run.sh and CI both run; its absence "
                "is a build misconfiguration, not a machine state."
            )
        return path

    def test_a_helper_that_unshares_but_mounts_nothing_is_refused(self):
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=self.probe_argv(),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            realm_init=str(self.fixture()),
            wait=False,
        )
        rc = core.await_exit(timeout=60.0)
        self.assertNotEqual(
            rc,
            0,
            "a core whose confinement helper mounted nothing must not come up serving. "
            "Six differing namespace inodes prove a helper unshared; they prove nothing "
            "about what it mounted",
        )
        self.assertFalse(
            core.socket.exists(),
            "the core bound its socket despite refusing the realm: a client could have "
            "connected to a session with no realm in it",
        )
        output = core.output()
        self.assertIn(
            "same device",
            output,
            "the refusal must name the check that fired -- the realm's root being on the "
            f"host's device -- so an operator can tell it from any other startup failure. "
            f"Core said:\n{output[-3000:]}",
        )

    def test_a_leaked_directory_descriptor_does_not_survive_into_the_shim(self):
        """The other fixture: a helper that leaks an `O_DIRECTORY` handle on the
        HOST ROOT without `O_CLOEXEC`, then `execve`s the real helper.

        `openat(fd, "../../..")` and `fchdir(fd)` both work through such a
        handle, and after the `MNT_DETACH` it is the only remaining one on the
        host tree -- so this is a complete `pivot_root` escape, demonstrated
        and then observed to be closed by K13's
        `close_range(4, ~0, CLOSE_RANGE_CLOEXEC)`.

        Checked by inode, never by name: `readlink /proc/<shim>/fd/N` on such a
        handle says `/`, and `/` exists inside the realm too.
        """
        fixture = VITRIND.parent / "leaks-a-dirfd-init"
        if not (fixture.is_file() and os.access(fixture, os.X_OK)):
            self.fail(
                f"{fixture} is missing; it is built by `cargo build --workspace` "
                "(crates/vitrin-realm-init-fixtures)."
            )
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=self.probe_argv(),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            realm_init=str(fixture),
        )
        probe = self.await_report(core)
        shims = await_shims(core.pid)
        self.assertEqual(len(shims), 1, f"expected one shim; got {shims}")
        host_root = file_identity("/")
        for number, target in sorted(fd_targets_of(shims[0]).items()):
            if not target.startswith("/"):
                continue  # sockets, pipes, anon inodes: not a path handle
            identity = file_identity(f"/proc/{shims[0]}/fd/{number}")
            self.assertNotEqual(
                identity,
                host_root,
                f"the shim holds descriptor {number} on the HOST ROOT ({target}); "
                "`fchdir` through it is the whole pivot_root gone. K13's close_range must "
                "have closed it",
            )
        # And the confinement it protects still holds, on the same run.
        self.assertEqual(probe.outcome(str(self.canary)), "fail")
        core.terminate()


if __name__ == "__main__":
    import unittest

    unittest.main()
