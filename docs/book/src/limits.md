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

**On bare metal, a realm's app can plausibly open the real keyboard and read
every key you type — including into other realms, and including a passphrase.**
This is the sandbox gap above, pointed at the one device the whole architecture
is built to mediate. `logind` ACLs `/dev/input/event*` to the user owning the
active seat session; the confined app runs as the core's **own uid** with the
core's full filesystem view and no namespace, no seccomp filter and no Landlock
policy, so nothing stops it from opening those nodes directly. On this
project's own target machine the maintainer is additionally a member of the
`input` group, which grants that access independently of any seat — so this is
concrete rather than theoretical.

What that bypasses is not a feature but the premise: `vitrind`'s input router,
the origin tag that distinguishes a human from an agent, the per-realm routing,
the consent grab that makes a prompt unspoofable, and the lock screen are all
*downstream* of a device the app reached around. An app doing this is not
observed by the journal, is not refused `preempted`, and does not appear in any
capture.

Two bounds, and neither is a fix. **It is not reachable today**: there is no
DRM/KMS backend, and under `--nested` the host compositor is the only reader of
those devices. It becomes reachable the moment a bare-metal backend lands
(WS-E.3.2), which is why it is published here **ahead of** the code rather than
with it. And it is the same hole `crates/vitrin-core/src/spawn/isolation.rs`
already probes for and enforces nothing about — Phase-2 confinement (E2.6/E2.7)
is what closes it, by giving the realm a device namespace it cannot see those
nodes from.

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

**The DRM/KMS backend will never have a green gate behind it, and that is the
weakest evidence in this repository.** Every other claim on this page closes on
a named, mock-free test. This one cannot, and the reasons are structural rather
than budgetary. Six of them, named rather than summarised:

- **No DRM device in CI.** A GitHub runner has no display controller. Nothing
  there can set a mode, commit a frame or receive a page flip.
- **No seat in CI.** No `logind` session, no `seatd`, nothing for `libseat` to
  open a card through. The backend cannot even reach the point of failing
  usefully.
- **Not even a compile-check, yet.** An earlier draft of this page said, in the
  present tense, that a CI rung runs `cargo clippy … --features drm-backend`.
  **No such rung exists and no such feature exists** — the backend itself is
  unwritten (#218). The claim is corrected rather than deleted, because a limits
  page that quietly acquires the right words teaches nothing about how it got
  the wrong ones, and this is a page whose entire value is that it can be
  believed. When #218 lands, a compile rung is the *floor* it must bring with
  it, and even then it proves the code type-checks against the smithay API and
  nothing whatsoever about behaviour — a green tick in a repository whose
  readers are trained to trust green ticks is exactly how a compile check gets
  cited as a functional one.
- **`vkms-advisory` does not close this, and must never be read as if it did.**
  There is an advisory job that attempts `sudo modprobe vkms` and, when the
  module is available, reports what it found. **What the job actually does is
  narrower than the device's capabilities**, and the distinction matters: it
  opens the node, reads mode-setting resources, and probes GBM/EGL/GLES up to
  locking a front buffer. It deliberately never calls `drmSetMaster`, never sets
  a mode and never flips a page — so it says nothing about mode setting, atomic
  commit or the page-flip loop, whatever a vkms device is capable of in
  principle. Whether it exercises the **GBM + GLES scanout path at all** is
  *unmeasured* — vkms exposes no render node, so the GLES half would need a
  software renderer and may not import into a vkms scanout buffer. The job
  measures and publishes that answer on each run; until it has run, this
  sentence is the honest state of it. It is advisory, it never gates a PR, and
  it is never to be named without the word *advisory*.
- **One machine, one GPU, one panel, one kernel.** The evidence that this
  backend works is one person executing
  [`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md)
  on one laptop: a single Intel-driven `eDP-1` at 2560x1600, scale 1, on one
  Arch kernel and one mesa version. It says nothing about any other GPU, panel,
  kernel or mesa. The PRD names "hardware matrix" as the first item of the
  support treadmill that consumed prior alternative display servers; this closes
  none of it and must not read as if it does.
- **The runbook has been executed once**, on 2026-08-09, and it carries its own
  dated record block. It was not a clean pass: three defects came out of it, one
  of which was that the page's own first line of recovery did not exist. A runbook nobody has executed is a plan, and the wlcs
  number above is this repository's standing example of how a manual result
  ages once it is taken.
- **The session-lifecycle checklist has one rung executed and no recorded run.**
  Blanking, suspend, lid handling, deliberate-wedge recovery and returning from
  another VT are rungs `L1`–`L7` in
  [Getting out of a wedged session](recovery.md#the-hardware-checklist), and its
  record block is empty. `L4` (blank and unblank) was run once, on 2026-08-11:
  it did not pass, and the three defects it found are
  [#257](https://github.com/vitrin-os/vitrin-os/issues/257),
  [#258](https://github.com/vitrin-os/vitrin-os/issues/258) and
  [#259](https://github.com/vitrin-os/vitrin-os/issues/259) — the panel blanking
  ~1.5 s after a return from another VT, a silent unblank, and neither
  transition reaching the flight recorder. **All three fixes are code with
  component tests behind them and none has been re-observed on hardware**, which
  is what `L7` exists to close. No other rung has been executed in any form:
  suspend and lid have never been exercised on this backend by anyone, because
  the bring-up runbook's own checklist has no suspend, lid or blank step and
  never did, so those steps had to be *written* before they could be run.
  Everything this release says about idle blanking is a claim about code
  corrected once by a laptop, not a report from one.

This is a recorded decision with a scheduled closure in the sense that page's
last section means: the closure is a dated human run, not a job. The alternative
— a green check proving compilation, read as proving function — is strictly
worse, and is precisely the honesty gap this page exists to prevent.

## Model gaps

**The trusted indicator is unforgeable within one VT, and not necessarily
noticed.** There is a rigorous gate proving a client cannot counterfeit the
band. There is no evidence that a human *notices* when it is wrong — that needs
user research nobody has done. The plan explicitly adjudicated unspoofability
out of M1.4's criteria for exactly this reason. Do not cite the milestone as
evidence for the human half. The second qualifier is the VT: the band says
nothing about any screen other than the one this core is driving, which is
spelled out with the VT-switch entry below.

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
attention marker, the lock screen and the status strip) that no client can add
to.

**No client status bar is possible, and the core's `--status` strip is the
whole of the replacement.** `zwlr_layer_shell_v1` is not in the shim's global
contract, and that was measured rather than assumed: waybar connects, binds six
globals, and never maps a surface; rofi and wofi are the same class. So
`vitrind --status` draws the strip itself, in reserved rows immediately below
the trusted band, and it shows **three facts**: the focused realm's name, the
battery, and a clock. There is no tray, no notifications, no workspace
switcher, and no click targets — it is not interactive at all, because a
principal cannot receive physical input (above) and the core does not want a
fourth core-owned gesture for a status bar. Four further limits belong with it:

- **The strip is unspoofable in pixels but is not self-authenticating.** It
  always wins the composite, so a confined app cannot cover it — but an app
  *can* paint a convincing fake strip one row lower. The band above it is the
  anchor, and the rule is **"trusted content is everything above the coloured
  line"**. That is strictly weaker than the band's own guarantee: the band
  proves itself, the strip only inherits position from it. This makes the
  indicator story three rules where there was one, and a human who cannot state
  the rule cannot apply it.
- **Every app loses rows while the strip is on.** The realm view is *not* inset
  — the app is not configured smaller, its top rows are overdrawn, exactly as
  the band's 8 rows already are. Issue #215 asks for the inset and it is
  unimplemented; `--status` is off by default so no session pays for a strip it
  did not ask for.
- **The clock is UTC unless you say otherwise, and there is no DST.** The core
  carries no timezone database — a `tzfile` parser and a recurring read of
  `/usr/share/zoneinfo` is authority the TCB is not taking for a cosmetic field
  — so `--status-utc-offset +09:00` states a fixed offset and the strip always
  labels the zone it is showing. A session running across a DST boundary shows
  an hour that is wrong until the operator changes the flag.
- **The strip is a recurring filesystem read inside the TCB.** The battery
  comes from `/sys/class/power_supply`, re-read every 30 s, bounded to one fixed
  root, 16 directory entries and 16–32 bytes per attribute, with every failure —
  no battery, a desktop, a machine mid-suspend — collapsing to an **empty slot**
  rather than a guess. When Landlock over the core's own process lands, this
  becomes a rule the core must grant itself, i.e. this widens that future
  sandbox.

**A principal cannot receive physical input either, so no client has a
hotkey.** There is no `observe_input` verb and none is designed. The core owns
five physical gestures — the dead-man switch, the attention key, the two
clipboard chords, the lock chord and the screenshot key — and owns them
*precisely because* the human's off-switch, the human's attention gesture, a
cross-realm transfer, the act of locking a screen and a picture of one's own
screen must not depend on a client being alive and correct. A
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

**Every realm renders, whether or not you are looking at it — and whether or
not any agent is connected.** A hidden realm keeps receiving frame callbacks
paced by the output's composites, and keeps having its view composed. That is
not generosity: a Wayland client throttles on frame callbacks, so a realm that
stopped being paced would stop repainting and its capture would go stale —
which the protocol forbids outright (`no_surface` is documented as "never a
stale frame"). The second half is the one this page used to leave out: the
compositor also does **not** ask whether anything will read a composed view.
With no agent connected, no `--capture-dump` and no `--screenshot-dir`, a
realm that is painting still has its view composed and cached every round.
Gating that on whether a grant happens to exist would make what a capture
returns depend on when the grant appeared, and that trade was declined.

What the compositor does skip is a realm whose scene has not changed since the
last composite: the cached view is then already byte-for-byte what recomposing
would produce, so nothing about what any reader is served depends on it. That
saves the idle case and the sibling case — one realm painting no longer costs
its fifteen neighbours a composite each — and saves nothing at all for a
single realm whose app is busy painting. The rest of the cost is real and is
not traded away: on a laptop, up to sixteen apps compositing at the output's
rate with nobody watching fifteen of them, plus roughly
`2 x width x height x 4` bytes of core-side pixels per realm (~590 MiB
resident at sixteen realms on a 2560x1600 panel, measured).

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

**A human can now move text between two realms, and that is a channel with a
stated bandwidth.** Copy-paste between realms exists as of WS-E.2.1: pressing
Ctrl-Shift-Insert asks the realm you are looking at for its selection and puts
it in a single core-held slot; pressing Shift-Insert in another realm offers
that slot to *that* realm's app, which you then paste into with the app's own
paste key. Two gestures, one direction each, and **no client can trigger,
force or observe either** — the core asks, and there is no message by which an
app or a shim can announce a copy.

Read the rest as a bound rather than as an absence, because that is what it is:

- `text/plain;charset=utf-8` only. No images, no rich text, no file paths.
- **60 KiB** at a time (61 440 bytes), measured against real file sizes and
  against the wire's own 64 KiB frame ceiling — a larger cap is not expressible
  without handing the trusted core a shim-controlled memory mapping.
- The slot is cleared after two minutes, when the realm its contents came from
  dies, and whenever the dead-man switch fires. Nothing tells you it was
  cleared; a gesture that finds an empty slot simply does nothing.
- **Two colluding realms can therefore move ~60 KiB per human gesture pair.**
  Qubes accepts the same bound. The honest statement is the bound, never
  "there is no channel"; the PRD's threat-model row was edited rather than left
  standing.

**The trusted core now stores bytes an application authored.** Nothing else in
it does — it holds client *pixels* it never interprets and typed values it
validated itself. A password copied from a manager transits `vitrind` and rests
in that slot until one of the three clearing rules fires. The cap, the
one-type allow-list, the digest-only journaling (the flight recorder records a
length and a BLAKE3 digest, never content) and the three clears bound it; none
removes it. This was decided deliberately, with that cost stated, and it is the
first time this project has made that trade.

**Two more keys are taken from every app.** Ctrl-Shift-Insert and Shift-Insert
are consumed by the core in every realm, with no pass-through and no way to ask
for one. Shift-Insert is the historical X11 primary-paste chord, so an app that
binds it loses it. `--clipboard-key` moves both to another key, which is not a
remedy so much as a different loss.

**The lock screen does not lock out agents, and this is the single most
surprising thing on this page.** As of WS-E.2.2 there is a lock screen, and
**three** things raise it: Ctrl-Alt-Delete, `--lock-idle SECS` of no physical
input, and — only if you asked for it — a VT switch away under
`--lock-on-seat-change immediate`, which is described with the other two seat
policies further down this page and which a session that never names that flag
can never produce. Whichever one raised it, it covers the output with a
core-drawn card and takes **every physical event** away from every realm until
you type your passphrase. What it does not do is touch a grant. An agent
holding `observe` **keeps capturing the realm across a lock**, frame for frame,
exactly as if you were sitting there; one holding `actuate_pointer` or
`actuate_text` keeps acting.

That is a decision, not a gap somebody forgot to close. Observation is
concurrent by design in the wire protocol (`vitrin_view`), so `preempted` and
`consent_held` never refuse a capture, and a lock takes away **your** input, not
an agent's authority. Three alternatives were considered and rejected: a new
refusal code (a v0 wire-semantic change, which belongs to the protocol track);
blanking the realm view so agents receive black frames (a lie by omission — the
agent is never told why it sees black); and routing the lock through the
enforcement chokepoint as a synthetic human principal (which invents a wire
principal the identity layer does not have).

The instrument for "stop everything" is unchanged and still works while locked:
**hold the dead-man chord**, which revokes every grant in the session, denies
every pending petition and clears the clipboard slot. The lock card says all of
this on the card itself, in the same words, because a human who locks a screen
and walks away should not learn it from a documentation page.

**On bare metal, that same continued observation holds across a VT switch too —
with one difference that is worse and is not softened here.** The subject here
is the agent's access, *not* the paragraph immediately above: your dead-man
chord is a **physical** gesture, and physical input is suspended for the whole
time you are on another VT, so the emergency stop that still works while locked
does **not** work while you are switched away. From another VT your only stop is
a shell and a signal. An agent holding `observe` keeps being served
its realm's capture while you are on another VT — the capture is composed from
the realm's scene, which a VT switch does not touch, so the request keeps
succeeding — and one holding `actuate_pointer` or `actuate_text` keeps acting on
the app. Keeping both is right for the reason above: the grant is the authority,
not your gaze. **But the pixels stop changing.** While the seat holds the
devices no page flip lands, so no `frame_done` is issued, so every app that
paces on it stops painting; the agent is served the same frame it had when you
switched away, with no staleness signal and no refusal. That is *not* what
happens across a lock, where the frame clock keeps running — so read the
sentence above as true of a lock and this one as true of a VT switch. The net
effect is the uncomfortable one: **across a VT switch an agent can still act and
cannot see the consequences, and neither can you.** Giving realms a software
frame cadence while the seat is away would fix the observation half; it is not
scheduled, and the human's half needs a mission-control shell (E3), which is
also not in this workstream.

One more thing that gets quietly wider while you are away: `preempted` — the
refusal that stops an agent acting where your hands are — is judged against
recent *physical* input, and physical input is suspended for the whole switch.
So the moment you leave is the moment agent actuation stops being refused
`preempted`. That is correct (you really are absent) and it means agent
authority is at its widest exactly when your view of it is at its narrowest.

**In nested mode the lock screen locks a window, not a session.** `vitrind`
runs as a client of your real compositor, which is above it and owns the actual
session: anyone can alt-tab away from the locked window, and the host's own
screen lock is still the thing protecting the machine. Treat the nested lock as
what it is — a privacy cover over the realms `vitrind` is showing — and not as
an authentication boundary for the seat.

**`vitrind` never inhibits VT switching, and on bare metal it has to
*implement* `Ctrl-Alt-F<n>` for it to work at all.** On the nested backend the
chord is the host compositor's business and outside this project's reach. On
bare metal it is `vitrind`'s, and there is no third option: **once a process
holds the display, the kernel stops handling that chord**, so a display server
that does not implement it is one you cannot leave.

An earlier release of this page said the opposite — that the chord was left
alone on purpose, because a display server that traps you on its own VT is one
you cannot leave when it wedges. **The reasoning was right and the effect was
its own opposite.** That code was run on a real panel for the first time on
2026-08-09 and the human could not leave: `Ctrl-Alt-F1` and `Ctrl-Alt-F2` did
nothing, and the session ended only because it was killed from another shell.
The words are being changed, not quietly swapped: the decision that was written
to keep the escape hatch open is what welded it shut.

So, in this release:

- **`Ctrl-Alt-F1` … `Ctrl-Alt-F12` switch virtual terminal**, exactly as they do
  under every other Linux compositor. `vitrind` never switches your VT for any
  other reason — not on a timer, not on an agent's request, not to bring you
  back. Only your own hands can move it, and no principal on the wire can,
  whatever it holds.
- **They work while the screen is locked.** That is deliberate, and it is
  argued rather than assumed: being trapped is worst in the state where you
  cannot dismiss what is in front of you. It is **never a way past the lock** —
  the lock stays up and still wants your passphrase when you come back. What
  someone standing at your locked laptop gains by pressing it is a login prompt
  on another terminal, which they could have reached before you started
  `vitrind` or by power-cycling the machine. It is strictly less than what they
  can already do: the dead-man chord revokes every grant in your session and
  fires through the lock on purpose.
- **Twelve keys are taken from every confined app on bare metal.** The chords
  are consumed in every realm and never delivered. Same as every other Linux
  compositor; stated because this project states what it takes. `f1`…`f12` are
  also no longer available to `--dead-man-chord`, `--lock-chord`,
  `--clipboard-key` or `--screenshot-chord` under `--drm`, and a command line
  that asks for one is refused at startup rather than silently rearming your
  off-switch every time you leave the terminal.
- **Know your own VT number before you start.** The startup banner logs it.
  A human who can leave and cannot come back is only half rescued.
- **If a switch fails, you will see it on the panel**, in a red band that names
  what happened and what you can still do. A log line is worth nothing to
  somebody who cannot leave the screen to read it. If that band ever appears,
  the session is trapped: record it and treat it as serious.

**None of the five bullets above has been confirmed on hardware.** They are a
design with unit tests behind it. No test in this project can take DRM master
or a seat, so whether `Ctrl-Alt-F2` really puts a tty on your panel is knowable
only by running the bring-up runbook, and that has not been done since this
changed.

**The trusted band covers this screen and this process, and nothing else.** The
coloured strip means one thing: everything above the line on *this* display was
drawn by the `vitrind` you started, not by an app running inside it. It makes no
claim about any other virtual terminal. While you are away, `vitrind` cannot see
that screen, cannot draw on it, and cannot tell you afterwards what was on it.

What *is* checkable when you come back is the colour. It is minted once per
`vitrind` process and **never rotated** — not on a VT switch, not on resume, not
for any reason — so **the same colour means the same core, and a different
colour means the core you left is not the core you came back to.** Treat
everything on screen as untrusted until you know why it restarted.
**Photograph the band before you switch and compare side by side on return**,
rather than trusting your memory of an arbitrary colour: this page already says
nobody has evidence a human reliably notices a wrong band, and a memory test is
not a check.

**Your screen now goes dark on its own — and a dark screen is not a locked
session.** With `--blank-idle SECS` on bare metal, `vitrind` turns the panel off
after that long with no physical input from you. **The session behind it stays
unlocked.** Any physical input brings it back, and what comes back is your
session exactly as you left it — not a passphrase prompt.

Say the consequence rather than the feature: **anyone who walks up to your dark
laptop and touches a key is inside your session.** That is worse than what most
desktops do, where the screen blanking and the screen locking are the same
timer. Here they are deliberately not coupled — locking is `Ctrl-Alt-Delete`, or
`--lock-idle SECS`, and it is a separate thing you have to ask for. If you want
a dark screen to mean a locked screen, set `--lock-idle` to a value you are
comfortable with; nothing in the blank will do it for you, and the two timers do
not know about each other beyond sharing the answer to *"when did a human last
touch this?"*.

Two smaller things that come with it. **Idle inhibition is not yet served**, so
full-screen video will blank the screen — the client-side protocol for saying
"don't blank, I'm playing a film" (`zwp_idle_inhibit_manager_v1`) needs both a
new shim global and a new wire verb, and neither exists. That is a *not yet*
with a named condition for changing, not a refusal. And **`--blank-idle` is
refused on `--nested`**: a `vitrind` running inside your real compositor's
window would be painting a black rectangle and calling it a dark screen, which
asserts something about a display it does not own.

**A dark screen is not evidence that nothing is watching, either — and this is
the same decision as the lock screen, not a second accident.** An agent holding
`observe` **keeps capturing the realm while your panel is off**, exactly as it
does across a lock. Read the lock-screen entry above and read this as the same
sentence: a lock, and now a blank, takes away **your** input and **your** view;
neither touches an agent's authority, because the grant is what confers it and
your gaze never did. The instrument for "stop everything" is unchanged and still
works in the dark: **hold the dead-man chord**. It fires through a blank for a
structural reason rather than a lucky one — the switch watches an input tap no
gate can suppress, so the very press that wakes your screen is also the first
press of the hold.

**But it is worse than that, and the honest version is uncomfortable: a blank
stops every realm's frame clock.** With the display powered off there are no
vertical blanks, so nothing tells the compositor a frame landed, so no
application is given permission to draw the next one. Every app in the session
stops painting for as long as the screen is dark. An agent holding `observe`
therefore does not "keep seeing" — **it is served the frame from just before the
blank, over and over, indefinitely, with no signal that the picture is stale and
no refusal to tell it something is wrong.** This is the same effect this page
already describes for a VT switch, where the project's own notes call it *"worse
than a stall"*; the difference is that a VT switch is something you do
deliberately and a blank happens on a timer. So on a `--blank-idle` session,
**an agent can still act and cannot see the consequences, on a schedule, without
anybody choosing it.** The fix — giving realms a software frame cadence while
the display is off — is named and is not scheduled. If you are running agents
unattended, this is the entry to read twice, and the shortest honest advice is
that a blank timeout and unattended agent work do not currently mix.

**And `vitrind` still cannot see a panel that went dark for any other reason.**
It knows about the darkness it caused itself, and that is all: your monitor's own
power button, and the backlight controls your laptop exposes outside the display
server, remain beyond it. So a consent card can still in principle be raised —
and recorded as shown to you — while you are looking at a screen something *else*
turned off. What `vitrind` does hold back is a prompt while its own blank is up,
and a prompt while the *seat* is taken away from it, which is what happens when
you switch to another VT and is the common case by a wide margin. **An earlier
release of this page said `vitrind` "never turns your screen off and has no way
to tell that something else did".** The first half stopped being true with this
release and the second half never covered the case it is now narrowed to; the
sentence is corrected here rather than quietly replaced, because a limits page
that acquires the right words without saying how it had the wrong ones is a page
you cannot check.

**None of the blanking behaviour above has been confirmed on hardware.** It is a
design with unit tests behind it. No test in this project can take DRM master, a
seat, an ACPI event or a backlight, so whether the panel actually goes dark,
whether it actually comes back, and whether a suspend or a lid close leaves you
with a working screen are all knowable only by a human running
[the recovery runbook's checklist](recovery.md#the-hardware-checklist) on the one
machine that has the hardware. **Suspend and lid handling have never been
exercised on this backend by anyone, in any form.** Until those numbers exist and
are dated, treat every sentence in this section as a prediction about code rather
than a report about a laptop.

**That check runs in one direction only, and you should know which.** A
*different* colour on return is a sound alarm: the core you left is not the core
you came back to. A *matching* colour is **not** proof that it is. Anyone who
photographed your band — the very exposure the next paragraph describes — can
reproduce it exactly, so photograph-and-compare catches a restarted or
substituted core and does not catch a patient one. It is worth doing because the
first case is the common one, not because it closes the second.

The cost of never rotating is real and is the price of the property. A colour
observed once — a camera pointed at the panel, which is newly plausible when the
panel is physically in the room — is observed for the whole session, and there
is no rotation path to reach for. Rotation was refused because it would destroy
what it appears to protect: a human who cannot tell a legitimate change from a
forgery has no check left. What still holds is that **a forged card gets no
input grab**, so a replica cannot mint a grant; the harm is deception, not
authority. And the fix for a compromised colour is ending the session — which
means leaving `vitrind`'s VT and killing the process. **The dead-man chord is
not that fix**: it revokes every grant and denies every petition, which is the
right instrument for "stop everything" and does nothing for the trust colour,
because the process and its colour keep running. Note too that the chord is a
*physical* gesture and physical input is suspended for the whole time you are on
another VT, so from there your only stop is a shell and a signal.

**By default a VT switch does not raise the lock screen, and a consent prompt
raised while you are away is not recorded as shown.** Switching away is not
treated as walking away: it costs no passphrase, because making the escape hatch
expensive to come back from would erode the reason it is open. **The idle timer
is also stopped while you are away**, and the countdown restarts when you come
back — so with `--lock-idle` a switch away does not lock the session either,
however long it lasts. **The cost of that is plain: a session you switched away
from eight hours ago is unlocked when you switch back to it.**

That is the default and it is unchanged, but it is no longer the only behaviour
available: `--lock-on-seat-change immediate|idle|never` picks one, on `--drm`
only, and `never` is what you get if you say nothing.

| Policy | What leaving does |
|---|---|
| `immediate` | The lock goes up as you leave, so coming back always costs a passphrase (or an Enter, with no `--lock-passphrase-file`). |
| `idle` | The idle countdown keeps running across the absence, so a long switch-away comes back to a locked screen and a short one does not. Needs `--lock-idle`; with no countdown there is nothing to keep running. |
| `never` | **The default, described above.** The countdown freezes for the absence and restarts when you return. |

Two things no policy changes. **A lock already up is untouched** — a VT switch is
never a way past a lock screen, under any of the three. And **none of them
suspends an agent**: a locked screen does not stop observation or actuation
(above), so `immediate` buys you a passphrase prompt and not a pause. What
`vitrind` will not do is put a consent prompt on a screen it does not own: a
petition that arrives while you are on another VT stays pending, is never
journalled as shown, and times out on the ordinary sweep, which reaches the
agent as a refusal. You will experience that as the system being obstructive.
It is the fail-closed answer, and the flight recorder carries the reason
(`petition_resolved{timed_out}` with no `consent_transition{shown}` before it).

**Without `--lock-passphrase-file` the lock is a privacy screen, and it says
so.** Enter dismisses it, with no authentication of any kind. The passphrase
path exists (Argon2id, one digest per session, one journal entry per attempt
including the failures) and it is refused at startup with `--headless`, for a
reason worth stating plainly: **a headless core holds no keymap and has no
keyboard**. Letters and digits reach a nested core only because the host
compositor interprets the layout; with no host and no device there is nothing
to type with at all, so a passphrase would be unenterable and a session that
came up that way would be locked out rather than locked.

**That sentence used to say "the core holds no keymap", full stop, and it was
about to stop being true.** WS-E.3.1
([D-028](https://github.com/vitrin-os/vitrin-os/issues/217)) puts an xkb keymap
inside the core for the bare-metal backend — behind an off-by-default build
feature, so a nested or headless `vitrind` links no `libxkbcommon` at all and
this paragraph still describes it exactly. Two things follow that are worth
knowing before that backend exists. The keymap is a **pre-compiled file an
operator points the core at**, never a layout name: libxkbcommon's name
resolution searches `~/.config/xkb` before the system path, and a realm's app
runs as the core's own uid, so a name-resolved keymap would be an app-writable
file the trusted core parses. And the core will link 383 KB of C it does not
link today, which is a real increase in the trusted computing base, stated here
rather than in a changelog.

**Turkish, and every other layout whose letters are not Latin-1, is where this
gets sharp.** The lock passphrase reads a keysym as a codepoint, and
libxkbcommon reports `ğ ş ı İ` as *legacy* keysyms whose number is not their
codepoint — while `ö ç ü` are Latin-1 and are. The core normalises both into
one convention so all of them type, but the failure it is avoiding is worth
naming: some of your letters working and some of them silently vanishing looks
like a typo, not a bug, and it would be discovered at a lock screen.

**A fourth chord is now taken from every app, and it constrains the other
three.** Ctrl-Alt-Delete is consumed in every realm. It also means
`--dead-man-chord delete` is refused at startup on an otherwise default command
line: the dead-man switch detects in the router's unconditional observe tap, so
a lock chord sharing its key would arm your off-switch every single time you
locked your screen. `--lock-chord` moves it, which — as with the clipboard — is
a different loss rather than a remedy.

**A vitrin screenshot shows the realm, not what you saw — and it cannot show a
consent prompt.** As of WS-E.2.4 there is a screenshot key: with
`--screenshot-dir PATH`, Ctrl-PrintScreen writes one PNG of the focused realm's
view into that directory. No grant is involved at any point — a human
photographing their own screen is not an agent capability — and the core mints
the filename itself, so nothing a client controls reaches a path component.

What the file contains is the limit, and it is a deliberate one: **the realm's
view only.** No trusted band, no consent card, no trusted ring, no lock screen,
no status strip, no agent cursor. So the single most useful thing a screenshot
does — "send me a picture of that weird dialog" — is the thing this one cannot
do. The correct answer today is a phone camera, and that is worse than every
other desktop offers.

The reason is the trusted band, whose colour **is** this session's secret. The
confined realm runs as the core's own uid, so any file the core writes is a
file any app can read: a screenshot carrying the band would hand a forger the
one thing that distinguishes a genuine consent prompt from a painted replica,
permanently, on the first press of the key. Two softer designs were examined
and both are worse. Cropping the band's rows out does not close it — a genuine
consent card is framed in the same colour, in the middle of the output, so a
crop protects the secret only while no prompt is up, which is exactly when you
want the screenshot. Replacing every pixel equal to the secret hands the app an
**oracle**: an app paints a field of candidate colours, you take a screenshot,
and it reads back which of its own pixels were recoloured — a ~22-bit secret
falls in a handful of screenshots at 1080p.

Four more things belong with it:

- **The screenshots are readable by every app in every realm.** They are files
  written as your uid, and there is no sandbox (above, D9). The file mode is
  `600`, which keeps them from *other users* and does nothing whatsoever about
  the confined app. This page creates no new hole — that is D9 — but this
  feature creates the files.
- **A fifth chord is taken from every app.** Ctrl-PrintScreen is consumed in
  every realm. It is a *chord* rather than a bare PrintScreen deliberately, and
  that is the one cost this feature pays back: bare PrintScreen is still
  delivered, so an app that binds it keeps it. `--screenshot-chord` moves the
  gesture, and — as with the clipboard and the lock — that is a different loss
  rather than a remedy. It may not share a key with any of the other four
  gestures; startup refuses it if it does.
- **A wrong `--screenshot-dir` puts pictures of your screen somewhere you did
  not intend, and nothing below the core enforces the choice.** The directory
  is audited at startup (absolute, existing, a directory, not a symlink, not
  group- or world-writable) and then held open for the process's life, so no
  later rename or planted symlink can redirect a write. Until E2.6/E2.7 confine
  the core, that audit is the whole of the enforcement.
- **The screenshot key does not work through a lock or a consent prompt** —
  each of those consumes *all* physical input while it is up, so the chord never
  reaches the screenshot hook. The lock one is deliberate rather than
  incidental: a person standing at your locked machine must not be able to write
  the session behind it to a file.
- **Pressing it costs the compositor about 4 ms, and the encode no longer
  happens there at all.** It used to be about 70 ms: 71.7 ms in a release build
  to encode one 2560x1600 frame into a 12.3 MB PNG, synchronously on the
  event-loop thread — roughly seventeen dropped frames at 240 Hz, on every
  press. Since issue #240 the encode runs on a worker thread that owns the
  screenshot directory, and what the press pays on the compositor thread is one
  copy of the frame out of the capture cache: **4.2 ms** for the same
  2560x1600 frame, measured in the same release build (73.9 ms for the encode
  itself, unchanged, on the same run). That is one frame at 240 Hz rather than
  seventeen. The remaining cost is the copy, and it scales with pixel count the
  same way; the queue is bounded at two presses behind the one being encoded,
  and a press past that is refused and journalled (`encoder_busy`) rather than
  queued, because each job is a whole frame of memory.

  Both numbers are CPU measurements on the development machine, in a release
  build — **not** a measurement of a session driving a real panel. What a
  screenshot does to a bare-metal session's frame timing is knowable only from
  a run of `docs/drm-bringup.md`, which needs hardware CI does not have.
- **It DOES work during a dead-man hold, and that is deliberate.** An earlier
  version of this page said otherwise and was simply wrong: `DeadManHook::gate`
  consumes only its own chord's key and delivers every other, so Ctrl+Print
  reaches the screenshot hook mid-hold and a file is written. The behaviour is
  the intended one — the off-switch destroys *authority*, and a human
  photographing their own screen is not authority — but it means the dead-man
  chord is not a way to stop a screenshot you have already started, and a
  screenshot taken during a hold captures the session as it was before the
  revocation landed. Since the encode moved to a worker thread this is literal:
  a hold does not cancel an encode already accepted, and the end of the session
  waits for it — but only for five seconds. Past that the core stops waiting,
  the process exits over the top of the worker, and that last file may be
  truncated. The wait is bounded on purpose and in both halves (the outcome
  *and* the thread), because a screenshot directory on a mount that has stopped
  answering must not be able to make `SIGTERM` do nothing.

**Identities are static tokens.** Listed in `principals.toml`. The IDL is
shaped for SPIFFE/OIDC credentials; the machinery is not here yet.

**No semantic layer.** Agents work on pixels. The AccessKit/AT-SPI2 bridge,
versioned and diffable semantic trees, and epoch/CAS action semantics are
all Phase 2 — which is to say the token-hungry screenshot loop this project
criticises is still what an agent does against it today. The difference so
far is authorization, not efficiency.

**No X11 shim.** Wayland only. Per-app X11 with an embedded WM is Phase 3.
There is no X server anywhere in this stack — not in the core, not among the
globals a shim advertises, not as a process anything here ever starts — and a
realm's app is handed no `DISPLAY` at all, because `DISPLAY` and `XAUTHORITY`
are refused outright by the environment the core builds for it. So `xterm` in a
realm dies before it draws anything, and the failure this project recorded is
`Can't open display`. That is the fragment the run wrote down; the full line
`xterm` emitted was not captured, and this page does not reconstruct it.

For anyone thinking of this as a desktop, the consequence is not a footnote. On
the one machine that has been measured, `xterm`, `feh`, `xsel` and
`nvidia-settings` are X11-only, as are the X11 window manager, compositor, bar,
launcher and screen locker installed on it. None of them can run here. The
maintainer's interim is **a second session, on another virtual terminal, for
X11-only software** — so "I did not have to go back to my old compositor" is
false for that set of programs. That is a workaround he accepts, not something
this project offers or confines: the second session is another compositor with
full access to the same devices, nothing here knows it exists, and switching to
it leaves the confined world entirely. It is also one person's arrangement on
one machine, and it is not advice.

What *has* been run, with the observable each run actually checked, is
[the session app matrix](session-app-matrix.md). Read it before assuming
anything else works; it is deliberately shorter than the list of things people
expect a desktop to run.

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
