# SPDX-License-Identifier: Apache-2.0
"""**Can a realm reach the core's principal socket?** — a measurement (#311).

This module answers no question. It produces the evidence
[D-038](../../docs/plan/20-decision-log.md) says must exist before the question
is answered:

> **The unanswered question is routed, not guessed: may a realm reach the core's
> principal socket, and under what authority?**

The three candidate mechanisms D-038 names — a grant, a mount, a bind — are a
choice about what to *build*. What nothing in the tree states is what the kernel
does **today**, and a decision taken without that is a decision about an
imagined starting point. So this file measures the starting point and stops
there. It adds no confinement code, changes no policy, and asserts nothing about
what the answer should be.

## What is probed

The realm's app is a Python probe (`p311_realm_probe.py`) — copied into a
scratch directory the realm's `binds` list names, so the only thing this adds to
a realm is the adversary's own code, which every realm has by construction. It
enumerates, with an errno for each:

* the composed environment, from `os.environ` **and** `/proc/self/environ`;
* every named path a program could compute for the core socket — the injected
  `$XDG_RUNTIME_DIR` plus `..`, the host absolute path, the production
  `/run/user/<uid>/vitrin-0/core.sock` convention, `/run/vitrin/..`, and the
  `/proc/self/root` and `/proc/1/root` magic links;
* the **abstract** Unix namespace, which is scoped by the network namespace and
  by nothing else: `/proc/net/unix` and a `connect` to every abstract name in it;
* the mount table (`/proc/self/mountinfo`), verbatim;
* every visible pid, its `/proc/<pid>/root` and `/proc/<pid>/cwd`, and a
  `connect` through that root at the host's core-socket path;
* every descriptor that survived the `execve`, and whether any is a socket;
* a **walk** of the realm's whole filesystem for AF_UNIX socket inodes, with a
  `connect` to each. This is the route a candidate list cannot cover — "any path
  the mount table happens to expose" — and it is the reason the negative here is
  a measurement rather than a list of paths somebody thought of.

Anything that connects is then driven all the way through the shipped SDK's
`hello`, twice: with the deployment's real credential and with a wrong one.
**Reachable-but-rejected and reachable-and-accepted are different answers**, and
a probe that stopped at `connect` could not tell them apart.

## Three runs, one argv, and why the controls are not optional

`tests/integration/README.md` §4 and `test_real_confinement.py` both say it:
a negative claim needs a positive control **in the same run**, because an
absence over a path nothing proved reachable is satisfied by no path at all. A
probe that reported "no route" because its socket layer never worked, or because
it was pointed at a core that was not listening, would print the same lines as a
confined realm.

So there are three controls, at three different depths:

1. **The socket layer, inside the realm.** `WAYLAND_DISPLAY` is a socket the app
   is *given*. It must `connect` in **both** runs. Without it every negative
   here would be satisfied by a probe that could open no socket at all.
2. **The core socket, from the host.** In both runs the harness itself binds a
   principal connection over the very socket the realm is being measured
   against. Without it a negative would be satisfied by a core that was not
   listening.
3. **The same probe at `--isolation=off`.** The whole enumeration, byte-identical
   argv, with the confinement removed. This is what turns "the confined realm
   could not reach it" from an assertion into a difference.

There is a third arm for a reason found by running the first two: at the shipped
default a Landlock domain denies `READ_DIR` on the realm root, so the filesystem
walk enumerates **nothing** and "no socket found" would have meant "I was not
allowed to look". `--isolation=default --landlock=off` is the arm that looks —
same confinement, same argv, mount table alone — and it is what makes the walk's
negative exhaustive rather than hollow. It is also the arm that separates the two
barriers, which matters because #311's answer may be built on either.

All three runs share **one** runtime tree, sequentially, so the host absolute
path in the probe's argv is the same string in each — which is what lets the
three `realm.toml` files be compared byte for byte. Only the core's own flags
differ.

## What this file is not

Not a milestone gate (no milestone is scheduled here), not a `known-limit`
closure, and not a property gate: it asserts the state of an **open** question
rather than a property the project publishes. It carries no `test_` prefix for that reason: `run.sh`'s three-way gate
partition (issue #288) requires every `test_*.py` on disk to be a milestone,
property or supporting gate, and this is none of them — it is a measurement run
explicitly, not collected by `unittest discover`. Run it with::

    VITRIN_C_SHIM_BIN=shim/build/vitrin-shim PYTHONPATH=sdk/python/src \
      python3 -m unittest tests.integration.p311_principal_socket_reach -v

If #311 is decided and the decision has a property, that property gets its own
`test_*.py` gate and this file is superseded by it.
"""

from __future__ import annotations

import base64
import errno
import os
import pathlib
import shutil
import tempfile
import time

from harness import (
    DEMO_IDENTITY,
    DEMO_TOKEN,
    IntegrationTest,
    require_binaries,
)

require_binaries()

REPO = pathlib.Path(__file__).resolve().parents[2]

#: The interpreter the realm runs. `/usr/bin/python3` deliberately, not
#: `sys.executable`: `app_dir_to_bind` returns `None` for anything under `/usr`
#: (`crates/vitrin-core/src/spawn.rs`), so the realm's mount table gains nothing
#: for the interpreter itself and the only thing this measurement adds is the
#: one scratch bind holding the probe.
PYTHON = "/usr/bin/python3"

#: The probe's report, resolved by the app against its own `$XDG_RUNTIME_DIR` —
#: `/run/vitrin` in a confined realm, the host path at `--isolation=off`, the
#: same host file either way. That is what lets one argv serve both runs.
PROBE_OUT = "p311-principal-socket-probe.txt"

#: Long enough that the host side can bind its control connection and read the
#: report while the realm is still up.
HOLD_MS = "25000"

#: A credential the registry does not hold. The control that makes a `bound`
#: result mean something: a core that bound anything would bind this too.
BAD_TOKEN = "b" * 64

REALM_SIZE = "320x200"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: Labels whose target is the shim's own Wayland socket. They are the in-realm
#: positive control and are never expected to be denied.
CONTROL_LABELS = ("wayland-display",)


class Report:
    """One parsed probe report."""

    def __init__(self, text: str) -> None:
        self.raw = text
        self.records: list[tuple[str, dict[str, str]]] = []
        self.complete = False
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            kind = fields[0]
            kv = dict(f.split("=", 1) for f in fields[1:] if "=" in f)
            self.records.append((kind, kv))
            if kind == "P311-END":
                self.complete = kv.get("ok") == "1"

    def of(self, kind: str) -> list[dict[str, str]]:
        return [kv for k, kv in self.records if k == kind]

    def one(self, kind: str) -> dict[str, str]:
        rows = self.of(kind)
        assert len(rows) == 1, f"{kind}: expected exactly one record, got {len(rows)}"
        return rows[0]

    def env(self) -> dict[str, str]:
        return {
            _un(row["name"]): _un(row["value"]) for row in self.of("P311-ENV")
        }

    def connects(self) -> dict[str, tuple[int, int]]:
        return {
            row["label"]: (int(row["connect"]), int(row["errno"]))
            for row in self.of("P311-CONNECT")
        }

    def paths(self) -> dict[str, dict[str, str]]:
        return {row["label"]: row for row in self.of("P311-PATH")}

    def ns(self) -> dict[str, str]:
        return {row["kind"]: _un(row["link"]) for row in self.of("P311-NS") if row["ok"] == "1"}

    def handshakes(self) -> dict[tuple[str, str], tuple[str, str]]:
        return {
            (row["label"], row["cred"]): (row["result"], _un(row["detail"]))
            for row in self.of("P311-HANDSHAKE")
        }


def _un(value: str) -> str:
    if not value:
        return ""
    return base64.b64decode(value).decode("utf-8", "replace")


class Arm:
    """One run's settings, and everything harvested from it."""

    def __init__(self, name: str, isolation: str, landlock: str | None) -> None:
        self.name = name
        self.isolation = isolation
        self.landlock = landlock
        self.report: Report | None = None
        self.realm_toml = ""
        self.host_bound = ""
        self.core_ns_net = ""
        self.entries: list[dict] = []


class RealmReachesPrincipalSocket(IntegrationTest):
    """The measurement. One test, three arms, one run."""

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
                "Build it (meson setup shim/build shim && meson compile -C shim/build) and "
                "point the variable at shim/build/vitrin-shim."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim.")
        if not (os.path.isfile(PYTHON) and os.access(PYTHON, os.X_OK)):
            self.fail(
                f"{PYTHON} is not an executable file. This measurement runs its probe as the "
                "realm's app and the interpreter must be the system one under /usr, because "
                "`app_dir_to_bind` returns None there and the realm's mount table therefore "
                "gains nothing for it."
            )

        # The realm's app, and the SDK it speaks Vitrin with, copied rather than
        # bound from the checkout: binding `tests/integration/` would put the
        # repository inside the realm, which is a far larger hole than the
        # measurement needs and would contaminate what it measures.
        self.scratch = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-p311-app-"))
        self.addCleanup(shutil.rmtree, self.scratch, ignore_errors=True)
        os.chmod(self.scratch, 0o755)
        shutil.copy2(pathlib.Path(__file__).parent / "p311_realm_probe.py", self.scratch)
        shutil.copytree(
            REPO / "sdk" / "python" / "src" / "vitrin_os",
            self.scratch / "vitrin_os",
            ignore=shutil.ignore_patterns("__pycache__"),
        )
        for path in self.scratch.rglob("*"):
            os.chmod(path, 0o644 if path.is_file() else 0o755)

        # ONE runtime tree for both arms, used sequentially. That is what makes
        # the probe's argv byte-identical across the two runs: the host absolute
        # path of the core socket is the same string in each.
        base = pathlib.Path(os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir())
        if not os.access(base, os.W_OK):
            base = pathlib.Path(tempfile.gettempdir())
        self.runtime = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-p311-rt-", dir=base))
        self.addCleanup(shutil.rmtree, self.runtime, ignore_errors=True)
        self.core_sock = self.runtime / "vitrin-0" / "core.sock"

        self.evidence = os.environ.get("VITRIN_P311_EVIDENCE_DIR")
        if self.evidence:
            os.makedirs(self.evidence, exist_ok=True)

    # -- one arm ------------------------------------------------------------

    def probe_argv(self) -> list[str]:
        """The app's argv. **Identical in both isolation settings, deliberately.**"""
        return [
            "-B",
            str(self.scratch / "p311_realm_probe.py"),
            "--out",
            PROBE_OUT,
            "--core-sock",
            str(self.core_sock),
            "--identity",
            DEMO_IDENTITY,
            "--token",
            DEMO_TOKEN,
            "--bad-token",
            BAD_TOKEN,
            "--hold-ms",
            HOLD_MS,
        ]

    def run_arm(self, name: str, isolation: str, landlock: str | None = None) -> Arm:
        arm = Arm(name, isolation, landlock)
        recorder = self.runtime / "flight.jsonl"
        if recorder.exists():
            recorder.unlink()
        core = self.core(
            size=REALM_SIZE,
            runtime_dir=str(self.runtime),
            shim=str(self.shim_bin),
            command=PYTHON,
            args=self.probe_argv(),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            binds=(str(self.scratch),),
            isolation=isolation,
            landlock=landlock,
        )
        arm.realm_toml = (self.runtime / "realm.toml").read_text()
        report_path = self.runtime / "vitrin-0" / "realm-0" / PROBE_OUT
        arm.report = self._await_report(core, report_path)

        # Control 2: the core socket is live and binds the demo principal, from
        # the host, while this very realm is running. Without it "the realm
        # could not reach it" is satisfied by a core that was not listening.
        with core.connect() as conn:
            arm.host_bound = conn.identity
        arm.core_ns_net = os.readlink(f"/proc/{core.proc.pid}/ns/net")

        core.terminate()
        arm.entries = core.entries()
        if self.evidence:
            out = pathlib.Path(self.evidence)
            (out / f"report-{name}.txt").write_text(arm.report.raw)
            (out / f"realm-{name}.toml").write_text(arm.realm_toml)
            (out / f"core-{name}.log").write_text(core.output())
        return arm

    def _await_report(self, core, path: pathlib.Path, timeout: float = 45.0) -> Report:
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            if core.proc.poll() is not None:
                self.fail(
                    f"the core exited {core.proc.returncode} before the probe reported:\n"
                    f"{core.output()}"
                )
            try:
                last = path.read_text()
            except OSError:
                last = ""
            report = Report(last)
            if report.complete:
                return report
            time.sleep(0.05)
        realm_log = self.runtime / "vitrin-0" / "realm-0" / "realm.log"
        tail = realm_log.read_text()[-4000:] if realm_log.exists() else "<no realm log>"
        self.fail(
            f"the probe never wrote a complete report at {path} within {timeout}s. "
            f"What was there: {last[-2000:]!r}\nThe realm's log:\n{tail}"
        )

    # -- the measurement ----------------------------------------------------

    def test_a_realm_reaching_the_core_principal_socket_is_measured_three_ways(self):
        confined = self.run_arm("default", "default")
        # The same confinement with the ruleset off. It is not a second control
        # for politeness: at the shipped default a Landlock domain denies
        # `READ_DIR` on the realm root, so the filesystem WALK enumerates
        # nothing and "no socket found" would mean "I was not allowed to look".
        # This arm is the one that looks.
        mount_only = self.run_arm("default-landlock-off", "default", landlock="off")
        unconfined = self.run_arm("off", "off")
        arms = (confined, mount_only, unconfined)

        # (0) Every run produced a complete report, and the arms differ in
        #     nothing but the core's own flags. A byte-identical `realm.toml` is
        #     what makes "same app, same argv" a fact rather than an intention.
        for arm in arms:
            self.assertTrue(arm.report.complete, f"[{arm.name}] probe report incomplete")
        self.assertEqual(
            {arm.realm_toml for arm in arms},
            {confined.realm_toml},
            "the arms ran different realm configurations, so nothing below is a "
            "controlled comparison",
        )

        # (1) Control 2, every arm: the socket under measurement was serving.
        for arm in arms:
            self.assertEqual(
                arm.host_bound,
                DEMO_IDENTITY,
                f"[{arm.name}] the core socket did not bind the demo principal from the "
                "host, so a realm failing to reach it proves nothing",
            )

        # (2) Control 1, every arm: the probe's socket layer works *inside* the
        #     realm. The shim's own Wayland socket is a socket the app is given.
        for arm in arms:
            connects = arm.report.connects()
            for label in CONTROL_LABELS:
                self.assertIn(label, connects, f"[{arm.name}] no {label} record")
                self.assertEqual(
                    connects[label][0],
                    1,
                    f"[{arm.name}] the in-realm positive control {label} did not connect "
                    f"(errno {connects[label][1]}); every negative below would be satisfied "
                    "by a probe that can open no socket at all",
                )

        # (3) The unconfined arm reaches the core socket AND completes a real
        #     handshake on it -- and is refused with a wrong credential. This is
        #     the positive control for the whole enumeration: it proves the
        #     candidate paths, the connect and the handshake all work when
        #     nothing is in the way, and it is the only arm in which the answer
        #     to #311's question is "yes, and it binds".
        off_connects = unconfined.report.connects()
        reached = [
            label
            for label, (ok, _) in off_connects.items()
            if ok == 1 and label not in CONTROL_LABELS
        ]
        self.assertTrue(
            reached,
            "at --isolation=off no candidate path reached the core socket. That is not a "
            "confinement result, it is a broken probe: the realm shares the host's mount "
            f"and network namespaces there. Records: {off_connects}",
        )
        off_shakes = unconfined.report.handshakes()
        bound = [
            label for (label, cred), (result, _) in off_shakes.items()
            if cred == "good" and result == "bound"
        ]
        self.assertTrue(
            bound, f"at --isolation=off no reachable socket completed a hello: {off_shakes}"
        )
        for label in bound:
            self.assertEqual(
                off_shakes[(label, "good")][1],
                DEMO_IDENTITY,
                f"[off] {label} bound something other than the identity that was presented",
            )
            self.assertEqual(
                off_shakes[(label, "bad")][0],
                "refused",
                f"[off] {label} bound with a credential the registry does not hold, so "
                "'bound' is not evidence of authentication",
            )

        # (4) The confined arms. Every route separately, because one aggregate
        #     assertion would hide which route failed -- and both arms, because
        #     the mount table and the ruleset are different barriers and #311's
        #     answer may be built on either.
        for arm in (confined, mount_only):
            for label, (ok, err) in sorted(arm.report.connects().items()):
                if label in CONTROL_LABELS:
                    continue
                self.assertEqual(
                    ok, 0,
                    f"[{arm.name}] a confined realm CONNECTED to {label} (errno {err}). That "
                    "is the finding #311 exists to establish; record it rather than deleting "
                    "this test.",
                )
            self.assertEqual(
                [row for row in arm.report.of("P311-ABSTRACT") if row["connect"] == "1"],
                [],
                f"[{arm.name}] an abstract-namespace name was reachable from inside the realm",
            )
            self.assertEqual(
                arm.report.one("P311-PID-REACHED")["n"], "0",
                f"[{arm.name}] a visible process's /proc/<pid>/root was a door to the core "
                "socket",
            )
            self.assertEqual(
                [row for row in arm.report.of("P311-FD") if row["issock"] == "1"], [],
                f"[{arm.name}] a socket descriptor survived the execve into the realm",
            )
            self.assertEqual(
                [row for row in arm.report.of("P311-PEER-FD") if row["reopen"] == "1"], [],
                f"[{arm.name}] a socket in another process's descriptor table re-opened "
                "through procfs",
            )
            for name, value in sorted(arm.report.env().items()):
                self.assertNotIn(
                    "core.sock", value,
                    f"[{arm.name}] the composed environment leaks the core socket path in "
                    f"{name}",
                )

        # (5) The walk, and the reason there are two confined arms. `mount_only`
        #     must have ENUMERATED the realm root -- otherwise its silence about
        #     sockets is the silence of a directory it could not open.
        walked = mount_only.report.one("P311-WALK-DONE")
        self.assertGreater(
            int(walked["seen"]), 0,
            "[default-landlock-off] the walk enumerated nothing, so it found nothing for a "
            "reason that has nothing to do with confinement",
        )
        self.assertEqual(
            walked["truncated"], "0",
            "[default-landlock-off] the walk hit its budget cap, so 'no stray socket' is a "
            "statement about the part it reached, not about the realm's whole filesystem",
        )
        roots = {_un(row["path"]) for row in mount_only.report.of("P311-WALK-DIR")}
        self.assertIn(
            "/", roots,
            "[default-landlock-off] the realm root was not enumerable even with the ruleset "
            f"off; enumerated roots were {sorted(roots)}",
        )
        shim_socket = mount_only.report.env().get("WAYLAND_DISPLAY")
        strays = [
            _un(row["path"])
            for row in mount_only.report.of("P311-WALK-SOCKET")
            if row["connect"] == "1" and _un(row["path"]) != shim_socket
        ]
        self.assertEqual(
            strays, [],
            "[default-landlock-off] walking the realm's whole filesystem found a connectable "
            "socket that is not the shim's own",
        )
        # And the ruleset's own contribution, stated rather than assumed: at the
        # shipped default the same walk is refused at the root.
        self.assertTrue(
            [
                row for row in confined.report.of("P311-WALK-DENIED")
                if _un(row["path"]) == "/" and row["errno"] == str(errno.EACCES)
            ],
            "[default] the realm root was enumerable under the shipped ruleset, so the "
            "landlock-off arm is measuring the same thing twice rather than a second barrier",
        )

        # (6) The namespaces, stated rather than assumed. P2.7.1 is not done, so
        #     whether a realm is in its own network namespace -- which is what
        #     scopes the abstract Unix socket namespace, and nothing else does --
        #     is a measurement here, not a citation.
        for arm in (confined, mount_only):
            self.assertNotEqual(
                arm.report.ns().get("net"), arm.core_ns_net,
                f"[{arm.name}] the realm shares the core's network namespace, so the abstract "
                "Unix namespace is shared with it",
            )
        self.assertEqual(
            unconfined.report.ns().get("net"), unconfined.core_ns_net,
            "[off] the unconfined realm is NOT in the core's network namespace, so the "
            "abstract-namespace half of this measurement has no positive control",
        )
        self.assertGreater(
            int(unconfined.report.one("P311-ABSTRACT-COUNT")["n"]), 0,
            "[off] the unconfined realm saw no abstract names at all, so the confined arms' "
            "zero is not evidence of a namespace boundary",
        )
