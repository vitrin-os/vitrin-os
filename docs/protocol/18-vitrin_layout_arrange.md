# vitrin_layout_arrange — the arrangement facet

**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** principal · **Messages:** 1 request · **`@verb`:** `layout_arrange`

## Purpose

`vitrin_layout_arrange` is the capability object through which the
[`layout_arrange`](04-vitrin_grant.md#verb) verb is exercised: it chooses
whether the granted realm's view **fills the output** or is composited at the
size **its own app chose**. It is minted by
[`vitrin_grant.get_layout_arrange`](04-vitrin_grant.md#get_layout_arrange) and
is born **inert**, on exactly the terms
[`vitrin_layout_focus`](17-vitrin_layout_focus.md) is.

## One request, and the absences are the design

There is no `place`, no `resize`, no `raise`, no `lower`, no `close` and no
stacking request on this interface — **not a request that refuses, but no
request at all**.

The scene shows one realm at a time, unstacked and unoverlapped, at one output
size. A `place` or `raise` request would have to either widen that scene or
accept an argument it then ignored, and a verb that silently does less than
its name is worse than one that has no request: a client that sends `raise`
and hears nothing has been told its authority worked.

Every one of those absences is therefore a statement about what a granted
`layout_arrange` can and cannot do. A deployment that adds a request here
before its scene can honour it breaks the IDL's own rule that **a deployment
MUST NOT grant a verb it does not enforce**.

Growth arrives as `since`-gated sibling requests when the scene can honour
them, exactly as every other interface grows, and never by changing
`set_fullscreen`'s meaning.

## At most one holder per output

A petition for `layout_arrange` while the verb is already **spoken for**
resolves [`layout_held`](04-vitrin_grant.md#outcome) **at admission** — it
never reaches a human, so it costs the human nothing.

Spoken for means either of two things, and the second is easy to miss:

- a **live grant** carrying the verb, or
- **another petition still pending** for it.

The pending half is deliberate. Were only live grants counted, two petitions
could both be admitted while both waited, a human could approve both, and the
session would hold the two arrangers this rule exists to make impossible —
with no answer left to give the second, because it has already been granted.
So the slot is taken from admission and released when the pending petition
resolves to anything other than `granted`. The cost, stated: a principal whose
petition is in front of a human locks out a second principal for as long as
that human takes to answer.

Arbitrating between two would-be arrangers is window-management *policy*,
which the core refuses to do rather than choosing a rule that would then be
the core's rule. `layout_held` is the only thing the core says about
contention. Retrying once the holder's grant expires, is revoked, or its
connection ends — or once the pending petition resolves non-`granted` — is
legal.

The check is against **any** holder, not against another *principal*'s:
scoping it per principal would let one agent hold N arrangement grants and
defeat the rule by fragmenting itself, leaving the core arbitrating between
the fragments.

[`layout_focus`](17-vitrin_layout_focus.md) carries no such restriction. Focus
is a momentary act rather than a standing arrangement, and several principals
may hold it at once.

## What this facet cannot reach

The consent surface and the trust indicator are **not part of any realm's
view**, and no arrangement moves, resizes, occludes or fullscreens over them.
That is [ordering invariant 3](00-conventions.md#14-scene-authority-arrangement-ordering-cursors),
so it is not a check this interface performs on the way past — there is no
request through which the attempt is even expressible, and the overlay
composites at a stage downstream of the scene an arrangement selects.

## Served status

Whether `layout_arrange` is served is a property of a **deployment**, not of
the wire. The reference core serves it as of WS-E.1.4; a deployment that does
not refuses the petition `unsupported` at admission. See
[§ defined but unserved](04-vitrin_grant.md#defined-but-unserved).

This interface's messages are `since="2"`, so they do not exist on a
version-1 connection at all; sending one there is fatal `invalid_opcode`. The
verb bit is *not* version-gated, so a version-1 client may name
`layout_arrange` in a petition and is answered `unsupported` (or
`layout_held`).

## Lifecycle

Identical to [`vitrin_layout_focus`](17-vitrin_layout_focus.md#lifecycle):
minted by a structural mint on the grant, always legal, born inert, inert
again when the grant dies, no destructor, duplicates permitted and conferring
nothing extra.

**A separate mint from `get_layout_focus`, deliberately.** One facet interface
declares exactly one grant verb — that is what generates the single-site
authority check — and the two layout verbs must stay independently
attenuable. A shell granted arrangement but not focus holds a live
`vitrin_layout_arrange` and a `vitrin_layout_focus` that refuses `not_granted`
forever, which is exactly the shape that separation is for.

## Requests

### set_fullscreen

`set_fullscreen(mode: uint /* enum mode */)` — **since version 2**

| arg | type | description |
|---|---|---|
| `mode` | `uint` → [`mode`](#mode) | `windowed` (0) or `fullscreen` (1) |

No realm argument, for the same reason
[`focus`](17-vitrin_layout_focus.md#focus) has none: the grant names the realm.

**Delivery class:** **fire-and-forget**. No terminal event; refusals MAY be
coalesced per the [delivery
classification](00-conventions.md#6-delivery-classification). Rate-limited by
the grant's `max_event_rate` like every other use.

### What the two modes mean, precisely

The difference is a **size**, not a decoration.

- **`fullscreen`** means the realm's view size **tracks the output's**. The
  core sends [`vitrin_shim_session.configure`](09-vitrin_shim_session.md) with
  the output's size on entering the mode, and again whenever the output
  resizes while the realm is in it, so the app fills the output edge to edge.
- **`windowed`** means it does **not**. The core imposes no size at all, sends
  nothing, the realm's view keeps whatever size it already had, and the
  compositor letterboxes that buffer centered and unscaled inside the output.

Re-sending `configure` is a documented permission of that event ("May be
re-sent when the view resizes"), not a new mechanism.

### The core never invents a window size

That is why "windowed" is an **absence** rather than a second size. A core
that chose one would be choosing where a window goes and how big it is, which
is window-management policy and belongs outside the core (PRD §5.1's permanent
invariant). The only two sizes in the system are the output's and the one the
realm already has, and these two modes are exactly the choice between them.

### A consequence worth stating plainly

While the output's size and the realm's size are equal, **the two modes are
indistinguishable** and switching between them changes nothing an observer can
see. They diverge the moment the output resizes under a windowed realm, and
converge again the moment that realm is fullscreened.

A client must not read a mode change as a guarantee that anything moved.
Setting the mode a realm is already in is legal and is a no-op.

### Arranging a hidden realm

This request is defined for the granted realm **whether or not that realm
holds the output**. A realm arranged while hidden takes the arrangement with
it when it is next focused; nothing here reads or changes the binding, which
is [`layout_focus`](17-vitrin_layout_focus.md)'s alone.

### Failure modes

*Fatal (the connection dies).* Sending this opcode on a version-1 connection
is `invalid_opcode`, as is any opcode this interface does not define. A
`mode` value outside the enum is `invalid_argument` — plain enums decode by
whole-value membership, and an out-of-range value is grammar the client could
have known, which is the [error razor](00-conventions.md#5-the-error-razor)'s
own test. A frame whose declared size or fd count disagrees with the signature
is `oversized` or `fd_violation`.

*Recoverable (an event is delivered, the connection lives).* Every one of
these arrives as `vitrin_grant.refused(layout_arrange, code, retry_after_ms)`,
and the set is exactly
[`focus`](17-vitrin_layout_focus.md#failure-modes)'s, for the same reasons:

| code | when |
|---|---|
| `not_granted` | the grant never held `layout_arrange`, or has not resolved `granted` |
| `expired` | the grant's expiry passed |
| `revoked` | the grant was revoked |
| `rate_limited` | the grant's token bucket is empty; `retry_after_ms` > 0 |
| `no_surface` | the granted realm has no live view |
| `preempted` | physical human input owns the target right now — **conditional**, see below |
| `consent_held` | this principal's own consent prompt is up |
| `internal` | a server-side failure carrying out the arrangement |

`no_surface` here means what it says: an app that has committed nothing has
neither an own size to return to nor a buffer to fill the output with.

**`preempted` is conditional on exactly the terms
[`focus`](17-vitrin_layout_focus.md#failure-modes)'s is**: while the human's
attention signal ([`vitrin_principal.attention`](02-vitrin_principal.md#attention),
version 2) is live for a principal, that principal's `set_fullscreen` is not
refused `preempted`. The signal lifts it for **both** layout verbs or for
neither — `fullscreen on<Enter>` hits the identical loop that
`focus editor<Enter>` does, and the harm profiles argue *for* including
arrangement rather than against: a stolen focus moves the human's keystrokes
into another realm, while a stolen `set_fullscreen` resizes the view and cannot
stack, occlude the consent surface, or move anything.

## Enums

### mode

A closed pair rather than a boolean, so that a scene able to express a third
arrangement appends an entry here instead of needing a second request. Values
are immutable and entries append, like every other enum; an out-of-range value
is fatal `invalid_argument`.

| entry | value | meaning |
|---|---|---|
| `windowed` | 0 | compose the realm's view at the size its own app last committed, letterboxed centered and unscaled inside the output |
| `fullscreen` | 1 | configure the realm's view to the output's size, so the app fills the output |

## Growth

Documented seams, each additive and each gated on the scene being able to
honour it:

- **`place`, `raise`, stacking** — only once the scene composites more than
  one realm at a time. Adding one before that is the failure this page's
  first section is about.
- **per-output arrangement**, when a deployment has more than one output.

## See also

- [`vitrin_layout_focus`](17-vitrin_layout_focus.md) — the other half of
  layout
- [`vitrin_grant.get_layout_arrange`](04-vitrin_grant.md#get_layout_arrange) —
  the mint
- [`vitrin_grant.outcome`](04-vitrin_grant.md#outcome) — `layout_held`
- [`vitrin_shim_session.configure`](09-vitrin_shim_session.md) — the event
  `fullscreen` re-sends
