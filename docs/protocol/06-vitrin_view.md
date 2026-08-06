# vitrin_view — observation facet (poll-model frame capture)

**Interface version:** 1 · **Connection class:** principal · **Grant verb:** `observe` · **Messages:** 1 request + 1 event

## Purpose

`vitrin_view` is the observation capability onto a realm's composited framebuffer. In version 1 a realm has a single view, and that view is the realm's whole composited output; one `vitrin_view` object *is* that whole view. It carries no geometry protocol of its own — the agent learns the view's dimensions from the `frame_ready` event, never from a separate query.

Capture is a **poll model**: one `capture_frame` request yields exactly one frame. There is no streaming, no subscription, and no server-initiated frame push. This keeps observation on the same request/reply spine as the rest of the principal connection and makes a threadless blocking SDK correct without extra machinery (see [conventions § delivery classification](00-conventions.md)). Streaming capture is a deliberate deferral, not an omission (decision D6): if a later version adds it, it arrives as `since`-gated sibling messages beside this poll pair, which stays valid forever.

Observation is **concurrent by design**: capture never contends with physical human input or with a pending consent prompt, so the refusal codes `preempted` and `consent_held` are actuation-only and never refuse a capture. The consent overlay is composited into human-visible output only, so a frame captured while a prompt is up simply does not contain it (see [`vitrin_consent`](05-vitrin_consent.md)) — captures keep flowing while a prompt is pending.

### What a capture does not contain

**A captured frame contains no cursor except the human principal's, and that one only for a grant whose effective verb set holds [`observe_cursor`](04-vitrin_grant.md#verb).** Otherwise a capture carries realm content alone; every core-composited cursor — like the consent overlay and the trust indicator — is drawn into human-visible output only. The two halves of the rule are deliberately unequal, and the asymmetry is the decision: agent→agent is closed outright and purchasable at no verb set, ever, while agent→human is closed by default and opens only through a verb the human sees on a consent prompt and can revoke with the grant. A per-grant verb is the only "toggle" that exists; there is deliberately no per-pair one.

| viewer → subject | version 1 | why |
|---|---|---|
| agent A → agent B's cursor | **never**, not purchasable by any verb | reveals what another principal is doing — a cross-principal side channel |
| agent → the human's cursor | off by default; purchasable as the distinct verb `observe_cursor`, which version 1 refuses `unsupported` | reading the human's cursor is surveillance of *attention*: an agent can time its actions to it. Closer to `observe` than to a display preference, hence a verb (D-017). It is meaningful only alongside `observe`; naming it alone resolves `unsupported` |
| human → agent cursors | on, per-agent toggle | observability is the point, but thirty simultaneous cursors are unreadable. This is a shell/core concern; the human has **no wire presence** in version 0, so it is not agent-expressible |

Version 1 refuses `observe_cursor` `unsupported`, so no capture contains any cursor today; version 1 **does** composite an agent principal's own cursor, at the same output stage as the consent overlay and the trust indicator, so the rule binds the compositor **now** rather than vacuously and is exercised by every frame drawn while an agent is pointing. No human cursor is composited at all: in nested operation the host desktop draws it outside the realm view entirely. The exclusion is structural, not a checked flag — the sprite joins downstream of the `Scene::compose` a capture is taken from, exactly as the consent card does — and `backend/headless.rs`'s `the_agent_cursor_reaches_human_visible_output_but_never_a_capture` asserts both halves on real composited pixels, including that no sprite byte survives into the frame a `capture_frame` would seal into a memfd. See [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md) for the cursor model itself (one virtual pointer per principal, core-composited, cursorless by construction).

In the object graph, `vitrin_view` is one of the three authority *facets* co-minted by [`vitrin_realm.request_grant`](03-vitrin_realm.md) alongside the [`vitrin_grant`](04-vitrin_grant.md) handle and its consent observer. The facet is the observation-shaped surface of a single grant-table row; the grant is its authority. `vitrin_view` holds no authority of its own — every capture is checked at the grant's single enforcement chokepoint (grant alive, `observe` in the effective verb set, rate bucket not empty, realm has a surface), and every refusal is voiced not here but on [`vitrin_grant.refused`](04-vitrin_grant.md). The design idea is attenuation by construction: because the facet is minted only by the petition that also minted its grant, an agent can never name — and so never capture through — a view it was not granted.

## Lifecycle

A `vitrin_view` instance comes into existence only as the `view` `new_id` argument of [`vitrin_realm.request_grant`](03-vitrin_realm.md). It is never created by a request on this interface and has no constructor of its own. It is co-minted with its grant, its consent observer, and the pointer and text facets, all under the multi-`new_id` rule described in [conventions § object ids](00-conventions.md) (distinct, strictly increasing in argument order, above the connection's allocation watermark).

The facet is **born inert**. Existence confers nothing: until the grant resolves `granted` with `observe` in its effective verb set, every `capture_frame` on this object is refused recoverably with [`vitrin_grant.refused`](04-vitrin_grant.md)`(observe, not_granted, …)`. This is deliberate — mint-freely, check-at-use — and it means a well-behaved agent may hold the facet through the whole pending phase and only discover the outcome on [`vitrin_grant.resolved`](04-vitrin_grant.md).

Version 1 defines no destructors. A `vitrin_view` object lives for the connection. When its grant dies — expiry or revocation — the facet goes inert again rather than being destroyed: captures then refuse recoverably (`expired`, `revoked`), never fatally. The realm's surface going away does **not** kill the grant: `no_surface` is a use-time refusal on a *live* grant (the shim crashed or exited), and captures succeed again if the surface returns. Because object ids are never reused, the server MAY still emit or reference this object after its grant has died, and clients MUST tolerate and discard anything stale (see [conventions § object ids](00-conventions.md)). All of a connection's facets die with the connection; version 1 has no grant persistence.

## Requests

### capture_frame

```
capture_frame()
```

This request takes no arguments — deliberately. A capture is always the whole realm view, and the pixel format is server-chosen, announced per-frame by `frame_ready`. Region selection, format negotiation, or a streaming subscription would arrive as sibling messages in a later version, never as changes to this one (a message signature is immutable forever — [conventions § versioning](00-conventions.md)).

`capture_frame` requests exactly one frame of the realm view. It is **reply-bearing**: every `capture_frame` receives exactly one terminal event — `frame_ready` on success, or [`vitrin_grant.refused`](04-vitrin_grant.md)`(observe, …)` on failure — delivered in request order and **never coalesced**. This one-to-one, order-preserving pairing is what lets a client pipeline captures: send several `capture_frame` requests, then read terminals off the stream knowing the *n*-th terminal answers the *n*-th request.

The pairing is forced by the type system rather than by convention. An `fd` argument has no null form, so a failed capture cannot be signalled as a `frame_ready` with an absent fd; failure must therefore be a distinct event, which is exactly [`vitrin_grant.refused`](04-vitrin_grant.md). A receiver that gets `frame_ready` knows it holds a real frame.

Each capture passes the grant's single enforcement chokepoint. Captures are rate-limited by the grant's effective event-rate ceiling (requested as `max_event_rate` at petition time; the effective value is deliberately not echoed on `resolved` — an agent discovers throttling through `refused(rate_limited)` and its `retry_after_ms` hint); the token bucket governs observation just as it governs actuation.

**Freshness.** The frame carries the realm view's most recently composited content as of when the server processes the request; version 1 makes no freshness promise beyond that, and an agent observes change by capturing again. "Never a stale frame" is the `no_surface` rule: a realm whose surface is gone (its shim crashed or exited) refuses rather than re-serving its last content, so a delivered frame always describes a live realm view.

**Delivery class:** reply-bearing (exactly one terminal event per request, in request order, never coalesced — unlike fire-and-forget actuation refusals, capture refusals are never merged).

**Failure modes.** `capture_frame` has no fatal failure of its own — a well-formed request on a live object never kills the connection. Every failure is a recoverable [`vitrin_grant.refused`](04-vitrin_grant.md)`(observe, code, retry_after_ms)`, where `code` is drawn from the grant's [`refusal`](04-vitrin_grant.md) enum:

| code | when |
| --- | --- |
| `not_granted` | grant not (or not yet) active, or `observe` outside the effective verb set: capture while pending, after denial, or on a facet whose verb was not granted |
| `expired` | the grant's expiry passed (checked on use and by a proactive timer) |
| `revoked` | the grant was revoked by hold-Esc, panel, or policy; effective on the very next capture |
| `rate_limited` | the capture token bucket is empty; `retry_after_ms` hints the refill |
| `no_surface` | the realm has no surface (its shim crashed or exited) — a refusal, never a stale frame |
| `internal` | server-side failure during this capture (renderer, memfd, delivery) |

Three refusal codes are deliberately absent from this table. `preempted` and `consent_held` are **actuation-only** and never refuse a capture: observation is concurrent with physical input by design (concurrent observers are a documented non-error — [conventions § delivery classification](00-conventions.md)), and the consent overlay is never part of the realm view, so there is nothing a pending prompt would need to hide from capture. `capacity` (added at version 2) is **launch-only** — it answers "this deployment is at its realm limit", which no capture can ever provoke; see [`vitrin_launcher`](16-vitrin_launcher.md).

(Fatal errors — bad opcode, an unsolicited fd, a foreign object id — belong to the framing and object-graph layers documented in [conventions § error taxonomy](00-conventions.md), not to `capture_frame`'s semantics.)

## Events

### frame_ready

```
frame_ready(fd: fd, format: uint, width: uint, height: uint, stride: uint, flags: uint)
```

| arg | type | description |
| --- | --- | --- |
| `fd` | `fd` | fresh memfd holding the frame; ownership transfers to the receiver |
| `format` | `uint` (enum [`format`](#format)) | pixel format as a DRM fourcc value; `xrgb8888` in version 1 |
| `width` | `uint` | frame width in pixels |
| `height` | `uint` | frame height in pixels |
| `stride` | `uint` | row stride in bytes; equals `width * 4` exactly in version 1 |
| `flags` | `uint` (bitfield [`frame_flags`](#frame_flags)) | frame flags; always `0` in version 1 |

`frame_ready` is delivered exactly once per successful `capture_frame`, and it is that capture's terminal event.

The `fd` is a **fresh memfd containing the frame — always a copy**. Agents never see live buffers. Ownership of the fd transfers to the receiver, which **MUST** close it after use; the server closes its own copy after sending. (At most one fd travels per frame; this is the framing invariant that lets a receiver drop any frame and still close its fd — see [conventions § framing](00-conventions.md).)

Version 1 pixels are `xrgb8888`, row-major, origin top-left, little-endian, with `stride` equal to `width * 4` exactly. Pinning the stride makes the buffer layout unambiguous: it fixes the golden-frame tests and gives observation digests a single well-defined domain (the whole buffer). The padding byte of every pixel — byte 3 of each little-endian `xrgb8888` pixel, the X channel — is `0xFF` exactly, so identical content yields identical buffer bytes and digests stay deterministic (an unpinned don't-care channel would make the whole-buffer digest domain nondeterministic).

The agent learns the view's dimensions from `width` and `height` on this event; both are always nonzero — a realm view that does not exist refuses `no_surface` rather than delivering an empty frame. Version 1 has no separate geometry protocol.

#### The memfd contract

Binding whenever the `dmabuf` flag is unset — in version 1, always:

- **Size.** The memfd's size (`st_size`) is **exactly `stride * height` bytes**. The whole file is the frame; the whole file is the digest domain.
- **Seals.** The server **MUST** seal the memfd with `F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_SEAL` before sending. The write seal is the mechanical form of the always-a-copy rule: a buffer still being rendered into cannot be write-sealed, so a sealed fd is *provably* not a live buffer, the mapping cannot be truncated or mutated under the reader, and the frame's bytes — and digest — are immutable for the fd's lifetime. The receiver **SHOULD** verify the size with `fstat` and **SHOULD** verify the seals (`F_GET_SEALS`) before mapping — the immutability above is client-provable rather than taken on trust only when the seals are actually checked.
- **Addressing.** Pixels are addressed only through this event's arguments (`width`, `height`, `stride`, `format`), never inferred from the fd. Row order follows the [`y_invert`](#frame_flags) flag: row *r* of the image begins at byte offset `r * stride` while it is unset — in version 1, always. `stride` equals `width` times the announced format's bytes per pixel — `width * 4` for `xrgb8888`, the only format captured in version 1.
- **Arithmetic.** All contract arithmetic is exact: the equalities hold in unbounded integers, and a receiver **MUST** evaluate them without 32-bit wraparound (widening to at least 64 bits). A frame that satisfies them only modulo 2³² violates the contract.

**Contract violations.** A frame that violates this contract — a memfd whose size is not `stride * height`, a missing seal, a zero dimension, a stride inconsistent with the announced format's bytes per pixel (other than `width * 4` in version 1), or nonzero `flags` in version 1 — is a **server protocol violation, not a refusal**: the client MUST discard the frame and close the fd, MAY close the connection, and MUST NOT attribute the failure to the grant (an SDK surfaces it as a client-side protocol error, never as a typed refusal exception). There is no wire message for it — version 1 has no client-to-server error channel by design, so a client's only sanction is disconnecting. A correct server never produces such a frame.

#### Flags

`flags` is always `0` in version 1; the reserved [`frame_flags`](#frame_flags) bits let a later zero-copy dmabuf handoff reuse this same message with a flag set, never a new signature. Flag bits are **version-gated** and there is **no ignore-unknown-bits rule** on this event: the server sets a bit only when the negotiated version defines its semantics — silently ignoring a bit like `y_invert` would mean silently misreading the frame — so under version 1 any nonzero `flags` is a contract violation, handled as above.

**Delivery class:** the success terminal of a reply-bearing request (paired one-to-one with `capture_frame`, in request order, never coalesced).

## Enums

### format

Pixel formats, carried as DRM fourcc codes so the enum never diverges from the kernel's format namespace when dmabuf arrives. This enum is **defined on `vitrin_view`** and is **shared cross-interface**: [`vitrin_shim_surface.attach`](10-vitrin_shim_surface.md) references it as `vitrin_view.format` for the shim's buffer path. Version-1 capture always announces `xrgb8888`; `argb8888` exists for the shim attach path, whose apps commit ARGB buffers. New formats append as new entries with immutable values, so widening either path is never a signature change.

| entry | value | meaning |
| --- | --- | --- |
| `xrgb8888` | `0x34325258` | 32-bit xRGB, `DRM_FORMAT_XRGB8888` (the only format captured in version 1) |
| `argb8888` | `0x34325241` | 32-bit ARGB, `DRM_FORMAT_ARGB8888` |

### frame_flags

Delivery variations of `frame_ready`. Bitfield (entries are powers of two), defined in version 1 but never set by it: the reserved bits are the seam that keeps later capture modes on this same message.

| entry | value | meaning |
| --- | --- | --- |
| `y_invert` | `1` | rows are bottom-up (reserved; never set in version 1) |
| `dmabuf` | `2` | the `fd` is a dmabuf rather than a memfd (reserved; never set in version 1) |

Exact semantics, pinned now so the later handoff needs no re-litigation:

- **`y_invert`** — when set, rows are bottom-up: row *r* of the image begins at byte offset `(height - 1 - r) * stride`. GPU readback is naturally bottom-up; the bit lets a later version hand a frame over without a flip pass instead of mandating one forever.
- **`dmabuf`** — when set, the `fd` is a **single-plane dmabuf with the linear modifier implied** — mirroring [`vitrin_shim_surface.attach`](10-vitrin_shim_surface.md), there is deliberately no modifier argument to fail to honor. The memfd seal contract does not apply (dmabufs cannot be sealed); in its place, the dmabuf's size — as reported by `lseek(fd, 0, SEEK_END)` — **MUST** be at least `stride * height` bytes (allocators round up, so exactness cannot hold), the receiver **SHOULD** verify that bound before mapping or importing, and a shorter dmabuf is a contract violation handled exactly like a memfd one. The frame is still a capture **copy**, never the live composite — always-a-copy is the capture model, not the transport — but on this path immutability is a **server obligation** rather than a client-provable property: the server MUST complete all writes to the buffer before sending and MUST NOT write to it afterwards; a client that needs provably immutable bytes (deterministic digests) uses the sealed-memfd path.

The bits compose (a later version may deliver a y-inverted dmabuf). Future bits append as new entries with immutable values.

## Threadless client shape

The message design is sized so a pure-Python, blocking, single-threaded SDK expresses the whole flow — this is a design constraint, not an afterthought. Three properties carry it: `capture_frame` is reply-bearing with **exactly one** terminal event; terminals arrive **in request order** on the connection's **single ordered event stream** (so the *n*-th terminal answers the *n*-th outstanding capture); and the fd rides the terminal itself, matched positionally (no side channel to poll). A blocking capture is therefore one send followed by one read loop:

```
def capture(view):                       # blocking, no threads
    send(view.capture_frame())
    while True:
        ev = read_event()                # single ordered stream (conventions section 4)
        if ev is frame_ready on view:    # the paired terminal
            size = ev.stride * ev.height # exact (64-bit) arithmetic; == fstat(ev.fd).st_size, verified
            check_seals(ev.fd)           # F_GET_SEALS has all four -> provably immutable
            buf = mmap(ev.fd, size, PROT_READ)   # sealed: cannot change underneath
            frame = Frame(buf, ev.width, ev.height, ev.stride, ev.format)
            close(ev.fd)                 # ownership is the receiver's
            return frame
        if ev is refused(observe) on the grant:  # the failure terminal
            raise typed_exception(ev.code)       # conventions section 5.3 mapping
        dispatch(ev)                     # resolved, consent.state, done, ... and continue
```

Non-terminal events read mid-loop (a `resolved` for another petition, a `done` for an earlier sync) are dispatched or queued, never lost — the exactly-one-terminal rule guarantees the loop exits on this capture's own terminal and nothing else. Pipelined captures generalize the same loop: send *k* requests, then read *k* terminals off the stream in order.

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent. These are the version-1-shaped scenarios from the message-flow catalog that touch `vitrin_view`; steps not involving this interface are abbreviated. Note that observation facets are co-minted by `request_grant` (there is no separate bind step), and capture failures arrive on `vitrin_grant.refused` (not on any per-view error).

### Flow A — walking skeleton: handshake → grant → single capture

1. A→C `vitrin_handshake.hello(version=1, principal, identity, credential_type, credential)`
2. C→A `vitrin_principal.bound(identity)`
3. A→C `vitrin_principal.get_realm(realm, name="realm-0")`
4. A→C `vitrin_realm.request_grant(grant, consent, view, pointer, text, resource=null, verbs=observe, …)` — co-mints the `view` facet, born inert
5. C→A `vitrin_consent.state(closed)` — under auto-approve consent (loudly logged)
6. C→A `vitrin_grant.resolved(granted, verbs=observe, persistence=while_running, expiry_ms)` — the facet is now live
7. A→C `vitrin_view.capture_frame()`
8. C→A `vitrin_view.frame_ready(fd=<memfd>, format=xrgb8888, width=1280, height=800, stride=5120, flags=0)`

The SDK mmaps the memfd and closes it after use.

### Flow B — demo: capture, actuate, sync, re-capture

1–6. As Flow A steps 1–6, but `verbs=observe | actuate_pointer | actuate_text` and the consent prompt is shown to a human who approves (`vitrin_consent.state` → `shown` → `closed`, then `vitrin_grant.resolved(granted, …)`).
7. A→C `vitrin_view.capture_frame()` → C→A `vitrin_view.frame_ready(…)` — SDK locates the target by pixels
8. A→C pointer/text actuations on the sibling facets (see [`vitrin_actuator_pointer`](07-vitrin_actuator_pointer.md), [`vitrin_actuator_text`](08-vitrin_actuator_text.md))
9. A→C `vitrin_handshake.sync(cookie)` → C→A `vitrin_handshake.done(cookie)` — flush any pending refusals before asserting
10. A→C `vitrin_view.capture_frame()` → C→A `vitrin_view.frame_ready(…)` — SDK asserts the frame changed

### Flow C — revocation mid-loop (recoverable refusal)

1. Agent is in an observe/act loop under an active grant.
2. A human holds Esc; the core revokes the grant. The `view` facet goes inert.
3. A→C `vitrin_view.capture_frame()`
4. C→A `vitrin_grant.refused(observe, revoked, 0)` — the SDK raises `Revoked`; the connection lives.

### Flow D — expiry (recoverable refusal)

1. The grant was issued with a bounded `expiry_ms`; the deadline passes.
2. A→C `vitrin_view.capture_frame()`
3. C→A `vitrin_grant.refused(observe, expired, 0)` — the SDK raises `GrantExpired`.

### Flow E — rate-limit hit (never coalesced)

1. A→C `vitrin_view.capture_frame()` ×5 within one second → C→A `vitrin_view.frame_ready(…)` ×5 (the token bucket admits five).
2. A→C `vitrin_view.capture_frame()` (sixth) → C→A `vitrin_grant.refused(observe, rate_limited, retry_after_ms>0)` — one refusal per refused capture. Because `capture_frame` is reply-bearing, these refusals are **never coalesced** (unlike fire-and-forget actuation floods, which may coalesce one `refused(rate_limited)` per bucket-refill window).
3. The bucket refills; the next second's first five captures succeed again.

### Flow F — shim death mid-capture

1. Agent capture loop is running against a live realm surface.
2. The realm's shim is killed; the core reaps it, closes in-flight buffer fds, and drops the realm's surface.
3. A→C `vitrin_view.capture_frame()`
4. C→A `vitrin_grant.refused(observe, no_surface, 0)` — a refusal, never a stale frame; the SDK raises `NoSurface`.

## Growth

Every version-2+ addition below is purely additive: version-1 clients see no signature change. The reserved fields and the shared enum absorb new delivery capabilities on the *existing* messages; anything genuinely new (a streaming subscription) arrives as appended sibling messages, never as a changed one.

- **Zero-copy dmabuf handoff.** The `dmabuf` bit of [`frame_flags`](#frame_flags) and the DRM-fourcc [`format`](#format) enum let a later version deliver the frame as a dmabuf over the *same* `frame_ready` message — the `fd` becomes a dmabuf (single-plane, linear modifier implied, mirroring the shim attach posture), the `dmabuf` flag is set, and the format code already names the kernel format. No new event, no changed signature.
- **Wider formats.** Because [`format`](#format) mirrors the DRM fourcc namespace, new pixel formats append as new enum entries (values immutable) without touching the message.
- **Streaming capture.** Deliberately deferred (decision D6). If a later version adds it, it arrives as `since`-gated sibling messages (a subscription request and its frame-push event appended after the poll pair); `capture_frame`/`frame_ready` stay valid forever, refusals still voice through [`vitrin_grant.refused`](04-vitrin_grant.md), and each pushed frame would still carry one fd (the one-fd rule holds).
- **Geometry and multi-view realms.** Version 1's "one view is the whole realm" is a deliberate floor; multi-surface and multi-view realms add their enumeration and geometry surface to [`vitrin_realm`](03-vitrin_realm.md) and its addressing objects, not by re-plumbing `vitrin_view`.
- **Cursor-bearing captures.** Serving the [`observe_cursor`](04-vitrin_grant.md) verb widens what this *same* `frame_ready` composites for a grant that holds it — the human's cursor appears in the frame. No new message and no changed signature: the verb bit already exists, and version 1 refuses it `unsupported` (D-017). Another *agent* principal's cursor stays out of the frame at every version; that one is not purchasable, which is why the rule above is stated as an asymmetry rather than as a blanket exclusion.

See [conventions § versioning](00-conventions.md) for the append-only growth rules and the additive-safety table naming every reserved seam.
