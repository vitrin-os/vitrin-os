# vitrin_grant — capability handle and enforcement voice

**Interface version:** 2 · **Connection class:** principal · **Messages:** 5 requests *(all since 2)* + 2 events

## Purpose

`vitrin_grant` is the wire projection of a single grant-table row: one
principal's authority over one resource, expressed as a verb set plus
constraints (expiry, rate ceiling, persistence rung). It is the object an
agent holds to represent "I have been granted authority here", and it is the
single voice of the enforcement chokepoint — every capture and every actuation
performed under this grant is checked by one server-side function, and every
refusal that function produces is delivered on this object.

The grant sits at the center of the connection's authority chain. It is minted
— together with its [consent observer](05-vitrin_consent.md) and its three
authority facets ([view](06-vitrin_view.md),
[pointer](07-vitrin_actuator_pointer.md),
[text](08-vitrin_actuator_text.md)) — by
[`vitrin_realm.request_grant`](03-vitrin_realm.md). The facets carry the
verbs; the grant carries the row's identity and lifecycle. A facet confers
nothing on its own: it is checked at use time against *this* grant's effective
verb set, and a use outside that set is refused here.

The design idea is one chokepoint, one refusal voice. Rather than scatter
authority checks across the observation and actuation interfaces — each with
its own error vocabulary — the protocol routes every use-time failure through
`vitrin_grant.refused`, and every petition-time outcome through
`vitrin_grant.resolved`. This keeps the enforcement surface auditable (one
function to fuzz, one event to test) and gives the SDK a single place to map
wire codes onto typed exceptions.

The interface is events-only in version 1: it defines no requests. The grant
handle is not something the agent commands; it is something the agent observes.
Authority is exercised through the facets, and the grant merely reports how the
petition resolved and, thereafter, why any use was refused.

Version 2 adds five requests, and none is a command either:
[`get_launcher`](#get_launcher), [`get_layout_focus`](#get_layout_focus),
[`get_layout_arrange`](#get_layout_arrange),
[`get_powerbox`](#get_powerbox) and [`get_egress`](#get_egress) are
**structural mints** that hand back a
facet. The grant still answers no authority question itself. This is where
*every* facet added after version 1 must be minted, for one mechanical reason:
`request_grant`'s five `new_id` arguments are frozen forever, so a sixth
co-minted facet does not exist and never will.

## Lifecycle

A grant is born **pending** by `vitrin_realm.request_grant`, conferring
nothing. Its birth follows the [multi-`new_id` rule](00-conventions.md): it is
one of five `new_id` arguments minted in that single request, all distinct,
strictly increasing in argument order, and above the connection's allocation
watermark.

Exactly one [`resolved`](#resolved) event ever decides the grant's fate. On
outcome `granted` the grant becomes active and its facets confer their verbs;
on any other outcome the grant is dead from birth and its facets never confer
anything. This event fires **exactly once per grant, ever** — denial, timeout,
and every other terminal outcome are clean distinct events, never a hang and
never a connection death.

A grant that resolved `granted` later goes **dead** when its expiry passes or
it is revoked (by hold-Esc, a panel action, or policy). A dead grant's facets
go **inert**: requests on them are refused *recoverably* via
[`refused`](#refused), never fatally. This is the taxonomy corollary described
on the [conventions page](00-conventions.md) — human revocation racing an
in-flight request must not kill a well-behaved agent, so a dead grant yields
`refused`, never `invalid_object`. Because object ids are never reused, the
server MAY continue to emit events referencing a dead grant's objects; clients
MUST tolerate and discard them.

Version 1 has **no grant persistence**: every grant dies with the connection,
and there are no restore tokens (a future version's addition). A pending
petition is withdrawn if the connection closes — consent is in-context, so the
prompt disappears with the petitioner. Neither version defines a **destructor**
on this interface; see [Growth](#growth).

## Requests

### get_launcher

`get_launcher(launcher: new_id)` — **since version 2**

| arg | type | description |
|---|---|---|
| `launcher` | `new_id` → [`vitrin_launcher`](16-vitrin_launcher.md) | the launch facet, born **inert** |

A **structural mint**: it creates the facet through which
[`realm_launch`](#verb) is exercised, and nothing else. Like every structural
mint it is neither reply-bearing nor refusable — no terminal event, no wire
acknowledgement — and a malformed mint (an id at or below the watermark, in
the server-reserved range, or otherwise illegal) is a fatal `invalid_object`,
an object-graph error rather than an authority answer.

**Minting is always legal.** The request is defined for every grant, whatever
verbs it holds and whether or not it has resolved. That is deliberate and
follows the pattern `request_grant`'s co-minted facets already establish: mint
freely, check at use. A launcher minted from a grant without `realm_launch`
refuses `not_granted` on its first `launch`; a launcher minted while the
petition is still pending does the same. Neither is a protocol error, and
neither leaks anything about the petition — refusing at mint time would make
the mint an authority oracle, which is exactly what the inert-birth rule
avoids.

**Why here and not on `request_grant`.** `request_grant`'s five `new_id`
arguments are frozen forever, like every message signature, so no facet added
after version 1 can be co-minted. A mint on the grant that authorizes the
facet is the documented route (see [Growth](#growth)); the two layout facets
below arrived the same way.

**Calling it twice** mints a second, equivalent facet. This is not a special
case: no version defines a destructor, ids are never reused, and each minted
object is checked against the *same* grant at use time, so a duplicate confers
no additional authority. The per-connection live-object cap is what bounds it,
and breaching that cap is fatal `resource_exhausted` like any other.

**Version gating.** `get_launcher` is `since="2"`: it is not defined on a
version-1 connection, where sending its opcode is fatal `invalid_opcode`. The
[`realm_launch`](#verb) *verb bit*, by contrast, is not version-gated at all —
a version-1 connection may name it in a petition and is answered
`unsupported`, because a bitfield is one mask checked identically on every
version (see [conventions § 7.3](00-conventions.md)).

### get_layout_focus

`get_layout_focus(layout_focus: new_id)` — **since version 2**

| arg | type | description |
|---|---|---|
| `layout_focus` | `new_id` → [`vitrin_layout_focus`](17-vitrin_layout_focus.md) | the focus facet, born **inert** |

A **structural mint**, on exactly [`get_launcher`](#get_launcher)'s terms:
neither reply-bearing nor refusable, always legal whatever verbs the grant
holds and whether or not it has resolved, born inert, duplicates permitted and
conferring nothing extra, bounded by the per-connection live-object cap whose
breach is fatal `resource_exhausted`, and `since="2"` so the opcode does not
exist on a version-1 connection.

The one difference worth naming: unlike a launcher, the use this mints **can
succeed** — the reference core serves `layout_focus`.

### get_layout_arrange

`get_layout_arrange(layout_arrange: new_id)` — **since version 2**

| arg | type | description |
|---|---|---|
| `layout_arrange` | `new_id` → [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) | the arrangement facet, born **inert** |

Identical in every structural respect to
[`get_layout_focus`](#get_layout_focus).

**A separate mint, deliberately.** One facet interface declares exactly one
grant verb — that is what generates the single-site authority check — and
`layout_arrange` and `layout_focus` must stay independently attenuable. A
shell granted arrangement but not focus holds a live
`vitrin_layout_arrange` and a `vitrin_layout_focus` that refuses `not_granted`
forever, which is exactly the shape that separation is for. A combined
`get_layout` could not express it.

### get_powerbox

`get_powerbox(powerbox: new_id)` — **since version 2**

| arg | type | description |
|---|---|---|
| `powerbox` | `new_id` → [`vitrin_powerbox`](13-vitrin_powerbox.md) | the powerbox facet, born **inert** |

A **structural mint**, on exactly [`get_launcher`](#get_launcher)'s terms:
neither reply-bearing nor refusable, always legal whatever verbs the grant
holds and whether or not it has resolved, born inert, duplicates permitted and
conferring nothing extra, bounded by the per-connection live-object cap whose
breach is fatal `resource_exhausted`, and `since="2"` so the opcode does not
exist on a version-1 connection.

**The mint is not an oracle, and today that is doing visible work.** No
deployment serves [`designate_file`](#defined-but-unserved), so no grant
carries the bit and no petition naming it resolves `granted`. A server that
implements this request therefore mints successfully *everywhere* and refuses
every use of the facet *everywhere* — the defined-but-unserved staging
behaving exactly as designed, not a defect. Refusing at mint time would leak
what a grant holds; that is why the mint never answers an authority question.

**The reference core is now such a server** (issue #322). `vitrind`
dispatches this opcode: the mint succeeds, the facet enters the object table
bound to this grant, nothing is put on the wire in answer, and every
[`request_file`](13-vitrin_powerbox.md#request_file) and
[`request_dir`](13-vitrin_powerbox.md#request_dir) asked through it draws
`refused(designate_file, not_granted)` — recoverable, connection intact. Only
the object-graph rules can fail the mint, fatally: a `new_id` that breaks the
id rules (`invalid_object`), or the per-connection live-object cap
(`resource_exhausted`). What is still absent is the picker (**P2.6.6**) and the
consent copy (**P2.6.8**) that would let a petition naming `designate_file`
resolve `granted` at all — an absence in the *verb*, not in this request.

> **It was not, until issue #322, and the record is kept because the failure
> mode is generic.** This request reached the wire with no dispatch arm behind
> it, so a conformant version-2 client that minted the facet was answered
> fatal `invalid_opcode` and disconnected for sending a documented request of
> the version that core negotiates. A paragraph here said so, a paragraph in
> the IDL said so, and a core test pinned the fatal answer; none of the three
> made the arm land, because prose that describes a defect is not a check that
> closes it. What replaced them is structural: a core test derives this
> interface's mint opcodes from generated code and fails on any one of them no
> arm dispatches, so a later mint cannot land vocabulary-only the way this one
> did.

### get_egress

`get_egress(egress: new_id)` — **since version 2**

| arg | type | description |
|---|---|---|
| `egress` | `new_id` → [`vitrin_egress`](19-vitrin_egress.md) | the egress facet, born **inert** |

Identical in every structural respect to
[`get_layout_focus`](#get_layout_focus) and to
[`get_powerbox`](#get_powerbox): neither reply-bearing nor refusable,
always legal whatever verbs the grant holds and whether or not it has
resolved, born inert, duplicates permitted and conferring nothing extra,
bounded by the per-connection live-object cap, and `since="2"`.

**A separate interface, for the same reason the layout facet is two.**
`interface/@verb` is one value per interface, so the interface that declares
`verb="designate_file"` — the filesystem powerbox, page 13 — cannot also
declare `verb="egress"`. Its `request_connect` would reach the enforcement
chokepoint with no verb to check it against. See [Growth](#growth) for the
full argument.

**This request is opcode 4, not opcode 3.** It was drafted as the fourth
mint at opcode 3 while [`get_powerbox`](#get_powerbox) was being drafted at the
same number on a parallel branch. Powerbox shipped first and keeps 3; opcodes
follow declaration order and are immutable once shipped, so the unshipped one
moved rather than the shipped one.

**The use this mints cannot succeed at any deployment**, which it shares with
`get_powerbox` alone among its four siblings and is a stronger statement than
`get_launcher`'s ever was. `egress` is in no deployment's served set, so no
grant resolves `granted` carrying it, so every `request_connect` through every
facet minted here refuses `not_granted`. Minting is defined anyway, on exactly
the terms `get_launcher` was defined on before any deployment served
`realm_launch`: a mint that refused would be an authority oracle, and a facet
that refuses on first use leaks nothing.

> **Implementation status, stated rather than implied.** The reference core
> implements this request (issue #322): `vitrind`'s grant-object dispatch
> mints the facet, answers nothing on the wire, and refuses every
> [`request_connect`](19-vitrin_egress.md#request_connect) asked through it
> `not_granted` — "always legal" at the mint, "refuses at use" at the
> chokepoint, exactly as this page describes. Nothing about the **verb** moved
> with it: `egress` is served by nobody, and P2.7.3 still owns the proxy that
> would change that. Until #322 this and `get_powerbox` were the two mints on
> this interface a reader could not assume the shipped binary answered — both
> were on the wire with no arm behind them, so both were answered fatal
> `invalid_opcode` and the connection died. One change closed both, together
> with the two facets' own requests, because a mint whose object serves no
> request moves the identical defect one interface down.

## Events

### resolved

`resolved(outcome: uint, verbs: uint, persistence: uint, expiry_ms: uint)`

| arg | type | description |
|---|---|---|
| `outcome` | uint (enum [`outcome`](#outcome)) | how the petition resolved |
| `verbs` | uint (enum [`verb`](#verb), bitfield) | effective verb set; 0 unless `outcome` is `granted` |
| `persistence` | uint (enum [`persistence`](#persistence)) | effective persistence rung; `once` (0) unless granted |
| `expiry_ms` | uint | effective lifetime in milliseconds; 0 = bounded by the persistence rung |

The terminal event of the pending phase, sent after the consent decision (or
policy decision) completes. It is sent **exactly once per grant, ever**.

On outcome `granted`, the trailing arguments carry the **effective** authority
— the verb set, persistence rung, and expiry the human actually chose. These
MAY be narrower than what `request_grant` petitioned for: an "Allow once"
button versus "Allow while running" changes the lifetime, and the human may
approve fewer verbs than were requested. The agent MUST treat these effective
values, not its request, as ground truth for what the grant confers.

On any outcome other than `granted`, the trailing arguments are zero-filled
(`verbs` = 0, `persistence` = `once`, `expiry_ms` = 0) and carry no meaning.

The effective `max_event_rate` is deliberately **not** echoed here: an agent
discovers throttling operationally, through
[`refused(rate_limited)`](#refused) and its `retry_after_ms` hint.

**Delivery class:** this is the terminal event of the reply-bearing
`request_grant` and is never coalesced — but unlike other reply-bearing
terminals it is **exempt from the cross-request "in request order" rule**: it
waits on an unbounded human consent delay, is sequenced only within its own
petition's lifecycle (zero or more `vitrin_consent.state` events, then exactly
one `resolved`), and a later `sync`'s `done` does **not** wait for it. Every
outcome — including `denied`, `timed_out`, `unavailable`, `unsupported`, and
`busy` — is a normal recoverable answer, not a protocol violation: a denial is
an answer, not an error. The blocking SDK sends `request_grant`, then reads
until `resolved`, mapping the outcome onto success or a typed exception.

### refused

`refused(verb: uint, code: uint, retry_after_ms: uint)`

| arg | type | description |
|---|---|---|
| `verb` | uint (enum [`verb`](#verb)) | the verb whose use was refused; also identifies the facet |
| `code` | uint (enum [`refusal`](#refusal)) | why the use was refused |
| `retry_after_ms` | uint | refill hint in milliseconds; greater than zero only for `rate_limited`, otherwise 0 |

The single recoverable-error event for grant *use*, covering capture and
actuation alike — one chokepoint, one refusal voice. `verb` names the verb
whose use was refused, which also identifies the facet that issued the offending
request (`observe` → [view](06-vitrin_view.md), `actuate_pointer` →
[pointer](07-vitrin_actuator_pointer.md), `actuate_text` →
[text](08-vitrin_actuator_text.md)).

**Delivery class depends on the refused request.** For the reply-bearing
[`vitrin_view.capture_frame`](06-vitrin_view.md), this event is that capture's
terminal: exactly one of `vitrin_view.frame_ready` or
`refused(observe, …)` per capture, in request order, **never coalesced**. The
type system forces this pairing — an `fd` argument has no null form, so a
failed capture cannot be a `frame_ready` with an absent fd; it must be a
distinct event.

For the fire-and-forget actuation requests (`move`, `button`, `scroll`,
`type`), refusals **MAY be coalesced** per the [delivery
classification](00-conventions.md): at most one `refused(rate_limited)` per
grant per bucket-refill window, and at most one `refused` per grant per
(verb, code) pair until a subsequent request on that grant succeeds. A
threadless client bounds its refusal discovery to one round trip by issuing
[`vitrin_handshake.sync`](01-vitrin_handshake.md) after a batch of actuations
and reading until `done`, raising on any `refused` seen.

`retry_after_ms` is greater than zero **only** for `rate_limited`, where it
hints when the token bucket refills; for every other code it is 0.

Because `refused` is a use-time event, it may reference a facet whose grant has
already gone dead. This is expected: `refused` remains the enforcement-bearing
signal even for expired and revoked grants (`expired`, `revoked` codes), and
it never escalates to a fatal error.

## Enums

### verb

Bitfield. The grantable verbs. Every entry has one SDK-level dotted name,
formed by replacing the first underscore of the wire name with a dot:
<!-- vitrin-verb-set: all-verbs = observe, actuate_pointer, actuate_text, observe_cursor, layout_arrange, layout_focus, designate_file, egress, realm_launch -->
`observe`, `actuate.pointer`, `actuate.text`, `observe.cursor`,
`layout.arrange`, `layout.focus`, `designate.file`, `egress`,
`realm.launch`. The spelling is
fixed by the IDL so a second implementation transcribing this enum has no name
to invent — including the case the rule does not cover on its face: a wire name
with **no** underscore has nothing to replace, so its dotted name is the wire
name unchanged. `egress` is the first such entry.

| entry | value | served | meaning |
|---|---|---|---|
| `observe` | 0x1 | yes | capture frames of the granted resource |
| `actuate_pointer` | 0x2 | yes | inject pointer motion, buttons, and scroll |
| `actuate_text` | 0x4 | yes | inject Unicode text |
| `observe_cursor` | 0x8 | **no** — resolves `unsupported` | capture frames that include the human principal's cursor; meaningful only alongside `observe` |
| `layout_arrange` | 0x10 | yes | arrange the granted realm's view, through the [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) facet; **one holder per output** — a live grant carrying it, or a petition still pending for it — so a second petition while either exists resolves `layout_held` |
| `layout_focus` | 0x20 | yes | bind the output to the granted realm and direct the human's input there, through the [`vitrin_layout_focus`](17-vitrin_layout_focus.md) facet |
| `designate_file` | 0x40 | **no** — resolves `unsupported` everywhere | designate one file or one directory subtree to the granted realm, through the [`vitrin_powerbox`](13-vitrin_powerbox.md) facet; the human picks and what crosses is a **descriptor, never a path**. **A delivered fd cannot be recalled** — see [that page](13-vitrin_powerbox.md#revocation-cannot-recall-a-delivered-descriptor) |
| `egress` | 0x80 | **no** — resolves `unsupported` everywhere | open one outbound connection to the single `host:port` this grant's [`net:` selector](#the-net-resource-prefix) names, through an out-of-core mediating proxy, using the [`vitrin_egress`](19-vitrin_egress.md) facet. The facet exists; **the proxy does not**, so no deployment serves the verb |
| `realm_launch` | 0x200 | yes | launch the realm template this grant addresses into a new realm instance, through the [`vitrin_launcher`](16-vitrin_launcher.md) facet |

The **served** column describes the reference core. Whether a defined verb is
served is a property of a *deployment*, so a client reads `unsupported` as
"not here, not now", never as "not in this protocol".

`VALID_MASK` is therefore **767** (`0x2ff`), not `0x1ff`. It was 575 until P2.6.5 added bit 64, and 639 until P2.7.2 added bit 128. The plan's registry re-pins it **once per epic**, never once per task, and [names every site that holds it](../plan/02-phase-2-semantic-epochs.md) rather than leaving them to be found — a list `cargo xtask limits-check` holds to the tree in both directions, so a pin the registry does not name is a red build. The same gate holds this sentence's own number against the generated constant, which is why no count of sites is repeated here.

This enum is the type of `request_grant`'s `verbs` argument, of
`resolved.verbs`, and of `refused.verb`.
<!-- vitrin-verb-set: facet-verbs = observe, actuate_pointer, actuate_text, layout_arrange, layout_focus, designate_file, egress, realm_launch | count: eight -->
**Eight** verbs map one-to-one to a
facet interface and to that interface's `@verb` annotation, which drives the
scanner-generated chokepoint table; `observe_cursor` is the **one** that does
not, by construction. `designate_file` and `egress` were exceptions until their
facets ([`vitrin_powerbox`](13-vitrin_powerbox.md) and
[`vitrin_egress`](19-vitrin_egress.md)) landed in P2.6.5 and P2.7.2's second
half — which changed this count and changed nothing about whether either verb
is *served*, a distinction [Defined but unserved](#defined-but-unserved) keeps.
That eight is **derived rather than remembered**, and the first attempt at
saying so overstated it.
`crates/vitrin-protocol/tests/decode_errors.rs`'s
`every_verb_is_classified_as_facet_bearing_or_not` used to count a
hand-maintained list of per-interface `VERB` constants, which meant a
*seventh* facet on an existing verb — exactly what landed with the egress facet
— left it green while this sentence became false. Both sides now come from the
generator: `generated::FACET_VERBS` is emitted from `interface/@verb`, and the
facetless remainder is a set difference over `Verb::ENTRIES`. So a verb
appended without being classified, or a facet added to a verb that had none,
is a red test rather than a stale sentence — this sentence and [its
restatement further down](#defined-but-unserved) — and `cargo xtask verb-sets
--check` holds every other page that states either set. Later
phases append entries (for example
key actuation, credential presentation, subtree reads) without touching
existing bits; values are immutable.

#### The gap between 0x20 and 0x200 is allocation, not free space

`realm_launch` is 512 rather than 64 because **64, 128 and 256 were already
spoken for** — allocated to verbs (`designate_file`, `egress`,
`publish_tree`) that had not landed in the IDL. Bits are allocated once,
repo-wide, in `docs/plan/02-phase-2-semantic-epochs.md` §5, and anything
adding a verb allocates there first, whatever document schedules the work.

**64 has since landed as `designate_file`** (P2.6.5) and **128 as `egress`**
(P2.7.2), each taken from that registry rather than from the next
unused-looking power of two — two parallel workstreams drawing one bit each
and not colliding, which is the registry doing its job. **256
(`publish_tree`) is the one bit still spoken for and still absent.**

This matters because a verb value is **immutable once landed**: a collision is
not a rename, it is two authorities permanently sharing one bit. Reading the
IDL and taking the next unused-looking power of two is exactly how that
happens — and it nearly did here, which is why the rule is written down rather
than assumed.

A reserved-but-undefined bit is still **out of range on the wire**, so
petitioning for 256 today is fatal `invalid_argument`, not
`unsupported`. Petitioning for 64 or 128 is now `unsupported` instead — that
flip, from a killed connection to an answer, is the whole of what "defining a
bit before serving it" buys.

#### The `net:` resource prefix

`egress` is the one verb whose authority the realm does not fully name. A grant
that said only "this realm may reach the network" would be exactly the blanket
authority this design exists to refuse, so the rest of the authority travels in
[`request_grant`'s `resource` selector](03-vitrin_realm.md#request_grant), in
its type-prefixed vocabulary, spelled:

```
net:HOST:PORT
```

The grammar is **wildcard-free by construction**, and that is the point rather
than a restriction someone will later relax:

| element | admitted | refused |
|---|---|---|
| host | exactly one — a DNS name, an IPv4 literal, or a **bracketed** IPv6 literal (`[2001:db8::1]`) | `*`, `*.example.com`, a leading `.`, an empty label, a CIDR suffix (`10.0.0.0/8`), a comma-separated list, whitespace, an unbracketed IPv6 literal |
| port | exactly one decimal integer in `1`–`65535`, in its canonical spelling | `0`, `65536`, a range (`443-8443`), a list (`443,80`), a signed form (`+443`), a leading zero (`0443`) |

The canonical-spelling rule on the port is not fussiness, and the reason is
narrower than it looks: **the port is the one half of the selector a parser
normalises** — it becomes an integer — so a non-canonical spelling would
re-serialize to a *different* string from the one the human approved. `0443`
is refused so that `parse` followed by re-serialization is byte-identity, not
so that the selector as a whole is canonical.

**The selector as a whole is deliberately not canonical, and a reader must not
infer that it is.** The host is stored exactly as it was presented, so **one
endpoint can have more than one selector string**, and none of them covers
another:

| one endpoint | two selectors, because |
|---|---|
| `net:Example.com:443` and `net:example.com:443` | DNS is case-insensitive; these bytes are not |
| `net:[2001:db8::1]:443` and `net:[2001:0db8:0000:0000:0000:0000:0000:0001]:443` | one IPv6 address has many legal literals, and the literal is kept verbatim |

Both spellings parse, both round-trip byte-identically, and `covers` is false
between them. That errs **narrow** — the wrong answer is a refusal, never an
unapproved connection — which is the only direction this comparison is
permitted to be wrong in, and normalising instead would make the grant row hold
a string the human was never shown. The reference core pins this by test
(`crates/vitrin-core/src/grants.rs`,
`one_endpoint_can_have_several_selector_strings_and_none_covers_another`).

Consequences worth stating rather than deriving:

- **A blanket egress grant is not refused; it is inexpressible.** There is no
  syntax to abuse. That is why an ergonomics answer for browser-shaped realms
  can be an *enumerated template* rather than an allowlist language — a
  template is a petition shortcut, never a pre-approval, and it still raises
  exactly one prompt.
- **One selector covers exactly one selector: itself.** `net:example.com:443`
  does not cover `net:example.com:80` and does not cover
  `net:sub.example.com:443`. With no wildcard there is no subsumption to
  express, and a server that invented one would widen authority the human never
  approved. Comparison is byte-exact, with the consequence stated in full
  above: two spellings of one endpoint are two selectors, and neither covers
  the other.
- **Whole-realm authority is not egress authority.** A null-or-empty `resource`
  means the whole realm, and it does **not** cover a `net:` endpoint. Reading it
  as covering one would make every `observe` grant an egress grant.
- **A name is not the authority; the addresses behind it at grant time are —
  and this rule is specified, not implemented.** DNS is to resolve only in the
  out-of-core proxy — there is no resolver inside a realm to route around — and
  the addresses the name resolved to when the human approved are to be pinned
  into the grant row, so that a connection to an address the pin does not
  contain is outside what was approved and is refused `not_granted`,
  **including a literal-IP connection under a name-scoped grant**. Keeping the
  pin in the row rather than in the proxy is what stops a DNS rebind winning by
  outlasting a process. **Nothing enforces any of this today**: the resolver
  and the proxy are P2.7.3's, the pin is P2.7.4's, and the reference core's
  `pinned_addrs` column is present-but-null by construction (an empty enum, so
  no row can carry a value). The rule is written in the future tense it is in
  because the column and the proxy are *built to* it — not because a deployment
  can be found that obeys it.

**The fatal-vs-recoverable razor, for this vocabulary specifically**
([conventions §5](00-conventions.md#5-error-taxonomy)):

- an **out-of-range verb bit** (64 or 256 today) stays fatal
  `invalid_argument` — the client violated the grammar;
- a **defined-but-unserved** `egress` resolves `unsupported` — a well-formed
  petition the deployment declines, whole, never narrowed to the served
  remainder;
- a `net:` selector that **does not parse** likewise resolves `unsupported`,
  not `invalid_argument`: the wire bound on `resource` is a byte length, and
  its *content* is a policy question. A selector is never widened to something
  that does parse;
- a **use-time** refusal voices through [`vitrin_grant.refused`](#refused) at
  the one emission site, like every other use-time refusal in this protocol —
  including a connection to an address the pin does not contain, which is
  `not_granted` because it is authority the human did not approve;
- a **transport** failure — the endpoint was covered, the chokepoint admitted
  the use, and the far end did not answer — is **not** a `refused` code at
  all. It voices through
  [`vitrin_egress.connect_failed`](19-vitrin_egress.md#connect_failed), and
  that page argues why at length. The short form: `refused` is the
  chokepoint's voice and every code in it is something a server *decided*
  about a grant, so rounding "the host is down" into `not_granted` would tell
  an agent to stop asking about work it is permitted to do.

**What has landed and what has not, stated so it is findable.** The verb bit
and this grammar are P2.7.2's first half; the facet through which a connection
is *asked for* — [`vitrin_egress`](19-vitrin_egress.md), carrying
`request_connect`, `connected` and `connect_failed`, minted by
[`get_egress`](#get_egress) — is its second half and is in the IDL now. It is
**an interface of its own**, not a request on the filesystem powerbox
[`vitrin_powerbox`](13-vitrin_powerbox.md) (page 13, which P2.6.5 landed
first), and the dialect settles that
rather than taste: `interface/@verb` is one value per interface, so the
interface that declares `verb="designate_file"` cannot also declare
`verb="egress"` (see [Growth](#growth) for the full argument, and
[`get_layout_arrange`](#get_layout_arrange) for the same rule splitting the
layout facet).

**The facet does not make the verb served, and the two are worth keeping
apart.** Every deployment still refuses a petition naming `egress`
`unsupported`, and the reason has narrowed rather than gone: what is missing
is the out-of-core mediating proxy, which is P2.7.3's. A facet is a request to
ask through; it is not a mechanism to answer with.

Three implementation gaps, named here so this section is a *complete* list
rather than a list that reads complete:

- **The reference core implements the facet's messages and none of the
  comparison behind them.** Since issue #322 `vitrind` mints `vitrin_egress`
  and decodes `request_connect` — enforcing the `port` domain fatally and the
  `host` bound in the decoder — and then refuses `not_granted` at the
  chokepoint, because `egress` is in no served set. The endpoint the request
  names is decoded and **dropped**: the narrowing comparison this section
  specifies, request endpoint against the grant's `net:` selector, has nothing
  to run against while no grant row can carry the bit. P2.7.3 owes the
  comparison and the proxy *in one change* — serving the verb without the
  comparison would turn a grant over one endpoint into reach to every
  endpoint.
- **The parser for this grammar is never called.** It exists
  (`crates/vitrin-core/src/grants.rs`, `NetSelector`) and nothing in the
  admission path reaches it — a `net:` petition is refused like any other
  non-empty selector.

- **The address pin is specified and unimplemented.** `pinned_addrs` is a
  present-but-null column (an empty enum); there is no resolver, no proxy and
  no chokepoint arm that consults a pin. P2.7.3 builds the proxy and P2.7.4
  fills the column. Until then the bullet above describes a design, not a
  behaviour.

One more, which is about the grammar rather than about an implementation:
- **The host is not validated as a DNS name or an IP literal.** The parser
  enforces a *denylist* — `*`, `/`, `,`, `[`, `]`, `:`, whitespace, control
  characters, and an empty label in any position (a leading `.`, a doubled
  `..`, or a trailing `.`) — and keeps whatever else it was given. So `net:-:443`, `net:user@evil.com:443`, `net:999.999.999.999:443` and
  a Unicode homograph of a real name all parse today. None of them *widens*
  authority — `covers` is exact match, so no accepted selector can ever name
  more than one endpoint, which is the sense in which the grammar is
  wildcard-free by construction — but a homograph is a confusion attack on the
  human, and P2.7.3, which is the first task to render one of these strings on
  a consent card, owns deciding the host charset before it does.

#### Defined but unserved

A verb may be defined on the wire ahead of being served and **refused
`unsupported`** by a deployment that does not serve it — the same posture the
[`persistence`](#persistence) ladder takes toward its durable rungs.

Six verbs have been defined this way. `observe_cursor`, `layout_arrange` and
`layout_focus` were defined from day one; `realm_launch` and `designate_file`
arrived with version 2, and `egress` at P2.7.2; of those, **three are now
served** by the reference core — each has
a facet interface, an enforcement arm and consent copy naming its consequence
in plain language. `layout_arrange` and `layout_focus` joined at WS-E.1.4, and
`realm_launch` at WS-E.1.1, when the core gained the spawn path, the realm cap
and the prompt line its refusal had stood for.

<!-- vitrin-verb-set: unserved-verbs = observe_cursor, designate_file, egress | count: three -->
**Three remain**, and for three different missing mechanisms.

`observe_cursor`'s reason has not moved: the
per-principal cursor *delivery* it would widen a capture with does not exist
(D-017, D-019), so serving the verb would promise something no capture
carries.

**`designate_file` is the second**, and unlike `observe_cursor` its refusal has
a scheduled end. It landed at P2.6.5 with its facet interface and nothing
else: no picker mints a descriptor (P2.6.6) and no consent copy names what
approving it costs (P2.6.8 — Q13's rule that no verb is served before a human
can be told what it means). Both must land before any deployment may answer a
petition naming it anything but `unsupported`. In the reference core that is
structural rather than a promise: `SERVED_VERB_BITS` does not list the bit,
the unserved set is *derived* from the wire mask, and admission refuses the
petition **whole** — so forgetting the rest of E2.6 produces a refusal, never
a grant nothing enforces.

**`egress` is the third**, refused by **every** deployment, and its reason has
*narrowed once* without going away — worth stating in that shape, because "the
facet does not exist" was the reason given when the bit landed and it is no
longer true. [`vitrin_egress`](19-vitrin_egress.md) is in the IDL, an interface
of its own rather than a request on the filesystem powerbox (see
[Growth](#growth)). What is still absent is the out-of-core mediating proxy
that would ask the chokepoint per connection, which is P2.7.3's — and a verb
whose mechanism does not exist cannot be served, because a deployment MUST NOT
grant a verb it does not enforce. Landing the bit before the facet, and the
facet before the mechanism, is the same staging `realm_launch` used and for the
same reason: a petition naming it is answered rather than killed. As with
`designate_file`, the reference core makes that structural rather than a
promise: `SERVED_VERB_BITS` does not list the bit.

Serving a verb is a **deployment** property, not a version property. A
deployment that will not host process creation must refuse `realm_launch`
`unsupported` — nothing in this protocol obliges a server to serve every verb
it can decode, and `capacity`'s [own note](#refusal) says in as many words
that a deployment which cannot afford that refusal's side channel must not
serve the verb at all.

The staging is structural rather than cosmetic, for two reasons:

1. **An out-of-range bit is fatal.** A bitfield argument is validated as a
   mask, so a bit outside the defined union is `invalid_argument` and the
   connection dies (see [conventions § error taxonomy](00-conventions.md)). A
   client petitioning for authority this deployment does not serve would be
   killed rather than answered. Defining the bit converts that into a
   recoverable `resolved(unsupported)`.
2. **The model would otherwise be unstateable.** Decisions D-017 and D-018
   settle that cursor visibility is *authority* rather than a display
   preference, and that scene arrangement is a *grant* rather than the shell's
   ambient property. `realm_launch` settles the same kind of question for
   process creation: starting an app is authority, so it is a verb rather than
   a request on the authority-free realm handle, and the program it starts is
   named by operator configuration rather than by the wire. None of that is
   expressible without a verb, and adding one later is a version bump against
   deployed clients.

Two rules hold for every verb a deployment does not serve:

- **A deployment MUST NOT grant a verb it does not enforce.** `unsupported` is
  the honest answer; accepting the bit and enforcing nothing is the failure
  this section exists to prevent.
- **A mixed petition is refused whole.** `verbs = observe|observe_cursor`
  resolves `unsupported`; the server does not quietly drop the unserved bit and
  grant `observe`. Narrowing a verb set is the human's move at consent time —
  a silent server-side edit would leave the agent believing it holds authority
  nobody checks.

<!-- vitrin-verb-set: facetless-verbs = observe_cursor -->
Not every verb has a facet interface. **One** does not: `observe_cursor`, and
by construction rather than by schedule — it widens what
[`vitrin_view.capture_frame`](06-vitrin_view.md) composites rather than adding
a request, so there is nothing for an interface to be. It was one of *three*
until the powerbox and egress facets landed; `designate_file` and `egress` were
facetless only **yet**, which is
the difference between a gap and a design. The other **eight** verbs each have
one — the same eight counted [above](#verb), and the count is held by a test
rather than by memory (`crates/vitrin-protocol/tests/decode_errors.rs`,
`every_verb_is_classified_as_facet_bearing_or_not`, which fails the moment a
verb bit appears without being put on one side of this line, and which went
red on exactly these changes) — and the **five** added at version 2 all arrive as
`since`-gated mints on *this* interface, because
`request_grant`'s five `new_id` arguments are frozen forever (see
[Growth](#growth)): [`get_launcher`](#get_launcher),
[`get_layout_focus`](#get_layout_focus),
[`get_layout_arrange`](#get_layout_arrange),
[`get_powerbox`](#get_powerbox) and
[`get_egress`](#get_egress).

**The layout verbs take two facets, not one, and that is forced rather than
chosen.** A facet interface declares exactly one grant verb — that is what
generates the single-site authority check — so one combined layout facet could
name only one of them, and the other's requests would reach the chokepoint
with no verb to check them against. D-018(3) requires the two independently
attenuable; two interfaces is how the dialect makes that structural.

Where a deployment does not serve one of these verbs, the facet is still on
the wire: the deployment refuses the **verb**, not the mint. Minting is always
legal for every facet, and the use is what refuses — which is what keeps the
mint from becoming an oracle for what a grant holds.

#### Verb composition

One dependency exists, and it is the only one. **`observe_cursor` is meaningful
only alongside `observe`.** It widens what a capture *contains*, so a petition
naming it without `observe` names no capture to widen; such a petition resolves
`unsupported` rather than granting a bit that changes nothing. That follows from
the rule directly above — a deployment MUST NOT grant a verb it does not
enforce — and it settles the case the wire would otherwise leave open:
`observe_cursor` is **not** an independent authority and is never
inert-but-held. Every other verb (`observe`, `actuate_pointer`, `actuate_text`,
`layout_arrange`, `layout_focus`, `designate_file`, `egress`,
`realm_launch`) is independently
petitionable — including the two layout verbs, which is the whole point of
`layout_focus` being its own bit, and including `egress`: reaching one
`host:port` is not authority over the realm's pixels, and holding every other
verb buys no packet. A deployment that
refuses `observe_cursor` in any combination cannot distinguish this rule from
the blanket unserved refusal; it is stated now because the enum entry is
frozen now.

`layout_arrange` carries a different kind of constraint, and it is **not** a
composition rule: at most one principal holds it per output, so a second
petition for it resolves [`layout_held`](#outcome) rather than reaching a
human. "Holds" counts a **pending** petition as well as a live grant — see
[the outcome](#outcome) for why. That is contention, not dependency — the verb
needs no other verb alongside it.

`realm_launch` in particular does **not** imply `observe`. Authority to start
an app is not authority to watch it, and an agent that wants both petitions
for both — separately, so the human sees both. This is worth stating because
the convenient shape (launching hands back an observable realm) is exactly the
one that would smuggle observation in behind a single prompt.

#### What no grant can purchase

Layout being grant-governed is only half the answer; the other half is what a
layout grant can never buy, however permissive the consent decision. The core
enforces these **ordering invariants** unconditionally:

- the consent surface and the trust indicator composite **above** every
  principal's content;
- the core's own hit test — never a client's claimed stacking — decides which
  surface an input event reaches;
- no arrangement may occlude, fullscreen over, or resize away the consent
  surface;
- no **agent** principal's cursor is composited into another principal's
  captured frame — the one cursor a capture may ever contain is the human
  principal's, and only for a grant holding `observe_cursor`;
- the **human's own physical input** reaches only the realm the output is
  bound to.

The fifth joined the set when `layout_focus` became servable: it is why that
verb is one act, and why no verb set separates "show a realm" from "send the
human's keys there". An agent's *injected* input is not governed by it.

**What "unconditionally" means today.** All five are now tested **as
invariants** — against a real client, over a real socket, holding every verb
the reference core serves, sweeping the whole arrangement space those verbs
can express. See [conventions § 1.4](00-conventions.md#14-scene-authority-arrangement-ordering-cursors)
for the test names and what each asserts. Two limits, stated rather than left
to be found: they are **component** tests rather than mock-free integration
gates, and invariant 4's agent→agent half stays unpurchasable *by
construction* rather than by test, because agent-to-agent observation has no
wire surface to try it through — no agent can name another's grant.

The split is deliberate: a shell gets *arrangement*, the core keeps *ordering*.
This is what lets window-management policy live outside the TCB (PRD §5.1)
without making the shell trusted. See [conventions § 1.4](00-conventions.md)
for the full statement.

### persistence

The consent persistence ladder. The full ladder is defined from day one so the
wire never changes shape when durable rungs arrive.

| entry | value | meaning |
|---|---|---|
| `once` | 0 | single-use authority |
| `while_running` | 1 | lives while the requesting principal's connection lives |
| `until_revoked` | 2 | durable until explicitly revoked (requires verified provenance; refused in version 1) |
| `always` | 3 | durable and auto-reissued (requires verified provenance; refused in version 1) |

Version 1 accepts only `once` and `while_running`. A petition for a durable
rung (`until_revoked` or `always`) resolves [`unsupported`](#outcome), because
durable authority is valid only with verified provenance — a later phase. This
enum types both `request_grant`'s `persistence` argument and
`resolved.persistence`.

### outcome

Petition-time results. Each maps to a distinct typed SDK error (or success).
All outcomes are recoverable: a denial is an answer, not a protocol violation.

| entry | value | meaning |
|---|---|---|
| `granted` | 0 | authority active; the `resolved` event carries the effective verbs, rung, and expiry |
| `denied` | 1 | the human said no |
| `timed_out` | 2 | the consent prompt expired unanswered; petitioning again later is legal |
| `unavailable` | 3 | the realm was unknown, vacant, or closed while the petition was pending |
| `unsupported` | 4 | well-formed but refused by policy: durable rung without provenance, reserved flag set, unserved resource prefix, or a defined verb this deployment does not serve (an *out-of-range* verb bit is instead fatal `invalid_argument`) |
| `busy` | 5 | the pending-petition admission cap for this verified identity (across all of its connections) was reached |
| `layout_held` | 6 | `layout_arrange` is already spoken for on this output — by a live grant carrying it, or by another petition still pending for it |

This enum types `resolved.outcome`.

**Why `layout_held` is its own entry.** D-018(4) fixes at most one
`layout_arrange` holder per output and refuses to arbitrate between two, which
would be the window-management policy PRD §5.1 exiles from the core. The
answer a second holder receives had to be *some* entry, and it could not be
`busy`: `busy` means the consent-fatigue valve tripped, and widening its
meaning is a wire-semantics change the [growth rules](00-conventions.md) forbid.

It is decided **at admission**, not at use, because contention is about who
*holds* the authority rather than about one use of it — a use-time answer
would let two principals both believe they hold arrangement and discover
otherwise one request at a time. It therefore never reaches a prompt and costs
the human nothing.

**A pending petition holds the slot too**, and that is a design choice rather
than an implementation accident. If only *live grants* counted, two petitions
for `layout_arrange` could both be admitted while both waited, a human could
approve both, and the session would end up with exactly the two holders this
rule exists to make unreachable — with no answer left to give the second one,
because it has already been granted. So the slot is taken from admission, and
it is released when the pending petition resolves to anything other than
`granted`. The cost is stated plainly: a principal whose petition is sitting
in front of a human can lock out a second principal for as long as that human
takes to answer, and the second principal is told `layout_held` rather than
`busy`, so it can tell contention from fatigue.

Retrying once the holder's grant expires, is revoked, or its connection ends —
or once the pending petition resolves non-`granted` — is legal, and this
outcome is the **only** thing the core says about arbitration.

### refusal

Use-time refusal codes, emitted by the enforcement chokepoint on every refused
*use* of a grant — capture, actuation, launch, the layout verbs, **designation**
and egress alike. **Six** classes. That list is a **closed enumeration** of the
use classes this enum answers for, and the IDL records that it has gone stale
three times, each lapse worse than the last: it read "capture, actuation and
launch" after the layout verbs had already earned two paragraphs below; it
still read so once egress had a paragraph of its own too; and the rewrite that
closed both of those **declared the list closed in the same sentence that
omitted designation**, while
[`vitrin_powerbox.request_file`](13-vitrin_powerbox.md#request_file) two
thousand lines further down the IDL names
`vitrin_grant.refused(designate_file, …)` as one of its three terminals. A gap
is a gap; an incomplete list calling itself closed is a contradiction against
its own document. The mechanism was `designate_file` (P2.6.5) and `egress`
(P2.7.2) landing on two branches at once, each able to see only its own new
class — the shape to expect from the *next* append too, not an accident
peculiar to these two. It groups by **facet shape**, not by verb — the two
actuators are one class, the two layout interfaces are one, the powerbox's two
asks are one — so a verb appended to this protocol either joins a class named
here or adds one. Each code maps to a distinct typed SDK exception
(NotGranted, GrantExpired, Revoked, RateLimited, Preempted, ConsentHeld,
NoSurface, OperationFailed, AtCapacity).

| entry | value | meaning |
|---|---|---|
| `not_granted` | 0 | the grant is not (or not yet) active, or the verb is outside its effective set: use while pending, through an ungranted facet, or after any non-`granted` resolution (`denied`, `timed_out`, `unavailable`, `unsupported`, `busy`) |
| `expired` | 1 | the grant's expiry passed; checked on use and by a proactive timer |
| `revoked` | 2 | revoked by hold-Esc, panel, or policy; effective on the very next request |
| `rate_limited` | 3 | the token bucket is empty; `retry_after_ms` hints the refill |
| `preempted` | 4 | physical human input owns the target right now — **conditional for the two layout verbs**, see below |
| `consent_held` | 5 | the principal's **own** pending petition has a prompt up; that principal's actuation is refused (never delivered to the app) until the prompt closes — other principals' grants are unaffected |
| `no_surface` | 6 | the realm has no surface (its shim crashed or exited); never a stale frame |
| `internal` | 7 | server-side failure during this use (renderer, memfd, delivery) |
| `capacity` | 8 | the deployment is at its realm capacity, so no new realm can be created |

This enum types `refused.code`.

**Why `capacity` is not `internal`.** `internal` means the server tried and
something broke — a renderer, a memfd, a delivery path. A session at its realm
limit is a **policy answer**: nothing failed, the deployment declines. Folding
it into `internal` would make an agent retry a bug report and would make a
real bug indistinguishable from a configured limit in the journal.
`retry_after_ms` is 0, because the core cannot know when a realm will exit —
this is not a rate limit wearing a different code, and an agent that treats it
as one will spin.

**Not every code is reachable by every verb.** `no_surface` answers "the realm
has no live surface", which is the state `realm_launch` exists to leave, so a
launch is never refused `no_surface`. `capacity` concerns creating a realm and
so reaches only `realm_launch`. A code's absence from a verb's reachable set
is a property of the operation, never a promise the code is unused.

**`designate_file` reaches the grant-lifecycle four** (`not_granted`,
`expired`, `revoked`, `rate_limited`) **and `internal`** — and it is the one
class whose set this document does **not** yet close. Never `no_surface`, for a
reason that is neither launch's nor egress's: a designation is delivered to the
realm's *shim*, which exists from the moment the realm does, whether or not its
app has ever committed a surface. Never `capacity`. **What is not settled is
`preempted` and `consent_held`.** Both are attention-shaped, and
[`request_file`](13-vitrin_powerbox.md#request_file)/`request_dir` are the only
**uses of a grant** in this protocol that *raise a prompt of their own* (a
petition does too, but a petition is not a use), so the argument that mutes an
actuation while the human's hand is on the input does not transfer unexamined
to a request whose whole purpose is to put something in front of that same
human. P2.6.6 answers it when it builds the picker; nothing here forecloses
either answer, and a server must not read the silence as licence to give either
code a third meaning. What *is* settled: two pickers colliding is already
answered, by [`vitrin_powerbox.refusal`](13-vitrin_powerbox.md#refusal)'s
`busy` on that interface's own event. Only the human's hand is undecided.

**`egress` reaches the grant-lifecycle four** (`not_granted`, `expired`,
`revoked`, `rate_limited`) **and `internal`, and nothing else.** Never
`no_surface`, on `realm_launch`'s terms and for a reason of its own: a
connection is not made to a window, and a realm whose app has committed
nothing may still legitimately have something to say to the network — a
**normative exemption a server must implement**, since the obvious
implementation refuses every non-launch use when the realm has no live view.
Never `preempted` or `consent_held`, which are attention-shaped and an
outbound socket is neither seen by the human nor delivered into their realm.
Never `capacity`. And what the far end did instead of answering is not in this
enum at all — see
[`vitrin_egress.connect_failed`](19-vitrin_egress.md#connect_failed), which
exists because none of these codes can honestly carry it.

**What "the target" is, when a deployment serves several realms** — an
implementation note, not a wire rule. The IDL says `preempted` means "physical
human input owns **the target** right now" and deliberately does not say what
the target is; that is the server's to decide, and a client must not assume
either answer. The reference core (WS-E.1.6) judges it **per realm**: an
actuation's target is the realm its own grant names, so a human working in one
realm does not preempt an agent working in another, and a layout request's
target is the realm the human's own input currently follows, since a layout act
moves what the human is looking at rather than being delivered into a realm.
Before that it answered session-wide, which refused strictly more. Either
reading satisfies the IDL, so an agent that treats `preempted` as "yield and
retry" is correct against both — which is the only behaviour the wire actually
requires.

**The layout verbs are refused `no_surface`, `preempted` and `consent_held`**,
and each is deliberate. `no_surface`: focusing a realm with no live view would
bind the output to nothing and arranging one has no geometry to arrange — the
asymmetry with `realm_launch` is that a vacant realm is the state *launch*
exists to leave, and focus has no such excuse. `preempted` and `consent_held`:
moving the human's attention while the human's own hand is on the input, or
while that principal's consent prompt is up, is exactly the attention-shaped
hazard those two codes exist for, so they are no longer actuation-only. They
still refuse neither a capture (observation is concurrent by design) nor a
launch.

**`preempted` is conditional for the layout verbs, and only for them.** A server
MAY define an attention signal by which the human states that their *own* hand is
off the input — [`vitrin_principal.attention`](02-vitrin_principal.md#attention),
version 2 — and while that signal is live for a principal, that principal's
[`layout_focus`](17-vitrin_layout_focus.md) / [`layout_arrange`](18-vitrin_layout_arrange.md)
use is not refused `preempted`. It exists because a human at a shell running
inside a realm otherwise cannot ask it to change the layout: the keystroke that
sends the request is the physical input that forbids it. The exemption is
deliberately narrow in three ways a client should not misread:

- **`consent_held` is never conditional in the same way.** A prompt up means the
  human is answering a security question, and no signal about their hand lifts it.
- **The two actuation verbs are never exempted at all.** A human's hand still
  mutes an agent actuating into the realm the hand is in, and no human gesture can
  lift that.
- **The signal delegates nothing.** Everything the client may do afterwards, it
  could already do; what changes is one refusal, once.

The consequence a client must accept is that **two identical layout requests can
be answered differently by server state the client cannot read**. An agent
reading only its own journal can no longer reconstruct why one `focus` landed
and an identical one did not. `preempted` used to mean one thing; it now means
"the human's hand owns the target, and the human has not said otherwise".

**`capacity` is a cross-principal side channel, and the only one here.** Every
other code in this table answers from the asking principal's *own* grant:
whether it resolved, whether it expired, whether it was revoked, whether *its*
bucket is empty, whether *its* realm has a surface. `capacity` answers from
**deployment-wide** state. So a principal holding one launch grant can call
[`launch`](16-vitrin_launcher.md#launch) on a timer and watch the answer flip
between `capacity` and something else — learning that *some* other principal
created or exited a realm. It learns no identity, no template, and no count;
the leak is one bit, at whatever rate its `max_event_rate` allows.

This is inherent to answering the question at all — "the deployment is full"
is not a fact that can be scoped to one principal — so it is stated rather
than fixed. Three consequences worth being explicit about:

- **No attenuation removes it.** It is not a separate verb, so a narrower
  launch grant still carries it; the only way to withhold it is to withhold
  `realm_launch`.
- **The rate ceiling is the only bound.** `max_event_rate` bounds the polling
  frequency, hence the channel's bandwidth. It is not a fix.
- **A deployment that cannot afford it must not serve the verb.** The
  reference core serves `realm_launch` (WS-E.1.1), so the channel is
  reachable there rather than hypothetical, bounded by nothing but
  `max_event_rate`; this paragraph was written while the enum entry was
  being frozen, and is now what a deployment declining to serve the verb
  is declining on.

## Flows

Direction key: **A→C** agent→core, **C→A** core→agent. These are the
[canonical scenarios](00-conventions.md) that touch `vitrin_grant`; steps not
involving this interface are elided. Facet objects (`view`, `pointer`, `text`)
are co-minted by `request_grant`, so no separate getter step appears.

### 1. Grant success (walking skeleton)

```
1. A→C  vitrin_realm.request_grant(grant, consent, view, pointer, text,
                                   resource=null, verbs=observe, …)
2. C→A  vitrin_consent.state(closed)          [auto-approve; loudly logged]
3. C→A  vitrin_grant.resolved(granted, verbs=observe,
                              persistence=while_running, expiry_ms)
4. A→C  vitrin_view.capture_frame()
5. C→A  vitrin_view.frame_ready(fd, format=xrgb8888, …)
```

The grant confers `observe` only after step 3; a `capture_frame` sent before
`resolved` would be refused `not_granted`.

### 2. Consent approve, actuate, capture (M1.4 demo)

```
1. A→C  vitrin_realm.request_grant(grant, consent, view, pointer, text,
                                   verbs=observe|actuate_pointer|actuate_text, …)
2. C→A  vitrin_consent.state(shown)           [human sees the prompt]
3. C→A  vitrin_consent.state(closed)
4. C→A  vitrin_grant.resolved(granted, verbs=observe|actuate_pointer|actuate_text,
                              persistence=while_running, expiry_ms)
5. A→C  vitrin_view.capture_frame() → C→A frame_ready(…)
6. A→C  vitrin_actuator_pointer.move / button …   (fire-and-forget)
7. A→C  vitrin_actuator_text.type("http://…/\n")   (fire-and-forget)
8. A→C  vitrin_handshake.sync(cookie) → C→A done(cookie)   [flush refusals]
9. A→C  vitrin_view.capture_frame() → C→A frame_ready(…)   [assert change]
```

### 3. Denial and timeout

```
Denial:   … prompt shown …
          C→A  vitrin_consent.state(closed)
          C→A  vitrin_grant.resolved(denied, 0, once, 0)

Timeout:  … prompt shown, no human action before the deadline …
          C→A  vitrin_consent.state(closed)
          C→A  vitrin_grant.resolved(timed_out, 0, once, 0)
```

Both terminate the SDK's `await`, one raising GrantDenied and the other
ConsentTimeout. Petitioning again after a timeout is legal.

### 4. Revocation mid-loop (hold-Esc)

```
1. [agent in observe→actuate loop under an active grant]
2. [human holds Esc; core revokes the grant → the grant goes dead]
3. A→C  vitrin_actuator_pointer.move(x, y)
4. C→A  vitrin_grant.refused(actuate_pointer, revoked, 0)
5. A→C  vitrin_view.capture_frame()
6. C→A  vitrin_grant.refused(observe, revoked, 0)
```

Both the actuation path and the capture path fail with `revoked` — one code,
two verbs, one chokepoint. With `sync` the agent observes the refusal within
one round trip.

### 5. Expiry

```
1. [grant issued with a bounded expiry; the deadline passes]
2. A→C  vitrin_view.capture_frame()
3. C→A  vitrin_grant.refused(observe, expired, 0)
4. A→C  vitrin_actuator_text.type("late")
5. C→A  vitrin_grant.refused(actuate_text, expired, 0)
```

### 6. Rate-limit hit

```
1. A→C  vitrin_view.capture_frame() ×N within the window
2. C→A  vitrin_view.frame_ready(…) for those the bucket admits
3. A→C  vitrin_view.capture_frame() (over ceiling)
4. C→A  vitrin_grant.refused(observe, rate_limited, retry_after_ms>0)
```

Capture refusals are the terminal of a reply-bearing request and are **never
coalesced** — one `refused` per over-ceiling capture. An actuation flood over
the ceiling produces `refused(verb, rate_limited)` that **MAY be coalesced** to
one per bucket-refill window.

## Growth

The following seams are named in the XML description as purely additive
version-2+ extensions. Each is a new message or enum entry — never a change to
a version-1 signature, whose immutability the [versioning
rules](00-conventions.md) guarantee.

- **`release` destructor.** A request to withdraw a pending petition or
  relinquish authority early, carrying the tombstone rule that clients discard
  events referencing released ids. Version 1 has no destructors; the
  server-side consent timeout (`resolved(timed_out)`) already bounds the
  lingering-prompt harm, so `release` is deferred.
- **`revoked` push event.** An asynchronous event announcing revocation before
  the next use. It is a *different* event from `resolved`, which still fires
  exactly once ever; `refused(revoked)` remains the enforcement-bearing signal,
  so there is no status double-fire.
- **Attenuation.** A request minting narrower child grants from an existing
  grant.
- **Restore tokens.** Durable grant persistence (the `until_revoked` and
  `always` rungs, already present in the [`persistence`](#persistence) enum)
  becomes usable once provenance verification lands.
- **Epoch-staleness refusal sibling.** A new event carrying the current epoch
  for compare-and-swap actuation, because `retry_after_ms` cannot express an
  epoch. It pairs with clamped-coordinate stale-observation detection.
- **New verbs.** Appended [`verb`](#verb) bits (for example key actuation,
  credential presentation, subtree reads) extend the bitfield without touching
  existing bits. Bit *values* come from the repo-wide allocation registry, not
  from the next unused-looking power of two — see [the gap between 0x40 and
  0x200](#the-gap-between-0x40-and-0x200-is-allocation-not-free-space).
- **Layout facet mints — landed at version 2.**
  [`get_layout_focus`](#get_layout_focus) and
  [`get_layout_arrange`](#get_layout_arrange), minting
  [`vitrin_layout_focus`](17-vitrin_layout_focus.md) and
  [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md). **Two** mints, where
  this row previously anticipated one `get_layout`: a facet interface declares
  exactly one grant verb, so a combined facet could name only one of the pair
  and D-018(3) requires them independently attenuable. The correction is
  recorded here rather than made silently, because this row was the record.
- **Launch facet mint — landed at version 2.**
  [`get_launcher`](#get_launcher) is the first facet minted this way rather
  than co-minted, and it is the worked example the layout mint follows: a
  structural mint on the grant, an inert facet, and a verb
  (`realm_launch`) that was refused `unsupported` until a deployment served it
  — the reference core did, at WS-E.1.1. The staging outlives its example: a
  deployment that does not serve a defined verb still answers `unsupported`
  rather than killing the connection, which is the whole of what defining a bit
  ahead of serving it buys. The row stays here because *how it arrived* is what
  the next seam copies.
- **Powerbox facet mint — landed at version 2.**
  [`get_powerbox`](#get_powerbox), minting
  [`vitrin_powerbox`](13-vitrin_powerbox.md), through which
  [`designate_file`](#verb) (64) is exercised. It follows the launcher's shape
  exactly and adds two things to the pattern that the earlier rows did not
  need. First, **two newly *defined* resource prefixes** — `file:` and `dir:`
  in [`request_grant`](03-vitrin_realm.md#request_grant)'s type-prefixed
  vocabulary, defined and **not** served: they resolve `unsupported` in every
  deployment today, exactly as the verb they select for does. That is why they
  break no existing client — an unserved prefix already resolves `unsupported`
  recoverably. Second, a **limitation that does not go away when the verb is
  served**: a delivered file descriptor is kernel authority the core cannot
  recall, so revocation stops future designations and kills the grant row
  while every descriptor already handed over keeps working until its realm
  dies. Stated here as well as on the facet page
  because this list is where a later seam looks for what a shape costs.
- **Egress facet mint — landed at version 2, one step behind its verb bit, and
  at opcode 4 rather than the opcode 3 it was drafted at.** `get_powerbox`
  shipped first from a parallel workstream and keeps 3; opcodes follow
  declaration order and are immutable once shipped, so the unshipped mint
  moved. That is the same registry discipline the verb bits get, applied to
  the one address space that had no registry.
  P2.7.2's first half appended the [`egress`](#verb) bit (128) and the
  [`net:` selector grammar](#the-net-resource-prefix) and **no message at
  all** — the first time in this document's history that vocabulary went on
  the wire with no request to exercise it. Its second half added
  [`get_egress`](#get_egress) and [`vitrin_egress`](19-vitrin_egress.md).
  **The facet is its own interface, not a request on `vitrin_powerbox`**, and
  the dialect settles that rather than taste: `interface/@verb` is **one value
  per interface**, so an interface declaring `verb="designate_file"` cannot
  also declare `verb="egress"`, and the `egress` requests on it would reach the
  enforcement chokepoint with no verb to check them against. This is the same
  rule that made the layout facet *two* interfaces (see the
  [paragraph above](#verb) and
  [`get_layout_arrange`](#get_layout_arrange)), and the same correction: an
  earlier plan row anticipated one powerbox carrying `request_file`,
  `request_dir` **and** `request_connect`, and the dialect cannot express it.
  So `vitrin_powerbox` (P2.6.5) carries the file half and `egress` gets a
  separate facet interface of its own. The correction is recorded here rather
  than made silently, because this row was the record.
  **What this row must not be read as saying:** the verb is *still* unserved
  everywhere. Its refusal reason moved from "no request exercises it" to "no
  proxy answers it", and a facet is not a mechanism. This is also the first
  row in this table where the reference core implements *nothing* of what
  landed — see [`get_egress`](#get_egress)'s implementation-status note. The
  powerbox row above is the second: `get_powerbox` is unimplemented too, and
  the two gaps are separately owned (P2.6.6 and P2.7.3).
- **A third terminal on a reply-bearing request — new at version 2, and the
  SECOND request family to need three rather than the first.**
  [`vitrin_egress.request_connect`](19-vitrin_egress.md) is
  answered by `connected`, by [`refused`](#refused), *or* by
  `connect_failed` — where `capture_frame` and `launch` each have exactly two.
  [`request_file`](13-vitrin_powerbox.md#request_file) and `request_dir` (the
  powerbox row above, P2.6.5) got there one task earlier, on the unrelated
  argument that a designation has **two answerers**; `request_connect`'s third
  arm is the far end's non-answer. Three-terminal requests are therefore a
  family of three, and this bullet said *"new to this protocol"* with only
  `request_connect` beside it until the two parallel branches were merged. It
  is the sixth carrier of that superlative;
  [§6.1](00-conventions.md#61-reply-bearing-requests) holds the record of all
  six, and this one shares no wording with the other five, which is why it
  outlived the sweep that fixed them.
  The seam it opens is general: a future reply-bearing request whose failure
  is not the server's decision adds its own terminal on its own facet rather
  than a code in [`refusal`](#refusal), which stays the chokepoint's voice and
  nothing else.

## Version history

| Version | Change |
|---|---|
| 1 | `resolved`, `refused`; no requests |
| 2 | `get_launcher` (structural mint, request opcode 0), `get_layout_focus` (opcode 1), `get_layout_arrange` (opcode 2), `get_powerbox` (opcode 3) and `get_egress` (opcode 4); `verb` gains `realm_launch` = 512, `designate_file` = 64 and, at P2.7.2, `egress` = 128 (**defined, faceted, and served by no deployment** — see [the `net:` prefix](#the-net-resource-prefix)); `outcome` gains `layout_held` = 6; `refusal` gains `capacity` = 8 |

Neither version-1 event's signature changed, and no existing enum value moved.
