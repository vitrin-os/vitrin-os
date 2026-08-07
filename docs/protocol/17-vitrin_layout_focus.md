# vitrin_layout_focus — the focus facet

**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** principal · **Messages:** 1 request · **`@verb`:** `layout_focus`

## Purpose

`vitrin_layout_focus` is the capability object through which the
[`layout_focus`](04-vitrin_grant.md#verb) verb is exercised: it makes the
granted realm the one the output shows, and the one the human's own keyboard
and pointer reach. It is minted by
[`vitrin_grant.get_layout_focus`](04-vitrin_grant.md#get_layout_focus) and is
born **inert** — it confers nothing until its grant resolves `granted` with
`layout_focus` in the *effective* verb set, every use passes the single
server-side enforcement chokepoint, and a refused use arrives recoverably as
[`vitrin_grant.refused`](04-vitrin_grant.md#refused), never as a connection
death.

This is the first authority in the protocol over **where the human is
looking**, so most of this page is about what it deliberately cannot do.

## Focus is one act, not two

Showing a realm and directing the human's own physical input to it are the
**same request** here, and there is no verb set, and no attenuation, that
separates them.

A holder that could bind the output to realm A while the human's keystrokes
continued to reach realm B — or the reverse — would be able to make a human
type into a realm they cannot see. That is focus theft in its sharpest form,
and it is the reason `layout_focus` is a separate bit from
[`layout_arrange`](18-vitrin_layout_arrange.md) in the first place: the attack
is sharp enough that the authority must be refusable on its own. Having
separated it, splitting it *again* into "show" and "route" would hand back the
exact primitive the separation exists to bound.

The coupling is therefore an **ordering rule**, not a facet policy: it sits
with the other four in [§1.4 Scene authority](00-conventions.md#14-scene-authority-arrangement-ordering-cursors)
as the fifth, and no grant purchases it away.

### This is a rule about physical input, not about actuation

An agent's injected pointer and text are addressed to the realm **its own
grant** names, are consented per realm, and carry the `emulated` origin tag
([`vitrin_shim_seat`](11-vitrin_shim_seat.md)'s B2 rule). They are not the
human's attention and are not governed by the binding. An agent actuating
inside a realm that is not on the output is doing exactly what its grant says.

## What this facet is not

It cannot raise, stack, place, resize or close anything, and there is
deliberately no request that does — see
[`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) for why those absences
are the design rather than an omission.

Neither can it **read** anything. Which realm currently holds the output is
not reported here or anywhere else on the wire: there is no `focused` event,
no query, and no reply. A holder learns the effect of its own focus request
only through a capture it holds separate `observe` authority for. Focus is a
write, not a window into the session, and a client that needs to know what is
on screen petitions for observation and is seen doing so.

## What a focus grant does not bound

The human's own input is not mediated by this grant and never yields to it.
Physical input in progress **preempts** a focus request exactly as it preempts
an actuation, and the human's revocation gesture is unreachable from here.

## Served status

Whether `layout_focus` is served is a property of a **deployment**, not of the
wire. The reference core serves it as of WS-E.1.4; a deployment that does not
refuses the petition `unsupported` at admission, and a client must read that
answer as *"not here, not now"* rather than *"not in this protocol"*. See
[§ defined but unserved](04-vitrin_grant.md#defined-but-unserved).

This interface's messages are `since="2"`, so they do not exist on a
version-1 connection at all; sending one there is fatal `invalid_opcode`. The
verb bit is *not* version-gated — a bitfield is one mask checked identically
on every negotiated version — so a version-1 client may name `layout_focus` in
a petition and is answered `unsupported`.

## Lifecycle

The facet comes into existence when a principal calls
[`get_layout_focus`](04-vitrin_grant.md#get_layout_focus) on a grant, which
allocates the client-supplied `new_id` and binds it to this interface. Minting
is always structurally successful and is **not** an authority oracle: a facet
minted from a grant that lacks `layout_focus`, or from a grant that has not
resolved yet, mints fine and refuses `not_granted` on first use.

It is grant-derived, so it follows the inert-object rule: when its grant dies
(expiry or revocation) the facet goes **inert**, and requests on it are
refused *recoverably* via [`refused`](04-vitrin_grant.md#refused), never
`invalid_object`. Neither version defines a destructor; the object lives for
the connection and its id is never reused. Minting a second facet on the same
grant is legal and confers nothing extra.

## Requests

### focus

`focus()` — **since version 2**

No arguments, and that is load-bearing rather than an economy. The realm this
grant was petitioned over **is** the realm it focuses, so a holder can only
ever move the output to a realm the human saw named on the consent prompt. A
`realm` argument would let one grant move the output to a realm nobody
consented to — the authority-over-a-name mistake
[`vitrin_realm`](03-vitrin_realm.md) refuses in the same words.

Focusing the realm that is already focused is legal and is a no-op.

**Delivery class:** **fire-and-forget**. There is no terminal event, and
refusals MAY be coalesced per the [delivery
classification](00-conventions.md#6-delivery-classification). Not
reply-bearing, deliberately: a terminal would carry no information a
subsequent capture does not already carry, and adding one would make the reply
the place a client learns which realm is focused — a read this interface does
not offer.

Focus is rate-limited by the grant's `max_event_rate` like every other use of
a grant, which is what stops a holder flickering the output between realms
faster than a human can read it.

### Failure modes

*Fatal (the connection dies).* Sending this opcode on a version-1 connection
is `invalid_opcode`, as is any opcode this interface does not define. A frame
whose declared size or fd count disagrees with the signature is `oversized` or
`fd_violation`. There is nothing else to get wrong: the request has no
arguments.

*Recoverable (an event is delivered, the connection lives).* Every one of
these arrives as `vitrin_grant.refused(layout_focus, code, retry_after_ms)`:

| code | when |
|---|---|
| `not_granted` | the grant never held `layout_focus`, or has not resolved `granted` |
| `expired` | the grant's expiry passed |
| `revoked` | the grant was revoked |
| `rate_limited` | the grant's token bucket is empty; `retry_after_ms` > 0 |
| `no_surface` | the granted realm has no live view to show |
| `preempted` | physical human input owns the target right now |
| `consent_held` | this principal's own consent prompt is up |
| `internal` | a server-side failure carrying out the binding |

`capacity` is never produced — it concerns creating a realm.

**`no_surface` applies here, and the asymmetry with `realm_launch` is
deliberate.** Focusing a realm with no live view would bind the output to
nothing: a successful-looking answer to a request that did nothing. A launch
is exempt from `no_surface` because a vacant realm is the state it exists to
*leave*; focus has no such excuse.

**`preempted` and `consent_held` apply here too**, on the same terms an
actuation meets them. Moving the human's attention while the human's own hand
is on the input, or while that principal's consent prompt is up, is the
actuation-shaped hazard those codes exist for. (The consent surface is
untouchable by any arrangement whatever these gates do — that is
[ordering invariant 3](00-conventions.md#14-scene-authority-arrangement-ordering-cursors),
enforced elsewhere and unconditionally. Both exist.)

## Growth

Documented seams, each purely additive and each arriving as a `since`-gated
sibling **request or event on this interface**, never as an argument added to
`focus` (whose signature is frozen forever):

- **a focused-realm event**, if a shell is ever given a read of the binding —
  which needs its own decision about what that read discloses to a principal
  that holds no observation authority;
- **per-output focus**, when a deployment has more than one output. Version 2
  has exactly one, and `focus` names none for that reason.

## See also

- [`vitrin_layout_arrange`](18-vitrin_layout_arrange.md) — the other half of
  layout, separately petitionable and separately attenuable
- [`vitrin_grant.get_layout_focus`](04-vitrin_grant.md#get_layout_focus) — the
  mint
- [§1.4 Scene authority](00-conventions.md#14-scene-authority-arrangement-ordering-cursors)
  — the five ordering rules no grant purchases
- [`vitrin_shim_seat`](11-vitrin_shim_seat.md) — where input actually arrives,
  and the `origin` tag that distinguishes the human's from an agent's
