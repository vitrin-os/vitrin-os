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
4. `$HOME` itself, under `default`, fails with **exactly one** errno, chosen
   from the mount table rather than from a menu: **EACCES** when the app lives
   under `$HOME` (the mount table had to create the stub, and no Landlock rule
   covers it), **ENOENT** when it does not (the path was never created). The
   host's own inode under `off`.
5. `/dev/null` opens in both.
6. The two runs' `realm.toml` are byte-identical.

`RealConfinementLandlockDenial` -- the ruleset, measured rather than read off
the journal:

6b. `/vitrin` is created by the mount table and granted by no Landlock rule.
    Under the shipped default the app's `open()` on it fails **EACCES**; under
    `--isolation=default --landlock=off`, same core and byte-identical
    `realm.toml`, it succeeds. `/vitrin/home` -- a hierarchy the ruleset *does*
    grant -- opens in the confined run, so the denial is the enumerated read
    set refusing a path outside it rather than a domain refusing everything.

`RealConfinementNestedUserns` -- the one property here that is about a
**syscall** rather than a path (P2.6.3, `vitrin-realm-init`'s K9b):

6c. A realm's app cannot create a user namespace inside the realm. Three runs,
    byte-identical argv: `unshare(CLONE_NEWUSER)` in a forked child fails
    **ENOSPC** at the shipped default **and** at `--isolation=default
    --landlock=off` -- so the refusal is the realm's own
    `/proc/sys/user/max_user_namespaces`, not the Landlock domain, which has no
    hook there -- and **succeeds** at `--isolation=off`, the positive control
    without which both negatives would be satisfied by a host that forbids
    unprivileged user namespaces outright. It is a *hardening* and the gate's
    docstring says so: a Landlock domain forbids `mount(2)` unconditionally, so
    a nested namespace inside a realm could never mount anything; what changes
    is that the refusal arrives at the namespace instead of at the mount, which
    is the difference between an app that degrades and an app that aborts.

`RealConfinementJournal` -- the flight recorder carries what the **parent**
observed, not what the child claimed:

7. `realm_spawned.isolation` under `default`: `applied_profile` is
   `namespaces+landlock-abiN` for the rung the realm **obtained** (never a tier
   name -- a tier is what the *machine* measured and a profile is what *this
   spawn* obtained, which stays true now that this build applies all three of
   `intra-user`'s mechanisms),
   `parent_observed.namespaces_verified` is the five the supervisor is checked
   on, `root_dev_differs` and `canaries_unreachable` are true, `writable` is a
   measured list, `landlock` is an object whose obtained rung is **at least
   this build's declared ABI floor** (`harness.LANDLOCK_MIN_ABI`, owner's
   decision 2026-08-15 — a kernel below it is refused, so a spawn that got far
   enough to journal cannot be under it) and at most the kernel's own ABI,
   which labels both numbers child-asserted and
   carries `clamped_by_build`, and `seccomp` is an object carrying the row and
   instruction counts PID 1 reported -- a SIZE and never a policy -- rather
   than the pre-P2.6.4 `not-applied` string or an absent key.
8. `shim_host_pid` and `supervisor_pid` agree with what **this process** read
   out of procfs -- the journal's account of the spine checked against an
   independent one.
9. Under `off`, every parent-observed field is null/empty and
   `writable_source` says why rather than printing a hopeful default.

**Two of (7)'s fields are shape assertions and are labelled as such at the
assertion.** `root_dev_differs` and `canaries_unreachable` cannot be journaled
`false` by any break: `verify_root_view` refuses the spawn instead, and the run
dies at `await_report` carrying the core's own refusal. They catch an absent,
`null` or defaulted key -- the `Some(true)`-beside-a-deleted-check shape
`spawn.rs`'s `RootView` docs record having been fixed -- and nothing more. The
fields with real discriminating power here are `namespaces_verified`,
`canaries_probed`, `uid_map`/`gid_map`, `supervisor_pid`/`shim_host_pid` and
`writable`, each of which was made to fail on its own (2026-08-13).

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
    exits non-zero, binds no socket, and names the root-**device** check. Six
    differing namespace inodes prove a helper unshared; they prove nothing
    about what it mounted, and this is the assertion that knows the difference.
    Without it, the gate above would stay green if `verify_root_view`'s
    `st_dev` comparison were deleted -- a correct helper never trips it.

    **The needle is `never pivoted onto its own tree`, and that is not
    cosmetic.** `verify_root_view` has two checkpoints that catch this fixture,
    and the canary loop's refusal reads "... same device N and same inode M" --
    so the `same device` needle this assertion shipped with matched *both*, and
    the assertion went green with the `st_dev` comparison deleted (measured
    2026-08-13, by deleting exactly that). Its own text above claimed the
    opposite. `spawn.rs`'s in-crate twin had already learned this and greps for
    the same needle for the same reason.

## What this gate deliberately does not prove

* **Which rung's rights a real realm has.** P2.6.3 (#187) applies a ruleset,
  and `RealConfinementLandlockDenial` above measures that it denies a reachable
  path -- so this gate does prove a denial, from inside a real realm, against a
  `--landlock=off` control in the same suite. What it does **not** prove is any
  particular rung's rights: the *number* in the journal is child-asserted and
  the parent cannot corroborate it, and no assertion here distinguishes a
  rung-9 domain from a rung-1 one. Per-rung behaviour (`TRUNCATE` at rung 3,
  `REFER` at rung 2) is measured in `vitrin-realm-init`'s own suite, where a
  forked child can enforce a capped domain and then try the syscall.
* **Seccomp.** Applied since P2.6.4, and this gate asserts only that the
  journal carries a filter of non-zero size. **Nothing here is evidence of a
  syscall policy**: the counts are child-asserted, they say what shape of
  filter was installed and not what it denies, and what a realm actually
  refuses is `test_real_seccomp.py`'s question -- table-driven, exact errno per
  row, positive control per row. `RealConfinementNestedUserns` is **not** an
  exception either: it measures one ucount limit, which the kernel enforces
  inside `create_user_ns`, and says nothing about any other syscall.
* **That a nested sandbox works.** It does not, and 6c does not claim it does.
  Mounting is denied to any process in a Landlock domain, so a realm's app
  cannot build a second boundary inside the first one -- what 6c measures is
  that the refusal is now *legible* to the libraries that ask for one.
* **That the realm cannot reach the network.** `CLONE_NEWNET` is verified by
  inode above, and a loopback interface is brought up inside; whether anything
  routable remains is a separate measurement this gate does not make.
* **The GPU render node.** It is bound read-write, with its ioctl surface and
  cross-realm GPU-memory side channels intact -- a published, unmitigated cost
  of binding it at all (D-037). P2.6.3 does **not** change that: Landlock's
  `IOCTL_DEV` is one all-or-nothing bit per hierarchy and the app needs the
  node's ioctls to render, so the ruleset grants it there. What that rung
  narrows is every other device node in the realm. This gate asserts nothing
  about either.
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
    LANDLOCK_BUILD_MAX_RUNG,
    LANDLOCK_MIN_ABI,
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

#: **The one path on this page that separates the mount table from the
#: ruleset.** `/vitrin` is created by `vitrin-realm-init` because it has to
#: hold `/vitrin/vitrin-shim` (the shim binary) and `/vitrin/home` (the realm's
#: private storage), and both of *those* are Landlock grants. The parent
#: directory is not, because the read set is enumerated rather than granted at
#: a root. So it is reachable by the mount table and denied by the ruleset --
#: which is what `RealConfinementLandlockDenial` needs and what no other probe
#: here provides: every other negative on this page is satisfied by the mount
#: table alone and would stay green with Landlock deleted.
IN_REALM_PREFIX = "/vitrin"

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
        #: `PROBE-USERNS` (report version 2, P2.6.3): `"yes"`, `"no"`,
        #: `"error"`, or `None` when the line is absent altogether -- which is
        #: what a stale `solid-client` beside a new gate looks like, and is
        #: asserted on rather than defaulted to either verdict.
        self.userns_created: str | None = None
        self.userns_errno: int | None = None
        self.complete = False
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            if fields[0] == "PROBE-END":
                self.complete = True
            elif fields[0] == "PROBE-USERNS":
                kv = dict(f.split("=", 1) for f in fields[1:] if "=" in f)
                self.userns_created = kv.get("created")
                self.userns_errno = int(kv.get("errno", "0"))
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
            "--probe",
            IN_REALM_PREFIX,
            "--probe",
            IN_REALM_HOME,
            "--probe-out",
            PROBE_OUT,
        ]

    def real_core(self, isolation: str, landlock: str | None = None):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=self.probe_argv(),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            isolation=isolation,
            landlock=landlock,
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

    def _is_under_home(self, path: str) -> bool:
        """Does `path` resolve inside the operator's `$HOME`?

        It decides which errno `$HOME` itself must answer with inside a
        confined realm, because it decides whether the mount table had to
        create that path at all: `vitrin-realm-init` binds the app's own
        directory at its host path, and making that target `mkdir -p`s every
        ancestor onto the realm's root tmpfs. Read rather than assumed, so the
        assertion stays exact on a machine where the built shim lives outside
        the operator's home.
        """
        try:
            pathlib.Path(path).resolve().relative_to(self.home.resolve())
        except ValueError:
            return False
        return True

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

        # (4) $HOME itself, and **exactly one errno is accepted**, chosen from
        #     the mount table rather than from a menu.
        #
        #     Which one is decided by one fact this test can read: whether the
        #     app's own directory lives under $HOME. If it does, the mount
        #     table has to CREATE the operator's home path inside the realm to
        #     bind the app's directory at its host path, so the stub exists and
        #     the open is a Landlock question -- no rule covers it, because the
        #     read set is enumerated and $HOME is not in the enumeration, so
        #     EACCES. If it does not, the path was never created at all and the
        #     answer is ENOENT.
        #
        #     **An `assertIn((ENOENT, EACCES))` was here and has been removed.**
        #     It passed identically whether or not the ruleset denied anything:
        #     ENOENT is what the mount table alone produces, so a build with
        #     Landlock deleted satisfied it. Under an app that lives beneath
        #     $HOME -- which is every checkout-relative run, including CI's --
        #     the branch below is now a real Landlock denial, and the
        #     `--landlock=off` control in `RealConfinementLandlockDenial`
        #     proves the same path is reachable without the ruleset.
        home = str(self.home)
        host_home = file_identity(self.home)
        app_under_home = self._is_under_home(self.app_bin)
        if app_under_home:
            self.assertEqual(
                confined.outcome(home),
                "fail",
                f"the app lives under $HOME, so the mount table created $HOME inside the "
                f"realm as a stub -- and NO Landlock rule covers it, so the open must be "
                f"refused. It succeeded, which means the ruleset granted a path outside its "
                f"enumerated read set; report:\n{confined.raw}",
            )
            self.assertEqual(
                confined.err(home),
                errno.EACCES,
                f"$HOME exists inside the realm (the mount table had to create it) and the "
                f"open must therefore fail EACCES ({errno.EACCES}) -- the Landlock ruleset "
                f"refusing a path it never granted. ENOENT here would mean the stub was not "
                f"created and this assertion is measuring the mount table instead; "
                f"report:\n{confined.raw}",
            )
        else:
            self.assertEqual(
                confined.outcome(home),
                "fail",
                f"$HOME was openable from inside the realm; report:\n{confined.raw}",
            )
            self.assertEqual(
                confined.err(home),
                errno.ENOENT,
                f"the app does not live under $HOME, so nothing created that path inside the "
                f"realm and the open must fail ENOENT ({errno.ENOENT}); "
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
        self.assertEqual(
            file_identity(self.canary),
            self.canary_identity,
            "the canary was replaced or removed while the two runs were in flight, so the "
            "second run's success is about a different file from the first run's absence",
        )


class RealConfinementLandlockDenial(_RealChain):
    """**The Landlock ruleset, measured** (P2.6.3, issue #187).

    Every other negative on this page is satisfied by the mount table alone: a
    path that is not in the realm's mount table answers `ENOENT` whether or not
    a ruleset exists, so those assertions stay green with Landlock deleted.
    This one does not. It opens a path the mount table **creates** and the
    ruleset **does not grant**, so the only thing that can refuse it is the
    domain.

    Two runs, identical in everything but one flag:

    * `--isolation=default` (the shipped default, `--landlock=highest`): the
      open must fail `EACCES`.
    * `--isolation=default --landlock=off`: the **same** core, the **same**
      mount table, byte-identical argv and `realm.toml` -- and the open must
      succeed.

    The control is not a nicety. Without it, `EACCES` could mean the mount
    table put something unreadable there, or the app crashed before it looked,
    or the path never existed; with it, the same path in the same realm shape
    is reachable exactly when the ruleset is absent. `/dev/null` is probed in
    both runs as the second control, so a report written by a process that
    could open nothing cannot satisfy the negative.

    And the pairing is checked in the other direction too: `/vitrin/home` --
    a hierarchy the ruleset **does** grant -- is reachable in the confined run.
    So the denial is the *enumeration* refusing a path outside it, not a domain
    that refuses everything.
    """

    def test_a_path_the_mount_table_leaves_reachable_is_denied_by_the_ruleset(self):
        confined_core = self.real_core("default")
        confined = self.await_report(confined_core)
        confined_realm_toml = confined_core.realm.read_text()
        confined_entry = self.spawn_entry(confined_core)
        confined_core.terminate()

        unruled_core = self.real_core("default", landlock="off")
        unruled = self.await_report(unruled_core)
        unruled_realm_toml = unruled_core.realm.read_text()
        unruled_entry = self.spawn_entry(unruled_core)
        unruled_core.terminate()

        # The two runs differ in `--landlock` and in nothing else, asserted
        # from the files the harness wrote rather than assumed.
        self.assertEqual(
            confined_realm_toml,
            unruled_realm_toml,
            "the two runs must differ ONLY in --landlock; their realm configurations differ, "
            "so the comparison below is between two different experiments",
        )

        # The inside-the-realm positive control, before any negative is read
        # out of the same report.
        for label, probe in (("default", confined), ("--landlock=off", unruled)):
            self.assertEqual(
                probe.outcome(DEV_PRESENT),
                "ok",
                f"the {label} app could not open {DEV_PRESENT} (errno "
                f"{probe.err(DEV_PRESENT)}); every 'denied' below would then be satisfied by "
                f"an app that can open nothing. Report:\n{probe.raw}",
            )

        # The control: with no ruleset, the mount table leaves this reachable.
        self.assertEqual(
            unruled.outcome(IN_REALM_PREFIX),
            "ok",
            f"at --landlock=off the realm must be able to open {IN_REALM_PREFIX} -- the mount "
            f"table creates it, and nothing else was changed. If it cannot, the denial in the "
            f"other run is about the mount table and proves nothing about Landlock. "
            f"Report:\n{unruled.raw}",
        )

        # The measurement: with the ruleset, the same path is refused, and
        # refused with the errno a Landlock denial produces rather than the
        # one an absent path produces.
        self.assertEqual(
            confined.outcome(IN_REALM_PREFIX),
            "fail",
            f"the confined realm opened {IN_REALM_PREFIX}, which no Landlock rule grants -- "
            f"the read set is enumerated and this directory is not in the enumeration. "
            f"Report:\n{confined.raw}",
        )
        self.assertEqual(
            confined.err(IN_REALM_PREFIX),
            errno.EACCES,
            f"{IN_REALM_PREFIX} must be refused EACCES ({errno.EACCES}) -- present in the "
            f"realm's mount table and denied by its Landlock domain. ENOENT here would mean "
            f"the mount table never created it, and this gate would be measuring the mount "
            f"table again. Report:\n{confined.raw}",
        )

        # The other direction, in the same run: a hierarchy the ruleset DOES
        # grant is reachable. Without this the assertion above would also pass
        # for a domain that granted nothing at all.
        self.assertEqual(
            confined.outcome(IN_REALM_HOME),
            "ok",
            f"the confined realm could not open its OWN granted storage {IN_REALM_HOME} "
            f"(errno {confined.err(IN_REALM_HOME)}); the denial above is then a blanket one "
            f"rather than the enumerated read set refusing a path outside it. "
            f"Report:\n{confined.raw}",
        )

        # And the journals say which run was which, so a future change that
        # made `--landlock=off` a no-op cannot leave this gate comparing one
        # configuration against itself.
        confined_landlock = confined_entry["isolation"]["landlock"]
        unruled_landlock = unruled_entry["isolation"]["landlock"]
        self.assertEqual(confined_landlock["requested"], "highest")
        self.assertEqual(unruled_landlock["requested"], "off")
        self.assertGreaterEqual(
            confined_landlock["obtained_rung"],
            1,
            "the run that denied the path reports no ruleset, so something other than "
            "Landlock refused it",
        )
        self.assertEqual(
            unruled_landlock["obtained_rung"],
            0,
            "the control run reports a ruleset, so it is not a control",
        )
        self.assertEqual(
            unruled_entry["isolation"]["applied_profile"],
            "namespaces-only",
            "a session with no ruleset must not journal a profile that names Landlock",
        )
        self.assertEqual(
            confined_entry["isolation"]["applied_profile"],
            f"namespaces+landlock-abi{confined_landlock['obtained_rung']}",
            "the profile must name the rung the realm OBTAINED, so a ladder fallback is "
            "visible in the field named for what was applied",
        )


class RealConfinementNestedUserns(_RealChain):
    """**A realm's app cannot create a user namespace inside the realm**
    (P2.6.3, `vitrin-realm-init`'s K9b).

    Three runs, one app, byte-identical argv:

    * `--isolation=default` (the shipped default): `unshare(CLONE_NEWUSER)` in
      a forked child must fail, with `ENOSPC` -- the errno a ucount limit
      produces, and the one thing here that names the *mechanism* rather than
      merely an absence.
    * `--isolation=default --landlock=off`: the same refusal. This arm is what
      stops the gate from crediting Landlock for a denial Landlock does not
      make: the ruleset has no hook on namespace creation, and the realm's own
      `max_user_namespaces` is what refuses.
    * `--isolation=off`: the **positive control**. The same probe, the same
      binary, on the same host, must SUCCEED -- or "cannot create one" is a
      statement about this machine's `kernel.unprivileged_userns_clone` rather
      than about the realm, and the two negatives above prove nothing.

    `/dev/null` is probed in every run as the second control, on the same terms
    as the rest of this module: a report written by a process that could do
    nothing at all would satisfy both negatives.

    **What this does not claim.** It does not claim a nested sandbox used to
    work and now does not: a Landlock filesystem domain forbids `mount(2)`
    unconditionally, so a nested user namespace inside a realm could never
    create a mount and was already useless. What it measures is that the
    refusal now arrives at the namespace instead of at the mount -- which is
    the difference between an app that degrades and an app that aborts, and is
    why `docs/book/src/limits.md`'s `landlock-breaks-nested-image-sandboxes`
    reads the way it now does.
    """

    def test_a_realms_app_cannot_create_a_nested_user_namespace(self):
        confined_core = self.real_core("default")
        confined = self.await_report(confined_core)
        confined_realm_toml = confined_core.realm.read_text()
        confined_core.terminate()

        unruled_core = self.real_core("default", landlock="off")
        unruled = self.await_report(unruled_core)
        unruled_realm_toml = unruled_core.realm.read_text()
        unruled_core.terminate()

        unconfined_core = self.real_core("off")
        unconfined = self.await_report(unconfined_core)
        unconfined_realm_toml = unconfined_core.realm.read_text()
        unconfined_core.terminate()

        # The three runs differ in the flags and in nothing else, asserted from
        # the files the harness wrote.
        self.assertEqual(confined_realm_toml, unruled_realm_toml)
        self.assertEqual(
            confined_realm_toml,
            unconfined_realm_toml,
            "the three runs must differ ONLY in --isolation/--landlock; their realm "
            "configurations differ, so the comparison below is between different experiments",
        )

        # The inside-the-app positive control, before any negative is read out
        # of the same report.
        for label, probe in (
            ("default", confined),
            ("--landlock=off", unruled),
            ("--isolation=off", unconfined),
        ):
            self.assertEqual(
                probe.outcome(DEV_PRESENT),
                "ok",
                f"the {label} app could not open {DEV_PRESENT} (errno "
                f"{probe.err(DEV_PRESENT)}); every verdict below would then be about a "
                f"process that could do nothing. Report:\n{probe.raw}",
            )
            self.assertIsNotNone(
                probe.userns_created,
                f"the {label} report carries no PROBE-USERNS line at all, so the probe never "
                f"ran. That is a STALE solid-client beside a new gate (report version 2 added "
                f"the line); rebuild the shim tree. Report:\n{probe.raw}",
            )

        # The positive control. Stated first, because both negatives are empty
        # without it: on a host that forbids unprivileged user namespaces
        # outright, a realm's app would fail this probe with no help from the
        # realm at all.
        self.assertEqual(
            unconfined.userns_created,
            "yes",
            "at --isolation=off the app must be ABLE to create a user namespace (errno "
            f"{unconfined.userns_errno}). If it cannot, this host forbids unprivileged user "
            "namespaces generally and the two refusals below are facts about the host, not "
            f"about the realm. Report:\n{unconfined.raw}",
        )

        # The measurement.
        self.assertEqual(
            confined.userns_created,
            "no",
            "the confined realm's app created a user namespace INSIDE the realm. K9b writes 0 "
            "to the realm's own /proc/sys/user/max_user_namespaces precisely so that a nested "
            f"sandbox is refused at the namespace rather than at its first mount. "
            f"Report:\n{confined.raw}",
        )
        self.assertEqual(
            confined.userns_errno,
            errno.ENOSPC,
            f"the refusal must be ENOSPC ({errno.ENOSPC}) -- what create_user_ns returns when "
            f"the ucount limit is exceeded, and therefore the errno that names K9b as the "
            f"cause. EPERM here would mean something else refused (a host policy, a seccomp "
            f"filter) and this gate would be measuring that instead. Report:\n{confined.raw}",
        )

        # The third arm: the same refusal with no Landlock domain at all, so
        # the denial cannot be credited to the ruleset.
        self.assertEqual(
            unruled.userns_created,
            "no",
            "at --isolation=default --landlock=off the refusal must still hold: it comes from "
            "the realm's own ucount limit, not from the Landlock domain, and a gate that let "
            f"this arm pass would be attributing K9b's work to the ruleset. "
            f"Report:\n{unruled.raw}",
        )
        self.assertEqual(
            unruled.userns_errno,
            errno.ENOSPC,
            f"the same mechanism must produce the same errno with no ruleset in the picture; "
            f"report:\n{unruled.raw}",
        )


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
            "/vitrin/vitrin-shim and its comm is that basename whichever binary it is",
        )
        core.terminate()

        iso = self.spawn_entry(core)["isolation"]
        observed = iso["parent_observed"]

        # P2.6.3 (#187) LANDED, and this block is what its tripwire became.
        # The journal's `landlock` key stopped being the fixed string
        # "not-applied (P2.6.3)" and became an object, because one string
        # cannot carry the numbers a reader needs.
        landlock = iso["landlock"]
        self.assertIsInstance(
            landlock,
            dict,
            "since P2.6.3 the journal's `landlock` key is an object carrying what the session "
            "REQUESTED and what the realm's PID 1 reported it OBTAINED; a bare string here "
            "means the entry lost half the pair",
        )
        self.assertEqual(
            landlock["requested"],
            "highest",
            "this gate runs the shipped default, which asks for the highest rung this build "
            "knows that the kernel accepts",
        )
        self.assertGreaterEqual(
            landlock["obtained_rung"],
            LANDLOCK_MIN_ABI,
            f"a confined spawn at the shipped default must obtain at least this build's "
            f"declared ABI floor ({LANDLOCK_MIN_ABI}); anything lower means the session "
            f"started somewhere it should have been refused, or the helper degraded to a rung "
            f"the core's own preflight ruled out. Rung 0 specifically is what "
            f"`--landlock=off` journals",
        )
        self.assertLessEqual(
            landlock["obtained_rung"],
            landlock["kernel_abi"],
            "the rung obtained cannot exceed the ABI the same child read from the same kernel",
        )
        self.assertEqual(
            landlock["rung_evidence"],
            "child-asserted",
            "both numbers come from the child, and the entry has to say so: there is no "
            "/proc file naming a process's Landlock domain, so the parent cannot corroborate "
            "them the way it corroborates the namespace inodes",
        )
        # Whether this BUILD's ladder is what held the rung down -- a parent
        # conclusion about the child's number, and a boolean rather than a
        # missing key, so an operator on a kernel newer than this build sees
        # the clamp instead of inferring it from two numbers.
        self.assertIn(
            landlock["clamped_by_build"],
            (True, False),
            "the clamp must be reported as a boolean, not omitted or null: it is computed "
            "for every confined spawn, and until it was journaled it was computed and thrown "
            "away",
        )
        self.assertEqual(
            landlock["clamped_by_build"],
            landlock["kernel_abi"] > LANDLOCK_BUILD_MAX_RUNG,
            f"the clamp must agree with the numbers beside it: it is true exactly when this "
            f"kernel's ABI ({landlock['kernel_abi']}) is above the highest rung this build "
            f"knows ({LANDLOCK_BUILD_MAX_RUNG}), and is independent of what the session "
            f"asked for",
        )
        # (7) The shape of a confined spawn: the profile names the rung the
        # realm OBTAINED, not the one the session asked for. A profile derived
        # from the request would read the same at rung 9 and at rung 1.
        self.assertEqual(
            iso["applied_profile"],
            f"namespaces+landlock-abi{landlock['obtained_rung']}",
            "the profile must name the obtained rung, and must never be a tier name: "
            "`intra-user` is defined as namespaces PLUS Landlock PLUS seccomp, and this "
            "build applies the first two",
        )
        self.assertNotIn(
            "intra-user",
            iso["applied_profile"],
            "a per-realm profile named a TIER. The reason changed at P2.6.4 and the rule did "
            "not: the tier is what the MACHINE measured and the profile is what THIS SPAWN "
            "obtained, so the day the two agree in value is not the day one may be printed "
            "for the other",
        )
        # This tripwire FIRED, as it was written to. It asserted the string
        # `not-applied (P2.6.4)` and demanded that whoever landed P2.6.4 edit
        # this gate's module docs and `docs/book/src/limits.md` with the same
        # commit. Both were edited; what it asserts now is the other side of
        # the same property -- the journal must carry a filter, and it must
        # carry it as a SIZE rather than as a policy. What the filter actually
        # denies is `test_real_seccomp.py`'s question, with a positive control
        # per row; nothing here is evidence about a syscall.
        self.assertIsInstance(
            iso["seccomp"],
            dict,
            "`isolation.seccomp` is still the pre-P2.6.4 `not-applied` string while the helper "
            f"installs a filter, or the key changed shape again: {iso['seccomp']!r}",
        )
        self.assertGreater(
            iso["seccomp"]["rows"],
            0,
            "a filter of zero rows denies nothing, and journaling it would publish confinement "
            "this session did not apply",
        )
        self.assertEqual(
            iso["seccomp"]["evidence"],
            "child-asserted",
            "no /proc interface reports a process's seccomp RULES -- only its mode -- so the "
            "parent cannot corroborate these counts and the journal must say so",
        )
        self.assertEqual(
            observed["namespaces_verified"],
            SUPERVISOR_NAMESPACES,
            "the core journals the namespace kinds whose /proc/<pid>/ns inode it READ and "
            "found different from its own",
        )
        # **These two are shape assertions, and saying so is the honest label.**
        # `verify_root_view` returns `Err` the moment either comparison fails,
        # so a confined spawn that got far enough for the app to write a report
        # cannot journal `false` here -- the run would have died at
        # `await_report` instead, with the core's own refusal in the message.
        # What they still catch is a refactor that made either key absent,
        # `null`, or defaulted at `--isolation=default`, which is exactly the
        # `Some(true)`-beside-a-deleted-check shape `spawn.rs`'s `RootView`
        # docs record having been fixed. Measured 2026-08-13: no core or helper
        # break makes them fail as booleans.
        self.assertIs(
            observed["root_dev_differs"],
            True,
            "a confined spawn must journal the root-device comparison as a boolean it ran, "
            "not as an absent or null key",
        )
        self.assertIs(
            observed["canaries_unreachable"],
            True,
            "a confined spawn must journal the canary probe's own verdict as a boolean it "
            "ran, not as an absent or null key",
        )
        self.assertGreaterEqual(
            observed["canaries_probed"],
            3,
            "the core probes its own socket, its recorder and the operator's $HOME on every "
            "single spawn; a shrinking list must be visible in the log",
        )
        self.assertIs(
            observed["setgroups_denied"],
            True,
            "`/proc/<pid>/setgroups` must read back `deny`: it is the precondition for an "
            "unprivileged single-id gid map existing at all, so a realm journaling anything "
            "else has a map this core did not write",
        )
        # The single identity line, and nothing wider. `0 <euid> 1` would be
        # namespace-root and would hand the app CAP_SYS_ADMIN inside its own
        # user namespace; a multi-id map is not a shape an unprivileged writer
        # can produce at all. Read from the journal, so this also catches a
        # core that verified one string and recorded another.
        self.assertEqual(
            observed["uid_map"].split(),
            [str(os.geteuid()), str(os.geteuid()), "1"],
            "the journaled uid_map must be the single identity line this core wrote and "
            "verified; anything else means the recorded value is not the verified one",
        )
        self.assertEqual(
            observed["gid_map"].split(),
            [str(os.getegid()), str(os.getegid()), "1"],
            "the journaled gid_map must be the single identity line this core wrote and "
            "verified; anything else means the recorded value is not the verified one",
        )
        self.assertEqual(
            observed["stdio"],
            "per-realm log file",
            "a confined realm's stdout and stderr must go to its own log file. Inheriting the "
            "core's would be the operator's tty on a real session, and no mount flag revokes "
            "a descriptor",
        )

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
        self.assertGreater(
            iso["child_asserted"]["mount_count"],
            0,
            "the child's own post-pivot /proc/self/mountinfo must have counted something. "
            "Zero would mean the helper sent a number it never read -- which is exactly why "
            "this half of the entry is labelled `child_asserted` and licenses nothing",
        )
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
        # Every one of these carries its own message, because a bare
        # `assertIsNone` fails as `True is not None` and names neither the field
        # nor the claim -- and this whole test method is *about* one claim
        # (nothing was measured, so nothing is reported) restated over nine
        # fields, which is precisely the case where the reader needs to be told
        # which of the nine moved.
        unmeasured = (
            "at --isolation=off nothing was applied and therefore nothing was measured; a "
            "value here is a hopeful default, and a hopeful default in a confinement journal "
            "reads as evidence"
        )
        self.assertEqual(
            iso["applied_profile"],
            "none",
            "an unconfined spawn must journal `none` as its profile, not the name of a "
            "profile it did not apply",
        )
        self.assertEqual(
            observed["namespaces_verified"],
            [],
            "an unconfined spawn unshares nothing, so the verified-namespace list must be "
            "empty rather than carrying names nothing read",
        )
        # The Landlock object, present with every field null rather than
        # omitted -- the recorder's rule that absent information is an explicit
        # value. At `--isolation=off` no helper runs, so nothing was asked for
        # and nothing was obtained.
        #
        # `rung_evidence` is in this list since #187's adversarial pass, and it
        # is the one field here that used to carry a value: the recorder wrote
        # `"child-asserted"` unconditionally, so the object labelled four nulls
        # with the provenance of a number no child had sent. The key is still
        # written -- `iso["landlock"]["rung_evidence"]` would KeyError if it
        # were dropped, which is the same rule stated the other way.
        for field in (
            "requested",
            "obtained_rung",
            "kernel_abi",
            "clamped_by_build",
            "rung_evidence",
        ):
            self.assertIsNone(iso["landlock"][field], f"landlock.{field}: {unmeasured}")
        self.assertIsNone(observed["root_dev_differs"], f"root_dev_differs: {unmeasured}")
        self.assertIsNone(observed["canaries_unreachable"], f"canaries_unreachable: {unmeasured}")
        self.assertEqual(
            observed["canaries_probed"],
            0,
            "an unconfined spawn has no realm root to probe canaries through, so the count "
            "must be 0 rather than the number the confined path would have used",
        )
        self.assertIsNone(observed["setgroups_denied"], f"setgroups_denied: {unmeasured}")
        self.assertIsNone(observed["uid_map"], f"uid_map: {unmeasured}")
        self.assertIsNone(
            observed["shim_host_pid"],
            "at --isolation=off there is no PID-1 child to name; the one pid this spawn has "
            "is journaled as supervisor_pid, asserted against procfs below",
        )
        self.assertIsNone(observed["writable"], f"writable: {unmeasured}")
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
        self.assertEqual(
            confined.err(DEV_INPUT),
            errno.ENOENT,
            f"{DEV_INPUT} must be absent from the realm's filesystem (ENOENT), not merely "
            f"unopenable (EACCES): the realm holds the operator's `input` group, so a "
            f"permission refusal would be the kernel disagreeing with the group list rather "
            f"than the mount table doing the work. Report:\n{confined.raw}",
        )


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
        # The needle is unique to the ROOT-DEVICE refusal, and that is the whole
        # point of it. `verify_root_view` has two checkpoints that can catch this
        # fixture, and the canary loop's message says "... same device N and same
        # inode M" -- so a needle of `same device` matches BOTH, and this
        # assertion went green with `verify_root_view`'s `st_dev` comparison
        # deleted (measured, 2026-08-13, by deleting exactly that). Its own
        # docstring above claims the opposite, which is how a message assertion
        # comes to be decoration. `spawn.rs`'s in-crate twin
        # (`a_helper_that_unshares_but_mounts_nothing_is_refused`) had already
        # learned this and greps for the same needle for the same reason.
        self.assertIn(
            "never pivoted onto its own tree",
            output,
            "the refusal must name the check that fired -- the realm's root being on the "
            "host's device -- so an operator can tell it from any other startup failure, and "
            "so that deleting that check cannot be covered for by the canary loop, which "
            f"refuses this same fixture one step later. Core said:\n{output[-3000:]}",
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
        self.assertEqual(
            probe.outcome(str(self.canary)),
            "fail",
            "the confinement the closed descriptor protects must still hold on this same run "
            f"-- a leak test over an unconfined realm proves nothing. Report:\n{probe.raw}",
        )
        core.terminate()


if __name__ == "__main__":
    import unittest

    unittest.main()
