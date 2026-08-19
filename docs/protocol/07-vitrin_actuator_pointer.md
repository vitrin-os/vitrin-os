# vitrin_actuator_pointer — pointer actuation facet

**Interface version:** 1 · **Connection class:** principal · **Grant verb:** `actuate_pointer` · **Messages:** 3 requests + 0 events

> Framing, object-id rules, the fatal/recoverable taxonomy, delivery classification, and versioning are defined once in [00-conventions.md](00-conventions.md) and are not restated here. This page documents only what `vitrin_actuator_pointer` adds.

## Purpose

`vitrin_actuator_pointer` is the pointer-injection capability: it folds pointer motion, button presses, and scrolling into one facet for version 1. It is one of the three authority facets that a principal receives from a grant — the sibling of [`vitrin_view`](06-vitrin_view.md) (observation) and [`vitrin_actuator_text`](08-vitrin_actuator_text.md) (text injection). Where the view lets an agent *see* the realm, this facet lets it *point*.

The facet sits below a [`vitrin_grant`](04-vitrin_grant.md) in the object graph: it is co-minted with the grant by [`vitrin_realm.request_grant`](03-vitrin_realm.md) and never exists independently of one. It carries the interface annotation `verb="actuate_pointer"`, which means every request on it exercises that verb; the scanner uses this annotation to build the `(interface, opcode) → required-verb` table that the enforcement chokepoint consults. The facet holds no state of its own — it is a thin, typed door onto the grant's single server-side enforcement function.

The design idea is coordinate honesty. All coordinates are **realm-view pixels**: what the agent observed in a captured frame from [`vitrin_view`](06-vitrin_view.md) is exactly what it addresses here. The core (and, on the shim side, [`vitrin_shim_seat`](11-vitrin_shim_seat.md)) maps realm-view coordinates down to surface-local coordinates; the agent never reasons about surface geometry. Coordinates outside the view are clamped, not rejected — detecting that a target moved underneath a stale observation is deferred to the epoch mechanism of a later phase (see [Growth](#growth)).

The facet is deliberately primitive. It offers `move`, `button`, and `scroll` and nothing else; a client-level click is composed as `move` → `button` press → `button` release → [`sync`](01-vitrin_handshake.md). Intent-level motion (a drag with duration and easing, interpolated server-side) is a later addition that arrives as sibling requests, leaving these primitives valid forever.

## The cursor model

PRD §5.1's P1 promises each principal "its own cursor (or cursorless)". This interface is where that lands on the wire. Decision **D-017** settles it; the summary below is normative prose restating the IDL.

**Identity: the facet is the cursor's name.** Each principal has exactly one virtual pointer per realm it holds pointer authority over, and `vitrin_actuator_pointer` is that pointer's only name on the wire. `move` moves *this principal's* pointer, never a shared one. This is why per-principal cursors need no new agent-facing vocabulary: the agent-facing half of the wire is already principal-relative, and object ids are per-connection and sender-constrained, so a principal structurally cannot address another's pointer.

**Cursorless is by construction, not by declaration.** A principal that never petitions for `actuate_pointer` has no pointer. The headless-fleet case — an agent that only observes — costs nothing and needs no wire message. There is deliberately **no request** by which a principal declares, disowns, or hides a cursor, and the absence of a *hide* request is itself a decision: a visually distinct agent cursor is a **human override** (PRD P10), so cursor visibility is never the actuating principal's own choice.

**Cursors are core-composited.** A realm may never supply the pointer bitmap: [`vitrin_shim_surface`](10-vitrin_shim_surface.md) has no cursor-surface role and will not gain one. A realm that drew its own could paint a **decoy cursor** and mislead the human about where input is going — the same spoofing class the consent surface exists to exclude. A pointer image an app paints is ordinary content inside its realm view, never the pointer the core composites.

**Who may see whose** is a relation, not a flag, and it is settled on the observation side — asymmetrically, on purpose. A captured frame contains no cursor except the human's, and that one only for a grant holding the distinct `observe_cursor` verb (meaningful only alongside `observe`, and refused `unsupported` in version 1). Seeing another *agent's* cursor is not purchasable by any verb, at any verb set, ever. See [`vitrin_view`](06-vitrin_view.md#what-a-capture-does-not-contain) for the table.

### Version-1 limitation, stated rather than implied

Version 1 delivers **one shared pointer position** per realm view to the shim: [`vitrin_shim_seat`](11-vitrin_shim_seat.md) events carry `origin`, not principal identity, so a realm's app sees a single pointer whoever moved it. **Drawing is not delivery:** the core *does* composite this principal's own cursor into human-visible output, from a position only this principal's motion moves, and that changes nothing about what the shim is delivered. Whether the **human's** cursor is composited depends on who owns the display, and the answer changes nothing about delivery: in nested operation the core composites none, because the host desktop draws it outside the realm view entirely; on bare metal (WS-E.3.2, [D-029](../plan/20-decision-log.md)), where there is no host desktop, the core draws the human's pointer itself, or the human has none. Both answers sit at the human-visible output stage, alongside the consent overlay and the trust indicator, so neither can reach a captured frame.

That shared position is why the core needs a defensive rule in its consent grab: emulated motion must not relocate the position the human's hit test reads, or an agent holding a pointer grant could slide the hit target under the human's finger and turn a click aimed at *Deny* into an *Allow*. **Per-principal delivery deletes that special case rather than complicating it** — the human's hit test follows the human's pointer because they are structurally distinct. It also clarifies preemption: with one pointer, "physical input preempts agent input" is a contention rule; with N+1 pointers there is nothing to contend for on the pointer axis, and preemption is purely about focus and actuation ordering.

Per-principal delivery is **deferred to M2** (spec 1.0-candidate) and arrives as `since`-gated sibling events on `vitrin_shim_seat` that also name the principal. Nothing on *this* interface changes when it does.

## Lifecycle

Instances of `vitrin_actuator_pointer` come into existence in exactly one way: as one of the five `new_id` arguments of [`vitrin_realm.request_grant`](03-vitrin_realm.md). There is no factory request on this interface and no other path to one. The `new_id` obeys the multi-`new_id` rule (distinct, strictly increasing in argument order, above the connection watermark; see [00-conventions.md](00-conventions.md)).

The facet is **born inert**. From petition time it confers nothing: it exists as a live object id, but every request on it is checked at *use* time against the grant's effective verb set at the single enforcement chokepoint. Until the grant emits [`resolved(granted, …)`](04-vitrin_grant.md) with `actuate_pointer` in its effective verb set, every request here is refused recoverably with [`vitrin_grant.refused(actuate_pointer, not_granted, …)`](04-vitrin_grant.md). A grant that resolves without `actuate_pointer` (the human narrowed the verb set) leaves this facet permanently inert.

Version 1 defines **no destructor** on this interface. The facet lives for the connection; when its grant later expires or is revoked the facet goes inert again — its requests yield recoverable refusals (`expired`, `revoked`), never a fatal `invalid_object`. A release destructor is a documented growth seam on [`vitrin_grant`](04-vitrin_grant.md), not here. Because ids are never reused, a client that keeps sending after inertness is always answered recoverably.

## Requests

All three requests are **fire-and-forget**: they carry no reply and receive no per-request acknowledgement. When the chokepoint refuses one, the failure arrives asynchronously as [`vitrin_grant.refused(actuate_pointer, …)`](04-vitrin_grant.md), which MAY be coalesced per the delivery classification (at most one `refused(rate_limited)` per grant per bucket-refill window; at most one `refused` per grant per `(verb, code)` pair until a subsequent request on the grant succeeds). A client bounds refusal discovery to one round trip by following its actuations with [`vitrin_handshake.sync`](01-vitrin_handshake.md) and reading until `done`, raising on any `refused` seen.

Coordinates outside the realm view are **clamped**, not refused — clamping is a non-error, documented behavior, not a wire signal.

### `move(x: int, y: int)`

| arg | type | description |
|---|---|---|
| `x` | int | realm-view x in pixels (signed; clamped into the view) |
| `y` | int | realm-view y in pixels (signed; clamped into the view) |

Moves this principal's virtual pointer to realm-view `(x, y)`. The agent-facing coordinate is an integer because agents address captured pixels; the corresponding [`vitrin_shim_seat.motion`](11-vitrin_shim_seat.md) event carries fixed-point coordinates so later server-side motion synthesis can be sub-pixel without a signature change.

**Delivery:** fire-and-forget. **Failure:** recoverable via `vitrin_grant.refused(actuate_pointer, …)`.

### `button(button: uint, state: button_state)`

| arg | type | description |
|---|---|---|
| `button` | uint | Linux evdev button code (e.g. `BTN_LEFT` = `0x110`). Not enum-bounded; any evdev code is accepted |
| `state` | uint (enum [`button_state`](#enum-button_state)) | `pressed` or `released` |

Presses or releases `button` at the current pointer position. The button code is a raw Linux evdev code and is not validated against an enum on the wire; `state` is strictly enum-typed, so an out-of-range `state` value is a fatal `invalid_argument` (see [Failure modes](#failure-modes)).

**Delivery:** fire-and-forget. **Failure:** recoverable via `vitrin_grant.refused(actuate_pointer, …)`.

### `scroll(axis: axis, value120: int)`

| arg | type | description |
|---|---|---|
| `axis` | uint (enum [`axis`](#enum-axis)) | `vertical` or `horizontal` |
| `value120` | int | high-resolution scroll amount; one wheel notch is `+120` or `-120` |

High-resolution scroll on one axis. The `value120` unit follows the Wayland high-resolution scroll convention: one physical wheel notch equals `±120`, so fractional and continuous scroll are expressible without a distinct discrete-scroll message.

**Delivery:** fire-and-forget. **Failure:** recoverable via `vitrin_grant.refused(actuate_pointer, …)`.

### Failure modes

**Fatal** (connection dies via [`vitrin_handshake.error`](01-vitrin_handshake.md); the client violated something it could have known):

- An out-of-range value for the strictly-typed enum arguments (`button.state`, `scroll.axis`) is fatal `invalid_argument`.
- Generic grammar and object-graph violations (unknown opcode, malformed frame, unexpected fd, an id at or below the watermark) are the connection-global fatal codes defined in [00-conventions.md](00-conventions.md). No fatal code is specific to this interface.

**Recoverable** (the facet lives; a well-formed request whose authority or target changed underneath it) — every failure of a *well-formed* pointer request surfaces as [`vitrin_grant.refused(verb = actuate_pointer, code, retry_after_ms)`](04-vitrin_grant.md). The applicable `refusal` codes are:

- `not_granted` — the grant is not (or not yet) active, was denied, or `actuate_pointer` is outside its effective verb set (the inert-facet case).
- `expired` — the grant's expiry passed.
- `revoked` — the grant was revoked (hold-Esc, panel, or policy); effective on the very next request.
- `rate_limited` — the grant's `max_event_rate` token bucket is empty; `retry_after_ms` hints the refill.
- `preempted` — physical human input owns the target right now. *(What "the target" is with several realms is the server's to decide and the IDL does not fix it; see [`vitrin_grant`](04-vitrin_grant.md#refusal) for what the reference core answers.)*
- `consent_held` — a consent prompt is up; agent actuation is refused, never delivered to the app.
- `no_surface` — the realm has no surface (its shim crashed or exited).
- `internal` — a server-side failure during this use.

## Enums

Both enums are **defined on this interface** and are **shared** cross-interface: [`vitrin_shim_seat.button`](11-vitrin_shim_seat.md) references `vitrin_actuator_pointer.button_state`, and [`vitrin_shim_seat.scroll`](11-vitrin_shim_seat.md) references `vitrin_actuator_pointer.axis`. This keeps the agent actuation path and the human/replay path on one vocabulary.

### enum `button_state`

| entry | value | meaning |
|---|---|---|
| `released` | 0 | button released |
| `pressed` | 1 | button pressed |

### enum `axis`

| entry | value | meaning |
|---|---|---|
| `vertical` | 0 | vertical scroll |
| `horizontal` | 1 | horizontal scroll |

Neither enum is a bitfield.

## Flows

Message sequences below use the direction key **A→C** (agent→core) and **C→A** (core→agent). They are the scenarios from the canonical flow set that exercise this facet, corrected for the final XML shapes: the three facets are co-minted by `request_grant` (there is no `grant.bind` step), the petition terminal is [`vitrin_grant.resolved`](04-vitrin_grant.md), and use-time failures arrive on [`vitrin_grant.refused`](04-vitrin_grant.md).

### Flow 1 — click the URL bar (M1.4 demo, pointer segment)

Prerequisite: a grant has resolved `granted` with `actuate_pointer` in its effective verb set, co-minting `pointer` (this facet). A click is `move` + press + release + `sync`.

1. A→C `vitrin_actuator_pointer.move(x_urlbar, y_urlbar)` — realm-view pixels located from a prior capture.
2. A→C `vitrin_actuator_pointer.button(BTN_LEFT, pressed)`
3. A→C `vitrin_actuator_pointer.button(BTN_LEFT, released)`
4. A→C `vitrin_handshake.sync(cookie)`
5. C→A `vitrin_handshake.done(cookie)` — no `refused` arrived ahead of it, so all three actuations were delivered.

(Server-side, each accepted request is relayed to the app's shim as an origin-tagged [`vitrin_shim_seat`](11-vitrin_shim_seat.md) event with `origin = emulated`.)

### Flow 2 — revocation mid-loop (hold-Esc)

Prerequisite: an active grant; the human holds Esc and the core revokes it.

1. A→C `vitrin_actuator_pointer.move(x, y)` — the next actuation after revocation.
2. C→A `vitrin_grant.refused(actuate_pointer, revoked, 0)` — the actuation never reached the shim; the SDK raises `Revoked`. With a following `sync` this is observable within one round trip.

### Flow 3 — expiry

Prerequisite: a grant issued with a short expiry that has now elapsed.

1. A→C `vitrin_actuator_pointer.move(x, y)`
2. C→A `vitrin_grant.refused(actuate_pointer, expired, 0)` — the SDK raises `GrantExpired`. The same code appears on the [`vitrin_actuator_text.type`](08-vitrin_actuator_text.md) and [`vitrin_view.capture_frame`](06-vitrin_view.md) paths: one chokepoint, one voice.

### Flow 4 — rate-limit flood (actuation twin)

Prerequisite: an active grant with a low `max_event_rate`; the agent floods pointer requests.

1. A→C `vitrin_actuator_pointer.move(…)` ×N at a rate above the bucket.
2. C→A `vitrin_grant.refused(actuate_pointer, rate_limited, retry_after_ms)` — **coalesced**: because these are fire-and-forget requests, at most one `refused(rate_limited)` is emitted per grant per bucket-refill window (contrast [`vitrin_view.capture_frame`](06-vitrin_view.md), whose refusals are reply-bearing and never coalesced). This prevents an error storm from colliding with backpressure death.

## Growth

Named version-2+ seams, each purely additive (a new message or enum entry, never a changed signature — see the additive-safety appendix in [00-conventions.md](00-conventions.md)):

- **Intent-level motion.** A `drag(x0, y0, x1, y1, duration_ms, easing)` family arrives as `since="2"` sibling requests on this interface, interpolated server-side. The version-1 `move`/`button`/`scroll` primitives remain valid forever; intent motion is layered above them, not a replacement.
- **Epoch-guarded siblings.** The stale-target problem that version 1 answers by clamping is answered precisely in a later phase by an epoch/compare-and-swap mechanism: epoch-carrying sibling requests here, paired with the `since="2"` epoch-staleness refusal sibling documented on [`vitrin_grant`](04-vitrin_grant.md). This lets an actuation assert "act only if the view is still at epoch E", which the version-1 wire cannot express.
- **Key actuation is a different verb.** Chord and raw-key injection are not added to this pointer facet; they arrive as a distinct `actuate_key` verb (a later entry in the [`vitrin_grant.verb`](04-vitrin_grant.md) bitfield) with its own facet. The pointer facet stays pointer-only.
- **Per-principal pointer delivery.** The shared-position limitation above is corrected on the *shim* side: `since`-gated sibling events on [`vitrin_shim_seat`](11-vitrin_shim_seat.md) that name the principal, each still ending with `origin` so the schema's B2 rule holds. This interface is unchanged by that correction — `move` has always meant "this principal's pointer" — which is exactly why the wire does not need to change shape when it lands (D-017, deferred to M2).
