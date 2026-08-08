# Where this is honest about its limits

Phase 1 is complete. That is a statement about a defined slice closing on
named, mock-free gates — not a statement that this is ready for anything
real. This page is the whole list, in one place, so you never have to
discover an item on it yourself.

## Do not deploy this yet

**There is no sandbox.** Decision D9. No namespaces, no seccomp, no
Landlock. A realm's app runs as the core's own uid with the core's full view
of the filesystem and the network. An app that ignores `WAYLAND_DISPLAY` and
connects to a path it already knows is not stopped by anything here. The
session D-Bus remains reachable in practice.

Environment hygiene confines the well-behaved; it does not contain the
hostile. Do not run untrusted applications, or untrusted agents, against
this. Real sandboxing is Phase 2 (E2.6/E2.7, P13).

## Testing gaps

**The 24-hour fuzz soak has never been run.** `fuzz/` ships two cargo-fuzz
targets with a checked-in corpus that CI replays on every PR, plus a short
per-PR burst. The 24-hour clean run the plan asks for is a documented manual
procedure, not a scheduled job, and nobody has executed it end to end.

**wlcs conformance is advisory and mostly red.** The 2026-07-25 run:
`total=180 passed=3 failed=145 skipped=32`.

That number needs its context, and the context is not an excuse. wlcs tests
a general-purpose desktop compositor. The shim deliberately serves a narrow
surface — no touch, no full `xdg-shell` policy, no decoration protocols — so
most failures are "no such global" rather than misbehaviour, and the
excluded touch tests are excluded for a structural absence rather than an
expected failure. `shim/wlcs/README.md` separates the two categories
honestly. But it is still the real number, it has **not** been re-measured
since that date, and a partial run's `failed=` count is a floor rather than
a tally. It never gates a PR and is never built by default.

**dmabuf zero-copy is proven by an env-gated test, not by CI.** The path is
implemented and wired on the nested backend. The zero-memcpy assertion needs
a real GPU (EGL + a DRM render node) and runs only under
`VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests --
--ignored dmabuf`. CI is GPU-free and exercises the shm path exclusively.

## Model gaps

**The trusted indicator is unforgeable, not necessarily noticed.** There is
a rigorous gate proving a client cannot counterfeit the band. There is no
evidence that a human *notices* when it is wrong — that needs user research
nobody has done. The plan explicitly adjudicated unspoofability out of
M1.4's criteria for exactly this reason. Do not cite the milestone as
evidence for the human half.

**Several realms run; only one is visible.** A `realm.toml` may now declare
up to 16 realms, and each gets its own shim process, its own private runtime
tree, its own Wayland socket and — since the output binding landed — its own
scene, its own capture and its own seat state. What it does *not* get is its
own **output**: the core composites one output from one realm's scene, so with two realms
running only the realm the output is bound to is on screen. Which realm that
is **is now somebody's to choose**: a client holding the `layout.focus` grant
verb moves the output, and the human's own keyboard and pointer move with it —
one act, because showing a realm and typing into it must never come apart. An
**agent's** actuation does not follow the output at all — it follows the realm
its own grant names, so an agent works in a realm nobody is looking at.
Absent such a client the output binds to the first realm to attach, and moves
on one event nobody chooses: the bound realm's app exiting, after which the
output follows to the first realm still serving, and to no realm at all once
none is serving. Treat a multi-realm configuration as "several apps running,
one of them on screen".

**Layout is two requests, and the absences are deliberate.** A holder can
focus a realm and choose whether it fills the output or keeps its own size.
There is no `place`, no `resize`, no `raise` and no stacking — not requests
that refuse, but no requests at all, because a scene showing one unstacked
realm cannot honour them and a verb that silently does less than its name is
worse than one with no request. Do not plan a tiling shell against this yet.

**A principal cannot draw, so nothing a client builds can be on screen.**
`vitrin_view` is capture-only and there is **no principal-facing surface
interface anywhere in the IDL** — a grant can read a realm's pixels and can put
none back. So the switcher this project ships
([`examples/shell/run_shell.py`](https://github.com/vitrin-os/vitrin-os/blob/main/examples/shell/run_shell.py))
is a line-oriented program in a host terminal, and that is not a placeholder
for a graphical one: no amount of client work reaches the output. The intended
eventual shape is the shell running **as a realm**, drawing through its own
shim like any other app while holding the layout verbs through the ordinary
grant path — which needs no new protocol, but does need that realm to reach the
core socket, and that is a confinement question nobody has answered. Until
then, anything you would call a desktop shell — a bar, a launcher, an OSD, a
window-switcher overlay — cannot exist on this display server, and the
replacements are core-owned surfaces (the trusted band, the consent card, the
attention marker) that no client can add to.

**A principal cannot receive physical input either, so no client has a
hotkey.** There is no `observe_input` verb and none is designed. The core owns
exactly two physical chords — the dead-man switch and the attention key — and
owns both *precisely because* the human's off-switch and the human's
attention gesture must not depend on a client being alive and correct. A
convenience hotkey is not in that class and must not borrow that warrant, so
"Super+Tab switches windows" is not a missing feature: it would mean the core
reserving a chord on behalf of whichever client asked first, which is
window-management policy the core deliberately does not have. What follows for
a user is concrete: **every layout change starts as a line you type into a
terminal**, and the terminal has to be somewhere you can reach.

**If the shell dies, you keep the session and lose the ability to re-aim it.**
The switcher is a client (PRD §5.1, D-021(4)), so there is no core-side
fallback — that is the price of the invariant, paid rather than argued away.
Kill it and both realms keep running, their shims and apps being children of
`vitrind` rather than of the shell, and the realm it last focused **keeps
receiving your keyboard and pointer**, because the output binding is core state
and nothing revokes it when the principal that set it disappears. What you
cannot do is move it. Recovery is running the shell again, which re-petitions
from zero and raises a fresh prompt per realm — **and in a real session the
terminal you would restart it from must already be the bound realm.** If the
shell died while the output was pointed at a realm with no terminal in it,
nothing on screen can start it, and every remedy is outside Vitrin: an SSH
session from another machine, a VT switch, or restarting `vitrind`. This is a
genuine wedge; it is documented, asserted
([`tests/integration/test_shell.py`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/integration/test_shell.py)),
and not solved.

**`set_fullscreen` is a no-op whenever the output and the realm are the same
size.** The two modes differ only in whether the realm's view size tracks the
output's, so while they are equal — which is the ordinary case for a realm
spawned into an output that has not resized — switching between them changes
nothing you can see. It is honest and it is surprising; the IDL says so in as
many words.

**Captures do tell realms apart, and that is enforced rather than
incidental.** An agent's capture is of the realm its **grant names**, never
of whatever is on the output: the compositor keeps one composed frame per
realm and the chokepoint resolves the frame from the grant's realm id, on
the same line it judges that realm's liveness. A grant over a realm whose
app has died refuses `no_surface` however busy its siblings are; a grant
over a live but hidden realm returns that realm's own pixels. There was a
window — between the realm cap being raised and the output being bound —
where this was not true and a capture could carry a live sibling's pixels;
it is closed.

**Every realm renders, whether or not you are looking at it.** A hidden
realm keeps receiving frame callbacks paced by the output's composites, and
keeps having its view composed. That is not generosity: a Wayland client
throttles on frame callbacks, so a realm that stopped being paced would stop
repainting and its capture would go stale — which the protocol forbids
outright (`no_surface` is documented as "never a stale frame"). The cost is
real and is not traded away: on a laptop, up to sixteen apps compositing at
the output's rate with nobody watching fifteen of them, plus roughly
`2 x width x height x 4` bytes of core-side pixels per realm (~590 MiB
resident at sixteen realms on a 2560x1600 panel, measured). Visibility-aware
pacing would buy the power back at the cost of capture honesty, and that
trade was declined.

**The agent cursor is drawn only for the visible realm.** The core paints a
small crosshair where an agent is pointing, so a human can see that an agent
is acting. It is painted into the output, which shows one realm — so an
agent actuating inside a **hidden** realm draws no sprite, and the human
loses that signal entirely for everything happening off-screen. This
reintroduces, for hidden realms, exactly the defect the sprite was added to
close, and per-realm input routing makes it more likely to bite rather than
less: agents can now actually work in hidden realms, so there is more going on
that nothing draws. The fix is a per-realm indicator in the trusted band and
it is not built.

**A human's hand no longer stops agents in other realms — and that is a
narrowing of a blanket safety behaviour.** The core refuses an agent's
actuation `preempted` while physical human input owns the target, and "the
target" used to be the whole session: touching the keyboard suspended every
agent everywhere for half a second, whatever realm each was working in. It is
now judged **per realm**. Typing in realm A suspends agents acting on realm A
and leaves agents acting on realm B alone, which is what "several apps running
concurrently" has to mean — but if you were relying on the old breadth as a
crude session-wide "hands off while I work", you no longer have it, and no
wire event tells you so. Layout requests (`focus`, `set_fullscreen`) still
yield to a hand anywhere the human is: those move what you are looking at
rather than being delivered into a realm, so they are judged against the realm
your input is following.

**You cannot switch realms in the same half-second you typed — unless you tap
Super first.** Because layout requests yield to a hand, any physical input marks
the realm you are in as yours for 500 ms, and a `focus` or `set_fullscreen`
arriving inside that window is refused `preempted`. That is correct for the case
the rule was written for — an agent must not move the output out from under
someone mid-keystroke — but it lands hardest on the most ordinary human action
there is: type `focus editor` into a shell and press Enter, and the Enter is
itself the physical input that preempts the request the Enter just sent.

The core therefore owns a second key, **Super** (configurable to right-Super,
and to nothing else). Tapping it opens a one-second, single-use window in which
a layout request from a principal holding layout authority is not refused
`preempted`, and it sends those principals a one-bit `attention` event so they
know to send the request they had staged. The key is **consumed**: no app in any
realm ever sees it, which is also why it cannot be used as a keystroke-timing
oracle. **It delegates nothing** — everything the client does afterwards it
could already do; what you withdrew was a courtesy the core was extending to
your own typing.

What it costs you:

- **The core eats Super, everywhere.** A nested compositor, a VM viewer, or a
  remote-desktop client running in a realm loses that key with no pass-through
  and no way to ask for one. The only remedy is `--attention-chord rsuper`,
  which is not really a remedy.
- **The window is session-wide.** If two clients hold layout authority, either
  of them may consume the press — the core cannot know which one you meant, and
  choosing would be window-management policy it deliberately does not have. Your
  own switch then silently fails and the other one lands. The claim is journaled
  with the principal that took it, and the grant is revocable, and it is still a
  hole.
- **A client can ask you to press it.** A shell printing "press Super to apply"
  is doing the right thing; a malicious one printing the same string and banking
  the timing is indistinguishable from it. What bounds this is that the press
  confers no authority the client did not already hold — but a human who learns
  "press Super when the screen tells me to" has learned a habit an attacker can
  invoke.
- **`preempted` on the layout verbs is now conditional on core state you cannot
  see.** An agent reading its own journal can no longer reconstruct why one
  `focus` landed and an identical one did not.
- **Other principals lose a guarantee nobody tells them they lost.** "A human
  typing means nobody moves the output" was true for 500 ms at a time and is now
  suspendable by a gesture no wire event announces to anyone but layout holders.

While the window is open the core draws a small marker just below the trusted
band — never inside it, because the band has exactly one correct appearance and
that is the whole of its value. **A focus change that happened with no marker up
was not yours.**

**Switching realms mid-gesture releases what you were holding, into the realm
you left.** A key or pointer button you are physically holding when the output
binding moves is released to the app you are leaving, because your actual
release will be delivered to the realm you moved to. The app cannot tell that
release from a real one — it is told you let go when you did not. The
alternative is worse (a modifier latched down forever, or a wedged pointer
grab, in an app you can no longer see), and it is the same trade the core
already makes when the nested window loses host keyboard focus; the difference
is that it now happens on every switcher keypress rather than only on alt-tab.
An **agent's** held keys are not touched — its grant still reaches that realm,
so it can release them itself.

**Resizing the nested window does not resize the apps.** `vitrind --nested`
runs inside a host compositor's window, and that window can be dragged to any
size. The core announces the view size to a realm's shim exactly once, when
the shim session starts, and never re-announces it — so an app keeps its
startup size and the composite centres it, 1:1, in whatever the window has
become: background bars around it when the window grows, a centre crop when it
shrinks. Nothing is lost and no capture is wrong (a capture is composed at the
same view size), it simply looks wrong. This is not the *per-realm* resize
WS-E.1.3 declined; it is the one output changing size with nothing told about
it. `--headless` has a fixed virtual output and cannot reach this, which is
also why no CI gate can see it — `shim/docs/nested-multi-realm.md` carries it
as a manual step.

**No realm enumeration on the wire — but do not read that as unguessable.**
A client cannot ask what realms exist; `realm-0` is the one name it can know
without being told, and `vitrin_launcher.launched` hands back the ids of realms
it started itself. What that does *not* buy is secrecy of the others: instance
ids are `<template>.<n>` with a small session-global counter, so a client that
knows or guesses a template name can guess live instance ids cheaply, and
petition admission answers differently for a realm that exists than for one
that does not. So the id space is a **naming** scheme, not a capability — the
grant is what confers authority, and knowing a name gets you a petition the
human still has to approve. Treat any design that leans on an id being secret
as broken. Multi-realm *fleet* mode — a 50-realm headless box — is Phase 3 and
is a different thing again.

**Runtime launch exists, and it is a real reduction in what the core
guarantees.** `realm_launch` is served: a principal holding it over a realm
template can make the trusted core fork a process, repeatedly, for as long as
its grant lives. Until this landed, the *only* thing that could make `vitrind`
fork was startup reading a file the operator had hardened. That property is
gone. What replaces it is weaker than "impossible" and is stated as such: a
human's approval on a card naming the template's program, an expiry,
revocation, the grant's rate ceiling, a cap of 16 live realms (refused
`capacity`), and a journal entry naming the principal and grant behind every
spawn.

**And nothing bounds what a launched app then does.** A launch grant is
authority to start an *unconfined* process with the core's own uid and
filesystem view — the confinement limits below apply to it unchanged, one
authority level up. Phase-2 confinement (E2.6/E2.7) is what changes that.

**A launched realm cannot be closed, by anybody, ever.** This is the sharper
half of the point below and it is worth stating on its own: there is no wire
request that ends a realm, and nothing in the core reclaims one. Revoking the
launch grant does not close what it started; nor does closing the connection
that asked; nor does the dead-man switch, which revokes every *grant* and
leaves every *process* running. A realm ends when its own app exits, and not
otherwise. So one approved `realm_launch` grant, exercised 15 times before the
human revokes it, permanently commits every remaining slot of the 16-realm cap
— and the core-side memory behind them — for the rest of the session. The only
remedy is restarting `vitrind`. Revocation bounds *future* launches and nothing
else; read it that way when deciding whether to approve one.

**Launched realms accumulate for the life of a session.** An exited realm
keeps its row so `unavailable` keeps meaning *not ever*, so a session that
launches continuously grows a table of dead names it never frees. It costs no
process, no descriptor and no pixels — a name and a spawn config — and it is
bounded only by the grant's rate ceiling and expiry, not by a count. A
long-lived session driven by an agent launching on a timer will grow that
table without limit.

**Identities are static tokens.** Listed in `principals.toml`. The IDL is
shaped for SPIFFE/OIDC credentials; the machinery is not here yet.

**No semantic layer.** Agents work on pixels. The AccessKit/AT-SPI2 bridge,
versioned and diffable semantic trees, and epoch/CAS action semantics are
all Phase 2 — which is to say the token-hungry screenshot loop this project
criticises is still what an agent does against it today. The difference so
far is authorization, not efficiency.

**No X11 shim.** Wayland only. Per-app X11 with an embedded WM is Phase 3.

**The protocol will break.** v0 is frozen for Phase 1, not forever.

## Project gaps

**One maintainer.** Governance is a documented BDFL. Bus factor is tracked
as a first-class project risk rather than waved away; the standing
mitigations are spec-first artifacts, a design-doc-per-subsystem rule, and a
review norm against cleverness in the TCB.

**No OIN membership yet.** The project files no patents and relies on
defensive publication plus the Apache-2.0 §3 and MPL-2.0 §2.1(b) grants,
which are in force today. Joining the Open Invention Network is decided and
not yet done. None of this is a freedom-to-operate opinion.

**SPDX header coverage is not machine-checked.** There is no `reuse
lint`-style CI gate, so a new file without a header will not be caught
automatically.

## Why this page exists

From the project's own security notes: *a half-believed confinement claim is
worse than an honest gap.* Every item above is a recorded decision with a
scheduled closure, not an oversight — see
[the decision log](https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md).

If you find something true that belongs on this page and is not here, that
is a bug worth reporting, and it will be treated as one.
