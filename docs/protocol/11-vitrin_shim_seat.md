# vitrin_shim_seat — origin-tagged input delivery from core to the shim

**Interface version:** 1 · **Connection class:** shim · **Messages:** 0 requests + 5 events

## Purpose

`vitrin_shim_seat` is the input-delivery interface of a shim connection: it carries pointer motion, buttons, scroll, keys, and text *from the core to the shim*, which then replays them to its nested application through its own virtual `wl_seat`. It is an **events-only** interface — the shim never sends on it. The core owns the mapping from principals (agent actuators, and in a later phase physical human input) to shim seats; the shim is a dumb replay target that knows nothing of grants, verbs, or principals. See [conventions](00-conventions.md) for the two connection classes and the shim's structural (socketpair-inherited) authentication.

The seat sits under [`vitrin_shim_session`](09-vitrin_shim_session.md): a session mints exactly one seat via `get_seat` (a second attempt is the log-and-close condition `already_initialized`). Where [`vitrin_shim_surface`](10-vitrin_shim_surface.md) is the *output* half of the shim connection (buffers flowing shim→core), the seat is the *input* half (events flowing core→shim). Two agent-facing actuator interfaces feed it: a principal's [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) request becomes a `motion`/`button`/`scroll` event here, and a [`vitrin_actuator_text`](08-vitrin_actuator_text.md) `type` request becomes a `text` event here. The `key` event has no v0 agent-facing source — it exists for the human path.

The organising design idea is **per-event origin tagging**. Every event on this interface carries `origin` as its final argument, distinguishing input a physical human device produced (`physical`) from input a principal's actuator emulated (`emulated`). This preserves end to end the distinction that libei/EIS drops at the hop into the compositor. The tag is present *from day one* on every event, never as a modal start/stop envelope — modal "physical input begins / ends" framing desyncs and fuzzes badly, whereas per-event tags cannot drift out of sync. This structural rule (last arg MUST be `origin` with `enum="origin"`) is enforced by the RNG schema, not merely by convention. It is load-bearing for a later phase in which physically-originated consent is built directly on the physical/emulated distinction.

Two coordinate and focus conventions frame the interface. Pointer coordinates are **realm-view pixels** (the same pixel space agents address via the actuator); the shim maps view coordinates to surface-local coordinates before replaying. Focus in version 1 is **synthesized shim-side**: the realm is single-surface, so the shim generates pointer-enter on first motion and holds keyboard focus on the app. There is no wire focus event in v0; an explicit tagged focus event is the multi-surface addition of a later version.

**One shared pointer position in version 1.** These events carry `origin`, not principal identity, so a realm's app sees a single pointer whoever moved it. This is the version-1 half of the per-principal cursor model (D-017; see [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md#the-cursor-model)), and it is the reason the core carries a defensive rule keeping emulated motion from relocating the position a consent grab hit-tests. The agent-facing side of the wire is already principal-relative — `move` moves *that principal's* pointer — so the gap is exactly here, in delivery. Per-principal delivery is deferred to M2 and arrives as `since`-gated sibling events that also name the principal, each still ending with `origin` so B2 holds; these five events stay valid forever.

## Lifecycle

A seat instance comes to exist when the shim calls `get_seat(seat)` on its [`vitrin_shim_session`](09-vitrin_shim_session.md) (object 1 of the shim connection). Exactly one seat exists per session; a second `get_seat` is the shim log-only fatal condition `already_initialized` (log-and-close, no wire error message — shim connections use log-and-close rather than the principal connection's `error` carrier; see [conventions](00-conventions.md)).

The seat is not grant-derived and therefore has no inert semantics of its own: it lives for the life of the shim connection. It is destroyed when that connection ends. The normal end is shim EOF — the core detects the socketpair closing, treats it as shim death, and tears the seat down along with the surface and session. v0 defines **zero destructors**, so there is no `release`/`destroy` request for the seat; teardown is by connection close only.

Because the seat carries no client-created object references and mints no `new_id`s, the [inert-ID tolerance rule](00-conventions.md) does not bear on it directly, but the general guarantee holds: IDs are never reused, so any late-arriving event is safe to interpret.

## Events

All five events are **fire-and-forget** (no reply, no terminal event, delivery class per [conventions](00-conventions.md)). The core emits them core→shim; the shim replays them and never acknowledges on the wire. There are no recoverable refusals or fatal codes *on this interface* — enforcement (grant active? verb granted? rate? expiry?) happens upstream at the single chokepoint on the principal connection, before the core ever emits a seat event. By the time an event reaches the seat, the authority question is already settled. Malformed shim-connection traffic is handled by the session's log-and-close conditions, not here.

Every event's final argument is `origin` (enum [`origin`](#origin)); this is schema-enforced.

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

## Growth

The interface is designed so that every version-2+ addition is purely additive — appended events under `since="2"`, never a changed signature (message signatures are immutable forever; see [conventions](00-conventions.md) versioning).

- **Tagged focus event.** v0 is single-surface, with focus synthesized shim-side (pointer-enter on first motion, keyboard focus held on the app). A multi-surface realm gains an explicit focus event, itself origin-tagged, as a later addition. Because v0 has no focus event to change, this is additive.
- **Raw-scancode fidelity for keymap-sensitive apps.** v0 delivers keysyms only. A later version adds a keymap relay (`keymap(fd, size, origin)`) plus a `keycode` event, so keymap-sensitive apps can receive raw scancodes against a relayed keymap. The v0 `key` event is unchanged. [D-028](../plan/20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode) **considered taking this path early and declined it**, and the reason bounds what it is for: a relay moves modifier resolution to the shim, which inverts the normative rule above and would make agent `text` actuation and human keys resolve through two different mechanisms. Because that decision left the wire untouched, this addition stays purely additive and is not foreclosed.
- **Physically-originated consent.** No new seat message, but the `origin=physical` tag present on every event from day one is the structural hook a later phase uses to let physical input drive consent decisions. Nothing new on the wire is required for the tag itself.
- **Sub-pixel motion synthesis.** The fixed-point `motion` coordinates already carry the fractional headroom for server-side motion synthesis; a later phase can emit sub-pixel motion with no signature change.
- **Per-principal pointer delivery.** v0 delivers one shared pointer position; the per-principal model (D-017) arrives as `since`-gated sibling events that name the principal alongside the coordinates. They are siblings rather than a changed `motion`, because a signature is immutable forever — and because each new event must still end with `origin`, the B2 rule is satisfied structurally rather than by review. The five v0 events stay valid; a shim that speaks only them keeps working against a core that also speaks the new ones.
