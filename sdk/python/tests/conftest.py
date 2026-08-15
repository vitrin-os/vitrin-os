# SPDX-License-Identifier: Apache-2.0
"""Fixtures, and the pytest half of the sanctioned skip (issue #288).

`crates/vitrin-skip` makes a Rust skip loud and, on a machine that declared
it could run, fatal. Until this file, the Python suite got **reporting and
no enforcement**: `pytest -q -rs` printed each skip, an expected set lived in
a prose comment in `.github/workflows/ci.yml`, and nothing compared the two.
A comment is not a gate. This is the gate, and it mirrors the Rust design
exactly rather than inventing a second vocabulary:

* every skip must carry a ``@pytest.mark.capability("<class>")`` naming the
  machine state it describes — an unclassified skip fails the run, the way
  a marker with no ``INVENTORY`` entry fails ``cargo xtask skip-census``;
* each class has a **require-variable** that *excuses* rather than requires:
  under CI a skip is a failure unless the job set that variable to ``0``, so
  a typo in the workflow makes a job red instead of silently disabling the
  gate, and an unrecognised value is rejected rather than read as "off";
* every skip prints one machine-readable ``VITRIN-SKIP`` marker line, the
  same shape the Rust emitters use, so one grep reads both halves;
* the session fails if it collected fewer tests than ``VITRIN_MIN_TESTS``,
  because a run that collected nothing exits 0 and proves nothing — the same
  floor ``cargo xtask skip-census --min-tests`` applies on the Rust side.

What this does **not** do is stop a test from asserting nothing. It measures
whether tests ran, not whether they were worth running.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

# Make the sibling test helpers importable regardless of rootdir layout.
sys.path.insert(0, str(Path(__file__).parent))

from mock_server import MockServer  # noqa: E402

#: The one string the emitters print and a census greps for, kept
#: character-for-character identical to ``vitrin_skip::MARKER``.
MARKER = "VITRIN-SKIP"

#: Each skip class, and the variable a job sets to declare this machine able
#: to satisfy it. Adding a class here is the visible decision, the same way
#: adding an ``INVENTORY`` row is on the Rust side.
CAPABILITIES = {
    # The test-only independent PNG decoder. Installed on one matrix leg on
    # purpose: the other proves the stdlib round-trip needs no decoder at
    # all, so THAT leg declares `VITRIN_REQUIRE_PILLOW=0` and its skip is
    # honest. The leg that installs Pillow declares nothing and is required.
    "pillow": "VITRIN_REQUIRE_PILLOW",
    # `protocol/vitrin-v0.xml`, absent only in an installed sdist. Every CI
    # job runs from a checkout, so no job declares this and every job is
    # required: a skip here would mean the checkout is what is wrong.
    "idl": "VITRIN_REQUIRE_IDL",
}


def _is_required(klass: str) -> bool:
    """Whether a skip for ``klass`` is a failure on this machine.

    **Fail-closed under CI**, which is the whole polarity question and the
    reason it is written this way: an unset variable under CI means
    "required", so a one-character typo in ``.github/workflows/ci.yml`` turns
    a job RED rather than quietly disabling the enforcement. The variable
    EXCUSES rather than requires — the same shape as
    ``vitrin_skip::Require::UnderCiUnlessDeclared``, which is the one pattern
    that already worked in this repository before #288.

    Off CI it is inert: a developer with no Pillow installed still skips.

    An unrecognised value is rejected rather than treated as "off": an
    unvalidated variable is an enforcement mechanism a typo can switch off,
    which is the failure class this file exists to close.
    """
    raw = os.environ.get(CAPABILITIES[klass])
    if raw is None:
        return "CI" in os.environ
    value = raw.strip()
    if value == "1":
        return True
    if value == "0":
        return False
    raise pytest.UsageError(
        f"{CAPABILITIES[klass]}={raw!r} is not a recognised value: use `1` (this "
        f"machine can satisfy the `{klass}` class, so a skip is a failure) or `0` "
        f"(it genuinely cannot, so its skip is honest). Unset means `1` under CI "
        f"and `0` off it."
    )


def _floor() -> int:
    """The collection floor, validated the way the require-variables are.

    ``VITRIN_MIN_TESTS`` is the pytest side of ``cargo xtask skip-census
    --min-tests``, and it is held to the same two rules for the same reasons:

    * a value that is not a non-negative integer is REJECTED rather than
      crashing inside a hook halfway through a run. An ``int()`` that raises
      in ``pytest_sessionfinish`` produces a traceback nobody reads as "your
      workflow has a typo", and a mechanism that fails obscurely is a
      mechanism people switch off;
    * **zero is refused under CI**, and so is leaving it unset. A floor of
      zero is satisfied by a run that collected nothing, so it is an escape
      hatch shaped like compliance -- the one spelling of "no floor" that a
      reviewer would not read as a decision. Off CI it is the default and
      inert, because a developer running a subset with ``-k`` is not making a
      claim about anything.
    """
    raw = os.environ.get("VITRIN_MIN_TESTS")
    under_ci = "CI" in os.environ
    if raw is None:
        if under_ci:
            raise pytest.UsageError(
                "VITRIN_MIN_TESTS is unset and this is CI. Every job that runs this "
                "suite has to declare how many tests it expects to COLLECT, because a "
                "discovery pattern that matches nothing exits 0 and `0 skipped` over "
                "it is a claim about tests that did not run (issue #288). Measure the "
                "number for this job and set it; there is deliberately no default."
            )
        return 0
    try:
        value = int(raw.strip())
    except ValueError:
        raise pytest.UsageError(
            f"VITRIN_MIN_TESTS={raw!r} is not a number. It is the count of tests this "
            f"run must collect before its census may say anything affirmative; a "
            f"value nobody validates is an enforcement mechanism a typo can switch "
            f"off, which is issue #288 in the code that closes issue #288."
        ) from None
    if value < 0:
        raise pytest.UsageError(
            f"VITRIN_MIN_TESTS={raw!r} is negative, and a negative floor is met by "
            f"every possible run."
        )
    if value == 0 and under_ci:
        raise pytest.UsageError(
            "VITRIN_MIN_TESTS=0 under CI is refused. A floor of zero is satisfied by "
            "a run that collected NOTHING -- an escape hatch shaped like compliance, "
            "and the exact hole the floor exists to close. Declare the real number."
        )
    return value


def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "capability(name): the machine capability this test's skip describes "
        "(issue #288). Every skip must carry one; see sdk/python/tests/conftest.py.",
    )
    # Validated HERE rather than where they are first read, so a workflow typo
    # fails in the first second of a job with a message about the workflow,
    # instead of after the whole suite has run -- or, for a class nothing
    # happened to skip, not at all.
    config._vitrin_floor = _floor()
    for klass in CAPABILITIES:
        _is_required(klass)
    # (nodeid -> class) for the tests that declared one, filled at collection.
    config._vitrin_capability = {}
    # (nodeid, class, reason) for every skip this session saw.
    config._vitrin_skips = []


def pytest_collection_modifyitems(config, items):
    for item in items:
        mark = item.get_closest_marker("capability")
        if mark is not None and mark.args:
            config._vitrin_capability[item.nodeid] = mark.args[0]


def pytest_runtest_logreport(report):
    if not report.skipped:
        return
    # `longrepr` for a skip is (path, lineno, "Skipped: <reason>").
    reason = ""
    if isinstance(report.longrepr, tuple) and len(report.longrepr) == 3:
        reason = str(report.longrepr[2]).removeprefix("Skipped: ")
    config = getattr(report, "_vitrin_config", None) or _CONFIG[0]
    klass = config._vitrin_capability.get(report.nodeid)
    config._vitrin_skips.append((report.nodeid, klass, reason))


#: `pytest_runtest_logreport` is handed no config, so one is stashed here at
#: session start. A list rather than a bare global so the assignment inside
#: the hook needs no `global` declaration.
_CONFIG: list = [None]


def pytest_sessionstart(session):
    _CONFIG[0] = session.config


def pytest_sessionfinish(session, exitstatus):
    config = session.config
    skips = getattr(config, "_vitrin_skips", [])
    problems = []

    for nodeid, klass, reason in skips:
        print(f"{MARKER} {klass or 'UNCLASSIFIED'} {nodeid} {reason}")
        if klass is None:
            problems.append(
                f"{nodeid} skipped and carries no `@pytest.mark.capability(...)`. "
                f"Name the machine state its skip describes and add the class to "
                f"CAPABILITIES in sdk/python/tests/conftest.py, so a job that can "
                f"satisfy it is able to require it. An unclassified skip is a green "
                f"count standing in for absent evidence (issue #288)."
            )
        elif klass not in CAPABILITIES:
            problems.append(
                f"{nodeid} names capability `{klass}`, which is not one of "
                f"{sorted(CAPABILITIES)}."
            )
        elif _is_required(klass):
            problems.append(
                f"{nodeid} skipped for class `{klass}`, and {CAPABILITIES[klass]}=1 "
                f"declares that this machine can satisfy it. Either the machine "
                f"regressed, or this skip is for a reason that has nothing to do "
                f"with the class it named. Reason given: {reason}"
            )

    # The floor. A suite that collected nothing exits 0, and "0 skipped" over
    # it is an affirmative claim about tests that did not run. Read from the
    # config, where `pytest_configure` already validated it -- parsing it here
    # would mean a bad value surfaced as a traceback at the END of a run.
    floor = getattr(config, "_vitrin_floor", 0)
    collected = session.testscollected
    if collected < floor:
        problems.append(
            f"this run collected {collected} test(s) and VITRIN_MIN_TESTS declares a "
            f"floor of {floor}. A discovery pattern change, a module that fails to "
            f"import under a `--continue-on-collection-errors`, or a renamed "
            f"directory all look like this and all exit 0."
        )

    if collected >= floor:
        print(
            f"==> pytest skip census: {len(skips)} skip(s) in {collected} collected "
            f"test(s) (floor {floor})."
        )
    else:
        print(
            f"==> pytest skip census: NOT ASSERTED. {collected} collected test(s), "
            f"floor {floor}."
        )

    for problem in problems:
        print(f"pytest skip census: {problem}", file=sys.stderr)
    if problems:
        session.exitstatus = 1


@pytest.fixture
def server(tmp_path):
    """A scripted mock core listening on a fresh Unix socket path."""
    srv = MockServer(str(tmp_path / "core.sock"))
    try:
        yield srv
        srv.join()
    finally:
        srv.close()
