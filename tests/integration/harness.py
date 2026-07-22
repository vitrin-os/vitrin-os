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


def _toml_string(value: str) -> str:
    r"""A TOML basic string literal for `value`.

    The realm loader (`crates/vitrin-core/src/toml_subset.rs`) reads a small
    TOML subset; a program path or env name never contains a quote or
    backslash, but escaping them keeps this honest rather than assuming it.
    """
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _toml_string_array(values: list[str]) -> str:
    """A TOML inline array of basic strings (`[]` when empty)."""
    return "[" + ", ".join(_toml_string(v) for v in values) + "]"


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
        shim: str | os.PathLike[str] | None = None,
        command: str | os.PathLike[str] | None = None,
        args: list[str] | None = None,
        env_allow: tuple[str, ...] = (),
        extra_env: dict[str, str] | None = None,
        log_file: str | os.PathLike[str] | None = None,
    ) -> None:
        self.runtime = pathlib.Path(runtime_dir or tempfile.mkdtemp(prefix="vitrin-it-"))
        self._owns_runtime = runtime_dir is None
        self.recorder = self.runtime / "flight.jsonl"
        self.principals = self.runtime / "principals.toml"
        self.realm = self.runtime / "realm.toml"
        self.proc: subprocess.Popen[str] | None = None
        self._output = ""
        self._entries: list[dict] | None = None
        # A verbose realm (a probing shim under Firefox emits thousands of
        # DEBUG lines; the browser is chatty on its own) can write more to the
        # core's inherited stdout/stderr than a pipe holds -- and this harness
        # reads that pipe only *after* the process exits, so a run that fills
        # ~64 KiB before then would wedge the writer forever. `log_file`
        # redirects the child's combined output to a file instead of a pipe,
        # which cannot back-pressure; `output()` reads it back the same way.
        # Default (None) keeps the pipe every existing caller relies on.
        self._logf = open(log_file, "w+") if log_file is not None else None

        # The shim binary the core execs to hold fd 3 (issue #103). Default
        # is `vitrin-mock-shim`, which every existing test uses and which is
        # BOTH the fd-3 peer and the app stand-in it ignores. The real-app
        # gate (test_real_app.py) overrides it with the built C shim, whose
        # `command` names a genuine Wayland app the C shim fork/execs.
        shim_bin = pathlib.Path(shim) if shim is not None else MOCK_SHIM

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
            if command is None:
                # The mock path, unchanged: the mock shim is the realm's
                # `command` app stand-in (which it ignores) as well as the
                # `--shim` fd-3 peer. `--seat` (opt-in) mints the shim's
                # input-delivery object so routed seat events actually land
                # rather than dropping undelivered — what the #43 demo needs
                # to exercise the seat path. Default off, so every existing
                # caller's argv is unchanged.
                seat_arg = ', "--seat"' if seat else ""
                self.realm.write_text(
                    "[[realm]]\n"
                    'id = "realm-0"\n'
                    f'command = "{MOCK_SHIM}"\n'
                    f'args = ["--serve"{seat_arg}, "--animate", "{animate}"]\n'
                )
            else:
                # The real-app path: `command` names a genuine app the C shim
                # fork/execs after the `--` tail, and `env_allow` is the only
                # route the app's (and the shim's) environment is allowed to
                # grow by — the headless/software-render WLR_* names travel
                # this way, exactly as the c_shim conformance test threads
                # them (crates/vitrin-core/src/shim.rs).
                self.realm.write_text(
                    "[[realm]]\n"
                    'id = "realm-0"\n'
                    f"command = {_toml_string(os.fspath(command))}\n"
                    f"args = {_toml_string_array(args or [])}\n"
                    f"env_allow = {_toml_string_array(list(env_allow))}\n"
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
            # Since #103 the core execs a `--shim` binary (fd-3 holder) that
            # conveys the realm's `command` app in argv after `--`. The mock
            # shim is that binary by default; the real-app gate points it at
            # the built C shim, which in turn fork/execs the `command` app.
            "--shim",
            str(shim_bin),
        ]
        env = {**os.environ, "XDG_RUNTIME_DIR": str(self.runtime), "RUST_LOG": "info"}
        # The core's own environment is the source `env_allow` copies from,
        # so the real-app gate seeds WLR_* here for the allowlist to forward.
        if extra_env:
            env.update(extra_env)
        self.proc = subprocess.Popen(
            argv,
            env=env,
            stdout=self._logf if self._logf is not None else subprocess.PIPE,
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
        """Everything the core wrote, cached so it can be read more than once.

        Reads from the redirect file when one was given (`log_file`), else from
        the stdout pipe. Either way only after the process has exited, so the
        text is complete and the read cannot race the writer.
        """
        if not self._output and self.proc is not None and self.proc.poll() is not None:
            if self._logf is not None:
                self._logf.flush()
                self._logf.seek(0)
                self._output = self._logf.read() or ""
            elif self.proc.stdout is not None:
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
        if self._logf is not None:
            self._logf.close()
            self._logf = None
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


def descendant_named(pid: int, name: str, timeout: float = 15.0) -> int | None:
    """Wait for a descendant of `pid` whose `comm` matches `name`, DFS.

    The real spawn spine is core → C shim → app, so the app the test cares
    about is a *grand*child of the core, not a direct child. `comm` is
    truncated to 15 bytes by the kernel, so the match is a prefix test —
    `weston-terminal` arrives as `weston-terminal` (exactly 15) but a longer
    name would be clipped, and matching a prefix is what survives that.
    """
    deadline = time.monotonic() + timeout
    prefix = name[:15]
    while True:
        stack = list(children_of(pid))
        while stack:
            candidate = stack.pop()
            if comm_of(candidate).startswith(prefix):
                return candidate
            stack.extend(children_of(candidate))
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.05)


def environ_of(pid: int) -> dict[str, str]:
    """A pid's environment at exec time, parsed from `/proc/<pid>/environ`.

    NUL-separated `NAME=VALUE` records; a record without `=` (there is
    normally none) is skipped rather than guessed at.
    """
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
    except OSError:
        return {}
    env: dict[str, str] = {}
    for record in raw.split(b"\0"):
        if not record:
            continue
        name, sep, value = record.partition(b"=")
        if sep:
            env[name.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
    return env


def fd_targets_of(pid: int) -> dict[int, str]:
    """Every open descriptor of `pid` mapped to its `readlink` target.

    Used to prove the confined app holds no descriptor number 3 that the
    core's socketpair would occupy — the fd-3 leak is the whole confinement
    gone (`crates/vitrin-core/src/spawn.rs`).
    """
    targets: dict[int, str] = {}
    for entry in pathlib.Path(f"/proc/{pid}/fd").glob("*"):
        try:
            targets[int(entry.name)] = os.readlink(entry)
        except (OSError, ValueError):
            continue
    return targets


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


# -- captured-frame colour analysis (shared by every real-app gate) ---------
#
# The M1.2 render proof, in three layers used across the real-app ladder
# (test_real_app.py weston rung, test_real_gtk.py, test_real_firefox.py):
# `packed_pixels` turns a stride-generic wire frame into tight BGRX pixels,
# `has_real_content` is the weston/GTK "not the shim's empty fill" check, and
# `dominant_colour` is the Firefox "a known solid colour is on screen" check.


def _packed_pixels(frame) -> bytes:
    """`frame.raw` as tightly-packed 4-byte ``B,G,R,X`` pixels.

    Stride-generic per the IDL (row ``r`` begins at ``r * stride`` and carries
    ``width * 4`` payload bytes); a frame with ``stride > width * 4`` padding
    rows is repacked, the tight ``stride == width*4`` case (v1's pin) passes
    through. This is the one place the wire's row addressing is undone, so the
    two analyses below never re-derive it.
    """
    raw = frame.raw
    row_len = frame.width * 4
    if frame.stride == row_len:
        return raw
    return b"".join(
        raw[r * frame.stride : r * frame.stride + row_len] for r in range(frame.height)
    )


def colour_bytes(frame) -> bytes:
    """The colour channels of an xrgb8888 frame, padding byte stripped.

    Each 4-byte pixel is ``B, G, R, X`` (little-endian xrgb8888) with the
    ``X`` padding byte last. Dropping the padding plane is load-bearing: the C
    shim composites an **opaque** background whose padding plane is a constant
    ``0xFF`` even with no client buffer committed, so a check over *all* bytes
    reads ``{0x00, 0xFF}`` as "content" on an empty frame. The three colour
    planes concatenated carry only what a client actually painted.
    """
    packed = _packed_pixels(frame)
    return packed[0::4] + packed[1::4] + packed[2::4]


def has_real_content(frame) -> bool:
    """True once a frame carries real, non-uniform content, not an empty fill.

    Content-bearing iff the colour channels are both non-zero (some pixel is
    not black) and non-uniform (more than one colour value). The shim's opaque
    background and a toolkit's pre-chrome fill are each a single value and both
    fail this -- which is what makes "a real app frame reached the agent" a
    genuine claim rather than a pass on the shim's empty output.
    """
    colour = colour_bytes(frame)
    return bool(any(colour)) and len(set(colour)) > 1


#: Keep only a byte's top nibble -- a 4-bit-per-channel quantisation. It
#: matches mock_core.c's dominant-colour histogram, which is why the Firefox
#: acceptance pages use channel values that are multiples of 0x11 (they
#: survive it exactly, ``0xff -> 0xf0`` and back to ``0xff``).
_TOP_NIBBLE = bytes(i & 0xF0 for i in range(256))


def dominant_colour(frame) -> tuple[str, int]:
    """The most common colour of a captured frame as ``("rrggbb", percent)``.

    A 4-bit-per-channel histogram (mock_core.c's quantisation, so a page whose
    CSS colour has channels in multiples of 0x11 reads back as that literal
    colour with no tolerance): each pixel's ``R,G,B`` is reduced to its top
    nibble, the modal quantised colour wins, and its nibbles are expanded
    ``n -> n * 0x11`` into a ``rrggbb`` string. ``percent`` is that colour's
    share of the frame, floored -- the same "dominant over enough of the view
    to mean it" bar firefox_bringup.sh applies. This is the real-core render
    proof for Firefox: a local solid-colour page's colour dominating a real
    captured frame, distinct from the shim's black background and Firefox's
    grey chrome.
    """
    packed = _packed_pixels(frame)
    # Quantise each colour plane to its top nibble in one C-speed translate;
    # the padding plane (packed[3::4]) is never read.
    b = packed[0::4].translate(_TOP_NIBBLE)
    g = packed[1::4].translate(_TOP_NIBBLE)
    r = packed[2::4].translate(_TOP_NIBBLE)
    from collections import Counter

    counts = Counter(zip(r, g, b))
    (rq, gq, bq), top = counts.most_common(1)[0]
    total = len(r)
    # Expand each quantised channel (a multiple of 0x10) by nibble replication
    # (n -> n*0x11), the inverse the acceptance pages are authored against.
    hex6 = "".join(f"{(v >> 4) * 0x11:02x}" for v in (rq, gq, bq))
    return hex6, top * 100 // total
