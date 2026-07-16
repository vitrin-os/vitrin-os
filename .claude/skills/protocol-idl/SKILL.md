---
name: protocol-idl
description: Cheat-sheet for the Vitrin OS wire protocol IDL — wire framing, argument types, object-id rules, the fatal-vs-recoverable error razor, and Wayland-style growth rules. Use when authoring, editing, or reviewing anything in protocol/vitrin-v0.xml, protocol/vitrin-v0.rng, or docs/protocol/*.md.
---

# Vitrin protocol IDL — cheat-sheet

Distilled from `docs/protocol/00-conventions.md`, the normative reference.
Read the full doc before a non-trivial protocol change; use this for quick
recall during routine authoring/review. **Where this cheat-sheet and the full
conventions doc disagree, the conventions doc wins; where the conventions doc
and the IDL disagree, the IDL's `<description>` text wins.**

## Wire framing

8-byte header per message, little-endian: `object_id (u32)`, `size (u16,`
whole frame incl. header, max 65535`)`, `opcode (u8)`, `fd_count (u8, 0 or 1)`.

## The seven argument types (closed set — no arrays, no 64-bit)

`int`, `uint`, `fixed` (24.8 fixed-point), `string` (u32 length + UTF-8 bytes,
no NUL terminator, padded to 4 bytes), `object`, `new_id`, `fd` (out-of-band
via `SCM_RIGHTS`, one per message, matched positionally).

- Every `string` arg documents a max byte length; violation is fatal
  `invalid_argument`.
- `new_id`/`object` args MUST name their `interface`.
- `allow-null` is legal only on `string`/`object`.
- `enum` references are legal only on `int`/`uint`.

## Object ids

Per-connection `u32`. `0` = null (only where `allow-null`). `1` = the
bootstrap object (implicit, never created by a message). Client ids in
`[2, 0xfeffffff]` must be **strictly increasing, never reused** (watermark
rule) — an id at/below the watermark, in the reserved server range, or
unknown is fatal `invalid_object`. Zero destructors in v0: grant-derived
objects go **inert** on grant death, not deleted; requests on inert objects
are refused recoverably, never fatally; clients must tolerate events to dead
objects.

## The error razor

> **FATAL** (connection dies): the client violated something it could have
> known — grammar, handshake order, its own object graph — or breached a
> documented per-connection resource bound (`resource_exhausted`).
> **RECOVERABLE** (event delivered, connection lives): a well-formed
> request's authority/target changed underneath it — consent, expiry,
> revocation, preemption, a granted verb's rate limit.

Ten fatal codes live on `vitrin_handshake.error`: `invalid_object`,
`invalid_opcode`, `invalid_argument`, `oversized`, `fd_violation`,
`pre_handshake`, `version_unsupported`, `auth_failed`, `internal`,
`resource_exhausted`. Shim connections have **no fatal-error message** — a
violation is log-and-close (the shim is a disposable core-spawned child).

Recoverable failures: `vitrin_grant.resolved` (exactly once per grant, ever)
and `vitrin_grant.refused` (from the single enforcement chokepoint) — each
maps to exactly one typed SDK exception.

## Delivery classification

Every request is **reply-bearing** (exactly one terminal event, never
coalesced), **fire-and-forget** (no reply; refusals MAY coalesce), or a
**structural mint** (`get_realm`, `create_surface`, `get_seat` — mints an
object, no terminal event, not refusable). Petition-lifecycle events
(`resolved`, `consent.state`) are exempt from cross-request ordering and the
sync barrier. Ordering
is a single stream per direction, across objects — this is what makes the
`sync`/`done` barrier idiom and a threadless blocking SDK possible with no
extra machinery.

## Growth rules (Wayland-style — append-only, additive-only)

- New messages get `since` attributes; opcodes are implicit document order
  and **append-only** (never reorder/insert).
- A message signature is **immutable forever** — extension is always a new
  message, never a changed one.
- Enum entries are appended, values immutable; `deprecated-since` marks, never
  removes.
- Check `docs/protocol/00-conventions.md` Appendix A before assuming a growth
  need lacks a planned arrival mechanism — most future seams (grant release,
  attenuation, restore tokens, epoch staleness, dmabuf params, etc.) are
  already documented there.

## The XML dialect (`protocol/vitrin-v0.rng`)

Two extension attributes beyond plain Wayland-XML shape:

- `protocol/@version` — single source of truth for the `hello` version
  integer.
- `interface/@verb` ∈ `{observe, actuate_pointer, actuate_text}` on
  `vitrin_view`/`vitrin_actuator_pointer`/`vitrin_actuator_text` — declares
  every request on that interface exercises the named grant verb; this is
  the codegen chokepoint for the single-site authority check.

Structural rule **B2**: `vitrin_shim_seat` defines no requests, and every one
of its events ends with the `origin` argument
(`type="uint" enum="origin"`) — schema-enforced, not just conventional.

Descriptions are required everywhere (protocol, every interface, every
request/event/enum; every enum entry **and every arg** needs a `summary`, and
every string arg's summary needs its `(max N bytes)` token — schema-enforced).

## Validate every change

```bash
xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml
protocol/test-mutations.sh   # negative corpus: every illegal mutation must be rejected
```

## Paired-edit rule

A protocol change is not done until both are updated together:
`protocol/vitrin-v0.xml` (the definition) and the matching
`docs/protocol/NN-vitrin_name.md` (the prose page).
