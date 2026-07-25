"""**Component test, NOT a milestone gate** (issue #138): the fail-closed
matrix of the `consent-injector` channel, driven against the shipped `vitrind`
binary and `vitrin-mock-shim`.

Plan §5 D12 is explicit that a test wired to `vitrin-mock-shim` may never be
cited as a milestone's definition-of-done evidence, and nothing here claims
otherwise. M1.4's consent half is closed by `test_real_consent.py`, which
drives the same channel against the real C shim and a real `click-target`.

This module exists to keep that gate about the milestone property. The
interesting cases for a *channel* are all the unhappy ones -- a replayed
token, a button the card does not draw, a line that is not a request -- and
each of them wants a fresh core with a prompt in a known state. Running them
on the real chain would add a real app boot to every one of them and would
bury the one assertion the gate is for.

# What each case pins

Every one of them is a way the channel could **widen** authority, and the
required answer is always "nothing was queued":

* `decide` with no card up -> `no-prompt`;
* a button the raised prompt does not offer -> `no-such-button`. The
  `PromptContent::choices()` filter only offers rungs that *narrow* the
  petition, so a `once` petition never draws "Allow while running";
* a token that names nothing -> `unknown-token`;
* a **spent** token replayed while the same card is still up ->
  `unknown-token`, and exactly one `petition_resolved` in the journal;
* a line that is not a request -> `decided-ack malformed` and **nothing
  queued** -- deliberately not a synthesised `Deny`, which would journal a
  refusal no human took. The petition simply stays pending, which is equally
  fail-closed and true;
* the peer disappearing mid-prompt -> the core survives, keeps serving, and
  the petition stays pending (it would resolve `timed_out` after the
  registry's 120 s consent timeout, which is longer than this suite's
  per-test deadline and so is not waited out here).

# Requires a `consent-injector` build

`--headless --consent=interactive --consent-injector-fd N` parses in exactly
one build of `vitrind`. `tests/integration/run.sh` builds it; a plain build
fails these tests at `Core()` construction with the core's own refusal, which
`_WRONG_BUILD` below explains.
"""

from __future__ import annotations

import unittest

from harness import ALL_VERBS, ConsentInjector, CoreFailed, IntegrationTest, require_binaries

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

#: Big enough that the 560-wide consent card fits below the trust band, so
#: `describe` can export its footprint instead of refusing on geometry.
REALM_SIZE = "640x480"

#: A syntactically valid token that names no prompt: 16 lowercase hex chars,
#: which is exactly what `PromptToken::parse_hex` accepts.
BOGUS_TOKEN = "0123456789abcdef"

_WRONG_BUILD = (
    "This is what a `vitrind` built WITHOUT the `consent-injector` cargo feature does: it "
    "REFUSES `--headless --consent=interactive` at startup by design, and it does not know the "
    "`--consent-injector-fd` flag at all. Rebuild with `cargo build --workspace --features "
    "vitrin-core/dead-man-injector,vitrin-core/consent-injector` -- tests/integration/run.sh "
    "does this automatically, and CI's warm-build step passes the same feature list."
)


class ConsentInjectorFailsClosed(IntegrationTest):
    """Every way the channel could widen authority, and the refusal for it."""

    def injected_core(self):
        try:
            return self.core(
                consent="interactive",
                size=REALM_SIZE,
                consent_injector=True,
            )
        except CoreFailed as exc:
            self.fail(f"{_WRONG_BUILD}\n\nThe core's own words:\n{exc}")

    def _ready(self):
        core = self.injected_core()
        injector = core.injector
        assert isinstance(injector, ConsentInjector)
        self.assertEqual(
            injector.await_banner(),
            "vitrin-consent-injector 1",
            "an instrumented core greets the channel it adopted",
        )
        return core, injector

    def _petition(self, conn, persistence=vitrin_os.Persistence.WHILE_RUNNING):
        """Petition WITHOUT waiting: the pending window is the subject here."""
        return conn.request_grant(verbs=ALL_VERBS, persistence=persistence)

    # -- no card up --------------------------------------------------------

    def test_a_decision_with_no_prompt_up_is_refused_and_queues_nothing(self):
        core, injector = self._ready()
        self.assertEqual(injector.decide(BOGUS_TOKEN, "allow-while-running"), "no-prompt")
        fields, pixels = injector.describe()
        self.assertEqual(fields["state"], "none")
        self.assertEqual(fields["bytes"], 0)
        self.assertIsNone(pixels, "with no card up there is nothing to export")

        core.terminate()
        kinds = core.kinds()
        self.assertNotIn(
            "petition_resolved",
            kinds,
            "a decision with no prompt up must resolve nothing at all",
        )

    # -- a button the card does not draw -----------------------------------

    def test_a_button_the_prompt_does_not_offer_is_refused(self):
        core, injector = self._ready()
        conn = core.connect()
        # A `once` petition: approval may only NARROW, so `while_running` is
        # not one of `PromptContent::choices()` and the card never draws it.
        grant = self._petition(conn, persistence=vitrin_os.Persistence.ONCE)
        _petition_id, token = injector.await_raised()

        self.assertEqual(
            injector.decide(token, "allow-while-running"),
            "no-such-button",
            "the channel may not synthesise a press on a button the card does not draw",
        )
        # The card is untouched and the token is unspent, so the honest
        # decision still lands -- which is what proves the refusal above
        # rejected the *button*, not the token or the prompt.
        fields, _pixels = injector.describe()
        self.assertEqual(fields["state"], "shown")
        self.assertEqual(fields["token"], token)
        self.assertEqual(injector.decide(token, "allow-once"), "queued")
        grant.await_consent()
        self.assertEqual(grant.effective_persistence, vitrin_os.Persistence.ONCE)

        conn.close()
        core.terminate()
        resolved = [e for e in core.entries() if e["kind"] == "petition_resolved"]
        self.assertEqual(len(resolved), 1, f"exactly one resolution: {resolved}")
        self.assertEqual(resolved[0].get("outcome"), "granted")

    # -- tokens ------------------------------------------------------------

    def test_a_token_that_names_no_prompt_is_refused(self):
        core, injector = self._ready()
        conn = core.connect()
        grant = self._petition(conn)
        _petition_id, token = injector.await_raised()
        self.assertNotEqual(token, BOGUS_TOKEN)

        self.assertEqual(injector.decide(BOGUS_TOKEN, "deny"), "unknown-token")
        # The real token still works, so the refusal was about the name.
        self.assertEqual(injector.decide(token, "deny"), "queued")
        with self.assertRaises(errors.GrantDenied):
            grant.await_consent()

        conn.close()
        core.terminate()
        resolved = [e for e in core.entries() if e["kind"] == "petition_resolved"]
        self.assertEqual(len(resolved), 1, f"exactly one resolution: {resolved}")
        self.assertEqual(resolved[0].get("outcome"), "denied")

    def test_a_spent_token_replayed_in_the_same_batch_is_refused(self):
        core, injector = self._ready()
        conn = core.connect()
        grant = self._petition(conn)
        _petition_id, token = injector.await_raised()

        # Both lines in ONE write, so the core drains them inside a single
        # `service_injector` call -- before `post_dispatch` can lower the
        # card. The second is therefore judged against a prompt that is
        # provably still on screen, which is what makes `unknown-token` (a
        # spent name) the right answer rather than `no-prompt`.
        injector.send_raw(
            f"decide {token} deny\ndecide {token} allow-while-running\n".encode()
        )
        self.assertEqual(injector.next_ack(), "queued")
        self.assertEqual(
            injector.next_ack(),
            "unknown-token",
            "a token is spent by the decision it carried; a replay may never widen it",
        )
        with self.assertRaises(errors.GrantDenied):
            grant.await_consent()

        conn.close()
        core.terminate()
        resolved = [e for e in core.entries() if e["kind"] == "petition_resolved"]
        self.assertEqual(
            len(resolved),
            1,
            f"one petition, one resolution -- the replay must have conferred nothing: {resolved}",
        )
        self.assertEqual(resolved[0].get("outcome"), "denied")

    # -- malformed input ---------------------------------------------------

    #: Everything the core must answer `malformed` and act on in no way. The
    #: Rust-side negative table (`consent::injector`'s
    #: `the_injector_recognises_no_line_that_is_not_a_request`) is the
    #: exhaustive one; this is the subset worth proving travels a real socket
    #: to a real running core.
    MALFORMED = [
        b"\n",
        b"decide\n",
        b"decide 7\n",
        b"decide 7 allow-once\n",
        b"allow\n",
        b"yes\n",
        b"DESCRIBE\n",
        b"decide %s ALLOW-ONCE\n" % BOGUS_TOKEN.encode(),
        b"decide %s allow-once extra\n" % BOGUS_TOKEN.encode(),
        b"decide %s deny\x00\n" % BOGUS_TOKEN.encode(),
    ]

    def test_malformed_lines_are_refused_and_queue_nothing(self):
        core, injector = self._ready()
        conn = core.connect()
        grant = self._petition(conn)
        _petition_id, token = injector.await_raised()

        for raw in self.MALFORMED:
            injector.send_raw(raw)
            self.assertEqual(
                injector.next_ack(),
                "malformed",
                f"{raw!r} must be refused as unparseable",
            )
            # ...and nothing about the prompt changed: same card, same token.
            # A `Deny` synthesised from an unparseable line would have taken
            # the card down here and journalled a refusal nobody took.
            fields, _pixels = injector.describe()
            self.assertEqual(fields["state"], "shown", f"{raw!r} took the card down")
            self.assertEqual(fields["token"], token, f"{raw!r} disturbed the token")

        self.assertFalse(grant.resolved, "no malformed line may resolve a petition")
        self.assertEqual(injector.decide(token, "deny"), "queued")
        with self.assertRaises(errors.GrantDenied):
            grant.await_consent()

        conn.close()
        core.terminate()
        resolved = [e for e in core.entries() if e["kind"] == "petition_resolved"]
        self.assertEqual(len(resolved), 1, f"exactly one resolution: {resolved}")

    def test_an_over_long_line_closes_the_channel_and_the_core_survives(self):
        core, injector = self._ready()
        conn = core.connect()
        grant = self._petition(conn)
        _petition_id, _token = injector.await_raised()

        # `MAX_LINE` is 128; this is a kilobyte with no newline in it, which
        # is the shape a peer would use to make the core buffer without bound.
        injector.send_raw(b"x" * 1024)
        # The core drops the source and keeps running: still serving the
        # protocol socket, and the petition it cannot now be told about stays
        # pending rather than being resolved either way.
        second = core.connect()
        second.close()
        self.assertIsNone(core.proc.poll(), "the core must survive a hostile peer")
        self.assertFalse(grant.resolved, "a dropped channel must resolve nothing")

        conn.close()
        core.terminate()
        self.assertNotIn("petition_resolved", core.kinds())

    def test_the_peer_disappearing_mid_prompt_leaves_the_core_serving(self):
        core, injector = self._ready()
        conn = core.connect()
        grant = self._petition(conn)
        _petition_id, _token = injector.await_raised()

        injector.close()
        core.injector = None
        second = core.connect()
        second.close()
        self.assertIsNone(core.proc.poll(), "EOF on the test channel is not a fatal condition")
        self.assertFalse(grant.resolved, "an absent peer must resolve nothing")

        conn.close()
        core.terminate()
        self.assertNotIn("petition_resolved", core.kinds())


if __name__ == "__main__":
    unittest.main()
