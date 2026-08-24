# vitrin_egress — the egress facet

**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** principal · **Messages:** 1 request + 2 events · **`@verb`:** `egress`

## Purpose

`vitrin_egress` is the capability object through which the
[`egress`](04-vitrin_grant.md#verb) verb is exercised: it opens one outbound
connection to the single `host:port` the grant's
[`net:` selector](04-vitrin_grant.md#the-net-resource-prefix) names, and hands
the connected socket to the principal as a file descriptor. It is minted by
[`vitrin_grant.get_egress`](04-vitrin_grant.md#get_egress) and is born
**inert** — it confers nothing until its grant resolves `granted` with
`egress` in the *effective* verb set, every connection passes the single
server-side enforcement chokepoint, and a refused connection arrives
recoverably, never as a connection death.

## Nothing serves this interface. Read this section first

Two separate statements, and they are not the same statement:

1. **No deployment serves the `egress` verb.** It is refused `unsupported`
   everywhere (see [Defined but
   unserved](04-vitrin_grant.md#defined-but-unserved)), so no grant resolves
   `granted` carrying it, so every `request_connect` through every facet
   minted from every grant refuses `not_granted`. What is missing is the
   out-of-core mediating proxy a connection would be made through. This
   interface is a *request to ask through*; it is not a *mechanism to answer
   with*, and landing one does not land the other.
2. **The reference core implements none of these messages at all.**
   `vitrind`'s dispatch has no arm for `get_egress` and no object kind for
   this interface, so sending either message today is answered
   `invalid_opcode` and the connection dies — where this page says the mint is
   always legal and the use refuses recoverably. That is a gap between this
   specification and the shipped binary, not a second reading of the
   specification. **P2.7.3 owns it**, together with the proxy.

Everything below describes the wire contract. Where it uses the present
tense, it is stating what a conforming server does, not what `vitrind` does.

The staging is the same one `realm_launch` used, carried one step further: the
verb bit landed with no message at all, the facet landed next, and the
mechanism is still owed. A client asking for authority nobody serves must get
an *answer* rather than a dead socket, which is the whole of what defining a
bit ahead of serving it buys.

## A separate interface from the filesystem powerbox

`interface/@verb` is **one value per interface**. An interface declaring
`verb="designate_file"` cannot also declare `verb="egress"`, and its `egress`
requests would reach the enforcement chokepoint with no verb to check them
against — the chokepoint's `(interface, opcode) → required-verb` table is
generated from that one attribute.

So this is the second decomposition the dialect has forced on a design someone
planned as one interface. The first was the layout facet
([`vitrin_layout_focus`](17-vitrin_layout_focus.md) +
[`vitrin_layout_arrange`](18-vitrin_layout_arrange.md)). Here, a plan row had
anticipated one `vitrin_powerbox` carrying `request_file`, `request_dir`
**and** `request_connect`; the dialect cannot express it. `vitrin_powerbox`
(page 13, P2.6.5) carries the filesystem half; this page carries the socket
half. "The socket analog of `request_file`" survives as the *shape* of the
request, not as a claim about which interface hosts it.

## What this facet is not

It cannot listen, cannot bind, cannot name an endpoint outside the grant, and
cannot resolve a name on the principal's behalf. There is deliberately no
request that does any of those.

Inbound reach in particular is not something this protocol declines to
attenuate — it is something the realm does not have. A realm's network
namespace holds exactly `lo` and nothing routable, so a `listen` request would
be *adding* the authority rather than exposing it.

## What an egress grant does not bound

The delivered fd is kernel authority. Revoking the grant does not reach into
the realm and close it.

What revocation *does* reach is the far end. The socket is a connection made
**by the proxy**, so the proxy holds the other side and can close it — which
is why live connections can be torn down on revocation, where a delivered
**file** descriptor's contents cannot be recalled at all. That asymmetry is
real and is stated here rather than generalized from the powerbox's residue,
which is the opposite case.

Nothing implements either half today, so this describes what the design is
built to and not what any deployment does.

## Lifecycle

The facet comes into existence when a principal calls
[`get_egress`](04-vitrin_grant.md#get_egress) on a grant, which allocates the
client-supplied `new_id` and binds it to this interface. Minting is always
structurally successful and is not an authority oracle: a facet minted from a
grant that lacks `egress`, or from a grant that has not resolved, mints fine
and refuses `not_granted` on first use. Since no deployment serves the verb,
today that is *every* facet minted from *every* grant.

It is grant-derived, so it follows the inert-object rule: when its grant dies
(expiry or revocation) the facet goes **inert**, and requests on it are
refused *recoverably* via
[`vitrin_grant.refused`](04-vitrin_grant.md#refused), never `invalid_object`.
Neither version defines a destructor; the object lives for the connection and
its id is never reused.

## Requests

### request_connect

`request_connect(host: string, port: uint)` — **since version 2**

| arg | type | description |
|---|---|---|
| `host` | `string` (max 253 bytes) | the host half of the grant's `net:` selector, byte-exact, IPv6 literals **without** brackets |
| `port` | `uint` | the port half; outside `1`–`65535` is fatal `invalid_argument` |

**Delivery class:** **reply-bearing**. Exactly one terminal event per request,
in request order, never coalesced. Unlike a fire-and-forget actuation — whose
refusals *may* be coalesced — this request's answer is a resource the client
is waiting for, so its terminals pair one-to-one exactly as
[`capture_frame`](06-vitrin_view.md)'s and
[`launch`](16-vitrin_launcher.md#launch)'s do.

### Three terminals, not two

This is the **second facet** in this protocol to need three, not the first and
not a departure from every other:
[`vitrin_powerbox`](13-vitrin_powerbox.md#request_file) got there first, at
P2.6.5, and its `request_file` and `request_dir` are the first two of the three
requests that carry a three-terminal set. A client must handle all three:

| terminal | means |
|---|---|
| [`connected(fd, host, port)`](#connected) | the chokepoint admitted the use and the connection was made |
| [`vitrin_grant.refused(egress, code, …)`](04-vitrin_grant.md#refused) | the chokepoint withheld the authority, **or** this server failed while carrying the use out |
| [`connect_failed(reason)`](#connect_failed) | the chokepoint **admitted** the use and the far end did not answer |

**Why the third exists.** `vitrin_grant.refused` is the enforcement
chokepoint's voice: every code in
[`refusal`](04-vitrin_grant.md#refusal) names something a server *decided*
about a grant. A host that is down decided nothing about any grant, and there
is no honest code for it:

- `not_granted` is the worst available rounding, because an agent that hears
  it **correctly stops asking** — right for a withheld authority, wrong for a
  host that is briefly unreachable;
- `internal` names a failure of *this* server (renderer, memfd, delivery), and
  the far end is not this server;
- `no_surface` is about the realm's own window.

Rounding a transport failure into any of them would make `refused` stop
meaning *"authority was withheld"*, which is the one thing it has to keep
meaning.

Keeping the two apart is not an ergonomic nicety in a capability system. It is
the distinction between **"you may not"** and **"it did not work"**, and
blurring it is a bug in both directions: an agent that reads a transport
failure as a lost authority abandons work it is permitted to do, and an agent
that reads a refusal as a transient failure retries forever against a wall.

**The cost, named rather than hidden — and shared rather than unique.**
Three-terminal requests are a family of three:
[`request_file`](13-vitrin_powerbox.md#request_file),
[`request_dir`](13-vitrin_powerbox.md#request_dir) and this one — against two on
[`capture_frame`](06-vitrin_view.md) and [`launch`](16-vitrin_launcher.md#launch),
one apiece on [`sync`](01-vitrin_handshake.md) and
[`request_grant`](03-vitrin_realm.md#request_grant). The powerbox pair reached
three first, and for an argument that shares nothing with this one but the
arity: *two answerers*, the chokepoint and the human, where this request's third
arm is the far end's non-answer. This paragraph said *"three terminals here and
**at most two** anywhere else"* until the two facets, landed on parallel
branches, were read side by side; nothing machine-checks it, because "which
events terminate this request" is not stated in the IDL at all ([conventions
§6.1](00-conventions.md#61-reply-bearing-requests) carries the table and the
same warning). The seam this opens is general: a later reply-bearing request
whose failure is not the server's decision adds *its own* terminal on *its own*
facet, never a code in `refusal`.

**What does not move is the rule.** [Conventions
§6.1](00-conventions.md#61-reply-bearing-requests) says every reply-bearing
request receives *exactly one* terminal, in request order, never coalesced, and
`request_connect` obeys it unchanged. Three is the size of the set this
request's one terminal is **drawn from** — never a count of events it receives.

### Authority is decided before anything touches the network

The order is **normative**, not an implementation preference. The chokepoint
answers first; only an admitted use reaches the proxy.

Two consequences:

- `connect_failed` is **unreachable** for a principal whose grant does not
  cover the endpoint, so no probe of this interface can measure the
  reachability of a host the asker was not authorized to reach;
- a principal that *is* authorized does learn whether that one endpoint
  answers. That is inside what the grant approved — the authority to reach an
  endpoint is the authority to discover that it is down — and it is named here
  rather than left to be found.

### Why this request names an endpoint at all

The grant's `net:` selector already names exactly one endpoint, so today these
arguments look redundant. They are not redundant by design, and a signature is
frozen forever, so the room has to be here now or never.

`request_grant`'s [`resource`](03-vitrin_realm.md#request_grant) vocabulary
grows by version **without a new request**, and the ergonomics answer this
design has already committed to for browser-shaped realms is an *enumerated
template* — a named, fully enumerated `host:port` set the human approves as an
enumeration. A grant over such a resource covers more than one endpoint, and
the request has to say which one it wants.

**Naming an endpoint is not authority over it.** The chokepoint compares what
this request names against what the grant `covers`, and the comparison can
only ever *narrow*: an endpoint outside the grant refuses `not_granted`, and
every endpoint inside it was on the consent card. This is
[`vitrin_realm`](03-vitrin_realm.md)'s naming-is-not-authority rule applied at
use time.

It is also the exact contrast with
[`vitrin_launcher.launch`](16-vitrin_launcher.md#launch), which takes **no**
arguments. There the empty signature *is* the security property — a command
chosen after consent would be a program the human never saw. Here the endpoint
crosses the wire and chooses nothing.

### How `host` is spelled

`host` carries the host half of the grant's `net:` selector **exactly as that
selector spells it**, with one difference: an IPv6 literal is carried
**without** its brackets.

The brackets exist only to keep the selector's final colon unambiguous
(`net:[2001:db8::1]:443`). This request carries the port as its own argument,
so there is nothing to disambiguate, and admitting brackets would give one
endpoint two request spellings under one selector.

Comparison is **byte-exact**, on the same terms and for the same reasons the
selector's is: `Example.com` and `example.com` are two strings, a grant naming
one does not cover the other, and erring *narrow* is the only direction this
comparison may be wrong in. See [the `net:`
prefix](04-vitrin_grant.md#the-net-resource-prefix) for the full statement,
including the two spellings-of-one-endpoint cases.

**The 253-byte bound is the DNS name maximum**, stated in its own terms rather
than derived from `resource`'s 256-byte bound. It is the *looser* of the two:
a 251-byte host fits here and fits in no `net:` selector, so it is accepted by
the decoder and refused `not_granted` by the chokepoint — the fail-closed
direction.

### The fatal-vs-recoverable razor, per message

Per [conventions §5](00-conventions.md#5-error-taxonomy), stated for every
message this interface defines rather than left to be inferred.

**`request_connect` — fatal, and all of it grammar:**

- a malformed frame (bad padding, embedded NUL, invalid UTF-8, a size field
  that disagrees with the delivered bytes);
- this opcode on a version-1 connection — `invalid_opcode`;
- a `host` longer than its 253-byte bound — `invalid_argument`;
- a `port` outside `1`–`65535` — `invalid_argument`.

The port rule is *argument validation the decoder cannot do*, exactly like
`request_grant`'s non-zero-`verbs` rule. It is **fatal here** where the same
mistake inside a `net:` selector resolves `unsupported`, and the two differ
for a stated reason: a selector is a **string** whose only wire constraint is
a byte length and whose content is a policy question, while `port` is a
**numeric argument with a stated domain**, the same shape as an enum.

**`request_connect` — recoverable, everything else**, including every
authority answer, every server-side failure, every transport outcome, and the
case where the deployment serves no egress at all (which is every deployment).

**`connected` — never an error.** An event refuses nothing. Its only fatal
side is the *client's*, and it is the one-fd rule below.

**`connect_failed` — recoverable, always, at every `reason`.** The connection
did not happen; the facet, the grant and the connection all stay exactly as
they were, and the client may ask again.

### Which `refusal` codes reach this verb

[`refusal`](04-vitrin_grant.md#refusal)'s own rule is that codes are not all
reachable by every verb, and that a code's absence is a property of the
operation. For `egress` the reachable set is the grant-lifecycle four —
`not_granted`, `expired`, `revoked`, `rate_limited` — plus `internal`, and
nothing else.

- **Never `no_surface`**, on the same terms as
  [`launch`](16-vitrin_launcher.md#launch) and for a reason of its own: a
  connection is not made to a window, and a realm whose app has committed
  nothing may still legitimately have something to say to the network. This is
  a **normative exemption a server must implement**, not an observation about
  one. The obvious implementation refuses every non-launch use when the realm
  has no live view; `egress` joining that arm would be a bug, and it is named
  here because the reference core's existing chokepoint has exactly that
  shape.
- **Never `preempted` or `consent_held`.** Both are attention-shaped. An
  outbound socket neither reaches the human's realm nor is visible to the
  human, so a hand on the input has nothing to say about it.
- **Never `capacity`.** That code concerns creating a realm.
- **`internal`** covers this server failing while carrying the use out —
  including a mediating proxy that is not running, not reachable, or that
  failed while setting the connection up. A proxy is part of *this*
  deployment, so its failure is the server's, not the far end's.

**The enum — fatal on an out-of-range value.** A `reason` outside
[`failure`](#failure)'s defined entries is `invalid_argument`, like every
other enum argument on this wire.

## Events

### connected

`connected(fd: fd, host: string, port: uint)` — **since version 2**

| arg | type | description |
|---|---|---|
| `fd` | `fd` | the connected stream socket, owned by the receiving principal |
| `host` | `string` (max 253 bytes) | echo of the host this socket is connected to |
| `port` | `uint` | echo of the port this socket is connected to |

Exactly one per successful `request_connect`, in request order, never
coalesced. The socket is already open; the principal owns it and closes it.

**One fd per message**, which is the wire's rule and not this interface's
choice ([conventions §2.4](00-conventions.md#2-wire-format)): a message
carries at most one file descriptor, so a request that needed two sockets
would be two requests. A client receiving this event with no fd, or with more
than one, has met a transport violation and treats it as fatal — the same rule
[`frame_ready`](06-vitrin_view.md) is delivered under.

**The echo is not decoration.** Replies pair in request order, which is a
bookkeeping obligation on the client. If that bookkeeping is wrong, a
mis-paired frame shows the wrong pixels and a mis-paired **socket** sends
whatever the agent writes next to the wrong host. The echo turns an assumption
into something the client can check — the same reason
[`sync`](01-vitrin_handshake.md)'s cookie is echoed by `done`. A client that
finds the echo disagreeing with what it asked has met a server bug or a
desync, and must not write to the fd.

**What the fd is not.** It is not a channel this protocol continues to
mediate. Once delivered, bytes travel between the principal and the far end
through the proxy, and nothing in this document inspects them. Revocation
kills the grant row and stops further `request_connect` immediately; whether
it also closes *this* socket is the proxy's to do, and it can, for the reason
[above](#what-an-egress-grant-does-not-bound).

### connect_failed

`connect_failed(reason: uint)` — **since version 2**

| arg | type | description |
|---|---|---|
| `reason` | `uint` → [`failure`](#failure) | what the far end did instead of answering |

Exactly one per failed `request_connect`, in request order, never coalesced,
and it is a **terminal**: the request is over, and no `connected` follows.

**This event is never an authority answer.** Reaching it means the enforcement
chokepoint **admitted** the use — the grant is live, the verb is in the
effective set, and the endpoint is covered — and the far end then did not
answer. Every authority answer, and every failure of *this* server, is
[`vitrin_grant.refused`](04-vitrin_grant.md#refused) instead. A client that
treats this event as a lost authority is wrong in the expensive direction: it
will abandon work it is permitted to do.

**Deliberately not in the enum:** a proxy that is not running, not reachable,
or that failed while setting the connection up. That is a failure of *this*
server, so it voices `vitrin_grant.refused(egress, internal)` like every other
server-side failure. The line this enum draws is the far end; anything on this
side of it is the chokepoint's to answer.

**There is no retry hint.** `vitrin_grant.refused` carries `retry_after_ms`
because a token bucket has a known refill time. A host that is down has none,
and inventing one would be a number with nothing behind it. An agent's backoff
is its own.

## Enums

### failure

Every entry names something that happened at or beyond the far end. Nothing
here is a decision about a grant, and nothing here is a failure of this server
— those are [`vitrin_grant.refusal`](04-vitrin_grant.md#refusal)'s. Keeping
the two enums **disjoint** is what lets a client route on the event type
alone.

| entry | value | meaning |
|---|---|---|
| `refused` | 0 | the far end actively refused the connection (nothing listening on that port) |
| `unreachable` | 1 | no route to the host or the network; the packet had nowhere to go |
| `timed_out` | 2 | the attempt exceeded the proxy's deadline with no answer either way |
| `resolution_failed` | 3 | the selector named a DNS name and resolution — which happens only in the proxy, never inside the realm — did not yield an address |

Values are immutable and entries append. A deployment that meets a failure
mode not listed here reports the closest listed one rather than inventing a
value, because an out-of-range value is fatal `invalid_argument`.

**The set is deliberately coarse.** A finer taxonomy would be a finer
description of the network the realm cannot otherwise see, and the grant's
authority is to reach one endpoint rather than to survey the path to it. Four
outcomes are what an agent's retry decision actually turns on.

## Flows

Every flow below is what a **conforming** server does. None of them runs
against `vitrind` today — see [Nothing serves this
interface](#nothing-serves-this-interface-read-this-section-first).

### 1. Mint, connect, use

```
1. A→C  vitrin_realm.request_grant(…, resource="net:api.example.com:443",
                                   verbs=egress, …)
2. C→A  vitrin_grant.resolved(granted, egress, once, 300000)
3. A→C  vitrin_grant.get_egress(egress=@7)
4. A→C  vitrin_egress.request_connect("api.example.com", 443)      [on @7]
5. C→A  vitrin_egress.connected(fd, "api.example.com", 443)        [on @7]
```

The agent then speaks its own protocol over `fd`. Nothing in this document
sees those bytes.

### 2. Minting without the verb

```
1. A→C  vitrin_grant.get_egress(egress=@7)          — always legal
2. A→C  vitrin_egress.request_connect("api.example.com", 443)
3. C→A  vitrin_grant.refused(egress, not_granted, 0)
```

The mint succeeds whatever the grant holds; the refusal comes at use. This is
**every** exchange on **every** deployment today, because no deployment serves
the verb.

### 3. The far end is down

```
1. A→C  vitrin_egress.request_connect("api.example.com", 443)
2. C→A  vitrin_egress.connect_failed(refused)
```

Note what is *not* here: no `vitrin_grant.refused`. The chokepoint admitted
this use, so the grant is intact and the agent may ask again immediately.

### 4. An endpoint the grant does not cover

```
1. A→C  vitrin_egress.request_connect("evil.example.com", 443)
2. C→A  vitrin_grant.refused(egress, not_granted, 0)
```

`covers` is byte-exact, so this is also the answer for
`request_connect("API.example.com", 443)` under a grant naming
`net:api.example.com:443`, and for `request_connect("api.example.com", 80)`.
Erring narrow is the only direction that comparison may be wrong in.

## Growth

- **A second endpoint per grant** arrives as a
  [`resource`](03-vitrin_realm.md#request_grant) vocabulary change — an
  enumerated template — not as a change to `request_connect`, whose signature
  already carries the endpoint for exactly this reason.
- **Datagram or other socket types** would be a `since`-gated sibling request
  with its own delivery event, never a type argument added to
  `request_connect`. The grant's `net:` selector names a TCP endpoint today;
  widening that is a vocabulary question first.
- **Listening** is not a growth seam. It is authority the realm does not have
  (see [What this facet is not](#what-this-facet-is-not)), and adding a
  request would be adding the authority.
- **More `failure` entries** append, like every enum on this wire. Values are
  immutable; a deployment meeting an unlisted mode reports the closest listed
  one rather than inventing a value.

## See also

- [`vitrin_grant`](04-vitrin_grant.md) — the [`egress`](04-vitrin_grant.md#verb)
  verb, the [`net:` selector
  grammar](04-vitrin_grant.md#the-net-resource-prefix), the
  [`get_egress`](04-vitrin_grant.md#get_egress) mint, and
  [`refused`](04-vitrin_grant.md#refused), which carries every authority
  answer this facet's uses receive.
- [`vitrin_launcher`](16-vitrin_launcher.md) — the other facet whose request
  is reply-bearing and whose verb was staged ahead of being served; its
  argument-free `launch` is the deliberate contrast with `request_connect`.
- [conventions §5](00-conventions.md#5-error-taxonomy) — the fatal-vs-recoverable
  razor this page applies per message.
