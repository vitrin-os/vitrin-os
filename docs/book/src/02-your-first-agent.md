# Your first agent

The SDK is pure Python — standard library only, no dependencies, by decision
(D8). If you can run `python3`, you can drive a Vitrin core.

```sh
python -m pip install -e 'sdk/python[dev]'
```

Requires Python 3.11 or newer. The `[dev]` extra is only for the SDK's own
test suite; the client itself pulls in nothing.

## The whole thing

```python
import vitrin_os

conn = vitrin_os.connect(
    "/run/user/1000/vitrin-0/core.sock",
    identity="vitrin://local/agent/demo",
)

grant = conn.request_grant(
    realm="realm-0",
    verbs=("observe", "actuate.pointer", "actuate.text"),
    expiry_ms=300_000,          # five minutes, enforced by the core
)

grant.await_consent()           # blocks until a human decides

frame = grant.observe()
frame.to_png("before.png")

grant.pointer.click(640, 84)
grant.text.type("example.com\n")

grant.observe().to_png("after.png")

conn.close()
```

That is a complete agent. Everything below is what each line means and how
it fails.

## Connecting

```python
conn = vitrin_os.connect(path, *, identity, credential_type="static-token",
                         credential="", timeout=None)
```

The handshake happens inside `connect`. It either returns a **bound**
connection or raises — and the socket is closed on every failure path, so
there is no half-open state to clean up.

```python
from vitrin_os import AuthFailed, VersionUnsupported

try:
    conn = vitrin_os.connect(sock, identity="vitrin://local/agent/demo", timeout=30)
except AuthFailed:
    ...   # the core does not know this identity
except VersionUnsupported:
    ...   # this SDK speaks a protocol version the core will not serve
```

Version 1 identities are static tokens listed in the core's
`principals.toml`. The IDL is shaped for SPIFFE/OIDC credentials; that lands
later.

## Petitioning for a grant

```python
grant = conn.request_grant(
    realm="realm-0",
    resource=None,              # None = the whole realm view
    verbs=("observe", "actuate.pointer", "actuate.text"),
    expiry_ms=300_000,
    max_event_rate=0,           # 0 = the core's default ceiling
    persistence=vitrin_os.Persistence.ONCE,
)
```

One request co-mints the grant, a consent observer, and the facets you asked
for — `grant.observe()`, `grant.pointer`, `grant.text`. **The facets are
born inert.** They exist as objects immediately, and they confer nothing
until the grant resolves. There is no window in which you hold a usable
handle to an unapproved capability.

Verbs can be strings, or the `Verb` flag enum:

```python
from vitrin_os import Verb
verbs = Verb.OBSERVE | Verb.ACTUATE_POINTER
```

<!-- vitrin-verb-set: all-verbs = observe, actuate_pointer, actuate_text, observe_cursor, layout_arrange, layout_focus, realm_launch, egress | count: eight -->

The enum has **eight** members, and the petition above asked for three of
them: `OBSERVE`, `ACTUATE_POINTER` and `ACTUATE_TEXT`. Three more are
**served** by this core on the same terms — `LAYOUT_ARRANGE`, `LAYOUT_FOCUS`
and `REALM_LAUNCH`, exercised by `grant.set_fullscreen()`, `grant.focus()` and
`grant.launch()`.

<!-- vitrin-verb-set: unserved-verbs = observe_cursor, egress -->

The remaining two — `OBSERVE_CURSOR` and `EGRESS` — are defined and resolve
`unsupported`: the first because per-principal cursor delivery does not exist
yet, the second because the out-of-core proxy an outbound connection would be
made through does not exist. `EGRESS` does have a facet on the wire
(`vitrin_egress`), which is worth knowing only so it is not mistaken for
evidence the verb works: a facet is the request you would ask through, and
there is nothing behind it to answer.

Whether a verb is served is a property of the *deployment*, not of the
protocol — a deployment that will not host process creation refuses
`REALM_LAUNCH` even though this one serves it — so read `unsupported` as "not
here, not now" rather than "not in this protocol".

The SDK carries every defined bit either way, for a precise reason: an
out-of-range verb bit is a **fatal** `invalid_argument` that kills the
connection, so an SDK that omitted one would turn a recoverable "not yet"
into a dead socket.

## Waiting for consent

```python
grant.await_consent()
```

Blocks until the petition resolves. In nested mode that means a human looked
at a prompt the core drew and clicked Allow. Your agent's code is identical
either way — which is the point. It cannot tell, and must not care, how the
decision was reached.

Failure is typed:

```python
from vitrin_os import (
    Busy, ConsentTimeout, GrantDenied, GrantUnsupported, LayoutHeld,
    RealmUnavailable,
)

try:
    grant.await_consent()
except GrantDenied:
    ...      # a human said no
except ConsentTimeout:
    ...      # nobody answered
except RealmUnavailable:
    ...      # no such realm, or it is not running
except GrantUnsupported:
    ...      # this deployment does not serve a verb you named
except Busy:
    ...      # another petition is already up
except LayoutHeld:
    ...      # somebody else holds layout.arrange for this output
```

There is one class per non-`granted` outcome, and the list is exhaustive on
purpose: an outcome the SDK could not map would raise
`ServerContractViolation` and **close the connection**, which is the opposite
of what a recoverable answer is for.

After it returns, inspect what you actually got — the core may have
**attenuated** your request:

```python
print(grant.effective_verbs())        # may be narrower than you asked for
print(grant.effective_expiry_ms())    # may be shorter
```

Never assume the grant you hold is the grant you requested.

## Observing

```python
frame = grant.observe()
print(frame.width, frame.height, frame.format, frame.stride)
frame.to_png("shot.png")
raw = frame.raw                       # stride * height bytes, xrgb/argb8888
```

`observe()` is poll-model: you ask, you get the current frame. There is no
subscription and no push.

One race worth knowing: `await_consent()` can return before the app inside
the realm has painted anything. The honest reply then is `NoSurface` — and
it is judged *before* the rate-limit bucket, so retrying costs you no
budget:

```python
from vitrin_os import NoSurface
import time

for _ in range(50):
    try:
        frame = grant.observe()
        break
    except NoSurface:
        time.sleep(0.1)
```

## Actuating

```python
grant.pointer.move(x, y)
grant.pointer.button(vitrin_os.BTN_LEFT, vitrin_os.ButtonState.PRESSED)
grant.pointer.scroll(vitrin_os.Axis.VERTICAL, 120)
grant.pointer.click(x, y)             # move + press + release

grant.text.type("héllo 世界\n")        # a trailing newline presses Enter
```

Text goes in as text, not as scancodes — the core synthesises the keymap
needed to deliver it, so non-ASCII works without your agent knowing anything
about layouts.

For a batch, suppress the per-call flush and bound it with one barrier:

```python
grant.pointer.move(10, 10, flush=False)
grant.pointer.move(20, 20, flush=False)
grant.text.type("hello", flush=False)
conn.sync(grant)          # pass the grant so refusals raise here
```

`conn.sync(grant)` is a real barrier: it returns once every prior request
has been processed and its events delivered. Pass the grant and any refusal
in the batch surfaces as a typed exception at that point. Omit it and
refusals stay queued on the grant until its next barrier — which is a good
way to not notice that nothing you sent landed.

## When actuation is refused

Every refusal is a distinct exception, because they mean genuinely different
things:

```python
from vitrin_os import (NotGranted, GrantExpired, Revoked, RateLimited,
                       Preempted, ConsentHeld, NoSurface, OperationFailed)
```

| Exception | Meaning | Retry? |
|---|---|---|
| `NotGranted` | This verb is not in your grant. | No — fix the petition. |
| `GrantExpired` | `expiry_ms` elapsed. | No — petition again. |
| `Revoked` | Someone revoked it. Often the dead-man switch. | No. A human meant this. |
| `RateLimited` | Over the event-rate ceiling. | Yes, after `retry_after_ms`. |
| `Preempted` | A human touched the input device. | Yes, but back off — a person is using the machine. |
| `ConsentHeld` | A consent prompt is up; actuation is frozen. | Yes, once it resolves. |
| `NoSurface` | Nothing to act on yet. | Yes, cheaply. |
| `OperationFailed` | The core tried and could not. | Maybe. |

There is a ninth, `AtCapacity`, and it is missing from the list above on
purpose: it is only ever produced by `realm.launch`, so an actuation can never
see it. If you hold a launch grant, it means the session is already running as
many realms as it will, and retrying is legal once one exits.

`Revoked` and `Preempted` are the two that matter for behaving well. Both
mean a human intervened, and an agent that hammers through them is exactly
the failure mode this project exists to make structurally impossible — so it
will not work, but writing it that way is still bad manners.

```python
try:
    grant.pointer.click(x, y)
except RateLimited as e:
    time.sleep(e.retry_after_ms / 1000)
except (Revoked, Preempted):
    return          # a human took over. stop.
```

## Fatal versus recoverable

The exception hierarchy encodes the protocol's central razor:

- **`GrantRefused`** and **`GrantResolutionError`** are recoverable. Your
  request failed; the connection is fine.
- **`FatalError`** means you violated the protocol. The core has closed the
  connection. `InvalidObject`, `InvalidOpcode`, `InvalidArgument`,
  `Oversized`, `FdViolation`, `PreHandshake` — all of these mean the bug is
  in your client, and no retry will help.

If you are catching `FatalError` and retrying, you have misread the model.

## Where to go next

`examples/agent-demo/run_demo.py` is a real agent that does all of the
above, including the parts this page glossed — settling the app before
capture, locating a UI feature by pixels, and asserting the actuation
landed. It is also the M1.5 gate, so it is kept honest by CI.

Next: [Grants, consent, and revocation](03-grants-consent-revocation.md).
