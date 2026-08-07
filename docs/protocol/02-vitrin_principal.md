# vitrin_principal — the authenticated principal, root of the connection's authority chain

**Interface version:** 1 · **Connection class:** principal · **Messages:** 1 request + 1 event

## Purpose

`vitrin_principal` is the wire projection of a bound identity: the agent that
successfully authenticated on a [principal connection](00-conventions.md#12-the-two-connection-classes-and-their-bootstrap-objects).
It is the root of the connection's authority chain. Everything an agent is
allowed to do flows down from this object — but the principal itself holds only
two narrow powers: it can mint an address handle for a realm
([`get_realm`](#get_realm)), and, through that handle, it can petition for
authority (`vitrin_realm.request_grant`). It confers no capability of its own;
naming a realm is not authority over it.

In the object graph the principal sits directly below the connection bootstrap.
Object 1 of a principal connection is [`vitrin_handshake`](01-vitrin_handshake.md);
the principal is the object that `vitrin_handshake.hello` pre-allocates and that
a successful handshake binds. From the principal hang realm address handles
([`vitrin_realm`](03-vitrin_realm.md)), and from each petition hangs a grant and
its facets. The principal is therefore the single per-connection identity anchor:
one connection, one principal, one verified identity.

The design idea is attenuation from a verified root. The identity carried by the
[`bound`](#bound) event is the verifier's canonical value — normalized and
recorded in the grant table — never an echo of the client's claimed string.
Every downstream artifact that renders "who is asking" (the consent prompt, the
flight recorder) derives from that verifier-owned value, so free client text can
never reach a human decision. Authority only narrows as it descends: nothing the
principal can express widens what the verifier decided.

Version 1 has no grant persistence. All of a principal's grants die with the
connection; restore tokens that would let authority outlive a connection are a
[later version's addition](#growth).

## Lifecycle

A principal object comes into existence in two steps. First,
`vitrin_handshake.hello` carries a `new_id` argument that **pre-allocates** the
principal object; at that moment the id is claimed but the object is not yet
live. Second, once the core's pluggable verifier accepts the credential, the
principal is bound and the server emits [`bound`](#bound) — the terminal event of
the successful handshake. Only after `bound` is the principal live and are
queued post-`hello` requests processed.

If verification fails, the principal never becomes live: the connection dies
fatally with `auth_failed` (or `version_unsupported` for a version mismatch),
carried by `vitrin_handshake.error`, and any requests the client pipelined after
`hello` are never processed. See [the handshake state machine](01-vitrin_handshake.md#lifecycle).

The principal is not grant-derived, so it has no inert state: it is either
unborn (pre-`bound`) or live for the remainder of the connection. Version 1
defines no destructors on this interface; the principal lives until the
connection closes, and its death takes every grant, realm handle, and facet with
it. A pending petition made by this principal is withdrawn on connection close
(consent is in-context — the prompt disappears with the petitioner).

## Requests

### get_realm

```
get_realm(realm: new_id<vitrin_realm>, name: string)
```

| arg | type | description |
|---|---|---|
| `realm` | `new_id<vitrin_realm>` | the new realm address handle; MUST obey the [id-allocation rules](00-conventions.md#3-object-ids) (strictly increasing, above the watermark, never reused) |
| `name` | `string` | realm name (max 64 bytes); `"realm-0"` is the well-known one — the single well-known realm of version 1, and a required member of every version-2 deployment |

Creates a [`vitrin_realm`](03-vitrin_realm.md) address object for a realm known
by name. This request **always succeeds structurally**: minting the handle is an
addressing operation, not an authority check. Holding a realm handle lets the
principal petition and nothing more.

**Realm cardinality.** Version 1 fixes the count as well as the name: exactly
one realm, `"realm-0"`. Version 2 lifts the count to a deployment-chosen limit
and keeps `"realm-0"` **mandatory**, so a conformant version-1 client's
`get_realm("realm-0")` still names a realm that exists whatever else the
deployment serves. The other names are not discoverable on the wire at either
version — enumeration is a reserved `since="2"` seam on
[`vitrin_realm`](03-vitrin_realm.md#growth) and is deliberately unbuilt — so a
client learns one from
[`vitrin_launcher.launched`](16-vitrin_launcher.md#launched) or out of band.
The full argument, including the two naming authorities, is on
[`vitrin_realm`](03-vitrin_realm.md#realm-cardinality-one-at-version-1-a-bounded-set-at-version-2).

Realm absence is deliberately not surfaced here. A name that is unknown or vacant
still yields a well-formed handle; the absence is discovered later, as a
petition outcome — `vitrin_realm.request_grant` on such a handle resolves
`vitrin_grant.resolved(unavailable)`. This is because realms are dynamic in later
phases: absence is a race against realm lifecycle, not a protocol error, and
treating it as an outcome (not a refusal at address time) keeps the addressing
layer stable when multi-realm enumeration arrives.

**Delivery class:** a **structural mint** — neither reply-bearing nor refusable.
`get_realm` is a pure factory: it mints server state (the realm handle)
synchronously and emits no terminal event. Its effect is observable only through
the object it returns, never through a reply.

**Failure modes.** There are no recoverable refusals — an unknown name is not a
failure of this request (see above). The only failures are
[fatal](00-conventions.md#5-error-taxonomy), and each is something a correct client
can never trigger:

- `invalid_object` — the `realm` new_id violates the id-allocation rules
  (reuse at or below the watermark, a reserved-range id, or non-increasing order
  relative to prior allocations).
- `invalid_argument` — `name` exceeds 64 bytes, is not valid UTF-8, contains an
  embedded NUL or a forbidden control character, or has malformed padding.

## Events

### bound

```
bound(identity: string)
```

| arg | type | description |
|---|---|---|
| `identity` | `string` | verifier-canonical principal identity (max 2048 bytes) |

The terminal event of a successful handshake, sent exactly once when the
principal is bound. `identity` is the canonical identity **as normalized and
verified by the core's verifier** and recorded in the grant table — it is not an
echo of the string the client claimed in `hello`. Everything the consent prompt
and the flight recorder later render about this principal derives from this
verifier-owned value, never from free client text.

**Delivery class:** `bound` is the single terminal event of the reply-bearing
`vitrin_handshake.hello` request. On the failure path `hello` has no `bound`; the
connection instead dies fatally with `auth_failed` or `version_unsupported`.
Because `bound` is a handshake terminal it is never coalesced, and it is ordered
ahead of any event caused by requests the client pipelined after `hello`.

**Failure modes.** None on this event itself; failure of the handshake it
concludes is fatal and carried by `vitrin_handshake.error`, not here.

## Flows

`vitrin_principal` participates in the prelude common to every principal-side
scenario: the handshake concludes with [`bound`](#bound), then the agent
addresses a realm with [`get_realm`](#get_realm) before petitioning. Sequences
below are corrected for the final XML (co-minting in `request_grant`; the old
`grant.bind`/`grant.result`/`consent.state(approved)` shapes are gone). Direction
key: **A→C** agent→core, **C→A** core→agent.

### Flow 1 — Bind and address (prelude of scenario (a), the walking skeleton)

```
1.  [A connects to $XDG_RUNTIME_DIR/vitrin-0/core.sock; core records SO_PEERCRED]
2.  A→C  vitrin_handshake.hello(version=1, principal=new_id,
             identity="vitrin://local/agent/demo",
             credential_type="static-token", credential=<token>)
3.  C→A  vitrin_principal.bound(identity="vitrin://local/agent/demo")
             — verifier-canonical; principal now live
4.  A→C  vitrin_principal.get_realm(realm=new_id, name="realm-0")
5.  A→C  vitrin_realm.request_grant(...)   [continues on the vitrin_realm page]
```

Steps 3–4 are the whole of this interface's involvement: `bound` makes the
principal live (step 3), and `get_realm` mints the address handle the petition
in step 5 needs (step 4). The auto-approve walking skeleton then proceeds through
`vitrin_grant.resolved(granted)` and capture; see
[`vitrin_realm`](03-vitrin_realm.md) and [`vitrin_grant`](04-vitrin_grant.md).

### Flow 2 — Bind and address (prelude of scenario (b), the interactive demo)

Identical to Flow 1 steps 1–4, with a broader verb set requested at step 5
(`observe | actuate_pointer | actuate_text`) and a real consent prompt rather
than auto-approve. The `vitrin_principal` segment is unchanged: one `bound`, one
`get_realm`. The grant-denial and consent-timeout variants (scenario (c)) share
this same prelude and diverge only at the petition's outcome, which is carried on
[`vitrin_grant.resolved`](04-vitrin_grant.md), never here.

## Growth

The interface descriptions name one purely-additive seam for this object.

- **Restore tokens (grant persistence).** Version 1 grants die with the
  connection; there is no way for authority to survive a reconnect. A later
  version adds restore-token machinery so a principal can re-present authority
  granted under a durable persistence rung
  ([`until_revoked` / `always`](04-vitrin_grant.md), which resolve `unsupported`
  in version 1 pending provenance verification). This arrives as new
  `since`-gated messages; it changes no existing signature and leaves the
  version-1 lifecycle — bind, address, petition, die-with-connection —
  untouched. See the [additive-safety appendix](00-conventions.md#appendix-a--additive-safety-table).

Realm enumeration and lifecycle events are a related later-phase addition, but
they attach to [`vitrin_realm`](03-vitrin_realm.md), not to the principal.
