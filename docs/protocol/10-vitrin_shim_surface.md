# vitrin_shim_surface — the shim-to-core buffer path

**Interface version:** 1 · **Connection class:** shim · **Messages:** 3 requests + 2 events

> Framing, object-id rules, the error taxonomy, delivery classification, and versioning are defined once in [00-conventions.md](./00-conventions.md) and are not restated here. This page documents only what `vitrin_shim_surface` adds.

## Purpose

`vitrin_shim_surface` is the channel through which a per-app shim forwards its app's rendered buffers up to the core for composition into the realm view. It is a [shim-connection](./00-conventions.md#connection-classes) interface: it is spoken only over a core-inherited socketpair, never on the principal listening socket, and its opcodes are unreachable from a principal connection (an attempt is a fatal protocol error with no special casing).

The interface transplants Wayland's proven pending-state model. `attach` and `damage` mutate a *pending* state that has no visible effect; `commit` atomically latches that pending state into the surface's *current* state. This split is what makes a frame update a single indivisible fact on the wire — the core never composites a half-updated surface — and it lets the shim relay its app's own `wl_surface` semantics upward without translation.

Buffers are named by shim-chosen integer cookies (`buffer_id`) rather than by protocol objects. This is deliberate: a buffer is a transient, high-churn thing, and minting a wire object per buffer would burn ids on every frame. The cookie is opaque to the core; the core only echoes it back in `buffer_done` to hand ownership of that buffer back to the shim. Because the whole path is one-fd-per-message ([a framing invariant](./00-conventions.md#framing)), a single `attach` carries exactly one buffer fd, and the core can always drain and close a dropped frame's fd without consulting the schema.

An instance is created by [`vitrin_shim_session.create_surface`](./09-vitrin_shim_session.md). Nothing in version 1 hard-codes exactly one surface per shim, though the version-1 core composites realm surfaces with a trivial single-maximized layout.

## Lifecycle

A `vitrin_shim_surface` is minted by `vitrin_shim_session.create_surface`, which allocates its `new_id` on the shim connection. There is no destructor in version 1 — the surface lives for the connection ([version 1 defines no destructors](./00-conventions.md#object-ids)).

The surface is destroyed only when the shim connection dies. Socketpair EOF is shim death: the core survives, closes every buffer fd it still holds for the connection (no fd leaks), and drops the realm's surface from the scene. Agents observing that realm get [`vitrin_grant.refused(observe, no_surface)`](./04-vitrin_grant.md) on their next capture — never a stale frame. Version 1 has no shim restart policy.

Because this is a shim connection, there is no fatal-error event: a shim protocol violation is a *log-and-close* condition (the shim is a core-spawned disposable child, and the core log is the debugging channel). See [Failure modes](#failure-modes-on-this-interface) below for the named conditions this interface can trigger.

## Requests

All three requests are **fire-and-forget** ([delivery classification](./00-conventions.md#delivery-classification)): none carries a reply, and none is individually acknowledged. Their effect becomes observable through the buffer-lifecycle events (`buffer_done`, `frame_done`) and, ultimately, through what the core composites. There is no sync barrier on shim connections.

### attach

```
attach(buffer_id: uint, fd: fd, kind: uint, format: uint, width: uint, height: uint, stride: uint)
```

| arg | type | description |
|---|---|---|
| `buffer_id` | uint | shim-chosen cookie identifying this buffer; echoed by the matching `buffer_done` |
| `fd` | fd | the buffer's file descriptor: a memfd (`kind` shm) or a dmabuf (`kind` dmabuf) |
| `kind` | uint (enum [`kind`](#kind)) | what the fd is |
| `format` | uint (enum [`vitrin_view.format`](./06-vitrin_view.md#format)) | pixel format as a DRM fourcc value |
| `width` | uint | buffer width in pixels |
| `height` | uint | buffer height in pixels |
| `stride` | uint | row stride in bytes |

Stages a buffer as the surface's pending buffer for the next `commit`. `buffer_id` is the shim's own cookie for this buffer; it need not be unique across time except that it must not name a buffer whose ownership has not yet returned via `buffer_done` (see `bad_order` below). The cookie is how buffer identity is carried *without* buffer objects.

`format` references the [`format` enum defined on `vitrin_view`](./06-vitrin_view.md#format) — the same DRM-fourcc namespace the agent-facing capture path uses, so the shim's supply side and the agent's observation side never diverge on pixel encoding.

In version-1 practice `kind` is `shm`: the fd is a memfd, and the core copies its pixels in at `commit`. `kind` `dmabuf` is *the same message* — a single-plane buffer with a linear modifier implied. There is deliberately no modifier argument the core could fail to honor, and no plane-count argument; multi-planar formats and explicit modifiers are a [version-2 growth seam](#growth) that arrives as a since-gated parameter builder (one fd per message), so the one-fd-per-message rule never becomes a wall.

**Failure modes.** Geometry inconsistent with the fd's actual size, a zero dimension, or a stride overflow is the log-and-close condition `invalid_buffer`. Re-attaching a `dmabuf` `buffer_id` that has not yet received its `buffer_done` is the log-and-close condition `bad_order`. Neither delivers a wire event — the connection is logged and closed. These are conditions a correct shim can always avoid; they are not recoverable refusals.

### damage

```
damage(x: int, y: int, width: int, height: int)
```

| arg | type | description |
|---|---|---|
| `x` | int | rectangle x in buffer coordinates |
| `y` | int | rectangle y in buffer coordinates |
| `width` | int | rectangle width |
| `height` | int | rectangle height |

Adds one buffer-coordinate damage rectangle to pending state. The request is repeatable: rectangles accumulate as a union until the next `commit`. Damage travels per-rectangle on the wire (never pre-coalesced to the full surface) so that damage-only updates are verifiable in the core log.

Damage is a hint — the core MAY repaint more than the damaged region. Out-of-bounds rectangles are clamped, not fatal.

**Failure modes.** `damage` against a surface that has never been attached is the log-and-close condition `bad_order`. Out-of-bounds rectangles are a non-error (clamped).

### commit

```
commit()
```

Takes no arguments. Atomically latches the pending `attach` and pending `damage` into the surface's current state. This is the only point at which staged state becomes visible to the core's compositor; before `commit` the pending state has no effect.

A `commit` with no new pending `attach` re-presents the current buffer with the new damage — a repaint. This is legal, not an error.

**Failure modes.** `commit` against a surface that has never been attached is the log-and-close condition `bad_order`.

## Events

Neither event is a reply to a request in the reply-bearing sense; both are server-originated facts about buffers and pacing. `buffer_done`, however, is an exactly-once terminal per `attach` and is the interface's one recoverable-signal carrier (its failure statuses drive the dmabuf-to-shm fallback).

### frame_done

```
frame_done(time_ms: uint)
```

| arg | type | description |
|---|---|---|
| `time_ms` | uint | presentation time in milliseconds, monotonic domain |

The frame-callback relay. Sent after a commit's content has been presented (or, headless, after it would have been). At most one `frame_done` is outstanding per commit, FIFO-correlated. The shim fans this out to its app's `wl_surface.frame` callbacks so the app throttles to the true output cadence.

`frame_done` is deliberately **not** merged with `buffer_done`: buffer reusability and frame pacing are distinct facts that only coincide under copy-in (see `buffer_done`).

### buffer_done

```
buffer_done(buffer_id: uint, status: uint)
```

| arg | type | description |
|---|---|---|
| `buffer_id` | uint | the cookie given in the matching `attach` |
| `status` | uint (enum [`buffer_status`](#buffer_status)) | disposition of that attach |

The terminal disposition of one `attach`. **Exactly one `buffer_done` is delivered per `attach`, in attach order per surface** — including an attach that is superseded by a later attach before any `commit`, which is released promptly (with status `released`) without ever being used. This closes the replaced-attach fd gap: no `attach` can leave a buffer's ownership dangling.

Status `released` means the core (and the GPU) are done with the buffer: the shim, and its app, MAY reuse it. Under version-1 shm copy-in this fires promptly, after the copy; under dmabuf passthrough it fires at GPU-done. It is the same message — only the timing regime changes.

The failure statuses (`import_failed`, `format_unsupported`, `too_large`) mean the buffer was **not** used: the previously committed content remains on screen, and the shim is directed to fall back to shm. Ownership returns in the same breath — a failure status is still a `buffer_done`, so it still counts as the one terminal for that attach. This is the recoverable dmabuf-fallback path: it is the shim-connection analogue of a recoverable refusal, delivered as an event rather than killing the connection.

## Enums

### kind

Which kind of fd an `attach` carries.

| entry | value | meaning |
|---|---|---|
| `shm` | 0 | memfd; the core copies pixels in at commit |
| `dmabuf` | 1 | dmabuf; the core imports it zero-copy, single-plane, linear modifier implied |

### buffer_status

The disposition of one `attach`, carried by `buffer_done`.

| entry | value | meaning |
|---|---|---|
| `released` | 0 | the buffer was used (or superseded) and may be reused |
| `import_failed` | 1 | dmabuf import failed; fall back to shm; buffer unused and released |
| `format_unsupported` | 2 | format not usable by the renderer; fall back to shm; buffer unused and released |
| `too_large` | 3 | buffer exceeds the renderer's limits; fall back to shm; buffer unused and released |

### format (shared, defined elsewhere)

`attach.format` references the [`format` enum defined on `vitrin_view`](./06-vitrin_view.md#format) — DRM fourcc values (`xrgb8888` = 0x34325258, `argb8888` = 0x34325241). The enum lives on `vitrin_view` because the agent-facing capture path defines the canonical pixel-format vocabulary; the shim's supply side reuses it so the two never diverge.

## Failure modes on this interface

Because `vitrin_shim_surface` is a shim-connection interface, it has no fatal-error event. Every protocol violation is a **log-and-close** condition — the core logs a named reason and closes the connection. Version 1's named conditions reachable through this interface:

| condition | triggered by |
|---|---|
| `invalid_buffer` | `attach` with geometry inconsistent with the fd's actual size, a zero dimension, or a stride overflow |
| `bad_order` | `damage` or `commit` against a never-attached surface; an unknown `buffer_id`; re-attaching a `dmabuf` `buffer_id` that has not yet received `buffer_done` |

These are conditions a correct shim can always avoid, so they are treated like a client's own object-graph violation — not a recoverable refusal. The dmabuf-import path does have a recoverable outcome, but it is delivered as `buffer_done(status ≠ released)`, not as a connection death.

## Flows

The one canonical scenario that exercises this interface is the shim frame loop (the shim spawn → attach/damage/commit → frame-done loop, milestone M1.2). Direction key: `S→C` = shim→core, `C→S` = core→shim.

### Flow g — shim frame loop (shm copy-in)

```
1.  [core forks the shim with an inherited socketpair; realm identity assigned at fork]
2.  C→S  vitrin_shim_session.configure(realm="realm-0", width=1280, height=800)
3.  S→C  vitrin_shim_session.create_surface(surface=new_id)
4.  [shim's app connects to the shim's private Wayland socket and commits a buffer]
5.  S→C  vitrin_shim_surface.attach(buffer_id=1, fd=<memfd>, kind=shm, format=xrgb8888, width, height, stride)
6.  S→C  vitrin_shim_surface.damage(x, y, width, height)      [repeat per damaged rect]
7.  S→C  vitrin_shim_surface.commit()
8.  [core copies the buffer in and composites the realm view]
9.  C→S  vitrin_shim_surface.buffer_done(buffer_id=1, status=released)   [prompt: copy is done]
10. C→S  vitrin_shim_surface.frame_done(time_ms=…)                       [shim fires wl_surface.frame]
11. [loop steps 4–10 per app frame]
```

Steps 9 and 10 are two distinct facts: `buffer_done(released)` says the buffer is reusable; `frame_done` paces the app. Under copy-in they arrive close together, but they are never merged.

### Flow g′ — dmabuf variant with import failure

```
5.  S→C  vitrin_shim_surface.attach(buffer_id=1, fd=<dmabuf>, kind=dmabuf, format, width, height, stride)
6.  S→C  vitrin_shim_surface.commit()
7.  C→S  vitrin_shim_surface.buffer_done(buffer_id=1, status=import_failed)
8.  [shim falls back to shm: re-attach the same content as kind=shm and commit again]
```

The previously committed content stays on screen through the failure; the shim's fallback is a normal shm `attach`/`commit`. On a successful dmabuf import, step 7 is instead `buffer_done(buffer_id=1, status=released)`, fired at GPU-done rather than promptly.

### Related — shim death (scenario (h))

If the shim is killed mid-loop, the socketpair reaches EOF. The core closes every buffer fd it still holds for this surface (no fd leak), drops the surface, and — on the agent side — answers the next `vitrin_view.capture_frame` with `vitrin_grant.refused(observe, no_surface)`. No further `vitrin_shim_surface` message is delivered; the connection is gone.

## Growth

Version 1 freezes the three request signatures and the two event signatures forever; growth is [additive](./00-conventions.md#versioning). The named version-2+ seams for this interface:

- **Explicit dmabuf modifiers and multi-planar formats.** Version 1's `attach` is single-plane with a linear modifier implied — deliberately no modifier or plane-count argument. Richer buffers arrive as a since-gated *parameter builder* request (linux-dmabuf precedent): a small object accumulates one fd per message, so the one-fd-per-message framing invariant never becomes a wall. `attach` itself is untouched.
- **The `kind`, `format`, and `buffer_status` enums are open.** New fd kinds, pixel formats, and failure dispositions are appended with immutable values; nothing shifts. `format` is shared with `vitrin_view`, so a format added there is available here automatically.
- **The `dmabuf` path is already present.** No signature changes when zero-copy passthrough becomes the common case — only the timing regime of `buffer_done(released)` changes (prompt under copy-in, GPU-done under passthrough), which the wire already accommodates.
