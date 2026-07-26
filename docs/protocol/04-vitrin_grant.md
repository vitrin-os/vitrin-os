# vitrin_grant — capability handle and enforcement voice

**Interface version:** 1 · **Connection class:** principal · **Messages:** 0 requests + 2 events

## Purpose

`vitrin_grant` is the wire projection of a single grant-table row: one
principal's authority over one resource, expressed as a verb set plus
constraints (expiry, rate ceiling, persistence rung). It is the object an
agent holds to represent "I have been granted authority here", and it is the
single voice of the enforcement chokepoint — every capture and every actuation
performed under this grant is checked by one server-side function, and every
refusal that function produces is delivered on this object.

The grant sits at the center of the connection's authority chain. It is minted
— together with its [consent observer](05-vitrin_consent.md) and its three
authority facets ([view](06-vitrin_view.md),
[pointer](07-vitrin_actuator_pointer.md),
[text](08-vitrin_actuator_text.md)) — by
[`vitrin_realm.request_grant`](03-vitrin_realm.md). The facets carry the
verbs; the grant carries the row's identity and lifecycle. A facet confers
nothing on its own: it is checked at use time against *this* grant's effective
verb set, and a use outside that set is refused here.

The design idea is one chokepoint, one refusal voice. Rather than scatter
authority checks across the observation and actuation interfaces — each with
its own error vocabulary — the protocol routes every use-time failure through
`vitrin_grant.refused`, and every petition-time outcome through
`vitrin_grant.resolved`. This keeps the enforcement surface auditable (one
function to fuzz, one event to test) and gives the SDK a single place to map
wire codes onto typed exceptions.

The interface is events-only in version 1: it defines no requests. The grant
handle is not something the agent commands; it is something the agent observes.
Authority is exercised through the facets, and the grant merely reports how the
petition resolved and, thereafter, why any use was refused.

## Lifecycle

A grant is born **pending** by `vitrin_realm.request_grant`, conferring
nothing. Its birth follows the [multi-`new_id` rule](00-conventions.md): it is
one of five `new_id` arguments minted in that single request, all distinct,
strictly increasing in argument order, and above the connection's allocation
watermark.

Exactly one [`resolved`](#resolved) event ever decides the grant's fate. On
outcome `granted` the grant becomes active and its facets confer their verbs;
on any other outcome the grant is dead from birth and its facets never confer
anything. This event fires **exactly once per grant, ever** — denial, timeout,
and every other terminal outcome are clean distinct events, never a hang and
never a connection death.

A grant that resolved `granted` later goes **dead** when its expiry passes or
it is revoked (by hold-Esc, a panel action, or policy). A dead grant's facets
go **inert**: requests on them are refused *recoverably* via
[`refused`](#refused), never fatally. This is the taxonomy corollary described
on the [conventions page](00-conventions.md) — human revocation racing an
in-flight request must not kill a well-behaved agent, so a dead grant yields
`refused`, never `invalid_object`. Because object ids are never reused, the
server MAY continue to emit events referencing a dead grant's objects; clients
MUST tolerate and discard them.

Version 1 has **no grant persistence**: every grant dies with the connection,
and there are no restore tokens (a future version's addition). A pending
petition is withdrawn if the connection closes — consent is in-context, so the
prompt disappears with the petitioner. Version 1 defines **no destructors** on
this interface; see [Growth](#growth).

## Events

### resolved

`resolved(outcome: uint, verbs: uint, persistence: uint, expiry_ms: uint)`

| arg | type | description |
|---|---|---|
| `outcome` | uint (enum [`outcome`](#outcome)) | how the petition resolved |
| `verbs` | uint (enum [`verb`](#verb), bitfield) | effective verb set; 0 unless `outcome` is `granted` |
| `persistence` | uint (enum [`persistence`](#persistence)) | effective persistence rung; `once` (0) unless granted |
| `expiry_ms` | uint | effective lifetime in milliseconds; 0 = bounded by the persistence rung |

The terminal event of the pending phase, sent after the consent decision (or
policy decision) completes. It is sent **exactly once per grant, ever**.

On outcome `granted`, the trailing arguments carry the **effective** authority
— the verb set, persistence rung, and expiry the human actually chose. These
MAY be narrower than what `request_grant` petitioned for: an "Allow once"
button versus "Allow while running" changes the lifetime, and the human may
approve fewer verbs than were requested. The agent MUST treat these effective
values, not its request, as ground truth for what the grant confers.

On any outcome other than `granted`, the trailing arguments are zero-filled
(`verbs` = 0, `persistence` = `once`, `expiry_ms` = 0) and carry no meaning.

The effective `max_event_rate` is deliberately **not** echoed here: an agent
discovers throttling operationally, through
[`refused(rate_limited)`](#refused) and its `retry_after_ms` hint.

**Delivery class:** this is the terminal event of the reply-bearing
`request_grant` and is never coalesced — but unlike other reply-bearing
terminals it is **exempt from the cross-request "in request order" rule**: it
waits on an unbounded human consent delay, is sequenced only within its own
petition's lifecycle (zero or more `vitrin_consent.state` events, then exactly
one `resolved`), and a later `sync`'s `done` does **not** wait for it. Every
outcome — including `denied`, `timed_out`, `unavailable`, `unsupported`, and
`busy` — is a normal recoverable answer, not a protocol violation: a denial is
an answer, not an error. The blocking SDK sends `request_grant`, then reads
until `resolved`, mapping the outcome onto success or a typed exception.

### refused

`refused(verb: uint, code: uint, retry_after_ms: uint)`

| arg | type | description |
|---|---|---|
| `verb` | uint (enum [`verb`](#verb)) | the verb whose use was refused; also identifies the facet |
| `code` | uint (enum [`refusal`](#refusal)) | why the use was refused |
| `retry_after_ms` | uint | refill hint in milliseconds; greater than zero only for `rate_limited`, otherwise 0 |

The single recoverable-error event for grant *use*, covering capture and
actuation alike — one chokepoint, one refusal voice. `verb` names the verb
whose use was refused, which also identifies the facet that issued the offending
request (`observe` → [view](06-vitrin_view.md), `actuate_pointer` →
[pointer](07-vitrin_actuator_pointer.md), `actuate_text` →
[text](08-vitrin_actuator_text.md)).

**Delivery class depends on the refused request.** For the reply-bearing
[`vitrin_view.capture_frame`](06-vitrin_view.md), this event is that capture's
terminal: exactly one of `vitrin_view.frame_ready` or
`refused(observe, …)` per capture, in request order, **never coalesced**. The
type system forces this pairing — an `fd` argument has no null form, so a
failed capture cannot be a `frame_ready` with an absent fd; it must be a
distinct event.

For the fire-and-forget actuation requests (`move`, `button`, `scroll`,
`type`), refusals **MAY be coalesced** per the [delivery
classification](00-conventions.md): at most one `refused(rate_limited)` per
grant per bucket-refill window, and at most one `refused` per grant per
(verb, code) pair until a subsequent request on that grant succeeds. A
threadless client bounds its refusal discovery to one round trip by issuing
[`vitrin_handshake.sync`](01-vitrin_handshake.md) after a batch of actuations
and reading until `done`, raising on any `refused` seen.

`retry_after_ms` is greater than zero **only** for `rate_limited`, where it
hints when the token bucket refills; for every other code it is 0.

Because `refused` is a use-time event, it may reference a facet whose grant has
already gone dead. This is expected: `refused` remains the enforcement-bearing
signal even for expired and revoked grants (`expired`, `revoked` codes), and
it never escalates to a fatal error.

## Enums

### verb

Bitfield. The grantable verbs. Every entry has one SDK-level dotted name,
formed by replacing the first underscore of the wire name with a dot:
`observe`, `actuate.pointer`, `actuate.text`, `observe.cursor`,
`layout.arrange`, `layout.focus`. The spelling is fixed by the IDL so a second
implementation transcribing this enum has no name to invent.

| entry | value | served in version 1 | meaning |
|---|---|---|---|
| `observe` | 0x1 | yes | capture frames of the granted resource |
| `actuate_pointer` | 0x2 | yes | inject pointer motion, buttons, and scroll |
| `actuate_text` | 0x4 | yes | inject Unicode text |
| `observe_cursor` | 0x8 | **no** — resolves `unsupported` | capture frames that include the human principal's cursor; meaningful only alongside `observe` |
| `layout_arrange` | 0x10 | **no** — resolves `unsupported` | place, resize, raise, and fullscreen the granted realm's views |
| `layout_focus` | 0x20 | **no** — resolves `unsupported` | direct keyboard focus to a view of the granted realm |

This enum is the type of `request_grant`'s `verbs` argument, of
`resolved.verbs`, and of `refused.verb`. The three version-1 verbs map
one-to-one to a facet interface and to that interface's `@verb` annotation,
which drives the scanner-generated chokepoint table. Later phases append
entries (for example key actuation, credential presentation, subtree reads)
without touching existing bits; values are immutable.

#### Defined but unserved

`observe_cursor`, `layout_arrange`, and `layout_focus` are defined on the wire
from day one and **refused `unsupported`** by version 1 — the same posture the
[`persistence`](#persistence) ladder takes toward its durable rungs. Two
reasons, both structural:

1. **An out-of-range bit is fatal.** A bitfield argument is validated as a
   mask, so a bit outside the defined union is `invalid_argument` and the
   connection dies (see [conventions § error taxonomy](00-conventions.md)). A
   client petitioning for authority this deployment does not serve would be
   killed rather than answered. Defining the bit converts that into a
   recoverable `resolved(unsupported)`.
2. **The model would otherwise be unstateable.** Decisions D-017 and D-018
   settle that cursor visibility is *authority* rather than a display
   preference, and that scene arrangement is a *grant* rather than the shell's
   ambient property. Neither is expressible without a verb, and adding one
   after v0 freezes is a version bump against deployed clients.

Two rules hold for every unserved verb:

- **A deployment MUST NOT grant a verb it does not enforce.** `unsupported` is
  the honest answer; accepting the bit and enforcing nothing is the failure
  this section exists to prevent.
- **A mixed petition is refused whole.** `verbs = observe|layout_arrange`
  resolves `unsupported`; the server does not quietly drop the unserved bit and
  grant `observe`. Narrowing a verb set is the human's move at consent time —
  a silent server-side edit would leave the agent believing it holds authority
  nobody checks.

Not every verb has a facet interface. `observe_cursor` has none by
construction: it widens what
[`vitrin_view.capture_frame`](06-vitrin_view.md) composites rather than adding
a request. The layout verbs' facet arrives as a `since`-gated mint on *this*
interface, because `request_grant`'s five `new_id` arguments are frozen forever
(see [Growth](#growth)).

#### Verb composition

One dependency exists, and it is the only one. **`observe_cursor` is meaningful
only alongside `observe`.** It widens what a capture *contains*, so a petition
naming it without `observe` names no capture to widen; such a petition resolves
`unsupported` rather than granting a bit that changes nothing. That follows from
the rule directly above — a deployment MUST NOT grant a verb it does not
enforce — and it settles the case the wire would otherwise leave open:
`observe_cursor` is **not** an independent authority and is never
inert-but-held. Every other verb (`observe`, `actuate_pointer`, `actuate_text`,
`layout_arrange`, `layout_focus`) is independently petitionable. Version 1
refuses `observe_cursor` in any combination, so the rule is not yet
distinguishable from the blanket unserved refusal; it is stated now because the
enum entry is frozen now.

#### What no grant can purchase

Layout being grant-governed is only half the answer; the other half is what a
layout grant can never buy, however permissive the consent decision. The core
enforces these **ordering invariants** unconditionally:

- the consent surface and the trust indicator composite **above** every
  principal's content;
- the core's own hit test — never a client's claimed stacking — decides which
  surface an input event reaches;
- no arrangement may occlude, fullscreen over, or resize away the consent
  surface;
- no **agent** principal's cursor is composited into another principal's
  captured frame — the one cursor a capture may ever contain is the human
  principal's, and only for a grant holding `observe_cursor`.

**What "unconditionally" means today.** The first invariant holds and is
exercised (the overlay composites at the output stage, above the scene a
capture is taken from; `backend/headless.rs`'s
`a_prompt_reaches_human_visible_output_but_never_a_capture` asserts it). The
second and fourth hold **vacuously** — no client can state a stacking order and
the core composites no cursor at all. The third has nothing to be true of, there
being no arrangement mechanism. **None of the four is tested *as an invariant***
against a client trying to violate it, and none can be until something outside
the core can arrange realms; that test belongs to the mission-control shell
(E3), and D-018 is the reason it must exist.

The split is deliberate: a shell gets *arrangement*, the core keeps *ordering*.
This is what lets window-management policy live outside the TCB (PRD §5.1)
without making the shell trusted. See [conventions § 1.4](00-conventions.md)
for the full statement.

### persistence

The consent persistence ladder. The full ladder is defined from day one so the
wire never changes shape when durable rungs arrive.

| entry | value | meaning |
|---|---|---|
| `once` | 0 | single-use authority |
| `while_running` | 1 | lives while the requesting principal's connection lives |
| `until_revoked` | 2 | durable until explicitly revoked (requires verified provenance; refused in version 1) |
| `always` | 3 | durable and auto-reissued (requires verified provenance; refused in version 1) |

Version 1 accepts only `once` and `while_running`. A petition for a durable
rung (`until_revoked` or `always`) resolves [`unsupported`](#outcome), because
durable authority is valid only with verified provenance — a later phase. This
enum types both `request_grant`'s `persistence` argument and
`resolved.persistence`.

### outcome

Petition-time results. Each maps to a distinct typed SDK error (or success).
All outcomes are recoverable: a denial is an answer, not a protocol violation.

| entry | value | meaning |
|---|---|---|
| `granted` | 0 | authority active; the `resolved` event carries the effective verbs, rung, and expiry |
| `denied` | 1 | the human said no |
| `timed_out` | 2 | the consent prompt expired unanswered; petitioning again later is legal |
| `unavailable` | 3 | the realm was unknown, vacant, or closed while the petition was pending |
| `unsupported` | 4 | well-formed but refused by policy: durable rung without provenance, reserved flag set, unserved resource prefix, or a defined verb this deployment does not serve (an *out-of-range* verb bit is instead fatal `invalid_argument`) |
| `busy` | 5 | the pending-petition admission cap for this verified identity (across all of its connections) was reached |

This enum types `resolved.outcome`.

### refusal

Use-time refusal codes, emitted by the enforcement chokepoint on every refused
capture or actuation. Each code maps to a distinct typed SDK exception
(NotGranted, GrantExpired, Revoked, RateLimited, Preempted, ConsentHeld,
NoSurface, OperationFailed).

| entry | value | meaning |
|---|---|---|
| `not_granted` | 0 | the grant is not (or not yet) active, or the verb is outside its effective set: use while pending, through an ungranted facet, or after any non-`granted` resolution (`denied`, `timed_out`, `unavailable`, `unsupported`, `busy`) |
| `expired` | 1 | the grant's expiry passed; checked on use and by a proactive timer |
| `revoked` | 2 | revoked by hold-Esc, panel, or policy; effective on the very next request |
| `rate_limited` | 3 | the token bucket is empty; `retry_after_ms` hints the refill |
| `preempted` | 4 | physical human input owns the target right now |
| `consent_held` | 5 | the principal's **own** pending petition has a prompt up; that principal's actuation is refused (never delivered to the app) until the prompt closes — other principals' grants are unaffected |
| `no_surface` | 6 | the realm has no surface (its shim crashed or exited); never a stale frame |
| `internal` | 7 | server-side failure during this use (renderer, memfd, delivery) |

This enum types `refused.code`.

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent. These are the
[canonical scenarios](00-conventions.md) that touch `vitrin_grant`; steps not
involving this interface are elided. Facet objects (`view`, `pointer`, `text`)
are co-minted by `request_grant`, so no separate getter step appears.

### 1. Grant success (walking skeleton)

```
1. A→C  vitrin_realm.request_grant(grant, consent, view, pointer, text,
                                   resource=null, verbs=observe, …)
2. C→A  vitrin_consent.state(closed)          [auto-approve; loudly logged]
3. C→A  vitrin_grant.resolved(granted, verbs=observe,
                              persistence=while_running, expiry_ms)
4. A→C  vitrin_view.capture_frame()
5. C→A  vitrin_view.frame_ready(fd, format=xrgb8888, …)
```

The grant confers `observe` only after step 3; a `capture_frame` sent before
`resolved` would be refused `not_granted`.

### 2. Consent approve, actuate, capture (M1.4 demo)

```
1. A→C  vitrin_realm.request_grant(grant, consent, view, pointer, text,
                                   verbs=observe|actuate_pointer|actuate_text, …)
2. C→A  vitrin_consent.state(shown)           [human sees the prompt]
3. C→A  vitrin_consent.state(closed)
4. C→A  vitrin_grant.resolved(granted, verbs=observe|actuate_pointer|actuate_text,
                              persistence=while_running, expiry_ms)
5. A→C  vitrin_view.capture_frame() → C→A frame_ready(…)
6. A→C  vitrin_actuator_pointer.move / button …   (fire-and-forget)
7. A→C  vitrin_actuator_text.type("http://…/\n")   (fire-and-forget)
8. A→C  vitrin_handshake.sync(cookie) → C→A done(cookie)   [flush refusals]
9. A→C  vitrin_view.capture_frame() → C→A frame_ready(…)   [assert change]
```

### 3. Denial and timeout

```
Denial:   … prompt shown …
          C→A  vitrin_consent.state(closed)
          C→A  vitrin_grant.resolved(denied, 0, once, 0)

Timeout:  … prompt shown, no human action before the deadline …
          C→A  vitrin_consent.state(closed)
          C→A  vitrin_grant.resolved(timed_out, 0, once, 0)
```

Both terminate the SDK's `await`, one raising GrantDenied and the other
ConsentTimeout. Petitioning again after a timeout is legal.

### 4. Revocation mid-loop (hold-Esc)

```
1. [agent in observe→actuate loop under an active grant]
2. [human holds Esc; core revokes the grant → the grant goes dead]
3. A→C  vitrin_actuator_pointer.move(x, y)
4. C→A  vitrin_grant.refused(actuate_pointer, revoked, 0)
5. A→C  vitrin_view.capture_frame()
6. C→A  vitrin_grant.refused(observe, revoked, 0)
```

Both the actuation path and the capture path fail with `revoked` — one code,
two verbs, one chokepoint. With `sync` the agent observes the refusal within
one round trip.

### 5. Expiry

```
1. [grant issued with a bounded expiry; the deadline passes]
2. A→C  vitrin_view.capture_frame()
3. C→A  vitrin_grant.refused(observe, expired, 0)
4. A→C  vitrin_actuator_text.type("late")
5. C→A  vitrin_grant.refused(actuate_text, expired, 0)
```

### 6. Rate-limit hit

```
1. A→C  vitrin_view.capture_frame() ×N within the window
2. C→A  vitrin_view.frame_ready(…) for those the bucket admits
3. A→C  vitrin_view.capture_frame() (over ceiling)
4. C→A  vitrin_grant.refused(observe, rate_limited, retry_after_ms>0)
```

Capture refusals are the terminal of a reply-bearing request and are **never
coalesced** — one `refused` per over-ceiling capture. An actuation flood over
the ceiling produces `refused(verb, rate_limited)` that **MAY be coalesced** to
one per bucket-refill window.

## Growth

The following seams are named in the XML description as purely additive
version-2+ extensions. Each is a new message or enum entry — never a change to
a version-1 signature, whose immutability the [versioning
rules](00-conventions.md) guarantee.

- **`release` destructor.** A request to withdraw a pending petition or
  relinquish authority early, carrying the tombstone rule that clients discard
  events referencing released ids. Version 1 has no destructors; the
  server-side consent timeout (`resolved(timed_out)`) already bounds the
  lingering-prompt harm, so `release` is deferred.
- **`revoked` push event.** An asynchronous event announcing revocation before
  the next use. It is a *different* event from `resolved`, which still fires
  exactly once ever; `refused(revoked)` remains the enforcement-bearing signal,
  so there is no status double-fire.
- **Attenuation.** A request minting narrower child grants from an existing
  grant.
- **Restore tokens.** Durable grant persistence (the `until_revoked` and
  `always` rungs, already present in the [`persistence`](#persistence) enum)
  becomes usable once provenance verification lands.
- **Epoch-staleness refusal sibling.** A new event carrying the current epoch
  for compare-and-swap actuation, because `retry_after_ms` cannot express an
  epoch. It pairs with clamped-coordinate stale-observation detection.
- **New verbs.** Appended [`verb`](#verb) bits (for example key actuation,
  credential presentation, subtree reads) extend the bitfield without touching
  existing bits.
- **Layout facet mint.** The `layout_arrange` and `layout_focus` verbs need a
  facet to be exercised through, and `request_grant`'s five `new_id` arguments
  are frozen. It therefore arrives as a `since`-gated **structural mint on this
  interface** (`get_layout(new_id)`), following the same mint-freely,
  check-at-use pattern as the co-minted facets. Version 1 refuses both verbs
  `unsupported`, so there is nothing to mint yet.
