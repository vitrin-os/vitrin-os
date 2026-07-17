# vitrin_handshake — principal connection bootstrap

**Interface version:** 1 · **Connection class:** principal · **Messages:** 2 requests + 2 events

> Framing, object-id allocation, the fatal/recoverable taxonomy, delivery classification, and
> versioning are defined once in [00-conventions.md](./00-conventions.md); this page assumes them
> and only expands what is specific to `vitrin_handshake`.

## Purpose

`vitrin_handshake` is the connection-lifecycle object of a **principal connection** — a
connection accepted on the core's listening socket (`$XDG_RUNTIME_DIR/vitrin-0/core.sock`). It is
the implicit **object 1** of that connection: present at connect, never created by a message, and
alive for the whole connection. It is the first thing an agent client speaks to and the last thing
that can address it as it dies.

The object fuses three connection-global roles that share the same lifetime. It is the
**authentication gate**: the only message legal before authentication is `hello`, and nothing an
agent can do reaches the rest of the object graph until a handshake succeeds. It is the
**fatal-error channel**: every connection-fatal protocol error on a principal connection is
carried by the `error` event and is followed by connection close (see the
[error taxonomy](./00-conventions.md)). It is the **sync barrier**: `sync`/`done` give a threadless
blocking client a one-round-trip way to flush and observe the consequences of the fire-and-forget
requests it has sent.

In the object graph `vitrin_handshake` sits above everything: `hello` pre-allocates the
[`vitrin_principal`](./02-vitrin_principal.md) that a successful handshake binds, and every other
principal-facing interface (`vitrin_realm`, `vitrin_grant`, and the facets) descends from that
principal. The handshake object itself is never derived from a grant and never goes inert; only the
connection's death ends it.

The human principal has **no wire presence** in version 1: host input in nested mode is the
implicit human, and only agents handshake. There is no `hello` for humans and no interface through
which one authenticates.

## Lifecycle

`vitrin_handshake` is object 1 on every principal connection, implicit at connect. It is never
minted by a message and never destroyed by one — version 1 defines no destructors. It lives for the
entire connection and is torn down only when the connection closes (whether after an `error` event,
after `auth_failed`, or by ordinary disconnect).

It is not a grant-derived object and therefore has no inert state: its requests are legal for the
whole life of the connection, subject only to the handshake state machine below.

### Handshake state machine

A principal connection moves through exactly one authentication, across four states —
**CONNECTED → VERIFYING → BOUND**, with **DEAD** reachable from anywhere:

1. **CONNECTED** (pre-handshake). The only legal message is `hello`; a complete `hello` moves the
   connection to VERIFYING. Any other traffic before a first `hello` is fatal `pre_handshake` —
   delivered best-effort as an `error` event if the frame parsed, or a silent close if the bytes
   were unframeable garbage. Either way the reason is logged.
2. **VERIFYING.** The client MAY pipeline further requests. The server queues them during
   credential verification and processes them only after the handshake succeeds — the queue is
   bounded by ordinary transport backpressure (the server may simply stop draining the socket),
   never an unbounded server-side buffer. `hello` itself is reply-bearing; its success terminal is
   [`vitrin_principal.bound`](./02-vitrin_principal.md), emitted on the pre-allocated principal
   object.
3. **BOUND.** Once `bound` is sent, the queued requests are processed in receipt order and the
   connection is fully live — this is the steady state. `sync` becomes valid here.
4. **DEAD.** If verification fails the connection dies fatal `auth_failed` (or
   `version_unsupported` for a version mismatch) and the queued requests are **never** processed.
   Any fatal protocol violation at any point produces an `error` event followed by close.

Two properties of the machine are load-bearing for security:

- **Nothing is processed before BOUND.** The server acts on zero client requests until
  verification succeeds; pipelining buys latency, never early execution. Combined with the
  logged-reason rule below, this realizes the acceptance criterion "any traffic before a
  successful handshake is dropped with a logged reason" — with one deliberate, **sanctioned
  exception** to its literal wording: pre-`hello` traffic dies `pre_handshake`, and requests
  pipelined behind a `hello` that fails are discarded unprocessed when the connection dies, but
  requests pipelined behind a `hello` that **succeeds** are queued and then served after `bound`.
  They were *sent* before the handshake completed yet are deliberately not dropped, because the
  security property the criterion protects is zero pre-BOUND **execution**, not zero pre-BOUND
  bytes. The queue rule scopes to requests outside the handshake exchange itself: in version 1
  the exchange is `hello` alone; a later version's proof-of-possession response request is part
  of the exchange and is processed inside VERIFYING (see [Growth](#growth)).
- **The unauthenticated phase is bounded in time.** The server SHOULD impose a
  deployment-configurable deadline on every unauthenticated interval spent waiting on the
  client. A connection that exhausts the deadline — nothing sent, or a partial frame dribbled —
  is closed **administratively**: no `error` event, because nothing was violated; the close is
  indistinguishable from an ordinary disconnect. In version 1 the client-attributable interval
  is exactly CONNECTED: once a complete `hello` has arrived, remaining VERIFYING time is the
  server's own verifier latency. A later version's proof-of-possession exchange keeps the
  deadline armed inside VERIFYING while the server awaits the client's response (see
  [Growth](#growth)); a BOUND connection may idle indefinitely (e.g. awaiting a pending consent
  resolution).

Every pre-BOUND death has its reason logged server-side: `pre_handshake` traffic, refused
verification, deadline expiry, unframeable garbage, and backpressure alike.

`hello` is legal **exactly once** per connection: its opcode is defined only in the CONNECTED
state, so a second `hello` — in VERIFYING or BOUND — is fatal `invalid_opcode`.

## Requests

### hello

```
hello(version: uint, principal: new_id<vitrin_principal>, identity: string, credential_type: string, credential: string)
```

| arg | type | description |
| --- | --- | --- |
| `version` | uint | The protocol version this connection will speak — the **negotiated version**. A version the server does not implement (above its maximum) is fatal `version_unsupported`. |
| `principal` | new_id → [`vitrin_principal`](./02-vitrin_principal.md) | The principal object pre-allocated by this request and bound on success. |
| `identity` | string | The claimed identity URI, SPIFFE-shaped (e.g. `vitrin://local/agent/demo`). Max 2048 bytes — the SPIFFE-ID maximum (a 255-byte trust domain plus path). |
| `credential_type` | string | Credential scheme discriminator naming how `credential` is interpreted (e.g. `static-token`, `spiffe-jwt-svid`, `oidc`, `ssh-cert`). Max 32 bytes. |
| `credential` | string | Opaque, scheme-defined credential bytes, interpreted only by the verifier. Max 32768 bytes. |

`hello` is the only pre-authentication message: **credential presentation is folded into `hello`**
rather than split across an exchange, so the whole handshake is one client message and one server
terminal — one round trip, no partially-authenticated wire state between them. Version 1's
credential schemes are bearer-shaped (a pre-shared token under D5; JWT-SVID/OIDC tokens are the
same shape), so nothing needs a server turn before the credential can be presented;
proof-of-possession schemes that do need one arrive as an appended exchange in a later version
(see [Growth](#growth)). `hello` presents the credential to the core's **pluggable verifier** and
pre-allocates the `vitrin_principal` object that a successful handshake binds. Success is
announced by [`vitrin_principal.bound`](./02-vitrin_principal.md); the identity that `bound`
carries is the verifier-canonical value, **not** an echo of the `identity` string presented here.

The version accepted here becomes the connection's **negotiated version** for its whole life:
messages introduced by a later `since` are not defined on the connection, and using one is fatal
`invalid_opcode` (see [version semantics](./00-conventions.md#73-version-semantics)).

Checks run in a **fixed order**: frame grammar first (fatal decode errors as usual), then the
`version` integer, then — only for a version-accepted `hello` — credential verification.
`version_unsupported` therefore reveals nothing about the credential or the claimed identity.

The credential encoding is deliberately opaque to the wire. `credential_type` names a scheme and
`credential` carries scheme-defined bytes; the wire hard-wires no scheme. This is the
signature-agility hook for future credential families. Because a `string` argument caps a single
frame's payload, credentials larger than one frame arrive via an fd-borne sibling request in a
future version (see [Growth](#growth)).

The credential is **secret material**: the server MUST NOT write credential bytes into logs,
`error.message` text, or the flight recorder — at most `credential_type` and the byte length may be
recorded.

**Sender-constraint.** Authentication binds a triple: this connection, the verified credential, and
the `SO_PEERCRED` recorded by the transport at accept time. A handle minted on one connection and
presented on another is fatal `invalid_object` — object ids are per-connection and are never
portable across connections.

This signature is **frozen forever**: extension is a new message, never a changed `hello`.

**Delivery class:** reply-bearing. Its terminal is either `vitrin_principal.bound` (success) or a
fatal `error` on this interface (`auth_failed` or `version_unsupported`) followed by connection
close. There is no `hello`-specific event on `vitrin_handshake` itself.

**Failure modes (all fatal — the connection dies):**

- `version_unsupported` — the server does not implement the offered `version`. Because growth is
  strictly additive, a server implements every version up to its maximum, so this means the
  client offered a version **above the server's maximum** (in version 1 exactly one version
  exists, so acceptance degenerates to an exact match). Downgrade is refusal, not negotiation:
  the server never counters with a different version and the error carries no supported-version
  hint (`error.message` is never machine-parsed); a newer client willing to speak an older
  version reconnects offering a lower integer — convergence by descending reoffer, bounded by
  the client's own maximum.
- `auth_failed` — the credential was rejected (unknown identity, bad token, verifier failure, or
  `SO_PEERCRED` mismatch — never distinguished on the wire; see below).
- `invalid_object` — the `new_id` for `principal` violated id allocation rules (at or below the
  watermark, reserved range, reused), or a cross-connection handle was presented.
- `invalid_opcode` — a second `hello`; the opcode is defined only in the CONNECTED state.
- `pre_handshake` — a non-`hello` message arrived before any `hello`.

There are **no recoverable refusals** of `hello`: a rejected credential is a client-known failure
(the client chose the credential), so it is fatal, not an event on a living connection.

#### What a refused handshake reveals (identity-probing resistance)

A refused handshake is **deliberately uniform on the wire**. Every credential-rejection cause —
unknown identity, bad token, verifier failure, `SO_PEERCRED` mismatch — collapses to the single
fatal code `auth_failed`, and the accompanying `error.message` MUST be a **fixed phrase** (e.g.
`"authentication refused"`) that neither names the cause nor echoes the claimed identity. A local
observer therefore cannot use the handshake as an enumeration oracle: probing
`vitrin://local/agent/alice` with a garbage token yields bytes identical to probing an identity
that does not exist. The cause and the claimed identity are recorded in the **server log only** —
that log entry is the "logged reason" the state machine promises, and it still MUST NOT contain
credential bytes.

Verification SHOULD take **uniform time** across rejection causes: constant-time credential
comparison, and identical verification work for unknown identities (e.g. verifying against a dummy
record), so that latency does not become the oracle the message text refuses to be.

What the wire does disclose, by design: `version_unsupported` reveals that the server exists and
does not speak the offered version (it is checked before the verifier ever runs, so it says
nothing about identities), and `auth_failed` reveals only that *some* verification step refused.
Reachability of the socket itself is governed by its filesystem permissions, not by the protocol.

### sync

```
sync(cookie: uint)
```

| arg | type | description |
| --- | --- | --- |
| `cookie` | uint | A client-chosen value echoed back verbatim by the `done` event. |

`sync` requests a `done` event carrying the same `cookie`. `done` is sent only after **every request
received before this `sync` has been processed and every event those requests caused has been queued
ahead of it** — the guarantee rests on the connection's single-ordered-stream property
(see [ordering](./00-conventions.md)). One exemption: petition-lifecycle events
([`vitrin_grant.resolved`](./04-vitrin_grant.md), [`vitrin_consent.state`](./05-vitrin_consent.md))
wait on human consent and do not participate in the barrier — `done` confirms a preceding petition
was registered and its consent initiated, never that it resolved.

This is the mechanism that makes a threadless blocking client correct. Because actuation requests
are fire-and-forget and their enforcement failures arrive as `vitrin_grant.refused` events, a client
sends its actuations, then `sync`, then reads events until `done`, raising on any `refused` event it
sees along the way. Failure discovery is thereby bounded to **one round trip** without a per-actuation
acknowledgement. `sync` is a cookie echo, not a callback object, so it costs no id churn.

`sync` is **valid only after `bound`**. It exercises no grant and never refuses recoverably.

**Delivery class:** reply-bearing. Its terminal is exactly one `done` per `sync`, in request order,
never coalesced.

## Events

### error

```
error(object_id: uint, code: uint, message: string)
```

| arg | type | description |
| --- | --- | --- |
| `object_id` | uint | The id of the object where the error occurred. MAY be `1` (this handshake object itself). Carried as a plain id number, not an object reference. |
| `code` | uint (enum [`error`](#enum-error)) | The fatal error code, namespaced by the cited object's interface error enum. |
| `message` | string | Free-form debugging text. Never machine-parsed. |

`error` is the connection-fatal error channel for a principal connection. `object_id` names where
the fault occurred; `code` is namespaced by that object's interface `error` enum — and in version 1
the only interface that defines an `error` enum is this one, so **every fatal code is
connection-global**. `message` is human-facing debug text and MUST NOT be parsed by clients; for
`auth_failed` it is a fixed, cause-free phrase (see
[identity-probing resistance](#what-a-refused-handshake-reveals-identity-probing-resistance)).

After sending `error` the server **closes the connection**. Delivery is best-effort: a peer that has
stopped reading (backpressure death) and an unframeable stream are closed **without** an `error`
event at all — an id can only be cited if the frame parsed far enough to name one. Where an `error`
can be delivered, it is; where it cannot, the close is silent and the reason is logged.

Shim connections have **no** counterpart to this event: a shim protocol violation is log-and-close,
because the shim is a disposable core child (see
[`vitrin_shim_session`](./09-vitrin_shim_session.md)).

`object_id` is typed `uint` rather than `object` on purpose: the erring object's interface is not
knowable statically for every fault (e.g. a message against an unknown id), so citing a bare id
number keeps the strict typed-object rule intact with no special case.

### done

```
done(cookie: uint)
```

| arg | type | description |
| --- | --- | --- |
| `cookie` | uint | The exact value passed to the corresponding `sync`. |

`done` answers `sync`: all requests received before that `sync` have been processed and all events
they caused have been queued ahead of this `done`. It is not coalesced with any other `done`, and
`done` events pair with their `sync` requests in order.

## Enums

### enum `error`

Connection-global fatal error codes. Every condition here is one a **correct client can never
trigger**; the fuzz target maps every decode failure onto exactly one of these codes with a clean
connection death, and every error path closes any received file descriptor before closing the
connection.

| entry | value | meaning |
| --- | --- | --- |
| `invalid_object` | 0 | Unknown or foreign object id, id reuse at or below the watermark, reserved-range id, or a multi-`new_id` rule violation. |
| `invalid_opcode` | 1 | Opcode not defined for the interface at the negotiated version — including opcodes belonging to the other connection class, and a second `hello` (its opcode is defined only in the CONNECTED state). |
| `invalid_argument` | 2 | Argument decode failure: bad UTF-8, embedded NUL, a string over its documented bound, an out-of-range enum value (a bit outside a bitfield's defined mask counts as out-of-range), a forbidden control character, zero verbs in a petition, or malformed padding. |
| `oversized` | 3 | Declared frame size below the 8-byte header minimum, or a payload shorter than the size declares. The 65535-byte ceiling binds senders — a u16 cannot express more. |
| `fd_violation` | 4 | The header `fd_count` disagrees with the message signature, or unsolicited fds were attached. |
| `pre_handshake` | 5 | Traffic before a first `hello` on a principal connection. |
| `version_unsupported` | 6 | `hello` carried a protocol version the server does not implement; downgrade is refusal. |
| `auth_failed` | 7 | Credential rejected: unknown identity, bad token, verifier failure, or `SO_PEERCRED` mismatch. The cause is never distinguished on the wire — uniform code, fixed message text, detail in the server log only. |
| `internal` | 8 | A server-side failure that poisoned the connection. |
| `resource_exhausted` | 9 | A documented per-connection resource bound was breached: the petition-rate ceiling, the live-object cap, or object-id exhaustion. Denial-of-service confinement, not a semantic judgement. |

This enum is not a bitfield: `code` carries exactly one value. Values are immutable across versions;
later versions append entries and mark deprecations, never renumber (see
[versioning](./00-conventions.md)).

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent. Sequences are corrected for the final XML
shapes (`bound` is on [`vitrin_principal`](./02-vitrin_principal.md); `hello` gained the
`credential_type` discriminator).

### Flow 1 — Successful handshake (walking skeleton, opening steps)

```
1. [A connects to $XDG_RUNTIME_DIR/vitrin-0/core.sock; core records SO_PEERCRED at accept]
2. A→C  vitrin_handshake.hello(version=1, principal=new_id, identity="vitrin://local/agent/demo",
                               credential_type="static-token", credential=<token bytes>)
3. C→A  vitrin_principal.bound(identity=<verifier-canonical identity>)
        — verifier checked the credential; the pre-allocated principal is now live
```

The handshake terminal lives on `vitrin_principal`, not here; from this point the client addresses
the principal to obtain realms and petition for grants.

### Flow 2 — Pipelining across verification

```
2. A→C  vitrin_handshake.hello(version=1, principal=new_id, identity=…, credential_type=…, credential=…)
3. A→C  vitrin_principal.get_realm(realm=new_id, name="realm-0")     [queued during verification]
4. A→C  vitrin_realm.request_grant(…)                                [queued during verification]
5. C→A  vitrin_principal.bound(identity=…)                            [verification succeeded]
6. C→A  … the queued requests from steps 3–4 are now processed in order …
```

If verification instead fails, step 5 is replaced by a fatal `error(auth_failed)` and the queued
requests are never processed.

### Flow 3 — Sync barrier flushing fire-and-forget actuations (M1.4 demo tail)

```
…  A→C  vitrin_actuator_pointer.move(…) / button(…) / vitrin_actuator_text.type("…\n")   [fire-and-forget]
N. A→C  vitrin_handshake.sync(cookie=42)
N+1. C→A  [any vitrin_grant.refused events caused by the actuations above, queued ahead of done]
N+2. C→A  vitrin_handshake.done(cookie=42)
        — SDK read the stream to done; if any refused was seen, it raises; else the actuations landed
```

This bounds revocation/refusal discovery to one round trip (the M1.4 "revocation latency ≤ one round
trip" property).

### Flow 4 — Fatal error paths

```
Pre-handshake traffic:
  A→C  <any non-hello message before hello>
  C→A  vitrin_handshake.error(object_id=1, code=pre_handshake, message="…")   → close

Version mismatch (this version-1 server's maximum is 1):
  A→C  vitrin_handshake.hello(version=2, …)
  C→A  vitrin_handshake.error(object_id=1, code=version_unsupported, message="…")   → close
        — a client willing to speak version 1 reconnects and offers the lower integer

Bad credential (any cause — unknown identity, bad token, verifier failure, peercred mismatch):
  A→C  vitrin_handshake.hello(version=1, …, credential=<rejected>)
  C→A  vitrin_handshake.error(object_id=1, code=auth_failed, message="authentication refused")   → close
        — same code, same fixed phrase for every cause; the cause is logged server-side only

Sender-constraint / cross-connection handle:
  A→C  <request citing an id minted on another connection>
  C→A  vitrin_handshake.error(object_id=<cited id>, code=invalid_object, message="…")   → close

Unauthenticated deadline (nothing sent, or a partial frame dribbled):
  [connection closed administratively, reason logged — no error event: nothing was violated]

Unframeable garbage / backpressure death:
  [connection closed silently, reason logged — no error event deliverable]
```

## Growth

Every seam below is purely additive under the protocol's Wayland-style growth rule: new messages are
appended with `since` attributes, enum entries are appended with immutable values, and existing
message signatures never change (see [versioning](./00-conventions.md)).

- **fd-borne credential sibling (`since="2"`).** `hello`'s `credential` string is capped by the
  single-frame limit. A future fd-carrying sibling request lets credentials that exceed one frame
  (large certificate chains) arrive out of band. `hello` itself stays frozen; the new message is the
  escape hatch, not a changed signature.
- **New credential families.** The `credential_type` discriminator already lets new schemes be
  introduced without any wire change — only the verifier learns to interpret new bytes.
- **Proof-of-possession credential exchange (`since="2"`).** Version 1 schemes are bearer-shaped,
  which is what lets the whole handshake be a single `hello`. A scheme whose `credential_type`
  demands proof of possession (e.g. X.509-SVID with a private-key challenge) arrives as an
  appended server-driven exchange inside VERIFYING — a new challenge event plus a new response
  request on this interface. The exchange is part of the handshake itself: the response request
  is exempt from the queued-until-BOUND rule (which scopes to requests outside the handshake
  exchange), and the unauthenticated deadline stays armed while the server awaits the client's
  response. `hello` and `bound` stay frozen; version-1 connections and bearer schemes never see
  the new messages.
- **Interface-namespaced error codes.** In version 1 this interface owns the only `error` enum, so
  all fatal codes are connection-global. When a later version gives another interface its own `error`
  enum, `error.object_id` already routes the namespacing: `code` is read against the cited object's
  interface. New `error` entries are appended with fixed values.
