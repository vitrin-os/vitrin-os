"""Boot a real `vitrind` and talk to it the way an agent does.

Everything in this suite goes through :class:`Core`, which starts the
**shipped binary** in its own throwaway `XDG_RUNTIME_DIR` and hands back the
socket, the pid and the flight-recorder log. No test constructs a runtime
in-process; that is what `crates/vitrin-core/src/session.rs`'s own tests do,
and the gap between the two is exactly what this suite exists to cover
(see ``run.sh``).
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

REPO = pathlib.Path(os.environ.get("VITRIN_REPO", pathlib.Path(__file__).resolve().parents[2]))
VITRIND = REPO / "target" / "debug" / "vitrind"
MOCK_SHIM = REPO / "target" / "debug" / "vitrin-mock-shim"

#: The demo identity, matching `examples/principals.toml`. Auto-approve is
#: only permitted when the registry holds nothing but this principal (R6),
#: so the registries written here deliberately hold exactly one row.
DEMO_IDENTITY = "vitrin://local/agent/demo"

#: 64 hex chars. The core refuses tokens under 16 bytes, and a registry that
#: tripped that refusal would fail every test here for the wrong reason.
DEMO_TOKEN = "a" * 64

#: How long a core gets to bind its socket before a test gives up. Generous
#: because CI runners are slow and a flaky integration suite is worse than a
#: slow one; the wait polls, so the cost is only paid when something is
#: actually wrong.
BOOT_TIMEOUT_S = 30.0

#: Frames the mock shim animates for, per core. **This is a CPU budget, not
#: a duration.** Headless has no output clock, so a paced shim composes as
#: fast as the runtime loop will dispatch it — the count is the only thing
#: bounding how long it spins. It needs to outlive the one test that asserts
#: two captures differ (~1 s of wall clock) and no longer; an earlier draft
#: used 100_000 and pinned a core for the whole suite.
ANIMATE_FRAMES = 1200

#: Hard per-test ceiling. The failure this exists for is issue #77's trap T1:
#: a regression that registers the shim socketpair's source after the fork
#: leaves the shim blocked on `configure` forever, and `observe()` then never
#: returns. Without this the suite would hang until CI's 10-minute cap and
#: report a timeout rather than a named failing test — the worst possible
#: reporting for the exact bug this suite exists to catch.
TEST_TIMEOUT_S = 90


class CoreFailed(Exception):
    """The core exited when a test expected it to be serving."""


class Core:
    """One `vitrind --headless` process, its runtime tree, and its log.

    Use as a context manager::

        with Core() as core:
            conn = core.connect()

    The runtime tree is a fresh temp directory per instance, which is what
    lets the two-cores test point a second core at the *same* tree
    deliberately rather than by accident.
    """

    def __init__(
        self,
        *,
        consent: str = "auto-approve",
        size: str = "320x200",
        animate: int = ANIMATE_FRAMES,
        seat: bool = False,
        runtime_dir: str | os.PathLike[str] | None = None,
        wait: bool = True,
        write_config: bool = True,
    ) -> None:
        self.runtime = pathlib.Path(runtime_dir or tempfile.mkdtemp(prefix="vitrin-it-"))
        self._owns_runtime = runtime_dir is None
        self.recorder = self.runtime / "flight.jsonl"
        self.principals = self.runtime / "principals.toml"
        self.realm = self.runtime / "realm.toml"
        self.proc: subprocess.Popen[str] | None = None
        self._output = ""
        self._entries: list[dict] | None = None

        # `write_config=False` reuses whatever config is already in the tree.
        # The R6 test needs it: it deliberately relaxes the registry's mode
        # and then starts a core against it, which a rewrite-and-chmod on
        # every construction would silently undo — leaving a test that
        # asserts a refusal against a file that no longer deserves one.
        if write_config:
            self.principals.write_text(
                f'[[principal]]\nidentity = "{DEMO_IDENTITY}"\ntoken = "{DEMO_TOKEN}"\n'
            )
            # 0600 or the core refuses to read it at all: the registry holds
            # bearer tokens, and the R6 audit fails closed on any wider mode.
            self.principals.chmod(0o600)
            # `--seat` (opt-in) mints the shim's input-delivery object so
            # routed seat events actually land rather than dropping
            # undelivered — what the #43 demo needs to exercise the seat path.
            # Default off, so every existing caller's argv is unchanged.
            seat_arg = ', "--seat"' if seat else ""
            self.realm.write_text(
                "[[realm]]\n"
                'id = "realm-0"\n'
                f'command = "{MOCK_SHIM}"\n'
                f'args = ["--serve"{seat_arg}, "--animate", "{animate}"]\n'
            )

        argv = [
            str(VITRIND),
            "--headless",
            "--size",
            size,
            f"--consent={consent}",
            "--principals",
            str(self.principals),
            "--realm",
            str(self.realm),
            "--recorder",
            str(self.recorder),
        ]
        self.proc = subprocess.Popen(
            argv,
            env={**os.environ, "XDG_RUNTIME_DIR": str(self.runtime), "RUST_LOG": "info"},
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if wait:
            self.await_socket()

    # -- lifecycle ---------------------------------------------------------

    @property
    def socket(self) -> pathlib.Path:
        return self.runtime / "vitrin-0" / "core.sock"

    @property
    def pid(self) -> int:
        assert self.proc is not None
        return self.proc.pid

    def await_socket(self, timeout: float = BOOT_TIMEOUT_S) -> None:
        """Block until the core is serving, or explain why it never will be.

        Polls for the socket rather than sleeping a fixed interval: the boot
        path does an R6 registry audit, a realm load, a `flock`, a recorder
        create and a fork, and any of them can be slow on a loaded runner.
        A dead core short-circuits with its own stderr, because "connection
        refused" three lines later tells you nothing about a refused
        registry mode.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc is not None and self.proc.poll() is not None:
                raise CoreFailed(
                    f"core exited {self.proc.returncode} during boot:\n{self.output()}"
                )
            if self.socket.exists():
                return
            time.sleep(0.05)
        raise CoreFailed(f"core never bound {self.socket} within {timeout}s:\n{self.output()}")

    def await_exit(self, timeout: float = 30.0) -> int:
        assert self.proc is not None
        return self.proc.wait(timeout=timeout)

    def terminate(self) -> int:
        """Stop the core with SIGTERM — the ordinary way a session ends.

        Deliberately not SIGKILL: `SIGTERM` is the path that runs the
        shutdown ladder, and the teardown test below asserts on what that
        ladder writes to the log.
        """
        assert self.proc is not None
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=10)
        return self.proc.returncode

    def output(self) -> str:
        """Everything the core wrote, cached so it can be read more than once."""
        if not self._output and self.proc is not None and self.proc.stdout is not None:
            if self.proc.poll() is not None:
                self._output = self.proc.stdout.read() or ""
        return self._output

    # -- observation -------------------------------------------------------

    def entries(self) -> list[dict]:
        """The flight recorder's entries, parsed.

        Call after the core has exited: `run_ended` is written at shutdown,
        and a test reading mid-run sees a footer-less file.

        Cached on first read after exit, because the log lives *inside* the
        runtime tree this object deletes on cleanup — and the natural way to
        write these tests is to assert on the log after the `with` block, by
        which point the file is gone. Without the cache every such assertion
        would silently see an empty list and pass or fail for the wrong
        reason.
        """
        if self._entries is None:
            if not self.recorder.exists():
                return []
            parsed = [
                json.loads(line)
                for line in self.recorder.read_text().splitlines()
                if line.strip()
            ]
            # Only freeze once the run is over; a mid-run read is a
            # legitimate snapshot but must not become the cached answer.
            if self.proc is not None and self.proc.poll() is not None:
                self._entries = parsed
            return parsed
        return self._entries

    def kinds(self) -> list[str]:
        return [entry["kind"] for entry in self.entries()]

    def connect(self):
        """An SDK connection, bound as the demo principal."""
        import vitrin_os

        return vitrin_os.connect(
            str(self.socket), identity=DEMO_IDENTITY, credential=DEMO_TOKEN
        )

    # -- context manager ---------------------------------------------------

    def __enter__(self) -> "Core":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.terminate()
        # Read the log before the tree it lives in goes away, so assertions
        # after the `with` block see the run rather than an empty list.
        self.entries()
        self.output()
        if self._owns_runtime:
            shutil.rmtree(self.runtime, ignore_errors=True)


class IntegrationTest(unittest.TestCase):
    """Base class: every test gets a hard deadline and leaves nothing running.

    Two failure modes this suite must not have, both learned rather than
    anticipated:

    - **Hangs.** A wedged shim makes `observe()` block forever. `SIGALRM`
      converts that into a named test failure in
      :data:`TEST_TIMEOUT_S` seconds instead of a nameless CI timeout ten
      minutes later.
    - **Leaks.** A test that fails between spawning a core and cleaning it
      up used to orphan a `vitrind` and its shim, which then kept composing
      — the next test would run alongside them. Registered cores are reaped
      in `tearDown` whatever the outcome.
    """

    def setUp(self) -> None:
        self._cores: list[Core] = []
        signal.signal(signal.SIGALRM, self._timed_out)
        signal.alarm(TEST_TIMEOUT_S)

    def tearDown(self) -> None:
        signal.alarm(0)
        for core in reversed(self._cores):
            try:
                core.__exit__()
            except Exception:  # cleanup must never mask the real failure
                pass

    def _timed_out(self, _signum: int, _frame: object) -> None:
        raise AssertionError(
            f"test exceeded {TEST_TIMEOUT_S}s. If this is `observe()` never returning, "
            "suspect issue #77's trap T1: the shim socketpair's event source registered "
            "after the fork leaves the shim blocked on `configure` forever."
        )

    def core(self, **kwargs: object) -> Core:
        """A core that is reaped when this test ends, pass or fail."""
        started = Core(**kwargs)  # type: ignore[arg-type]
        self._cores.append(started)
        return started


def children_of(pid: int) -> list[int]:
    """Direct children of `pid`, from procfs.

    `pstree` is not installed on every runner; `/proc/<pid>/task/*/children`
    is always there on Linux and is what `pstree` reads anyway.
    """
    kids: list[int] = []
    for task in pathlib.Path(f"/proc/{pid}/task").glob("*"):
        try:
            kids.extend(int(p) for p in (task / "children").read_text().split())
        except OSError:
            continue
    return kids


def comm_of(pid: int) -> str:
    """A pid's executable name, or '' if it is gone."""
    try:
        return pathlib.Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return ""


def require_binaries() -> None:
    missing = [str(p) for p in (VITRIND, MOCK_SHIM) if not p.is_file() or not os.access(p, os.X_OK)]
    if missing:
        sys.exit(
            "integration suite needs built binaries; missing: "
            + ", ".join(missing)
            + "\nrun `cargo build --workspace` (run.sh does this for you)"
        )


# -- shared agent-side helpers (used by more than one test module) ----------

ALL_VERBS = ("observe", "actuate.pointer", "actuate.text")


def whole_realm_grant(conn, verbs=ALL_VERBS):
    """Petition for the MVP's one grant shape and wait it out.

    `resource` is left empty deliberately: version 0 serves whole-realm grants
    only and refuses any finer granularity as `Unsupported` — an honest refusal
    rather than accepted-and-unenforced.
    """
    import vitrin_os

    return conn.request_grant(
        verbs=verbs, persistence=vitrin_os.Persistence.WHILE_RUNNING
    ).await_consent()


def capture_when_ready(grant, timeout=5.0, poll=0.02):
    """The first capture of a freshly-served realm, tolerating the startup race.

    A realm that has not yet committed and composited its first buffer has no
    surface, and the core answers `observe` with `NoSurface` — the honest reply
    the protocol prescribes, not a fault the agent could have prevented. So an
    agent's *first* `observe()` can lose a race with the realm's first frame:
    `await_consent()` returns the instant the grant resolves, which on a loaded
    machine is before the shim has drawn (seen on CI as a `NoSurface`). The poll
    model (D6, one fresh frame per call) is to retry, so this loops `observe()`
    until a frame lands or the deadline passes.

    A `NoSurface` refusal is judged *before* the token bucket (enforcement.rs),
    so a retry costs no rate budget; only the one admitted capture that returns
    consumes a token. That is why this doubles as the surface-ready barrier the
    actuation tests warm the realm with before a rate-sensitive burst.
    """
    from vitrin_os import errors

    deadline = time.monotonic() + timeout
    while True:
        try:
            return grant.observe()
        except errors.NoSurface:
            if time.monotonic() >= deadline:
                raise
            time.sleep(poll)
