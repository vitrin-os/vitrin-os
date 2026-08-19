# SPDX-License-Identifier: Apache-2.0
"""`protocol.Verb` is a hand transcription; this pins it to the IDL.

`vitrin_os.protocol` is deliberately not generated (decision D8: the Python
SDK is an independent second implementation of the wire format). The cost of
that choice is silent drift — an appended verb bit is invisible to every
other test here, because `enum.IntFlag` accepts undefined bits and no golden
vector covers the verb field. This module pays that cost down by reading
`protocol/vitrin-v0.xml` and comparing, without turning the transcription
into generated code: it asserts the two agree, it does not produce one from
the other.

Skipped when the IDL is not on disk (an installed sdist, not a repo tree).
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from pathlib import Path

import pytest

from vitrin_os import protocol

IDL = Path(__file__).resolve().parents[3] / "protocol" / "vitrin-v0.xml"

# The `capability` mark names WHICH machine state this module's skip
# describes, so `conftest.py` can hold it to `VITRIN_REQUIRE_IDL` (issue
# #288). A skip with no `capability` mark is an unclassified skip and fails
# the run — the pytest side of `crates/xtask`'s INVENTORY, and the reason the
# expected skip set is no longer a prose comment in .github/workflows/ci.yml.
pytestmark = [
    pytest.mark.capability("idl"),
    pytest.mark.skipif(
        not IDL.is_file(), reason=f"IDL not present at {IDL} (not a repo checkout)"
    ),
]

# The IDL's own phrasing for a verb that version 1 defines but refuses.
UNSERVED_MARKER = "refused unsupported in version 1"


def _verb_entries() -> list[ET.Element]:
    root = ET.parse(IDL).getroot()
    path = ".//interface[@name='vitrin_grant']/enum[@name='verb']/entry"
    entries = root.findall(path)
    assert entries, "no vitrin_grant.verb entries found in the IDL"
    return entries


def _dotted(wire_name: str) -> str:
    """The IDL's naming rule: the first underscore becomes a dot."""
    return wire_name.replace("_", ".", 1)


def test_verb_members_match_the_idl() -> None:
    """Every IDL entry is a Verb member with the same value, and vice versa."""
    from_idl = {e.get("name").upper(): int(e.get("value")) for e in _verb_entries()}
    from_sdk = {member.name: int(member.value) for member in protocol.Verb}
    assert from_sdk == from_idl


def test_dotted_names_match_the_idl() -> None:
    """VERB_BY_DOTTED_NAME spells every entry the way the IDL fixes it."""
    from_idl = {_dotted(e.get("name")): int(e.get("value")) for e in _verb_entries()}
    from_sdk = {name: int(bit) for name, bit in protocol.VERB_BY_DOTTED_NAME.items()}
    assert from_sdk == from_idl


def test_verb_mask_covers_exactly_the_defined_bits() -> None:
    """VERB_MASK is the fatal/recoverable boundary — it must not lag the IDL.

    A bit inside the mask is answered `unsupported` on the grant; a bit
    outside it is fatal `invalid_argument`. A stale mask would make the SDK
    reject, client-side, a petition the core would have answered.
    """
    defined = 0
    for entry in _verb_entries():
        defined |= int(entry.get("value"))
    assert protocol.VERB_MASK == defined


def test_the_unserved_marker_is_actually_a_phrase_the_idl_uses() -> None:
    """The served/unserved split below is derived from a STRING MATCH.

    That derivation has a vacuity mode the assertion after it cannot see: if
    the IDL rephrased its unserved entries, every verb would read as served,
    and a hand-edit of `VERBS_SERVED_IN_VERSION_1` to "all of them" would make
    the two agree again — a green test asserting nothing, exactly the shape
    D-017 recorded. So pin that the phrase is still load-bearing: at least one
    entry carries it, and at least one does not.

    At wire version 2 the marked side is `observe_cursor`, `designate_file`
    (added by P2.6.5, and unserved by every deployment until its picker and
    its consent copy exist) and `realm_launch` — the last of which stays out
    of the SERVED set for a reason about the version rather than the
    deployment (a version-1 connection cannot mint `vitrin_launcher` at all),
    and the IDL's own summary says so, which is why deriving from the IDL
    rather than from a second list is the point. No count is stated here: the
    assertions below are over the derived sets, and the one sentence that did
    state a count ("`observe_cursor` alone") went false the moment a second
    verb landed.
    """
    summaries = [(e.get("summary") or "") for e in _verb_entries()]
    marked = [s for s in summaries if UNSERVED_MARKER in s]
    assert marked, (
        f"no vitrin_grant.verb entry says {UNSERVED_MARKER!r} any more. Either every "
        "verb became served — in which case say so here deliberately — or the IDL "
        "rephrased and this whole derivation silently became a tautology."
    )
    assert len(marked) < len(summaries), (
        "every verb entry is marked unserved, so the derivation cannot discriminate"
    )


def test_served_set_matches_the_idl_summaries() -> None:
    """The served/unserved split tracks the IDL, not this file's memory.

    If this fails because a verb became served, update
    `VERBS_SERVED_IN_VERSION_1` — the SDK claiming a verb is refused when the
    core now grants it is the drift this test exists to catch.
    """
    served = 0
    for entry in _verb_entries():
        if UNSERVED_MARKER not in (entry.get("summary") or ""):
            served |= int(entry.get("value"))
    assert protocol.VERBS_SERVED_IN_VERSION_1 == served
