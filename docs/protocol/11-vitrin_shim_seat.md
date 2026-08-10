# vitrin_shim_seat — origin-tagged input delivery from core to the shim

**Interface version:** 2 · **Connection class:** shim · **Messages:** 0 requests + 10 events (the original five, which carry no `since` and are therefore version 1, plus five at `since="2"`)

## Purpose

`vitrin_shim_seat` is the input-delivery interface of a shim connection: it carries pointer motion, buttons, scroll, keys, text, relative pointer deltas, and multi-finger gestures *from the core to the shim*, which then replays them to its nested application through its own virtual `wl_seat`. It is an **events-only** interface — the shim never sends on it. The core owns the mapping from principals (agent actuators, and in a later phase physical human input) to shim seats; the shim is a dumb replay target that knows nothing of grants, verbs, or principals. See [conventions](00-conventions.md) for the two connection classes and the shim's structural (socketpair-inherited) authentication.

The seat sits under [`vitrin_shim_session`](09-vitrin_shim_session.md): a session mints exactly one seat via `get_seat` (a second attempt is the log-and-close condition `already_initialized`). Where [`vitrin_shim_surface`](10-vitrin_shim_surface.md) is the *output* half of the shim connection (buffers flowing shim→core), the seat is the *input* half (events flowing core→shim). Two agent-facing actuator interfaces feed it: a principal's [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) request becomes a `motion`/`button`/`scroll` event here, and a [`vitrin_actuator_text`](08-vitrin_actuator_text.md) `type` request becomes a `text` event here. The `key` event has no v0 agent-facing source — it exists for the human path, as do all five of the version-2 events: no actuator mints a relative delta or a gesture, and no verb was allocated to let one.

The organising design idea is **per-event origin tagging**. Every event on this interface carries `origin` as its final argument, distinguishing input a physical human device produced (`physical`) from input a principal's actuator emulated (`emulated`). This preserves end to end the distinction that libei/EIS drops at the hop into the compositor. The tag is present *from day one* on every event, never as a modal start/stop envelope — modal "physical input begins / ends" framing desyncs and fuzzes badly, whereas per-event tags cannot drift out of sync. This structural rule (last arg MUST be `origin` with `enum="origin"`) is enforced by the RNG schema, not merely by convention. It is load-bearing for a later phase in which physically-originated consent is built directly on the physical/emulated distinction.

Two coordinate and focus conventions frame the interface. Pointer coordinates are **realm-view pixels** (the same pixel space agents address via the actuator); the shim maps view coordinates to surface-local coordinates before replaying. Focus in version 1 is **synthesized shim-side**: the realm is single-surface, so the shim generates pointer-enter on first motion and holds keyboard focus on the app. There is no wire focus event in v0; an explicit tagged focus event is the multi-surface addition of a later version.

**One shared pointer position in version 1, and still in version 2.** These events carry `origin`, not principal identity, so a realm's app sees a single pointer whoever moved it. This is the version-1 half of the per-principal cursor model (D-017; see [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md#the-cursor-model)), and it is the reason the core carries a defensive rule keeping emulated motion from relocating the position a consent grab hit-tests. The agent-facing side of the wire is already principal-relative — `move` moves *that principal's* pointer — so the gap is exactly here, in delivery. Per-principal delivery is deferred to M2 and arrives as `since`-gated sibling events that also name the principal, each still ending with `origin` so B2 holds; the version-1 five stay valid forever, and so does everything appended to them.

**What version 2 appended, and what it did not.** Five more events: [`relative_motion`](#relative_motion), and the four that carry a multi-finger touchpad gesture — [`gesture_begin`](#gesture_begin), [`gesture_swipe_update`](#gesture_swipe_update), [`gesture_pinch_update`](#gesture_pinch_update), [`gesture_end`](#gesture_end) — with two new enums, [`gesture_kind`](#gesture_kind) and [`gesture_state`](#gesture_state). They are appended siblings at event opcodes 5–9; nothing in the version-1 five changed, and **no verb bit was allocated** — these are core→shim delivery events on the physical path, and this interface carries no `@verb` attribute and no requests for a verb to gate. Two-finger scroll was already served as [`scroll`](#scroll) (the core converts the device's axis events to v120), so the gesture gap version 2 closes is specifically **pinch and multi-finger swipe**, not "gestures" wholesale. See [D-032](../plan/20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence) for the per-class decision, the device measurement it rests on, and the classes that are **not yet served** — each recorded with the evidence that would reopen it rather than closed off.

## Lifecycle

A seat instance comes to exist when the shim calls `get_seat(seat)` on its [`vitrin_shim_session`](09-vitrin_shim_session.md) (object 1 of the shim connection). Exactly one seat exists per session; a second `get_seat` is the shim log-only fatal condition `already_initialized` (log-and-close, no wire error message — shim connections use log-and-close rather than the principal connection's `error` carrier; see [conventions](00-conventions.md)).

The seat is not grant-derived and therefore has no inert semantics of its own: it lives for the life of the shim connection. It is destroyed when that connection ends. The normal end is shim EOF — the core detects the socketpair closing, treats it as shim death, and tears the seat down along with the surface and session. v0 defines **zero destructors**, so there is no `release`/`destroy` request for the seat; teardown is by connection close only.

Because the seat carries no client-created object references and mints no `new_id`s, the [inert-ID tolerance rule](00-conventions.md) does not bear on it directly, but the general guarantee holds: IDs are never reused, so any late-arriving event is safe to interpret.

## Events

All ten events are **fire-and-forget** (no reply, no terminal event, delivery class per [conventions](00-conventions.md)). The core emits them core→shim; the shim replays them and never acknowledges on the wire. There are no recoverable refusals or fatal codes *on this interface* — enforcement (grant active? verb granted? rate? expiry?) happens upstream at the single chokepoint on the principal connection, before the core ever emits a seat event. By the time an event reaches the seat, the authority question is already settled. Malformed shim-connection traffic is handled by the session's log-and-close conditions, not here.

Every event's final argument is `origin` (enum [`origin`](#origin)); this is schema-enforced.

**One asymmetry the version-2 events introduce, stated once here.** The version-1 five are each self-contained: a `motion` means something on its own. `gesture_swipe_update`, `gesture_pinch_update` and `gesture_end` are not — each is only meaningful between a `gesture_begin` and its `gesture_end`. That makes the core the party holding a **pairing obligation** (exactly one end per begin it sent, on every path), and makes the shim the party holding a **tolerance obligation**: an event that arrives without its begin is a *core* bug, and the shim ignores it rather than replaying it and rather than closing the connection. Log-and-close on a shim connection is the remedy for a **shim's** violations (see [`vitrin_shim_session`](09-vitrin_shim_session.md)); an application must not die of the core's mistake. This is spelled out in the IDL's own `gesture_begin` description, which is authoritative.

### motion

`motion(x: fixed, y: fixed, origin: uint)`

| arg | type | description |
|---|---|---|
| `x` | `fixed` | realm-view x coordinate (24.8 fixed-point) |
| `y` | `fixed` | realm-view y coordinate (24.8 fixed-point) |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

Pointer motion to realm-view (`x`, `y`). The shim maps these view coordinates to surface-local coordinates for replay on its `wl_seat`. Coordinates are **fixed-point** so that later server-side motion synthesis can be sub-pixel without a signature change — note the asymmetry with the agent-facing [`vitrin_actuator_pointer.move`](07-vitrin_actuator_pointer.md), which stays integer because agents address captured pixels. In v0 the values delivered are whole-pixel (integer actuator input widened to fixed); the fractional headroom is reserved for the Phase-2 synthesis path.

### button

`button(button: uint, state: uint, origin: uint)`

| arg | type | description |
|---|---|---|
| `button` | `uint` | Linux evdev button code (e.g. `BTN_LEFT` = 0x110) |
| `state` | `uint` enum [`vitrin_actuator_pointer.button_state`](07-vitrin_actuator_pointer.md) | pressed or released |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

Pointer button press or release at the current pointer position, using Linux evdev button codes. `state` reuses the `button_state` enum defined on [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) (`released` = 0, `pressed` = 1) rather than redefining it.

### scroll

`scroll(axis: uint, value120: int, origin: uint)`

| arg | type | description |
|---|---|---|
| `axis` | `uint` enum [`vitrin_actuator_pointer.axis`](07-vitrin_actuator_pointer.md) | scroll axis (`vertical` = 0, `horizontal` = 1) |
| `value120` | `int` | scroll amount; one wheel notch = ±120 |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

High-resolution scroll. One wheel notch is ±120, matching the high-resolution scroll convention. `axis` reuses the `axis` enum defined on [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md).

### key

`key(keysym: uint, state: uint, origin: uint)`

| arg | type | description |
|---|---|---|
| `keysym` | `uint` | xkbcommon keysym, already modifier-resolved |
| `state` | `uint` enum [`key_state`](#key_state) | pressed or released |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

A key event for the **human path**: physical typing flows core→shim→app, and humans press chords, arrows, and function keys that a text string cannot carry. In v0 this event has no agent-facing source — agents actuate via [`text`](#text) — so it is exercised with `origin=physical`.

Keys travel as **xkbcommon keysyms, not keycodes**: layout-independent, requiring no keymap-relay message. The shim maps keysyms into the same dynamically generated keymap machinery that text delivery already requires.

**Where the keysym is resolved is a backend property of the core, not a property of this wire.** A nested core receives keys the host compositor has already interpreted and interprets nothing itself. A core driving physical input devices directly receives evdev scancodes and nothing else, so it resolves them against a keymap of its own before sending this event. Both produce a modifier-resolved keysym, both leave this interface unchanged, and a shim cannot tell them apart. Earlier revisions of this page and of the IDL said that **no keymap interpretation happens inside the core**, with "nested host input already arrives interpreted" as the justification; that justification is false for a libinput-driven backend, and the claim is corrected here rather than promoted to a rule the core would then have to break. See [D-028](../plan/20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode) for the decision, including why the keymap-relay growth path below was *not* taken.

**Normative modifier-suppression rule.** Keysyms arrive already modifier-resolved. The shim MUST bind each non-modifier keysym at an unmodified (plain) level of its dynamic keymap for replay, so that modifiers already applied are never applied twice. Modifier keysyms (`Shift_L`, `Control_L`, `Alt_L`, `Super_L`, and friends) are forwarded as ordinary key events and convey chord state to the app only; they MUST NOT change the level at which subsequent resolved keysyms bind. (This rules out the classic VNC double-shift bug normatively.)

Documented version-1 limitation: raw-scancode fidelity for keymap-sensitive apps is a later version's addition — a keymap relay (`keymap(fd, size, origin)`) plus a `keycode` event.

### text

`text(text: string, origin: uint)`

| arg | type | description |
|---|---|---|
| `text` | `string` | UTF-8 text to deliver (max 4096 bytes) |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

Text delivery — the **agent actuation path** in version 1, always `origin=emulated`. It is the seat-side counterpart of [`vitrin_actuator_text.type`](08-vitrin_actuator_text.md). The shim renders the string via its dynamic keymap; a newline (`\n`) MUST be rendered as `Return` and a tab (`\t`) as `Tab`, mirroring the actuator contract. The `origin` tag on text is what keeps a later phase's human input-method text (`origin=physical`) purely additive.

### relative_motion

`relative_motion(dx: fixed, dy: fixed, dx_unaccel: fixed, dy_unaccel: fixed, origin: uint)` · `since="2"` · opcode 5

| arg | type | description |
|---|---|---|
| `dx` | `fixed` | accelerated delta x, realm-view pixels (24.8 fixed-point) |
| `dy` | `fixed` | accelerated delta y, realm-view pixels |
| `dx_unaccel` | `fixed` | unaccelerated delta x, realm-view pixels |
| `dy_unaccel` | `fixed` | unaccelerated delta y, realm-view pixels |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

Pointer motion expressed as a **delta** rather than a destination. It **accompanies** [`motion`](#motion) rather than replacing it: one physical movement produces both, and the shim replays both, because an app binds whichever of the two it understands and must not have to guess which one this core sends.

**Two deltas, not one.** `dx`/`dy` are the movement *after* the core applied pointer acceleration — the movement that agrees with where the pointer visibly went. `dx_unaccel`/`dy_unaccel` are the same movement *before* acceleration, which is what a camera control or a drawing tool wants and is most of why this event exists. Carrying one alone would force a shim to copy it into the other, producing a field it would then be lying about.

**No timestamp, deliberately.** No event on this interface carries one, and the shim stamps each replay with its own clock; a device clock on the wire beside it would be a second, unsynchronised clock. The cost is paid rather than argued away: a consumer integrating deltas over `dt` gets the time the shim replayed them, not the time the device produced them.

**No pairing of its own** — no begin, no end. Where a latch could form is the **pointer lock** this event is the input half of: a lock stops absolute `motion` and freezes the position the core's own hit tests use, so a lock whose end is lost is a pointer that never moves again. Such state is therefore the core's to own and to end on every path that takes input away, and an *emulated* motion must not relocate the frozen position — the same defensive rule this interface already requires for the shared pointer position, now with a second reason to want it.

**The lock itself is not an event on this interface, and the reason is structural.** Every event here ends with `origin`, and `origin` has exactly two values: a human's device, or a principal's actuator. A lock is asked for by the **confined app**, which is neither, so any tag it carried would be false — on the one interface whose whole design idea is that the tag never drifts. The ask and the core's verdict therefore belong on [`vitrin_shim_session`](09-vitrin_shim_session.md), which already carries shim→core requests and is the only interface that can (B2 gives this one no requests at all). **Version 2 defines both halves there**: the [`pointer_constraint`](09-vitrin_shim_session.md#pointer_constraint-since-2) request and the [`pointer_constraint_state`](09-vitrin_shim_session.md#pointer_constraint_state-since-2) event. `relative_motion` is emitted for ordinary unlocked motion as well as under a lock — it is the input half either way, and a lock changes what accompanies it rather than whether it arrives. Call it a *pointer lock* or a *pointer constraint*, never a bare "constraint" — in this protocol that word already means a **petition** constraint ([`vitrin_realm.request_grant`](03-vitrin_realm.md)'s `flags`).

Version 2 defines no emulated source of a delta — the agent-facing [`vitrin_actuator_pointer.move`](07-vitrin_actuator_pointer.md) is absolute — so a version-2 core emits this event with `origin=physical` only. The tag is mandatory anyway, and is what keeps a later emulated source purely additive.

### gesture_begin

`gesture_begin(kind: uint, fingers: uint, origin: uint)` · `since="2"` · opcode 6

| arg | type | description |
|---|---|---|
| `kind` | `uint` enum [`gesture_kind`](#gesture_kind) | which gesture began (`swipe` = 0, `pinch` = 1) |
| `fingers` | `uint` | finger count, fixed for this gesture's life |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

A multi-finger touchpad gesture began. **Swipe and pinch share this event**, and share [`gesture_end`](#gesture_end), because the begin and end signatures of the two are identical in the pointer-gesture vocabulary these events serve. A signature is immutable forever, so four events with no dead argument is the shape — rather than six with duplicated ones, and rather than two phase-tagged events whose deltas and completion flag would each be meaningless in two phases out of three.

`fingers` is how many fingers the gesture began with, and is **fixed** for its life: a gesture that gains or loses a finger ends, and a new one begins.

**At most one gesture is in flight per seat.** Every `gesture_swipe_update`, `gesture_pinch_update` and `gesture_end` that follows belongs to this begin until that end. The core does not send a second begin while one is in flight, and does not send an update or an end for a begin it did not send; a shim that receives one anyway has met a core bug and **ignores** it (see the asymmetry note under [Events](#events)).

**What the core owes in return** is exactly one `gesture_end` per `gesture_begin` it sent, without exception — a begin with no end leaves an app accumulating a gesture forever, which is the latched-modifier failure wearing a new shape. When something takes input away mid-gesture **for good** (a realm switch, a seat pause), the core **mints** an end, `cancelled`, rather than dropping the gesture silently. [`gesture_end`](#gesture_end) states which paths those are, and which paths withhold a gesture's updates without minting one.

In version 2 the only source is a physical device: no actuator mints a gesture and no verb grants one.

### gesture_swipe_update

`gesture_swipe_update(dx: fixed, dy: fixed, origin: uint)` · `since="2"` · opcode 7

| arg | type | description |
|---|---|---|
| `dx` | `fixed` | delta x since this gesture's previous event, realm-view pixels |
| `dy` | `fixed` | delta y since this gesture's previous event, realm-view pixels |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

How far the finger group moved since the previous event of the gesture in flight, in realm-view pixels. A **delta**, never a position, and never cumulative.

Valid only between a [`gesture_begin`](#gesture_begin) whose `kind` is `swipe` and that gesture's [`gesture_end`](#gesture_end). It is a separate event from `gesture_pinch_update` rather than a shared one because pinch additionally carries `scale` and `rotation`: a single shared update would freeze two arguments meaningless for swipe into a signature that can never afterwards be changed.

### gesture_pinch_update

`gesture_pinch_update(dx: fixed, dy: fixed, scale: fixed, rotation: fixed, origin: uint)` · `since="2"` · opcode 8

| arg | type | description |
|---|---|---|
| `dx` | `fixed` | centre delta x since this gesture's previous event, realm-view pixels |
| `dy` | `fixed` | centre delta y since this gesture's previous event, realm-view pixels |
| `scale` | `fixed` | scale relative to this gesture's begin, 1.0 at the begin |
| `rotation` | `fixed` | degrees turned since this gesture's previous event, positive clockwise |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

The motion of the pinch in flight. Valid only between a [`gesture_begin`](#gesture_begin) whose `kind` is `pinch` and that gesture's [`gesture_end`](#gesture_end).

**The four quantities do not share a reference point**, and reading one as though it were another is the mistake the IDL's description exists to prevent:

- `dx`/`dy` — how far the centre of the finger group moved **since this gesture's previous event**, in realm-view pixels. Deltas.
- `scale` — the pinch's scale relative to **its own begin**, not to the previous event. It is 1.0 at the begin, and an app applies it to the size the object had at the begin; multiplying successive values into a running product is wrong and compounds. Absolute, not a delta.
- `rotation` — the angle turned **since this gesture's previous event**, in degrees, positive clockwise. A delta again.

That asymmetry is inherited rather than invented — it is what the pointer-gesture vocabulary defines and what the device layer under the core reports — and it is written out because a wire that quietly reinterpreted either quantity would be wrong in a way apps cannot detect: a scale read as a delta zooms toward zero, and a rotation read as absolute snaps back on every event.

### gesture_end

`gesture_end(kind: uint, state: uint, origin: uint)` · `since="2"` · opcode 9

| arg | type | description |
|---|---|---|
| `kind` | `uint` enum [`gesture_kind`](#gesture_kind) | which gesture ended; repeats the in-flight kind |
| `state` | `uint` enum [`gesture_state`](#gesture_state) | whether the human completed the gesture |
| `origin` | `uint` enum [`origin`](#origin) | who caused this event |

Ends the gesture the last [`gesture_begin`](#gesture_begin) started — the second half of the guarantee that begin states: exactly one of these per begin the core sent, always, on every path.

`state` distinguishes the two ways a gesture stops, and the distinction is what lets an app commit rather than guess. `completed` means the human finished it and what it did should stand; `cancelled` means it did not finish and a preview should be undone. **Cancelled covers the device layer's own cancellation *and* the core's own.** Version 2 ends a gesture `cancelled` on **exactly two core paths: a realm switch, and a seat pause.** On both, the human's fingers are still down and no end can ever arrive, so an end is minted rather than waited for. **A consent prompt and a screen lock do *not* mint one**: they withhold the gesture's updates and then deliver the device's own end when the human lifts, so no latch forms — but an app that was previewing is told the human `completed` what they in fact abandoned. That gap is named here rather than smoothed over, and closing it is owed. Where an end *is* minted, the app is told the truth about the gesture rather than a convenient falsehood about the human — the same trade this system already makes for keys and buttons held across such a moment.

`kind` repeats the in-flight gesture's kind. It is redundant by construction (only one gesture is ever in flight) and is carried anyway so that a disagreement with what the shim has in flight is a **detectable** core bug rather than a silent mis-replay. A shim that sees one ends the gesture it actually has and logs; as with an unpaired update, it does not close the connection over the core's mistake.

## Enums

### key_state

Key states.

| entry | value | meaning |
|---|---|---|
| `released` | 0 | key released |
| `pressed` | 1 | key pressed |

Note: value-identical to [`vitrin_actuator_pointer.button_state`](07-vitrin_actuator_pointer.md) but defined separately as its own semantic enum for the key axis.

### origin

Input origin (physical versus emulated) — the libei/EIS distinction, preserved through the hop where libei drops it. Present on every seat event as the final argument, from day one, because later phases hang physically-originated consent on it. Enforced structurally by the RNG schema.

| entry | value | meaning |
|---|---|---|
| `physical` | 0 | produced by a physical human input device |
| `emulated` | 1 | produced by a principal's actuator (or, later, motion synthesis) |

### gesture_kind

Which gesture a shared begin or end names. `gesture_begin` and `gesture_end` are shared by every gesture kind, so this is the argument that says which one.

| entry | value | meaning |
|---|---|---|
| `swipe` | 0 | multi-finger swipe; motion arrives as [`gesture_swipe_update`](#gesture_swipe_update) |
| `pinch` | 1 | pinch; motion arrives as [`gesture_pinch_update`](#gesture_pinch_update) |

**A further kind is an appended entry, not a new begin or end.** A hold, for instance, has the same begin and end shape and no motion at all, so serving it would add one entry here and no event — which is the whole reason begin and end are shared. Values are immutable and entries are never renumbered, like every enum here. Nothing beyond swipe and pinch is served yet; that is a *not yet*, and what would reopen it is recorded in [D-032](../plan/20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence) rather than on the wire — a wire cannot say what evidence would change it.

### gesture_state

How a gesture ended. This is an enum rather than a bare flag because every state argument on this interface is enum-typed, and because the difference is what an app acts on: a completed pinch keeps its zoom, a cancelled one puts it back.

| entry | value | meaning |
|---|---|---|
| `completed` | 0 | the human finished the gesture |
| `cancelled` | 1 | the gesture did not finish; a preview should be undone |

### Shared cross-interface enums

`button.state` uses `button_state` and `scroll.axis` uses `axis`, both defined on [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md). This interface references but does not redefine them.

## Flows

Direction key: `C→S` core→shim, `A→C` agent→core, `C→A` core→agent. Corrected for the final XML (co-minted facets in `request_grant`; keysym rather than keycode on the human path).

### Agent actuation reaching the seat — from scenario (b)

An agent clicks a URL bar and types a URL, after a grant covering `observe | actuate_pointer | actuate_text` resolves granted. Only the seat-touching tail is shown.

1. `A→C` `vitrin_actuator_pointer.move(x_urlbar, y_urlbar)` — realm-view pixels
2. `C→S` `vitrin_shim_seat.motion(x_urlbar, y_urlbar, emulated)` — shim maps view→surface-local, replays on its `wl_seat`
3. `A→C` `vitrin_actuator_pointer.button(BTN_LEFT, pressed)`
4. `A→C` `vitrin_actuator_pointer.button(BTN_LEFT, released)`
5. `C→S` `vitrin_shim_seat.button(BTN_LEFT, pressed, emulated)`
6. `C→S` `vitrin_shim_seat.button(BTN_LEFT, released, emulated)`
7. `A→C` `vitrin_actuator_text.type("http://127.0.0.1:8000/\n")` — Enter as trailing `\n`
8. `C→S` `vitrin_shim_seat.text("http://127.0.0.1:8000/\n", emulated)` — shim replays via dynamic keymap; `\n` ⇒ `Return`

Each agent actuation is chokepoint-checked on the principal connection *before* the corresponding seat event is emitted; a failed check yields a `refused` on the agent's [`vitrin_grant`](04-vitrin_grant.md) and **no** seat event is produced.

### Human input reaching the seat — scenario (i), physical origin end to end

The host compositor delivers input to `vitrind`'s nested window; that window *is* the human principal (implicit, no login — a documented MVP limitation). The core tags each event `origin=physical` and routes it to the realm's shim. There is no grant check for the human principal in MVP.

1. `C→S` `vitrin_shim_seat.motion(x, y, physical)` — realm-view coords, shim maps to surface-local
2. `C→S` `vitrin_shim_seat.button(BTN_LEFT, pressed, physical)`
3. `C→S` `vitrin_shim_seat.button(BTN_LEFT, released, physical)`
4. `C→S` `vitrin_shim_seat.key(keysym_F, pressed, physical)` — real key events, not text: humans press Ctrl+C, arrows, F5
5. `C→S` `vitrin_shim_seat.key(keysym_F, released, physical)`
6. The shim replays on its `wl_seat`; the app sees ordinary seat input. The physical/emulated distinction is preserved core→shim even though the legacy app cannot see it.

Mid-consent variant (preemption): while a consent prompt is up, physical input is grabbed exclusively by the prompt, so steps 1–5 do **not** reach the seat, and concurrent agent actuations are held or refused upstream (`consent_held` on the grant).

### A pinch the human does not get to finish — the version-2 pairing rule, shown

A human starts a two-finger pinch on a touchpad and, mid-gesture, switches the output to another realm. The gesture is never completed, and the app is told exactly that.

1. `C→S` `vitrin_shim_seat.gesture_begin(pinch, 2, physical)`
2. `C→S` `vitrin_shim_seat.gesture_pinch_update(dx, dy, 1.4, 0.0, physical)` — `scale` is relative to step 1, not to the previous update
3. `C→S` `vitrin_shim_seat.gesture_pinch_update(dx, dy, 1.9, 2.5, physical)` — `scale` still relative to step 1; `rotation` is 2.5° *since step 2*
4. The human switches realms. The core's drain runs, exactly as it does for keys and buttons held across the same moment.
5. `C→S` `vitrin_shim_seat.gesture_end(pinch, cancelled, physical)` — **`cancelled`, not `completed`**: the human did not let go, so the app undoes its preview rather than committing a zoom the human never confirmed.

Step 5 is not optional and not best-effort. It is the core's side of the guarantee [`gesture_begin`](#gesture_begin) states, and the reason a `gesture_begin` with no `gesture_end` is treated as the latched-modifier failure in a new shape.

## Growth

The interface is designed so that every version-2+ addition is purely additive — appended events under `since="2"`, never a changed signature (message signatures are immutable forever; see [conventions](00-conventions.md) versioning). Version 2 exercised that design for the first time, and the five events it appended are the worked example the seams below copy.

> **Event opcodes 5–9 are taken; the next appended event on this interface starts at 10.** Opcodes are implicit document order, numbered separately from 0 for requests and events ([conventions](00-conventions.md) §7.4). The version-1 five are 0–4 and version 2's are 5–9. None of the seams below reserves a number — they name mechanisms only — so whoever lands the focus event next takes 10, not 5.

- **Relative pointer motion** *(landed, version 2)*. [`relative_motion`](#relative_motion) at opcode 5. The one dropped class whose shape the existing pointer vocabulary was already half-built for, and the input a pointer lock actually delivers.
- **Pinch and multi-finger swipe** *(landed, version 2)*. [`gesture_begin`](#gesture_begin) / [`gesture_swipe_update`](#gesture_swipe_update) / [`gesture_pinch_update`](#gesture_pinch_update) / [`gesture_end`](#gesture_end) at opcodes 6–9, plus [`gesture_kind`](#gesture_kind) and [`gesture_state`](#gesture_state). Two-finger scroll was already served by [`scroll`](#scroll), so this closed a narrower gap than "gestures".
- **The pointer lock's ask and verdict** *(landed, version 2 — and **not on this interface**)*. [`relative_motion`](#relative_motion) is the input half of a pointer lock; the half that asks for one and answers it could not live here. B2 makes `origin` the mandatory final argument of every event on this interface, `origin` names a human device or a principal's actuator, and a lock is asked for by the *confined app*, which is neither — a tag it carried would be false. So the pair is a shim→core request and a core→shim event on [`vitrin_shim_session`](09-vitrin_shim_session.md), the interface that already carries shim→core requests and the only one that can (this one has none, by schema): [`pointer_constraint`](09-vitrin_shim_session.md#pointer_constraint-since-2) and [`pointer_constraint_state`](09-vitrin_shim_session.md#pointer_constraint_state-since-2). This bullet stays as the record of *why* it landed there rather than here — and it took **no** event opcode on this interface, so the "next appended event starts at 10" note above is unaffected.
- **Tagged focus event.** v0 is single-surface, with focus synthesized shim-side (pointer-enter on first motion, keyboard focus held on the app). A multi-surface realm gains an explicit focus event, itself origin-tagged, as a later addition. Because v0 has no focus event to change, this is additive.
- **Raw-scancode fidelity for keymap-sensitive apps.** v0 delivers keysyms only. A later version adds a keymap relay (`keymap(fd, size, origin)`) plus a `keycode` event, so keymap-sensitive apps can receive raw scancodes against a relayed keymap. The v0 `key` event is unchanged. [D-028](../plan/20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode) **considered taking this path early and declined it**, and the reason bounds what it is for: a relay moves modifier resolution to the shim, which inverts the normative rule above and would make agent `text` actuation and human keys resolve through two different mechanisms. Because that decision left the wire untouched, this addition stays purely additive and is not foreclosed.
- **Physically-originated consent.** No new seat message, but the `origin=physical` tag present on every event from day one is the structural hook a later phase uses to let physical input drive consent decisions. Nothing new on the wire is required for the tag itself.
- **Sub-pixel motion synthesis.** The fixed-point `motion` coordinates already carry the fractional headroom for server-side motion synthesis; a later phase can emit sub-pixel motion with no signature change.
- **Per-principal pointer delivery.** v0 delivers one shared pointer position and so does version 2; the per-principal model (D-017) arrives as `since`-gated sibling events that name the principal alongside the coordinates. They are siblings rather than a changed `motion`, because a signature is immutable forever — and because each new event must still end with `origin`, the B2 rule is satisfied structurally rather than by review. The five v0 events stay valid, as do version 2's; a shim that speaks only the v0 five keeps working against a core that also speaks the newer ones.
- **Input classes this interface does not yet carry.** Touch and tablet have **no event here yet**, and that is a *not yet* rather than a refusal: the decision that left them out is a measurement of one machine's device set, not a property of the class, and a permanent wire protocol may not foreclose a device class on that ground. Each is recorded in [D-032](../plan/20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence) with the evidence that reopens it — for touch, a touchscreen in the measured device set *and* an application that needs it; for tablet, a tablet or stylus device, the application half of its evidence being already banked. Either arrives the way version 2's five did: appended `since`-gated events, each ending with `origin`, changing nothing already here. What the shim must **not** do meanwhile is advertise a `wl_seat` capability it cannot deliver — a class advertised but never delivered is worse than an absent one, which is why the shim advertises `POINTER | KEYBOARD` and no `wl_touch`.
