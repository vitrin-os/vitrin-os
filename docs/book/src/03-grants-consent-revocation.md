# Grants, consent, and revocation

This is the security model. If you read one chapter, read this one.

## No ambient authority

The rule the whole design turns on: **a connection confers nothing.** Being
connected, being authenticated, even being a highly trusted principal —
none of it lets you observe or touch anything. Authority exists only as
grants, and a grant is checked on every single action.

Compare what the alternatives do:

| | Unit of authority | Who can revoke | Granularity |
|---|---|---|---|
| X11 | The connection | Nobody, really | The whole session |
| Wayland | The connection | Nobody | Your own surfaces only |
| AT-SPI2 | Ambient — anyone on the bus | Nobody | Every widget of every app |
| VM-per-agent | The VM | Destroy the VM | One whole desktop |
| **Vitrin** | **The grant** | **Anyone, immediately** | **Verb × resource × constraints** |

## What a grant is

A row in the core's grant table:

```
(principal × resource × verbs × constraints)
```

- **principal** — who. Authenticated at handshake, never asserted by the
  requester afterwards.
- **resource** — what. A realm, or a specific surface within it.
- **verbs** — which actions. `observe`, `actuate.pointer`, `actuate.text` and
  the two `layout.*` verbs today; two more are defined and refuse
  `unsupported` — `observe.cursor` and `realm.launch` (added at wire version
  2). Defining a verb before serving it is deliberate: it makes asking for one
  a recoverable refusal instead of a fatal out-of-range bit. Which of the
  defined verbs a deployment actually serves is that deployment's property,
  not the wire's.
- **constraints** — under what limits: expiry, event-rate ceiling, focus
  conditions, persistence.

Three properties make it a capability rather than a permission bit:

**Sender-constrained.** The grant is bound to the connection that petitioned
for it. Stealing the identifier gets you nothing; you would have to be that
connection.

**Attenuable.** A grant can be narrowed — never widened — and handed on. An
agent that needs a sub-agent to do one thing can pass a grant that permits
exactly that thing and expires sooner.

**Revocable, immediately and transitively.** Revoking a grant kills
everything attenuated from it, in the same operation. Not eventually. Not at
the next check-in.

## The lifecycle

```
   request_grant()
        │
        ▼
   ┌─────────┐  the facets exist here, and confer NOTHING
   │ pending │  ← consent prompt is up; actuation refuses ConsentHeld
   └─────────┘
        │
        ├──── denied / timeout / busy ──→ raises, no authority ever existed
        │
        ▼
   ┌──────────┐
   │ resolved │  ← effective_verbs() may be NARROWER than requested
   └──────────┘
        │
        ├──── expiry_ms elapses ────────→ GrantExpired
        ├──── human holds Escape ───────→ Revoked (and everything attenuated)
        ├──── human touches the mouse ──→ Preempted
        └──── over the rate ceiling ────→ RateLimited (retry_after_ms)
```

A grant resolves **exactly once**. There is no path back to pending, and no
way to re-open a resolved grant into a wider one.

## Consent the core draws itself

The prompt asking you to approve a petition is rendered by `vitrind` — the
process that owns the screen and the input devices. That is the whole
security argument, and it is worth being precise about why it works.

An application cannot draw a convincing fake, because:

1. The core composites the prompt **above** every client surface. There is
   no z-order a client can request that goes higher.
2. The core takes an **exclusive input grab** while it is up. Clicks land on
   the prompt, not on whatever is beneath it.
3. Actuation on already-granted grants refuses `ConsentHeld` while a prompt
   is up — so an agent cannot act during the window in which a human is
   being asked about it.

This has its own mock-free gate, `tests/integration/test_real_consent.py`,
and the gate is stricter than "a prompt appeared". It proves the exported
footprint really is a card raster at exactly the rectangle the core named —
accent ring on all four edges, exact perimeter count, opaque body, buttons,
antialiased text — and then that it carries **zero** of the app's pixels.
Separately, it proves the prompt does **not** leak into the capture path:
the realm-view dump taken mid-prompt is byte-identical to a settled control,
and the agent's own `observe()` agrees with it.

Then it proves the freeze: a mid-prompt actuation on an already-granted
grant, on a *second connection of the same principal*, refuses `ConsentHeld`
specifically — and the journal shows that refusal falling strictly between
the prompt's `shown` and its resolution.

### The trusted indicator, and what it does not prove

The core paints a band it owns, in a colour randomised per session. A client
cannot match a colour it cannot observe.

`test_real_trust_band.py` proves the negative rigorously: a real app repaints
its *entire* surface, band rows included, and the band's rows still carry the
app's colour in both capture artifacts rather than the indicator's — with a
core-side witness reporting zero band changes across every composite it
evaluated, held up by counterweights so a witness wired only into the reply
path would fail. The harness never learns the indicator colour.

**This is a proof that the band cannot be forged by a client. It is not a
proof that a human notices when it is wrong.** Those are different claims,
and the second one needs user research this project has not done. The plan
explicitly adjudicated unspoofability *out* of M1.4's criteria for that
reason — so do not cite the milestone as evidence for it.

## Human override

Physical input preempts agent input **by construction**, not by a race.
Input is origin-tagged at the core: the router knows which events came from
a human device and which came from an agent's actuation call, and the human
wins because the code says so, not because it arrived first.

## The dead-man switch

Hold Escape for one second. Every live grant is revoked.

The agent's very next call — `observe()` or any actuation — raises
`Revoked`. Not "at its next poll", not "within a few seconds":
`test_real_deadman.py` asserts both refuse on the *immediately following*
check, that the real app's target is left untouched (read via
`--capture-dump`, which bypasses the now-revoked grant entirely), and that
the flight recorder journals `dead_man_triggered` followed by
`grant_revoked`.

Headless has no physical key to hold, so that gate uses a signal to stand in
for the chord. The nested recipe for a genuinely held Escape is in
[`shim/docs/firefox.md`](https://github.com/vitrin-os/vitrin-os/blob/main/shim/docs/firefox.md)
§9 — and it is worth doing once by hand, because watching an agent die
mid-keystroke is the moment the model stops being abstract.

## The chokepoint

Every one of these checks happens in one place. There is no fast path, no
cache that skips the grant table, and no module that can act without going
through it. That is what makes the trusted core auditable: the interesting
question is only ever "what does the chokepoint do", never "which of forty
call sites forgot to check".

The flight recorder journals each decision, so a run is reconstructible
after the fact:

```sh
jq -c 'select(.event | test("grant|consent|revok|refus"))' flight.jsonl
```

Next: [Realms and shims](04-realms-and-shims.md).
