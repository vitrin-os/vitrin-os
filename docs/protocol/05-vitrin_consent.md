# vitrin_consent — prompt-visibility observer for one petition

**Interface version:** 1 · **Connection class:** principal · **Messages:** 0 requests + 1 event

See [conventions](./00-conventions.md) for framing, object ids, the error
taxonomy, delivery classification, and versioning. This page documents only
what is specific to `vitrin_consent`.

## Purpose

`vitrin_consent` is the prompt-visibility lifecycle of exactly one petition,
visible to exactly its petitioner. It exposes, to the agent that asked for a
grant, whether and how the human-facing consent prompt for *that* petition is
progressing — queued, shown, or gone — and nothing else.

Scoping is structural rather than checked. The object is minted by
[`vitrin_realm.request_grant`](./03-vitrin_realm.md) in the same request that
mints the grant and the three authority facets, so an agent can only name — and
therefore can only observe — consent activity for a petition it itself made. No
message anywhere in the protocol hands one connection a consent object minted
on another petition, so there is no cross-petition prompt visibility to leak.

The interface deliberately carries **no requests**. Nothing an agent sends can
put content in front of the human, and everything the prompt renders is
core-validated data: the verifier-canonical identity from
[`vitrin_principal.bound`](./02-vitrin_principal.md), and the parsed verbs,
realm, and expiry the core derived from the petition — never free client text.
Consent decisions are not protocol-expressible by design: the scripted-consent
injector used by integration tests is a build-gated core hook, not a wire
message.

The authoritative decision never arrives here. It arrives as
[`vitrin_grant.resolved`](./04-vitrin_grant.md), which fires exactly once per
grant. `vitrin_consent` is therefore advisory: a threadless blocking client MAY
ignore consent events entirely and simply read until `resolved`. The events
exist for a supervising agent UX that wants to know a prompt is up (for
instance, to explain a stall to its own operator), not for the decision logic.

## Lifecycle

An instance comes into existence only as one of the five `new_id` arguments of
`vitrin_realm.request_grant` (grant, consent, view, pointer, text), created in
attenuation order under the multi-new_id rule described in
[conventions](./00-conventions.md). It is bound to the single petition that
request opened.

The object carries no requests and no destructor. Version 1 defines no
destructors at all: the object lives for the connection. Its *useful* lifetime
ends when the petition resolves — every `state` transition is delivered before
the grant's `resolved` event, so once `resolved` has fired no further `state`
events are emitted for this petition. Because object ids are never reused, the
id remains permanently bound to this dead petition and is safe to retain or
discard.

A pending petition is withdrawn if the connection closes (consent is
in-context: the prompt disappears with the petitioner), and the connection's
death takes this object with it like every other.

Per the inert-id tolerance rule in [conventions](./00-conventions.md), a client
MUST tolerate and discard any late or unexpected event referencing an id whose
petition it considers finished; this is always safe because ids are never
reused.

## Events

### state

`state(state: consent_state)`

| arg | type | description |
|---|---|---|
| `state` | `uint` (enum [`consent_state`](#consent_state)) | the new prompt state: `queued`, `shown`, or `closed` |

Reports one transition in the consent prompt's visibility. The server emits
**zero or more** `state` events for a petition, and **all of them are delivered
before** that petition's [`vitrin_grant.resolved`](./04-vitrin_grant.md) event.
The three transitions carry these meanings:

- `queued` — the prompt is waiting behind another prompt or a policy decision;
  the agent should keep waiting.
- `shown` — the prompt is visible. All physical input now routes exclusively to
  it (the input grab), and the agent knows to keep waiting. While its own prompt
  is shown, that principal's actuation is refused
  [`vitrin_grant.refused(_, consent_held)`](./04-vitrin_grant.md) and never
  delivered to the app until the prompt closes; other principals' grants are
  unaffected.
- `closed` — the prompt is gone; the authoritative decision follows on the
  grant as `resolved`.

The consent overlay is composited into human-visible output only. It never
appears in captured frames delivered by
[`vitrin_view.frame_ready`](./06-vitrin_view.md), so agents cannot watch
prompts even with an active observe grant.

Under an auto-approve consent policy — a headless/CI mode that is explicitly
flagged and loudly logged — the server MAY emit only `closed`, or no `state`
events at all, before `resolved`. A correct client therefore MUST NOT treat any
particular `state` transition (including `shown` or `closed`) as guaranteed to
arrive, and MUST rely on `vitrin_grant.resolved` for the outcome.

**Delivery class:** pure server-pushed event. It is not the terminal of any
reply-bearing request (the petition's terminal is `vitrin_grant.resolved`) and
carries no cookie or correlation id — pairing to the petition is entirely by
object identity. `state` events are neither coalesced with nor substitutable
for the grant terminal.

## Enums

### consent_state

Prompt visibility states. Not a bitfield.

| entry | value | meaning |
|---|---|---|
| `queued` | 0 | waiting behind another prompt or a policy decision |
| `shown` | 1 | visible; physical input is grabbed by the prompt |
| `closed` | 2 | gone; the decision arrives on the grant |

This enum is local to `vitrin_consent` and is not referenced by any other
interface.

## Flows

Direction key: `A→C` agent→core (request), `C→A` core→agent (event). Sequences
are corrected for the final XML: the view/pointer/text facets and the consent
observer are all co-minted by `request_grant`; the authoritative outcome is
always `vitrin_grant.resolved`. Steps not touching `vitrin_consent` are
abbreviated.

### Flow 1 — Consent approved (M1.4 demo, scenario b)

1. `A→C` `vitrin_realm.request_grant(grant, consent, view, pointer, text, resource=null, verbs=observe|actuate_pointer|actuate_text, …)` — mints all five objects; the grant is born pending and the facets inert.
2. `C→A` `vitrin_consent.state(queued)` — *optional; may be skipped if no prompt is ahead.*
3. `C→A` `vitrin_consent.state(shown)` — the prompt is visible; physical input is grabbed; A's own actuation would now refuse `consent_held`.
4. *(out of band)* the human clicks "Allow-while-running".
5. `C→A` `vitrin_consent.state(closed)` — the prompt is gone.
6. `C→A` `vitrin_grant.resolved(granted, verbs=…, persistence=while_running, expiry_ms=…)` — the authoritative decision, carrying the effective authority the human chose.

### Flow 2 — Consent denied (scenario c)

1. `A→C` `vitrin_realm.request_grant(…)`.
2. `C→A` `vitrin_consent.state(shown)`.
3. *(out of band)* the human clicks "Deny".
4. `C→A` `vitrin_consent.state(closed)`.
5. `C→A` `vitrin_grant.resolved(denied, 0, 0, 0)` — a clean event, never a hang and never a connection death.

### Flow 3 — Consent timeout (scenario c, timeout variant)

1. `A→C` `vitrin_realm.request_grant(…)`.
2. `C→A` `vitrin_consent.state(shown)`.
3. *(out of band)* the deadline passes with no human action.
4. `C→A` `vitrin_consent.state(closed)`.
5. `C→A` `vitrin_grant.resolved(timed_out, 0, 0, 0)` — petitioning again later is legal.

### Flow 4 — Auto-approve (walking skeleton, scenario a)

1. `A→C` `vitrin_realm.request_grant(…, verbs=observe, …)`.
2. `C→A` `vitrin_consent.state(closed)` — *may be omitted entirely under the explicitly-flagged, loudly-logged auto-approve policy.*
3. `C→A` `vitrin_grant.resolved(granted, verbs=observe, persistence=while_running, expiry_ms=…)`.

## Growth

Every seam below is purely additive under the versioning rules in
[conventions](./00-conventions.md): appended messages and appended enum entries
only; existing signatures and enum values are immutable.

- **More `consent_state` entries.** The `consent_state` enum may gain entries in
  a later version (for example an intermediate state distinguishing a
  policy-only decision from a rendered prompt). Existing values stay fixed, and
  clients already discard states they do not recognize because the outcome is
  authoritative on the grant.
- **Physically-originated consent.** Later phases build human-anchored consent
  on the `origin` tagging carried by [`vitrin_shim_seat`](./11-vitrin_shim_seat.md)
  events. Any wire surface that adds is expected to arrive as new, since-gated
  messages, keeping `vitrin_consent`'s events-only, request-free shape intact —
  the property that guarantees an agent can never inject prompt content remains
  structural.
