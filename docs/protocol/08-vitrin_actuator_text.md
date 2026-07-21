# vitrin_actuator_text — the text-injection facet

**Interface version:** 1 · **Connection class:** principal · **Grant verb:** `actuate_text` · **Messages:** 1 request + 0 events

## Purpose

`vitrin_actuator_text` is the capability facet through which a principal delivers Unicode text to a granted realm target. Its single semantic is *"deliver this Unicode string"* — never *"press these keys."* The payload bypasses keymaps and input methods entirely; the delivery path (a dynamically generated keymap synthesized by the shim) is an implementation detail the agent cannot observe or address. An agent that wants a page to receive the characters `héllo世界` sends exactly those characters, and the [shim seat](11-vitrin_shim_seat.md) replays them against the app — no charset negotiation, no keycode arithmetic, no double-shift hazard.

In the object graph this interface is one of the three authority facets co-minted by [`vitrin_realm.request_grant`](03-vitrin_realm.md), alongside [`vitrin_view`](06-vitrin_view.md) (observation) and [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) (pointer actuation). All three hang off a single [`vitrin_grant`](04-vitrin_grant.md) capability handle, and all use of them is checked at one server-side enforcement chokepoint whose refusals are delivered on that grant. The facet carries no authority of its own: it is a typed name that the chokepoint validates against the grant's effective verb set at the moment of each request.

The design idea is a clean separation between *intent* and *mechanism*. Agents speak in the vocabulary they can verify — Unicode text they observed in a captured frame or composed themselves — while the mechanism of turning that text into input events the app understands lives entirely below the wire, inside the trusted shim. This keeps xkb, keymaps, and input methods out of the agent's trusted surface, and it keeps the wire message stable across every layout the app might use.

Key chords (Ctrl+C, Alt+Tab, and the like) are deliberately **not** expressible by agents in version 1. Chord actuation is a distinct verb — key actuation — reserved for a later phase; see [Growth](#growth). Version 1 draws the line at "deliver text," and the one control-character escape below is the only structured input the facet admits.

## Lifecycle

An instance of `vitrin_actuator_text` comes into existence only as the `text` argument of [`request_grant`](03-vitrin_realm.md#request_grant): that one request atomically mints the grant handle, its consent observer, and the three facets (view, pointer, text) as distinct `new_id`s obeying the [multi-`new_id` rule](00-conventions.md#32-multi-new_id-rule). There is no getter and no other constructor — a principal cannot obtain a text facet except by petitioning.

The facet is **born inert**. It confers nothing until its grant resolves `granted` *and* the effective verb set carries the `actuate_text` bit (see [`vitrin_grant.verb`](04-vitrin_grant.md#verb)). While the grant is pending, and forever after if the grant resolves `denied`, `timed_out`, `unavailable`, `unsupported`, or `busy`, every request on this facet refuses recoverably with [`not_granted`](04-vitrin_grant.md#refusal). The same holds if the grant later goes dead (expiry or revocation) or if `actuate_text` was requested but the human granted a narrower verb set: inert facets refuse, they never fault.

Version 1 defines **no destructor** on this interface. The facet is not independently released; it dies when its grant dies, which in v1 happens when the grant expires, is revoked, or the connection closes (all v1 grants die with the connection — there is no grant persistence). Per the [inert-ID rule](00-conventions.md#33-no-destructors-inert-objects-tolerate-dead-events), object ids are never reused, so any late request the agent sends to a dead facet is safely answered with a recoverable refusal rather than a fatal `invalid_object`.

## Requests

### type

```
type(text: string)
```

| arg | type | description |
| --- | --- | --- |
| `text` | string | UTF-8 text to deliver, at most 4096 bytes (byte length of the encoded string, not codepoint count). |

Delivers the UTF-8 `text` to the granted target. The string is injected as characters, not as key events; the shim's dynamic keymap is responsible for making each character appear in the focused app.

**Control-character rule (normative).** Exactly two C0 control characters are legal in the payload, and each has a fixed rendering the delivery path MUST honor:

- a newline `U+000A` MUST be rendered as a **Return** keypress, and
- a tab `U+0009` MUST be rendered as a **Tab** keypress.

This is the mechanism by which an agent submits a form or presses Enter after typing a URL: the Enter travels as a trailing `\n` in the same `type` payload (see [Flow 1](#flow-1--type-a-url-and-press-enter)). **Every other control character is fatal** — the rest of C0 (`U+0000`–`U+001F`), DEL (`U+007F`), and C1 (`U+0080`–`U+009F`), i.e. the whole Unicode `Cc` category. A correct client never emits them; a payload containing any such control character is a client bug, resolved as the fatal error `invalid_argument`.

**Failure modes.**

- *Fatal* (`invalid_argument`, carried on [`vitrin_handshake.error`](01-vitrin_handshake.md#error), then the connection closes): the payload exceeds 4096 bytes, is not well-formed UTF-8, contains a NUL, or contains a disallowed control character. These are grammar violations the client could have known about — see the [error razor](00-conventions.md#5-error-taxonomy).
- *Recoverable* refusals arrive as [`vitrin_grant.refused`](04-vitrin_grant.md#refused)`(verb = actuate_text, code, retry_after_ms)`. The applicable [`refusal`](04-vitrin_grant.md#refusal) codes are the full chokepoint set: `not_granted` (grant pending, denied, dead, or `actuate_text` outside the effective verb set), `expired`, `revoked`, `rate_limited` (with `retry_after_ms > 0`), `preempted` (physical human input owns the target), `consent_held` (a consent prompt is up; agent actuation is refused, never delivered to the app), `no_surface` (the realm's shim crashed or exited), and `internal`.

**Delivery class: fire-and-forget.** `type` bears no reply. A well-formed, authorized `type` produces no wire acknowledgement; its effect is observed by capturing a later frame. Because it is fire-and-forget, its refusals MAY be coalesced per the [delivery classification](00-conventions.md#6-delivery-classification): at most one `refused` per grant per `(verb, code)` until a subsequent request on that grant succeeds, and at most one `refused(rate_limited)` per grant per bucket-refill window. An agent that needs to bound error discovery to one round trip issues a [`sync`](01-vitrin_handshake.md#sync) barrier after its actuations and waits for `done`.

## Enums

This interface defines no enums of its own. Its single request references one shared enum defined elsewhere:

- **`vitrin_grant.verb`** — the grantable-verb bitfield. This facet corresponds to the `actuate_text` bit (value `4`). See [`vitrin_grant.verb`](04-vitrin_grant.md#verb). Refusals on this facet always carry `verb = actuate_text`.

The `refusal` codes cited under [`type`](#type) are likewise defined on [`vitrin_grant`](04-vitrin_grant.md#refusal), not here.

## Flows

Message sequences below use the direction key from the [conventions page](00-conventions.md): `A→C` agent→core, `C→A` core→agent, `C→S` core→shim, `[ ]` out-of-band.

### Flow 1 — type a URL and press Enter

Excerpt of the M1.4 demo (scenario **b**), from the point the grant is active and the URL bar has been clicked. `request_grant` has already co-minted the `view`, `pointer`, and `text` facets; the grant resolved `granted` with `observe | actuate_pointer | actuate_text`.

1. `A→C` `view.capture_frame()` → `C→A` `view.frame_ready(...)` — SDK locates the URL bar by pixels.
2. `A→C` `pointer.move(x_urlbar, y_urlbar)`; `A→C` `pointer.button(BTN_LEFT, pressed)`; `A→C` `pointer.button(BTN_LEFT, released)` — realm-view pixel coordinates; the shim maps and replays them (see [pointer flows](07-vitrin_actuator_pointer.md#flows)).
3. `[Firefox focuses the URL bar and repaints]`
4. `A→C` **`text.type("http://127.0.0.1:8000/\n")`** — the Enter travels as the trailing `\n` in the same payload.
5. `C→S` `shim_seat.text("http://127.0.0.1:8000/\n", origin = emulated)` — the shim replays via its dynamic keymap; the `\n` becomes a Return keysym.
6. `A→C` `handshake.sync(cookie)` → `C→A` `handshake.done(cookie)` — flush any pending refusal before asserting.
7. `A→C` `view.capture_frame()` → `C→A` `view.frame_ready(...)` — SDK asserts the page changed versus the frame from step 1.

There is no wire event between steps 4 and 6 in the success case: `type` is fire-and-forget, and the barrier in step 6 is what proves no refusal was queued.

### Flow 2 — actuation after expiry

From scenario **e**: a grant issued with a 5-second expiry; the clock advances and the grant is marked expired (by a proactive timer or by the check at use time).

1. `A→C` `text.type("late")`
2. `C→A` `vitrin_grant.refused(verb = actuate_text, code = expired, retry_after_ms = 0)` — the text never reaches the shim; the SDK raises `GrantExpired`.

The same shape applies to revocation (scenario **d**, `code = revoked`), rate-limiting (`code = rate_limited`, `retry_after_ms > 0`), a held consent prompt (`code = consent_held`), and shim death (`code = no_surface`): one chokepoint, one refusal voice, the `verb` field always naming `actuate_text`.

## Growth

Every seam below is purely additive: version-1 signatures are immutable, and each addition is a new message or a new appended enum entry, never a change to `type`.

- **Key actuation (a distinct verb).** Chords and raw key input are out of scope for `actuate_text` by design. A later phase introduces a separate key-actuation verb; it appends a new bit to [`vitrin_grant.verb`](04-vitrin_grant.md#verb) (the version-1 bits `observe`/`actuate_pointer`/`actuate_text` are untouched) and adds its own facet. Text delivery and key actuation stay orthogonal capabilities, so `type` never has to grow a "modifier" argument.
- **Value-bearing text primitives** (should any arrive) would be appended as sibling requests gated by `since="2"`, exactly as the [pointer facet](07-vitrin_actuator_pointer.md#growth) reserves room for intent-level motion. The version-1 `type` primitive stays valid forever.

None of these changes the wire shape of `type` or the inert-until-granted lifecycle described above.
