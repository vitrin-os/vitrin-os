"""P1.8.3 (#42) acceptance: the SDK's actuation API and its typed grant-error
exceptions, verified against the shipped `vitrind` — the "live core" the issue
calls for, not the in-process mock the unit tests (`sdk/python/tests`) use.

The API surface and the refusal→exception mapping are exercised exhaustively
against the mock in `test_request_reply.py`; what only a live core can show is
that the *real* enforcement chokepoint produces those refusals — an expired
grant, an over-rate burst — and that the SDK raises the right typed exception
carrying the protocol payload.

Two acceptance items are deliberately NOT here, and neither is a gap:

- **`Revoked` (hold-Esc → `Revoked`)** is a *nested*-mode path: the dead-man
  chord is physical human input, which `--headless` has none of (P1.7.2/P1.7.3
  — headless is refused with `--consent=interactive`). The mapping is covered
  against the mock (`test_request_reply.py`), and its live verification is the
  M1.4 nested demo (P1.7.3's own acceptance: "hold Esc mid-demo; the agent's
  next actuation fails with revoked; the flight recorder shows it").

- **A click landing on a *surface* pixel (criterion 4, D10)** needs a real
  target that visibly responds — Firefox, or the `input-echo` client — which is
  the #43 demo's assertion. Here we verify the parameter path the SDK controls:
  what it actuates is what the chokepoint records, byte-for-byte.
"""

from __future__ import annotations

import time
import unittest

from harness import (
    IntegrationTest,
    capture_when_ready,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

WHILE_RUNNING = vitrin_os.Persistence.WHILE_RUNNING


def _refused(entries, code: str):
    """The `use_decision` entries the core recorded as refused with `code`."""
    return [
        e
        for e in entries
        if e["kind"] == "use_decision"
        and e.get("decision") == "refused"
        and e.get("refusal") == code
    ]


class ExpiredGrant(IntegrationTest):
    """An expired grant's next actuation raises :class:`GrantExpired`.

    Expiry is checked *at use* by the same predicate the advisory sweep uses
    (`session.rs`: "authority is re-checked at use time"), and the sweep marks
    rather than removes the row, so a use past the deadline is refused
    `expired` — never `not_granted` — whether or not the 1 s sweep has run.
    That is what makes this deterministic rather than a race.
    """

    def test_using_a_grant_past_its_expiry_raises_grantexpired(self):
        core = self.core()
        conn = core.connect()
        grant = conn.request_grant(
            verbs=("actuate.pointer",),
            persistence=WHILE_RUNNING,
            expiry_ms=100,
        ).await_consent()

        time.sleep(0.3)  # comfortably past the 100 ms deadline

        with self.assertRaises(errors.GrantExpired) as caught:
            grant.pointer.move(10, 20)
        # The payload identifies the grant and the verb (P1.8.3 criterion 3).
        self.assertEqual(caught.exception.grant_id, grant.id)
        self.assertEqual(caught.exception.code, errors.GrantExpired.REFUSAL)

        conn.close()
        core.terminate()
        self.assertTrue(
            _refused(core.entries(), "expired"),
            "the expired use must be refused at the chokepoint and recorded",
        )


class RateCeiling(IntegrationTest):
    """Bursting past `max_event_rate` raises :class:`RateLimited`.

    The per-grant token bucket has capacity = `max_event_rate` (one second of
    burst; `enforcement.rs`), so at rate 1 the first event consumes the token
    and the second — within the same second — is refused `rate_limited`. No
    timing slack needed: it is the *second* event, not the wall clock, that
    empties the bucket.
    """

    def test_the_second_event_at_rate_one_raises_ratelimited(self):
        core = self.core()
        conn = core.connect()

        # Warm the realm's surface FIRST, on a SEPARATE default-rate grant. The
        # `no_surface` gate is judged before the rate bucket, so without a live
        # surface `move(1, 1)` would be refused `no_surface` and never consume
        # the single token — the burst would then not be over-rate. The warm-up
        # must ride its own grant: the token bucket is per-grant, and warming on
        # the rate=1 grant would itself spend the one token this test needs.
        capture_when_ready(whole_realm_grant(conn))

        grant = conn.request_grant(
            verbs=("actuate.pointer",),
            persistence=WHILE_RUNNING,
            max_event_rate=1,
        ).await_consent()

        grant.pointer.move(1, 1)  # surface live -> admitted, consumes the token

        with self.assertRaises(errors.RateLimited) as caught:
            grant.pointer.move(2, 2)  # bucket empty -> refused
        self.assertEqual(caught.exception.grant_id, grant.id)
        self.assertGreater(
            caught.exception.retry_after_ms,
            0,
            "RateLimited must carry the refill hint (never zero, rounded up)",
        )

        conn.close()
        core.terminate()
        self.assertTrue(
            _refused(core.entries(), "rate_limited"),
            "the over-rate use must be refused at the chokepoint and recorded",
        )


class ActuationEcho(IntegrationTest):
    """What the SDK actuates is what the chokepoint records, verbatim.

    The "input echo" a headless run can observe: the flight recorder's
    `use_decision` entry reproduces the actuation's parameters (D10 coordinates,
    the typed string's shape). This verifies the SDK→core parameter path; the
    pixel actually landing on the app's surface is the #43 demo's assertion.
    """

    def test_move_and_type_parameters_reach_the_chokepoint_verbatim(self):
        core = self.core()
        conn = core.connect()
        grant = conn.request_grant(
            verbs=("observe", "actuate.pointer", "actuate.text"),
            persistence=WHILE_RUNNING,
        ).await_consent()

        # Wait for the realm's first frame before actuating: `no_surface` is
        # judged before the actuation runs, so a race-lost move/type would be
        # refused `NoSurface` and leave no admitted `use_decision` for the
        # assertions. A `no_surface` retry costs no rate budget, and the one
        # admitted capture leaves the default 20/s bucket ample for the two
        # actuations that follow.
        capture_when_ready(grant)

        grant.pointer.move(37, 42)
        secret = "héllo→世界"
        grant.text.type(secret)

        conn.close()
        core.terminate()

        allowed = [
            e
            for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "allowed"
        ]
        # A capture's use_decision carries `input: null` (JSON null decodes to
        # None, not a missing key), so coerce before indexing; the warm-up
        # observe above is exactly such an entry.
        moves = [e for e in allowed if (e.get("input") or {}).get("action") == "move"]
        self.assertTrue(
            any(m["input"]["x"] == 37 and m["input"]["y"] == 42 for m in moves),
            f"the actuated coordinate must reach the chokepoint unchanged; saw {moves}",
        )
        types = [e for e in allowed if (e.get("input") or {}).get("action") == "type"]
        self.assertTrue(
            # Shape only — the recorder never writes the typed bytes (keylogger
            # avoidance); the string round-trips as its char count and digest.
            any(t["input"]["chars"] == len(secret) for t in types),
            f"the typed string's shape must reach the chokepoint; saw {types}",
        )


if __name__ == "__main__":
    unittest.main()
