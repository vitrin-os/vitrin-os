# SPDX-License-Identifier: Apache-2.0
"""The **seccomp deny-list gate** (P2.6.4, issue #188).

Mock-free and real-app, on the same terms as `test_real_confinement.py`: the
shipped `vitrind`, the shipped `vitrin-realm-init`, the built C `vitrin-shim`
and a real Wayland client (`solid-client`) that connects over a real socket.
`vitrin-mock-shim` appears nowhere. It is deliberately **not** a milestone
gate -- no plan document schedules a milestone here -- and it closes a gap the
project *publishes*.

## What this gate is, in one sentence

It runs one probe binary twice with byte-identical argv, inside a realm and at
`--isolation=off`, and for **every row of the shipped filter table** asserts
the exact errno inside the realm and reads the outside run as that row's
positive control.

## The three properties, and why each one is load-bearing

**1. Exhaustive over the table, not a hand-written list.** The row set comes
from `vitrind --print-seccomp`, which renders
`vitrin_realm_init::seccomp::ROWS` directly. So a row added to the filter
without a probe case turns this gate RED rather than passing unmeasured, which
is #188's acceptance criterion in as many words. There is no second copy of
the table in this file -- only the *expectations* the table cannot state about
itself.

**2. The exact errno per row, never "not zero".** Errno is part of the
filter's contract: a library that probes a syscall and falls back on `ENOSYS`
will abort on `EPERM`, so a filter change that "still denies" can silently
break an app. And three of the candidate rows already answer a *non-EPERM*
error inside a realm before any filter exists -- `request_key` answers
`ENOKEY`, `migrate_pages` answers `EINVAL`, `uselib` answers `ENOSYS` from the
kernel itself -- so a test asserting "not zero" would pass on those with no
filter installed at all. That is not a hypothetical: it is why two of those
three are not rows.

What this gate does **not** do is hold the errno *itself*. The expected value
comes from `--print-seccomp`, so changing a row's `errno` moves both sides of
the comparison at once and this stays green. That is deliberate -- the row set
is read from the binary precisely so nothing here is transcribed -- and it is
why every row's errno is also pinned as a **literal** in
`crates/vitrin-realm-init/src/seccomp.rs`'s `tests::PINNED`, the only copy in
the repository not derived from the table. The two checks answer different
questions: that one asks whether the contract changed, this one asks whether a
*realm* really sees it.

**3. A positive control per row, in the same run, and a row that fails it is
reported NOT DEMONSTRATED rather than counted.** `docs/plan/02-phase-2-semantic-epochs.md`
§4 is explicit that M2.5 is almost entirely absences and that every negative
claim needs one. A realm already cannot `mount`, cannot create a nested user
namespace, and cannot open an `AF_PACKET` socket -- none of which this filter
did. So for each row the same probe binary runs OUTSIDE the realm, and:

* the syscall **succeeds** outside → the row is *demonstrated confinement* on
  this kernel;
* the syscall **fails** outside too → the row is *not demonstrated here*, and
  this gate says so by name instead of adding it to a count.

Two rows are expected to land in the second bucket on a hardened box and are
not failures: `bpf` and `userfaultfd` are shadowed by
`kernel.unprivileged_bpf_disabled` and `vm.unprivileged_userfaultfd`, whose
values differ per machine. Their demonstrability is a property of the kernel,
which is why it is *measured here* rather than declared in the table.

## What is NOT the demonstration, stated because it is the tempting one

`socket(AF_PACKET)` already fails `EPERM` inside a realm without any filter --
it needs `CAP_NET_RAW`, dropped at K10. The socket-family row is demonstrated
against `AF_VSOCK` and `AF_ALG`, both measured reachable inside a realm before
this task, and `AF_PACKET` is probed only so this gate can *say* it is not a
demonstration rather than leave it unmeasured.

## The negative controls, which are the half that can break an application

A seccomp filter is inherited and unremovable, so anything this table denies is
denied to Firefox's own sandbox too. The syscalls
`vitrin_realm_init::seccomp::NEVER_DENIED` names must **succeed inside the
realm**, and this gate reads that list from the same `--print-seccomp` output.
`seccomp(SECCOMP_SET_MODE_FILTER)` is the one that matters most: Firefox and
Chromium install their own filters for content processes, and a deny there
would present as an unrelated startup failure.

## Independence, stated rather than left to be found (R2.7)

**The probe is repo-authored.** `shim/tests/solid_client.c`'s `--syscall-probe`
is written by the same repository that writes the filter it measures, so a
filter and a probe could agree with each other and both be wrong about the
world. Its own header says so. What makes this gate worth more than a
self-consistent pair is the positive control: the outside run is the same
binary against an *unfiltered* kernel, so a probe that got a syscall wrong
fails there too and the row is reported not demonstrated rather than counted.
"""

import os
import pathlib
import subprocess
import time

from harness import VITRIND, IntegrationTest, require_binaries

require_binaries()

APP_NAME = "solid-client"

#: Where the app writes its report. Relative, so it resolves against
#: `$XDG_RUNTIME_DIR` **inside** the app -- `/run/vitrin` in a confined realm
#: and the host path at `--isolation=off`, landing in the same host file either
#: way. That is what lets one argv, byte for byte, serve both runs.
PROBE_OUT = "seccomp-probe.txt"

REALM_SIZE = "320x200"
RUN_MS = "25000"
COLOUR = "0000ff"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: Probe keys this gate understands that are **not** rows of the filter table.
#:
#: `socket_alg` is the socket-family row's second demonstration (the kernel
#: crypto API, measured reachable in a realm before this task). `socket_packet`
#: is the one that must NOT be read as a demonstration -- it is already `EPERM`
#: without `CAP_NET_RAW`.
EXTRA_SOCKET_PROBES = {"socket_alg", "socket_packet"}

#: Errnos, spelled here because the printed table spells them by name and this
#: side has to compare numbers. Held to the table by
#: `test_the_gate_knows_every_errno_the_table_can_return`, so a row using a
#: fourth errno fails loudly instead of being skipped.
ERRNO = {"EPERM": 1, "ENOSYS": 38, "EAFNOSUPPORT": 97}

#: **The run behind the published "N of the 13 denied syscall rows are
#: demonstrated" figure**, and the canonical source `cargo xtask limits-check`
#: reads it from.
#:
#: Measured by this gate on Arch `7.1.8-arch1-3`, 2026-08-16. It is a property
#: of that kernel and not of the filter — which rows have a working positive
#: control depends on the box's sysctls — so it is recorded here, in the file
#: that produced it, rather than declared in the filter table. Three surfaces
#: state it (`README.md`, `SECURITY.md`, `docs/book/src/limits.md`) and nothing
#: held them to anything before this constant existed, so adding a fourteenth
#: row would have left all three saying eleven with no build going red.
PUBLISHED_DEMONSTRATED = 11

#: The rows that failed their positive control on that same run, named because
#: two of the three surfaces name them. Both are sysctl-shadowed there
#: (`kernel.unprivileged_bpf_disabled`, `vm.unprivileged_userfaultfd`).
#:
#: `test_the_published_demonstration_figure_still_describes_this_table` holds
#: the arithmetic: these two plus `PUBLISHED_DEMONSTRATED` must be the whole
#: table, so a row added or removed makes the published figure red instead of
#: quietly stale.
PUBLISHED_NOT_DEMONSTRATED = ("bpf", "userfaultfd")


def _resolve_app(shim_bin: pathlib.Path) -> str | None:
    explicit = os.environ.get("VITRIN_SOLID_APP")
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / APP_NAME
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


class _Table:
    """The shipped filter table, read from `vitrind --print-seccomp`.

    Read from the binary rather than transcribed, so this file holds no second
    copy of the row set. A row added to
    `crates/vitrin-realm-init/src/seccomp.rs` appears here on the next build
    and fails the exhaustiveness assertion until the probe learns it.
    """

    def __init__(self, text: str) -> None:
        self.raw = text
        self.rows: dict[str, dict[str, str]] = {}
        self.never_denied: list[str] = []
        self.instructions: int | None = None
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            if fields[0] == "row":
                kv = _kv(fields[1:])
                self.rows[kv["name"]] = kv
            elif fields[0] == "never-denied":
                self.never_denied.append(_kv(fields[1:])["name"])
            elif line.startswith("table.instructions="):
                value = line.split("=", 1)[1]
                self.instructions = int(value) if value.isdigit() else None


def _kv(fields: list[str]) -> dict[str, str]:
    """Split `k=v` fields, keeping later spaces inside the last value.

    `class=Reachable-service lateral escape` carries spaces, so a naive
    per-field split would lose everything after the first word. Keys are
    known-shaped (`name`, `nr`, `errno`, `class`, `predicate`, `shadow`), so a
    field without `=` continues the previous value.
    """
    out: dict[str, str] = {}
    key = None
    for field in fields:
        if "=" in field:
            key, value = field.split("=", 1)
            out[key] = value
        elif key is not None:
            out[key] += " " + field
    return out


class _Report:
    """One parsed `--probe-out` report's `PROBE-SYSCALL` lines."""

    def __init__(self, text: str) -> None:
        self.raw = text
        self.rows: dict[str, tuple[int, int]] = {}
        self.complete = False
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            if fields[0] == "PROBE-END":
                self.complete = True
            elif fields[0] == "PROBE-SYSCALL":
                kv = _kv(fields[1:])
                self.rows[kv["name"]] = (int(kv["rc"]), int(kv["errno"]))

    def rc(self, name: str) -> int:
        return self.rows[name][0]

    def err(self, name: str) -> int:
        return self.rows[name][1]

    def succeeded(self, name: str) -> bool:
        return self.rows[name][0] >= 0


class PublishedFigures(IntegrationTest):
    """The published numbers about this table, held to the table itself.

    Deliberately **not** a `_RealChain` subclass: it needs the shipped
    `vitrind` and nothing else — no shim, no realm, no app — so it runs even
    where the real-app ladder is opted out of. A figure that goes unchecked
    exactly when the suite is trimmed is a figure nothing checks.
    """

    def table(self) -> "_Table":
        return _Table(
            subprocess.run(
                [str(VITRIND), "--print-seccomp"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        )

    def test_the_published_demonstration_figure_still_describes_this_table(self):
        """`11 of the 13` is arithmetic about a table that can grow.

        `cargo xtask limits-check` holds three published surfaces to
        `PUBLISHED_DEMONSTRATED`, and the in-crate
        `the_published_row_count_is_the_tables_own` holds six surfaces to
        `ROWS.len()`. Neither notices that the two numbers stopped adding up:
        add a fourteenth row and every surface would say `14 denied syscall
        rows` in one sentence and `11 of the 13` in the next, with every check
        green. This is the join.
        """
        rows = self.table().rows
        self.assertTrue(rows, "`vitrind --print-seccomp` printed no rows")
        for name in PUBLISHED_NOT_DEMONSTRATED:
            self.assertIn(
                name,
                rows,
                f"the published figure names `{name}` as a row that is not demonstrated, and "
                "the shipped table has no such row. The sentence on README.md, SECURITY.md and "
                "docs/book/src/limits.md is about a table that no longer exists",
            )
            self.assertEqual(
                rows[name]["shadow"],
                "yes",
                f"`{name}` is published as not-demonstrated-because-shadowed and its row now "
                "declares no shadowing mechanism",
            )
        self.assertEqual(
            PUBLISHED_DEMONSTRATED + len(PUBLISHED_NOT_DEMONSTRATED),
            len(rows),
            f"the published figure says {PUBLISHED_DEMONSTRATED} of the "
            f"{PUBLISHED_DEMONSTRATED + len(PUBLISHED_NOT_DEMONSTRATED)} rows are demonstrated "
            f"and the shipped table has {len(rows)} rows. The measurement has to be RE-TAKEN "
            f"(run this file's gate and read its printed verdict), not re-arithmetic'd: which "
            f"rows have a working positive control is a property of the kernel it was taken on. "
            f"Then update PUBLISHED_DEMONSTRATED here -- `cargo xtask limits-check` carries it "
            f"to README.md, SECURITY.md and docs/book/src/limits.md",
        )


class _RealChain(IntegrationTest):
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
        self.table = _Table(
            subprocess.run(
                [str(VITRIND), "--print-seccomp"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        )
        self.assertTrue(
            self.table.rows,
            "`vitrind --print-seccomp` printed no rows. Either the table is empty -- in which "
            "case this build installs a filter that denies nothing and every published "
            f"sentence about it is false -- or the verb changed shape:\n{self.table.raw}",
        )

    def probe_argv(self) -> list[str]:
        """The app's argv. **Identical in both isolation modes, deliberately.**"""
        return [
            "--run-ms",
            RUN_MS,
            "--colour",
            COLOUR,
            "--syscall-probe",
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

    def await_report(self, core, timeout: float = 30.0) -> _Report:
        path = core.runtime / "vitrin-0" / "realm-0" / PROBE_OUT
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
            report = _Report(last)
            if report.complete:
                return report
            time.sleep(0.05)
        self.fail(
            f"the real app never wrote a complete probe report at {path} within {timeout}s. "
            f"What was there: {last!r}. The realm's log: "
            f"{(core.runtime / 'vitrin-0' / 'realm-0' / 'realm.log').read_text()[-2000:]!r}"
        )


class RealSeccompTable(_RealChain):
    """The table-driven gate: every row, its errno, and its positive control."""

    def test_every_table_row_is_denied_in_realm_with_its_own_errno(self):
        confined_core = self.real_core("default")
        confined = self.await_report(confined_core)
        confined_argv = confined_core.realm.read_text()
        confined_core.terminate()

        unconfined_core = self.real_core("off")
        unconfined = self.await_report(unconfined_core)
        unconfined_argv = unconfined_core.realm.read_text()
        unconfined_core.terminate()

        # The two runs differ in the flag and in nothing else. Read from the
        # files the harness actually wrote, so a future convenience that
        # quietly varied one run's argv turns this red instead of turning the
        # comparison into one between two different experiments.
        self.assertEqual(
            confined_argv,
            unconfined_argv,
            "the confined and unconfined runs must differ ONLY in --isolation",
        )

        # (1) EXHAUSTIVENESS. Every row of the shipped table has a probe case.
        #     This is what makes "a row added to the filter without a case
        #     fails the test" true -- the row set is read from the binary, so
        #     there is nothing here to forget to update.
        missing = sorted(set(self.table.rows) - set(confined.rows))
        self.assertEqual(
            missing,
            [],
            f"the filter table has rows this probe never attempted: {missing}. Add a case to "
            f"`SYSCALL_PROBES` in shim/tests/solid_client.c. A denied syscall nobody probed is "
            f"a row whose errno contract and whose positive control are both unmeasured.\n"
            f"Probe report:\n{confined.raw}",
        )
        # And nothing the probe reports is a stranger to the table, except the
        # two socket demonstrations and the standing negative controls -- so a
        # probe key that is neither a row nor a control cannot sit there
        # unexamined.
        known = set(self.table.rows) | set(self.table.never_denied) | EXTRA_SOCKET_PROBES
        strangers = sorted(set(confined.rows) - known)
        self.assertEqual(
            strangers,
            [],
            f"the probe reports {strangers}, which is neither a filter row nor a declared "
            "negative control. Either it is a row the table lost, or this gate is reading a "
            "measurement nobody assigned a meaning to",
        )

        # (2) THE POSITIVE CONTROL, before any negative is read. The syscalls
        #     that fail outside the realm too are named here, once, and are
        #     reported NOT DEMONSTRATED below rather than counted.
        demonstrated: list[str] = []
        not_demonstrated: list[str] = []
        for name in sorted(self.table.rows):
            (demonstrated if unconfined.succeeded(name) else not_demonstrated).append(name)
        self.assertTrue(
            demonstrated,
            "NOT ONE row of this table denies something the same probe can do outside a "
            f"realm. Every negative below would then be a fact about this kernel rather than "
            f"about the filter. Control report:\n{unconfined.raw}",
        )

        # (3) THE ERRNO CONTRACT, per row, exact.
        for name, row in sorted(self.table.rows.items()):
            want = ERRNO.get(row["errno"])
            self.assertIsNotNone(
                want,
                f"row {name} returns {row['errno']}, which this gate cannot translate to a "
                "number. Add it to ERRNO rather than letting the row go unchecked",
            )
            self.assertEqual(
                confined.rc(name),
                -1,
                f"{name} SUCCEEDED inside the realm (rc={confined.rc(name)}). It is a row of "
                f"the shipped deny-list, so either the filter was not installed or the "
                f"assembler dropped it.\nReport:\n{confined.raw}",
            )
            self.assertEqual(
                confined.err(name),
                want,
                f"{name} returned errno {confined.err(name)} inside the realm; its row "
                f"promises {row['errno']} ({want}). Errno is part of this table's contract: a "
                f"library that probes and falls back on ENOSYS aborts on EPERM, so a filter "
                f"change that 'still denies' can break an app silently",
            )

        # (4) THE NEGATIVE CONTROLS, inside the realm. These are the half that
        #     breaks an application: a seccomp filter is inherited and
        #     unremovable, so anything denied here is denied to Firefox's own
        #     sandbox too.
        for name in self.table.never_denied:
            self.assertIn(
                name,
                confined.rows,
                f"{name} is a declared standing negative control and the probe never "
                "attempted it",
            )
            self.assertTrue(
                confined.succeeded(name),
                f"{name} FAILED inside the realm with errno {confined.err(name)}. It is on "
                f"`NEVER_DENIED`: the filter has regressed into denying something the "
                f"acceptance apps need. `seccomp` in particular is how Firefox and Chromium "
                f"install their own content-process filters.\nReport:\n{confined.raw}",
            )

        # (5) AF_PACKET is measured and explicitly NOT counted, so this gate
        #     cannot be read as demonstrating the socket row with it.
        self.assertFalse(
            unconfined.succeeded("socket_packet"),
            "socket(AF_PACKET) succeeded OUTSIDE the realm on this machine, which means this "
            "run had CAP_NET_RAW. That does not make it a demonstration of the socket-family "
            "row -- inside a realm it is denied by the capability drop at K10 before the "
            "filter is reached -- but the note below would be wrong, so re-read it",
        )
        self.assertFalse(
            unconfined.succeeded("socket_packet") or confined.succeeded("socket_packet"),
            "AF_PACKET must fail in both runs",
        )
        # The socket row's second demonstration, which IS one.
        self.assertTrue(
            unconfined.succeeded("socket_alg"),
            "socket(AF_ALG) failed outside the realm, so the socket-family row has only one "
            "demonstration left on this kernel",
        )
        self.assertEqual(
            confined.err("socket_alg"),
            ERRNO["EAFNOSUPPORT"],
            "AF_ALG must meet the same predicate AF_VSOCK does",
        )

        # (6) The verdict, printed rather than only asserted, because "not
        #     demonstrated" is a result this gate is required to REPORT and not
        #     merely to tolerate.
        print(
            f"\nseccomp rows demonstrated on this kernel ({len(demonstrated)}/"
            f"{len(self.table.rows)}): {', '.join(demonstrated)}"
        )
        # The published figure beside this run's, so a reader of the log can
        # see whether the sentence three surfaces carry still describes the
        # machine it was taken on. NOT an assertion: which rows have a working
        # positive control is a property of the kernel, and a box with
        # different sysctls is a different answer rather than a regression --
        # asserting here would make this gate red on a hardened machine for
        # being honest. What IS asserted is the arithmetic, in
        # `PublishedFigures` above, which is the half that can go stale.
        if len(demonstrated) != PUBLISHED_DEMONSTRATED:
            print(
                f"NOTE: the published figure is {PUBLISHED_DEMONSTRATED} of "
                f"{len(self.table.rows)}, measured on Arch 7.1.8-arch1-3 (2026-08-16). This box "
                f"answered {len(demonstrated)}. That is a difference between two KERNELS, not a "
                f"filter regression -- but if this box is the one the published sentence "
                f"describes, re-take the figure in tests/integration/test_real_seccomp.py"
            )
        if not_demonstrated:
            shadows = ", ".join(
                f"{name} (shadowed: {self.table.rows[name]['shadow']})"
                for name in not_demonstrated
            )
            print(
                f"seccomp rows NOT DEMONSTRATED on this kernel -- the same syscall already "
                f"fails outside a realm here, so the denial is real but is not confinement "
                f"THIS FILTER adds on THIS machine: {shadows}"
            )
        # A row that is not demonstrated must at least have declared that it
        # could be shadowed. A row with `shadow=no` failing its positive
        # control is a row whose justification was wrong, not a kernel quirk.
        for name in not_demonstrated:
            self.assertEqual(
                self.table.rows[name]["shadow"],
                "yes",
                f"{name} failed its positive control and its table row declares no shadowing "
                f"mechanism. Either the row's argument is wrong, or this machine denies it for "
                f"a reason nobody wrote down -- both are a re-read of the table, not a pass",
            )

    def test_the_journal_reports_the_filter_it_installed(self):
        """The `realm_spawned` entry's `seccomp` object, and what it is not.

        A **size**, never a policy: `rows` and `instructions` say a filter of a
        stated shape was installed and say nothing about what it denies. The
        test above is what measures the denials. This one exists because until
        #188 the same key was the string `not-applied (P2.6.4)` on every
        confined spawn, and a build that installs a filter while journaling
        that string would be publishing a false row in the safe direction --
        which this repository treats as a defect of the same class as
        overclaiming.
        """
        core = self.real_core("default")
        self.await_report(core)
        entry = None
        for candidate in core.entries():
            if candidate["kind"] == "realm_spawned" and candidate.get("realm") == "realm-0":
                entry = candidate
        core.terminate()
        self.assertIsNotNone(entry, "the flight recorder holds no realm_spawned for realm-0")
        seccomp = entry["isolation"]["seccomp"]
        self.assertIsInstance(
            seccomp,
            dict,
            "`isolation.seccomp` is not an object. It was the string `not-applied (P2.6.4)` "
            f"until #188 installed a filter; a build that still writes a string is a build "
            f"whose journal denies what it does: {seccomp!r}",
        )
        self.assertEqual(
            seccomp["rows"],
            len(self.table.rows),
            "the realm reported a different number of rows than `vitrind --print-seccomp` "
            "renders. The helper and the core are built from one table, so a mismatch means "
            "one of the two binaries is stale",
        )
        self.assertEqual(seccomp["instructions"], self.table.instructions)
        self.assertEqual(
            seccomp["evidence"],
            "child-asserted",
            "the counts come from PID 1 inside the realm. No /proc interface reports a "
            "process's seccomp RULES -- only its mode -- so the parent cannot corroborate "
            "them, and the journal must say so rather than let a reader take them for a "
            "parent reading",
        )

    def test_an_unconfined_spawn_journals_no_filter_it_did_not_install(self):
        """`--isolation=off` writes `null`, not a count.

        The control for the test above: without it, a journal that hard-coded
        a row count would satisfy every assertion there. The rule is this
        repository's standing one -- absent information is an explicit value,
        never a missing key, and never a hopeful default.
        """
        core = self.real_core("off")
        self.await_report(core)
        entry = None
        for candidate in core.entries():
            if candidate["kind"] == "realm_spawned" and candidate.get("realm") == "realm-0":
                entry = candidate
        core.terminate()
        self.assertIsNotNone(entry, "the flight recorder holds no realm_spawned for realm-0")
        self.assertIsNone(
            entry["isolation"]["seccomp"],
            "an unconfined session journaled a seccomp filter. No helper runs at "
            "`--isolation=off`, so there is nothing to have installed one",
        )
        self.assertIn(
            "seccomp",
            entry["isolation"],
            "the key must be present and null rather than omitted: absent information is an "
            "explicit value on this journal's own rule",
        )
