# vitrin_launcher — the realm-launch facet

**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** principal · **Messages:** 1 request + 1 event · **`@verb`:** `realm_launch`

## Purpose

`vitrin_launcher` is the capability object through which the
[`realm_launch`](04-vitrin_grant.md#verb) verb is exercised: it starts the app
a **realm template** names, into a fresh realm instance. It is minted by
[`vitrin_grant.get_launcher`](04-vitrin_grant.md#get_launcher) and is born
**inert** — it confers nothing until its grant resolves `granted` with
`realm_launch` in the *effective* verb set, every launch passes the single
server-side enforcement chokepoint, and a refused launch arrives recoverably
as [`vitrin_grant.refused`](04-vitrin_grant.md#refused), never as a connection
death.

Before this interface existed, realms came only from a configuration file read
at startup, so changing which app ran meant restarting the core. Putting that
on the wire raises one question the rest of this page answers: **who may
start a process, and what may they start?**

### Launch is a verb, not a request on the realm handle

[`vitrin_realm`](03-vitrin_realm.md)'s own description calls it *deliberately
authority-free* — the handle answers "which realm are you asking about",
nothing more. A `launch` request there would make **holding a name** confer
the power to start a process, and a message signature is immutable forever, so
no later attenuation could undo it.

Routing launch through a grant instead buys, with no new machinery: a consent
prompt the human sees, an expiry, revocation, a journal entry naming who
asked, and the grant's rate ceiling. Launching becomes an authority that can
be attenuated and taken away, rather than a property of having connected.

### The command never crosses the wire

`launch` takes **no arguments**, and that is the security property rather than
an economy.

The realm this grant was petitioned over names a realm **template**; the
template names the program. Whoever may write a deployment's realm
configuration is who chooses what the trusted core spawns — a property the
core's startup audit of that file depends on. A command string off the wire
would hand that choice to any principal holding one grant, and would leave the
audit with nothing to audit.

The consequence for clients is deliberate: selecting *which* program to run is
done by petitioning over a **different realm**, at consent time, in front of
the human — never by an argument after the fact. An agent that holds one
launch grant can start exactly one template's app, however many times its rate
ceiling allows.

A future need to parameterize a launch arrives as a `since`-gated sibling
request, or a builder preceding this one, never as arguments added here.

### What this facet is not

It cannot stop, restart, or reconfigure a realm, and there is deliberately no
request that does. This interface's whole authority is *"create one more realm
from a template the operator wrote"*. Lifecycle beyond creation is not
expressible here at any verb set.

### What a launch grant does not bound

A launched app is confined by whatever the deployment confines realms with,
which this facet neither states nor strengthens. **Authority to launch is
authority to start a process under the core's own uid and filesystem view**
unless something outside this protocol says otherwise. The grant bounds *who*
may launch and *how often*; it says nothing about what the launched program
may then do. That is a real reduction in the guarantee the core used to
offer — "only startup can fork" — and it is stated rather than left to be
discovered.

## Served status

**No deployment serves `realm_launch` today.** The verb is defined,
petitionable, and refused `unsupported` at admission — the same staging
[`observe_cursor`, `layout_arrange` and
`layout_focus`](04-vitrin_grant.md#defined-but-unserved) already use, and for
the same structural reason: an *out-of-range* bit is fatal `invalid_argument`
and kills the connection, so a client asking for authority a deployment does
not serve must get an answer rather than a dead socket.

This interface's messages are `since="2"`, so they do not exist on a
version-1 connection at all; sending one there is fatal `invalid_opcode`. The
verb bit is *not* version-gated — a bitfield is one mask checked identically
on every negotiated version — so a version-1 client may name `realm_launch`
in a petition and is answered `unsupported`.

## Lifecycle

The facet comes into existence when a principal calls
[`get_launcher`](04-vitrin_grant.md#get_launcher) on a grant, which allocates
the client-supplied `new_id` and binds it to this interface. Minting is always
structurally successful and is not an authority oracle: a launcher minted from
a grant that lacks `realm_launch`, or from a grant that has not resolved yet,
mints fine and refuses `not_granted` on first use.

It is grant-derived, so it follows the inert-object rule: when its grant dies
(expiry or revocation) the facet goes **inert**, and requests on it are
refused *recoverably* via [`refused`](04-vitrin_grant.md#refused), never
`invalid_object`. Neither version defines a destructor; the object lives for
the connection and its id is never reused.

## Requests

### launch

`launch()` — **since version 2**

No arguments. See [the command never crosses the
wire](#the-command-never-crosses-the-wire) for why that is load-bearing.

**Delivery class:** **reply-bearing**. Exactly one terminal event per request,
in request order, never coalesced:

- [`launched(realm)`](#launched) on success, or
- [`vitrin_grant.refused(realm_launch, …)`](04-vitrin_grant.md#refused) on
  failure.

Pipelining launches is legal; replies pair in order, exactly as
[`capture_frame`](06-vitrin_view.md)'s do. The one-of pairing is not optional
bookkeeping: a realm id has no "no realm" value that would not also be a legal
id, so failure must be a distinct event rather than a sentinel string.

Launches are rate-limited by the grant's `max_event_rate` like every other
use of a grant.

### Every launch creates a new realm; nothing is ever relaunched

An exited realm stays exited. This follows from what `unavailable` means: a
petition against a dead realm resolves `unavailable` precisely so an agent
that hears it **stops asking**. Reviving a realm behind that answer would make
the answer a lie, and would park consent prompts in front of humans about
authority over a corpse.

So each `launch` mints a *new* realm id, unique for the life of the session,
and the template it came from is never itself a running realm. A template is
addressable but never paints: an `observe` grant over one refuses
`no_surface` forever — authority over nothing, which is inert rather than
dangerous, but a shape a confused client can reach.

### Failure modes

*Fatal (the connection dies).* Sending this opcode on a version-1 connection
is `invalid_opcode`, as is any opcode this interface does not define. A frame
whose declared size or fd count disagrees with the signature is `oversized` or
`fd_violation`. There is nothing else to get wrong: the request has no
arguments.

*Recoverable (an event is delivered, the connection lives).* Every one of
these arrives as `vitrin_grant.refused(realm_launch, code, retry_after_ms)`:

| code | when |
|---|---|
| `not_granted` | the grant never held `realm_launch`, or has not resolved `granted` |
| `expired` | the grant's expiry passed |
| `revoked` | the grant was revoked |
| `rate_limited` | the grant's token bucket is empty; `retry_after_ms` > 0 |
| `capacity` | the deployment is at its realm limit; `retry_after_ms` is 0 |
| `internal` | the spawn failed for a reason the core did not choose |

Three codes are **never** produced for a launch, and their absence is a
property of the operation rather than a promise they are unused:
`no_surface` (a realm with no surface is what launch exists to fix),
`preempted` and `consent_held` (both actuation-only).

`capacity` is the one refusal in that table answered from **deployment-wide**
state rather than from this grant, so polling `launch` observes one bit about
*other* principals' realms — a side channel no attenuation of a launch grant
removes. Stated in full, with what a deployment has to weigh, on
[`vitrin_grant`](04-vitrin_grant.md#refusal).

## Events

### launched

`launched(realm: string)` — **since version 2**

| arg | type | description |
|---|---|---|
| `realm` | `string` (max 64 bytes) | id of the newly created realm instance |

Exactly one per successful `launch`, in request order, never coalesced.

`realm` is immediately usable as
[`vitrin_principal.get_realm`](02-vitrin_principal.md)'s `name`. It carries
the same 64-byte bound as every other realm id on the wire, which is what lets
it travel back through `get_realm` unchanged. The id is minted by the core and
is unique for the life of the session; its internal shape is the core's
business — **treat it as opaque**, do not parse or predict it.

**Launching confers nothing over what was launched.** Observing or actuating
the new realm is a separate petition, seen by the human separately, and
`realm_launch` does not imply `observe`. The convenient alternative — handing
back an already-observable realm — is exactly the shape that would smuggle
observation in behind a single prompt.

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent.

### 1. Launch, then petition for what was launched

```
1. A→C  vitrin_principal.get_realm(realm=3, name="<template>")
2. A→C  vitrin_realm.request_grant(grant=4, consent=5, view=6, pointer=7,
                                   text=8, resource=null, verbs=realm_launch, …)
3. C→A  vitrin_consent.state(shown)  [the prompt names the principal + template]
4. C→A  vitrin_grant.resolved(granted, verbs=realm_launch, …)
5. A→C  vitrin_grant.get_launcher(launcher=9)      [structural mint, no reply]
6. A→C  vitrin_launcher.launch()
7. C→A  vitrin_launcher.launched(realm="<instance>")
8. A→C  vitrin_principal.get_realm(realm=10, name="<instance>")
9. A→C  vitrin_realm.request_grant(…, verbs=observe, …)   [a SECOND prompt]
```

Steps 8–9 are not ceremony: they are the reason step 7 hands back a *name*
rather than an object. Watching what you started is a distinct authority and
gets a distinct prompt.

### 2. Minting without the verb

```
1. [grant resolved granted with verbs=observe only]
2. A→C  vitrin_grant.get_launcher(launcher=9)   [legal; mints fine]
3. A→C  vitrin_launcher.launch()
4. C→A  vitrin_grant.refused(realm_launch, not_granted, 0)
```

The mint succeeds and the *use* refuses. Refusing at mint time would turn the
mint into an oracle for what a grant holds.

### 3. Today's answer, on every deployment

```
1. A→C  vitrin_realm.request_grant(…, verbs=realm_launch, …)
2. C→A  vitrin_grant.resolved(unsupported, 0, once, 0)
```

No deployment serves the verb yet, and a petition mixing `realm_launch` with a
served verb is refused **whole** — never narrowed to the served remainder.

## Growth

- **Parameterized launch.** A `since`-gated sibling request, or a builder
  preceding `launch`, never arguments added to `launch` — its signature is
  frozen forever like every other.
- **Realm lifecycle.** Stopping or restarting a realm is not expressible here
  and would be a separate verb with its own consent copy, not a request added
  to this facet.
