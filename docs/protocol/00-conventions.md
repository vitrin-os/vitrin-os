# Vitrin protocol — conventions (normative)

This is the normative reference for the Vitrin OS wire protocol, version 0
(the `hello`/document version integer starts at `1` and is `2` today). Every
interface page links here; this page defines the rules those pages assume.
Where this page and the IDL (`protocol/vitrin-v0.xml`) disagree, **the IDL
wins** — its `<description>` text is the source of truth and this page
restates it.

Two wire versions exist. Version 2 appends, and changes nothing else — every
version-1 signature is byte-identical at version 2:

- the `realm_launch` verb bit, the structural mint
  `vitrin_grant.get_launcher`, and the
  [`vitrin_launcher`](16-vitrin_launcher.md) interface it mints;
- the structural mints `vitrin_grant.get_layout_focus` and
  `vitrin_grant.get_layout_arrange`, and the two interfaces they mint —
  [`vitrin_layout_focus`](17-vitrin_layout_focus.md) and
  [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) — through which the
  already-allocated `layout_focus` and `layout_arrange` verbs are exercised;
- the structural mint `vitrin_grant.get_egress` and the
  [`vitrin_egress`](19-vitrin_egress.md) interface it mints, through which the
  already-allocated `egress` verb is exercised — a verb **no deployment
  serves**, because the out-of-core proxy behind the facet does not exist;
- the `layout_held` entry on `vitrin_grant.outcome`.

Those four are the version-2 additions that mint an **interface page**, which
is why they are the ones listed beside the page index above. They are **not** the
whole of version 2 — the cross-realm clipboard, the pointer constraint, the idle
inhibit and the seat's relative-motion and gesture events all landed at version 2
on interfaces that already existed. The complete, normative enumeration is
[§7.3](#73-version-semantics), and this list deliberately does not duplicate it:
two closed lists of the same facts is how one of them goes stale.

Statements below that say "version 1" are statements about version 1 and
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
- [17 — vitrin_layout_focus](17-vitrin_layout_focus.md) *(since version 2)*
- [18 — vitrin_layout_arrange](18-vitrin_layout_arrange.md) *(since version 2)*
- [19 — vitrin_egress](19-vitrin_egress.md) *(since version 2; served by no deployment)*

The gap from 11 to 16 is deliberate: pages 12–15 are allocated to interfaces
that have not landed yet (`docs/plan/02-phase-2-semantic-epochs.md` §5), and
taking an allocated number would be the collision that registry exists to
prevent. Page 19 was allocated by the same registry and is taken here rather
than page 12: `vitrin_egress` landed before the four interfaces below it, and
a page number is a permanent address, not a queue position.

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
   single carve-out and is itself grant-governed rather than free;
5. the **human's own physical input** reaches only the realm the output is
   bound to.

Rule 5 joined the set when `layout_focus` became servable, and it is the
reason that verb is **one act**. Splitting "which realm is shown" from "which
realm receives the human's keys", in a scene that shows one realm at a time,
would let a holder make a human type into a realm they cannot see. An agent's
*injected* input is not governed by this rule: it is addressed to the realm
its own grant names and carries the `emulated` origin tag. (D-018(2)
enumerated four; this fifth is appended by a superseding decision-log entry
rather than by editing that one.)

**Their standing today, stated rather than implied.** D-018's cost note said
of these rules that "none of the four is tested *as an invariant* against a
client trying to violate it, and none can be until something outside the core
can arrange realms". Serving the layout verbs is that moment, and the tests
now exist. Each drives a real client over a real socket, holding **every verb
the reference core serves**, through the whole production path
(`request_grant` → consent → `get_layout_*` → `focus`/`set_fullscreen` → the
enforcement chokepoint → the presenter), and sweeps the *entire* arrangement
space those two verbs can express — which is finite and small precisely
because `layout_arrange` defines one request. A property proved at the maximum
verb set is a property of *no grant*, not of the grant a test happened to
construct.

- **Invariant 1 and 3** — `session.rs`'s
  `no_arrangement_at_the_maximum_verb_set_can_touch_the_consent_card`: with
  another principal's prompt on screen, every arrangement leaves the trust
  band's colour at pixel (0,0) and the consent card byte-identical, row by
  row, to an independently rasterized card. What enforces it is *where* the
  overlay composites — at the output stage, downstream of the scene an
  arrangement selects — so it is structural rather than a check on the way
  past. Still also exercised on real backend pixels by `backend/headless.rs`'s
  `a_prompt_reaches_human_visible_output_but_never_a_capture`.
- **Invariant 2** — `the_core_not_the_client_decides_which_surface_input_reaches`,
  in two halves. There is no stacking to claim: neither served facet defines
  any request beyond the one it ships, and the test pins that, so a `place` or
  `raise` appended before the scene can honour it turns red. And what a holder
  *can* move, it moves wholly: across the sweep, the realm the output shows and
  the realm the human's input reaches are never two different realms.
- **Invariant 4** — `no_arrangement_puts_an_agent_cursor_into_any_realms_capture`:
  one principal actuates (driving a real cursor sprite through a real
  `vitrin_actuator_pointer.move`), a *second* principal observes another
  realm, and across the sweep both principals' captures — read back from the
  sealed memfd, not from a readback beside it — stay byte-identical to their
  own bare scene. What enforces it is that the sprite is composited at the
  output stage (D-019(3)); the test is what stops that drifting.
- **Invariant 5** — `the_humans_input_follows_the_realm_a_focus_holder_bound`:
  before the request the output and the seat are both on the first realm,
  after it both are on the granted one.

Two limits stated rather than left to be found. These are **component** tests
against a real socket and real forked shims in-process, not mock-free
integration gates; and invariant 4's agent→agent half remains unpurchasable
*by construction* rather than by test, because agent-to-agent observation has
no wire surface to try it through — no agent can name another's grant.

The shell gets *arrangement*; the core keeps *ordering*. Read the ordinary
window-manager verbs as authority rather than decoration and the reason is
plain: `raise` would put a surface over the consent card, `move` would slide a
different target under the pointer mid-decision, `fullscreen` would impersonate
the whole session. Invariants 1–3 are what make those attack primitives
unpurchasable, which is also why `raise`, `move`, `resize`, and `fullscreen`
are **not** separate verbs — with ordering held by the core, splitting them
further would be attenuation theatre.

**What `layout_arrange` actually says, and what it deliberately cannot.** The
two verbs are served through two facets,
[`vitrin_layout_focus`](17-vitrin_layout_focus.md) and
[`vitrin_layout_arrange`](18-vitrin_layout_arrange.md), and between them they
define exactly two requests: `focus` and `set_fullscreen`. There is **no**
`place`, `resize`, `raise` or stacking request at all — not a request that
refuses, but no request. A scene that shows one realm at a time, unstacked and
unoverlapped, cannot honour them, and a granted verb whose requests the server
cannot carry out is the "a deployment MUST NOT grant a verb it does not
enforce" rule turned inside out. Growth arrives when the scene can honour it,
as `since`-gated siblings, never as a request that silently does less than its
name.

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

**What a deployment actually serves.** Whether a defined verb is served is a
property of the deployment, not of the wire: a client must read `unsupported`
as *"not here, not now"*, never as *"not in this protocol"*. The reference
core serves both layout verbs as of WS-E.1.4 — each has a facet interface, an
enforcement-chokepoint arm and consent-prompt copy — and still refuses
`observe_cursor`, because per-principal cursor *delivery* does not exist
(D-017, D-019). It **does** composite an agent principal's own cursor, and
only into human-visible output — never into a captured frame — at the same
output stage as the consent overlay and the trust indicator; nested mode draws
it always, and `--headless --agent-cursor` on request (the headless
human-visible framebuffer is otherwise measured byte-for-byte against the realm
view by the trusted-band witness). **Whether a *human* cursor is composited
depends on who owns the display**, and the answer changes nothing above: in
nested operation the core composites none, because the host desktop draws it
outside the realm view entirely; on bare metal, where there is no host desktop,
the core draws the human's pointer itself, or the human has none. This paragraph
said flatly that no human cursor is composited until WS-E.4.2 corrected it; the
IDL (`vitrin_view`'s description) had been nested-conditional since WS-E.3.2,
and prose restating the IDL is the rule, so the prose was the surface that was
wrong. Where the core *does* draw that sprite it also **hides** it while a
pointer constraint is active on the bound realm
([`vitrin_shim_session.pointer_constraint_state`](09-vitrin_shim_session.md#pointer_constraint_state-since-2)),
which is the one case in this protocol where an app's ask changes what the human
sees of their own pointer — and is precisely why the hiding, and the un-hiding
on every path that ends a constraint, are the **core's** and not the app's.
Delivery is a separate thing from drawing, and delivery has not changed:
version 1 still
delivers **one shared pointer position** per realm view to the shim, and
per-principal delivery stays deferred to M2 (D-017, D-019). Of the three verbs this section governs, `layout_arrange` and `layout_focus`
are **servable** — each has a facet interface, and the reference core serves
both — while `observe_cursor` is refused `unsupported` by every deployment,
because the delivery it would widen a capture with does not exist.
`realm_launch` was in that same staged posture and no longer is: the reference
core serves it as of WS-E.1.1, though a deployment that will not host process
creation still refuses it. See
[§ defined but unserved](04-vitrin_grant.md#defined-but-unserved) for why
defining a verb before serving it is structural rather than cosmetic, and for
why "unserved" is a statement about a deployment rather than about the wire.

**One more verb sits in that staged posture and is not governed by this
section at all**, named here because the enumeration above is the one a reader
checks a claim about "which verbs are unserved" against. **`egress`** (128,
added at P2.7.2) is refused `unsupported` by *every* deployment — not because
of what a deployment declines, but because the out-of-core mediating proxy an
outbound connection would be made through does not exist. Its facet does:
[`vitrin_egress`](19-vitrin_egress.md) landed in the same task's second half,
so the request to ask through is on the wire and the mechanism to answer with
is not. Its authority is named by
the [`net:` resource selector](04-vitrin_grant.md#the-net-resource-prefix):
`covers` is exact match, so no accepted selector can ever name more than one
endpoint and a blanket egress grant is inexpressible rather than refused.

<!-- vitrin-verb-set: unserved-verbs = observe_cursor, egress | count: two -->
So the count this enumeration exists to answer is **two**: `observe_cursor`
and `egress` are the verbs no deployment serves today, and `layout_arrange`,
`layout_focus` and `realm_launch` have each left that posture. That count is
not a sentence anyone has to remember: `cargo xtask verb-sets --check`
derives the set from the IDL and from the reference core's `SERVED_VERB_BITS`
and fails on every surface still enumerating the old one.

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
| `vitrin_shim_session.selection` | `mime` | 32 (the one served value, `text/plain;charset=utf-8`, is 24) |
| `vitrin_shim_session.selection` | `data` | 61440 (the cross-realm clipboard cap: measured, and 4039 bytes clear of the frame ceiling — [D-024](../plan/20-decision-log.md)(5)) |
| `vitrin_shim_session.offer_selection` | `mime` | 32 |
| `vitrin_shim_session.offer_selection` | `data` | 61440 |
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
the single enforcement chokepoint, covering capture, actuation, launch, the
layout verbs and egress alike — a closed list of use classes, grouped by facet
shape rather than by verb, and whose two past lapses are recorded at
[`vitrin_grant.refusal`](04-vitrin_grant.md#refusal).
`retry_after_ms` is greater than zero only for `rate_limited`.
Which codes a given use can draw is not uniform: `preempted` and
`consent_held` are attention-shaped (actuation and the layout verbs),
`capacity` is launch-only, and a launch is
never refused `no_surface` (a vacant realm is the state `realm_launch` exists
to leave). The single voice is the invariant; the applicable set is per-verb
and is stated on each facet's page. One code is also **conditional**:
`preempted` on the two layout verbs is lifted while the human's attention
signal ([`vitrin_principal.attention`](02-vitrin_principal.md#attention),
version 2) is live for that principal — so an agent cannot reconstruct from its
own journal why one layout request landed and an identical one did not (see
[`vitrin_grant.refusal`](04-vitrin_grant.md#refusal)).

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
version 2) `get_launcher`, `get_layout_focus`, `get_layout_arrange` and
`get_egress` — **seven**: the
request only mints an object, so it is neither
reply-bearing nor refusable — no terminal event, no wire acknowledgement. A
malformed mint is a fatal object-graph error; a mint whose target is unknown
or vacant surfaces that on first *use* (e.g. petitions resolving
`unavailable`, or an inert facet refusing `not_granted`), never at mint time.

`get_egress` ([`vitrin_grant`](04-vitrin_grant.md#get_egress)) is the newest
and is a mint like the other six in every structural respect, including the
one that reads oddest: no deployment serves the `egress` verb, so *every*
facet it mints refuses `not_granted` on first use — and the mint is still
always legal, because minting is not an authority oracle. A separate and
larger gap belongs to the reference core rather than to this document: it
dispatches neither `get_egress` nor `vitrin_egress`'s own request at all, so
`vitrind` answers both `invalid_opcode` today where this section says the mint
is unrefusable. See [`vitrin_egress`](19-vitrin_egress.md#nothing-serves-this-interface-read-this-section-first).

### 6.1 Reply-bearing requests

| Request | Terminal event(s) |
|---|---|
| `vitrin_handshake.hello` | `vitrin_principal.bound` (success) or fatal `auth_failed` / `version_unsupported` |
| `vitrin_handshake.sync` | `vitrin_handshake.done` |
| `vitrin_realm.request_grant` | `vitrin_grant.resolved` (delivered when the petition resolves — exempt from cross-request order, §4) |
| `vitrin_view.capture_frame` | `vitrin_view.frame_ready` **or** `vitrin_grant.refused(observe, …)` |
| `vitrin_launcher.launch` *(since 2)* | `vitrin_launcher.launched` **or** `vitrin_grant.refused(realm_launch, …)` |
| `vitrin_egress.request_connect` *(since 2)* | `vitrin_egress.connected` **or** `vitrin_grant.refused(egress, …)` **or** `vitrin_egress.connect_failed` — three, see below |

**Exactly-one-terminal rule.** Every reply-bearing request receives **exactly
one** terminal event, in request order (petition resolution excepted — §4), and
such terminals are **never
coalesced**. For `capture_frame` the one-of pairing is forced by the type
system: `fd` arguments have no null form, so failure must be a distinct event.
`launch`'s pairing is the same shape for a different reason: a realm id has no
"no realm" value that would not also be a legal id, so failure is again a
distinct event rather than a sentinel string.

**The rule is exactly-one *terminal*, not one-of-two *events*, and
`request_connect` is where the difference shows.** The rows above already carry
terminal sets of three different sizes: `sync` and `request_grant` have exactly
one terminal each, `capture_frame` and `launch` one **of two**, and
`request_connect` one **of three**. Every one of them obeys the same rule —
one request, one terminal, drawn from that row's set. So this paragraph is
**not widened** by the egress facet: what the rule bounds is how many terminals
a request receives, which is still exactly one, and never how many the set it
is drawn from holds.

**Why there is a third here**, in short — the full argument is
[`vitrin_egress`](19-vitrin_egress.md#three-terminals-not-two)'s.
`vitrin_grant.refused` is the enforcement chokepoint's voice, and every code in
[`refusal`](04-vitrin_grant.md#refusal) names something a server **decided**
about a grant. A host that is down decided nothing, so routing it through
`refused` would make that event stop meaning *"authority was withheld"* — the
one thing it has to keep meaning. `connect_failed` therefore exists to keep
**"you may not"** and **"it did not work"** apart, and the ordering is
normative: the chokepoint answers first, so `connect_failed` is unreachable for
a principal whose grant does not cover the endpoint.

**The seam this opens, named rather than discovered later.** Clients handle
three terminals on this one request and at most two anywhere else. The general
rule that follows is: a later reply-bearing request whose failure is **not the
server's decision** adds *its own* terminal event on *its own* facet, and never
a new code in `refusal`. What it may not do is deliver a *second* terminal for
one request — that is the part of this rule that has not moved.

### 6.2 Fire-and-forget requests

`move`, `button`, `scroll` (`vitrin_actuator_pointer`); `type`
(`vitrin_actuator_text`); `attach`, `damage`, `commit`
(`vitrin_shim_surface`); `focus` ([`vitrin_layout_focus`](17-vitrin_layout_focus.md))
and `set_fullscreen` ([`vitrin_layout_arrange`](18-vitrin_layout_arrange.md));
`selection`, `pointer_constraint` and `idle_inhibit`
(`vitrin_shim_session`). Every message named after `commit` is *since 2*. These
carry no reply. Their **refusals
MAY be coalesced**:

- at most one `refused(rate_limited)` per grant per bucket-refill window;
- at most one `refused` per grant per `(verb, code)` pair until a subsequent
  request on that grant succeeds.

Three qualifications on the shim-connection trio, all stated rather than left to
be inferred. `selection` was **missing from this list** until WS-E.4.2 and is
added here with the message that noticed it; it has no refusal at all, so the
coalescing rule is vacuous for it. `pointer_constraint` **does** get an answer —
`vitrin_shim_session.pointer_constraint_state`, correlated by serial — but that
answer is not a terminal event and [§6.1](#61-reply-bearing-requests)'s
exactly-one-terminal rule deliberately does not apply to it: a constraint's
state changes for reasons the shim never asked about, so an answer bound
one-to-one to the ask would leave an app locked with no message able to tell it
otherwise. For that message the coalescing licence above is **overridden in the
strict direction**: the core sends at most one `pointer_constraint_state` per
transition and never coalesces two different states.
`idle_inhibit`, the third, needs no override in either direction: it carries no
serial and has no verdict event ([`vitrin_shim_session`](09-vitrin_shim_session.md)),
so there is no answer to coalesce and none to hold stricter.

This list is a **closed enumeration**, and it has now gone stale twice.
`selection` above records the first. The second is a later edit: `focus`,
`set_fullscreen` and `idle_inhibit` all landed at version 2 without arriving
here, and were added after the fact rather than in the change that shipped
them. What would catch the next omission without inventing a tool: §6's three
classes partition every `<request>` in `protocol/vitrin-v0.xml`, of which there
are **twenty-five** — six
reply-bearing ([§6.1](#61-reply-bearing-requests)), seven structural mints
([§6](#6-delivery-classification)), and the **twelve** named above. Those counts
are stated as numbers *and* as lists for the same reason the version table's
enum count is ([§7.3](#73-version-semantics)): counting `<request name=` in
`protocol/vitrin-v0.xml` and comparing it against the number is the cheapest way
for a reader to notice a drop, and nothing in CI does it for them.

**The third staleness was not in this list — it was in the other two classes,
and it is why the numbers above moved.** The egress facet added two requests,
`vitrin_grant.get_egress` and `vitrin_egress.request_connect`, and neither
reached §6: the mint list said six, [§6.1](#61-reply-bearing-requests)'s table
had no `request_connect` row, and the total here said twenty-three — in a
section that declares itself closed. The fire-and-forget twelve are the one
count that did not move, because nothing was appended to *them*.

The same shape a third time, so the sentence above about nothing in CI doing
this for the reader is a live cost rather than a caveat.
`cargo xtask verb-sets --check` — landed with this facet — holds every
enumeration of the **verb** sets in this repository, and holds **none** of the
three request counts in this paragraph. Extending it to them was considered and
is not done here: the three classes are a human judgement about each request's
delivery contract, not something derivable from the IDL, so a tool could check
the total against `<request name=` but could not check the split. Until
somebody writes that, the total is the number to recount.

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
Version 2 appends the `realm_launch` and `egress` verb bits and these
messages, and nothing else:

| interface | message |
|---|---|
| `vitrin_principal` | event `attention` |
| `vitrin_grant` | requests `get_launcher`, `get_layout_focus`, `get_layout_arrange`, `get_egress` |
| `vitrin_launcher` | request `launch`, event `launched` |
| `vitrin_layout_focus` | request `focus` |
| `vitrin_layout_arrange` | request `set_fullscreen` |
| `vitrin_egress` | request `request_connect`, events `connected`, `connect_failed` |
| `vitrin_shim_session` | events `request_selection`, `offer_selection`, `pointer_constraint_state`; requests `selection`, `pointer_constraint`, `idle_inhibit` |
| `vitrin_shim_seat` | events `relative_motion`, `gesture_begin`, `gesture_swipe_update`, `gesture_pinch_update`, `gesture_end` |

`egress` (128) appended **no message at all** when the bit landed, and it is
listed above with
`realm_launch` rather than beside them for that reason: a verb bit is not
version-gated (see below), so it is a version-2 addition to the *bitfield*
without being a version-2 addition to any signature. Its facet arrived
separately, in the same task's second half, and the table above carries it —
`vitrin_grant.get_egress` and the three messages of
[`vitrin_egress`](19-vitrin_egress.md). Both halves are version 2; a client
that negotiated version 1 sees neither the mint nor the facet, and may still
name the verb in a petition and be answered `unsupported`.

Plus, at the same version and for the same reason, nine enums that carry
arguments of those messages and are defined nowhere else:
`vitrin_egress.failure`,
`vitrin_shim_session.selection_status`,
`vitrin_shim_session.pointer_constraint_kind`,
`vitrin_shim_session.pointer_constraint_lifetime`,
`vitrin_shim_session.pointer_constraint_status`,
`vitrin_shim_session.idle_inhibit_state`,
`vitrin_shim_seat.gesture_kind`,
`vitrin_shim_seat.gesture_state` and
`vitrin_layout_arrange.mode`. Enum entries carry no `since` of their own
(see [§7.4](#74-growth-rules-wayland-style)), so the version a new enum belongs
to is recorded here or nowhere.

> **This paragraph had drifted twice, and both corrections are recorded rather
> than silently applied.** It read "`vitrin_grant.get_launcher`, and
> `vitrin_launcher`'s `launch`/`launched` — nothing else" until WS-E.1.7. That
> was already false before that issue touched it: WS-E.1.4 landed four more
> `since="2"` messages (`get_layout_focus`, `get_layout_arrange`, `focus`,
> `set_fullscreen`) without updating it, and WS-E.1.7 added a fifth
> (`attention`). **It then went stale a second time, in exactly the way the rule
> below was written to prevent**: WS-E.2.1's three cross-realm-clipboard
> messages on `vitrin_shim_session` landed without extending this table, and the
> omission stood until WS-E.4.2 came to append to it. Both rows are now present.
> A closed enumeration nobody re-reads while appending is a tripwire that only
> ever fires late — twice, so far — which is why the growth rules below say in
> as many words that this table is normative and must be extended in the same
> edit as any `since=` addition. WS-E.4.2's own two additions — the seat's five
> events, then the session's pointer-constraint pair and its **three** enums —
> were each made in the edit that landed them, which is what the rule asks for.
> Three enums in one change is the largest single addition this paragraph has
> taken, and therefore the likeliest to be dropped next time.
>
> **It did not go stale a third time by omission of a message.** Issue #306
> appended `vitrin_shim_session.idle_inhibit` and its `idle_inhibit_state` enum,
> and both rows above were extended in that same edit — the first addition since
> the rule below was written down that had the warning above already in front of
> it. The enum count in the paragraph *above* is stated as a number ("eight")
> *and* as a list on purpose: the number is what a reader checks the list
> against, and a disagreement between the two is the cheapest possible way to
> notice a drop.
>
> **It went stale a third time by omission of an enum, and the check fired with
> nobody acting on it.** The enum list has been short by one since the edit that
> created it: WS-E.4.2 (#255) built it out of the enums *that* change was
> landing plus the clipboard's, and never swept the enums already reachable from
> the message table above it. `vitrin_layout_arrange.mode` carries
> `set_fullscreen`'s only argument, is defined nowhere else, and landed at
> WS-E.1.4 (#210) — before the list existed, and before WS-E.1.7 (#232)
> put `set_fullscreen` into the message table it is checked against. So the
> count read "seven" over a list that should have held eight for the whole of
> its life, and the number-versus-list disagreement advertised one paragraph up
> as "the cheapest possible way to notice a drop" was sitting there to be
> noticed the entire time. It reads "eight" now, and the enum is listed. Cheap
> to notice is not the same as noticed: a check nobody re-runs has stopped being
> a check, which is the failure mode this whole block exists to document rather
> than the one it was written to prevent.

A version-1
connection is served exactly as before: it never sees a `since="2"` event, and
sending a `since="2"` opcode on it is fatal `invalid_opcode`. One thing is
deliberately *not* version-gated: a **verb bit**. The `verb` bitfield is a
single mask checked identically on every negotiated version, so a version-1
connection may petition for `realm_launch` (512) and is answered rather than
killed — `unsupported` there, because version 1 cannot mint the
[`vitrin_launcher`](16-vitrin_launcher.md) facet at all. A recoverable answer
standing where a fatal `invalid_argument` would otherwise fall is the entire
reason a bit is defined **before it is served** — and `egress` (128) is the
same staging carried one step further, in two moves rather than one: the bit
landed with no message at all, its facet landed next, and *every* deployment
still answers `unsupported` at every version because the proxy behind the
facet does not exist (§5.1's razor: a well-formed
request the deployment will not serve is recoverable). `realm_launch` was that
staging's worked example, and a briefer one than the argument implies: the bit
went on the wire in **WS-E.1.1**'s protocol half (#225), the reference core
refused every petition naming it `unsupported`, and WS-E.1.1's core half (#207)
served it two PRs later. It is served now — `observe_cursor` and `egress` are
the bits left standing in that posture, for two different missing mechanisms
and neither of them a missing facet any more — and the rule the example stood
for did not move with it: a deployment that does not serve a defined verb
answers `unsupported`, never a killed connection.

> **Implementation status, stated rather than implied.** The rule above binds
> the *protocol*. The shipped core does not yet implement it: `vitrind`
> accepts exactly its maximum version and refuses everything else, so today it
> serves version 2 alone and answers a version-1 `hello` with fatal
> `version_unsupported`. That is a gap between this document and the
> implementation, not a second reading of the document. **Serving 1 and 2
> concurrently is P2.1.2's version matrix**, not WS-E.1.1's: that issue landed
> the `realm_launch` half of version 2 without touching version acceptance,
> and half a version matrix would be worse than none. Nothing outside the repo
> speaks version 1, so the gap costs no deployed client; it is recorded here so
> nobody reads "a server whose maximum is N implements every version from 1 to
> N" as a description of what runs today.

### 7.4 Growth rules (Wayland-style)

- **New messages** are appended with `since` attributes. Opcodes are implicit
  document order; **requests and events are numbered separately from 0**.
- **The version enumeration in [§7.3](#73-version-semantics) is
  normative and MUST be extended in the same edit** as any `since=` addition.
  It is a closed list of what separates one protocol version from another, so a
  message appended without it makes the document state something false about
  the wire — and, being a list nobody re-reads while appending, it goes stale
  silently and is discovered late. It has gone stale **twice**, and both are
  recorded there rather than quietly fixed. A new **enum** belongs in that
  table too: entries carry no `since` (below), so there is no other place a
  reader can learn which version first defined one.
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
| `interface/@verb` ∈ {`observe`, `actuate_pointer`, `actuate_text`, `layout_arrange`, `layout_focus`, `realm_launch`, `egress`} | `vitrin_view`, `vitrin_actuator_pointer`, `vitrin_actuator_text`, `vitrin_launcher`, `vitrin_layout_focus`, `vitrin_layout_arrange`, `vitrin_egress` | declares that **every request on the interface exercises the named grant verb**; the scanner derives the enforcement chokepoint's `(interface, opcode) → required-verb` table from it |
<!-- vitrin-verb-set: facet-verbs = observe, actuate_pointer, actuate_text, layout_arrange, layout_focus, realm_launch, egress -->
<!-- vitrin-verb-set: facet-interfaces = vitrin_view, vitrin_actuator_pointer, vitrin_actuator_text, vitrin_launcher, vitrin_layout_focus, vitrin_layout_arrange, vitrin_egress -->

Both halves of that row went stale at WS-E.1.4 — which added
`vitrin_layout_focus` and `vitrin_layout_arrange` and left the row reading four
values and four interfaces — and stayed that way until issue #196's third
review, fifteen lines above a paragraph on the same page that contradicted it.
So neither half is transcribed by hand any more: `cargo xtask verb-sets
--check` derives both from `interface/@verb` and fails when this row falls
behind. It did exactly that on the very next change, which added
`vitrin_egress`: the gate named this row, the two markers under it and three
other surfaces, and none of them could be satisfied by rewording.

`@verb` is the codegen chokepoint: one attribute per capability interface
generates the single-site authority check, so there is no second enforcement
location to keep in sync.

The `@verb` value set is **closed by the schema**, so widening it is a
**dialect** change: `protocol/vitrin-v0.rng` moves in the same commit as the
IDL, and `xmllint --relaxng` gates the pair. The set tracks the
*facet-bearing* verbs, not the whole `verb` bitfield, so it is **shorter than
the bitfield by one**:

<!-- vitrin-verb-set: facetless-verbs = observe_cursor | count: one -->

- **`observe_cursor` has no interface to annotate, by construction.** It
  widens what `capture_frame` composites rather than adding a request, so
  there is nothing for the attribute to sit on and there never will be.

It was shorter by **two** until the egress facet landed. `egress` had no
interface *yet* — P2.7.2's first half put the verb bit on the wire with no
message at all — and its second half added
[`vitrin_egress`](19-vitrin_egress.md), an interface of its own for the reason
[`vitrin_grant`'s `net:` prefix](04-vitrin_grant.md#the-net-resource-prefix)
gives, and the schema's value set gained `egress` in that same commit. The
distinction between the two kinds of absence — by construction versus not yet —
is the whole reason this list is prose and not a bare count.

Naming `observe_cursor` here is rejected by the schema, and
`protocol/test-mutations.sh` covers both directions: an invented verb name and
a real-but-facetless one. Its facetless case is pinned on `observe_cursor`
rather than on whichever verb is currently waiting for a facet, and that is
deliberate — the case was pinned on `layout_arrange` until that verb gained an
interface, at which point the mutated document became legal and the case
reported the schema as broken when the schema was right. `xmllint` can only see
one direction of the pairing
— an IDL using a name the schema omits — so `cargo xtask verb-sets --check`
holds the other, and fails if the schema ever admits a verb no interface
declares.

`layout_arrange`, `layout_focus` and `egress` were in that facetless list
until they gained one each. The layout pair are **two** interfaces rather than
one, and this
attribute is why: `@verb` is one value per interface, so a single combined
layout facet could name only one of them and the other's requests would reach
the chokepoint with no verb to check them against. D-018(3) requires the two
to be independently attenuable, and one attribute per interface is how the
dialect makes that structural rather than conventional.

**The same rule split the powerbox**, and it is the second time this attribute
has forced a decomposition someone had planned as one interface: a
`vitrin_powerbox` carrying `request_file`, `request_dir` **and**
`request_connect` cannot declare both `designate_file` and `egress`, so
[`vitrin_egress`](19-vitrin_egress.md) is its own interface and the powerbox
(page 13, not yet in this tree) carries the filesystem half alone. A proposal
for one facet covering two verbs is wrong on sight.

**A verb having a facet does not make it served.** `egress` has one here and no
deployment answers a petition naming it, because the mediating proxy behind
the facet does not exist. This attribute's value set is about which verbs have
a request to be exercised through; the served set is a property of a
deployment and lives in the reference core, not on the wire.

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
| **relative pointer motion** *(landed, version 2)* | one `since="2"` event `relative_motion(dx, dy, dx_unaccel, dy_unaccel, origin)` on [`vitrin_shim_seat`](11-vitrin_shim_seat.md#relative_motion), at event opcode 5, and **no verb bit** | Appended beside `motion` rather than changing it, because a signature is immutable forever and because the two are not alternatives: one physical movement produces both, and an app binds whichever it understands. It ends with `origin`, so B2 holds structurally. **Both an accelerated and an unaccelerated delta are carried**, since an app that wants the raw one cannot reconstruct it and a shim asked to supply one from the other would be inventing a value. **No timestamp**, deliberately — the shim stamps its own replay clock, and a device clock beside it would be a second unsynchronised one; the cost (a consumer integrating over `dt` gets arrival time, not event time) is paid rather than argued away. **No verb bit, positively**: this is core→shim delivery on the physical path, `vitrin_shim_seat` carries no `@verb` and no requests, and an emulated source stays additive because the tag is already there |
| **pinch and multi-finger swipe** *(landed, version 2)* | four `since="2"` events on [`vitrin_shim_seat`](11-vitrin_shim_seat.md#gesture_begin) at opcodes 6–9 — `gesture_begin(kind, fingers, origin)`, `gesture_swipe_update(dx, dy, origin)`, `gesture_pinch_update(dx, dy, scale, rotation, origin)`, `gesture_end(kind, state, origin)` — plus the `gesture_kind` and `gesture_state` enums, and **no verb bit** | Swipe and pinch **share** their begin and end because those two signatures are identical in the gesture vocabulary being served, and a signature is immutable forever: four events with no dead argument, rather than six with duplicated ones or two phase-tagged events whose deltas and completion flag are meaningless in two phases out of three. A further kind (a hold, say) is therefore an **appended `gesture_kind` entry and no new event** — which is what makes the shared begin/end pay for itself. Two-finger scroll was **already served** by `scroll`, so this row closes a narrower gap than "gestures". The one non-obvious cost is a *pairing* obligation, not a signature one: the core owes exactly one `gesture_end` per `gesture_begin` it sent, on every path, or a begin with no end is the latched-modifier failure wearing a new shape — an obligation, not yet fully discharged: the core mints a `cancelled` end on a realm switch and a seat pause, and **not** when a consent card or the lock screen raises, which withhold the updates and then deliver the device's own end. `gesture_end`'s IDL description names that gap and says closing it is owed |
| **pointer lock: the ask and the core's verdict** *(landed, version 2)* | two `since="2"` messages on [`vitrin_shim_session`](09-vitrin_shim_session.md#pointer_constraint-since-2) — the `pointer_constraint` request at request opcode 3 and the `pointer_constraint_state` event at event opcode 3 — plus the `pointer_constraint_kind`, `pointer_constraint_lifetime` and `pointer_constraint_status` enums, and **no verb bit**; **not** on `vitrin_shim_seat` | **The interface choice was forced by B2, not chosen.** Every `vitrin_shim_seat` event ends with `origin`, and `origin` names a human's device or a principal's actuator; a lock is asked for by the *confined app*, which is neither, so any tag it carried would be false on the one interface whose design idea is that the tag never drifts. `vitrin_shim_seat` also defines no requests by schema, so it could not carry the ask at all. The session bootstrap already carries shim→core requests and reuses the cross-realm clipboard's shape *with the asking party reversed*: the app asks, the core decides, and the state lives where nothing outside the core can strand it. The input half had already landed as `relative_motion`, so this pair added no seat event and took no seat opcode. **One message is the whole state machine's input** — `kind = none` is the withdrawal — so a withdrawal cannot race a set, and the verdict is deliberately **not** a terminal event (§6.2): a constraint deactivates for reasons the shim never asked about, and an answer bound one-to-one to the ask would leave an app locked with nothing able to tell it otherwise. `surface` is the protocol's **first `object` argument**, and its first nullable one. **No verb bit, positively**: the asking party is a confined app rather than a wire principal, so a constraint is derived from no grant and revoking every grant in a session leaves it untouched. Two costs are named on the request itself rather than left to be discovered: the region is one **rectangle**, so a non-rectangular confinement is widened to its bounding box without the app being told, and `lifetime` carries Wayland's `oneshot` though nothing here has yet needed it. Qualify it **`pointer_constraint`** or *pointer lock*: bare `constraint` already means a **petition** constraint here (`request_grant`'s `flags`, and the `set_constraint` builder row above) |
| **idle inhibition** *(landed, version 2)* | one `since="2"` request on [`vitrin_shim_session`](09-vitrin_shim_session.md#idle_inhibit-since-2) — `idle_inhibit(surface, state)` at request opcode 4 — plus the `idle_inhibit_state` enum, and **no verb bit** and **no event** | App-asked like the pointer constraint and landing in the same place, for one further reason: what it asks about is not input at all but the human's own panel. No signature changed and a version-1 shim never sends it. **No verdict event, positively**: `zwp_idle_inhibitor_v1` defines no events at all, so an app's only observable is whether its screen blanked, which makes a refusal both unobservable and harmless — [§5.1](#51-the-razor)'s recoverable half has nothing to carry, and this is the one place the pair differs from `pointer_constraint`, whose app *is* waiting for an activation and would latch forever without one. **No verb bit, positively**: an inhibit is asked for by a confined app rather than by a wire principal, so no grant confers it, no revocation reaches it, and the dead-man chord leaves it alone ([D-042](../plan/20-decision-log.md)). One bit per realm rather than a count, because the core is deciding one question about one panel and object lifetimes are visible only to the shim that holds them. **Three** things are named on the request itself rather than left to be discovered: an inhibit held by a realm the human is not looking at holds nothing; an inhibit suppresses the **blank** and never the **lock**; and the realm gate **bounds Wayland's own "only while this surface is visible" advice without discharging it** — a shim aggregates inhibitor *objects* and is not required to stop counting one whose surface has been unmapped but not destroyed, so an app holding a live inhibitor over a surface it has hidden still holds off the blank for as long as the human is looking at that realm. That is per-realm gating's accepted cost and a named gap, not a defect in either side's implementation. **One cost this column cannot argue away**: appending at a version that already shipped means the integer `2` no longer names one message set, so a shim built against this IDL and pointed at an older version-2 `vitrind` is answered with the `UnknownOpcode` catch-all — log-and-close on a shim connection — and loses its realm with nothing on the wire naming a version mismatch as the cause. Within [§7.4](#74-growth-rules-wayland-style)'s letter, an exception to [§7.3](#73-version-semantics)'s semantics, taken knowingly and owed to P2.1.2's version matrix ([D-042](../plan/20-decision-log.md)(4)) |
| touch and tablet events | new `since="2"` events on `vitrin_shim_seat`, each ending with `origin` | **Not yet served, not refused** — the distinction is load-bearing. The decision that left them out rests on one machine's measured device set (no `INPUT_PROP_DIRECT` node, no pen tool), which is evidence about a laptop rather than a property of a class, and a permanent wire protocol may not foreclose a device class on that ground. Each carries the evidence that reopens it: for **touch**, a touchscreen in the measured device set *and* an application that needs it; for **tablet**, a tablet or stylus device, the application half already being banked. Purely additive whenever either arrives — appended events, B2 satisfied structurally, nothing already here changed — and until then the correct behavior is the one already shipped: **do not advertise a `wl_seat` capability the wire cannot deliver**, because a class advertised and never delivered is worse than an absent one |
| `hello_fd` credential sibling | new `since="2"` fd-borne request on `vitrin_handshake` | `hello`'s signature is frozen forever; oversized credentials arrive via a sibling carrying one fd, so the 32768-byte in-frame bound is never a wall |
| proof-of-possession credential exchange | new `since="2"` challenge event + response request on `vitrin_handshake` | version-0 schemes are bearer-shaped (presented whole in `hello`); a `credential_type` demanding proof of possession (e.g. X.509-SVID) adds a server-driven exchange inside VERIFYING as appended messages — the exchange is part of the handshake itself, so the response request is exempt from the queued-until-BOUND rule (which scopes to non-handshake requests, §7.1) and the unauthenticated deadline stays armed while the server awaits the response; `hello` and `bound` stay frozen, and version-1 connections and bearer schemes never see the new messages |
| dmabuf params builder | new `since="2"` builder on `vitrin_shim_surface`, one fd per add | `attach` stays single-plane linear (no modifier argument to fail to honor); explicit modifiers / multi-planar formats accumulate fds across messages, preserving the one-fd rule |
| `frame_ready` `flags` bits | reserved bits in the existing `frame_flags` bitfield | a later zero-copy dmabuf handoff sets a flag on the *same* `frame_ready` message; `flags` is always 0 in version 0, so setting a bit is additive |
| capture streaming | new `since="2"` sibling messages on `vitrin_view` (a subscription request and its frame-push event, appended after the poll pair) | `capture_frame`/`frame_ready` stay valid forever; refusals still voice through `vitrin_grant.refused`, and each pushed frame carries one fd, so the one-fd rule holds |
| realm enumeration events | new `since="2"` events on `vitrin_realm` | `vitrin_realm` is authority-free and carries no version-0 events; multi-realm phases add enumeration/lifecycle here instead of re-plumbing addressing |
| drag intents | new `since="2"` sibling requests on `vitrin_actuator_pointer` | intent-level motion (drag with duration/easing, interpolated server-side) is added beside `move`/`button`/`scroll`, which stay valid forever |
| ~~**egress vocabulary and facet**~~ **(both landed, version 2; the verb is served by nobody)** | one appended `verb` entry (`egress` = 128, from the repo-wide registry) plus a normative grammar for the `net:HOST:PORT` value of `request_grant`'s **existing** `resource` argument, landed with **no new message at all**; then, separately, the facet: a `since="2"` structural mint `vitrin_grant.get_egress` and the [`vitrin_egress`](19-vitrin_egress.md) interface it mints | The bit appends without touching existing bits, and a verb bit is not version-gated, so a version-1 connection naming it is answered `unsupported` rather than killed. The prefix needs no message because `resource` was type-prefixed and growable from day one; an unserved prefix already resolved `unsupported`, so defining one changes no existing client's behaviour. **This row is the worked example of vocabulary landing before its facet** — the first in this document — and of what that does and does not buy: while the facet was absent, `egress` was a verb no request exercised, which is why every deployment refused it; now the facet is present and **every deployment still refuses it**, because the out-of-core proxy behind it does not exist. A facet is a request to ask through, not a mechanism to answer with, and the refusal reason narrowing without going away is the shape to expect from staged verbs generally. The grammar is **wildcard-free by construction** (one host, one port; no `*`, no CIDR, no list, no range), so a blanket egress grant is *inexpressible* rather than refused by a policy someone can relax; that in turn is what makes `covers` **exact match** rather than a subsumption rule the server would have to invent — and exact match is in turn what makes "wildcard-free" hold whatever the host string contains, since no accepted selector can name more than one endpoint. **Three things this row states that nothing implements**, named so the row is not read as a description of behaviour: the address pin (the addresses a name resolved to at grant time belong in the **grant row** rather than in the proxy, so a proxy restart cannot re-resolve away from what the human approved — but there is no proxy, no resolver, and the row's `pinned_addrs` column is present-but-null; P2.7.3 and P2.7.4 own it); host validation (the parser enforces a denylist of the wildcard-bearing punctuation, not "a DNS name or an IP literal"; see [the `net:` prefix](04-vitrin_grant.md#the-net-resource-prefix) for the full gap list); and **the facet's four messages themselves**, which the reference core does not dispatch at all — sending one to `vitrind` today is `invalid_opcode`, not the recoverable refusal this document specifies. **One tension this row previously left open and no longer does**: `interface/@verb` is one value per interface — the rule that forced the layout facet to be *two* interfaces — so a single `vitrin_powerbox` carrying both `designate_file` and `egress` requests cannot declare both. Same rule, same answer, now executed: `egress` has a **separate facet interface** of its own, and `vitrin_powerbox` will carry the filesystem half |
| **a third terminal on a reply-bearing request** *(landed with the egress facet)* | a new event on the **facet**, beside the existing `connected`/`vitrin_grant.refused` pair — never a new code in `vitrin_grant.refusal` | `request_connect` is answered by one of **three** terminals where no other reply-bearing request in this protocol offers more than two, because a connection can fail without any server deciding anything. The [exactly-one-terminal rule](#61-reply-bearing-requests) is untouched: three is the size of the set the one terminal is drawn from. `refusal` is the enforcement chokepoint's voice and every code in it names something a server decided about a grant; a host that is down decided nothing, and `not_granted` would be the worst available rounding, since an agent that hears it correctly stops asking. The seam generalizes: a later reply-bearing request whose failure is not the server's decision adds its own terminal on its own facet, and `refusal` stays what it is |
| `actuate_key` verb | new appended entry in the `verb` bitfield + a later key-actuation facet | version-0 verb bits are untouched; a new power-of-two bit and its facet are additive — see the landed `realm_launch` row for the shape, including that the bit's *value* comes from the repo-wide allocation registry rather than from the next unused-looking power of two |
| serving `observe_cursor` | no new message: the verb bit already exists and widens what the *existing* `frame_ready` composites for a grant that holds it | version 0 refuses the verb `unsupported`, so beginning to serve it changes no signature and no version-0 client's behavior (a client that never petitions for it sees nothing new) |
| ~~layout facet~~ **(landed, version 2)** | **two** `since="2"` structural mints on `vitrin_grant` — `get_layout_focus` and `get_layout_arrange` — minting [`vitrin_layout_focus`](17-vitrin_layout_focus.md) and [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) | `request_grant`'s five `new_id` arguments are frozen, so neither facet could be co-minted. This row said "a layout facet" singular and **understated the seam**: `@verb` is one value per interface, so one facet could declare only one of the two verbs, and D-018(3) requires them independently attenuable. Corrected here rather than silently, because the row was the record |
| **human attention signal** *(landed, version 2)* | one `since="2"` argument-free event `attention` on [`vitrin_principal`](02-vitrin_principal.md#attention), and **no verb bit** | Appended to the one object whose scope is the connection, which is the subject's scope: the event is about the *human*, not about this principal's authority. Purely additive — a version-1 connection never receives it, and no signature changed. **No verb bit is allocated, positively**: a grantable "receive the human's attention key" verb would put a delegation framing on a signal that delegates nothing, and delivery is instead filtered to principals already holding a layout verb, which is what keeps the wire silent for everyone else rather than opening a keystroke-timing oracle. It makes `preempted` *conditional* for the two layout verbs — the first conditional refusal code in this protocol, and the cost is stated at [`vitrin_grant.refusal`](04-vitrin_grant.md#refusal) rather than left to be discovered |
| **cross-realm clipboard** *(landed, version 2)* | three `since="2"` messages on [`vitrin_shim_session`](09-vitrin_shim_session.md) — the `request_selection` event, the `selection` request, the `offer_selection` event — plus the `selection_status` enum, and **no verb bit** | Appended to the shim bootstrap rather than to a facet of its own, because a facet needs a structural mint and the *core* is the party that asks: it must be able to ask a shim that has done nothing but read `configure`. Purely additive — a version-1 shim never sees the events and never sends the request. **No verb bit, positively**: the human at the keyboard is not a wire principal in version 1, so a `clipboard` verb would name a principal that is not the actor ([D-024](../plan/20-decision-log.md)(3)); the agent-facing verb is E3.5's and is not foreclosed, since `offer_selection` is addressed to a realm and says nothing about who asked. There is deliberately **no `selection_changed` event**: the core pulls, so an ordinary in-app copy is never a cross-realm event. `data`'s `(max N bytes)` token is part of an immutable signature, so raising the 61440-byte cap is a **new message**, never an edit to this one |
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
- **Network** — the protocol is local (Unix domain sockets); no remoting. The
  `egress` verb bit (128) and the [`vitrin_egress`](19-vitrin_egress.md) facet
  are **not** a counter-example and are named here so they are not read as
  one: they grant a realm's outbound reach to one `host:port`
  through an out-of-core proxy, and say nothing about carrying *this
  protocol* over a network. Nor is the verb served by any deployment — the
  proxy does not exist — so what is on the wire today is the vocabulary and
  the request shape, with nothing behind them.
- **Multi-realm** — version 1 serves exactly one well-known realm (`realm-0`).
  Version 2 raises the *count*: a deployment may serve `realm-0` plus further
  realms up to a limit of its own choosing (see
  [`vitrin_realm`](03-vitrin_realm.md#realm-cardinality-one-at-version-1-a-bounded-set-at-version-2)).
  What stays deferred is everything *around* that count. **Realm enumeration
  and lifecycle events** are absent at any verb set, so the further names are
  undiscoverable on the wire and `realm-0` remains the one name a conformant
  client can know without being told. Realm *creation* is on the wire as the
  `realm_launch` verb and the [`vitrin_launcher`](16-vitrin_launcher.md)
  facet, and **the reference core serves it** as of WS-E.1.1: a consented
  launch forks a realm instance from an operator-written template, under an id
  the core mints. A deployment that will not host process creation still
  refuses the verb `unsupported`, exactly as every deployment does for
  `observe_cursor` — which is why "unserved" is a statement about a deployment
  rather than about the wire. What the wire decides, and would otherwise be
  unstateable, is the *shape* — launch is an
  attenuable grant verb rather than a request on the realm handle, and the
  program name is never on the wire. **Stopping** a realm is not expressible at
  all.
- **Powerbox** — no system-mediated resource picker; petitions name resources
  by a type-prefixed string vocabulary.
- **Wallet** — no credential/secret storage or presentation verb.
- **Layout** — version 2 serves `layout_arrange` and `layout_focus` through
  two facets, and what stays deferred is everything the scene cannot honour:
  there is no `place`, `resize`, `raise` or stacking request, and the core
  still has no window manager (the shell that arranges realms is a client). A
  deployment that serves neither verb refuses both `unsupported`, which is a
  property of that deployment (§1.4, D-018). The posture was never deferred —
  layout is authority, not decoration — and the ordering invariants are now
  **tested as invariants** against a client holding every served verb (§1.4,
  "their standing today").
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
