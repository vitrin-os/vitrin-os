# Vitrin protocol — conventions (normative)

This is the normative reference for the Vitrin OS wire protocol, version 0
(the `hello`/document version integer starts at `1` and is `2` today). Every
interface page links here; this page defines the rules those pages assume.
Where this page and the IDL (`protocol/vitrin-v0.xml`) disagree, **the IDL
wins** — its `<description>` text is the source of truth and this page
restates it.

Two wire versions exist. Version 2 appends exactly three things — the
`realm_launch` verb bit, the structural mint `vitrin_grant.get_launcher`, and
the [`vitrin_launcher`](16-vitrin_launcher.md) interface it mints — and
changes nothing else: every version-1 signature is byte-identical at version
2. Statements below that say "version 1" are statements about version 1 and
stay accurate; where version 2 differs, it is named.

The words MUST, MUST NOT, SHOULD, and MAY are used in the RFC 2119 sense.

Interface pages:

- [01 — vitrin_handshake](01-vitrin_handshake.md)
- [02 — vitrin_principal](02-vitrin_principal.md)
- [03 — vitrin_realm](03-vitrin_realm.md)
- [04 — vitrin_grant](04-vitrin_grant.md)
- [05 — vitrin_consent](05-vitrin_consent.md)
- [06 — vitrin_view](06-vitrin_view.md)
- [07 — vitrin_actuator_pointer](07-vitrin_actuator_pointer.md)
- [08 — vitrin_actuator_text](08-vitrin_actuator_text.md)
- [09 — vitrin_shim_session](09-vitrin_shim_session.md)
- [10 — vitrin_shim_surface](10-vitrin_shim_surface.md)
- [11 — vitrin_shim_seat](11-vitrin_shim_seat.md)
- [16 — vitrin_launcher](16-vitrin_launcher.md) *(since version 2)*

The gap from 11 to 16 is deliberate: pages 12–15 are allocated to interfaces
that have not landed yet (`docs/plan/02-phase-2-semantic-epochs.md` §5), and
taking an allocated number would be the collision that registry exists to
prevent.

---

## 1. Overview and object model

Vitrin is a capability-oriented wire protocol between **agent principals** and
a compositing **core**, plus a private channel between the core and the
per-app **shims** it spawns. The protocol carries three intertwined concerns:
authenticating principals, mediating consent for authority over realms, and
moving frames and input across the trust boundary.

### 1.1 Principals, realms, grants, facets, consent

- **Principal** — an authenticated agent identity (SPIFFE-shaped, e.g.
  `vitrin://local/agent/demo`). The principal object is the root of a
  connection's authority chain: it can address realms and petition for grants,
  nothing more. The **human principal has no wire presence in version 0**;
  host input in nested mode is the implicit human, and only agents handshake.
- **Realm** — an addressing scope. Grants attach to realms and apps launch
  into realms. Version 1 serves exactly one well-known realm, `realm-0`;
  version 2 serves `realm-0` plus however many further realms the deployment
  configured, up to a limit of its own choosing, and keeps `realm-0`
  **mandatory** so a version-1 client's `get_realm("realm-0")` never breaks.
  The further names are not enumerable on the wire at either version (see
  §1.4's deferrals). A realm handle is deliberately authority-free: it answers
  "which realm are you asking about," and holding one only lets the principal
  petition.
- **Grant** — the wire projection of one grant-table row
  (principal × resource × verbs × constraints). A grant is born **pending**
  and confers nothing; **exactly one** `resolved` event decides its fate. A
  grant that later expires or is revoked goes **dead**.
- **Facet** — a capability object (`vitrin_view`, `vitrin_actuator_pointer`,
  `vitrin_actuator_text`, and since version 2 `vitrin_launcher`) through which
  a granted verb is exercised. Facets are born **inert**: they confer nothing
  until the grant resolves `granted`, and every use is checked at a single
  server-side enforcement chokepoint. When the grant dies, its facets go
  inert; their requests are **refused recoverably, never fatally**. The first
  three are **co-minted with the grant** by `request_grant`; every facet added
  after version 1 is instead minted **on the grant**, because
  `request_grant`'s five `new_id` arguments are frozen forever —
  `vitrin_launcher` is the first, minted by
  [`get_launcher`](04-vitrin_grant.md#get_launcher). Inert birth and
  check-at-use are identical either way.
- **Consent** — the prompt-visibility lifecycle of exactly one petition,
  minted by `request_grant` and visible only to its petitioner. It carries no
  requests and no authority: the authoritative decision arrives on the grant
  as `resolved`, so a blocking client MAY ignore consent events entirely.

### 1.2 The two connection classes and their bootstrap objects

The **class** of a connection is fixed at accept time by the transport and is
never negotiated on the wire:

| Class | Transport | Bootstrap object 1 | Authentication |
|---|---|---|---|
| **Principal** | listening socket `$XDG_RUNTIME_DIR/vitrin-0/core.sock` | `vitrin_handshake` | `hello` credential, verified by a pluggable verifier + `SO_PEERCRED` recorded at accept |
| **Shim** | core-inherited socketpair | `vitrin_shim_session` | structural: the inherited socketpair *is* the credential; realm identity assigned at fork |

Both classes share one wire format and codec; they differ only in
authentication and dispatch table. **The two classes' interfaces are mutually
unreachable**: a message using the other class's opcodes dies as a fatal
protocol error (`invalid_opcode`) with no special casing.

### 1.3 Capability and sender-constraint posture

Authority in Vitrin is **capability-based**: an object reference *is* the
authority to invoke that object's verbs, subject to the grant's effective verb
set checked at use time. Objects are minted in attenuation order — nothing
about a petition is observable through any object the petitioner does not hold.

Authority is **sender-constrained** to the connection that obtained it. The
sender-constraint triple is `(connection, verified credential, SO_PEERCRED
recorded at accept)`. A handle minted on one connection and presented on
another is fatal `invalid_object`. Because object ids are per-connection and
never reused (§3), a captured id from one connection is meaningless on
another.

### 1.4 Scene authority: arrangement, ordering, cursors

Two kinds of authority over the shared scene are kept distinct, and **only one
of them is purchasable**. This section is normative; decisions D-017 (cursors)
and D-018 (layout) record the reasoning.

**Arrangement is grant-governed.** Which realm view sits where, at what size,
and which one holds keyboard focus is named by the
[`layout_arrange` and `layout_focus`](04-vitrin_grant.md#verb) verbs. An
unprivileged shell that arranges realms therefore does so with attenuable,
revocable, journaled authority — *"this shell may arrange realm-7 and realm-9,
may not steal focus"* — rather than ambiently. This is what keeps
window-management policy outside the trusted core (PRD §5.1's invariant)
**without** making the shell trusted: "the shell is trusted" would move exactly
the code that invariant exiles back into the TCB. Focus is a separate verb from
placement because focus theft is simultaneously the sharpest attack (it
redirects keystrokes meant for another realm) and the most legitimate need, so
it must be attenuable alone.

**Ordering is purchasable by no grant, at any verb set, ever.** The core
enforces these invariants unconditionally:

1. the consent surface and the trust indicator composite **above** every
   principal's content;
2. the core's own hit test — never a client's claimed stacking — decides which
   surface an input event reaches;
3. no arrangement may occlude, fullscreen over, or resize away the consent
   surface;
4. no **agent** principal's cursor is composited into another principal's
   captured frame — the one cursor a capture may ever contain is the human
   principal's, and only for a grant holding `observe_cursor`, which is the
   single carve-out and is itself grant-governed rather than free.

**Their standing today, stated rather than implied** (D-018 costs; this
paragraph is the honest reading of "unconditionally", and the two must be read
together). Invariant 1 holds and is exercised: the overlay composites at the
output stage, structurally above the scene the capture is taken from, and
`backend/headless.rs`'s
`a_prompt_reaches_human_visible_output_but_never_a_capture` asserts exactly
that — the trust band's colour at pixel (0,0) of the human-visible output where
the capture at the same pixel still holds realm content, the consent card
verbatim in the output, and the capture byte-identical to the bare scene
compose. Invariant 4 **is no longer vacuous** (D-019): the core now composites
an agent principal's own cursor, so there is something the rule can be violated
by. What enforces it is *where* the sprite is drawn — at the output stage,
downstream of the `Scene::compose` every capture is taken from, so a capture
cannot carry it by construction rather than by a checked flag — and a test does
exist for exactly that: `backend/headless.rs`'s
`the_agent_cursor_reaches_human_visible_output_but_never_a_capture`, which
asserts the sprite really is on the human-visible framebuffer at the agent's
own position, that the realm view is unchanged to the byte, and that no sprite
pixel survives into the frame a `capture_frame` would seal into a memfd. Two
limits stated rather than left to be found: that test is a **component** test
against composited pixels in-process, not a mock-free integration gate, and it
proves the *core* excludes the sprite from a capture — it does not exercise a
second agent trying to obtain the first's cursor, because agent-to-agent
observation has no wire surface to try it through (no agent can name another's
grant), so that half remains unpurchasable by construction rather than by test.
Invariant 2 still holds **vacuously**: no client can state a stacking order.
Invariant 3 has nothing to be true of, because there is no arrangement
mechanism. **None of the four is tested *as an invariant*** — against a client
trying to violate it — and none can be until something outside the core can
arrange realms. That test is E3's, and D-018 is the reason it must exist.

The shell gets *arrangement*; the core keeps *ordering*. Read the ordinary
window-manager verbs as authority rather than decoration and the reason is
plain: `raise` would put a surface over the consent card, `move` would slide a
different target under the pointer mid-decision, `fullscreen` would impersonate
the whole session. Invariants 1–3 are what make those attack primitives
unpurchasable, which is also why `raise`, `move`, `resize`, and `fullscreen`
are **not** separate verbs — with ordering held by the core, splitting them
further would be attenuation theatre.

**Cursors.** Each principal has exactly one virtual pointer per realm it holds
pointer authority over, and
[`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) is that pointer's
only name on the wire. A principal is **cursorless by construction**: an agent
that never petitions for `actuate_pointer` has no pointer, so the headless-fleet
case needs no wire vocabulary, and there is deliberately no request by which a
principal declares, disowns, or hides a cursor. Cursors are **core-composited**
— a realm may never supply the pointer bitmap, because a realm that drew its
own could paint a decoy and mislead the human about where input is going.
Visibility is a relation, not a flag, and is settled **asymmetrically** by
invariant 4 plus the `observe_cursor` verb: agent→agent is unpurchasable at any
verb set, agent→human is closed by default and opens only through that verb,
which is meaningful only alongside `observe`. See
[`vitrin_view`](06-vitrin_view.md#what-a-capture-does-not-contain).

**What version 1 actually does.** It serves neither layout verb (it has no
window manager). It **does** composite an agent principal's own cursor, and
only into human-visible output — never into a captured frame — at the same
output stage as the consent overlay and the trust indicator; nested mode draws
it always, and `--headless --agent-cursor` on request (the headless
human-visible framebuffer is otherwise measured byte-for-byte against the realm
view by the trusted-band witness). No *human* cursor is composited: in nested
operation the host desktop draws it, outside the realm view entirely. Delivery
is a separate thing from drawing, and delivery has not changed: version 1 still
delivers **one shared pointer position** per realm view to the shim, and
per-principal delivery stays deferred to M2 (D-017, D-019). All three of the
verbs this section governs — `observe_cursor`, `layout_arrange`,
`layout_focus` — are defined on the wire and refused `unsupported`. They are
not the whole unserved set: version 2's `realm_launch` takes the same staged
posture for a different question, which is why
[§ defined but unserved](04-vitrin_grant.md#defined-but-unserved) lists four
verbs rather than these three. See it for why defining a verb early is
structural rather than cosmetic.

---

## 2. Wire format

### 2.1 Framing

Every message is a single frame. All multi-byte integers are **little-endian**.
Each frame begins with an **8-byte header**:

```
 byte:  0        1        2        3        4        5        6        7
      +--------+--------+--------+--------+--------+--------+--------+--------+
      |            object_id (u32)        |    size (u16)   | opcode | fd_cnt |
      +--------+--------+--------+--------+--------+--------+--------+--------+
      |<---------- 4 bytes ------------->|<-- 2 bytes --->|<- 1 ->|<- 1 -->|
              little-endian                little-endian    u8       u8
```

| Field | Type | Meaning |
|---|---|---|
| `object_id` | u32 | the target object of the message |
| `size` | u16 | **whole frame including the 8-byte header**; a declared size below the 8-byte minimum, or a payload shorter than the size declares, is fatal `oversized`. The 65535-byte ceiling binds **senders** (a u16 cannot express more), which MUST NOT construct a larger frame |
| `opcode` | u8 | the request/event opcode within the target object's interface at the negotiated version |
| `fd_count` | u8 | `0` or `1` — see the one-fd-per-message invariant below |

The header is followed by the argument payload. Because `fd_count` lives in
the header, a receiver can drop any frame it cannot interpret and still consume
and close its accompanying fd without consulting the schema.

### 2.2 The seven argument types

The argument type set is **closed at seven**. There are no arrays and no
64-bit scalars in version 0.

| Type | Wire encoding |
|---|---|
| `int` | signed 32-bit, little-endian |
| `uint` | unsigned 32-bit, little-endian |
| `fixed` | signed **24.8** fixed-point (32-bit: 24 integer bits, 8 fraction bits) |
| `string` | `u32` byte length, then that many UTF-8 bytes (**no NUL terminator on the wire; embedded NUL forbidden**), then zero padding to the next 4-byte boundary |
| `object` | `u32` object id (may be `0` only where `allow-null` is declared) |
| `new_id` | `u32` object id the sender allocates for a newly created object |
| `fd` | **not in the frame body**; transferred out-of-band via `SCM_RIGHTS` and matched to the signature's fd argument **positionally** |

Notes:

- **string**: the length prefix counts UTF-8 bytes, not code points. Bad UTF-8,
  an embedded NUL, exceeding the documented byte bound, or malformed padding is
  fatal `invalid_argument`. Additional per-message control-character rules
  (e.g. `vitrin_actuator_text.type`) are enforced on top of this.
- **fixed** is used only on `vitrin_shim_seat` motion so that later server-side
  sub-pixel motion synthesis needs no signature change. Agent-facing pointer
  `move` stays `int` because agents address captured pixels.
- **fd** ownership always transfers to the receiver, which MUST close it after
  use; the sender closes its own copy after sending. Error paths MUST close any
  received fd (no leak on reject).

### 2.3 Per-argument string bounds

Every `string` argument documents a maximum byte length. A violation is fatal
`invalid_argument`.

| Interface.message | Argument | Max bytes |
|---|---|---|
| `vitrin_handshake.hello` | `identity` | 2048 (the SPIFFE-ID maximum: a 255-byte trust domain plus path) |
| `vitrin_handshake.hello` | `credential_type` | 32 |
| `vitrin_handshake.hello` | `credential` | 32768 |
| `vitrin_handshake.error` | `message` | 1024 (free-form debug text, never parsed) |
| `vitrin_principal.bound` | `identity` | 2048 |
| `vitrin_principal.get_realm` | `name` | 64 |
| `vitrin_realm.request_grant` | `resource` | 256 (allow-null) |
| `vitrin_actuator_text.type` | `text` | 4096 |
| `vitrin_shim_session.configure` | `realm` | 64 |
| `vitrin_shim_seat.text` | `text` | 4096 |
| `vitrin_launcher.launched` | `realm` | 64 (same bound as every other realm id, so it passes back through `get_realm` unchanged) |

### 2.4 The one-fd-per-message invariant

`fd_count` is `0` or `1`: **at most one file descriptor per message** is a
framing invariant, not merely a property of the current signatures. If the
header's `fd_count` disagrees with the target message's signature, or fds are
attached to a message that declares none, the connection dies fatal
`fd_violation`. This invariant is why future multi-plane/multi-fd needs (e.g.
dmabuf params, oversized credentials) arrive as **builder patterns that add one
fd per message** rather than as multi-fd messages — the one-fd rule never
becomes a wall (see Appendix A).

---

## 3. Object ids

Object ids are **per-connection `u32` values**.

| Range | Meaning |
|---|---|
| `0` | the **null** object; legal only for arguments marked `allow-null` |
| `1` | the **bootstrap** object, implicit at connect, never created by a message (`vitrin_handshake` on principal connections, `vitrin_shim_session` on shim connections) |
| `[2, 0xfeffffff]` | client-allocated ids |
| `[0xff000000, 0xffffffff]` | reserved to the server; **unused at every version so far** (no event carries a `new_id`, so the server never allocates — `vitrin_launcher.launched` names the realm it created by *string*, not by a server-allocated object) |

### 3.1 Watermark rule (strictly increasing, never reused)

Client-allocated ids MUST be **strictly increasing and are never reused**. The
server tracks an allocation **watermark**. An id at or below the watermark, an
id in the reserved server range, or an otherwise unknown/foreign id is fatal
`invalid_object`. Because ids are never reused, a stale reference is always
safely diagnosable and never aliases a live object.

### 3.2 Multi-`new_id` rule

When one request carries several `new_id` arguments (only
`vitrin_realm.request_grant`, which co-mints five: grant, consent, view,
pointer, text), those ids MUST be:

1. **distinct**,
2. **strictly increasing in argument order**, and
3. **all above the connection's allocation watermark**.

Any violation is fatal `invalid_object`.

### 3.3 No destructors; inert objects; tolerate-dead events

- **Zero destructors at every version so far.** Objects live for the
  connection. There is no `delete_id` machinery. Per-connection object
  population is bounded (O(grants + facets)).
- **Inert-object rule.** Objects derived from a grant become **inert** when the
  grant dies (expiry or revocation). Requests on inert objects are **refused
  recoverably** (`vitrin_grant.refused`), never fatally. This is the corollary
  of the error razor (§5): human revocation racing in-flight requests must not
  kill a well-behaved agent.
- **Tolerate-events-to-dead-objects rule.** The server MAY emit events that
  reference an object whose grant has died. Clients MUST tolerate and discard
  such events. This is safe forever precisely because ids are never reused.

### 3.4 Id exhaustion

A connection that allocates all client ids in `[2, 0xfeffffff]` (roughly
4.28 billion) without any means to reuse them dies fatal `resource_exhausted`
(the practical population is far below this; exhaustion signals a client bug
or attack — the same code covers the per-connection live-object cap and the
petition-rate ceiling, §5.2).

---

## 4. Ordering guarantee

Each direction of a connection is a **single ordered stream**:

- the server processes requests in the order received;
- the client receives events in the order sent;
- **ordering holds across objects** within the connection, not merely
  per-object.

This guarantee is **load-bearing**. It is what makes the following correct with
no additional machinery (no serials, no per-message acks):

- the **sync barrier** (`sync`/`done`, §6.4);
- the **`buffer_done` cookie protocol** (attach-order disposition);
- **pipelined capture pairing** (`capture_frame` replies pair in request
  order);
- a **threadless blocking client SDK** (send, then read until a terminal
  event).

Concretely: an event caused by request *N* is always delivered before the
`done` of any `sync` issued after *N* — with one carve-out: **petition-lifecycle
events** (`vitrin_grant.resolved`, `vitrin_consent.state`) wait on an unbounded
human consent delay and do **not** participate in this rule. A `done` confirms
that an earlier petition was registered and its consent initiated, never that
it resolved (§6.4).

---

## 5. Error taxonomy

### 5.1 The razor

> A failure is **FATAL** (the connection dies) when the client violated
> something it could have known — the grammar, the handshake order, or its own
> connection's object graph — or breached a documented per-connection resource
> bound (`resource_exhausted`: denial-of-service confinement, not a semantic
> judgement).
>
> A failure is **RECOVERABLE** (an event is delivered and the connection lives)
> when a well-formed request's authority or target changed underneath it:
> consent outcome, expiry, revocation, human preemption, a granted verb's rate
> ceiling, or shim death.

Corollary: once a grant is revoked or expired its objects go inert — requests
on them yield recoverable refusals, never `invalid_object` death — otherwise
human revocation racing in-flight requests would kill well-behaved agents.

### 5.2 Fatal errors

Fatal errors are **total**: the fuzz target (hostile bytes, fd bombs, forged
ids) maps every decode failure onto exactly one of these codes with a clean
connection death, and error paths always close any received fd.

**Delivery differs by connection class.** On a **principal** connection a fatal
error is carried by `vitrin_handshake.error(object_id, code, message)` and then
the connection closes; delivery is best-effort (backpressure deaths and
unframeable streams are closed without it). On a **shim** connection there is
**no fatal-error message**: a shim protocol violation is **log-and-close** — the
shim is a core-spawned disposable child and the core log is the debugging
channel.

The ten fatal codes (the `vitrin_handshake.error` enum; in version 0 this is
the only error enum, so all fatal codes are connection-global):

| Code | Value | Condition |
|---|---|---|
| `invalid_object` | 0 | unknown or foreign object id, id reuse at or below the watermark, reserved-range id, or a multi-`new_id` rule violation |
| `invalid_opcode` | 1 | opcode not defined for the interface at the negotiated version, **including other-class opcodes and a second `hello`** (`hello`'s opcode is defined only in the CONNECTED state) |
| `invalid_argument` | 2 | argument decode failure: bad UTF-8, embedded NUL, string over its bound, out-of-range enum value, forbidden control character, zero verbs in a petition, malformed padding |
| `oversized` | 3 | declared frame `size` below the 8-byte minimum, or a payload shorter than the size declares (the 65535-byte ceiling binds senders — a u16 cannot express more) |
| `fd_violation` | 4 | header `fd_count` disagrees with the signature, or unsolicited fds attached |
| `pre_handshake` | 5 | traffic before a first `hello` on a principal connection |
| `version_unsupported` | 6 | `hello` offered a protocol version the server does not implement — i.e. above its maximum, since additive growth means a server implements every version up to its maximum; downgrade is refusal, not negotiation |
| `auth_failed` | 7 | credential rejected: unknown identity, bad token, verifier failure, or `SO_PEERCRED` mismatch — never distinguished on the wire (uniform code, fixed message text, detail in the server log only, §7.1) |
| `internal` | 8 | server-side failure that poisoned the connection |
| `resource_exhausted` | 9 | a documented per-connection resource bound was breached: the petition-rate ceiling, the live-object cap, or object-id exhaustion — denial-of-service confinement, not a semantic judgement |

**Shim log-only conditions** (defined here, but delivered as log-and-close, not
as a wire code):

| Condition | Trigger |
|---|---|
| `invalid_buffer` | geometry inconsistent with the fd's actual size, a zero dimension, or a stride overflow in `attach` |
| `bad_order` | `damage` or `commit` against a surface that has never been attached, an unknown `buffer_id`, or re-attaching a dmabuf `buffer_id` that has not yet received `buffer_done` |
| `already_initialized` | a second `get_seat` on a session |

### 5.3 Recoverable errors

Recoverable failures are delivered as ordinary events and never close the
connection. There are two petition/use moments plus the shim fallback path.

**Petition outcomes** — `vitrin_grant.resolved(outcome, verbs, persistence,
expiry_ms)`, sent **exactly once per grant, ever**. On `granted` the trailing
arguments carry the *effective* authority the human chose (which may be
narrower than requested); on any other outcome they are zero.

**Use-time refusals** — `vitrin_grant.refused(verb, code, retry_after_ms)` from
the single enforcement chokepoint, covering capture, actuation and launch
alike. `retry_after_ms` is greater than zero only for `rate_limited`.
Which codes a given use can draw is not uniform: `preempted` and
`consent_held` are actuation-only, `capacity` is launch-only, and a launch is
never refused `no_surface` (a vacant realm is the state `realm_launch` exists
to leave). The single voice is the invariant; the applicable set is per-verb
and is stated on each facet's page.

**Shim fallback** — `vitrin_shim_surface.buffer_done(buffer_id, status)` with a
non-`released` status is the recoverable dmabuf-import-fallback path
(shim-side).

#### SDK typed-exception mapping

Each recoverable code maps to exactly one distinct typed SDK exception (or
success), so a blocking SDK can translate the wire directly.

| `outcome` | SDK result |
|---|---|
| `granted` (0) | success (authority active) |
| `denied` (1) | `GrantDenied` |
| `timed_out` (2) | `ConsentTimeout` |
| `unavailable` (3) | `RealmUnavailable` |
| `unsupported` (4) | `GrantUnsupported` |
| `busy` (5) | `Busy` |

| `refusal` | SDK exception |
|---|---|
| `not_granted` (0) | `NotGranted` |
| `expired` (1) | `GrantExpired` |
| `revoked` (2) | `Revoked` |
| `rate_limited` (3) | `RateLimited` |
| `preempted` (4) | `Preempted` |
| `consent_held` (5) | `ConsentHeld` |
| `no_surface` (6) | `NoSurface` |
| `internal` (7) | `OperationFailed` |
| `capacity` (8) | `AtCapacity` |

`capacity` is reachable only through `realm_launch`: it says the deployment is
at its realm limit, which is a **policy answer** rather than the server-side
failure `internal` names. `retry_after_ms` is 0 — the core cannot know when a
realm will exit — so it is not a rate-limit hint in disguise. It is also the
one refusal answered from **deployment-wide** state rather than from the
asking principal's own grant, and therefore a one-bit cross-principal side
channel a launch grant cannot be attenuated out of; see
[`vitrin_grant`](04-vitrin_grant.md#refusal) for the full statement and what a
deployment has to weigh before serving the verb.

### 5.4 Backpressure deaths

A peer that stops reading (send-queue overflow / slow reader) is killed
**without an error message**: the queue that would carry the error is the queue
that is full. Unframeable garbage that cannot be attributed to an object is
likewise closed silently. Both are logged server-side. Error-then-close is
best-effort; silent-close is legal where no addressable error can be delivered.

---

## 6. Delivery classification

Every request is **reply-bearing**, **fire-and-forget**, or a **structural
mint**. This classification, together with the ordering guarantee (§4), is what
lets the SDK stay single-threaded and blocking.

The structural mints are `get_realm`, `create_surface`, `get_seat`, and (since
version 2) `get_launcher`: the request only mints an object, so it is neither
reply-bearing nor refusable — no terminal event, no wire acknowledgement. A
malformed mint is a fatal object-graph error; a mint whose target is unknown
or vacant surfaces that on first *use* (e.g. petitions resolving
`unavailable`, or an inert facet refusing `not_granted`), never at mint time.

### 6.1 Reply-bearing requests

| Request | Terminal event(s) |
|---|---|
| `vitrin_handshake.hello` | `vitrin_principal.bound` (success) or fatal `auth_failed` / `version_unsupported` |
| `vitrin_handshake.sync` | `vitrin_handshake.done` |
| `vitrin_realm.request_grant` | `vitrin_grant.resolved` (delivered when the petition resolves — exempt from cross-request order, §4) |
| `vitrin_view.capture_frame` | `vitrin_view.frame_ready` **or** `vitrin_grant.refused(observe, …)` |
| `vitrin_launcher.launch` *(since 2)* | `vitrin_launcher.launched` **or** `vitrin_grant.refused(realm_launch, …)` |

**Exactly-one-terminal rule.** Every reply-bearing request receives **exactly
one** terminal event, in request order (petition resolution excepted — §4), and
such terminals are **never
coalesced**. For `capture_frame` the one-of pairing is forced by the type
system: `fd` arguments have no null form, so failure must be a distinct event.
`launch`'s pairing is the same shape for a different reason: a realm id has no
"no realm" value that would not also be a legal id, so failure is again a
distinct event rather than a sentinel string.

### 6.2 Fire-and-forget requests

`move`, `button`, `scroll` (`vitrin_actuator_pointer`); `type`
(`vitrin_actuator_text`); `attach`, `damage`, `commit`
(`vitrin_shim_surface`). These carry no reply. Their **refusals MAY be
coalesced**:

- at most one `refused(rate_limited)` per grant per bucket-refill window;
- at most one `refused` per grant per `(verb, code)` pair until a subsequent
  request on that grant succeeds.

### 6.3 Non-error, legal-but-noteworthy cases

Documented as *not* errors: `commit` with no new `attach` (a repaint),
out-of-bounds `damage` rectangles (clamped), actuation coordinates outside the
view (clamped), concurrent observers, and shim EOF.

### 6.4 The sync barrier idiom

`sync(cookie)` requests a `done(cookie)` that is sent only after every request
received *before* the sync has been processed and every event those requests
caused has been queued *ahead* of the `done`. Because actuations are
fire-and-forget and enforcement failures are events, a threadless blocking
client bounds failure discovery to **one round trip**:

```
def actuate_and_flush(actions, cookie):
    for a in actions:
        send(a)                 # fire-and-forget: move / button / type / ...
    send(sync(cookie))          # barrier
    while True:
        ev = read_event()       # single ordered stream (Section 4)
        if ev is refused:       # any refusal caused by an action above
            raise typed_exception(ev.code)   # Section 5.3 mapping
        if ev is done and ev.cookie == cookie:
            return              # all prior actions processed, no refusal seen
        # else: dispatch other events (frame_ready, resolved, ...) and continue
```

The barrier costs no id churn: the cookie is an echoed value, not a callback
object. `sync` is valid only after `bound`, and there is no `sync` on shim
connections.

One exemption: petition-lifecycle events (`resolved`, `vitrin_consent.state`)
wait on human consent and do not participate in the barrier. A `done` confirms
that a preceding `request_grant` was processed — the petition registered, its
consent initiated — but does **not** wait for that petition to resolve, so a
blocking client reads `resolved` on its own schedule without deadlocking
against an unrelated pending prompt.

---

## 7. Handshake and versioning

### 7.1 Principal handshake state machine

```
                 (traffic before first hello)
                 ---> error(pre_handshake) --> close   [best-effort if framed]
                 (unparseable garbage)
                 ---> close silently (logged)
                 (no complete hello within the unauthenticated deadline)
                 ---> close administratively (logged; no error event)

   [CONNECTED] --hello--> [VERIFYING] --verify ok--> bound --> [BOUND]
                              |                                    |
                              | verify fails                       | (steady state:
                              v                                    |  requests processed,
                    error(auth_failed |                            |  sync/done, grants)
                    version_unsupported) --> close
```

- In **CONNECTED** the only legal message is `hello`. Any other traffic before
  a first `hello` is fatal `pre_handshake` (delivered best-effort if the frame
  parsed; unparseable bytes are closed silently). Either way the reason is
  logged.
- After `hello` the client **MAY pipeline** further requests; the server
  **queues them during credential verification** and processes them only after
  `vitrin_principal.bound` is sent — queued and then served, never dropped,
  when verification succeeds. The queue rule scopes to requests **outside the
  handshake exchange itself**: in version 1 the exchange is `hello` alone, and
  a later version's proof-of-possession response request (Appendix A) is part
  of the exchange, processed inside VERIFYING. If verification fails, the
  connection dies (`auth_failed` or `version_unsupported`) and the **queued
  requests are never processed**.
- `bound` carries the **verifier-canonical** identity, not an echo of the
  client's claimed string.
- `hello` is legal **exactly once** per connection: its opcode is defined only
  in CONNECTED, so a second `hello` — in VERIFYING or BOUND — is fatal
  `invalid_opcode`.
- **Checks run in a fixed order**: frame grammar, then the version integer,
  then — only for a version-accepted `hello` — credential verification.
  `version_unsupported` therefore reveals nothing about the credential or the
  claimed identity.
- **A refused handshake is uniform on the wire** (identity-probing
  resistance): every credential-rejection cause — unknown identity, bad
  token, verifier failure, `SO_PEERCRED` mismatch — collapses to the single
  code `auth_failed`, and its `error.message` MUST be a fixed phrase that
  neither names the cause nor echoes the claimed identity. Cause and claimed
  identity go to the server log only; verification SHOULD take uniform time
  across rejection causes.
- **The unauthenticated phase is time-bounded**: the server SHOULD impose a
  deployment-configurable deadline on every unauthenticated interval spent
  waiting on the client; a connection that exhausts it is closed
  administratively — no error event, reason logged. In version 1 that interval
  is exactly CONNECTED (after a complete `hello`, remaining VERIFYING latency
  is the server's own); a later version's proof-of-possession exchange keeps
  the deadline armed inside VERIFYING while the server awaits the client's
  response (Appendix A). A BOUND connection may idle indefinitely.
- The `credential` is secret material: the server MUST NOT write credential
  bytes into logs, `error.message` text, or the flight recorder — at most
  `credential_type` and the byte length may be recorded.

### 7.2 Shim "handshake"

There is no shim `hello`. Authentication is structural (the inherited
socketpair). The core's **first message** on a shim connection is
`vitrin_shim_session.configure(realm, width, height)`, guaranteed to precede
the processing of any shim request; the shim performs one synchronous read at
startup. **The shim's protocol version is pinned at spawn** — the core and its
shims are release-paired, so there is no shim version negotiation.

### 7.3 Version semantics

The protocol version is a **single integer**: the `version` attribute of the
protocol document *and* the first argument of `hello`. The version named in
`hello` is the **negotiated version**: the connection speaks exactly that
version for its whole life, and messages introduced by a later `since` are not
defined on it (using one is fatal `invalid_opcode`).

A server accepts any offered version it **implements** and refuses the rest
with fatal `version_unsupported`. Because growth is strictly additive, a
server whose maximum version is N implements every version from 1 to N —
serving an older version means never emitting later-`since` events and
rejecting later-`since` opcodes — so refusal means exactly that the client
offered a version **above the server's maximum**. Downgrade is refusal, not
negotiation: the server never counters with a different version and the
refusal carries no supported-version hint (`error.message` is never
machine-parsed); a newer client willing to speak an older version reconnects
offering a lower integer — convergence by descending reoffer, bounded by the
client's own maximum.

**The two versions, and what separates them.** Version 1 is the original wire.
Version 2 appends the `realm_launch` verb bit, `vitrin_grant.get_launcher`,
and `vitrin_launcher`'s `launch`/`launched` — nothing else. A version-1
connection is served exactly as before: it never sees a `since="2"` event, and
sending a `since="2"` opcode on it is fatal `invalid_opcode`. One thing is
deliberately *not* version-gated: a **verb bit**. The `verb` bitfield is a
single mask checked identically on every negotiated version, so a version-1
connection may petition for `realm_launch` and is answered `unsupported`
rather than killed — which is the entire reason a bit is defined before it is
served (§5.1's razor: a well-formed request the deployment will not serve is
recoverable).

> **Implementation status, stated rather than implied.** The rule above binds
> the *protocol*. The shipped core does not yet implement it: `vitrind`
> accepts exactly its maximum version and refuses everything else, so today it
> serves version 2 alone and answers a version-1 `hello` with fatal
> `version_unsupported`. That is a gap between this document and the
> implementation, not a second reading of the document — serving 1 and 2
> concurrently is WS-E.1.1's core half and has not landed. Nothing outside the
> repo speaks version 1, so the gap costs no deployed client; it is recorded
> here so nobody reads "a server whose maximum is N implements every version
> from 1 to N" as a description of what runs today.

### 7.4 Growth rules (Wayland-style)

- **New messages** are appended with `since` attributes. Opcodes are implicit
  document order; **requests and events are numbered separately from 0**.
- **Enum entries** are appended (values are immutable; a `deprecated-since`
  mark never removes). Entries carry **no `since`**: an enum's wire validation
  is one mask or membership table with no version dimension, so a
  version-gated entry would be accepted at every version regardless — the
  scanner rejects `since` on an `enum` or `entry` for exactly that reason.
- **A message signature is immutable forever** — extension is a *new* message,
  never a changed one.
- **Interface `version`** carries per-interface growth; the protocol-level
  version integer governs the wire/handshake compatibility. A message's
  `since` names the **protocol document's** version integer, not the
  interface's own counter — every seam in Appendix A is a `since="2"` message
  on an interface whose own `version` was 1 when the seam was written.
- **A verb bit's value is allocated once, repo-wide, and is immutable.** The
  gap between 32 and 512 in the `verb` bitfield is reserved allocation, not
  free space: a bit absent from the IDL may still be spoken for. The registry
  is `docs/plan/02-phase-2-semantic-epochs.md` §5, and anything adding a verb
  allocates there first, whatever document schedules the work.

---

## 8. The XML dialect and schema

The IDL is a stricter subset of the Wayland protocol XML shape, formalized by a
RELAX NG schema (`protocol/vitrin-v0.rng`) and validated with
`xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml`.

### 8.1 What the RNG enforces

- **Element structure**: `protocol` → optional `copyright`, required
  `description`, one-or-more `interface`; each `interface` holds `description`
  then `request`/`event`/`enum`; enums hold `description` then one-or-more
  `entry`.
- **Closed argument type set of seven** (no `array`).
- **Typed `new_id` and `object`**: both MUST name their `interface` (no untyped
  `new_id`; codegen stays a straight-line table). Multiple typed `new_id`
  arguments per request are permitted (this is what `request_grant` needs).
- **`allow-null`** is legal only on `string` and `object` arguments.
- **`enum` references** are legal only on `int` and `uint` arguments.
- **Descriptions required** on the protocol, every interface, and every
  request/event/enum; every enum `entry` **and every argument** carries a
  `summary`, and every `string` argument's summary carries the
  machine-readable `(max N bytes)` bound token (schema-enforced; CI
  additionally lints that the token appears exactly once).
- **Enum entry values required** (decimal or `0x` hex).
- **Structural rule B2**: `vitrin_shim_seat` is a specialized interface that
  defines **no requests**, and **every one of its events ends with the `origin`
  argument** (`type="uint" enum="origin"`). This makes the per-event
  origin-tagging invariant machine-checkable rather than conventional.

The negative-mutation corpus `protocol/test-mutations.sh` regression-tests
these rules: each case applies one illegal mutation to a copy of the IDL and
asserts the schema rejects it.

### 8.2 The two extension attributes

The dialect adds exactly two attributes beyond the Wayland shape:

| Attribute | Where | Why |
|---|---|---|
| `protocol/@version` (`positiveInteger`) | root element | single source of truth for the `hello` version integer |
| `interface/@verb` ∈ {`observe`, `actuate_pointer`, `actuate_text`, `realm_launch`} | `vitrin_view`, `vitrin_actuator_pointer`, `vitrin_actuator_text`, `vitrin_launcher` | declares that **every request on the interface exercises the named grant verb**; the scanner derives the enforcement chokepoint's `(interface, opcode) → required-verb` table from it |

`@verb` is the codegen chokepoint: one attribute per capability interface
generates the single-site authority check, so there is no second enforcement
location to keep in sync.

The `@verb` value set is **closed by the schema**, so widening it is a
**dialect** change: `protocol/vitrin-v0.rng` moves in the same commit as the
IDL, and `xmllint --relaxng` gates the pair. The set tracks the
*facet-bearing* verbs, not the whole `verb` bitfield — `observe_cursor`,
`layout_arrange` and `layout_focus` have no interface to annotate, so naming
one here is rejected by the schema (`protocol/test-mutations.sh` covers both
directions: an invented verb name and a real-but-facetless one).

### 8.3 Scanner lints

Beyond schema validation, the CI scanner enforces:

- **append-only opcodes** (document order is the opcode; reordering or
  inserting a message is a breaking change and is rejected);
- signatures expressible in both Rust and the generated C header (no exotic
  types);
- the round-trip property `encode(decode(bytes)) == bytes` (canonical encoding:
  fixed argument order, explicit enum values, deterministic padding,
  length-prefixed strings, no optional wire fields).

---

## Appendix A — Additive-safety table

Every version-2+ growth seam named in the IDL is listed with its arrival
mechanism and why it is **purely additive** (no existing signature changes, no
existing behavior breaks). Version 1 shipped none of these; they exist as
documented seams so the wire never changes shape when they arrive. A seam that
has **landed** says so in its row and stays in the table — the row is the
record of *how* it arrived, which is what a later seam copies.

| Seam | Arrival mechanism | Why purely additive |
|---|---|---|
| **realm launch** *(landed at version 2)* | new `since="2"` structural mint on `vitrin_grant` (`get_launcher`), a new `vitrin_launcher` interface carrying reply-bearing `launch` + terminal `launched`, a new `realm_launch` verb bit (512), and a new `capacity` entry on `refusal` | `request_grant`'s five `new_id` arguments are frozen, so the facet is minted on the grant instead — the same route the layout facet is documented to take. The verb bit appends without touching existing bits and the refusal entry appends without touching existing values. Nothing on `vitrin_realm` changed: launch is a **grant verb**, never a request on the authority-free realm handle, because holding a *name* must not confer the power to start a process. The command never crosses the wire — `launch` has no arguments; the realm names a template and the template names the program — so a launch grant is authority over operator-written configuration, never over an arbitrary command line |
| `grant.release` + tombstone rule | new `since="2"` destructor request on `vitrin_grant` | new opcode appended; the tombstone rule (clients discard events to released ids) is the existing tolerate-dead-events rule (§3.3) applied to a new id state — no signature changes |
| `revoked` push + `resolved`-exactly-once pinning | new `since="2"` event on `vitrin_grant` | `resolved` still fires exactly once ever; `revoked` is a *different* event and `refused` remains the enforcement-bearing signal, so no existing event double-fires |
| `attenuate` (narrower child grants) | new `since="2"` request minting a child grant | new opcode + new co-minted ids follow the existing multi-`new_id` rule; parent grant unchanged |
| `restore_token` | new `since="2"` request/argument path | version 0 has no grant persistence at all; adding durable rungs' restore path touches no version-0 message |
| epoch-staleness refusal sibling | new `since="2"` event on `vitrin_grant` carrying the current epoch | `retry_after_ms` cannot express an epoch, so this is a new event, not a changed `refused`; compare-and-swap actuation layers on top |
| `set_constraint` builder | new `since="2"` request preceding `request_grant` | value-bearing constraints (e.g. focus conditions) arrive as a builder request; boolean constraints use reserved `flags` bits — `request_grant`'s signature is frozen |
| focus event | new `since="2"` tagged event on `vitrin_shim_seat` | version 0 synthesizes focus shim-side (single-surface); the new event still ends with `origin`, satisfying B2 structurally |
| keymap relay + keycode event | new `since="2"` `keymap(fd, size, origin)` + keycode event on `vitrin_shim_seat` | keysym `key` events stay valid; raw-scancode fidelity is added alongside, one fd per message (one-fd rule holds) |
| `hello_fd` credential sibling | new `since="2"` fd-borne request on `vitrin_handshake` | `hello`'s signature is frozen forever; oversized credentials arrive via a sibling carrying one fd, so the 32768-byte in-frame bound is never a wall |
| proof-of-possession credential exchange | new `since="2"` challenge event + response request on `vitrin_handshake` | version-0 schemes are bearer-shaped (presented whole in `hello`); a `credential_type` demanding proof of possession (e.g. X.509-SVID) adds a server-driven exchange inside VERIFYING as appended messages — the exchange is part of the handshake itself, so the response request is exempt from the queued-until-BOUND rule (which scopes to non-handshake requests, §7.1) and the unauthenticated deadline stays armed while the server awaits the response; `hello` and `bound` stay frozen, and version-1 connections and bearer schemes never see the new messages |
| dmabuf params builder | new `since="2"` builder on `vitrin_shim_surface`, one fd per add | `attach` stays single-plane linear (no modifier argument to fail to honor); explicit modifiers / multi-planar formats accumulate fds across messages, preserving the one-fd rule |
| `frame_ready` `flags` bits | reserved bits in the existing `frame_flags` bitfield | a later zero-copy dmabuf handoff sets a flag on the *same* `frame_ready` message; `flags` is always 0 in version 0, so setting a bit is additive |
| capture streaming | new `since="2"` sibling messages on `vitrin_view` (a subscription request and its frame-push event, appended after the poll pair) | `capture_frame`/`frame_ready` stay valid forever; refusals still voice through `vitrin_grant.refused`, and each pushed frame carries one fd, so the one-fd rule holds |
| realm enumeration events | new `since="2"` events on `vitrin_realm` | `vitrin_realm` is authority-free and carries no version-0 events; multi-realm phases add enumeration/lifecycle here instead of re-plumbing addressing |
| drag intents | new `since="2"` sibling requests on `vitrin_actuator_pointer` | intent-level motion (drag with duration/easing, interpolated server-side) is added beside `move`/`button`/`scroll`, which stay valid forever |
| `actuate_key` verb | new appended entry in the `verb` bitfield + a later key-actuation facet | version-0 verb bits are untouched; a new power-of-two bit and its facet are additive — see the landed `realm_launch` row for the shape, including that the bit's *value* comes from the repo-wide allocation registry rather than from the next unused-looking power of two |
| serving `observe_cursor` | no new message: the verb bit already exists and widens what the *existing* `frame_ready` composites for a grant that holds it | version 0 refuses the verb `unsupported`, so beginning to serve it changes no signature and no version-0 client's behavior (a client that never petitions for it sees nothing new) |
| layout facet | new `since="2"` structural mint on `vitrin_grant` (`get_layout`), exercising the already-defined `layout_arrange`/`layout_focus` bits | `request_grant`'s five `new_id` arguments are frozen, so the facet cannot be co-minted; a mint on the grant follows the existing structural-mint class (no terminal event, not refusable) and the verbs are refused `unsupported` until it lands |
| per-principal pointer delivery | new `since="2"` sibling events on `vitrin_shim_seat` that name the principal alongside the coordinates | `motion`/`button`/`scroll` signatures are immutable, so delivery grows by sibling; each new event still ends with `origin`, satisfying B2 structurally, and a v0-only shim keeps working |
| IME physical text | reuse of the existing `origin` tag on `vitrin_shim_seat.text` | human input-method text arrives as `text` with `origin=physical`; the origin tag exists from day one, so the new source is additive with no signature change |

---

## Scope note — what version 0 deliberately does not carry

Version 0 is intentionally narrow. The following are **out of scope** and are
named here so their absence is understood as a decision, not an omission:

- **Semantic trees** — no accessibility/DOM-like node graph; observation is
  pixels only (`node:` resource prefixes are reserved but unserved).
- **Streaming capture** — observation is poll-only: one `capture_frame`, one
  frame. A push/subscription model is a later version's sibling messages on
  `vitrin_view` (see Appendix A), never a change to the poll pair.
- **Epochs** — no compare-and-swap staleness detection; out-of-view coordinates
  are clamped, and stale-observation detection is a later phase's epoch
  mechanism.
- **Network** — the protocol is local (Unix domain sockets); no remoting.
- **Multi-realm** — version 1 serves exactly one well-known realm (`realm-0`).
  Version 2 raises the *count*: a deployment may serve `realm-0` plus further
  realms up to a limit of its own choosing (see
  [`vitrin_realm`](03-vitrin_realm.md#realm-cardinality-one-at-version-1-a-bounded-set-at-version-2)).
  What stays deferred is everything *around* that count. **Realm enumeration
  and lifecycle events** are absent at any verb set, so the further names are
  undiscoverable on the wire and `realm-0` remains the one name a conformant
  client can know without being told. Realm *creation* is on the wire as the
  `realm_launch` verb and the [`vitrin_launcher`](16-vitrin_launcher.md)
  facet, but **no deployment serves it yet**: the verb is admitted and refused
  `unsupported`, exactly as `observe_cursor` and the layout verbs are. What is
  decided now, and would otherwise be unstateable, is the *shape* — launch is
  an attenuable grant verb rather than a request on the realm handle, and the
  program name is never on the wire. **Stopping** a realm is not expressible at
  all.
- **Powerbox** — no system-mediated resource picker; petitions name resources
  by a type-prefixed string vocabulary.
- **Wallet** — no credential/secret storage or presentation verb.
- **Layout** — the core has no window manager, and version 0 serves neither
  `layout_arrange` nor `layout_focus`; both are defined on the wire and refused
  `unsupported` (§1.4, D-018). What is *not* deferred is the posture: layout is
  authority, not decoration, and the ordering invariants bind the core from day
  one — though only the first has anything to bind *against* today, and none is
  yet tested as an invariant (§1.4, "their standing today").
- **Per-principal cursor delivery** — version 0 delivers one shared pointer
  position per realm view; per-principal **delivery** is deferred to M2 and
  arrives as sibling `vitrin_shim_seat` events (§1.4, D-017). What is *not*
  deferred, since D-019, is **drawing**: the core composites each actuating
  agent's own cursor into human-visible output, from a position only that
  agent's motion moves. The two are independent — a per-agent sprite over a
  shared delivered position is exactly what v1 ships — so the deferral above
  is unchanged rather than partially closed. The *model* — one pointer per
  principal, cursorless by construction, core-composited, visibility as a
  verb — is decided and on the wire now.

Finally: the **human principal has no wire presence** in version 0. Host input
in nested mode is the implicit human (tagged `origin=physical` on the shim
seat), and only agents handshake. Physically-originated consent — built on the
day-one `origin` tag — is a later phase. One consequence worth naming: the
human→agent cursor-visibility toggles (§1.4) are a shell and core concern, not
an agent-expressible one, precisely because the human is not on the wire.

---

## License

This page and every other page under `docs/protocol/` are prose describing
`protocol/vitrin-v0.xml` (Apache-2.0) and are themselves licensed under
[CC BY 4.0](../../LICENSE-CC-BY-4.0), per decision D-005. See the repository
root [`NOTICE`](../../NOTICE) for the full license split.
