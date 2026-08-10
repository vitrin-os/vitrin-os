# vitrin_shim_session — shim connection bootstrap

**Interface version:** 2 · **Connection class:** shim · **Messages:** 4 requests + 4 events

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

Version 2 also carries the **pointer constraint**
([D-032](../plan/20-decision-log.md), issue #222): the `pointer_constraint`
request and the `pointer_constraint_state` event. It is here for the *opposite*
reason and lands in the same place. The clipboard is here because the **core**
asks; the pointer constraint is here because the **app** asks, and
[`vitrin_shim_seat`](./11-vitrin_shim_seat.md) can carry neither half — that
interface defines no requests at all (structural rule B2, schema-enforced), and
every event on it ends with an `origin` tag naming a human's device or a
principal's actuator, which a confined app is neither. So this one object
carries the only two exchanges in the protocol whose asking party is not the one
the interface it would otherwise belong to was built around. The IDL's own
descriptions carry the full argument and are authoritative.

Two naming rules apply to everything below, and both are load-bearing. **Call it
a *pointer constraint* or a *pointer lock*, never a bare "constraint"** — in
this protocol that word already means a **petition** constraint
([`vitrin_realm.request_grant`](./03-vitrin_realm.md)'s `flags`, and the
`set_constraint` builder in [Appendix A](./00-conventions.md)), so every
identifier this pair mints carries the qualifier. And **the core decides**: the
shim's message is an *ask*, not a setting.

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

### pointer_constraint (since 2)

```
pointer_constraint(serial: uint, surface: object<vitrin_shim_surface>?,
                   kind: pointer_constraint_kind,
                   lifetime: pointer_constraint_lifetime,
                   x: int, y: int, width: uint, height: uint)
```

| name | type | description |
|---|---|---|
| `serial` | `uint` | **shim-minted**; names the answer this ask expects |
| `surface` | `object<`[`vitrin_shim_surface`](./10-vitrin_shim_surface.md)`>` (nullable) | the surface the constraint applies to; MUST be null when `kind` is `none` |
| `kind` | `uint`<`pointer_constraint_kind`> | `lock`, `confine`, or `none` to withdraw |
| `lifetime` | `uint`<`pointer_constraint_lifetime`> | `oneshot` or `persistent`; ignored when `kind` is `none` |
| `x` | `int` | region origin x, surface-local pixels |
| `y` | `int` | region origin y, surface-local pixels |
| `width` | `uint` | region width; zero with `height` zero means the whole surface |
| `height` | `uint` | region height; zero with `width` zero means the whole surface |

The confined app's **ask**, relayed by its shim. This is the mirror of
`request_selection`: there the core asks and the shim answers, here the app asks
and the core answers. The reversal is stated rather than left to be inferred,
because a reader carrying the clipboard's direction over to this pair reads
every rule below backwards. `serial` is minted **by the shim**, strictly
increasing within the connection and never reused — the same discipline the
core's own serials follow.

**One message is the whole state machine's input.** `kind = none` is the
*withdrawal*, so there is no separate unset message a withdrawal could race a
set against, and no order in which the two could be applied wrongly. When `kind`
is `none`, `surface` MUST be null and `x`, `y`, `width` and `height` MUST be
zero; the core ignores them in that case rather than treating a decorated
withdrawal as data — the same discipline `selection` states for `mime` and
`data` on a refusing status.

**The region is one rectangle, and its cost is named rather than discovered.**
`width` and `height` both zero means the whole surface, which is the
null-region meaning of Wayland's own pointer-constraint vocabulary carried
across without inventing a nullable rectangle type. Wayland permits an arbitrary
region; this carries a rectangle, so a genuinely non-rectangular confinement is
reduced to its **bounding box** and the app is *not* told that it was. The
coordinates are `int` and the extents `uint` rather than `fixed`: `fixed` is
reserved for [`vitrin_shim_seat`](./11-vitrin_shim_seat.md) motion
([conventions §2.2](./00-conventions.md)), and a region is not motion.

`lifetime` carries **Wayland's own two**, so a shim can honour a one-shot lock's
destroy semantics without inventing a policy of its own. Nothing in this system
has yet *needed* `oneshot`; it is carried because a signature is immutable
forever and adding it afterwards would be a whole new message.

**A second ask replaces the first.** One record exists per connection; a second
ask supersedes the earlier one, whose serial is answered `superseded` and hears
nothing further.

**Errors.** A constraint the deployment will not serve is **recoverable**: the
answer is `refused`, the connection lives, and the app's own lock object is left
inert — a legal Wayland state, since a compositor may decline to activate a
lock, and that is what lets a refusal not wedge the app. This is the same shape
[§7.3](./00-conventions.md) already settles by precedent for a verb a deployment
does not serve. What *is* fatal is unchanged decode-level ground: a `surface` id
this connection never minted, or one at or below its watermark, is
`invalid_object` (the shim violating its **own** object graph — the razor's
first clause); an out-of-range `kind` or `lifetime` is `invalid_argument`. On a
shim connection both mean log-and-close. Stated positively so nobody goes
looking: this request adds **no** new shim log-only condition, and — carrying no
`string` argument — **no** row to [§2.3](./00-conventions.md)'s per-argument
byte-bound table. An ask naming a surface that has never committed is a
legitimate ask answered `inactive`, not a `bad_order`.

**Delivery class:** **fire-and-forget**, and here the classification is
load-bearing rather than bookkeeping. The verdict does arrive — as a
`pointer_constraint_state` carrying this serial — but it is **not a terminal
event**, and [§6.1](./00-conventions.md)'s exactly-one-terminal rule
deliberately does not apply: a constraint's state changes for reasons the shim
never asked about (the human switched realms, raised a consent card, locked the
screen), so binding the answer one-to-one to the ask would leave an app locked
with no message that could ever tell it otherwise — the exact latch this design
exists to prevent. The coalescing licence that ordinarily accompanies
fire-and-forget ([§6.2](./00-conventions.md)) is **overridden in the strict
direction**: the core sends at most one `pointer_constraint_state` per
transition and never coalesces two different states.

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

### pointer_constraint_state (since 2)

```
pointer_constraint_state(serial: uint, state: pointer_constraint_status)
```

| name | type | description |
|---|---|---|
| `serial` | `uint` | the serial of the `pointer_constraint` ask this concerns |
| `state` | `uint`<`pointer_constraint_status`> | what the core did with that ask, and what is in force now |

The **verdict and the running state on one message**. They are not split into
two because two ways to learn the same fact is how the two come to disagree.

**Level-triggered, edge-reported.** The core recomputes each record's state from
live conditions and sends this event only when that state *changed*. Consecutive
identical states are not sent, and two different states are never coalesced into
one. A serial the shim does not recognise is stale — a later ask superseded it —
and is ignored, the same rule `request_selection` states for its own serials.

**A refusal cannot wedge an app**, because on `refused` the shim does nothing at
all: it does not destroy the app's constraint object and does not raise an error
on it, it simply never sends the activation the object is waiting for. An inert
`zwp_locked_pointer_v1` is a legal Wayland state, so the app's own state machine
handles a refusal with no behaviour specific to this system.

**What `active` means for the human**, stated on the wire because an app must
not have to discover it by experiment. While a constraint is active the core
stops delivering absolute `motion` to that realm, keeps delivering
[`relative_motion`](./11-vitrin_shim_seat.md#relative_motion), freezes the
position its own hit tests use, and — on a backend where it draws one — **hides
its own human cursor sprite**. The app cannot hide that sprite and there is
deliberately no message by which it could: the sprite is the core's, and a realm
able to hide the human's pointer could mislead the human about where their input
is going ([`vitrin_view`](./06-vitrin_view.md) states the same rule for the
bitmap). The core therefore owns the **un-hiding** too, on every path that ends
a constraint, without exception — a constraint that ended without its sprite
returning would leave a human with no visible pointer, a worse failure than any
this pair otherwise risks.

**What an active constraint does not reach.** It never confines a principal's
actuation: an emulated pointer `move` follows its grant and is delivered
absolutely whatever this record says, so an app cannot acquire authority over an
agent by locking a pointer. It never reaches the core's own overlays either — a
consent card, the lock screen, the dead-man hold and the core's notice each take
the pointer back, and each is a path on which this event reports the constraint
`inactive`.

**Delivery class:** a **server directive**, in the class `configure` occupies —
correlated to an ask by `serial`, never terminal to it.

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

### pointer_constraint_kind (since 2)

| entry | value | meaning |
|---|---|---|
| `none` | 0 | withdraw this connection's constraint; `surface` MUST be null |
| `lock` | 1 | pin the pointer; movement reaches the app as `relative_motion` only |
| `confine` | 2 | keep the pointer inside the region; absolute motion continues within it |

The three things one ask can say. Carrying `none` here is why there is no
separate unset message for a withdrawal to race against a set.

### pointer_constraint_lifetime (since 2)

| entry | value | meaning |
|---|---|---|
| `oneshot` | 0 | ends for good at its first deactivation |
| `persistent` | 1 | may deactivate and reactivate with no new ask |

Wayland's own two, carried across so a shim never has to invent a policy its app
did not ask for. `persistent` is what makes a realm switch survivable: the human
coming back re-activates what they left, with no second ask.

`persistent` survives a **deactivation**, not a **withdrawal**: a state of
`withdrawn` is terminal for the serial that carries it, so a constraint the core
withdrew — a seat pause, the dead-man chord — needs a new ask, and the app is
told by the `withdrawn` rather than left to discover it. The shim drops its live
record on that state (`shim/src/constraint.c`), which is why the distinction is
observable rather than a wording preference.

### pointer_constraint_status (since 2)

| entry | value | meaning |
|---|---|---|
| `inactive` | 0 | recorded but not in force; may become active later with no new ask |
| `active` | 1 | in force: absolute motion stops, `relative_motion` continues, the core hides its own cursor sprite |
| `withdrawn` | 2 | the record is gone: the shim withdrew it, or what it named went away |
| `refused` | 3 | not recorded at all; the app's object stays inert and this serial is not re-asked |
| `superseded` | 4 | a later ask on this connection replaced it; this serial gets nothing further |

**The zero value is deliberately not an `ok`**, which departs from
`selection_status` above, where `ok` is 0. The departure is argued rather than
left standing as an inconsistency: zero is where a mis-decode and a zeroed
struct both land, and *"not constrained"* is the safe reading of a byte nobody
can trust, while *"constrained"* is not.

`inactive` and `active` are the two live states, and one record moves between
them as often as the human's own actions require, with no further ask.
`withdrawn`, `refused` and `superseded` are terminal for the serial that carries
them: nothing follows on that serial ever again.

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

### Flow L — an app locks the pointer, and the human takes it back

Realm A's app is a 3-D viewport that wants raw deltas. Nothing below is
triggered by any principal, and nothing below is refusable by one: a pointer
constraint is derived from **no grant**, so revoking every grant in the session
leaves it exactly as it was.

1. `[A's app creates a zwp_locked_pointer_v1 on its own Wayland connection to the shim]`
2. **S(A)→C** `vitrin_shim_session.pointer_constraint(serial=1, surface=<A's surface>, kind=lock, lifetime=persistent, x=0, y=0, width=0, height=0)` — the whole surface
3. **C→S(A)** `vitrin_shim_session.pointer_constraint_state(serial=1, state=active)` — the pointer was inside the region, so it activates at once; the core stops sending absolute `motion` to A, keeps sending [`relative_motion`](./11-vitrin_shim_seat.md#relative_motion), and hides its own human cursor sprite
4. `[the shim sends its app zwp_locked_pointer_v1.locked]`
5. `[the human presses the dead-man chord, or a consent card raises, or the screen locks, or they switch the output to realm B]`
6. **C→S(A)** `pointer_constraint_state(serial=1, state=inactive)` — **and the sprite is back before the human's own gesture completes**; the shim sends its app `unlocked`
7. `[the human returns to A, or the card is answered]`
8. **C→S(A)** `pointer_constraint_state(serial=1, state=active)` — `persistent` reactivates with no second ask; the shim sends `locked` again
9. `[A's app destroys its lock object]` **S(A)→C** `pointer_constraint(serial=2, surface=null, kind=none, lifetime=oneshot, x=0, y=0, width=0, height=0)` — the withdrawal; every field but `serial` and `kind` is ignored
10. **C→S(A)** `pointer_constraint_state(serial=2, state=withdrawn)`

Step 6 is the whole safety property: **every** path that ends a constraint
restores the human's cursor, because a human on a display server they cannot
exit must never be left with no visible pointer. Step 8 is why `persistent`
exists — a design that only *un*-hid on the way out would owe a second, easily
forgotten re-hide on the way back.

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

- **A non-rectangular constraint region, and a per-surface constraint count.**
  `pointer_constraint` carries one rectangle and one record per connection.
  Wayland allows an arbitrary `wl_region` and more than one constraint object;
  both are appended siblings if an application ever needs them, never edits to
  this signature. The bounding-box widening is stated on the request above so
  that an app hitting it learns why from the spec rather than from a bug report.

- **A pointer-constraint verb, if a principal ever asks for one.** Nothing here
  is grant-governed, because the asking party is the **confined app** and not a
  wire principal — which is also why revoking a grant does not touch a
  constraint. A principal-facing equivalent (an agent asking to be delivered
  deltas, say) is additive and is not foreclosed: it would be a facet on
  [`vitrin_grant`](./04-vitrin_grant.md) under a verb of its own, reaching the
  same core-side record through the enforcement chokepoint.

- **Restart policy.** Version 1 defines none: shim death is terminal for the
  realm's surface. A restart/relaunch policy is a later addition; because it is
  orchestration above the wire, it need not alter this interface at all, and any
  wire support it does want (e.g. a relaunch directive) arrives as an appended
  `since`-gated message.
