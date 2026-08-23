# vitrin_grant — capability handle and enforcement voice

**Interface version:** 2 · **Connection class:** principal · **Messages:** 4 requests *(all since 2)* + 2 events

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

Version 2 adds four requests, and none is a command either:
[`get_launcher`](#get_launcher), [`get_layout_focus`](#get_layout_focus),
[`get_layout_arrange`](#get_layout_arrange) and
[`get_powerbox`](#get_powerbox) are **structural mints** that hand back a
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

**The reference core does not implement this request yet.** `vitrind`
negotiates exactly version 2 and has no dispatch arm for this opcode, so
`get_powerbox` sent to it is answered with fatal `invalid_opcode` and the
connection dies — not the mint-then-refuse above, and not the
`resource_exhausted` cap either, since no cap is reached by a request nothing
handles. It is the first mint this IDL has declared without a core arm, it is a
conformance gap in that implementation rather than a property of the request,
and it closes in **P2.6.6** with the picker. `crates/vitrin-core/src/principal.rs`
pins it with a test that asserts the fatal answer, so the arm cannot land
without this paragraph and its two siblings (the IDL's, and
[page 13's](13-vitrin_powerbox.md#served-status)) going with it.

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
`observe`, `actuate.pointer`, `actuate.text`, `observe.cursor`,
`layout.arrange`, `layout.focus`, `designate.file`, `realm.launch`. The
spelling is fixed by the
IDL so a second implementation transcribing this enum has no name to invent.

| entry | value | served | meaning |
|---|---|---|---|
| `observe` | 0x1 | yes | capture frames of the granted resource |
| `actuate_pointer` | 0x2 | yes | inject pointer motion, buttons, and scroll |
| `actuate_text` | 0x4 | yes | inject Unicode text |
| `observe_cursor` | 0x8 | **no** — resolves `unsupported` | capture frames that include the human principal's cursor; meaningful only alongside `observe` |
| `layout_arrange` | 0x10 | yes | arrange the granted realm's view, through the [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) facet; **one holder per output** — a live grant carrying it, or a petition still pending for it — so a second petition while either exists resolves `layout_held` |
| `layout_focus` | 0x20 | yes | bind the output to the granted realm and direct the human's input there, through the [`vitrin_layout_focus`](17-vitrin_layout_focus.md) facet |
| `designate_file` | 0x40 | **no** — resolves `unsupported` everywhere | designate one file or one directory subtree to the granted realm, through the [`vitrin_powerbox`](13-vitrin_powerbox.md) facet; the human picks and what crosses is a **descriptor, never a path**. **A delivered fd cannot be recalled** — see [that page](13-vitrin_powerbox.md#revocation-cannot-recall-a-delivered-descriptor) |
| `realm_launch` | 0x200 | yes | launch the realm template this grant addresses into a new realm instance, through the [`vitrin_launcher`](16-vitrin_launcher.md) facet |

The **served** column describes the reference core. Whether a defined verb is
served is a property of a *deployment*, so a client reads `unsupported` as
"not here, not now", never as "not in this protocol".

`VALID_MASK` is therefore **639** (`0x27f`), not `0xff`. It was 575 until P2.6.5 added bit 64. The plan's registry re-pins it **once per epic**, never once per task, and [names every site that holds it](../plan/02-phase-2-semantic-epochs.md) rather than leaving them to be found — a list `cargo xtask limits-check` holds to the tree in both directions, so a pin the registry does not name is a red build. The same gate holds this sentence's own number against the generated constant, which is why no count of sites is repeated here.

This enum is the type of `request_grant`'s `verbs` argument, of
`resolved.verbs`, and of `refused.verb`. Seven verbs map one-to-one to a facet
interface and to that interface's `@verb` annotation, which drives the
scanner-generated chokepoint table; `observe_cursor` is the one that does not,
by construction. Later phases append entries (for example
key actuation, credential presentation, subtree reads) without touching
existing bits; values are immutable.

#### The gap between 0x40 and 0x200 is allocation, not free space

`realm_launch` is 512 rather than 64 because **64, 128 and 256 were already
spoken for** — allocated to verbs (`designate_file`, `egress`,
`publish_tree`) that had not landed in the IDL yet. Bits are allocated once,
repo-wide, in `docs/plan/02-phase-2-semantic-epochs.md` §5, and anything
adding a verb allocates there first, whatever document schedules the work.
**64 has since landed as `designate_file`** (P2.6.5), taken from that registry
rather than from the next unused-looking power of two; 128 (`egress`) and 256
(`publish_tree`) are still spoken for and still absent.

This matters because a verb value is **immutable once landed**: a collision is
not a rename, it is two authorities permanently sharing one bit. Reading the
IDL and taking the next unused-looking power of two is exactly how that
happens — and it nearly did here, which is why the rule is written down rather
than assumed.

A reserved-but-undefined bit is still **out of range on the wire**, so
petitioning for 128 or 256 today is fatal `invalid_argument`, not
`unsupported`. Petitioning for 64 is now `unsupported` instead — that flip,
from a killed connection to an answer, is the whole of what "defining a bit
before serving it" buys.

#### Defined but unserved

A verb may be defined on the wire ahead of being served and **refused
`unsupported`** by a deployment that does not serve it — the same posture the
[`persistence`](#persistence) ladder takes toward its durable rungs.

Five verbs were defined this way. `observe_cursor`, `layout_arrange` and
`layout_focus` were defined from day one; `realm_launch` and `designate_file`
arrived with
version 2; of those, **three are now served** by the reference core — each has
a facet interface, an enforcement arm and consent copy naming its consequence
in plain language. `layout_arrange` and `layout_focus` joined at WS-E.1.4, and
`realm_launch` at WS-E.1.1, when the core gained the spawn path, the realm cap
and the prompt line its refusal had stood for.

**Two remain.** `observe_cursor`'s reason has not moved: the
per-principal cursor *delivery* it would widen a capture with does not exist
(D-017, D-019), so serving the verb would promise something no capture
carries.

**`designate_file` is the other**, and unlike `observe_cursor` its refusal has
a scheduled end. It landed at P2.6.5 with its facet interface and nothing
else: no picker mints a descriptor (P2.6.6) and no consent copy names what
approving it costs (P2.6.8 — Q13's rule that no verb is served before a human
can be told what it means). Both must land before any deployment may answer a
petition naming it anything but `unsupported`. In the reference core that is
structural rather than a promise: `SERVED_VERB_BITS` does not list the bit,
the unserved set is *derived* from the wire mask, and admission refuses the
petition **whole** — so forgetting the rest of E2.6 produces a refusal, never
a grant nothing enforces.

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

Not every verb has a facet interface. `observe_cursor` has none by
construction: it widens what
[`vitrin_view.capture_frame`](06-vitrin_view.md) composites rather than adding
a request. Every other verb does, and the four added at version 2 all arrive
as `since`-gated mints on *this* interface, because `request_grant`'s five
`new_id` arguments are frozen forever (see [Growth](#growth)):
[`get_launcher`](#get_launcher), [`get_layout_focus`](#get_layout_focus),
[`get_layout_arrange`](#get_layout_arrange) and
[`get_powerbox`](#get_powerbox).

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
`layout_arrange`, `layout_focus`, `designate_file`, `realm_launch`) is
independently
petitionable — including the two layout verbs, which is the whole point of
`layout_focus` being its own bit. A deployment that
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
*use* of a grant — capture, actuation and launch alike. Each code maps to a
distinct typed SDK exception (NotGranted, GrantExpired, Revoked, RateLimited,
Preempted, ConsentHeld, NoSurface, OperationFailed, AtCapacity).

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

## Version history

| Version | Change |
|---|---|
| 1 | `resolved`, `refused`; no requests |
| 2 | `get_launcher` (structural mint, request opcode 0), `get_layout_focus` (opcode 1), `get_layout_arrange` (opcode 2) and `get_powerbox` (opcode 3); `verb` gains `realm_launch` = 512 and `designate_file` = 64; `outcome` gains `layout_held` = 6; `refusal` gains `capacity` = 8 |

Neither version-1 event's signature changed, and no existing enum value moved.
