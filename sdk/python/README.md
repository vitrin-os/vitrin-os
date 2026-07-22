# vitrin-os — Python agent SDK

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

frame = grant.observe()             # one poll-model capture (D6); no cleanup owed
pixels = frame.raw                  # xrgb8888 wire bytes, stride * height of them
frame.to_png("shot.png")            # pure-stdlib PNG; Pillow never involved

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
| `vitrin_os.png` | minimal deterministic PNG encoder (pure stdlib; the XRGB→RGB boundary) |
| `vitrin_os.client` | `connect()`, `Connection`, `Realm`, `Grant`, facets, `Frame` — the blocking object API |

## Concurrency model

Single-threaded and blocking, exactly as the conventions doc's ordering
guarantee (§4) permits: send a request, then read the single ordered event
stream until the terminal event arrives. Actuation requests are
fire-and-forget; the SDK bounds failure discovery to one round trip with the
`sync`/`done` barrier idiom (§6.4) — every high-level actuation helper sends
its requests, syncs, and raises a typed exception on any refusal seen before
`done`.

## Frames: lifecycle and PNG (the P1.8.2 decisions)

`grant.observe()` returns a `Frame` **value object**:

- **Close-after-copy.** `observe()` verifies the `frame_ready` memfd
  contract (exact `fstat` size, all four seals), copies the buffer out, and
  closes the fd before returning. A `Frame` never owns a descriptor:
  nothing to close, no context manager, and "no fd leaks across a capture
  loop" holds unconditionally instead of only for disciplined callers. The
  alternative — an mmap-backed lazy view — would defer nothing under the
  poll model (every consumer reads the whole fresh-per-capture buffer
  exactly once) while adding a mapping lifetime to manage.
- **`.raw` is wire-exact.** The buffer verbatim (`stride * height` bytes of
  little-endian xrgb8888, row `r` at offset `r * stride`): the IDL's
  observation-digest domain, so a digest over `.raw` equals a digest over
  the memfd. XRGB→RGB conversion deliberately happens only at the
  presentation boundary, inside `.to_png()` (`vitrin_os.png`) — never in
  the core (plan risk R7: the wire carries raw xrgb8888 only) and never on
  `.raw`.
- **`.to_png(path)` is pure stdlib and never imports Pillow.** A ~60-line
  deterministic encoder (filter-0 scanlines, one zlib stream at a pinned
  level) instead of an optional-import fork whose output would vary per
  environment. Pillow appears in this repo only as a test-side independent
  decoder: `pip install pillow` before running the suite enables that test;
  it skips itself otherwise, and `.to_png()` behaves identically either
  way.
- **Stride-generic addressing.** Version 1 pins `stride == width * 4` on
  the wire (anything else is a server contract violation, sanctioned by
  disconnect), but `Frame` and the encoder address rows through `stride` —
  the seam a later version's padded or dmabuf frames arrive through.

### The capture golden (cross-pinned with the Rust core)

`tests/golden/test_pattern_64x40.xrgb` is the raw xrgb8888 wire conversion
of the core's synthetic test pattern at 64×40. The Rust test
`sdk_capture_golden_file_pins_the_wire_bytes`
(`crates/vitrin-core/src/capture.rs`) recomputes it from the generator and
pins the committed file; `tests/test_capture.py` serves those bytes as a
real sealed memfd through the mock core and asserts `observe()` surfaces
them through `.raw` and, decoded, through `.to_png()`. Rust CI pins the
file, Python CI consumes it — drift between the implementations is loud by
construction, and the golden stays raw bytes so no image codec ever enters
the core. Regenerate (deliberately only) through the single documented flow,
`cargo xtask bless --filter sdk_capture_golden` (see
`tests/golden/README.md`), which drives that test with
`VITRIN_REGEN_GOLDEN=1`.

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

Optionally `python -m pip install pillow` first: it enables the
independent-decoder PNG test (test-only; the SDK itself never uses Pillow —
without it that one test skips and everything else still runs).
