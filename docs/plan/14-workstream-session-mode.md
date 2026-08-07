# Workstream E — Session mode (maintainer dogfooding)

Getting Vitrin to the point where its maintainer runs it as his own desktop,
on one laptop, instead of an existing compositor.

`WS-E` in the cross-reference syntax ([README](README.md)). Opened by
**[D-021](20-decision-log.md#d-021--session-mode-is-scheduled-as-a-maintainer-dogfooding-workstream-ws-e-and-that-is-not-the-horizon-item)**,
which is the instrument [D-007](20-decision-log.md) requires for moving an item
out of the horizon tier.

## 0. What this is, and what it is not

**This is not Phase 4's session mode, and the M4 gate is untouched.**

[04-phase-4-horizon.md](04-phase-4-horizon.md) names session mode on bare
DRM/KMS the *last* horizon item, entered only through the M4 gate. That item is
**a display server other people can run**: a hardware matrix, HDR, color
management, fractional scaling, human accessibility, IME for every user — the
support treadmill [PRD](../PRD.md) §5.3 calls *"90% of the effort for 0% of the
differentiator … what consumed prior alternative display servers."*

WS-E is **one maintainer's one laptop**. A single Intel-driven eDP output at
scale 1, Wayland-only, spartan, with the maintainer fixing it when it breaks.

The two differ by an order of magnitude of effort and by their entire audience.
Holding them apart is this document's first job, because a dogfooding success
read as a horizon claim would make the M4 gate meaningless. **No WS-E
deliverable may be cited as evidence toward M4.**

## 1. Why it is worth doing anyway

Dogfooding has already paid for itself once, measurably.

Within an hour of `vitrind` being runnable by hand from `PATH`, a real terminal
aborted the shim on a wlroots assertion (#203): `on_new_deco` answered an
xdg-decoration request before the surface's initial commit, killing **every**
decoration-aware client — alacritty, kitty, and by extension most toolkits.

The entire milestone suite could not have found it. Firefox is the acceptance
app, and Firefox never binds `zxdg_decoration_manager_v1` at all —
`shim/docs/globals-touched-firefox-140.12.0esr.log` records it
`status=untouched`. A project whose acceptance set is two clients has a blind
spot exactly the width of what those two clients do not do. A maintainer living
inside the thing is the only instrument that finds the rest.

That is the argument for WS-E, and it is a *testing* argument, not a product
one.

## 2. What already works, measured

Run headless, scored as "mapped a window and repainted". Not a functionality
test — nothing was typed into these, clicked, or checked for correct rendering.

| Class | Result |
|---|---|
| Terminals (alacritty, kitty) | works |
| Chromium | works |
| Electron (VS Code — so also Discord, Slack, Obsidian) | works |
| GTK4 (nautilus) · GTK3 (gimp, inkscape, pavucontrol) | works |
| Firefox | works — already an acceptance gate |
| **X11 (xterm)** | **fails: `Can't open display`. No XWayland.** |
| **Bars/launchers (waybar, rofi, wofi)** | **fails: connects, binds six globals, never maps. No `zwlr_layer_shell_v1`.** |

Two structural facts sit behind the whole table: realms have **no session
D-Bus** (deliberate — it is the AT-SPI backdoor argument), so portals, file
pickers and notifications degrade; and rendering is software by default, with
dmabuf behind `--dmabuf`.

## 3. The four gaps that actually bind

None of these is DRM work. The backend is not the binding constraint.

1. **One app at a time — closed by WS-E.1.2 (#208) and WS-E.1.3 (#209).**
   `MAX_REALMS` is now **16** and a session runs every realm its `realm.toml`
   declares, each with its own shim, runtime tree, socket, **scene and
   capture**. Each `Scene` still holds at most one client surface,
   single-maximized — that is the model *per app*, not per session — and
   exactly one of them is bound to the one output, so several realms run and
   one is visible. Which one **is now a client's to choose** (WS-E.1.4/#210):
   a principal holding `layout_focus` binds the output, and `session::seat_target`
   follows that binding, so the visible realm and the realm the human's input
   reaches move as one act — the fifth of D-018(2)'s unpurchasable ordering
   rules. Absent such a client the old placeholder still answers: the first
   still-serving realm in id order, applied at first attach and again when the
   realm holding the output exits. Not moving it at all was the first shape and it was a
   defect: the output stayed bound to a realm that was gone, compositing the
   deterministic background for the rest of the session while live siblings
   ran. **The registry's claim that
   raising the cap was *"a deletion here rather than a re-plumbing"* turned
   out to be half true**, and `crates/vitrin-core/src/realm.rs`'s module docs
   now carry the audit: the registry, grant keying, path derivation, lifecycle
   and recorder really were deletions; `session.rs`'s single
   `Option<RealmRuntime>`, its ten call sites there, the nested backend's
   delivery sink, the input router's per-generation state and the runtime
   tree's flat namespace were not. Four of those were behavioural bugs that
   only a second realm could express — a dead realm's grant capturing a live
   sibling's scene, an agent's actuation being *delivered into a sibling's
   app*, one realm's death resetting the session-wide input router, and a
   failed spawn orphaning the realms already forked — which is the honest
   measure of how wrong "a deletion" was. All four were caught by review
   rather than by a test, which is its own finding. The scene was the real
   work and WS-E.1.3 did it: one scene per realm, one bound to the output,
   a capture resolved from the grant's realm id. Its own cost — every realm
   renders whether or not it is visible, and `MAX_REALMS` now has a measured
   memory bill rather than only a descriptor one — is published in
   `docs/book/src/limits.md` and re-derived in `realm.rs`.
2. **No way to launch an app.** `vitrin_realm` has exactly **one** request
   (`request_grant`). Realms exist only from `realm.toml` at startup, so
   changing app means restarting `vitrind`. This is new protocol, and it is an
   *authority* question: a principal that can spawn arbitrary realms holds a
   large new capability.
3. **No window management, by invariant.** [PRD](../PRD.md) §5.1 makes
   "window-management policy lives outside the core" permanent.
   [D-018](20-decision-log.md) allocated `layout_arrange` (0x10) and
   `layout_focus` (0x20); ~~both are served `unsupported` today~~ **both are
   served as of WS-E.1.4 (#210)**, through the `vitrin_layout_focus` and
   `vitrin_layout_arrange` facets. What is served is *arrangement*, not window
   management: focus and fullscreen-or-not, with no `place`, `resize`, `raise`
   or stacking request in existence. The shell is
   therefore still a **client**, never core code.
4. **No cross-realm clipboard.** `wl_data_device_manager` is per-shim and
   `shim/src/globals.c` states it *"GRANTS NOTHING ACROSS THE REALM
   BOUNDARY"*. Copy-paste between apps does not exist. It is a cross-realm
   mediator, i.e. a capability design, not plumbing.

## 4. Stages

Sequenced **nested-first, bare-metal-last**. Stages 1–2 build inside a window
on the existing desktop, so they carry no risk to the running session and can
be dogfooded incrementally. Only Stage 3 takes DRM master.

| Stage | Delivers | Est. |
|---|---|---|
| **1 — multi-app, nested** | Runtime app launch · ~~`MAX_REALMS` > 1~~ (**landed**, WS-E.1.2/#208: cap 16, `realm-0` mandatory) · ~~Scene binds the output to a focused realm~~ (**landed**, WS-E.1.3/#209: one scene per realm, one bound, captures resolved per grant) · ~~`layout_focus`/`layout_arrange` served~~ (**landed**, WS-E.1.4/#210: two facets, `focus` + `set_fullscreen`, `layout_held` for the second arranger, D-018(2)'s invariants tested as invariants) · a shell client (switcher + launcher) · input routed to the focused realm | 7–9 w |
| **2 — livable** | Cross-realm clipboard · core-drawn lock screen on the consent stack · status in the trusted band · human screenshot | 4–6 w |
| **3 — bare metal** | The keymap decision · DRM/KMS + GBM + GLES + libseat + libinput · VT switch and what the trusted band asserts across it · hardware bring-up and its evidence problem | 6–9 w |
| **4 — long tail** | X11 (defers to E3.2) · seat vocabulary for touch/gestures/lid · session lifecycle · the honesty sweep | open |

**Stage 1 is the one that is genuinely dual-use.** Layout verbs are allocated
and unserved, and multi-realm is Phase-3 fleet work; both get built here
regardless of whether Stage 3 ever happens. Stages 3–4 are not dual-use, and
that is where the schedule risk concentrates.

**Stage 3's first task is a decision, not code.** The core holds no keymap by
design — `vitrin_shim_seat.key` carries keysyms *"precisely so no keymap lives
here"*. libinput gives evdev scancodes, and `invariant_keysym` covers Escape,
arrows and modifiers and **not a single letter**. Either xkbcommon interprets
physical input inside the core (zero new crates; it is already a mandatory
Smithay dependency) or session mode cannot type. `input/mod.rs:109` already
records the consequence: key pairing moves from the keysym to the scancode.

## 5. The target machine, and why no number here generalizes

Every WS-E estimate is measured against hardware chosen for being easy:

- One connected output, eDP-1, 2560×1600@240, **scale 1** — no fractional
  scaling anywhere in the workstream.
- eDP-1 is on `card1` = **i915**. The discrete NVIDIA GPU's connector is
  disconnected and `nvidia_drm` is not loaded, so scanout *and* render are
  Intel: no PRIME, no multi-GPU renderer, the most well-trodden path in
  Wayland.
- 2560×1600@240 means CPU compositing is not viable (~16 MB/frame), so
  GLES+GBM is mandatory rather than optional — which on Intel is also the easy
  path.
- Every system library is already present: wlroots 0.19.3, libinput, libseat,
  libudev, gbm, xkbcommon, pixman.

**Stating a WS-E result as a portability claim would be false.** The horizon
item's cost is dominated by the machines this list excludes.

## 6. Limits this workstream creates

WS-E makes a thing that *looks* like a desktop, which is precisely when
unstated gaps become misleading. Each of these is a published `known-limit`
this workstream owns, not inherits:

- **No accessibility of any kind.** No screen reader, magnifier, high
  contrast, sticky or slow keys. The semantic channel is **not** a substitute
  for AT-SPI — it serves agents, not humans. A daily driver with no screen
  reader is a real exclusion and is stated as one.
- **No X11**, so no Steam and no legacy application.
- **No bars, launchers, notifications or OSD** — there is no
  `zwlr_layer_shell_v1` and there will not be one at the app level; the
  replacements are core-owned surfaces.
- **A shell crash loses window management**, because the shell is a client and
  there is no core-side fallback. §3(3)'s invariant is right and this is its
  price.
- **The DRM backend cannot be tested by CI** — no runner has a DRM device or a
  seat — so it arrives with structurally weaker evidence than anything else in
  the tree. That is an asymmetry against D12 and it is published, not
  discovered.
- **No touch, gestures, tablet, switches or relative motion**: v0's seat
  vocabulary is pointer + keyboard only, so on a laptop that means no touchpad
  gestures and no lid switch.
- ~~**Several realms run, one is visible, and a capture cannot tell them
  apart**~~ (created by WS-E.1.2, **closed by WS-E.1.3**). Raising the cap
  landed before the scene bound an output to a realm, so for one workstream
  task a multi-realm session composited one output from one single-surface
  scene: the last committer was what was on screen, and an agent's capture
  was of that output rather than of the realm its grant named. While two
  realms were **live**, a capture under a grant over one could carry the
  other's pixels. The exposure always stopped at *live* realms — liveness is
  judged per realm, against the realm the grant row names, so a grant over a
  dead realm refuses `no_surface` whatever its siblings are doing. (Judging
  it against *any* live realm was the obvious rewrite and would have been
  fail-**open** across realms — an authority bug, not a fidelity one; the
  review of WS-E.1.2 caught it before merge.) WS-E.1.3 closed the live half
  by making the frame a *function of the realm id*: one scene and one
  composed frame per realm, resolved from the grant row's realm on the same
  line that judges its liveness. Landing the cap first was the right order
  (WS-E.1.3 needs more than one realm to bind an output *to*); shipping the
  gap silently would not have been, and it was not.
- **Every realm renders, visible or not** (created by WS-E.1.3, no owner).
  The price of the item above. A Wayland client throttles on frame
  callbacks, so a realm that stopped being paced would stop repainting and
  its capture would go stale — which `refusal.no_surface` forbids in as many
  words. So every live realm is paced off the output's completed composite
  and every live realm's view is composed. On the WS-E laptop that is the
  difference between one app compositing and up to sixteen, plus
  `2 x width x height x 4` bytes of core-side pixels per realm (~590 MiB
  resident at sixteen realms on a 2560x1600 panel, measured, which is why
  `MAX_REALMS`'s justification had to be re-derived against memory rather
  than descriptors). Visibility-aware pacing would buy the power back at the
  cost of capture honesty; that trade was declined rather than overlooked.
  Published in `docs/book/src/limits.md`.
- **The agent cursor is drawn only for the visible realm** (created by
  WS-E.1.3, fixed by a per-realm indicator nobody has scheduled). D-019 added
  the sprite so a human can see that an agent is acting. It is painted into
  the output, which shows one realm, so an agent actuating inside a hidden
  realm draws nothing — the exact defect D-019 exists to close,
  reintroduced for hidden realms. Published in `docs/book/src/limits.md`.
- **Only one realm can be actuated, and the rest are refused rather than
  misdelivered** (created by WS-E.1.2, closed by WS-E.1.6). The write-side
  twin of the item above, and the one the same review found second. There is
  one input router and one delivery target (`session::seat_target`, the
  first still-serving realm in id order), so an actuation admitted under a
  grant naming *another* realm would have been delivered into a sibling's
  app — an agent driving an app it holds no authority over, which is a
  **write**, and worse than the read the item above describes. It was
  unreachable while `MAX_REALMS` was 1; raising the cap is what made it
  reachable.

  The fix is deliberately **not** routing. Routing — focus, per-realm
  `PhysicalPresence`, which realm physical input follows — is WS-E.1.6's
  whole deliverable and #208's Key decision 5 defers it there; building half
  of it under a bug fix would be the same mistake in the other direction.
  So the chokepoint compares the grant's realm with the realm the seat
  actually serves and **refuses** `internal` when they differ, delivering
  nothing. `internal` because the IDL already defines it as "server-side
  failure during this use (renderer, memfd, **delivery**)", which is exactly
  true here: the authority was real and this core cannot carry out the
  delivery. Nothing in the refusal vocabulary is invented, and `unsupported`
  is a petition outcome rather than a refusal code, so it was never a
  candidate.

  Published in `docs/book/src/limits.md`. WS-E.1.6 deletes the guard and the
  placeholder together; a real router makes the comparison unnecessary
  rather than merely passing.

## 7. Safety rule, non-negotiable

**A DRM backend takes DRM master and the seat. Running one from inside the live
session kills that session.** Every Stage-3 task runs on an isolated VT or a
second machine, with an SSH escape route. This is the same hazard class as
injecting input into a live session, and it is written here so no task has to
rediscover it.

## 8. What this workstream is not

- **Not the horizon item** (§0), and not evidence toward M4.
- **Not a product.** [PRD](../PRD.md) §5.4 renounces displacing Wayland on
  today's human desktop as a project aim; nothing here changes that.
- **Not a reason to stop Phase 2.** WS-E's estimate is roughly Phase 2's
  remaining budget. D-021 records that as an unmitigated cost and a priority
  choice, not a solved problem.
