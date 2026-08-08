# vitrin_realm — the authority-free realm address

**Interface version:** 1 · **Connection class:** principal · **Messages:** 1 request + 0 events

## Purpose

`vitrin_realm` is an addressing scope for petitions. It answers exactly one
question — *which realm are you asking about* — and confers no authority of its
own. Holding a realm handle lets a principal **petition** for a grant against
that realm; it does not observe, actuate, or read anything. Naming is not
authority: minting the handle is always structurally successful (see
[`vitrin_principal.get_realm`](./02-vitrin_principal.md)), and a handle whose
realm is unknown, vacant, or closed simply produces petitions that resolve
`unavailable` rather than a protocol error.

In the object graph the realm handle sits one hop below the principal: a
[`vitrin_principal`](./02-vitrin_principal.md) mints realm handles by name, and
each realm handle in turn mints grant petitions. Grants attach to realms, and
apps launch into realms, so the realm is the join point between the authority
chain (principal → grant → facets) and the composition model (realm view → shim
surface).

### Realm cardinality: one at version 1, a bounded set at version 2

Version 1 fixes the count as well as the name: **exactly one realm, `realm-0`.**

Version 2 lifts the count to a **deployment-chosen limit** and keeps `realm-0`
**mandatory**. A version-2 server serves `realm-0` and however many further
realms its operator configured or [`vitrin_launcher`](./16-vitrin_launcher.md)
created, up to that limit; a deployment already at the limit refuses a *launch*
`capacity`, and never refuses `get_realm`, which mints a handle for any name at
all.

Keeping `realm-0` mandatory is what makes the widening **additive** rather than
merely compatible-looking. Every conformant version-1 client petitions
`realm-0`, so a deployment that renamed its realms would answer all of them
`unavailable` forever — and the IDL specifies that absence as a *race* against
realm lifecycle, not as a permanent property of a correctly configured session.
A version-1 connection to a version-2 server is unaffected in every other
respect too: nothing about `get_realm` or `request_grant` changed.

**The other names are not discoverable on the wire, at either version.**
Enumeration is a reserved `since="2"` seam on this interface and is deliberately
unbuilt (see [Growth](#growth)), so a client learns a realm name from
[`vitrin_launcher.launched`](./16-vitrin_launcher.md#launched) or out of band.
That is why `realm-0` remains the *one* realm name a conformant client can know
without being told.

**Two authorities name realms, and they do not overlap.** An operator names
**templates**, in the server's own configuration; the server names **instances**
it creates from them, and those ids are opaque
([`launched`](./16-vitrin_launcher.md#launched)). A client MUST NOT construct or
predict either kind of name.

### Launching is deliberately not a request here

Starting an app is authority, and this handle has none. A `launch` request on
`vitrin_realm` would make **holding a name** confer the power to start a
process — the exact thing "naming is not authority" denies, and a shape no
attenuation could later undo, since a message signature is immutable forever.

Launch is instead the grant verb
[`realm_launch`](./04-vitrin_grant.md#verb), exercised through the
[`vitrin_launcher`](./16-vitrin_launcher.md) facet that
[`vitrin_grant.get_launcher`](./04-vitrin_grant.md#get_launcher) mints. Going
through a grant is what buys consent, expiry, revocation, journaling and the
rate ceiling with no new machinery: a launch petition is the shape that puts a
prompt naming the principal, the template and the template's program in front
of the human, and that ties the authority's death to the grant's.

**Served status.** The reference core serves `realm_launch` as of WS-E.1.1, so
that prompt is a thing a human really sees. Whether any *particular*
deployment serves it stays a deployment property: one that will not host
process creation refuses the verb `unsupported` at admission, exactly as every
deployment still does for `observe_cursor`. See [defined but
unserved](./04-vitrin_grant.md#defined-but-unserved).

A realm handle *is* still how such a petition is addressed — it names a realm
**template** rather than a live realm, and `launch` creates an instance from
it. The template names the program, so no command ever crosses the wire.
Choosing *which* program to run is therefore done by petitioning over a
different realm, in front of the human at consent time, rather than by an
argument after the fact. A template is addressable but never itself paints: an
`observe` grant over one refuses `no_surface` forever, which is authority over
nothing rather than authority over something dangerous.

The interface is deliberately minimal. It exists in the protocol from day one
so that later multi-realm phases can add enumeration and lifecycle **events**
to *this* interface — additively, behind `since` attributes — instead of
re-plumbing addressing. Version 1 defines no such events: the only message is
the petition request `request_grant`.

The single request, `request_grant`, does the heavy lifting: it both petitions
for a grant and co-mints, in one message, the entire object cluster that the
petition's outcome will animate — the grant handle, its consent observer, and
the three authority facets. See [Requests](#requests) below and the
[conventions page](./00-conventions.md) for framing, object-id, delivery, and
taxonomy rules referenced throughout.

## Lifecycle

A realm handle comes into existence when a principal calls
[`vitrin_principal.get_realm(realm, name)`](./02-vitrin_principal.md), which
allocates the client-supplied `new_id` and binds it to this interface. The
handle is **not** grant-derived: it is a plain addressing object with no
inert-when-dead semantics of its own. It carries no authority, so there is
nothing to revoke and nothing to make inert.

Version 1 defines no destructor. The handle lives for the connection and is
released only when the connection closes; its object id, like all client-
allocated ids, is never reused (see the [conventions page](./00-conventions.md)
on object ids). A realm handle MAY be used to launch more than one petition
over its lifetime — for example after an earlier petition resolved `denied` or
`timed_out`, the same handle may petition again — subject to the version-1
pending-petition admission cap, which is enforced per verified **identity**,
across all of that identity's connections.

## Requests

### request_grant

`request_grant(grant: new_id, consent: new_id, view: new_id, pointer: new_id, text: new_id, resource: string?, verbs: uint, expiry_ms: uint, max_event_rate: uint, persistence: uint, flags: uint)`

Petitions for a grant over this realm and, in the same message, co-mints the
grant handle, its consent observer, and the three authority facets.

| arg | type | description |
| --- | --- | --- |
| `grant` | `new_id` → [`vitrin_grant`](./04-vitrin_grant.md) | the grant handle, born **pending** |
| `consent` | `new_id` → [`vitrin_consent`](./05-vitrin_consent.md) | prompt-visibility observer scoped to this petition |
| `view` | `new_id` → [`vitrin_view`](./06-vitrin_view.md) | observation facet, inert until granted with `observe` |
| `pointer` | `new_id` → [`vitrin_actuator_pointer`](./07-vitrin_actuator_pointer.md) | pointer facet, inert until granted with `actuate_pointer` |
| `text` | `new_id` → [`vitrin_actuator_text`](./08-vitrin_actuator_text.md) | text facet, inert until granted with `actuate_text` |
| `resource` | `string` (nullable, max 256 bytes) | resource selector within the realm; **null or empty** = the whole realm (the only granularity version 1 serves). Vocabulary is type-prefixed (`surface:…`, `node:…`) and grows by version. |
| `verbs` | `uint` — bitfield [`vitrin_grant.verb`](./04-vitrin_grant.md#verb) | requested verb set; **MUST be non-zero** |
| `expiry_ms` | `uint` | requested lifetime in milliseconds; `0` = bounded only by the persistence rung |
| `max_event_rate` | `uint` | requested ceiling in **events per second**, governing observation and actuation alike; `0` = server default ceiling, **never** unlimited |
| `persistence` | `uint` — enum [`vitrin_grant.persistence`](./04-vitrin_grant.md#persistence) | requested persistence rung |
| `flags` | `uint` | boolean constraint bits; clients **MUST send 0** in version 1 (bit 0 reserved: `one_shot`) |

**The five `new_id` arguments.** All five follow the multi-`new_id` rule from
the [conventions page](./00-conventions.md): they MUST be distinct, strictly
increasing in argument order, and all above the connection's allocation
watermark. Any violation is fatal `invalid_object`. The creation order mirrors
attenuation: nothing about this petition is observable through any object the
petitioner does not hold — the consent observer is minted *by* the petition, so
an agent cannot name, and therefore cannot watch, consent activity for any
petition it did not make.

**Co-minting and inert birth.** This one request replaces what earlier drafts
split across a petition plus separate `get_view` / `get_pointer` / `get_text`
getters. All three facets are born **inert**: they confer nothing until the
grant resolves `granted`, and even then only for the verbs the human actually
approved. Every use of a facet is checked at use time against the grant's
effective verb set at the single enforcement chokepoint; a facet whose verb was
not granted refuses `not_granted` recoverably via
[`vitrin_grant.refused`](./04-vitrin_grant.md#refused) — never fatally. There
is no second enforcement site.

**Resolution.** The petition's outcome arrives exactly once as
[`vitrin_grant.resolved`](./04-vitrin_grant.md#resolved) on the co-minted grant
handle, after the consent decision (or a policy decision) completes. On outcome
`granted` that event carries the **effective** authority — the verb set,
persistence rung, and expiry the human actually chose, which may be narrower
than requested. On any other outcome the trailing arguments are zero. A denial
or a timeout is a clean, distinct event: never a hang, never a connection
death.

**Argument semantics.**
- `resource` selects what within the realm. Null or empty means the whole
  realm. An unserved resource prefix does not fail structurally; it resolves
  `unsupported`.
- `verbs` is the requested verb bitfield. It MUST be non-zero — a petition for
  nothing is a client bug, not a world change (see failure modes below).
- `expiry_ms` of `0` defers the lifetime to the persistence rung.
- `max_event_rate` is one ceiling for observation and actuation together; `0`
  selects the server's default ceiling, which is never unlimited.
- `persistence` names the requested rung. The durable rungs (`until_revoked`,
  `always`) exist in the [enum](./04-vitrin_grant.md#persistence) from day one
  but resolve `unsupported` in version 1, pending provenance verification in a
  later phase.
- Clients MUST send `flags` `0` in version 1. Bit 0 is reserved for the future
  `one_shot` constraint. `flags` references no enum and is deliberately not
  wire-validated, so a set reserved bit is not a protocol error and does not
  kill the connection; it resolves `unsupported` — honest refusal rather than
  accepted-and-unenforced.

**Delivery class.** `request_grant` is **reply-bearing**: it receives exactly
one terminal event, never coalesced. That terminal is
[`vitrin_grant.resolved`](./04-vitrin_grant.md#resolved), delivered on the
co-minted grant handle — the reply lands on a *different* object than the one
the request was sent to. Unlike other reply-bearing terminals, `resolved` is
**exempt from the cross-request "in request order" rule**: it waits on an
unbounded human consent delay, and a later `sync`'s `done` does **not** wait
for it (the `done` confirms the petition was registered and its consent
initiated, never that it resolved). See the [conventions page](./00-conventions.md)
on delivery classification and ordering.

**Failure modes.**
- *Fatal (connection dies).* `verbs == 0` is fatal `invalid_argument`: an empty
  petition is something a correct client can never intend. A `resource` string
  over 256 bytes, bad UTF-8, or an embedded NUL is likewise fatal
  `invalid_argument`. A malformed `new_id` set (non-distinct, non-increasing,
  or at/below the watermark) is fatal `invalid_object`. `request_grant` is
  additionally subject to a server-side petition-rate ceiling and a
  per-connection live-object cap (every petition permanently allocates five
  object ids — version 1 has no destructors): a connection that breaches
  either bound, or exhausts the id space itself, is closed fatal
  `resource_exhausted`, confining the denial-of-service to the offending
  connection. These are carried by
  [`vitrin_handshake.error`](./01-vitrin_handshake.md) and then the connection
  closes.
- *Recoverable (an event is delivered, the connection lives).* Every
  well-formed petition that policy or consent turns down resolves through
  [`vitrin_grant.resolved`](./04-vitrin_grant.md#resolved) with a non-`granted`
  outcome: `denied` (the human said no), `timed_out` (the prompt expired
  unanswered — petitioning again later is legal), `unavailable` (the realm was
  unknown, vacant, or closed while the petition was pending), `unsupported`
  (well-formed but refused by policy: a durable rung without provenance, a set
  reserved flag, an unserved resource prefix or verb), or `busy` (the
  identity's pending-petition admission cap was reached).

**Version 1 petition policy.** Pending-petition admission is capped **per
verified identity, across all of that identity's connections** — not merely per
connection, so opening many connections under one credential cannot multiply
concurrent prompts — and the deployment additionally enforces a global ceiling
on concurrent-plus-queued prompts. Excess petitions resolve `busy` — the
consent-spam valve. A pending petition is withdrawn if the connection closes:
consent is in-context, so the prompt disappears with the petitioner. There is
no wire message to withdraw a pending petition in version 1 (see
[Growth](#growth)).

## Enums

`vitrin_realm` defines no enums of its own. `request_grant` references two enums
that are **defined on [`vitrin_grant`](./04-vitrin_grant.md)** and documented
there:

- `verbs` uses the bitfield [`vitrin_grant.verb`](./04-vitrin_grant.md#verb)
  (`observe` = 1, `actuate_pointer` = 2, `actuate_text` = 4,
  `layout_arrange` = 16, `layout_focus` = 32, plus the defined-but-unserved
  `observe_cursor` = 8 and `realm_launch` = 512).
- `persistence` uses the enum
  [`vitrin_grant.persistence`](./04-vitrin_grant.md#persistence) (`once` = 0,
  `while_running` = 1, `until_revoked` = 2, `always` = 3).

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent. Sequences below are the
scenario walkthroughs from the message-flow catalog that touch `vitrin_realm`,
corrected for the final co-minting shape (the separate `get_view` /
`get_pointer` / `get_text` steps of earlier drafts are gone — all facets are
minted by `request_grant`).

### Flow 1 — observe-only petition, auto-approved (walking skeleton)

1. A→C `vitrin_principal.get_realm(realm=new_id, name="realm-0")`
2. A→C `vitrin_realm.request_grant(grant, consent, view, pointer, text, resource=null, verbs=observe, expiry_ms, max_event_rate, persistence=while_running, flags=0)`
3. C→A `vitrin_consent.state(closed)` — under an auto-approve policy the prompt is never rendered (loudly logged)
4. C→A `vitrin_grant.resolved(outcome=granted, verbs=observe, persistence=while_running, expiry_ms)` — the terminal reply, on the co-minted grant handle

The `view` facet is now live for `observe`; the `pointer` and `text` facets
remain inert because their verbs were not granted. Capture continues on
[`vitrin_view`](./06-vitrin_view.md).

### Flow 2 — full-authority petition, human approval (demo)

1. A→C `vitrin_principal.get_realm(realm=new_id, name="realm-0")`
2. A→C `vitrin_realm.request_grant(grant, consent, view, pointer, text, resource=null, verbs=observe|actuate_pointer|actuate_text, expiry_ms, max_event_rate, persistence=while_running, flags=0)`
3. C→A `vitrin_consent.state(shown)` — core renders the prompt; physical input is grabbed by it
4. *(human clicks "Allow while running")*
5. C→A `vitrin_consent.state(closed)`
6. C→A `vitrin_grant.resolved(outcome=granted, verbs=observe|actuate_pointer|actuate_text, persistence=while_running, expiry_ms)`

All three facets are now live. Actuation and capture proceed on
[`vitrin_view`](./06-vitrin_view.md),
[`vitrin_actuator_pointer`](./07-vitrin_actuator_pointer.md), and
[`vitrin_actuator_text`](./08-vitrin_actuator_text.md).

### Flow 3 — denial and timeout

1–3. As Flow 2, steps 1–3 (prompt shown).
4. *(human clicks "Deny")*
5. C→A `vitrin_consent.state(closed)`
6. C→A `vitrin_grant.resolved(outcome=denied, verbs=0, persistence=once, expiry_ms=0)`

Timeout variant: with no human action before the deadline, the core emits
`vitrin_consent.state(closed)` and then
`vitrin_grant.resolved(outcome=timed_out, …)`. In both cases the facets stay
inert forever and the connection lives; the SDK raises a distinct typed error.

### Flow 4 — busy (excess petition)

1. *(this identity's pending-petition admission cap is already reached)*
2. A→C `vitrin_realm.request_grant(…)` — an excess concurrent petition
3. C→A `vitrin_grant.resolved(outcome=busy, verbs=0, persistence=once, expiry_ms=0)` — the identity's admission cap

The busy petition's co-minted objects remain inert; the principal may retry
after its outstanding petition resolves.

### Flow 5 — unavailable realm

1. A→C `vitrin_principal.get_realm(realm=new_id, name="realm-does-not-exist")` — succeeds structurally
2. A→C `vitrin_realm.request_grant(…)` — mints the quintet as always
3. C→A `vitrin_grant.resolved(outcome=unavailable, verbs=0, persistence=once, expiry_ms=0)` — realm absence is a race, not a protocol error

## Growth

The interface is intentionally near-empty in version 1 so that all growth is
purely additive under `since` attributes (see the versioning rules and the
additive-safety appendix on the [conventions page](./00-conventions.md)):

- **Realm enumeration and lifecycle events.** Later multi-realm phases add
  enumeration and realm open/close lifecycle **events** to this interface. This
  is why realm ids are in the protocol from day one: addressing never has to be
  re-plumbed.
- **Value-bearing petition constraints.** Future boolean constraints on a
  petition arrive as new `flags` bits (bit 0 is already reserved for
  `one_shot`). Future *value-bearing* constraints — such as focus conditions —
  arrive as a `since`-gated builder request that precedes `request_grant`,
  because `request_grant`'s signature is frozen forever like every message
  signature.
- **Additional resource granularities.** The type-prefixed `resource`
  vocabulary (`surface:…`, `node:…`) grows by version without a new request;
  unserved prefixes resolve `unsupported` today.
- **New facets, minted elsewhere.** A verb added after version 1 cannot get a
  co-minted facet, because `request_grant`'s five `new_id` arguments are
  frozen. It arrives as a `since`-gated structural mint on
  [`vitrin_grant`](./04-vitrin_grant.md) instead —
  [`get_launcher`](./04-vitrin_grant.md#get_launcher) is the first, landed at
  version 2, and the layout facet is documented to follow the same route. This
  interface is untouched by any of it.

None of these change `request_grant`'s existing signature or the meaning of any
existing argument.

## Version history

| Version | Change |
|---|---|
| 1 | `request_grant`; no events |
| 2 | *(no message change — this interface's wire surface is unchanged at version 2; `realm_launch` is a grant verb, not a request here. What version 2 changes about realms is the **cardinality** stated above: more than one realm may exist, `realm-0` stays mandatory, and enumeration stays unbuilt)* |
