# vitrin_powerbox — the designation facet

**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** principal · **Messages:** 2 requests + 2 events · **`@verb`:** `designate_file`

> Framing, object-id allocation, the fatal/recoverable error taxonomy, delivery
> classification, and versioning are defined once on the [conventions
> page](./00-conventions.md); this page cites those rules rather than restating
> them. Where this page and the IDL's `<description>` text disagree, **the IDL
> wins**.

## Purpose

`vitrin_powerbox` is the capability object through which the
[`designate_file`](./04-vitrin_grant.md#verb) verb is exercised: it asks the
human to choose one file or one directory subtree, and has the resulting **file
descriptor** delivered to the granted realm. It is minted by
[`vitrin_grant.get_powerbox`](./04-vitrin_grant.md#get_powerbox) and is born
**inert** — it confers nothing until its grant resolves `granted` with
`designate_file` in the *effective* verb set, every ask passes the single
server-side enforcement chokepoint, and a chokepoint refusal arrives
recoverably as [`vitrin_grant.refused`](./04-vitrin_grant.md#refused), never as
a connection death.

A *powerbox* is the classic capability-system answer to "how does a confined
program get at a file it was never given?" The answer is that it does not open
one — a trusted mediator shows the human a picker and hands back an already-open
descriptor. That inverts the ordinary desktop arrangement, in which an app has
ambient reach over `$HOME` and a file dialog is a *convenience* drawn by the app
itself. Here the dialog is the authority, it is drawn by the core, and there is
no ambient reach for it to be a convenience over.

## Served status

**No deployment serves `designate_file`, including the reference core.** Every
petition naming the verb resolves `unsupported`, so no grant carries the bit,
so a server that implements this facet refuses every use `not_granted`.

**The reference core implements the facet's messages, which is a different and
much weaker claim than serving the verb.** Since issue #322 `vitrind`
dispatches [`get_powerbox`](./04-vitrin_grant.md#get_powerbox),
[`request_file`](#request_file) and [`request_dir`](#request_dir): the mint is
always legal and puts nothing on the wire, each ask is decoded (an
out-of-range `mode` is fatal `invalid_argument` from the decoder, as every enum
argument is), and each is then refused
[`refused(designate_file, not_granted)`](./04-vitrin_grant.md#refused) —
recoverably, connection intact, which is the answer this interface's inertness
is written to produce. No picker is raised, and **this interface's own
[`refused`](#refused) event is unreachable in that build**, because that event
belongs to an ask the chokepoint *allowed* and no ask is allowed anywhere.

> **Until issue #322 there was no arm for any of the three**, so a conformant
> version-2 client that minted this facet was answered fatal `invalid_opcode`
> and disconnected for sending a documented request of the version that core
> negotiates. Three paragraphs — this one, the IDL's and
> [page 04's](./04-vitrin_grant.md#get_powerbox) — described the gap and a core
> test pinned the fatal answer; none of them made the arm land. What replaced
> them is structural: a core test derives `vitrin_grant`'s mint opcodes from
> generated code and fails on any one of them no arm dispatches.

**The verb's refusal** is the
[defined-but-unserved](./04-vitrin_grant.md#defined-but-unserved)
staging, and unlike `observe_cursor`'s it has a **scheduled** end. Two things
are owed, both named in `docs/plan/02-phase-2-semantic-epochs.md` §2 E2.6:

| owed | task | why the verb cannot be served without it |
|---|---|---|
| the core-drawn picker, with `openat2 RESOLVE_NO_SYMLINKS` resolution from a directory fd and `SCM_RIGHTS` delivery | P2.6.6 | nothing exists that could mint a descriptor, so a granted verb would have no request the server can carry out |
| the human-readable consent copy for the verb | P2.6.8 | Q13's rule: no verb is served before a human can be told, in plain language, what approving it costs |

In the reference core the refusal is **structural rather than a promise**:
`designate_file` is absent from `SERVED_VERB_BITS`, the unserved set is
*derived* from the wire's `VALID_MASK`, and admission refuses a petition naming
an unserved bit **whole** — never narrowed to the served remainder. Forgetting
the rest of E2.6 therefore produces a refusal, not a grant nothing enforces.

This interface's messages are `since="2"`, so they do not exist on a version-1
connection at all; sending one there is fatal `invalid_opcode`. The verb *bit*
is not version-gated — a bitfield is one mask checked identically on every
negotiated version — so a version-1 client may name `designate_file` in a
petition and is answered `unsupported` rather than killed.

## The path never crosses the wire

`request_file` and `request_dir` name **no file**. They raise the core-drawn
picker; the human chooses in front of the trusted indicator; what comes back is
a descriptor. Nothing in either direction carries a path.

An agent holding this verb therefore holds authority *to ask*, never authority
over any file it can name. That is what makes a designation grant safe to hand
out long before any particular file is safe to hand over — and it is the reason
`file:` and `dir:` are useful as
[`request_grant`](./03-vitrin_realm.md#request_grant) resource prefixes at all:
the selector says what **kind** of thing this grant may cover, not which one.

The consequence a client must accept is stated rather than left to be
discovered: **an agent cannot script a designation.** There is no request by
which it says which file it wants, no filter, no suggestion, no starting
directory. Each of those would be an agent-supplied string steering what the
human sees in a window the human is meant to trust, and a message signature is
immutable forever, so a hint added here could never be taken back. A deployment
that wants a sensible starting directory chooses one itself.

[`designated.name`](#designated) is a **display label** — the basename of what
the human picked — and withholding the full path is **not** claimed as
confidentiality: whoever holds the descriptor can read its path out of
`/proc/self/fd`. It is withheld so that no path is ever part of this
interface's contract.

## Two parties receive the same designation

One designation, two connections:

| receiver | message | connection |
|---|---|---|
| the asking agent | [`designated`](#designated) on this facet | principal |
| the realm's shim, which relays to its app | [`vitrin_shim_session.designation`](./09-vitrin_shim_session.md#designation-since-2) | shim |

Both carry the same `designation_id`, so a human reading the journal can put
the two halves together. Neither is derived from the other on the wire.

An agent that only wanted the *app* to have the file may close its own
descriptor immediately; holding it is not required for the realm's copy to
work, and closing it does not revoke the realm's.

**The realm is never told which principal asked.** Naming the agent would make
every designation a cross-principal identifier the app could fingerprint and
correlate, in the one direction this protocol otherwise keeps closed: the app
is the least-trusted process in the system, with no petition, no grant, and no
name for any principal.

The shim relays rather than the core serving the app directly (P2.6.7): giving
the least-trusted process a socket into the TCB would add an attacker-facing
surface to the core for no gain, while the shim is already the app's
confinement peer and holds nothing it did not already hold.

**The shipped shim cannot receive the second half yet.** `designation` is the
first fd-bearing core→shim event this protocol has ever defined, and
`shim/include/wire.h` says in as many words that its transport implements
`SCM_RIGHTS` on the **send side only**: an arriving descriptor is a violation,
closed immediately, and then fatal. So a `designation` delivered to today's
shim would close the descriptor and kill the realm's connection. It costs
nothing today — no deployment serves the verb, so the core never sends the
event — and the receive-side machinery, along with the per-realm designation
socket, is what **P2.6.7** owes.

## Revocation cannot recall a delivered descriptor

**This is the limitation of this interface that will be misread if it is left
unstated, and it is stated in the IDL's normative `<description>` text as well
as here.**

A file descriptor that has crossed a socket is **kernel authority**. No
revocation, no expiry, no dead-man chord and no core-side bookkeeping closes it
in another process. Concretely:

- revoking the grant **stops future designations** and kills the grant row;
- every descriptor **already delivered** keeps working — on both connections —
  until its realm dies.

So PRD P2's *"revocation is immediate and transitive"* is **false for
designations already made**. The residue ends with the **realm**, not with the
grant.

This is inherent to handing out descriptors at all rather than a defect in any
implementation, and **no attenuation of a designation grant removes it**. A
deployment that cannot accept the residue must not serve `designate_file`.
E3.7's durable designation grants multiply exactly this residue rather than
reducing it, which is why it is exported as a limitation of artifact C5
(`docs/plan/02-phase-2-semantic-epochs.md` §1, risk R2.10) rather than left for
that epic to discover.

Two things this does *not* say. It does not say a designation is unbounded: the
descriptor names one file or one subtree, chosen by a human, and the realm's
confinement still bounds everything else it can reach. And it does not say
revocation is useless: an agent that has been revoked cannot ask again, which
is the half revocation *can* deliver.

## What this facet is not

It designates. It does not enumerate, browse, `stat`, or open by name. There is
no request that lists what a previous designation covered, no request that
re-opens one, and no request that revokes one — the last because there would be
nothing for it to do (see above).

A subtree arrives as **one directory fd** and the receiver walks it with the
kernel's own `openat`, which is what makes "subtree" a real containment
boundary rather than a prefix match on strings this protocol never sees.

## Lifecycle

The facet comes into existence when a principal calls
[`get_powerbox`](./04-vitrin_grant.md#get_powerbox) on a grant, which allocates
the client-supplied `new_id` and binds it to this interface. Minting is always
structurally successful and is **not an authority oracle** — and today that is
doing visible work: since no deployment serves the verb, a server that
implements the mint mints successfully and refuses every use. No shipped
server implements it yet; see [Served status](#served-status).

It is grant-derived, so it follows the inert-object rule: when its grant dies
(expiry or revocation) the facet goes **inert**, and requests on it are refused
*recoverably* via [`vitrin_grant.refused`](./04-vitrin_grant.md#refused), never
`invalid_object`. Neither version defines a destructor; the object lives for
the connection and its id is never reused.

## Requests

### request_file

`request_file(mode: mode)` — **since version 2**

| arg | type | description |
|---|---|---|
| `mode` | `uint` — enum [`mode`](#mode) | the access this ask is **for**; the human may narrow it |

Raises the core-drawn picker for a single file and, on the human's
confirmation, delivers its descriptor.

**Delivery class:** **reply-bearing**. Exactly one terminal per request, in
request order, never coalesced — and it is one of **three**:

| terminal | when | answered by |
|---|---|---|
| [`designated(…)`](#designated) | the human chose and the core resolved the choice safely | the human |
| [`refused(code)`](#refused) | the picker was raised and produced no descriptor | the human, or the core's own safety check |
| [`vitrin_grant.refused(designate_file, …)`](./04-vitrin_grant.md#refused) | the chokepoint declined the ask; **no picker was raised** | the chokepoint |

A three-way one-of rather than the usual pair, because there are genuinely two
different questions with two different answerers: the chokepoint decides
whether this grant may ask, and the human decides what to designate.
Collapsing them would make *"the human said no"* indistinguishable from *"your
grant expired"* — and an agent must be able to tell those apart, because one is
worth asking again and the other is not.

**This pair reached three first, and is no longer alone.**
[`vitrin_egress.request_connect`](19-vitrin_egress.md#three-terminals-not-two)
(P2.7.2) is the third member of the family, on an argument that shares nothing
with this one but the arity — its extra arm is the *far end's* non-answer, not a
second answerer. Recorded on both pages because *"the only request with three
terminals"* is exactly the sentence that went stale when these two facets landed
on parallel branches; [conventions
§6.1](00-conventions.md#61-reply-bearing-requests) holds the whole table and
says why no tool checks it.

**`mode` is what the agent asks for, never what it gets.** It selects which
picker chrome the human sees: a `read` ask is an open dialog, a `read_write`
ask is one that also offers to create. The human may narrow it, and
[`designated.mode`](#designated) carries the **effective** answer — the same
shape [`resolved.verbs`](./04-vitrin_grant.md#resolved) has toward
`request_grant`'s `verbs`.

Pipelining is legal and terminals pair in request order, exactly as
[`capture_frame`](./06-vitrin_view.md)'s do. Asks are rate-limited by the
grant's `max_event_rate` like every other use of a grant, which is what stops
an agent raising pickers faster than a human can dismiss them; the
per-principal single-picker rule ([`busy`](#refusal)) is the other half of the
same protection.

### request_dir

`request_dir()` — **since version 2**

No arguments. Raises the core-drawn picker for a single directory and, on the
human's confirmation, delivers **one directory fd** covering that subtree.
Identical to [`request_file`](#request_file) in delivery class, terminals,
ordering, rate limiting, and the absence of any steering argument.

**A subtree is one descriptor, not a batch.** That is the
[one-fd-per-message](./00-conventions.md) framing invariant doing real work
rather than being worked around. It is also why there is no request that
designates several files at once: a picker that let a human multi-select emits
one [`designated`](#designated) per file.

**No `mode` argument, deliberately**, and the asymmetry with `request_file` is
argued rather than accidental. `request_file`'s `mode` selects which picker
chrome the human sees; a subtree picker has one chrome, so a mode here would
steer nothing — and it would put the widest ask this verb can make (read-write
over a whole subtree) into the least visible place. The human's tick in the
picker decides it, and `designated.mode` carries the answer, so nothing is left
unstated on the wire. A chrome-selecting mode for directories, if ever needed,
is a `since`-gated sibling request; this signature is frozen forever.

## Events

### designated

`designated(fd: fd, designation_id: uint, kind: kind, mode: mode, name: string)` — **since version 2**

| arg | type | description |
|---|---|---|
| `fd` | `fd` | the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it |
| `designation_id` | `uint` | the core's **opaque** id for this designation, matching the journal record and the realm's `designation` |
| `kind` | `uint` — enum [`kind`](#kind) | file, or directory subtree |
| `mode` | `uint` — enum [`mode`](#mode) | the **effective** access the human approved |
| `name` | `string` (max 255 bytes) | basename of what the human chose, **for display only** |

Exactly one per successful ask, in request order, never coalesced, carrying
**exactly one** fd. Ownership transfers to the receiver, which MUST close it
after use; the core closes its own copy after sending.

**This fd outlives the grant** — see [revocation cannot recall a delivered
descriptor](#revocation-cannot-recall-a-delivered-descriptor).

`kind` is redundant with which request was answered, since terminals pair in
request order, and is carried anyway so a receiver that logs — or that hands
the descriptor onward — need not reconstruct it from a request it may no longer
have.

`mode` describes what the core **opened the descriptor with**. It is not a
promise about the file's permissions, which the kernel enforces and may change
underneath any holder.

### refused

`refused(code: refusal)` — **since version 2**

| arg | type | description |
|---|---|---|
| `code` | `uint` — enum [`refusal`](#refusal) | why the ask produced no descriptor |

The terminal of an ask that the chokepoint **allowed** and that still yielded
nothing — the human's answer, or the core's own refusal to designate what they
chose. Exactly one per refused ask, in request order, never coalesced.

**Not a second enforcement voice.** Authority questions are answered by
[`vitrin_grant.refused`](./04-vitrin_grant.md#refused), from the one
chokepoint, for every verb. This event answers a different question the
chokepoint never sees, and reading a code here as an authority verdict is a
mistake: **every code below is compatible with a perfectly live grant.**

**This event is not an oracle about the filesystem**, and the codes are chosen
so it cannot become one. None of them says anything about what exists, what is
readable, or where the human was looking. A code that distinguished *"you chose
a file you may not open"* from *"you cancelled"* would let an agent probe the
human's filesystem one prompt at a time, which is exactly the power a powerbox
exists to deny.

A refused ask leaves **no residue**: no descriptor was delivered, the realm was
told nothing, and the journal records the ask and its outcome. Asking again is
legal and is bounded by the same rate ceiling as any other use.

## Enums

### mode

The access a designation carries. Values are immutable and entries append; an
out-of-range value is fatal `invalid_argument`.

| entry | value | meaning |
|---|---|---|
| `read` | 0 | the descriptor is opened for reading |
| `read_write` | 1 | opened for reading and writing, so the holder may change or truncate what it names |

**There is no write-only rung, deliberately.** The pair exists to be put to a
human in one sentence — *may this app read it, or read and change it* — and a
third rung whose difference the human cannot act on is prompt noise rather than
attenuation. A save-only flow is served by `read_write`.

### kind

What a designated descriptor names. Values are immutable and entries append; an
out-of-range value is fatal `invalid_argument`.

| entry | value | meaning |
|---|---|---|
| `file` | 0 | a single file |
| `directory` | 1 | a directory, designating the whole subtree beneath it as one descriptor |

### refusal

Why a raised picker produced no descriptor. Answers are exhaustive rather than
optional: every ask the chokepoint allows gets exactly one terminal, and an ask
that designated nothing says why.

| entry | value | meaning |
|---|---|---|
| `cancelled` | 0 | the human dismissed the picker without choosing — the ordinary answer; asking again later is legal |
| `timed_out` | 1 | the picker was raised and expired unanswered, on the deployment's own deadline; distinct from `cancelled` because **nobody decided anything** |
| `busy` | 2 | a picker for this principal is already up; at most one at a time, because two stacked in front of one human is the consent-fatigue shape [`busy`](./04-vitrin_grant.md#outcome) already names at petition time |
| `unresolvable` | 3 | the human chose, and the core **would not** designate it: the entry could not be resolved without following a symlink, or the path lost a race between the confirmation and the open. The core refuses rather than delivering a descriptor that may not name what the human saw. It says **nothing** about whether the entry exists |

The set is deliberately small, and each entry is distinguished only because it
means something different to a client deciding whether to ask again — which is
the only decision this event informs.

**No SDK typed-exception mapping exists yet**, and that is recorded rather than
invented: the Python SDK does not implement the powerbox, so naming four
exceptions a second implementation would then be obliged to transcribe would be
fiction ([conventions § 5.3](./00-conventions.md#53-recoverable-errors)).

## Failure modes

*Fatal (the connection dies).* Sending any of this interface's opcodes on a
version-1 connection is `invalid_opcode`, as is an opcode this interface does
not define. An out-of-range `mode` on `request_file` is `invalid_argument`. A
frame whose declared size or `fd_count` disagrees with the signature is
`oversized` or `fd_violation` — and `designated` is one of the protocol's four
fd-bearing messages, so its `fd_count` **must** be 1 in the header *and* an fd
must actually accompany the frame; either disjunct alone failing is
`fd_violation` ([conventions § 2.4](./00-conventions.md#24-the-one-fd-per-message-invariant)).

*Recoverable.* Either [`refused`](#refused) above, or
`vitrin_grant.refused(designate_file, code, retry_after_ms)`:

| code | when |
|---|---|
| `not_granted` | the grant never held `designate_file`, or has not resolved `granted` — **the answer every deployment gives today** |
| `expired` | the grant's expiry passed |
| `revoked` | the grant was revoked |
| `rate_limited` | the grant's token bucket is empty; `retry_after_ms` > 0 |
| `internal` | a server-side failure while carrying the ask out |

`no_surface` is never produced: a designation reaches the realm's *shim*, which
exists whether or not its app has committed a surface. `capacity` is never
produced either — it concerns creating a realm. `preempted` and
`consent_held` are attention-shaped — they reach actuation and the layout
verbs — and whether they should reach a request that *raises a prompt of its
own* is a question P2.6.6 has to answer when it builds the picker; nothing here
forecloses either answer. That open question is **normative** and now says so
in the IDL, at [`vitrin_grant.refusal`](04-vitrin_grant.md#refusal), which
carries designation's reachable set alongside every other class's: leaving it
open in prose while the IDL's own enumeration silently closed it was the state
this page inherited.

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent, **C→S** core→shim.

### 1. What every deployment does today

```
1. A→C  vitrin_realm.request_grant(…, verbs=designate_file, …)
2. C→A  vitrin_grant.resolved(unsupported, 0, once, 0)
```

No deployment serves the verb, so this is the whole interaction. A petition
mixing `designate_file` with a served verb is refused **whole**, never narrowed
to the served remainder.

### 2. The shape the wire decides (not yet reachable)

```
1. [grant resolved granted with verbs=designate_file]
2. A→C  vitrin_grant.get_powerbox(powerbox=9)     [structural mint, no reply]
3. A→C  vitrin_powerbox.request_file(mode=read_write)
4.      [the core draws the picker; the human chooses]
5. C→A  vitrin_powerbox.designated(fd, id=7, kind=file, mode=read, "notes.txt")
6. C→S  vitrin_shim_session.designation(fd, id=7, kind=file, mode=read,
                                        "notes.txt")
```

Step 5 shows the narrowing that makes `designated.mode` load-bearing: the ask
was `read_write` and the human approved `read`.

### 3. The human declines

```
1. A→C  vitrin_powerbox.request_file(mode=read)
2. C→A  vitrin_powerbox.refused(cancelled)
```

The grant is live throughout. Nothing was delivered, the realm was told
nothing, and asking again is legal.

### 4. Minting without the verb

```
1. [grant resolved granted with verbs=observe only]
2. A→C  vitrin_grant.get_powerbox(powerbox=9)   [legal; mints fine]
3. A→C  vitrin_powerbox.request_dir()
4. C→A  vitrin_grant.refused(designate_file, not_granted, 0)
```

The mint succeeds and the *use* refuses. Refusing at mint time would turn the
mint into an oracle for what a grant holds.

Since issue #322 this is exactly what the reference core does, rather than the
shape a conforming server would have — and it is the answer for **every** verb
set, not only `observe`, because no grant anywhere carries `designate_file`.
The same four lines against a still-**pending** grant give the same answer:
minting before resolution is legal, and "use while pending, through an
ungranted facet" is one of the things
[`not_granted`](./04-vitrin_grant.md#refusal) names.

## Growth

- **`request_connect` and the `egress` verb — a facet of its own, not a
  request here.** `docs/plan/02-phase-2-semantic-epochs.md` E2.7 (P2.7.2) was
  drafted as a `request_connect(host, port)` on **this** interface, delivering
  a connected socket, on the reasoning that egress is "the socket analog of
  `request_file`". `interface/@verb` is **one value per interface** — it is what
  generates the single-site authority check — and this interface already
  declares `designate_file`, so a second verb's requests would reach the
  chokepoint with no entry. That is precisely the failure the
  [layout split](./04-vitrin_grant.md#get_layout_arrange) exists to prevent, so
  **`egress` takes its own facet interface and its own `get_*` mint on
  `vitrin_grant`**, exactly as the layout pair was forced to. The plan's
  sentence is the thing that was wrong, and it is corrected under E2.7's own
  issue rather than here; this interface stays a `designate_file` facet at
  every version.
- **A chrome-selecting `mode` for `request_dir`.** A `since`-gated sibling
  request, never arguments added to `request_dir`.
- **Further `mode` or `kind` entries.** Appended enum entries; values are
  immutable.
- **Re-designation and enumeration.** Not reserved for. A request that re-opened
  a previous designation would put an id where a human's choice belongs, and a
  request that enumerated live designations would be the filesystem oracle
  [`refused`](#refused) is written to deny. Either would need its own answer to
  those objections, not a widening of these two requests.

## Version history

| Version | Change |
|---|---|
| 1 | interface not defined |
| 2 | `request_file` (request opcode 0), `request_dir` (opcode 1), `designated` (event opcode 0), `refused` (opcode 1); enums `mode`, `kind`, `refusal`; minted by `vitrin_grant.get_powerbox`; exercises the `designate_file` verb (64) |

