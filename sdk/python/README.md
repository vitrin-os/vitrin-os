# vitrin-os — Python agent SDK (P1.8.1: pure-Python wire client)

The `vitrin_os` package: a pure-Python (>= 3.11, stdlib-only, no C
extension, no threads, no asyncio) client for the principal-facing half of
the Vitrin OS wire protocol. It is a **deliberately independent second
implementation** of the wire format defined by `protocol/vitrin-v0.xml` and
`docs/protocol/00-conventions.md` — it does not share code with the Rust
codec, so a spec ambiguity shows up as a byte-level disagreement instead of
a silently shared bug.

## Quick start

```python
from vitrin_os import connect

conn = connect("/run/user/1000/vitrin-0/core.sock",
               identity="vitrin://local/agent/demo",
               credential_type="static-token",
               credential="demo-token")

grant = conn.request_grant(realm="realm-0",
                           verbs=("observe", "actuate.pointer", "actuate.text"))
grant.await_consent()               # blocks on the core-rendered consent prompt

with grant.observe() as frame:      # sealed memfd; frame.width/height/stride
    pixels = frame.read_bytes()

grant.pointer.click(100, 200)       # move + press + release + sync barrier
grant.text.type("hello\n")          # newline renders as Return

conn.close()
```

## Package layout

| Module | What it is |
|---|---|
| `vitrin_os.errors` | typed exception hierarchy (skeleton; ergonomics in P1.8.3) |
| `vitrin_os.protocol` | enums and constants transcribed by hand from `protocol/vitrin-v0.xml` |
| `vitrin_os.wire` | framing primitives: 8-byte header, the seven argument types, string padding |
| `vitrin_os.messages` | per-message encoders (requests) and decoders (events) with opcode tables |
| `vitrin_os.transport` | blocking Unix-socket transport: frame buffering, `recvmsg` + `SCM_RIGHTS` fd queue |
| `vitrin_os.client` | `connect()`, `Connection`, `Realm`, `Grant`, facets, `Frame` — the blocking object API |

## Concurrency model

Single-threaded and blocking, exactly as the conventions doc's ordering
guarantee (§4) permits: send a request, then read the single ordered event
stream until the terminal event arrives. Actuation requests are
fire-and-forget; the SDK bounds failure discovery to one round trip with the
`sync`/`done` barrier idiom (§6.4) — every high-level actuation helper sends
its requests, syncs, and raises a typed exception on any refusal seen before
`done`.

## Error hierarchy (skeleton — fleshed out in P1.8.3)

```
VitrinError
├── ConnectionClosed            EOF / use after close
├── ServerContractViolation     server broke framing or the frame contract
│                               (bad frame_ready memfd, missing seals, ...)
├── ObjectIdsExhausted          client id watermark ran out of [2, 0xfeffffff]
├── FatalError                  vitrin_handshake.error received; connection dead
│   ├── InvalidObject ─ InvalidOpcode ─ InvalidArgument ─ Oversized ─
│   ├── FdViolation ─ PreHandshake ─ VersionUnsupported ─ AuthFailed ─
│   └── InternalError ─ ResourceExhausted            (one per fatal code, §5.2)
├── GrantResolutionError        vitrin_grant.resolved with outcome != granted
│   ├── GrantDenied ─ ConsentTimeout ─ RealmUnavailable ─
│   └── GrantUnsupported ─ Busy                      (one per outcome, §5.3)
└── GrantRefused                vitrin_grant.refused (use-time, recoverable)
    ├── NotGranted ─ GrantExpired ─ Revoked ─ RateLimited ─
    └── Preempted ─ ConsentHeld ─ NoSurface ─ OperationFailed  (one per code, §5.3)
```

The mapping is exhaustive by construction (module-level assertions in
`errors.py`, plus a test): every fatal code, every petition outcome, and
every refusal code maps to exactly one distinct exception class.

## Test-vector sharing (decision)

The repo's golden-bytes corpus lives in
`crates/vitrin-protocol/tests/golden.rs`, mirrored byte-for-byte by
`shim/tests/test_golden_frames.c`. The Python suite
(`tests/test_golden_vectors.py`) is the **third copy of the same
written-down bytes**: each implementation must match the frames as written
down independently — never each other's encoder output — so a symmetric
codec bug cannot hide. This follows the pattern the C side already
established; a generated shared JSON corpus (via `vitrin-scanner`) remains
an option once the vector set grows past hand-copy size, and would be a
`track:protocol`/`ci-docs` change. When editing any golden frame, update
all three files (each cross-references the others).

The mocked-server flow tests (`tests/mock_server.py`) additionally build
their scripted frames with a local, struct-level encoder in
`tests/vectors.py` that is independent of the SDK's own codec.

## Running tests

```sh
python -m pip install -e 'sdk/python[dev]'
python -m pytest sdk/python/tests
```
