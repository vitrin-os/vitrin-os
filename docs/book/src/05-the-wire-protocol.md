# The wire protocol

You need this chapter if you are writing a client in a language the project
does not ship an SDK for, or debugging one that exists.

**The IDL is the source of truth.**
[`protocol/vitrin-v0.xml`](https://github.com/vitrin-os/vitrin-os/blob/main/protocol/vitrin-v0.xml)
defines every interface, and where this book and an IDL `<description>`
disagree, the IDL wins.
[`docs/protocol/00-conventions.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/protocol/00-conventions.md)
is the normative conventions page this chapter summarises.

## Shape

Wayland-influenced and deliberately so: object-oriented, per-connection
object ids, requests and events over a Unix socket, file descriptors passed
by `SCM_RIGHTS`. If you have written a Wayland client, this will feel
familiar.

What is different is that authority is a first-class object. `vitrin_grant`
is on the wire, with a lifecycle you can observe.

## Framing

One message, one frame. All multi-byte integers little-endian. Every frame
opens with an 8-byte header:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+---------------------------------------------------------------+
|                        object_id (u32)                        |
+-------------------------------+---------------+---------------+
|           size (u16)          |  opcode (u8)  | fd_count (u8) |
+-------------------------------+---------------+---------------+
|                       argument payload                        |
```

| Field | Notes |
|---|---|
| `object_id` | The target. Per-connection; referencing an unknown id is fatal `invalid_object`. |
| `size` | The **whole frame including this 8-byte header**. Below 8, or a payload shorter than declared, is fatal `oversized`. The u16 ceiling of 65535 binds senders: do not construct a larger frame. |
| `opcode` | The request or event opcode within the target's interface, at the negotiated version. |
| `fd_count` | How many descriptors accompany this frame. |

`fd_count` living in the header is a deliberate framing invariant, not an
accident of the current signatures: a receiver can drop a frame it cannot
interpret and still consume exactly the right number of descriptors, so an
unknown message never desynchronises the fd stream. If the header's
`fd_count` disagrees with the target message's signature, that is fatal.

**At most one fd per message.** Nothing in v0 needs two.

### Argument types

| Type | Encoding |
|---|---|
| `int` | signed 32-bit LE |
| `uint` | unsigned 32-bit LE |
| `object` | `u32` object id — may be `0` only where the IDL declares `allow-null` |
| `new_id` | `u32` id the **sender** allocates for the object being created |
| `string` | length-prefixed, see the conventions page for padding |
| `fd` | not in the payload — carried out-of-band, counted by `fd_count` |

## Two connection classes, one wire format

| Class | Reaches the core by | Interfaces |
|---|---|---|
| **Agent principal** | Connecting to the core's listening socket and authenticating | `vitrin_handshake`, `vitrin_principal`, `vitrin_realm`, `vitrin_grant`, `vitrin_consent`, `vitrin_view`, `vitrin_actuator_*` |
| **Shim** | Inheriting a socketpair from the core across `fork`/`exec` | `vitrin_shim_session`, `vitrin_shim_surface`, `vitrin_shim_seat` |

The classes are mutually unreachable. A message using the other class's
opcodes dies as fatal `invalid_opcode`, with no special-casing anywhere.

Note what a shim's identity *is*: holding the inherited descriptor. There is
no shim credential and no shim handshake — the socketpair is the
authentication, because the core created both ends itself.

## The interfaces

| Interface | Purpose |
|---|---|
| `vitrin_handshake` | Version + identity hello, resolving to a bound principal |
| `vitrin_principal` | The authenticated principal — root of the connection's authority chain |
| `vitrin_realm` | Realm address; an authority-free scope handle (`realm-0` is the well-known realm and is always served; a deployment may configure more) |
| `vitrin_grant` | Capability handle — the wire projection of one grant-table row |
| `vitrin_consent` | Consent-prompt visibility for one petition (events only, no authority) |
| `vitrin_view` | Observation facet — poll-model frame capture |
| `vitrin_actuator_pointer` | Pointer actuation facet |
| `vitrin_actuator_text` | Text actuation facet |
| `vitrin_shim_session` | Shim connection bootstrap |
| `vitrin_shim_surface` | Shim-to-core buffer path |
| `vitrin_shim_seat` | Input delivery to the shim (events only, origin-tagged) |

Each has a prose page under
[`docs/protocol/`](https://github.com/vitrin-os/vitrin-os/tree/main/docs/protocol).

## The petition, in one request

A grant petition co-mints, in a single request: the **grant**, a **consent
observer**, and the **facets** (view, pointer, text). The client allocates a
`new_id` for each.

The facets are **born inert**. They are real objects from the moment the
request is sent, and they confer nothing until the grant resolves. That
shape is why there is no window in which a client holds a live handle to an
unapproved capability — the alternative, minting facets after approval,
would need a second round trip and a state machine on both sides.

```
client                                        core
  │  request_grant(new_id grant,                │
  │                new_id consent,              │
  │                new_id view, ptr, text)      │
  ├────────────────────────────────────────────>│
  │                                             │  petition registered,
  │              consent.state(shown)           │  prompt raised
  │<────────────────────────────────────────────┤
  │                                             │
  │              grant.resolved(outcome, verbs, │  human decides
  │                             expiry_ms, ...) │
  │<────────────────────────────────────────────┤
  │                                             │
  │  view.capture_frame()                       │
  ├────────────────────────────────────────────>│
  │              view.frame_ready(+ memfd)      │
  │<────────────────────────────────────────────┤
```

`resolved` carries the **effective** verbs and expiry, which may be narrower
than requested. A client that assumes it got what it asked for is wrong.

## The error razor

The single most important thing to get right.

**Fatal** — you violated the protocol. The core closes the connection. No
retry helps, because the bug is in your client.

`invalid_object` · `invalid_opcode` · `invalid_argument` · `oversized` ·
`fd_violation` · `pre_handshake` · `version_unsupported` · `auth_failed` ·
`internal_error` · `resource_exhausted`

**Recoverable** — your request failed. The connection is healthy and you may
try something else.

`not_granted` · `grant_expired` · `revoked` · `rate_limited` · `preempted` ·
`consent_held` · `no_surface` · `operation_failed` · `at_capacity`

The line is: *did the client send something incoherent, or did a coherent
request get refused?* Setting an out-of-range verb bit is incoherent —
fatal. Asking for a verb the core does not serve is coherent — refused,
`unsupported`. This is exactly why the Python SDK carries every defined verb
bit, served or not, and every defined outcome and refusal code: one it did not
know would turn a recoverable answer into a dead socket.

## Ordering

Requests on one connection are processed in order, and events for one object
arrive in order. `sync` is a barrier: it returns once all prior requests are
processed and their events delivered — petition resolution excepted, since
that waits on a human.

## Versioning

Wayland-style. Interfaces grow by appending messages and appending enum
values; existing opcodes never change meaning. The negotiated version comes
out of the handshake, and each side serves only what that version defines.

Version 0 is frozen for Phase 1 — **not forever**. The wire integer is now
**2**, and it appends, in full:

- the `realm_launch` verb;
- three structural mints on `vitrin_grant` — `get_launcher`,
  `get_layout_focus`, `get_layout_arrange`;
- the three interfaces they mint — `vitrin_launcher` (`launch`/`launched`),
  `vitrin_layout_focus` (`focus`), `vitrin_layout_arrange`
  (`set_fullscreen`) — and the `vitrin_layout_arrange.mode` enum;
- the `capacity` refusal code and the `layout_held` outcome.

It changes nothing else: every version-1 signature is byte-identical at
version 2. Phase 2 brings semantic trees and epoch/CAS action semantics.
Expect to move.

## Validating your understanding

The IDL is machine-checked and so is the code generated from it:

```sh
xmllint --noout --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml
cargo xtask codegen --check     # generated Rust + C header match the IDL
```

`crates/vitrin-golden` holds golden frame vectors, and the Python SDK's
`tests/test_golden_vectors.py` checks its encoder against the same bytes.
Those vectors are the cheapest way to validate a new client's codec.

Next: [Build your own client or shim](06-build-your-own-client.md).
