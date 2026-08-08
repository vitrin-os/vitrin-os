# vitrin_shim_session — shim connection bootstrap

**Interface version:** 2 · **Connection class:** shim · **Messages:** 3 requests + 3 events

> Framing, object-id allocation, the fatal/recoverable error taxonomy, delivery
> classification, and versioning are defined once on the [conventions
> page](./00-conventions.md); this page cites those rules rather than restating
> them.

## Purpose

`vitrin_shim_session` is the connection-lifecycle object of a *shim*
connection — the implicit object 1 that exists the instant a shim starts, never
created by any message. It is the shim-connection counterpart of
[`vitrin_handshake`](./01-vitrin_handshake.md) on a principal connection: the
two interfaces occupy object 1 on their respective connection classes, and
neither class's opcodes are reachable from the other (a cross-class opcode dies
as a fatal protocol error with no special casing — see
[conventions](./00-conventions.md)).

Where the principal bootstrap authenticates over the wire, the shim bootstrap
does not. Authentication is *structural*: the shim speaks over a socketpair the
core inherited to it at fork, and possession of that socketpair is the whole
credential. The shim's realm identity was likewise assigned at fork. This
protocol therefore contains **no** request by which a shim sets, claims, or
changes its realm identity — there is no shim `hello`, and there is no wire path
to assert who the shim is. The shim's protocol version is pinned at spawn: the
core and its shims are release-paired, so version negotiation would be
meaningless and is absent.

Since version 2 this object also carries the **cross-realm clipboard**
([D-024](../plan/20-decision-log.md), issue #213): `request_selection`,
`selection` and `offer_selection`. It lives here rather than on a facet of its
own because a facet would need a structural mint, and there is no point in the
shim's life at which the shim knows the core wants one — the core is the party
that asks, and it must be able to ask a shim that has done nothing but read
`configure`.

From this object the shim mints the two children that carry all subsequent
shim-connection traffic: a [`vitrin_shim_surface`](./10-vitrin_shim_surface.md)
(the buffer path by which the shim forwards its app's committed frames up to the
core) via `create_surface`, and a [`vitrin_shim_seat`](./11-vitrin_shim_seat.md)
(the input-delivery object by which the core replays pointer, key, and text
events down to the shim) via `get_seat`. The core opens the conversation from
its side with a single `configure` event carrying the realm identity and
realm-view geometry.

The design idea is minimality of trust surface: because the shim is a
core-spawned disposable child, everything it would otherwise have to prove
(identity, realm, version) is established out of band by the parent, and the
wire is left to carry only the buffer and input plumbing. That is also why this
interface — unlike the principal bootstrap — carries no fatal-error *event*: a
shim protocol violation is handled by log-and-close, not by a wire message.

## Lifecycle

The object exists implicitly at connect (object 1) and lives for the whole
connection; version 1 defines no destructor for it. Its children
(`vitrin_shim_surface`, `vitrin_shim_seat`) are minted by the requests below and
likewise live for the connection.

The core sends `configure` first, before it processes any request from the shim
(see the event below for the ordering guarantee). The shim then mints its
surface and its single seat.

Connection teardown is governed by socketpair EOF, which the core treats as
*shim death*: the core survives, closes every in-flight buffer fd it is holding
for that shim's surface, and drops the realm's surface from the scene. The
agent-facing consequence is that a principal's next capture on that realm
refuses recoverably with `no_surface` (see
[`vitrin_grant.refused`](./04-vitrin_grant.md) and
[`vitrin_view`](./06-vitrin_view.md)) — never a stale frame. Version 1 has no
restart policy: a dead shim stays dead until higher-level machinery relaunches
its app.

Because a shim connection carries no fatal-error message, protocol violations on
this connection are *log-and-close* conditions: the core logs a named reason and
closes the connection. The condition named on this interface is
`already_initialized` — a second `get_seat` on the same session (see below). The
other shim-connection log-only conditions live on
[`vitrin_shim_surface`](./10-vitrin_shim_surface.md) (`invalid_buffer`,
`bad_order`). See the [conventions page](./00-conventions.md) for why the shim
class carries no on-wire fatal channel.

## Requests

### create_surface

```
create_surface(surface: new_id<vitrin_shim_surface>)
```

| name | type | description |
|---|---|---|
| `surface` | `new_id<`[`vitrin_shim_surface`](./10-vitrin_shim_surface.md)`>` | the new surface object; must obey the id-allocation rules (strictly increasing, unused, above the connection watermark) |

Creates a surface through which the shim forwards its app's committed buffers up
to the core. Version 1 composites realm surfaces with a trivial single-maximized
layout, but nothing in the protocol hard-codes exactly one surface per shim —
the request MAY be issued more than once, and multi-surface composition is a
purely additive growth direction (see [Growth](#growth)).

**Delivery class:** a **structural mint** — neither reply-bearing nor refusable. The request only mints
an object; there is no terminal event and no wire acknowledgement. A malformed
`new_id` (reused, out of range, or below the watermark) is handled per the
shim-connection rule as log-and-close, not as a recoverable refusal.

### get_seat

```
get_seat(seat: new_id<vitrin_shim_seat>)
```

| name | type | description |
|---|---|---|
| `seat` | `new_id<`[`vitrin_shim_seat`](./11-vitrin_shim_seat.md)`>` | the new seat object; must obey the id-allocation rules |

Mints the seat through which the core delivers input for this realm — the object
that carries the origin-tagged `motion`, `button`, `scroll`, `key`, and `text`
events the shim replays to its app.

There is **exactly one seat per session.** A second `get_seat` on the same
session is the log-and-close condition `already_initialized`: the core logs the
reason and closes the connection.

**Delivery class:** a **structural mint**, as with `create_surface` — no
terminal event, no refusal. The single-seat violation is fatal to the connection
(log-and-close), consistent with the taxonomy rule that a client violating its
own object graph dies rather than being refused.

### selection (since 2)

```
selection(serial: uint, status: selection_status, mime: string, data: string)
```

| name | type | description |
|---|---|---|
| `serial` | `uint` | the serial of the `request_selection` this answers |
| `status` | `uint`<`selection_status`> | whether `data` follows, and why not |
| `mime` | `string` | MIME type of `data`, empty unless `status` is `ok` (max 32 bytes) |
| `data` | `string` | the selection as UTF-8, empty unless `status` is `ok` (max 61440 bytes) |

The shim's **answer**, and the only message on this protocol that carries an
app's own bytes upward. It is legal only in reply to a `request_selection`, and
a well-behaved shim sends exactly one per event received.

Every `request_selection` is answered, including with nothing: a shim whose app
has no selection answers `empty` rather than staying silent, because silence is
indistinguishable from a hung shim and would make the core wait on an untrusted
peer.

**Only `text/plain;charset=utf-8` crosses.** The shim answers `wrong_type` for
anything else — including bytes its app labelled as text that are not
well-formed UTF-8 — and `too_large` for a selection past `data`'s bound. The
shim is expected to make that judgement itself rather than let the bound decide
it: a `data` string past its declared maximum is fatal `invalid_argument`, which
on a shim connection means log-and-close, and a human copying a large file must
not kill their own app's shim.

`mime` and `data` MUST be empty unless `status` is `ok`. The core validates the
type against its own allow-list whatever this message claims — a shim is
untrusted, and `ok` is a claim rather than a credential — and an answer whose
serial is not the outstanding one is a stale answer to a superseded gesture and
is discarded.

**Delivery class:** **fire-and-forget.** There is no terminal event and no
acknowledgement, deliberately: what the core does with a selection is the
human's business, and a shim that learned whether its bytes were accepted would
learn something about another realm.

## Events

### configure

```
configure(realm: string, width: uint, height: uint)
```

| name | type | description |
|---|---|---|
| `realm` | `string` | realm identity assigned at fork (max 64 bytes); informational only |
| `width` | `uint` | realm-view width in pixels |
| `height` | `uint` | realm-view height in pixels |

The core's first message on a shim connection. It is guaranteed to precede the
processing of any shim request: the shim performs one synchronous read at
startup — before it begins serving its own private Wayland socket — so a
deferred/asynchronous configure state machine is unnecessary.

`realm` is purely informational. The shim can never *assert* its realm identity
(there is no wire path to do so); the identity is echoed here only so the shim
can label logs and, where relevant, present it to its app. `width` and `height`
are the realm-view size the shim advertises to its app through its own output
and window-configure machinery (the single-maximized layout of version 1).

`configure` MAY be re-sent when the realm view resizes. A version-1 core is
permitted to letterbox a fixed-size buffer instead of re-configuring, but the
message exists precisely so that a resize is never a protocol change — a later
core can drive genuine resize through the same event without a signature bump.

**Delivery class:** a server directive (a `configure`-style event), not a reply.
It correlates to no request; it is unsolicited and ordering-first.

### request_selection (since 2)

```
request_selection(serial: uint)
```

| name | type | description |
|---|---|---|
| `serial` | `uint` | names the answer this request expects |

**The core pulls; a shim never pushes.** This event is the first half of the
human's cross-realm copy gesture, and the core sends it only because a human at
the physical keyboard pressed the promote chord while the output was bound to
this realm.

There is deliberately **no `selection_changed` event**. A shim that forwarded
every app-side copy upstream would make every ordinary in-app copy a cross-realm
event — an ambient channel wearing a broker's clothes — and the core would
learn every copy a human made inside every realm. Because the core only ever
asks, a realm cannot place anything where another realm could reach it at a
moment of its own choosing, which is what makes *"cannot be triggered or forced
by any realm"* literally true here rather than a design intention.

`serial` is strictly increasing within a connection and never reused. It exists
so that a second gesture supersedes the first: an answer bearing an old serial
is discarded rather than filling the slot the newer gesture is waiting on.

**Delivery class:** a server directive. It correlates to no request, and its
answer (`selection`, above) is a request rather than a terminal event — the
shim connection has no reply channel from core to shim beyond events.

### offer_selection (since 2)

```
offer_selection(mime: string, data: string)
```

| name | type | description |
|---|---|---|
| `mime` | `string` | MIME type of `data` (max 32 bytes) |
| `data` | `string` | the clipboard contents as UTF-8 (max 61440 bytes) |

The second half of the human's gesture, and a **separate, later, physical act**:
the human pressed the offer chord while the output was bound to this realm.
**One gesture transfers nothing** — promoting places bytes in the core's slot
and reaches no other realm; offering reads that slot and never writes it — and
the two chords are distinct, so no single press can do both.

On receiving it the shim makes the payload its app's ordinary selection, through
a shim-owned data source on its own seat. It does **not** paste: the human still
presses their app's own paste key, exactly as the Qubes model this follows
separates *"move the clipboard into the target"* from *"paste it there"*.

The core sends no offer with an empty slot, so receiving this always means there
is something to install. It is sent to exactly one realm per gesture — the core
never fans an offer out, and never offers a slot back to the realm it was
promoted from unless the human gestures there.

**Delivery class:** a server directive, unsolicited from the shim's point of
view.

## Enums

### selection_status

| entry | value | meaning |
|---|---|---|
| `ok` | 0 | `mime` and `data` carry the app's selection |
| `empty` | 1 | the app has no selection at all |
| `wrong_type` | 2 | the selection is not well-formed `text/plain;charset=utf-8` |
| `too_large` | 3 | the selection exceeds `data`'s byte bound |

The three refusing entries are distinguished rather than collapsed into one
because they mean different things to a human debugging a clipboard that did not
work: *"I selected nothing"*, *"I selected a picture"* and *"I selected too
much"* are three different mistakes with three different fixes. The core
journals which one it saw, and never any content.

Entries are appended, never renumbered, like every enum in this protocol.

## Flows

The scenarios below are drawn from the canonical message-flow set; only the
steps that touch `vitrin_shim_session` are expanded here. Direction key: **C→S**
= core→shim, **S→C** = shim→core. Bracketed steps are out-of-band (no wire
message).

### Flow G — shim spawn and session bring-up

1. `[core forks/execs the shim with an inherited socketpair fd and a private runtime dir; realm identity assigned at fork; no handshake — authentication is by inheritance; vitrin_shim_session is implicit object 1]`
2. **C→S** `vitrin_shim_session.configure(realm="realm-0", width=1280, height=800)` — the core's first message, read synchronously by the shim at startup
3. **S→C** `vitrin_shim_session.create_surface(surface=new_id)` — the shim mints its buffer path
4. **S→C** `vitrin_shim_session.get_seat(seat=new_id)` — the shim mints its single input-delivery object
5. `[the shim serves its private Wayland socket; the app connects, binds its Wayland globals, and commits a buffer to the shim]`
6. … the attach / damage / commit / frame_done loop continues on [`vitrin_shim_surface`](./10-vitrin_shim_surface.md), and input replay on [`vitrin_shim_seat`](./11-vitrin_shim_seat.md)

### Flow H — shim killed mid-loop

1. `[the shim's frame loop is turning while an agent captures the realm]`
2. `[kill -9 on the shim ⇒ socketpair EOF + SIGCHLD at the core]`
3. `[core logs the crash, reaps the child, removes the surface from the scene, and closes every attached buffer fd it holds — no fd leak; the core survives; no restart in version 1]`
4. `[on the principal connection, the agent's next capture_frame refuses]` **C→A** [`vitrin_grant.refused`](./04-vitrin_grant.md)`(observe, no_surface, 0)` — "the realm has no surface", never a stale frame

### Flow K — cross-realm clipboard, two human gestures

Realms A and B are both live; the output is bound to A. Nothing below is
reachable by any client, at any verb set: both triggers are physical keys the
core consumes.

1. `[the human selects text in A's app and copies it with the app's own key — this is app-internal and touches no vitrin message]`
2. `[the human presses the promote chord (Ctrl-Shift-Insert). The core consumes both halves; A's app never learns it happened]`
3. **C→S(A)** `vitrin_shim_session.request_selection(serial=1)`
4. **S(A)→C** `vitrin_shim_session.selection(serial=1, status=ok, mime="text/plain;charset=utf-8", data=…)` — the core validates the type against its own allow-list and the length against its own cap, then fills its single slot and journals `clipboard_promoted` with a length and a digest, never content
5. `[the human moves the output to B — a layout_focus holder, or the operator's own shell]`
6. `[the human presses the offer chord (Shift-Insert). Consumed likewise]`
7. **C→S(B)** `vitrin_shim_session.offer_selection(mime="text/plain;charset=utf-8", data=…)` — B's shim installs it as its app's selection and the core journals `clipboard_offered`
8. `[the human presses B's app's own paste key. That keystroke is ordinary input and reaches the app; the app reads its own seat's selection]`

Step 2 alone transfers nothing to B, and step 6 alone (with an empty slot) sends
no event at all. That is the whole of the two-gesture property.

The `already_initialized` path (a second `get_seat`) does not appear in the
canonical scenarios; it is a hostile/buggy-shim condition that terminates the
connection by log-and-close.

## Growth

Every seam below is purely additive under the protocol's Wayland-style growth
rules ([conventions](./00-conventions.md)): new messages are appended with
`since` attributes, existing signatures are immutable forever, and enum values
never change meaning.

- **Multi-surface composition.** Version 1 composites with a trivial
  single-maximized layout, but `create_surface` is deliberately not restricted
  to one call. Later versions can composite several surfaces per shim without a
  new request — the surface-minting path already exists.

- **True resize.** `configure` already carries geometry and is explicitly
  re-sendable, so a later core can drive genuine realm-view resize (rather than
  version 1's letterbox option) with no protocol change.

- **An agent-facing clipboard verb.** The human path above is deliberately
  *not* grant-governed, because the human at the keyboard is not a wire
  principal in version 1 ([D-024](../plan/20-decision-log.md)(3)). A
  principal-facing clipboard — E3.5's, not this issue's — is additive and is
  not foreclosed by anything here: `offer_selection` is addressed to a realm
  and says nothing about who asked, so a later `vitrin_grant` facet can reach
  the same slot through the enforcement chokepoint under a verb of its own.

- **More MIME types, and a larger cap — each a new message, never an edit.**
  `data`'s `(max N bytes)` token is part of an immutable signature, so raising
  the cap means appending a sibling message (or a payload that travels as an
  fd), not changing this one. The same holds for any type beyond
  `text/plain;charset=utf-8`: version 2's allow-list is a core-side rule, but
  anything that needs a *decoder* is refused on the separate ground that the
  trusted core carries no image codec in any dependency class.

- **Restart policy.** Version 1 defines none: shim death is terminal for the
  realm's surface. A restart/relaunch policy is a later addition; because it is
  orchestration above the wire, it need not alter this interface at all, and any
  wire support it does want (e.g. a relaunch directive) arrives as an appended
  `since`-gated message.
