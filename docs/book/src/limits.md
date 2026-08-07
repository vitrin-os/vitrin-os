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

**You cannot switch realms in the same half-second you typed, and a
keyboard-driven switcher feels this constantly.** Because layout requests
yield to a hand, any physical input marks the realm you are in as yours for
500 ms, and a `focus` or `set_fullscreen` arriving inside that window is
refused `preempted`. That is correct for the case the rule was written for —
an agent must not move the output out from under someone mid-keystroke — but
it lands hardest on the most ordinary human action there is. Type `focus
editor` into a shell and press Enter, and the Enter is itself the physical
input that preempts the request the Enter just sent. The refusal is
recoverable and the same request succeeds a moment later, but nothing in the
core retries it, and a client that shows refusals rather than hiding them
(which is the behaviour this project asks of clients) will show this one
often. There is no attention key and no core-owned "the human meant this"
signal that would distinguish a switch the human just asked for from a
switch an agent attempted while they typed; naming that gap is not filling
it.

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

**No realm enumeration on the wire, and no runtime launch.** A client
cannot ask what realms exist; `realm-0` is the one name it can know without
being told. `realm_launch` is defined on the wire and served by no
deployment, so realms come only from the configuration file the core reads
at startup. Multi-realm *fleet* mode — a 50-realm headless box — is
Phase 3 and is a different thing again.

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
