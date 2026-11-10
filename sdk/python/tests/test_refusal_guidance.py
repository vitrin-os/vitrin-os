# SPDX-License-Identifier: Apache-2.0
"""Every refusal the IDL defines tells an agent what to do.

Every :class:`vitrin_os.errors.GrantRefused` subclass carries ``guidance``
(what to do next, in words a language model can act on) and ``retryable``
(whether retrying the same use is the right next step). A refusal with no
guidance is one no agent understands, and the way that ships is an appended
enum entry nobody wrote advice for.

So the list of codes comes from ``protocol/vitrin-v0.xml``, never from this
file. A hand-copied list of refusals is exactly what went wrong before: a
table of "all" of them listed seven and named ``grant_expired``, a code that
does not exist (the wire name is ``expired``; only the SDK class is
``GrantExpired``), and its correction to eight was overtaken by ``capacity``
before the work it specified had landed. Appending an entry to
``vitrin_grant.refusal`` fails these tests until the SDK says what it means.

Skipped when the IDL is not on disk (an installed sdist, not a repo tree).
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from pathlib import Path

import pytest

from vitrin_os import errors, protocol

IDL = Path(__file__).resolve().parents[3] / "protocol" / "vitrin-v0.xml"

# Same skip class as test_verb_parity.py: the IDL is absent only outside a
# repo checkout, and `conftest.py` holds the skip to `VITRIN_REQUIRE_IDL`.
pytestmark = [
    pytest.mark.capability("idl"),
    pytest.mark.skipif(
        not IDL.is_file(), reason=f"IDL not present at {IDL} (not a repo checkout)"
    ),
]


def _refusal_entries() -> list[ET.Element]:
    root = ET.parse(IDL).getroot()
    # `vitrin_grant` names the interface on purpose: `vitrin_powerbox` has an
    # enum called `refusal` too, and it is a different enum with no exception
    # classes behind it.
    path = ".//interface[@name='vitrin_grant']/enum[@name='refusal']/entry"
    entries = root.findall(path)
    assert entries, "no vitrin_grant.refusal entries found in the IDL"
    return entries


def _class_for(value: int) -> type[errors.GrantRefused]:
    return type(errors.refusal_error_by_code(verb=1, code=value, retry_after_ms=0))


def _missing_guidance(entries: list[ET.Element]) -> list[str]:
    """One line for each IDL entry an agent would get no advice for.

    Checks the attributes the class defines ITSELF (``vars``), not what it
    can reach: a subclass of a subclass that forgot its own guidance would
    otherwise pass by quoting its parent's, which is advice about a
    different refusal.
    """
    problems = []
    for entry in entries:
        label = f"{entry.get('name')} ({entry.get('value')})"
        try:
            cls = _class_for(int(entry.get("value")))
        except errors.ServerContractViolation:
            problems.append(f"{label}: no GrantRefused subclass maps this code")
            continue
        own = vars(cls)
        guidance = own.get("guidance")
        if not isinstance(guidance, str) or not guidance.strip():
            problems.append(f"{label}: {cls.__name__} sets no guidance of its own")
        if not isinstance(own.get("retryable"), bool):
            problems.append(f"{label}: {cls.__name__} sets no retryable bool of its own")
    return problems


def test_refusal_members_match_the_idl() -> None:
    """`protocol.Refusal` is a hand transcription; pin it to the IDL."""
    from_idl = {e.get("name").upper(): int(e.get("value")) for e in _refusal_entries()}
    from_sdk = {member.name: int(member.value) for member in protocol.Refusal}
    assert from_sdk == from_idl


def test_every_idl_refusal_carries_guidance() -> None:
    problems = _missing_guidance(_refusal_entries())
    assert not problems, (
        "these vitrin_grant.refusal entries would reach an agent with no advice on "
        "what to do. Give each its own `guidance` and `retryable` in "
        "sdk/python/src/vitrin_os/errors.py:\n" + "\n".join(problems)
    )


def test_an_appended_refusal_fails_the_guidance_check() -> None:
    """The check above is not vacuous: a new IDL entry really does trip it.

    Appends one entry past the IDL's highest value to the parsed enum and runs
    the same check. If the check ever stopped looking at the IDL — or stopped
    failing on a code it cannot map — this is the test that goes red.
    """
    entries = _refusal_entries()
    next_value = max(int(e.get("value")) for e in entries) + 1
    appended = ET.Element(
        "entry", {"name": "appended_by_this_test", "value": str(next_value)}
    )
    problems = _missing_guidance([*entries, appended])
    assert len(problems) == 1, problems
    assert problems[0].startswith(f"appended_by_this_test ({next_value}):"), problems


# Keyed by IDL wire name and held to the IDL's own name set below, so this
# table cannot drift silently either: an appended refusal fails here until
# somebody decides whether retrying it is right.
#
# `revoked` and `preempted` are the two that matter: a human intervened, and
# an agent that retries through either is hammering through a human's
# override. `capacity` is False because asking again is only right after one
# of the agent's realms exits, never on a timer.
EXPECTED_RETRYABLE = {
    "not_granted": False,
    "expired": False,
    "revoked": False,
    "rate_limited": True,
    "preempted": False,
    "consent_held": True,
    "no_surface": True,
    "internal": True,
    "capacity": False,
}


def test_retryable_is_the_documented_behaviour() -> None:
    entries = _refusal_entries()
    assert set(EXPECTED_RETRYABLE) == {e.get("name") for e in entries}
    for entry in entries:
        cls = _class_for(int(entry.get("value")))
        expected = EXPECTED_RETRYABLE[entry.get("name")]
        assert cls.retryable is expected, (
            f"{entry.get('name')}: {cls.__name__}.retryable is {cls.retryable}, "
            f"expected {expected}"
        )
