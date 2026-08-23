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

**Four things about this table, found by WS-E.4.1 (#221) when it went looking
for cells to seed a published matrix with, and recorded here rather than left
standing.**

- **It is undated.** Every row above arrived in one commit (`7863702`,
  2026-08-06) and there is no log, recorder dump, ledger or screenshot behind
  any of them. That commit date bounds when the row was *written*, not when the
  app was *run*. No date may therefore be attached to these cells: a commit's
  date is not a run's, and reusing it would be the stale-matrix failure #221
  names as worse than no matrix at all.
- **The Electron row names three applications that were never run.** Only VS
  Code was executed; *"so also Discord, Slack, Obsidian"* is an inference from a
  shared runtime, not three more measurements. A matrix cell exists only if a
  runner executed it, so those three earn no cell.
- **`pavucontrol` is in this table and missing from #221's own seed list**, on
  exactly the same (weak) evidence as gimp and inkscape. Noted so the omission
  is a decision rather than an accident.
- **Firefox's row is the one that is stronger than the bar above it.** It is
  `tests/integration/test_real_firefox.py` — the mock-free M1.2 gate, real core
  to real shim to real browser to the real SDK, asserting the captured frame's
  dominant colour — against the **pinned ESR 140.12.0 tarball**
  (`shim/tests/firefox/firefox-esr.pin`), which is not a Firefox installed on
  this machine. There is no `firefox` package here at all; what is installed is
  `firefox-developer-edition 154.0b8-1`. A cell for it must name the pin, never
  "Firefox".

`xterm`'s row and the bars/launchers row are scoped and measured by §4.2.

## 3. The four gaps that actually bind

None of these is DRM work. The backend is not the binding constraint.

1. **One app at a time — closed by WS-E.1.2 (#208) and WS-E.1.3 (#209).**
   `MAX_REALMS` is now **16** and a session runs every realm its `realm.toml`
   declares, each with its own shim, runtime tree, socket, **scene and
   capture**. Each `Scene` still holds at most one client surface,
   single-maximized — that is the model *per app*, not per session — and
   exactly one of them is bound to the one output, so several realms run and
   one is visible. Which one **is now a client's to choose** (WS-E.1.4/#210):
   a principal holding `layout_focus` binds the output, and
   `session::physical_seat_target` follows that binding, so the visible realm
   and the realm the **human's** input reaches move as one act — the fifth of
   D-018(2)'s unpurchasable ordering rules. An **agent's** actuation does not
   follow the binding at all (WS-E.1.6/#212): it goes to the realm its own
   grant names, so an agent works in a realm nobody is looking at. Absent such a client the old placeholder still answers: the first
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
2. ~~**No way to launch an app.**~~ **Closed by WS-E.1.1 (#207)**, in both
   halves: the protocol half (#225) put `realm_launch`, `vitrin_grant.get_launcher`
   and `vitrin_launcher` on the wire at version 2, and the core half serves
   them. A realm declared `autostart = false` in `realm.toml` is a
   **template**; a principal holding `realm_launch` over one calls
   `vitrin_launcher.launch` and the core forks an instance under an id it
   mints itself (`<template>.<n>`).

   The *authority* question this gap named is what the shape answers.
   `launch` carries no arguments, so the command never crosses the wire: the
   grant names a template, the template names the program, and the human sees
   that program on the consent card. A launch grant is therefore authority
   over operator-written configuration, never over an arbitrary command line
   — and it is capped (`MAX_REALMS`, refused `capacity`), rate-limited,
   revocable, expiring and journaled with the principal and grant that asked.
   What it is *not* is bounded in what the launched app may then do: that is
   Phase-2 confinement (E2.6/E2.7), and `docs/book/src/limits.md` says so.
3. **No window management, by invariant.** [PRD](../PRD.md) §5.1 makes
   "window-management policy lives outside the core" permanent.
   [D-018](20-decision-log.md) allocated `layout_arrange` (0x10) and
   `layout_focus` (0x20); ~~both are served `unsupported` today~~ **both are
   served as of WS-E.1.4 (#210)**, through the `vitrin_layout_focus` and
   `vitrin_layout_arrange` facets. What is served is *arrangement*, not window
   management: focus and fullscreen-or-not, with no `place`, `resize`, `raise`
   or stacking request in existence. The shell is
   therefore still a **client**, never core code.
4. ~~**No cross-realm clipboard.**~~ **Closed by WS-E.2.1 (#213)**, on the
   narrowest path that closes it: a **core-held single slot**, filled only
   when the human presses Ctrl-Shift-Insert in the realm the output is bound
   to and offered only when they press Shift-Insert in another,
   `text/plain;charset=utf-8` only, 60 KiB, cleared on timeout, on the
   source realm's death and on a dead-man trigger. `wl_data_device_manager`
   is still per-shim and still mediates nothing across the boundary itself —
   what changed is that the core now mediates, which is what
   `shim/src/globals.c` always said would happen *"when it is built"*. The
   design and every rejected option are §4.1; the decision is
   [D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing).
   It was a capability design and not plumbing, and it stayed one: the
   options table landed before a line of it was written.

## 4. Stages

Sequenced **nested-first, bare-metal-last**. Stages 1–2 build inside a window
on the existing desktop, so they carry no risk to the running session and can
be dogfooded incrementally. Only Stage 3 takes DRM master.

| Stage | Delivers | Est. |
|---|---|---|
| **1 — multi-app, nested** | ~~Runtime app launch~~ (**landed**, WS-E.1.1/#207: `autostart = false` templates, a served `realm_launch` verb, core-minted `<template>.<n>` instance ids, `capacity` at `MAX_REALMS`, and `realm_spawned` naming who asked) · ~~`MAX_REALMS` > 1~~ (**landed**, WS-E.1.2/#208: cap 16, `realm-0` mandatory) · ~~Scene binds the output to a focused realm~~ (**landed**, WS-E.1.3/#209: one scene per realm, one bound, captures resolved per grant) · ~~`layout_focus`/`layout_arrange` served~~ (**landed**, WS-E.1.4/#210: two facets, `focus` + `set_fullscreen`, `layout_held` for the second arranger, D-018(2)'s invariants tested as invariants) · ~~input routed to the focused realm~~ (**landed**, WS-E.1.6/#212: physical input follows the bound realm, an agent's follows its grant, per-realm `PhysicalPresence`, and the cross-realm refusal deleted) · ~~a core-owned attention key~~ (**landed**, WS-E.1.7/#232: a tapped, consumed Super lifts `preempted` for one layout use and delivers `vitrin_principal.attention`, so an in-realm shell can switch realms at all) · a shell client (switcher + launcher) | 7–9 w |
| **2 — livable** | ~~Cross-realm clipboard~~ (**landed**, WS-E.2.1/#213: a core-held single slot the core *pulls* into on Ctrl-Shift-Insert and offers on Shift-Insert, `text/plain;charset=utf-8` at 60 KiB, plus the modifier-aware chord matcher 2.2 and 2.4 consume — §4.1, [D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing)) · ~~core-drawn lock screen on the consent stack~~ (**landed**, WS-E.2.2/#214) · ~~status in the trusted band~~ (**landed**, WS-E.2.3/#215) · ~~human screenshot~~ (**landed**, WS-E.2.4/#216: `ctrl+print` writes one PNG of the REALM VIEW into one audited `--screenshot-dir`, touching no grant — §6) | 4–6 w |
| **3 — bare metal** | ~~The keymap decision~~ (**landed**, WS-E.3.1/#217: xkbcommon in the core behind the off-by-default `session-keymap` feature, fed a pre-compiled keymap **file** and never a layout name, keysyms normalised to one Unicode convention, and key pairing moved to the scancode — §4, [D-028](20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode)) · DRM/KMS + GBM + GLES + libseat + libinput · VT switch and what the trusted band asserts across it · hardware bring-up and its evidence problem | 6–9 w |
| **4 — long tail** | X11 (defers to E3.2 — **scoped, not built**, WS-E.4.1/#221: six requirements handed to E3.2, the X11-only software measured on this machine, and the interim, all in §4.2) · ~~seat vocabulary~~ (**landed, and exercised on the target laptop 2026-08-13 — five of [`docs/drm-bringup.md`](../drm-bringup.md) step 13a's six rungs PASS, including rung 13a-vi's cursor-sprite half that no headless backend can reach; the one failure is rung 13a-v, where a swipe interrupted by a VT switch is delivered `completed` where it must say `cancelled`, open as [#275](https://github.com/vitrin-os/vitrin-os/issues/275)**, WS-E.4.2/#222: `relative_motion` and four gesture events on `vitrin_shim_seat`, a `pointer_constraint` ask-and-verdict pair on `vitrin_shim_session`, three new shim globals, touch and tablet deferred against named reopening evidence, the lid handed to WS-E.4.3 — §4.3, [D-032](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)) · ~~session lifecycle~~ (**landed, and executed on hardware on 2026-08-11 and 2026-08-13, and again on 2026-08-12 for the blank rung alone** — rungs `L1`–`L7` of [the recovery runbook](../book/src/recovery.md#the-numbers-this-checklist-owes) have all been run and all have their numbers, that second `L4` execution reading the log and the flight recorder rather than only the panel and so observing #258's and #259's fixes on the machine that produced them, at the cost of #257–#260 and #277 filed against those runs — and of #268, which came out of the same 2026-08-11 session from driving alacritty and nautilus rather than from any rung, so a reader counting defects against that date should count five — with routes 3 and 4 still unexecuted predictions and the VKMS rung attempted on every PR while covering nothing, WS-E.4.3/#223: idle **blanks and does not lock**, one shared activity clock, suspend detected after the fact from the monotonic/wall clock pair rather than from D-Bus, lid and power delegated to logind, the XF86 media/brightness rows, and a recovery runbook whose SysRq path is `sudo`-only with the kernel mask untouched — §4.4, [D-033](20-decision-log.md#d-033--idle-blanks-the-screen-and-does-not-lock-it-suspend-is-detected-after-the-fact-or-not-at-all-and-the-recovery-path-is-sudo-only)) · ~~the honesty sweep~~ (**landed**, WS-E.4.4/#224: §6's register of the published surfaces a WS-E limit is written on, each with the register it is written in and what holds it, plus the marker-comment limit-set gate that compares this document's limit set against `limits.md`'s **by id and never by wording**, so either register can be reworded honestly — the §4.2 and §4.4 handoff tables stay as the dictated text for their own claims) | open |

**Stage 1 is the one that is genuinely dual-use.** Layout verbs are allocated
and unserved, and multi-realm is Phase-3 fleet work; both get built here
regardless of whether Stage 3 ever happens. Stages 3–4 are not dual-use, and
that is where the schedule risk concentrates.

**Stage 3's keymap decision was untouched by WS-E.1.7.** The attention key is
drawn from `invariant_keysym` — the two Super scancodes — and needs no keymap,
no modifier resolution, and nothing like `Super+1`..`Super+9`. That was a
deliberate limit rather than an accident: a hotkey would have pre-empted the
decision below.

**Stage 3's first task was a decision, and it is taken** —
[D-028](20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode),
2026-08-08. The core held no keymap by design; libinput gives evdev scancodes,
and `invariant_keysym` covers Escape, arrows and modifiers and **not a single
letter**, so session mode could not type. xkbcommon now interprets physical
input inside the core, and three things about that are worth carrying forward
rather than rediscovering:

- **"Zero new crates" was the wrong unit and this plan said it too.** The
  package is already in `Cargo.lock` via smithay, but the shipped binary does
  not link `libxkbcommon.so.0` — measured, zero `NEEDED` entries and zero
  `xkb_*` undefined symbols. Adopting it costs the TCB 383 144 bytes of C, 87
  `extern "C"` declarations and a parser over a ~73 KB file. It is therefore
  behind an **off-by-default** `session-keymap` feature that WS-E.3.2 turns
  on, and a default build must keep the old measurement byte for byte.
- **The keymap is a file the operator names, never a layout name.**
  `new_from_names` searches `~/.config/xkb` before the system path, and a
  realm's app runs as the core's uid, so a name-resolved keymap is an
  app-writable file the TCB parses.
- **Key pairing moved to the scancode, and the release now carries the
  keysym its own press delivered** — `input/mod.rs:109`'s warning, discharged.
  The second half is the one that reaches the app: the shim binds a keycode
  per keysym, so a release carrying the *release's* sym would strand the
  pressed one down.

## 4.1 The cross-realm clipboard: five axes, each with what was rejected

Stage 2's first deliverable (WS-E.2.1, issue #213), written out here rather
than in the issue because the issue closes and this does not. Landed as
**[D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing)**.

The whole of it is a **capability design**, not plumbing: [PRD](../PRD.md) §15's
first threat row lists *"reach the session's real seat/clipboard/a11y bus"*
among the things a malicious app cannot do, so every option below is a
deliberate, bounded hole in a published claim, and the argument for the bound
is the artifact.

### Axis 1 — where the bytes live

| Option | Cost | Verdict |
|---|---|---|
| **(a) A core-held single slot** — PRD §11's Qubes model | **For the first time the TCB retains bytes an application authored.** The core holds client *pixels* it never interprets and typed values it validated itself; a clipboard slot is neither. A password copied from a manager rests in `vitrind`'s address space with a lifetime, so a compromised core exposes whatever was last copied. | **ACCEPTED — the maintainer's own call, asked and answered 2026-08-08.** He was given (b), (c) and "don't build it" in plain terms, was told this cost in exactly these words, and chose (a) anyway. |
| (b) No buffer: a direct A→B pipe the core brokers at paste time | Tempting — the TCB never stores app bytes. But it requires **both realms alive simultaneously** (copy, close the app, paste is then impossible, which no human expects), and it **leaks a timing signal back to A**: A learns exactly when B pasted. That is a covert channel in the wrong direction — from the *sink* back to the *source* — and the whole point of the mediator is that a realm learns nothing about another. | REJECTED |
| (c) A per-realm broker each realm pulls from | Multiplies state by `MAX_REALMS` without changing the trust question: the bytes are still core-side, there are now sixteen places for them to be, and the lifetime rule has to hold in all of them. Strictly worse than (a) on every axis and better on none. | REJECTED |
| (d) Do not build it | Genuinely available, and offered. Copy-paste between apps is the single most-used cross-app act on a desktop; a session mode without it is not one a maintainer daily-drives, which is WS-E's entire purpose. | REJECTED by the maintainer, 2026-08-08 |

### Axis 2 — push or pull

| Option | Cost | Verdict |
|---|---|---|
| **The core PULLS: `request_selection` → `selection` → `offer_selection`** | Two round trips per gesture instead of a cached answer, and the source realm must still be alive at *promote* time (not at paste time — that is (b)'s failure, and this does not inherit it). | **ACCEPTED** |
| A `selection_changed` event the shim pushes on every app-side copy | **Every ordinary Ctrl-C becomes a cross-realm event.** An ambient channel wearing a broker's clothes: the human never asked, the core learns every copy the human makes inside every realm, and the Qubes phrase this design quotes — *"fully controlled by the user, it cannot be triggered/forced by any"* realm — stops being literally true. | REJECTED |
| A hybrid: the shim pushes a *notification* only (MIME + length, no bytes), the core pulls the bytes on the gesture | Looks like a compromise and is not. A per-copy cross-realm signal is still a per-copy cross-realm signal: it is a free keystroke-timing oracle over the human's own editing, available to nobody who asked for it, and it buys only the ability to grey out a menu item that does not exist. | REJECTED |

The wire shape is the mechanism, not a restatement of it: **there is no message
by which a shim can put bytes in the slot unasked.** `selection` is answerable
only against an outstanding, core-minted promotion ticket (`clipboard.rs`'s
`PendingPromotion`, which is not `Clone`, has no public constructor and is
consumed by value), so an unsolicited `selection` from a compromised shim has
nothing to consume and fills nothing. "The core pulls" is a type, not a rule.

### Axis 3 — is the human path grant-governed?

| Option | Cost | Verdict |
|---|---|---|
| **No. The human's gesture is a fact the core acts on, journaled — the dead-man switch's precedent** | An authority-bearing act with no grant behind it, so `known-limit` grows and a reader who expects every channel to appear in the grant table will not find this one. Mitigated by journaling both halves and by the fact that the *only* actor is the human at the physical keyboard. | **ACCEPTED** |
| A `clipboard` verb bit for the human path | **The human is not a wire principal in v0.** A verb keys on a `PrincipalIdentity` bound at `hello`; the human has none, so the bit would name a principal that is not the actor. Inventing one touches `identity.rs`, `principals.toml`, the grant table and the consent surface at once, and burns an immutable bitfield entry on authority nobody can hold. | REJECTED |
| A consent prompt per transfer | Strictly stronger on attribution, and refused for [D-023](20-decision-log.md#d-023--the-core-owns-a-second-physical-chord-a-tapped-consumed-attention-key-that-lifts-preempted-for-the-two-layout-verbs-only-and-delegates-nothing)(2)'s reason, which applies harder here: copy-paste is a several-times-a-minute act, and a modal card on it trains reflexive clicking on the one surface in this system whose entire worth is that a human reads it. Q9 already tracks consent fatigue as a live problem. | REJECTED |

**The agent-facing clipboard verb is E3.5's and is not built here.** What this
axis owes E3.5 is only that the shape must not foreclose it, and it does not:
`offer_selection` is addressed to a realm and carries no notion of who asked,
so an agent-facing `vitrin_grant` facet can later reach the same slot through
the enforcement chokepoint with a verb of its own.

### Axis 4 — the gesture, and why it cannot be Ctrl-Shift-C

| Option | Cost | Verdict |
|---|---|---|
| **Ctrl-Shift-Insert (promote) / Shift-Insert (offer)** | The core eats both chords in every realm, unconditionally: an app that wants X11-style Shift-Insert primary paste loses it with no pass-through, exactly as [D-023](20-decision-log.md)'s cost note says of Super. `KEY_INSERT` is in `invariant_keysym` (`input/mod.rs:1265`) and is already in the dead-man vocabulary, and Shift-Insert is the *historical X11 clipboard chord*, so it is familiar rather than invented. | **ACCEPTED** |
| Ctrl-Shift-C / Ctrl-Shift-V, the Qubes chord | **Not expressible.** `invariant_keysym` is a fixed scancode table containing no letters and no digits — asserted, not assumed (`input/mod.rs:1772-1774`: `KEY_A` → `None`). Letters arrive today only in nested mode, because winit's `logical_key` means the *host* compositor did the interpretation (#118). On bare DRM in Stage 3 there is no host, so this chord would work on the maintainer's laptop and stop working the day the workstream reached its point. | REJECTED |
| Grow an xkbcommon keymap in the core so letters *are* expressible | `input/mod.rs:106-109` warns that a real keymap forces key pairing to move from the keysym to the scancode — a change to a **router invariant**, on the path the dead-man switch depends on — and it is an R7 dependency event on top. It is also Stage 3's decision (§4 above), and pre-empting it from a clipboard issue is exactly the drift the stage list exists to prevent. | REJECTED **here**; taken at Stage 3 by [D-028](20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode), which does not reopen the chord: the clipboard chord stays inside `invariant_keysym`, because a chord whose keysym moves with the layout is a gesture that stops working on somebody else's keyboard |
| One chord plus a core-drawn direction picker | Needs a modal core-drawn surface for a several-times-a-minute act: Axis 3's consent-fatigue argument, one indirection over. | REJECTED |
| Two more Super-based taps, in the attention chord's mould | The attention chord already consumes both Supers (D-023). A third and fourth Super gesture would be distinguishable only by timing, which is how a human accidentally revokes the wrong thing. | REJECTED |

A chord this backend cannot deliver is **refused at startup**, exactly as
`deadman::Chord::parse` refuses one — same `keysym_is_intakeable` check, same
fail-closed posture, because a session that comes up with a clipboard gesture
that can never fire is the same trap one gesture milder.

### Axis 5 — what may cross, and for how long

| Option | Cost | Verdict |
|---|---|---|
| **`text/plain;charset=utf-8` only** | No image, no rich text, no files. A human copying a screenshot between realms gets nothing and no error they can see. | **ACCEPTED** |
| Any `image/*` type | A decoder in the TCB. *"No image codec in the core, in any dependency class"* is a rule `crates/vitrin-core/Cargo.toml` states twice, and it is stated twice because it is the single largest memory-unsafety surface a compositor can acquire. | REJECTED |
| `text/uri-list` | A path is a **designation**, and designation belongs to the powerbox (E2.6). A clipboard that moves paths is a file-transfer channel wearing a clipboard's clothes, and it would cross the confinement boundary Phase 2 is being built to draw. | REJECTED |
| **Hard cap: 61 440 bytes (60 KiB), measured** | 3.4% of this repo's own text files could not be copied whole. See below — the number is measured, not asserted. | **ACCEPTED** |
| A 1 MiB cap, as the issue proposed | **Not expressible on this wire.** The frame header's `size` is a `u16`, so a whole frame is ≤ 65 535 bytes and the largest string a `selection` can carry is 65 476. 1 MiB would have to travel as an **fd**, which puts a shim-controlled mapping inside the TCB and re-opens the size-lie and SIGBUS class the buffer path already spends `invalid_buffer` on — to carry a clipboard. | REJECTED |
| Cleared on paste | Humans paste twice. Surprising in the way that makes people stop trusting a mechanism. | REJECTED |
| Never cleared | A copied password resident for the life of the session. | REJECTED |
| **Cleared on timeout, on the source realm's death, and on a dead-man trigger** | Three clearing rules to keep consistent instead of zero. | **ACCEPTED** |

**Measuring the cap, because `MAX_REALMS` was argued wrongly three times before
anyone measured it.** Two bounds meet here and only one of them is a choice.

*The wire bound is not a choice.* A `selection` frame is 8 bytes of header +
`serial` (4) + `status` (4) + the MIME string at its declared bound (4 + 32) +
the data string's own 4-byte length: **56 bytes of overhead** against a
65 535-byte ceiling, leaving **65 476** bytes of payload once 4-aligned.
Nothing above that is sendable without an fd.

*The use bound is measured — and the first measurement was wrong, which is
recorded here rather than quietly replaced.* The proxy for "a human selects all
and copies" is the size of a real text file. The original figures were taken by
globbing the working tree, which at the time held **~40 scratch worktrees under
`.claude/`, a vendored wlroots checkout and generated build output** — so ~96%
of the 6 550 files counted were not this repository's source at all, and none of
the five published statistics reproduced. The population was the error, not the
arithmetic, and it is precisely the failure mode this section was written to
avoid (`MAX_REALMS = 16` was argued wrongly three times before anyone measured
it). Re-measured over `git ls-files` — the 310 tracked, non-binary text files
that actually are this repository:

| | |
|---|---|
| median | **11 410 B** |
| p90 | **53 286 B** |
| p95 | **106 707 B** |
| p99 | **240 239 B** |
| max | **400 932 B** |

Whole-file coverage by cap:

| Cap | Files copyable whole |
|---|---|
| 4 KiB | 24.84% |
| 32 KiB | 80.65% |
| **60 KiB (chosen)** | **92.26%** |
| 65 476 B (the frame maximum) | 92.90% |
| 1 MiB (inexpressible) | 100% |

So the last 4 036 bytes the frame could carry buy **0.64 percentage points** —
five times what the bad measurement claimed, and still not enough to justify
spending every byte of framing headroom on them.

**What this population is not.** It is one systems repository: prose-heavy
Markdown and Rust, no minified assets, no CSV, no logs, no notebooks. It stands
in for "a developer copies a file" and for nothing else, and a deployment whose
users copy spreadsheets would measure differently. The cap is chosen against
that stated proxy, not against a claim about text in general. 60 KiB leaves 4 039 bytes spare — room for
`selection` to gain an argument in a later version without the cap moving, which
matters because the cap lives in the IDL's immutable `(max N bytes)` token and
so cannot move. That, and not roundness, is why the number is 61 440.

### What this costs, stated as bandwidth rather than as absence

Two colluding realms can move **60 KiB per human gesture pair**. Qubes accepts
the same class of bound. The honest statement is the bound; *"there is no
channel"* stops being true the moment this lands, which is why PRD §15's threat
row and `docs/book/src/limits.md` are edited in the same commit rather than
left standing.

## 4.2 X11: what a daily driver needs from E3.2

Stage 4's first deliverable (WS-E.4.1, issue #221), written out here for §4.1's
reason: the issue closes and this does not.

**This is input to E3.2, and not a second design for it.**
[03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md) §E3.2 already
owns the shape, the recommendation between the two ways of building it, the
minimal `_NET_WM_*` coverage list and the anti-keylog exit test. None of that is
restated below and none of it is re-decided below — a workstream that redesigns
an epic is the competing roadmap the tracking model forbids outright. What this
section adds is the one thing an epic written before anybody ran the system
could not have: a closed list of what one maintainer's actual machine demands of
it, each item carrying the application that demands it and the measurement that
found it.

### How this was measured, and what the method cannot say

**Nothing was launched.** Not `vitrind`, not a realm, not a nested compositor,
not one application under test. Every statement below comes from two read-only
sources, on **2026-08-10**:

1. **Runs already recorded in this repository** — §2's table,
   [`docs/drm-bringup.md`](../drm-bringup.md), `shim/docs/`,
   `tests/integration/`.
2. **Static inspection of the installed software** — `pacman` for versions, and
   each binary's transitive `DT_NEEDED` closure read with `readelf -d`,
   resolving sonames through `ldconfig -p`. `ldd` was deliberately not used: it
   invokes the dynamic loader and can execute the binary it is asked about.

**Linkage is weaker than a run, and this scan proved it in both directions**, so
nothing below rests on linkage without saying that is what it is. Five binaries
whose closure carries `libX11` and no `libwayland-client` have a working Wayland
path anyway, because they load it at runtime or link their own copy statically:
`chromium 151.0.7922.108-1`, `blender 17:5.2.0-4`, `scrcpy 4.1-2` (whose X11
arrives from ffmpeg's `libavdevice` and not from its GUI, which is SDL3),
`openrgb 1.0rc3-1` and `rpi-imager 2.0.9-1` (Qt loads its platform plugin by
name; `qt5-wayland 5.15.19+kde+r55-1` and `qt6-wayland 6.11.1-1` are both
installed). Two more — `alacritty 0.17.0-1` and `kitty 0.48.2-1` — link
*neither* family and settle it at runtime. A cell in the generated matrix
([`docs/book/src/session-app-matrix.md`](../book/src/session-app-matrix.md)) is
an executed run; a classification here is labelled linkage wherever linkage is
all it is.

**The measured set widens by executing the runbook, never by editing a list.**
That is the same device P2.8.6 uses on the IME matrix and the reason the matrix
is generated rather than written.

### The requirement list

Six items. Each names the application that demands it — **an item with no
demanding application is a wish and is not here**, which is why three things the
maintainer named out loud (Java, a browser he has installed, and Steam) appear
in the section after this one instead. That is the measurement disagreeing with
the recollection that opened this issue, which is what it was for.

1. **An X display a realm's app can open at all.** Demanded by `xterm 410-1`,
   `feh 3.12.2-1`, `xsel 1.2.1-2` and `nvidia-settings 610.57.04-1`. `xterm` is
   the only one with a *run* behind it — §2's table, `Can't open display`. The
   other three are linkage: `feh`'s closure is 41 sonames with
   `libX11.so.6`/`libX11-xcb.so.1`/`libxcb.so.1` and no Wayland library at all,
   `xsel`'s is 6, and `nvidia-settings`' whole `NEEDED` set is
   `[libXxf86vm.so.1] [libjansson.so.4] [libX11.so.6] [libXext.so.6]
   [libm.so.6] [libc.so.6]` — `libXxf86vm` being an X-server-only extension with
   no Wayland counterpart to port to. `nvidia-settings` is also the trap that
   proves a grep is not a method: it *does* carry `wayland` strings, and reading
   them shows they are NVIDIA's own `libnvidia-wayland-client.so` output-query
   probes (`wconn_get_wayland_display`, `Wayland Connector Library failed to
   connect.`), not a GUI backend.
2. **A physical human's keystrokes reaching an X client.** Demanded by
   `xterm 410-1`. A terminal that maps a window and repaints and cannot be typed
   into is not a terminal, and §2's bar is exactly that bar — *"nothing was typed
   into these"*. The two halves of this are not equally unproven and the
   difference matters, so it is stated rather than averaged:
   - **An agent's** actuation into a real toolkit text field **is** proved, at
     milestone strength: `tests/integration/test_real_actuation.py`'s D7 rung
     (P1.8.6/#108) types `héllo→世界` through the real chokepoint into
     `gtk-entry-probe`, which reports the bytes back intact.
   - **A physical human's** keystrokes have never reached a desktop application
     here at all. That path has been exercised against `input-echo-client`
     (`shim/docs/nested-lock-screen.md`, 2026-08-09) and against the core's own
     lock screen on bare metal (`docs/drm-bringup.md`, item 13) — two repo test
     surfaces — while the GTK render gate says in as many words that it
     *"asserts render, not input"* (`tests/integration/test_real_gtk.py:25`).

   What E3.2 inherits from that split: D-028 put xkbcommon in the core behind
   the off-by-default `session-keymap` feature for the Wayland side, so an X
   path introduces a **second** interpreter of the same physical keyboard.
   Which one is authoritative is E3.2's to answer rather than to inherit.
3. **X selections joined to the clipboard the core already holds — or a refusal
   that says so.** Demanded by `xterm 410-1` and `xsel 1.2.1-2`. §4.1's slot is
   filled and offered by three `vitrin_shim_session` messages the **core**
   initiates against a Wayland shim; an X client's copy lives in an X selection
   owner that speaks none of them, so an X app in a realm has no route to the
   one cross-realm channel this workstream built. Worse, and already true: the
   offer chord is Shift-Insert, *the historical X11 primary-paste chord*, and
   the core eats it in every realm unconditionally (§4.1 Axis 4, §6). The X
   client's own paste gesture is gone before the X path exists.
4. **An X toplevel that fits a scene holding one client surface.** Demanded by
   `xterm 410-1`, the only X application with a recorded run. §3(1) is the
   constraint: each `Scene` holds at most one client surface, single-maximized,
   and that is the model *per app*. Every application in §2's table is a Wayland
   client whose shim maps one xdg toplevel. Whether an X client behind a
   rootless server arrives as one surface is E3.2's question; it is recorded
   here because the constraint belongs to **this** core and E3.2 cannot discover
   it from its own epic text.
5. **More than one X application, in more than one realm, at once.** Demanded by
   the measurement rather than by a single app: `xterm`, `feh`, `xsel` and
   `nvidia-settings` are four X11-only programs on one machine, and `MAX_REALMS`
   has been 16 since WS-E.1.2. E3.2's exit criteria already own the isolation
   that implies and it is not restated here. What this list contributes is only
   the fact that a one-X-application answer does not clear this machine.
6. **`DISPLAY` scrubbing must not be what does the isolating.** Demanded by
   `xterm 410-1` again, and it is the item most easily missed because today it
   looks like it already works. A realm's app is spawned from an environment
   built from **nothing** (`spawn.rs`'s "Environment hygiene": `env_clear`, then
   an allow-list), and `DISPLAY` and `XAUTHORITY` are refused outright by
   `realm::RESERVED_ENV` (`realm.rs:653`) with the reason recorded in the table
   itself — *"it addresses the host X server, which the core scrubs at spawn"*.
   That is **why** the recorded failure is `Can't open display` rather than a
   protocol error. Under D9 it is also a confinement of the well-behaved only:
   this machine's host session was running `Xwayland :0 -rootless` (pid 2062)
   with `/tmp/.X11-unix/X0` present and world-connectable when this was
   measured, and `spawn.rs` says in as many words that an app which ignores what
   it was handed *"and connects directly to a path it already knows is not
   stopped by anything in this file"*. An X path that hands realms a real X
   server must not be built such that the scrub is the security property.

**Six requirements, and the list ends here.** It is closed against one machine
on one day. It widens the way the matrix widens — by executing a runbook and
landing the run — and not by anyone adding a seventh line to it.

### What the measurement found that is *not* a requirement

- **X11 shell components are measured and deliberately excluded.**
  `picom-git 2855_12.197.g6d676824_2026.06.02-1`, `openbox 3.6.1-14`,
  `polybar 3.7.2-2`, `lemonbar-git v1.5.r2.g59b0d28-1`, `dmenu 5.4-1` and
  `slock 1.7-1` are all X11-only by linkage — `polybar`'s `NEEDED` set alone
  carries `libxcb-ewmh`, `libxcb-icccm`, `libxcb-randr`, `libxcb-composite`,
  `libxcb-xkb`, `libxcb-cursor` and `libxcb-xrm`, which are X11 window-manager
  protocols with no Wayland analogue. An X compositor, an X window manager, an X
  bar, an X launcher and an X screen locker are not applications to run *inside*
  a per-app X server, so no E3.2 exit criterion returns any of them. They are
  things this maintainer loses, and the loss is permanent rather than pending:
  PRD §5.1 keeps window-management policy out of the core, and §6 says there
  will be no `zwlr_layer_shell_v1` at the app level either.
- **The waybar/rofi class is not an X11 gap and must never be counted as one.**
  §2's table records that they connect, bind six globals and never map, for want
  of `zwlr_layer_shell_v1` — WS-E Stage 2's result, not this one's. Both are
  Wayland-native by linkage (`waybar 0.15.0-2` and `rofi 2.0.0-1` each link
  `libwayland-client.so.0` directly). The third name in that table row,
  `wofi`, **is not installed on this machine**, so its cell cannot be
  regenerated here by any runbook — the row names something nobody can measure.
- **#203 shrinks the Wayland set for a reason that has nothing to do with X11.**
  §1's assertion abort killed every decoration-aware client; it was fixed in
  `af98130` and **alacritty** was re-run to completion under `vitrind --nested`
  as #203's own acceptance criterion. **kitty** was proved to crash *before* the
  fix and there is no record anywhere of it being re-run after. Its cell is
  therefore weaker than §2's table row reads, and the matrix must not launder
  the difference.
- **`google-chrome 151.0.7922.71-1` is an unresolved contradiction, published as
  one.** Its 71-soname closure carries `libX11.so.6`, `libXext.so.6`,
  `libXfixes.so.3`, `libXi.so.6`, `libXrandr.so.2`, `libXrender.so.1` and
  `libxcb*.so` with no `libwayland-client` — yet the binary carries
  `ozone-platform`, `wl_compositor` and `libwayland-client` strings, and Arch's
  `chromium 151.0.7922.108-1` classifies **identically** while §2's table
  records Chromium working under a `vitrind` that serves no X11 whatsoever.
  Linkage cannot settle this one. It is an **unknown**, not an X11 requirement,
  and only a run settles it.
- **The Steam client is the same unknown, and this is the measurement
  contradicting the recollection that opened the issue.** #221 was written
  expecting Steam in the X11 set, and the first pass of this scan agreed:
  `steam 1.0.0.87-1` is only a two-line shell wrapper, so what matters is the
  Valve-bundled, self-updating binary at
  `~/.local/share/Steam/ubuntu12_64/steamwebhelper` (10 575 320 bytes,
  2026-07-22), whose 36-soname closure carries `libX11.so.6`, `libXext.so.6`,
  `libXfixes.so.3`, `libXi.so.6`, `libXrandr.so.2`, `libXrender.so.1`,
  `libXtst.so.6` and `libxcb*.so` with **no** `libwayland-client` — a count that
  is itself bounded by the method, because neither `libcef.so` nor
  `libSDL3.so.0`, both direct `NEEDED` entries, sits on `ldconfig`'s path, so
  the walk stops there — and which
  contains not one `ozone-platform`, `wl_compositor` or `libwayland-client`
  string of its own. That reads as settled and is not. Its own direct `NEEDED`
  entry `libcef.so` — the bundled 219 444 168-byte Chromium-embedded library
  that does the drawing, dated 2026-07-09 — **does** carry the ozone Wayland
  platform (`chrome_browser_main_extra_parts_ozone.cc`, `enable-wayland-ime`,
  *"Failed to initialize Wayland platform"*, *"Fatal Wayland protocol error %u
  on interface %s"*). So the Steam client is exactly `google-chrome`'s case one
  library deeper, an X11 requirement was nearly published on a scan that stopped
  at the executable, and the honest answer is **unknown**. Steam remains a
  measured dependency of this machine in the sense that matters for planning —
  it is installed, it has been run, `steamapps` holds seven app manifests —
  and *not* in the sense that would put it on a requirement list.
- **The system JVM has no Wayland backend, and no application here demands its
  X one.** `jdk-openjdk 26.0.2.u10-1` ships exactly `libawt.so`,
  `libawt_xawt.so` and `libawt_headless.so` — measured, and there is no
  `libawt_wayland.so`; `libawt_xawt.so`'s `NEEDED` set is
  `libX11`/`libXext`/`libXi`/`libXrender`/`libXtst` with no Wayland library. The
  one JetBrains-family application installed carries its own runtime that *does*
  have one: `/opt/android-studio/jbr/lib/libawt_wlawt.so`, JBR 25.0.2. So Java
  is a measured X11 dependency of a **runtime** and not of any application named
  on this machine, and by this section's own rule it earns no requirement line.
  It is recorded because the maintainer named the class, and because "Java needs
  X11" is exactly the kind of half-true a matrix is supposed to stop.
- **Electron and the modern toolkits are not X11 dependencies at all.**
  `/usr/lib/electron43/electron` links **no** `libwayland-*` directly — its
  103-soname closure reaches `libwayland-client.so.0`, `libwayland-cursor.so.0`
  and `libwayland-egl.so.1` only through `libgtk-3.so.0`, which is GTK's Wayland
  and not Electron's, and `/usr/share/code/code`, `/usr/lib/slack/slack`,
  `/opt/postman/app/postman` and `/opt/Antigravity/antigravity` all measure the
  same way. What says the ozone machinery is compiled in is `strings -a`:
  `ozone-platform`/`wl_compositor`/`libwayland-client` match 5 times in both
  `electron43` and `code`. `nautilus 50.2.2-1` links `libwayland-client.so.0`
  directly; `gimp 3.2.4-2` reaches it through GTK; `/usr/bin/inkscape` is a
  7-entry launcher stub whose GUI lives in
  `/usr/lib/inkscape/libinkscape_base.so`, which carries all three. The
  maintainer's own `~/.config/code-flags.conf` (287 bytes, 2026-07-21) already
  carries `--ozone-platform-hint=auto` and `--enable-wayland-ime`. Whatever
  fails about these under `vitrind`, it will not be X11.

### Games: a measured dependency of one machine, and not a commitment

The Steam bullet above records everything known about the **client**, which is
that its windowing path is unresolved. The **games** are not measured at all. Of
those seven app manifests exactly one is a game (appid 3949040,
`RV There Yet?`); the other six are Proton builds and the three Steam Linux
Runtimes. No game binary was inspected, and a Proton title's windowing path runs
through Wine inside a runtime container that no static scan of an installed
binary reaches. The one component of this stack that is unambiguously
Wayland-native is `gamescope 3.16.25-1`, which links `libwayland-client.so.0`
and `libwayland-server.so.0` directly — and which E3.2 already names as its own
prior art, for a reason that has nothing to do with running a game here.

**E3.2's exit criteria say nothing about games, and nothing in this list asks
them to.** A game additionally needs relative pointer motion, pointer
constraints, gamepads and GPU features far past anything E3.2 names. **The
first two are now on the wire and the third is not** (WS-E.4.2): version 2
carries `relative_motion` and a `pointer_constraint` ask-and-verdict pair, so
the two protocol pieces a game most obviously needs exist, while **gamepads
remain absent and nothing in WS-E.4.2 changes that** — no evdev gamepad node
exists on this machine, and no wire event carries one. (This paragraph read
*"no relative motion at all"* and then *"none of the first three is on the
wire today"*; the first was already false of the **core**, which has consumed
relative motion since #218, and the second stopped being true in the change
that landed WS-E.4.2. Both are corrected here rather than left standing.)
None of that makes a game runnable: the wire pieces are a necessary condition
and X11, GPU features and a gamepad path are all still missing.
Recording Steam here is a statement about what one laptop has installed.
It is not a statement that this project intends to run games, and it must not be
read or republished as a roadmap item.

### The interim, and what it costs

**The maintainer keeps a second session for X11-only software.** So the sentence
this whole workstream exists to be able to say — *"I did not have to reboot into
Hyprland"* — is **false for the named X11 set above**, and stays false until
E3.2 lands. Nothing in WS-E softens that; the interim is stated here because it
is what actually happens, confirmed by the maintainer on 2026-08-10 and
published as an accepted cost.

It is **a workaround the maintainer accepts, not a mitigation this project
offers**, and the difference is the whole point of writing it down. A mitigation
would reduce the exposure. This relocates the work to a place where none of the
exposure is measured:

- A second session on another VT runs **another compositor with full access to
  the same devices**. Nothing in this stack confines it. It is not a realm, it
  holds no grant, the core does not know it exists, and no capability this
  project enforces reaches it.
- **Switching VTs leaves the confined world entirely.** [D-031](20-decision-log.md#d-031--the-core-implements-ctrl-alt-fn-itself-because-refusing-to-is-what-trapped-the-human-d-030s-reasoning-stands-and-its-effect-was-its-own-opposite)
  has the core bind `Ctrl-Alt-F<n>` and call `change_vt` itself — superseding
  D-030(1)'s refusal, which on bare metal welded the hatch shut — and
  D-030(1)'s band scoping, one screen and one process, is untouched (§6). The
  interim is the reason a human presses that chord on purpose, repeatedly,
  every day.
- It applies to one person. It is not available to anyone evaluating this
  project as something to run, and it must never be published as though it were
  advice.

### Handoff to WS-E.4.4 (#224)

#221 publishes to `docs/plan/` and to `docs/book/src/limits.md` and stops there.
`README.md` and `site/index.html` were **not** edited by this issue — they are
#224's, so that the project's public claims are enumerated in one place rather
than drifting surface by surface. The exact text each surface must carry:

| Surface | The claim, as it must read |
|---|---|
| `README.md` | **No X11.** Wayland only. There is no X server anywhere in this stack, so `xterm` in a realm fails with `Can't open display` and every X11-only application on the maintainer's own machine fails with it. Per-app X11 with an embedded window manager is Phase 3 (E3.2). Until it lands the maintainer keeps a second session for X11-only software — a workaround he accepts, not something this project confines. The apps that *have* been run are listed, with the observable each run checked, in `docs/book/src/session-app-matrix.md`. |
| `site/index.html` | **No X11.** Wayland only: `xterm` in a realm fails with `Can't open display`. Per-app X11 is Phase 3. The maintainer runs a second, unconfined session for X11-only software in the meantime. |
| `docs/book/src/limits.md` | Landed by this issue, in the "No X11 shim" bullet. Reproduced in this table only so #224 can check that three surfaces say the same thing. |

Two constraints on that text, which are why it is dictated here rather than
summarised for #224 to re-word. **It must not say "Steam"**, for two independent
reasons: naming it on a public surface implies an intention this project has not
formed (§"Games"), and the measurement does not support the claim anyway — the
Steam client's windowing path came out *unknown*, not X11-only. **And it must
not call the second session a mitigation**; every surface says *workaround*, and
says whose. A surface that says "no X11 yet" and stops has published half of
this, and the half it dropped is the one a reader needs.

## 4.3 The five input classes the seat vocabulary drops: a verdict each

Stage 4's second deliverable (WS-E.4.2, issue #222), written out here for
§4.1's reason: the issue closes and this does not. Landed as
**[D-032](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)**.

`crates/vitrin-core::input::intake_physical`'s doc comment named five classes
it dropped at intake — *"touch, gestures, tablet, switches, relative motion"* —
and its `_ => Vec::new()` arm is what dropped them. **That sentence no longer
exists**: this section's change rewrote it, and the comment now names only two
classes as still dropped, each as a *not yet*. It is quoted in the past tense
because it is what these verdicts were taken against, not what the tree says.

Two of the five — **gestures** and **relative motion** — are **served** here,
and with them a sixth thing that comment does not name because it is not a
device class at all: **pointer constraints**. Two — **touch** and **tablet** —
are **deferred**, and a deferral here is not a polite spelling of a rejection:
each one names the evidence that would reopen it, because a permanent wire
protocol may not foreclose a device class on the ground that one laptop does
not have one. The fifth, **switches**, is not a seat-vocabulary question and
goes to WS-E.4.3.

**Citations in this section name symbols wherever a symbol exists.** Every
`file:line` in its first draft was checked at review and most resolved to
unrelated code. **The cause is not what it looks like:** checked against the
parent commit on 2026-08-10, nearly all of them were *correct when written*,
and **this change moved them** — it inserted well over a thousand lines into
the files it cites. A line number in a document describing a change is
invalidated by that change. A symbol name is not.

### How the device set was measured, and the one thing #222 asked for that does not exist

#222's first task is *"Measure first: `libinput list-devices` on the target
machine"*. **That command is not on this machine.** `libinput 1.31.3-1` is
installed and owns 75 files; `pacman -Ql libinput` resolves three udev helpers
under `/usr/lib/udev/` and **no `/usr/bin/libinput`** — Arch does not ship the
debug tools in this package, so the instrument the issue names cannot be run
here at all.

The measurement is therefore taken one layer below it, which is strictly more
than the CLI would have printed: **`/proc/bus/input/devices`, read on
2026-08-10**, with every `PROP=`, `KEY=`, `ABS=`, `REL=` and `SW=` bitmap
decoded to `input-event-codes.h` names rather than eyeballed. **Nothing was
launched** — no `vitrind`, no realm, no shim, no application under test — and
no input-injection tool of any kind was used, which on an input issue is the
rule that most wants restating (§7's hazard class; the one shared cursor is
the maintainer's).

**Twenty-eight evdev nodes, `event0`–`event27`.** What decides the verdicts:

| Finding | Evidence |
|---|---|
| **No touchscreen.** | **Not one node carries `INPUT_PROP_DIRECT`.** The only two nodes with any `PROP` bits at all are the two touchpads, both `POINTER \| BUTTONPAD` (`PROP=5`). A touchscreen is `INPUT_PROP_DIRECT` by definition, and nothing here has it. |
| **No tablet, no stylus.** | No node carries `BTN_TOOL_PEN`, `BTN_STYLUS`, `BTN_STYLUS2`, `BTN_TOOL_RUBBER`, `BTN_TOOL_BRUSH` or `BTN_TOOL_AIRBRUSH`, and no node carries `ABS_PRESSURE` together with a pen tool (the one `ABS_PRESSURE` on the machine is a touchpad's finger pressure). |
| **Two multi-finger touchpads, not one.** | `ELAN0305:00 04F3:31FD Touchpad` (`event15`, I²C-HID): `ABS_MT_SLOT`, `ABS_MT_POSITION_X/Y`, `ABS_MT_TRACKING_ID`, and `BTN_TOOL_DOUBLETAP`/`TRIPLETAP`/`QUADTAP`/**`QUINTTAP`** — five-finger detection. `ETPS/2 Elantech Touchpad` (`event25`, PS/2 serio): the same MT slots plus `ABS_MT_PRESSURE`, up to `QUADTAP`. #222's brief named only the first. |
| **Every pointing device on this machine is relative-only.** | `Logitech MX Ergo` (`event4`), `Logitech MX Keys` (`event6`) and `ELAN0305:00 ... Mouse` (`event14`) all report `REL_X`/`REL_Y` and **no `ABS_X`/`ABS_Y`**. There is not one absolute pointing device in the set. |
| **The lid switch is one of eleven `SW` devices.** | `Lid Switch` (`event0`, `SW` bit 0 = `SW_LID`). The other ten are audio-jack and HDMI presence detection on the ALSA nodes. |
| No gamepad, no joystick, no accelerometer. | No `INPUT_PROP_ACCELEROMETER`, no `BTN_GAMEPAD` block. |

Also present and not in #222's brief: two SteelSeries HID consumer-control
nodes (`event10`, `event11`), two Bluetooth AVRCP media-key nodes
(`Galaxy S22`, `Redmi Buds 6 Lite`), and `PC Speaker`. None of them changes a
verdict; they are recorded because the brief's device list was a subset and a
subset presented as the set is how a measurement stops being one.

**What this method cannot say**, stated in §4.2's register: capability bits are
what a device *advertises*, not what libinput will *emit*. That the ELAN
touchpad advertises five-finger MT slots is a necessary condition for
`GesturePinch*`/`GestureSwipe*` reaching the core and not a proof that they
will; only a run under `--drm` proves that, and no such run exists yet.

### The verdicts

| Class | Verdict | Turns on |
|---|---|---|
| **Relative motion** | **SERVED** | Every pointing device on this machine emits nothing else, and the DRM backend already consumes it |
| **Pointer constraints** | **SERVED — but not by a seat event** | A real `globals-demand` line, and a structural reason the verdict cannot ride this interface |
| **Gestures** (pinch, multi-finger swipe) | **SERVED** | Two five-finger touchpads present, and a real `globals-demand` line |
| **Touch** | **DEFERRED** — reopened by a touchscreen in the device set *and* an app that needs it | No `INPUT_PROP_DIRECT` device exists here |
| **Tablet** | **DEFERRED** — reopened by a tablet or stylus in the device set | No pen tool exists here, though the app-side demand already does |

Plus the sixth thing in that comment, which is not a seat-vocabulary question
at all: **the lid switch is handed to WS-E.4.3 (#223)**. Wayland clients never
receive switch events — the compositor consumes them, and on this machine
logind does — so growing a switch event would put a wire message under
something no app can use. #222 got this right and it is restated rather than
re-decided.

#### Relative motion — served, and the claim it corrects

**§4.2's line that v0 has *"no relative motion at all"* was already false when
it was written, and is corrected here rather than left standing.** Since
WS-E.3.2 (#218, `cf653eb`) the DRM backend's `handle_libinput` has had a
`PointerMotion` arm, which accumulates libinput's relative delta into an
absolute view position through `accumulate_pointer` and mints
`input::physical_motion`. What was true was narrower and was the actual gap:
**the core consumed relative motion and the wire could not carry it**, so an
app that wanted deltas — a 3D viewport, a drawing tool, anything that locks the
pointer — saw only the accumulated absolute position, and at the view edge saw
nothing at all.

That was the only one of the five classes whose *core* half was already
half-built, which is why it went first. **It is now whole.** The same
`PointerMotion` arm mints **two** wire events from one host event: the delta,
translated by `intake_physical` into `SeatInputKind::RelativeMotion`, and the
accumulated absolute position, which stays this backend's own novelty because
only the side that owns the output can hold and clamp a position. The IDL says
`relative_motion` *accompanies* `motion` rather than replacing it, so an app
binds whichever of the two it understands and never has to guess which one the
core sends. The order is delta first, then destination — what
`zwp_relative_pointer_v1` asks of a compositor.

#### Pointer constraints — served, and the surprise is where

The evidence the shim's own rule demands **already exists in this tree, from a
real run**: `shim/docs/globals-touched-firefox-140.12.0esr.log` carries
`globals-demand` lines for `zwp_pointer_constraints_v1` (`seq=80`) and
`zwp_relative_pointer_manager_v1` (`seq=81`), each summarised in the same file
as `class=probe advertised=1 binds=1 version_min=1 version_max=1 status=bound`
— Firefox 140.12.0esr bound both probes. `shim/src/globals.c`'s header comment
asks every addition to cite exactly such a line, and these two can.

`shim/docs/firefox.md`'s `zwp_pointer_constraints_v1` row records why they were
not served: *"a client that can warp or confine the pointer can invalidate what
the agent observed between observation and actuation."* **That objection is
answered rather than overruled, and the answer is one rule: a constraint binds
physical motion only.** An agent's actuation is minted absolute by the
chokepoint and routed by `InputRouter::route_emulated` to the realm its grant
names; it is never re-expressed as a delta and never clipped to a confinement
region. `route_physical` and `route_emulated` are two separately named entry
points precisely so a call site has to say which rule it is following, and the
constraint check is written on the physical arm only, gated on
`Origin::Physical`. So what an agent observed and where it then actuates stay
in one coordinate space, whatever the app has locked. The corollary is the
sharper half: **an app that locks the pointer must not thereby confine an
agent**, or a confined app would have acquired a way to trap a principal's
actuation.

**The structural finding, and it is the one that changed the shape of the
work: a pointer-constraint verdict cannot be a `vitrin_shim_seat` event.**
Backward requirement B2 makes `origin` the mandatory final argument of every
event on that interface — the RNG's `seat-event` define ends with a reference
to `origin-arg`, which pins the argument's name, type and enum — and `origin`
has exactly two values: `physical`, meaning a human device produced this, and
`emulated`, meaning a principal's actuator did. A constraint activation is
caused by **the confined app**, which is neither. Any tag it carried would be
false, and it would be false on the one interface whose entire design idea is
that the tag never drifts. The schema forecloses the obvious alternative too:
the `seat-interface` define admits only `seat-event` and `enum` children, so a
*request* on that interface is not merely unwise, it is inexpressible. So:

- the **ask** is a shim→core request on
  [`vitrin_shim_session`](../protocol/09-vitrin_shim_session.md) — the
  interface that already carries shim→core requests, and the only one that can;
- the **verdict** is a core→shim event on the same interface, beside
  `configure`, `request_selection` and `offer_selection`, none of which carries
  an origin either;
- what the **seat** gains for this class is only `relative_motion`, which is
  the input a lock actually delivers.

That is the clipboard's shape (WS-E.2.1) reused, and it is reuse rather than
resemblance: the core is again the party that decides, the shim is again the
party that asks, and the state again lives in the core where nothing outside it
can strand it.

**Why #222 did not see this coming, recorded because the issue's frame was
reasonable.** #222 asked one question five times: *the seat vocabulary drops
five classes, decide each*. Four of the five really are seat-vocabulary
questions — touch, tablet, switches and relative motion are all input events
with a physical origin. A pointer constraint is not an input event at all; it
is an application's **ask** and the core's **verdict**. The frame made the
mismatch invisible and the schema made it undeniable the moment anyone tried to
write the event down. The transferable lesson is small: *which interface does
this class grow?* is a question the RNG answers mechanically, and asking it
first is cheaper than discovering the answer in an IDL draft.

**The owner's decision, 2026-08-10: build it here.** The first draft of this
section left the whole constraint half downstream. Building only the input half
would have shipped `relative_motion` — whose serious consumer is a locked
pointer — with no way to lock a pointer, and would have left
`zwp_pointer_constraints_v1` advertised inert or not advertised at all. Both
halves land together.

**And the second owner decision, which is the one with a person on the other
end of it: while a constraint is active the core hides its own cursor sprite.**
The app cannot hide it — the core owns the sprite, which is why
`wp_cursor_shape_manager_v1` is not served (*"the shim has no cursor at all"*,
`shim/docs/firefox.md`) — so serving the lock without the hide would leave a
frozen arrow sitting over a game the human is aiming in.

> **THE UN-HIDE OBLIGATION IS THE SAFETY PROPERTY OF THIS ENTIRE CHANGE.**
> Every path on which a constraint ends must restore the sprite. **A single
> missed path leaves the human with no visible cursor on a display server they
> cannot exit**, on this project's only bare-metal machine. That is a worse
> outcome than any other defect this change could cause.

**So sprite visibility is a derived predicate and never a stored flag**, and
the argument is arithmetic rather than aesthetic. It is recomputed once per
frame at the single line in `backend::drm`'s `compose_and_queue` that decides
it — the one that feeds both the zero-copy and the CPU presentation paths —
and nothing else in the crate may write it. A flag toggled at N sites strands
the human's cursor by omission on the N+1st path; this workstream has already
learned that under a gentler penalty, when `forget_presence_of` was a caller's
obligation for exactly one review cycle before the realm-death funnel, which
did not know it existed, became its third caller. A per-frame predicate cannot
be stranded by omission at all. It can only be stranded by a stuck *record* —
and a stuck record still cannot hide the sprite unless the realm is focused, no
overlay is up, and the output is active.

That shape also buys the **reactivation** half for free, which a flag would
have had to remember: Wayland's `persistent` lifetime requires a lock to become
active again when its surface regains focus, so a flag needs an un-hide on
switch-away *and* a re-hide on switch-back — two sites, one of which gets
forgotten.

**The deactivation paths, split by whether they need code at all.** This split
*is* the design; the table is not a checklist bolted onto it.

| Class | Paths | What it costs in code |
|---|---|---|
| **Record removal** | the app withdraws (`kind = none`); the realm or its shim dies; the seat is paused (VT switch, suspend); the dead-man switch fires; the session tears down; a second ask supersedes the first | Five edits, each at a funnel that already exists. Realm and shim death go **inside `InputRouter::reset_for`**, not at its callers, because `lifecycle::die` reaches teardown by two arms and only the inside covers both |
| **Transient** | the realm is not focused; a consent card, the lock screen, the dead-man hold or the core notice is up; the surface is uncommitted; the output is inactive; the pointer is outside the region; the shim never minted its seat | **No code.** Each is already a term the predicate reads. This is the entire return on deriving rather than storing |

**Ranked, because the two effects a constraint has are not equally dangerous.**
A constraint also **freezes** the absolute position the core's own hit tests
use. The freeze ranks strictly *below* the sprite: a frozen visible cursor
looks like a hung compositor but leaves the human every escape they had, while
a hidden one does not. An implementation that has to retreat from one retreats
from the freeze.

**Why the core still wins, argued rather than assumed.** A client that locks
and hides the pointer is, on the face of it, a client that stops the pointer
leaving — which would be a confinement-relevant verb wearing a cosmetic one's
clothes. Six reasons it is not. Every citation below was re-derived by opening
the file on 2026-08-10; in this section's first draft, all of the first four
missed.

1. **The dead-man chord is untouched by construction.** The tap is
   `hook.observe(&input)` inside `InputRouter::route_into`, called
   unconditionally one statement *before* `hook.gate` and therefore above every
   gate. `DeadManHook::observe` reaches `DeadManSwitch::observe_event`, whose
   second statement destructures `SeatInputKind::Key` and returns on anything
   else — it is a *keyboard* chord and no pointer state is on its path. The
   unconditionality is structural: `GateOnlyHook::observe` forwards to its
   inner hook with no gate consulted, and the `ConsumingGate` trait it wraps
   declares **no observation method at all**, so an edit inside `crate::lock`
   cannot make observation conditional because the trait it calls through
   cannot express one. A constraint sits below both and reaches neither.
2. **The consent grab runs before the constraint is ever consulted.** The hook
   point is *after* origin binding and *before* coordinate mapping and
   hit-testing — the argument is in `crates/vitrin-core/src/input`'s module
   docs under *"The preemption hook"*, the one citation from this section's
   first draft that still resolved when checked. A constraint is a delivery
   decision inside `route_into`'s per-kind match, reached only after
   `hook.gate` declines to consume and after the realm resolves. The module
   docs already make this exact argument for the letterbox matte — *"a gate
   that ran after the app hit-test could be dodged by parking the pointer off
   the surface"* — and a lock is that dodge with a protocol behind it, landing
   on the wrong side of the hook point to attempt it. **And the belt:** the
   predicate goes false while a prompt is up, so during a consent round the
   pointer is not merely un-hidden but un-frozen. A locked app cannot make a
   consent card unanswerable, which is the failure that would actually matter.
3. **The core owns the constraint state and ends it alone.** The shim's request
   is an ask, not a fact. Record removal lives at `InputRouter::bind_to`,
   `session::suspend_physical_seat`, `InputRouter::reset_for` and
   `Runtime::apply_dead_man`; everything else in the table above is transient
   and needs no site.
4. **The app cannot hide the cursor, because it never had it** — which is
   exactly why the hide had to be decided rather than left open: the capability
   the app is asking for is one only the core can perform.
5. **The VT escape is outside all of it.** `VtHook` is the outermost member of
   the hook stack and `Ctrl-Alt-F<n>` is a keyboard chord ([D-031](20-decision-log.md#d-031--the-core-implements-ctrl-alt-fn-itself-because-refusing-to-is-what-trapped-the-human-d-030s-reasoning-stands-and-its-effect-was-its-own-opposite)).
   A locked, frozen, sprite-less pointer is not on its path at any layer. This
   reason was absent from the first draft and it is the maintainer's actual
   last resort on this machine.
6. **The core's own hit test is unmoved** — [00-conventions.md](../protocol/00-conventions.md)
   §1.4 invariant 2: *the core's own hit test, never a client's claimed
   stacking, decides which surface an input event reaches*. A constraint
   changes only what the **app** is told. The core's accumulated position, the
   consent card's recorded pointer and the lock screen's passphrase path are
   all upstream of it, and the freeze is applied where the app-facing position
   is *minted* rather than by rewriting any of them. The region check reuses
   the router's existing `pointer_over_surface`, because a second hit test
   written here would be a second answer to a question invariant 2 says has
   one.

**The blast radius, stated so nobody mistakes green CI for evidence.** The
human sprite exists only on bare metal: `backend::winit`'s `window_pixels` is
handed `None` for it on nested and headless, because the host desktop draws the
pointer there. **So no test in this workspace can strand a cursor even if the
logic is wrong**, and CI has no DRM device. Immune everywhere it is cheap to
test, dangerous in the one place it is not — the shape that lets a defect ship
green, and the reason the mitigation has to be a named integration rung on the
target machine, skipped with a stated reason when no DRM device is present,
rather than a unit test.

#### Gestures — served, at pinch and multi-finger swipe

Two-finger scroll is **already served** — libinput reports it as an axis event
and `intake_physical` converts pixels to v120 at the documented rate — so the
gap is pinch and multi-finger swipe, which is a smaller and more honest claim
than "no gestures". #222 states this correctly and it survives verification.

The demand evidence is real and doubled: the evidence log carries two
`globals-demand` lines for `zwp_pointer_gestures_v1` (`seq=36` at
`version_requested=1`, `seq=82` at `version_requested=3`) and summarises them
`binds=2 version_min=1 version_max=3 status=bound`. The device evidence is the
two touchpads above.

**Hold is advertised and never sent, and the global goes out at wlroots' own
version, which is 3.** This paragraph said the opposite — that the global would
be advertised at **version 2**, serving swipe and pinch completely and claiming
nothing about hold — and the shipped `shim/src/globals.c` did the opposite of
what it said. The paragraph was wrong, for a reason checked against this
machine rather than reasoned about:

- `wlr_pointer_gestures_v1_create(struct wl_display *display)` takes a display
  and **no version**, so capping the advertisement at 2 is not expressible
  through wlroots' helper at all. Verified in
  `/usr/include/wlroots-0.19/wlr/types/wlr_pointer_gestures_v1.h` (package
  `wlroots0.19 0.19.3-1`) on 2026-08-10.
- The same header declares `wlr_pointer_gestures_v1_send_hold_begin`/`_hold_end`
  and a `holds` resource list, and `get_hold_gesture` is `since="3"` in
  `/usr/share/wayland-protocols/unstable/pointer-gestures/pointer-gestures-unstable-v1.xml`
  (interface `version="3"`). A wlroots that implements hold advertises 3.

**That is not the half-serving `shim/src/globals.c` refuses elsewhere**, and
the distinction is the whole answer: the three gesture families live behind
**one** global, so declining hold would decline swipe and pinch with it, and a
client learns which gestures exist from **the events it receives** rather than
from the global — unlike a `wl_seat` capability, which is a positive claim a
toolkit changes its fallbacks on. Firefox's own two binds, one at version 1 and
one at version 3, are the same point from the application side. Hold reopens on
the same terms as anything else here: an application that binds version 3 *and*
does something with a hold.

#### Touch — deferred, and what would reopen it

**No device on this machine can produce a touch event**, and that is a fact
about one laptop rather than about the class. Everything below follows from
that and from nothing else.

`shim/src/globals.c`'s seat-capability comment is the precedent and **it
stays**: the shim advertises `WL_SEAT_CAPABILITY_POINTER | KEYBOARD` and adds
no `wl_touch`, because *"a `wl_touch` bound here would have nothing behind it,
and advertising a capability the shim cannot honour is worse than not
advertising it: a toolkit that sees TOUCH stops installing its pointer
fallbacks."* Do not half-serve a class. WS-E.4.2 rewrote that comment so it
states the narrower claim it always meant — the heading now reads `TOUCH IS NOT
YET SERVED`, which is the register every surface in this repository owes this
class. `wl_touch` is the one class here with no `globals-demand` line possible
either way, because touch is a `wl_seat` capability rather than a global, so
the ledger cannot record a demand for it even in principle.

**What reopens it:** a machine with a touchscreen (`INPUT_PROP_DIRECT`) in the
measured device set, **and** an application in the session matrix that needs
it. Both, because a device with no app that wants it buys a permanent wire
surface for nobody, and an app that wants it on a machine that cannot produce
it cannot be tested. The device half is a one-line re-run of the measurement
above.

#### Tablet — deferred, and the asymmetry is worth seeing

**No pen tool exists on this machine** — and unlike touch, **the app-side
demand already does**: the evidence log carries a `globals-demand` line for
`zwp_tablet_manager_v2` (`seq=39`, `version_requested=1`). Firefox binds the
tablet manager whether or not a tablet is plugged in, which is why that line is
evidence about GTK and not about this laptop. `shim/docs/firefox.md`'s
`zwp_tablet_manager_v2` row states the class as **not yet served**, on the same
not-half-serving rule that keeps `wl_touch` out of the seat capabilities, and
that row stays provisional by design.

**What reopens it:** a tablet or stylus device in the measured set — any node
carrying `BTN_TOOL_PEN` or `BTN_STYLUS`. The app half of the evidence is
already banked, so this deferral turns on the device alone, and it is the
class most likely to reopen first because a graphics tablet is a thing a person
buys rather than a thing a laptop has.

### What #222 asserted about today's code that is no longer true

#222 was filed **2026-08-06T08:35Z**, before WS-E Stage 2 (2026-08-08) and
Stage 3 (2026-08-09). Every "what exists today" claim in it predates two
stages, and this is the list, checked file by file rather than restated.

**The right-hand column names symbols, not lines, and that is a correction.**
Its first draft answered every stale line number with a fresh line number —
which is how this section then went stale a second time inside the same
change, since WS-E.4.2 inserted well over a thousand lines into the very files
it cites. The issue's own numbers are kept on the left because they are what
was written; the answer is a name, which survives the next shift.

| The issue says | The tree says (checked 2026-08-10, after WS-E.4.2) |
|---|---|
| `input/mod.rs:1049-1051` (dropped classes), `:1119` (the drop arm) | Both had moved ~1 100 lines by the time the issue was picked up, to `intake_physical`'s doc comment and its `_ => Vec::new()` arm. **The comment's text is now gone as well**: WS-E.4.2 rewrote it, and it no longer names five dropped classes — only touch and tablet, each as a *not yet* |
| `input/mod.rs:105-109` — *"per-keysym pairing and the warning that a real keymap moves pairing to scancodes"* | **The warning is discharged.** The module docs' section on what a press pairs *by* now records that pairing moved to `KeySource` (the scancode) in WS-E.3.1/#217 under [D-028](20-decision-log.md#d-028--a-bare-metal-session-interprets-the-keyboard-inside-the-core-from-a-pre-compiled-keymap-file-and-key-pairing-moves-to-the-scancode). Citing it as a live warning would have re-decided a decision |
| `input/mod.rs:482-490` — the `PreemptionHook` contract | Those lines are `SeatInput::physical`/`emulated`. The contract is the `PreemptionHook` trait and its doc prose; the single hook call is `hook.observe` inside `InputRouter::route_into`, one statement above `hook.gate` |
| `input/mod.rs:1239-1271` — `invariant_keysym` | The function still exists under that name; it has moved twice since the issue was filed |
| `protocol/vitrin-v0.rng:177,181` — the schema-enforced last-argument rule | The rule is the `seat-event` define's trailing reference to `origin-arg`, and the interface split is the `seat-interface` define. The rule itself is exactly as described and was re-verified in the schema, not taken from the issue. (This section's first draft mis-numbered the interface define by four lines, which is the whole argument for naming defines instead) |
| *"v0's seat vocabulary is pointer + keyboard … no relative motion"* | Was true of the **wire** and false of the **core** since #218. **Now false of both**: `relative_motion` is on the wire as of WS-E.4.2 |
| New seat events must *"either ride P2.1.2's version-2 landing or wait for the version after the P2.9.2 spec freeze"* | Version 2 landed **2026-08-06** in WS-E.1.1 (`6abe8dd`) and had been appended to three times before this change (`4fdcab6`, `53cee3a`, `2f7c7cf`), for eleven `since="2"` messages; WS-E.4.2 adds seven more, for eighteen. There was nothing left to ride and nothing to wait for (below) |
| `protocol/vitrin-v0.xml:1176` — `vitrin_shim_seat` | The interface had five events when the issue was filed, exactly as claimed — `motion`, `button`, `scroll`, `key`, `text` — verified. It now has ten |
| `shim/src/globals.c:185-192` — the touch choice and `wlr_seat_set_capabilities` | **The one issue citation that had not moved when checked — and WS-E.4.2 then moved it**, by inserting the version-2 seat paragraph above it. Both are now found by the comment heading `TOUCH IS NOT YET SERVED` and by the `wlr_seat_set_capabilities` call under it |
| `libinput list-devices` on the target machine | The command does not exist here (above) |

### Version scheduling: there is nothing to schedule

#222 treats the version as a hard constraint, citing
[02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md):303 — *"Nothing
may bump to version 3: P2.1.2 owns the single bump for the whole phase."* That
line is **half stale, and the same document already fixed the half that
matters**: `:383-390` says in as many words that *"the 1→2 version bump is
owned by whoever lands first, not by P2.1.2 by name … If it does, it performs
the bump and P2.1.2 rides it; the invariant that actually matters is unchanged
— one bump, and every later addition at `since="2"`."* WS-E.1.1 did land first.
So `:303`'s literal *"P2.1.2 — the protocol 1→2 bump"* now describes a bump
that has already happened, while *"nothing may bump to version 3"* is untouched
and is not tested by this work.

**That paragraph belongs to the Phase-2 plan and this workstream does not edit
it.** A WS-E issue rewriting the Phase-2 schedule is the competing roadmap the
tracking model forbids; the exact text is quoted here for whoever owns
[02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md).

Everything else about the version question is settled in
[D-032](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)(6),
including the one finding that would have stopped this work if it had gone the
other way: **version 2 has never been released, frozen or negotiated by
anything outside this repository**, so appending `since="2"` siblings to it is
ordinary additive growth rather than a silent redefinition of a shipped
version.

### The wire shape, and where each arm attaches

Five new `since="2"` sibling events on `vitrin_shim_seat`, event opcodes
**5–9** (document order; `motion`=0 … `text`=4), each ending with `origin`
because the schema will not accept it otherwise:

| Event | Signature |
|---|---|
| `relative_motion` | `(dx: fixed, dy: fixed, dx_unaccel: fixed, dy_unaccel: fixed, origin)` |
| `gesture_begin` | `(kind: uint enum gesture_kind, fingers: uint, origin)` |
| `gesture_swipe_update` | `(dx: fixed, dy: fixed, origin)` |
| `gesture_pinch_update` | `(dx: fixed, dy: fixed, scale: fixed, rotation: fixed, origin)` |
| `gesture_end` | `(kind: uint enum gesture_kind, state: uint enum gesture_state, origin)` |

plus two enums on the same interface, `gesture_kind {swipe=0, pinch=1}` and
`gesture_state {completed=0, cancelled=1}`. Swipe and pinch **share** their
begin and end because those two signatures are identical in Wayland's own
gesture protocol, and a signature is immutable forever — four events with no
dead argument beat six with duplicated ones, and beat two phase-tagged events
whose deltas and `cancelled` flag would each be meaningless in two phases out
of three.

**No timestamp argument, deliberately.** The five existing seat events carry
none, and `shim/src/seat.c` stamps each replay with its own `now_msec()`. A
device timestamp on the wire would put a second, unsynchronised clock beside
that one — which is the reason `wp_presentation` is not served
(`shim/docs/firefox.md`) — so the cost is paid instead: a consumer integrating
deltas over `dt` gets the shim's arrival time, not the device's event time.

**And the pointer-constraint pair on `vitrin_shim_session`**, whose signatures
are normative in the IDL and on [page 09](../protocol/09-vitrin_shim_session.md)
and are not restated here: a shim→core request `pointer_constraint(serial,
surface, kind, lifetime, x, y, width, height)` and a core→shim event
`pointer_constraint_state(serial, state)`, plus `pointer_constraint_kind`,
`pointer_constraint_lifetime` and `pointer_constraint_status`. Three shape
choices are worth carrying here because they are the ones a reader will
question:

- **One message, not `set`/`unset` siblings.** `kind = none` is the
  withdrawal, so the core's state machine has exactly one input and a
  withdrawal cannot race a set.
- **`inactive = 0`, departing from `selection_status`'s `ok = 0`.** Zero is
  where a mis-decode and a zeroed struct land, and *"not constrained"* is the
  safe reading of a byte nobody can trust.
- **Fire-and-forget, not reply-bearing.** A constraint's state changes for
  reasons the shim never asked about — the human switched realms — so binding
  the answer 1:1 to the ask would leave an app locked with no message that
  could ever tell it otherwise, which is the exact latch this design exists to
  prevent. The core sends at most one state per transition and never coalesces
  two different states.

**Every arm attaches at the one hook point, and by the one mechanism.** Each
input class becomes a new `SeatInputKind` variant, and `InputRouter::route_into`
takes a `SeatInput` — so `presence.note`, `hook.observe` and `hook.gate` see it
before any mapping, without a line of new plumbing. Nothing can route around
the hook without constructing a `SeatDelivery` directly, and `SeatDelivery`'s
construction sites are all inside `InputRouter`. Concretely:

- `intake_physical` gains `InputEvent::PointerMotion` and the six
  `Gesture{Swipe,Pinch}{Begin,Update,End}` arms, all before its
  `_ => Vec::new()` arm;
- `backend/drm.rs`'s `PointerMotion` arm returns **two** `SeatInput`s — the
  accumulated absolute motion it already mints, and the raw delta — so both
  pass the same tap. It must not keep intercepting the class privately;
- `SeatDeliveryKind`, `SeatDelivery::encode` and `event_label` gain one arm
  each, all three exhaustive with no catch-all, so a kind that forgets its
  recorder label does not compile.

**Pairing, and how a dropped end is stopped from latching the app.** A
`gesture_begin` with no `gesture_end` is the latched-modifier bug in a new
shape, and the razor is the one `pressed`/`pressed_keys` already enforce:

- `RealmSeat` holds at most one in-flight gesture, per realm like everything
  else in it. A second `begin` while one is live is dropped and traced rather
  than trusted away — libinput will not produce one, and the core is not in the
  business of believing that.
- An `update` or an `end` is delivered **iff its own `begin` was delivered**,
  exactly as *"a release is delivered iff its own press was"*. A gate-consumed
  `begin` therefore starts nothing, and its updates and end are dropped.
- A consumed `end` whose `begin` **was** delivered is reconciled the way a
  consumed release is: bookkeeping cleared, nothing on the wire, and the app
  left mid-gesture — the gate implementor's debt. The pairing contract on
  `PreemptionHook` gained one sentence saying a gate that begins consuming
  mid-gesture should keep answering `Gate::Deliver` for `gesture_end`, for the
  reason it already gives for releases.
- **A drain**, `InputRouter::end_physical_gesture`, sibling to
  `release_physical_keys` and `release_physical_buttons`. It emits
  `gesture_end(kind, cancelled)` — cancelled rather than completed, because the
  human did not finish it. §6's *"a realm switch mid-gesture tells the app the
  human let go"* limit already names this exact trade for keys and buttons; a
  touchpad gesture joins it rather than inventing a new one.

**The drain runs on two paths, and this section first claimed five.** The
correction matters more than most, because the over-claim reached the IDL,
where a `<description>` is normative and outranks every prose page. The two
real ones are `InputRouter::bind_to` (a realm switch) and
`session::suspend_physical_seat` (a seat pause); on both, the human's fingers
are still down and no end can ever arrive, so one is minted rather than waited
for. Of the three that were claimed:

- **a consent prompt** and **a raised screen lock** mint nothing. Both gates
  answer `Gate::Deliver` for `GestureEnd` — checked in `ConsentGrab`'s and
  `LockGate`'s judge arms on 2026-08-10 — so they withhold the *updates* and
  then deliver the device's own end when the human lifts. No latch forms, so
  this is a gap rather than a defect, but an app that was previewing a zoom is
  told the human **completed** what they in fact abandoned behind a card they
  could not see past. Closing it needs `scenes` in reach of the consent-round
  service point and is owed as a separate change.
- **the dead-man switch** never took physical input away in the first place; it
  revokes grants, so that clause was vacuous.

`NestedState::handle_focus` is a fourth non-caller and a *deliberate* one,
documented where it sits: a gesture is pointer-side, so it follows the buttons'
exclusion rather than the keys' inclusion — and smithay's winit backend
surfaces no gesture events at all, so a nested session can never have one in
flight.

`relative_motion` needs no pairing of its own — it has no begin and no end.
Its latch shape lives in the constraint instead: while one is active the core
stops emitting absolute `motion` and freezes the position its own hit tests
use, so a constraint whose end is lost would be a pointer that never moves
again. That is why the constraint's state is core-owned, why its removal lives
*inside* `InputRouter::reset_for` rather than at each caller, and why an
**emulated** motion must not move that frozen position while a constraint is
active — the defensive rule `pointer_constraint_state`'s IDL description
already states, now with a second reason to need it.

**Allocation, checked repo-wide** (§5 of
[02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) is the registry
and this consumes nothing in it): no verb bit — `Verb::VALID_MASK` stays
**575** — because nothing agent-facing is added; no new prose page, since these
extend [page 11](../protocol/11-vitrin_shim_seat.md) and
[page 09](../protocol/09-vitrin_shim_session.md), and `docs/protocol/`'s 12–15
stay reserved; event opcodes 5–9 on `vitrin_shim_seat` were unclaimed, request
opcode 3 and event opcode 3 on `vitrin_shim_session` likewise, and the three
Appendix-A seams that will also want seat opcodes (focus, the keymap relay plus
keycode, per-principal delivery) reserve **no numbers**, so whoever lands
`focus` next starts at 10 rather than at 5. `vitrin_shim_seat`'s own `version`
attribute moves 1 → 2, on the precedent of `vitrin_principal`, `vitrin_grant`
and `vitrin_shim_session`, each of which bumped its counter in the commit that
gave it a `since="2"` message.

**One name is already spoken for.** `constraint` in this protocol means a
*petition* constraint — `request_grant`'s `flags`, and Appendix A's
`set_constraint` builder row. The pointer variety must always be qualified
`pointer_constraint`, in the IDL, in prose, in Rust and in C, or two unrelated
capabilities share a noun in a document that cannot rename either.

### Handoff

- **WS-E.4.3 (#223)** takes the lid switch, unchanged from #222's reading.
  **Taken** — [§4.4](#44-session-lifecycle-build-what-the-hardware-forces-delegate-the-rest)
  delegates it to logind rather than growing a wire event, and confirms on the
  machine that the lid is `SW_LID` on `event0`, that `vitrind` sees it, and that
  `intake_physical` drops it.
- **WS-E.4.4 (#224)** publishes the deferrals. The register is dictated here
  for §4.2's reason: every surface states touch and tablet as **not yet
  served** and names the evidence that reopens each. §6's bullet is rewritten
  below on the same rule.
- **The paired IDL + prose edit, the core arms, the shim replay and the globals
  change landed with this section** (WS-E.4.2), and so — on the owner's
  decision of 2026-08-10 — did the pointer-constraint half that this section's
  first draft left downstream. **What has not happened is a run.** No gesture,
  no relative-motion event and no constraint verdict has yet reached a
  connected application on real hardware, and the cursor-sprite property is
  unreachable from every backend CI can execute. Until a named
  `tests/integration/test_real_*.py` rung runs on the target machine under a
  documented runbook and its result is dated, the status of everything in this
  section is **landed in the tree, unproven on hardware** — the same status
  Stage 3's DRM work had to carry, for the same reason.

  > **EXECUTED 2026-08-13, by a different instrument than this bullet named, and
  > the bullet above is left as written.** What ran is not a
  > `tests/integration/test_real_*.py` rung on the target machine: it is
  > [`docs/drm-bringup.md`](../drm-bringup.md) step 13a, a manual runbook rung,
  > and the substitution is recorded here rather than made silently.
  > `tests/integration/test_real_gestures.py` does exist and covers everything
  > from `input::intake_physical` onward by injection — which is why it cannot
  > be the witness for this bullet, since libinput's own classification of three
  > fingers, and the cursor sprite, sit *before* and *outside* that entry point
  > and are unreachable from every backend CI can execute. The maintainer ran
  > 13a on 2026-08-13 on the target laptop, on tty3, across three sessions,
  > against `shim/tests/gesture_probe.c` — this repo's own witness client, a
  > real Wayland client under the real shim, not a third-party app and not a
  > mock. `13a-i` relative motion, `13a-ii` three-finger swipe, `13a-iii`
  > two-finger pinch, `13a-iv` two-finger scroll and `13a-vi` the pointer lock,
  > including the cursor-sprite half, all PASS. `13a-v` is the one that did not:
  > a gesture interrupted by a VT switch arrives `completed` where the core owes
  > `cancelled`, because libinput flushes the in-flight swipe before
  > `session::suspend_physical_seat` runs. That is
  > [#275](https://github.com/vitrin-os/vitrin-os/issues/275), still open, and
  > it joins the Owed list below rather than replacing anything on it. So the
  > status of this section is no longer *"landed in the tree, unproven on
  > hardware"*; it is *"landed, exercised once on one laptop by one purpose-built
  > client, with one recorded defect still open"*. Of the "Owed" bullet below,
  > the hardware rung is discharged in substance by that run; cancelling an
  > in-flight gesture when a consent card or the lock screen raises is not —
  > `ConsentGrab` and `LockGate` still answer `Gate::Deliver` for `GestureEnd`
  > (`consent/grab.rs`, `lock/gate.rs`), exactly as written above.

- **Owed, and named rather than smoothed over:** cancelling an in-flight
  gesture when a consent card or the lock screen raises (above); and the
  hardware rung itself.

## 4.4 Session lifecycle: build what the hardware forces, delegate the rest

Stage 4's third deliverable (WS-E.4.3, issue #223), written out here for §4.1's
reason: the issue closes and this does not. Landed as
**[D-033](20-decision-log.md#d-033--idle-blanks-the-screen-and-does-not-lock-it-suspend-is-detected-after-the-fact-or-not-at-all-and-the-recovery-path-is-sudo-only)**.

On the nested and headless backends none of this exists to decide: `winit.rs` is
a client of a compositor that owns suspend, blanking and VT switching entirely,
and `headless.rs` has no output at all. On bare DRM the core becomes the thing
that owns the display and the devices, so a subset of session management is
**forced** on it. This section draws that line: build what the hardware forces,
delegate policy to systemd-logind, and defer the rest against named evidence.

### What #223 asserted about today's code that was no longer true

#223 was filed **2026-08-06T08:35Z**, before Stage 2 (2026-08-08), Stage 3
(2026-08-09) and WS-E.4.1/4.2 (2026-08-10). Five of its claims were checked
against the tree on 2026-08-10 by opening the file, and did not survive. They
are recorded because a stale issue acted on faithfully builds the wrong thing:

| #223 said | The tree said, 2026-08-10 |
|---|---|
| *"There is no lock screen and none is built here … Published as a limit"* | **False.** A full core-drawn lock screen landed in WS-E.2.2/#214 (`crates/vitrin-core/src/lock/`: `LockScreen`, `LockPolicy`, Argon2id passphrase, golden render) and was exercised end to end on bare metal — `session_locked(chord)` → `unlock_attempted(true)` → `session_unlocked`, first-run record in [`docs/drm-bringup.md`](../drm-bringup.md). Publishing *"no lock screen"* would have been a false limit in the pessimistic direction, which is still false. |
| Task 1, *"Activation transitions … drop DRM master and stop presenting on deactivate; on activate re-acquire"* | **Already built**, and more thoroughly than the task describes: `DrmState::handle_session_event` handles both arms, including the libinput suspend/resume, the held-press drain, the chord matchers forgetting physical state, the consent-guard restart and the idle-clock freeze. Confirmed on hardware — two full pause→activate cycles, master dropped and reclaimed (second-run record). |
| Task 1's *"and its `PauseDevice`/`ResumeDevice` passthrough"* | **Nothing to wire.** libseat's listener has exactly two callbacks, `enable_seat` and `disable_seat` (`/usr/include/libseat.h`), and smithay 0.7 collapses them into `SessionEvent::{PauseSession, ActivateSession}` and exposes nothing else. |
| Task 2, *"Route post-suspend recovery through the same reactivation path as VT switch, so there is one code path and not two"* | **The premise has no producer.** libseat does not handle `PrepareForSleep` at all; `disable_seat` fires on the logind session going inactive or on a `PauseDevice` signal, and a system suspend emits neither. A suspend/resume delivers **no session event**, so there is nothing to route and the sentence is vacuous as written. What replaces it is below. |
| *"An idle timer on the existing `calloop` loop"* | An idle timer **already exists** and is deliberately not a `calloop` timer: `--lock-idle` drives `LockScreen::tick`, evaluated once per dispatch round from `post_dispatch`, whose own docs give the rule — *"a second clock would be a second thing to keep in step"*. The blank inherits that mechanism rather than adding a second one. |

Two of #223's References were also line-drifted rather than wrong
(`session.rs:327,472` for `Presenter`/`RuntimeHost`; `input/mod.rs:1239-1271`
for `invariant_keysym`), and one — `lifecycle.rs:130-170`, cited as *"the
`calloop` signal source a timer joins"* — points at module prose describing the
realm shutdown ladder, which is not a signal source and which no timer joins.
**Citations in this section name symbols wherever a symbol exists**, for the
reason [§4.3](#43-the-five-input-classes-the-seat-vocabulary-drops-a-verdict-each)
gives: a line number in a document describing a change is invalidated by that
change; a symbol name is not.

### Decision 1, the owner's: idle blanks the screen, and does not lock it

**Taken by the owner on 2026-08-10 and not re-litigated here.** On idle the
panel goes dark; the session stays **unlocked**. Locking remains the human's
manual chord (`Ctrl-Alt-Delete`), which already exists. There is no idle-lock
timer added here and the two are **not coupled**.

The consequence is real and is published unsoftened rather than argued down: **an
unlocked session behind a dark screen.** Anyone who walks up and touches a key
gets the session, not a passphrase prompt. That is worse than what the machine
does today under Hyprland, and it is the owner's trade to make.

The two clocks share exactly one thing — the activity timestamp — and nothing
else. In particular the blank must **not** suppress the idle lock: a session run
with a blank timeout shorter than a lock timeout must still lock, or the blank
would have silently disabled a security feature, which is precisely the class of
unchosen behaviour [D-030(2)](20-decision-log.md#d-030--the-trusted-band-asserts-only-about-the-screen-this-core-is-driving-and-the-session-colour-is-never-re-minted-a-paused-seat-raises-no-prompt-it-could-record-as-shown)
was written to catch. Blanking while unlocked and then locking behind the dark
screen is the correct sequence: the human touches a key, the wake is consumed,
and the screen returns showing the lock card.

### The activity rules, as a stated policy

One writer, one clock. The timestamp is written at exactly **one** site — the
line already in `LockScreen::judge` — and read by two independent ticks. Two
clocks meaning *"when did the human last touch this session"* would drift, and
the drift would be invisible.

- **Postpones the blank:** any seat input whose origin is **physical** — key,
  button, motion, scroll, relative motion, gesture begin/update/end, text;
  presses *and* releases. Nothing else. So a human who only moves the mouse
  postpones the blank.
- **Wakes the screen:** the same predicate, at the same site. Any physical
  event.
- **An agent's actuation does neither**, and the postpone half was already
  decided by landed code rather than being restated here: `LockScreen::judge`
  returns early for non-physical origin *above* the stamp, commented *"an
  agent's actuations must not hold the idle lock open for a human who left"* and
  pinned by `an_agents_actuation_never_holds_the_idle_lock_open`. Sharing the
  clock makes the blank inherit it structurally.
- **The independent argument for the wake half** is the stronger one: **there is
  no verb in the IDL for "power the human's display".** An agent that could wake
  the screen would be making an unrequested change to the human's physical
  environment, remotely triggerable, under no grant — the same shape
  [D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing)
  refuses for the clipboard, and the inverse of what D-019 buys (an agent's
  action must be *visible when a human is there*, not *summoning*). An agent
  that could merely keep the screen awake is milder and still spends the human's
  battery and lights an empty room under no authority.
- **The counter-argument, stated and rejected:** *"an agent doing visible work
  should keep the screen on so the human can watch."* A human who wants to watch
  is present and touching things; a human who is not present is not watching.
  D-030(6) already publishes that agents keep working while the human's screen is
  not showing them.
- **The core's own drawing does neither**, and it is enumerated rather than
  implied, because *"the core drew something"* is exactly how a wake rule leaks:
  a **consent card raising** must not wake (a core that woke the screen because
  an agent petitioned would hand every principal a remote wake primitive, and
  petitioning needs no grant at all); the **status strip's minute rollover** must
  not wake (a session that woke itself once a minute would never blank); a
  **realm's commit, a realm launch, an SDK client's layout verb** must not wake
  (an app painting is not the human). The **dead-man chord** and the **VT chord**
  need no rule of their own — both are held physical keys, so the first press
  wakes by the ordinary predicate.
- **The wake event is consumed**, so a press aimed at a dark screen neither
  commits a consent card nor acts inside a confined app — and it is consumed
  from the moment the blank begins until the first frame after the wake lands,
  not for one event, because a modeset is long enough for a human to type several
  characters into an app they cannot see. **Bounded**, so that a dark screen that
  also swallows input can never become indistinguishable from a wedged session:
  after the bound, input is delivered regardless and the fault is logged. *Fail
  open on input, fail closed on authority* — the consent guard restart is what
  holds the security property, so the consume is defence in depth and may be lost
  to the bound without opening a clickjack.

### Suspend: what the core can actually observe, which is less than #223 assumed

**No D-Bus client goes into the core.** logind's `PrepareForSleep` is the
textbook way to prepare for suspend and taking it means a message bus and its
dependency tree inside the TCB, while
[D-020](20-decision-log.md#d-020--the-realm-boundary-is-a-namespace-boundary-intra-user-by-default-in-namespace-uidgid-and-a-residue-that-lives-outside-every-realm)
and #160 exist to *remove* ambient bus access from realms. That decision stands
and is not re-argued.

What #223 did not know is that **libseat delivers nothing on suspend either**
(above), so *"one code path and not two"* had no second path to unify. The only
fact the core already samples that a suspend perturbs is the **clock pair** it
takes once per round: `Instant` is `CLOCK_MONOTONIC` and does not advance across
a suspend, while `SystemTime` does. A round in which wall time advanced by
seconds and monotonic time did not is a suspend that just ended, detectable with
no D-Bus client, no new syscall and no new clock. Routing that detection into the
same reactivation path the blank's wake uses is what makes #223's *"one code
path"* sentence true rather than vacuous.

**The residual is stated rather than hidden, and it is the same one #223's own
key-decisions block names:** this detects a resume **after** the fact, never
before. A frame may still be submitted into a suspending device, and the first
post-resume frame may be late.

### Suspend, lid and power policy is logind's

`HandleLidSwitch`, `HandlePowerKey` and `HandleSuspendKey` already exist and are
configured per machine. Reimplementing them would be session policy inside the
TCB, and it is why the switch device class stays dropped at intake and why
WS-E.4.2 grew no switch event
([D-032(5)](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)).
Verified on this machine 2026-08-10: the lid is `SW_LID` on `event0`, `vitrind`
sees it and drops it, and `crate::input::intake_physical`'s doc comment says so
in as many words — *"Switch events (a lid closing) and hold gestures have no wire
event either."*

The cost is that **behaviour now depends on files this repository does not own**,
so *"suspend works"* is not reproducible from this checkout. The values are
therefore published with the runbook,
[read from the running logind rather than from a config file](../book/src/recovery.md#the-settings-this-depends-on-which-this-repository-does-not-own),
with the note that `/etc/systemd/logind.conf` is empty here so every one of them
is a systemd default that can move without any change to this repo.

### The biggest technical consequence, which #223 does not mention

**A DPMS blank stops every realm's frame clock, so it silently halts every agent
in the session.** CRTC disabled → no vblank → `DrmState::on_vblank` never runs →
`session::emit_presented` is never called → no `frame_done` is discharged →
every `frame_done`-paced client stops painting, and an agent holding `observe` is
served the pre-blank frame indefinitely with no staleness signal and no refusal.

That chain is verified rather than reasoned about: `emit_presented` has exactly
one bare-metal call site and it is inside `on_vblank`, pinned by a
source-inspecting test (`crates/vitrin-core/src/backend/drm.rs`), because
`redraw` returns `Scheduled` unconditionally.

D-030(6) already published this exact effect for a **VT switch** and called it
*"worse than a stall"*. What is new is that blanking makes it a **routine,
timer-driven** occurrence on an agent-first display server, rather than something
that happens when a human deliberately leaves. It is published in
`docs/book/src/limits.md` in the same register D-030 used, and the named fix —
*a software frame cadence for paused realms* — is D-030's existing unscheduled
deferral, inherited here rather than re-filed.

**The backlight alternative was examined and is not taken.** Writing
`/sys/class/backlight` keeps the CRTC active, so vblanks continue, the frame
clock never stops, and unblank is one sysfs write with no modeset risk. It is
refused because it puts a second display-power interface inside the TCB, has no
effect on external displays, and saves almost no power — and because D-030
already names it as an interface DRM master does not gate. The trade is recorded
so that a later reader does not have to rediscover it.

### The recovery runbook

#223's last task. It landed as a new book page —
[`docs/book/src/recovery.md`](../book/src/recovery.md) — wired into the book's
summary, rather than as another section of the bring-up page, because the two
answer different questions: the bring-up page is *how do I start this safely*
and the recovery page is *how do I get out*. It cross-references bring-up step 0
rather than restating it.

Three things about it are decisions rather than prose:

- **The route order is by symptom, and route 2 is first among the ones that have
  ever worked.** `Ctrl-Alt-F<n>` (D-031, second entry) is route 1 because it is
  the cheapest; a shell on another VT or in the Hyprland session on tty1, plus a
  signal to `vitrind`, is route 2 and is **the only route that has ever actually
  recovered a session** — it is what freed the first bare-metal run. The page
  says so rather than ranking by elegance.

  **The command this bullet originally named — `pkill -TERM -f "vitrind
  --drm"` — is wrong and must not be copied from here.** It is kept in this
  sentence only so the correction is legible. **#260, 2026-08-11:** `pkill -f`
  matches whole command lines, so a shell running it matches its own `argv` and
  the signal reaches the rescuer rather than the target; and against the command
  line `~/.local/bin/vitrind` actually produces, the literal string
  `vitrind --drm` never appears. The published form resolves the PID first —
  `pgrep -x -a vitrind`, then `kill -TERM <PID>` — and a signal wrapped in a
  `systemd-run` unit must carry a literal number, because there is no shell
  there to do the quoting.
- **The SysRq path is `sudo`-only and the kernel mask is not touched.** Settled
  by the owner on 2026-08-10. `/proc/sys/kernel/sysrq` is `16` here — sync only —
  so the physical `Alt+SysRq` sequence is inert by configuration, and **raising
  it is not proposed in any form.** The bring-up page's §0.4 carried a standing
  recommendation to do exactly that; it is **deleted**, not merely
  un-repeated, and its R2 rungs 2–3 that depended on it are rewritten.
- **The correct trigger-file sequence is not REISUB, and working that out was the
  substance of the task.** Three findings, each verified against the kernel's own
  documentation and source on 2026-08-10 and written up on the page:
  1. The mask claim holds — *"the value of `/proc/sys/kernel/sysrq` influences
     only the invocation via a keyboard. Invocation of any operation via
     `/proc/sysrq-trigger` is always allowed"* — and the mechanism is
     `write_sysrq_trigger` calling `__handle_sysrq(c, /* check_mask */ false)`.
  2. A naive `echo reisub > /proc/sysrq-trigger` performs `r` and **silently
     nothing else**: only the first character is processed unless the string is
     prefixed with `_`.
  3. **And the kernel's own bulk-mode example, `_reisub`, is a hard reboot with
     two no-ops in front of it.** `s` and `u` call `emergency_sync()` and
     `emergency_remount()`, both of which are `schedule_work(...)` and **return
     immediately**, while `b` calls `emergency_restart()`, which does not return
     at all — and bulk mode runs the whole string inside one `write()` with no
     pause between letters. So the sync and the remount are queued and the
     machine reboots before either can run. The kernel documentation says the
     same thing about the sync in its own words (*"the sync hasn't taken place
     until you see the "OK" and "Done" appear on the screen"*), and bulk mode is
     exactly the form that denies you the chance to see them.

     The page therefore prescribes **three separate writes with a wait between
     them** — `s`, wait; `u`, wait; `b` — and drops `e` and `i` entirely, on the
     ground that the trigger path needs a reachable shell by construction, so
     route 2's signal to one resolved PID is the aimed version of what `e` does
     bluntly, and `e` on this machine would destroy the Hyprland session that
     *is* the escape route. (Written as `pkill` here originally; corrected
     2026-08-11 per #260 — `pkill -f` on this pattern is not aimed.)
- **The caveat is stated plainly:** the trigger file needs a **reachable shell**.
  It covers *"vitrind wedged the display"* once a VT or the tty1 shell is
  reachable; it does **not** cover *"input is completely dead"*, which is the
  only case the physical combo would have covered, and which the owner has
  declined knowingly. No new mitigation is invented — the page documents the
  routes that exist.

One machine-specific finding worth carrying: `kernel.dmesg_restrict` is `1` and
the console log level is `1` here [verified 2026-08-10], so the kernel's
completion messages reach **neither** an unprivileged `dmesg` **nor** the
console. `sudo dmesg -w` in a second shell is the observable, which is a third
independent reason this route needs a shell rather than a keyboard.

### What is built, what is not, and what cannot be known here

Same device as
[D-032's table](20-decision-log.md#what-is-built-what-is-not-and-what-cannot-be-known-here),
for the same reason: a build status compressed into a status line is the sentence
nobody re-reads, and this workstream has already had five sentences claim
something was unbuilt while sitting in the change that built it.

| Task | State |
|---|---|
| Activation transitions (VT switch) | **Already on `main` before #223**, WS-E.3.3/D-030 and D-031. Hardware-confirmed: two pause→activate cycles, second run. #223 adds nothing here. |
| Resume | **Redefined, not routed.** No session event exists to route; post-hoc detection from the monotonic/wall clock pair is what replaces it. |
| Blank/unblank | **New.** The state machine, the cover surface and the activity clock; DPMS itself is `DrmSurface::clear()`, which is DRM-only. **Unproven on hardware when this was written; proven once on 2026-08-11** — blank at 61.2 s, unblank on physical input, no lock card — and the same run filed #257, #258 and #259 against it. |
| Consent and dead-man interaction | **New for the dark case** — a third `PromptVisibility` variant, discharging D-030's explicit *"a dark-output gate — to whichever change implements DPMS, as that change's own acceptance criterion"* deferral. The dead-man switch needs **nothing new**, and that is a finding rather than a shrug: it detects in the router's unconditional `observe` tap, which no gate can suppress, so a held Escape starts its hold on the very press that wakes the screen. |
| `RuntimeHost` surface | **Answered by landed code, and by neither option #223 offered.** Reactivation is a host-state method delegating to free `session::*` functions with explicit disjoint parameters; `RuntimeHost::split()` was never touched. A `Presenter` method would need both halves of `split()` at once, which is the borrow `split()` exists to avoid. |
| Media and brightness keys | **New rows in the invariant table**, and #223's list was incomplete — it named volume-**up** and mute but not volume-**down**. |
| Recovery runbook | **New page**, `docs/book/src/recovery.md`, wired into `SUMMARY.md`; bring-up §0.4 deleted and R2 rewritten. |

**What no amount of work in this branch can establish.** CI has no DRM device,
no seat, no ACPI and no backlight, and `DrmState` cannot be constructed without a
real `DrmDevice`, `LibSeatSession`, `GbmDevice` and `GlesRenderer` — so
`DrmSurface::clear()` is unreachable from every test in this workspace, exactly
as D-030 recorded for `handle_session_event`. **Not one hardware criterion of
#223 is claimed as met by this change.** The 10 VT switches, 5 suspend/resume
cycles, 5 lid cycles, the blank/unblank and the deliberate wedge are the owner's
to produce on the target machine, they are written as rungs `L1`–`L6` on the
recovery page with a record block to paste numbers into (a seventh, `L7`, was
added later by the fix for #257, which the first `L4` run found), and **#223
stays open until he pastes them in.** Note also that **suspend and lid have
never been exercised on this backend by anyone, in any form** — the bring-up
page's existing checklist runs 7–15 and contains no suspend, lid or blank rung
at all, which is why those rungs had to be written before they could be
executed.

> **AMENDED 2026-08-11.** The paragraph above is left as written because it is
> the honest status *of that change*, but its last sentence stopped being true
> on 2026-08-11: the owner executed `L1`–`L6` and pasted the numbers into #223.
> The counts are short of what the rungs ask — `L2` 4 of 5, `L3` 2 of 5 with
> only one cycle ever reaching sleep, `L6` recovered but by an unrecoverable
> route — and route 3 and the VKMS rung remain unexecuted. The dated record is
> the [first-run block on the recovery page](../book/src/recovery.md#first-run--2026-08-11-l1l6-on-the-target-machine),
> and the accepted-cost entry further down this page is narrowed to match.

**The VKMS rung was not attempted by this change, and that is recorded rather
than left to be noticed.** #223's acceptance criteria ask that it be "attempted
and its result recorded either way … it never silently disappears", and a
criterion that quietly produces no text is the failure it was written against.
`modprobe vkms` loads a kernel module on the owner's machine and was out of
bounds for the session that wrote this. VKMS is nevertheless the only place in CI
where `DrmSurface::clear()` could plausibly execute at all: it proves nothing
about suspend, seat handover or backlight — there is no ACPI and no panel behind
a virtual connector — but it would prove that the power-down and the unblank
modeset are accepted by a real DRM device rather than only by this workspace's
reasoning. It is therefore a named, unclaimed rung, **reopened by** anyone
running the existing `vkms-advisory` job against a `--blank-idle` session and
pasting the result either way.

### Deferred, each with the evidence that would reopen it

#223's body uses *"refused"* in several places. That wording **predates the
owner's correction on #222** — a capability this issue does not build is
**deferred**, and each deferral names the evidence that would reopen it. The
correction is followed here and not the issue.

- **Idle inhibition** (`zwp_idle_inhibit_manager_v1`). Needs a new shim global
  *and* a shim→core wire verb, i.e. paired IDL + prose work on `track:protocol`.
  **Reopened by:** that paired edit. Until then, publish plainly — **full-screen
  video will blank the screen.**

  > **TRACKED 2026-08-17.** The paired edit is still owed and this deferral is
  > therefore still standing — what changed is only that it is no longer
  > untracked. It is [#306](https://github.com/vitrin-os/vitrin-os/issues/306).
  > The bullet above is left as written because it is the honest status: nothing
  > has been built, and full-screen video still blanks the screen.

  > **DISCHARGED 2026-08-17, and both blocks above are left as written.** The
  > paired edit landed: `protocol/vitrin-v0.xml` grew
  > `vitrin_shim_session.idle_inhibit` at `since="2"` with its
  > `idle_inhibit_state` enum, `docs/protocol/09-vitrin_shim_session.md` grew the
  > matching prose page and Flow M, the shim grew the
  > `zwp_idle_inhibit_manager_v1` global (`shim/src/idle.c`, citing the
  > `globals-demand` line at
  > `shim/docs/globals-touched-firefox-140.12.0esr.log:158`), and the core holds
  > one bit per realm consulted at the single blank decision point. The grant
  > question #306 raised is answered by [D-042](20-decision-log.md): **holding an
  > inhibit is a property of the realm the human is looking at, not an authority
  > a grant confers.**
  >
  > **What has NOT changed, and is why the deferral's published limit became a
  > *bound* rather than being deleted:** the sentence *"full-screen video will
  > blank the screen"* has not been falsified on any machine. It is now *"the ask
  > reaches the core, and no human has watched a video to find out"* — blanking
  > needs a display controller, CI has none, and the shim-side proof
  > (`shim/tests/acceptance/idle_inhibit.sh`) is a **component** test against
  > `mock_core.c`. Two further bounds are new rather than removed: an inhibit
  > held by a realm the human is *not* looking at holds nothing, and an inhibit
  > never suppresses the idle **lock**. See the limit register below.
- **A software frame cadence for blanked or paused realms.** Inherits D-030's
  existing unscheduled deferral. **Reopened by:** the first agent-visible stall an
  operator reports, or an owner decision that a blank halting agents is
  unacceptable.
- **Backlight actuation for the brightness keys.** The new rows convert *"key
  dropped at intake"* into *"key delivered to an app that cannot act on it"* — no
  confined app can write `/sys/class/backlight`, so the human still presses
  brightness and nothing happens. **Reopened by:** a shell client holding a named
  verb (Stage 2's design), or an explicit owner decision to let the core write
  `/sys/class/backlight`.

  > **REOPENED 2026-08-17 BY THE SECOND OF ITS TWO NAMED TRIGGERS.** The owner
  > decided that the core writes `/sys/class/backlight` — not the shell-client
  > route this bullet named first. The decision is
  > [D-041](20-decision-log.md#d-041--the-core-writes-sysclassbacklight-so-the-brightness-keys-actuate-d-033-refused-that-interface-as-a-blanking-mechanism-and-half-of-its-reasoning-still-bites-here)
  > and the work is [#303](https://github.com/vitrin-os/vitrin-os/issues/303).
  > **Nothing is built yet**, so the sentence above — the human presses
  > brightness and nothing happens — is still true of every checkout today.
  >
  > **SUPERSEDED THE SAME DAY, 2026-08-17, BY THE BLOCK BELOW.** It is left
  > standing on the precedent this page sets for its 2026-08-11 amendment: the
  > paragraph is the honest status of the moment it records, and a plan document
  > that edits its own past is worth nothing. Its last two sentences are the
  > ones that went stale — #303 *is* built, so *"nothing is built yet"* is false
  > and the sentence above is no longer true of a checkout that passes
  > `--backlight` on `--drm`. It remains true of every other session, which is
  > what the next block says and this one does not.

  > **BUILT 2026-08-17, AND UNPROVEN ON HARDWARE.** #303 landed
  > `crates/vitrin-core/src/backlight.rs`: on `--drm --backlight` the core
  > consumes `XF86MonBrightnessUp`/`Down` and writes
  > `/sys/class/backlight/<device>/brightness`, 5% of `max_brightness` per press
  > with a floor at 5% — both rounded *up* and never below one raw unit, so the
  > published floor is literally 5% and not `floor(max/20)` — bounded exactly
  > the way `status/battery.rs`'s read is,
  > every failure collapsing to the key doing nothing and journalled as
  > `backlight_stepped`. **The paragraph above is therefore no longer true of a
  > session that passes the flag**, and it remains true of every other session:
  > without `--backlight`, and on `--nested`/`--headless` where the flag is a
  > startup error, both keys are still delivered to an app that cannot act on
  > them. The honest status is **landed in the tree, unproven on hardware** —
  > D-041 says CI structurally cannot test the actuation (no seat, no ACPI, no
  > `/sys/class/backlight` on any runner), so the evidence that would change
  > this sentence is rung 16 of [`docs/drm-bringup.md`](../drm-bringup.md), which
  > is **written and NOT YET RUN**. The volume half of the same published limit
  > did not move.
- **Preparing for suspend** (logind `PrepareForSleep`). Stands on the no-D-Bus-in-
  the-TCB decision. **Reopened by:** evidence that post-hoc resume detection is
  insufficient — specifically, a reproducible corrupted or lost frame across a
  real suspend on the target machine.
- **A configurable lock-on-blank / lock-on-switch policy.** D-030(2) already
  filed it; Decision 1 forbids coupling idle-blank to idle-lock here.
  **Reopened by:** the owner asking for it. **The lock-on-switch half is now
  built** — `--lock-on-seat-change immediate|idle|never` (issue #246,
  [D-034](20-decision-log.md#d-034--losing-the-seat-is-a-configurable-lock-policy-with-d-0302s-answer-as-the-default-and-lockcause-gains-the-third-variant-that-entry-declined)),
  defaulting to D-030(2)'s answer so no session that never names the flag
  changes. The lock-on-**blank** half stands deferred on Decision 1, which is
  the one this bullet's second sentence is about.
- **`--blank-idle` on `--nested`.** Refused at startup with a named reason: a
  nested `vitrind` painting its window black would be asserting something about a
  screen the host owns. **Reopened by:** someone naming a host fact that means
  *"the human cannot see this"*, which winit's `Occluded` is not — D-030's own
  wording.

### Handoff to WS-E.4.4 (#224)

#223 publishes to `docs/plan/`, to `docs/book/src/limits.md` and to the new
`docs/book/src/recovery.md`, and stops there. **`README.md` and
`site/index.html` were not edited by this issue** — they are #224's, so that the
project's public claims are enumerated in one place rather than drifting surface
by surface. This is the same split [§4.2](#the-interim-and-what-it-costs) used
for #221, and the exact text each surface must carry is dictated here rather than
summarised, for the same reason.

| Surface | The claim, as it must read |
|---|---|
| `README.md` | **Idle blanks the screen; it does not lock it.** With `--blank-idle` the panel goes dark after a period of no physical input and the session stays **unlocked** — anyone who touches a key gets the session, not a passphrase prompt. Locking is a separate, manual chord. **And a dark screen is not evidence that nothing is watching:** an agent holding an `observe` grant keeps capturing while the panel is off, exactly as it does across a lock. Idle inhibition is **served and bounded** since #306: only the realm your output is on can hold one, it suppresses the blank and never the lock, and no human has watched a video on hardware to confirm the panel stayed lit. None of the session-lifecycle behaviour has been confirmed on hardware. |
| `site/index.html` | **Idle blanks, it does not lock.** A dark screen is not a locked session and is not evidence that nothing is being observed — an agent with an observe grant keeps capturing. Idle inhibition is served and bounded three ways (the bound realm only, the blank and never the lock, and unconfirmed on a panel). Unconfirmed on hardware. |
| `docs/book/src/limits.md` | Landed by this issue, in the blank/idle entries. Reproduced in this table only so #224 can check that three surfaces say the same thing. |

Four constraints on that text, which are why it is dictated rather than left to
be re-worded:

1. **It must not say "refused"** for idle inhibition, or for anything else #223's
   body calls refused. The owner corrected that register on #222: these are
   **deferrals**, and each names the evidence that reopens it. A surface that
   says *"refused"* has published a permanence nobody decided.

   > **AMENDED 2026-08-17 by D-042 (#306).** The two rows above are updated
   > rather than left standing, because the table's own purpose is that three
   > surfaces say the same thing and #306 changed what the true thing is: idle
   > inhibition is **served**, and what is published is now a set of bounds. The
   > constraint in this numbered item is unaffected and gets sharper — a surface
   > must not say *"refused"*, and it must now also not say *"not yet served"*,
   > which has become false in the other direction. The fourth constraint below
   > still binds every word of it: no hardware claim may be made, and none is.
2. **It must not soften "unlocked".** The point of Decision 1 is that the cost is
   real; a surface that says *"the screen turns off after a while"* and omits
   *"and the session stays unlocked"* has published the half a reader does not
   need.
3. **The observe-across-a-blank claim must cite the lock-screen one**, not stand
   alone. They are one policy — the grant is the authority, not the human's gaze
   — decided by the owner on 2026-08-08 for the lock
   ([D-025](20-decision-log.md#d-025--a-locked-screen-does-not-suspend-agent-observation-the-gap-is-published-not-papered-over))
   and inherited here. Two surfaces stating them as unrelated accidents would
   misrepresent a deliberate posture as a pair of oversights.
4. **No hardware claim may be made.** Every lifecycle behaviour here is
   *unproven on hardware* until #223's L1–L7 numbers exist. #224 must not tidy
   that qualifier away, and if the numbers have landed by then it must cite the
   dated run rather than dropping the sentence. The one rung that *has* been run
   (`L4`, 2026-08-11) produced #257–#259 rather than a pass, and their fixes are
   themselves unobserved: a partial run is not a smaller version of a pass.

Also handed to #224, because #223 cannot close them: the two `docs/plan/` and one
`crates/` prose surfaces that this change falsifies and that live outside the
docs half — `crates/vitrin-core/src/session.rs`'s *"This core has no DPMS"*
paragraph and its *"there is deliberately no third variant for a dark panel"*
claim, which the `rust-core` half of #223 rewrites in the same change that makes
them false. If #224 finds either still standing, that is a defect in #223's
landing rather than a change of decision.

## 5. The target machine, and why no number here generalizes

Every WS-E estimate is measured against hardware chosen for being easy:

- One connected output, eDP-1, 2560×1600@240, **scale 1** — no fractional
  scaling anywhere in the workstream.
- eDP-1 is on `card1` = **i915**, and it is the only `connected` connector on
  the machine — so scanout *and* render are Intel: no PRIME, no multi-GPU
  renderer, the most well-trodden path in Wayland.
- **`nvidia_drm` *is* loaded, and `/dev/dri/card2` exists.** An earlier version
  of this line said it was not, and that was wrong (re-checked 2026-08-09). All
  four of `card2`'s connectors are `disconnected`, so it can light nothing — but
  it is a second DRM device that udev enumeration will find, so a backend taking
  "the first card" can take the one with no output. `docs/drm-bringup.md` carries
  it as bring-up hazard H1.
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
this workstream owns, not inherits.

### The surfaces, enumerated once (WS-E.4.4/#224)

Until #224 this list existed only as **per-issue handoff tables** —
[§4.2](#handoff-to-ws-e44-224) dictating #221's three sentences,
[§4.4](#handoff-to-ws-e44-224-1) dictating #223's — which is a handoff and not
an enumeration: each one names the surfaces *that issue* touches, so nothing
anywhere held the set. `CLAUDE.md`'s `known-limit` rule ("enumerate every
surface when closing one") needs the set to exist in one place. Here it is, and
it is the only copy; the handoff tables stay as **dictated text for their own
claims** and are inputs to this table rather than a second enumeration of it.

| Surface | Register it is written in | What it carries | What holds it |
|---|---|---|---|
| [`docs/book/src/limits.md`](../book/src/limits.md) | An argument, at length, hard on this project. **This is the governing surface**: where it and any other disagree, it wins. | Every limit in this section, each with its reasoning, its evidence and what it does *not* claim. | `cargo xtask limits-check` (see below): the anchored claims, **and a set cross-check against this section** — plus human review |
| [`README.md`](../../README.md), §"Running it as a desktop" | A contributor's summary — dense, linked, no argument. Sits directly under "What Phase 1 does *not* give you", because a reader who read that list will otherwise assume it is still the whole list. | The same limit set in one bullet each, every one linking onward. | `cargo xtask limits-check`, for the **anchored** claims only — see below |
| [`site/index.html`](../../site/index.html), §"It can drive a real panel. It is not a desktop." | A landing page's warning, in a reader's words. | An eleven-row table, **with the measurement dates, the two kernels and the single machine configuration above it**, and a link to the limits page as the surface that governs. | `cargo xtask limits-check` |
| [`docs/book/src/session-app-matrix.md`](../book/src/session-app-matrix.md) | Generated. Not hand-editable. | Which applications have actually been run, at what bar, with the observable checked. WS-E limits are **cited** from it, never restated in it. | `cargo xtask session-matrix --check` (byte-for-byte) |
| [`docs/book/src/recovery.md`](../book/src/recovery.md) | A runbook. | The hardware checklist `L1`–`L7` and its dated record; the `logind` settings this repository does not own. | A human executing it, and nothing else |
| [`NOTICE`](../../NOTICE) | Normative path→license map. | **Nothing from WS-E.** Checked and confirmed: no WS-E limit is licensing-relevant, and the workstream moved no file across a license boundary. Named here so the next sweep does not have to re-derive that it is out of scope. | — |
| [`docs/PRD.md`](../PRD.md) §15 | The threat model. | Already edited by WS-E.2.1: the row that said a malicious app cannot reach *"the session's real seat/clipboard/a11y bus"* now states the clipboard bound. Not re-edited here. | Human review |

**The site carries a stated subset, and that is a decision rather than an
oversight.** Eleven rows is already the outer limit of what a landing page can
carry before a reader stops reading, so it omits the touch/tablet deferral,
which needs the "deferral with named reopening evidence, not a refusal" framing
to be read correctly and is meaningless at one line; the second-GPU distinction,
which is a sentence about one laptop; the media/brightness-key half-fix and the
missing key repeat, each of which needs its "where the key stops changed, not
what it does" framing to be read as anything but a fix; and the per-run counts,
of which the site's preamble carries **only the dates, the two kernels and the
defect total** — `L1`–`L6`'s individual numbers are on `README.md` and
`limits.md` and nowhere on the site. Every one of those is on both other
surfaces, and the site's closing line names the limits page as the surface that
governs. What the site may **never** do is state a claim the limits page does
not, or state one more weakly — `limits-check` holds the ones it carries, and
this paragraph is the record for the ones it does not.

**The mechanism, and it is explicitly temporary.** `cargo xtask limits-check`
(`crates/xtask/src/limits.rs`, run by the `codegen-diff` job) holds a table of
claims. Each names the **anchor phrase** every surface must carry *and* the
**code evidence** that makes the claim true — a constant that must still read
`61440`, an interface name that must still appear nowhere in `shim/src/`, a CI
job whose own name must still say `COMPILE ONLY`. Deleting a claim from one
surface fails it; changing the code without changing the page fails it too, and
that second half is the one that matters here: **#224's own body carried two
items that were false of `main`**, and three surfaces agreeing with each other
would have agreed on both. Issue #172 owns the choice of mechanism for this
repository's honesty surfaces and has not made it; this is its option (b), built
narrow, and the module states what replaces it under each of #172's options.

**And a second mechanism, which compares this section's limit set against the
limits page's — the SET, never the wording.** #224's fifth acceptance criterion
asks for this section and `limits.md` to be cross-checked *"mechanically, not by
reading"*, and the obvious instrument is the wrong one: the two documents are
written in two registers on purpose, this one addressed to whoever maintains the
project (*"TCB growth for zero differentiator"*) and the limits page to a
stranger deciding whether to run it (*"anyone who walks up to your dark laptop
and touches a key is inside your session"*). Demanding a shared anchor phrase
across them would make an honest rewording of either register a red build, which
is the *"trains people to weaken the check"* failure #224's own risk list names.

So each limit carries an **identity that survives rewriting** instead: an
invisible marker comment holding a kebab-case id, in this section and beside the
matching entry on the limits page. A pair of HTML comments reading
`limit-set: begin` and `limit-set: end` bound the set below (the corrections and
measurement subsections have bullets too, and none of them are limits); inside
it every top-level bullet carries a comment reading either `limit: <id>`,
meaning the limits page publishes it under that id, or
`limit-not-on-page: <id> -- why`, which is the escape hatch and **costs a
written reason**. The delimiters are deliberately written here without their
comment brackets: the gate reads this file, and a literal delimiter in prose
would move where it thinks the set starts. The gate refuses a duplicated
delimiter for that reason. The gate then holds five
things: the two id sets are equal in both directions; every top-level list item
in the set carries a marker at all (a limit added here with no marker is one
nobody was ever told about, which is the failure item 6 of the corrections list
below records this sweep committing); an off-page reason exists; an off-page id
does not turn up on the page anyway; and **the comparison is refused outright if
both sets come out empty**, because two empty sets are equal and a gate that
prints "the same limit set" having read nothing is worse than no gate at all.
Reword either document however its register needs — the ids do not move.
Matching runs over whitespace-normalised text, so a reflow, and a wrapped
reason, change nothing.

**This section is no longer the only enumerating home, and that is a
correction to a sentence written here rather than a weakening of the gate.**
The check now compares the limits page against the **union** of the regions in
every registered plan document, because a limit's home is the document that
owns the work which created it — and §6's own opening scopes it to *"limits
**this workstream** creates ... not inherits"*. Phase-2 confinement (#286) is
the first limit outside WS-E to need one, and it went into
[`02-phase-2-semantic-epochs.md`](02-phase-2-semantic-epochs.md) §7 rather
than here: writing it into this section would have made that opening sentence
false and sent the next sweep to this document's surface table for a limit
this workstream does not own. Nothing was exempted to make that work — both
directions still hold over the union, and the multi-home shape cost three
further rules, each of which is a hole one home never had to think about: a
registered document must enumerate at least one limit, an id is declared by
exactly one document, and the every-bullet rule runs over each region. The
argument is in `crates/xtask/src/limits.rs`'s `ENUMERATORS`, including what a
carve-out would have looked like.

**The fifth rule and two others were added by an adversarial pass over the
first version of the gate, which found three ways to pass it while holding
nothing.** Emptying this region and stripping the page's markers was green.
So was adding any text to the `limit-set: begin` line — the set comparison
reads normalised text and went on working, while the line-based "every item is
marked" scan silently never entered the region. So was writing a new limit with
Markdown's equally valid `* ` instead of `- `. All three are now red, each with
its own test. They are recorded here rather than only in the code because the
lesson is not about Markdown: every one of them was a *comparison that held
between two things neither of which existed*, which is the failure mode this
whole section is written to refuse.

**Writing that check found two limits this section did not carry.** `one-output`
and `realm-cap-arithmetic` were published on all three surfaces, both have a row
in `limits-check`'s claim table, and this enumeration — the one `CLAUDE.md`'s
`known-limit` rule sends a reader to — did not list either. Both are added
below, marked as such. That is the criterion working before it was even green,
and it is recorded here because "the plan document is the set" was an assumption
nobody had tested.

What the gate **cannot** hold, stated so its green is not over-read. First,
anything about hardware: "One machine, one GPU, one panel, one kernel", the
counts from `L1`–`L7`, and every date on this page are claims about the world, a
runner cannot check one of them, and the gate is silent rather than reassuring.

Second — and this one is easier to over-read because it looks like coverage —
**the table holds the claims it has rows for, and the "What holds it" column
above means exactly that**. A published sentence with no row in `CLAIMS` can be
deleted from two surfaces with the build green. That was not a hypothetical: the
first draft of this sweep anchored the band-witness claim on `limits.md` alone
and left the shell-crash and lock/blank claims out of the table entirely, and
deleting the `README.md` bullet and the `site/index.html` row for any of the
three passed. Every claim published on all three surfaces now has a row anchored
on all three; the two claims published on two surfaces (the media/brightness
half-fix and the missing key repeat) are anchored on those two. The rule this
sweep leaves behind for the next one: **a claim that is not in the table is not
held, and adding a published claim to a surface without adding its row is how
the next stale sentence ships.**

Third, the set cross-check has its own edges, and they are not the same edges.
**It cannot see an unmarked paragraph on the limits page.** A WS-E limit
published there with no marker comment is held in neither direction, because
nothing in the text distinguishes it from that page's many Phase-1 entries,
which are inherited rather than created here and carry no marker on purpose —
the portals entry and the static-identities entry are two of them. The
every-bullet rule covers this section's side of that hole and there is no
equivalent for the page. **It says nothing about `README.md` or
`site/index.html`**, which are held claim by claim by the anchors above, because
the site carries a stated subset and set equality against a deliberate subset
would be wrong. **It does not check that a marker sits beside the right
paragraph**: an id moved to a neighbouring entry satisfies every rule, so what
is held is that the two documents enumerate the same limits and not that each
pair of entries says the same thing. And **one id is one anchor** — where the
limits page splits a limit across several paragraphs the marker goes on the
primary one and the rest are unheld, and where a bullet here covers two limits
it carries two markers, which is the shape to reach for rather than stretching
an id's meaning. Twelve limits in the set below are marked
`limit-not-on-page`, each with its reason; three of those are closed, and the
other nine are limits this project has written down for itself and published
nowhere. That list is not a gap in the gate — it is the gate's output, and it is
the honest state of what §6 carries that no reader ever sees.

### Corrections this sweep had to make before it could publish anything

Six claims were **false of `main` at sweep time** and are recorded here rather
than quietly fixed, because a sweep that tidies away its own inputs teaches the
next one nothing. Three came in as inputs; the other three the sweep **wrote
itself and then had to retract before publishing**, which is the more useful
half of this list — it is evidence that "verify against the code, not against
the plan document" is a rule this task could not follow from memory either:

1. **#224's item (6)** asked to publish *"no cross-realm clipboard of any
   kind"*. WS-E.2.1/#213 shipped one and `limits.md` already published the
   opposite. Corrected in the issue body on 2026-08-12; published here as the
   **bound**, never as an absence.
2. **#224's item (8)** asked to publish *"no lock screen and no idle
   inhibition"*. WS-E.2.2/#214 shipped the lock screen. Only the
   idle-inhibition half was true, and only that half is published.

   > **AMENDED 2026-08-17 by D-042 (#306).** The sentence above is left as
   > written, because it is the honest record of what this sweep found and a
   > correction list that edits its own findings teaches the next sweep nothing.
   > What has changed since: **neither half of #224's item (8) is true any
   > more.** #306 served idle inhibition, so "and no idle inhibition" went the
   > same way "no lock screen" already had — which makes this the *second* time
   > a #224 item was overtaken between being written and being published, and
   > that is the pattern worth carrying forward rather than either instance.
   > What is published in its place is a **bound**, never an absence: only the
   > realm the human's output is bound to holds the blank, the blank alone is
   > suppressed and never the idle lock, and no run on real hardware has
   > confirmed a video keeping a panel lit. The `limits-check` row
   > `idle-inhibit-bounded` gates that wording on all three surfaces, and
   > [D-042](20-decision-log.md#d-042--an-idle-inhibit-is-a-property-of-the-realm-the-human-is-looking-at-not-an-authority-a-grant-confers-it-suppresses-the-blank-and-never-the-lock)
   > is the decision.
3. **`limits.md`'s own DRM bullet** read *"Not even a compile-check, yet… **no
   such rung exists and no such feature exists** — the backend itself is
   unwritten (#218)"*. #218 landed. `.github/workflows/ci.yml` now carries a job
   named `drm-compile-check (COMPILE ONLY - no display controller is touched)`
   and `crates/vitrin-core/Cargo.toml` carries the `drm-backend` feature, so
   that correction had itself gone stale — in the **understating** direction,
   which this repository holds to be the more corrosive one. The bullet now
   keeps both corrections and quotes the job's name rather than paraphrasing it,
   and `limits-check` gates the quote.
4. **This sweep's own first draft published *"no AT-SPI2 bus **reachable**
   inside a realm"*** on all three surfaces — which is the exact
   advertised-versus-reachable error the sweep's *portals* bullet gets right
   forty lines earlier, repeated one section later about accessibility.
   `crates/vitrin-core/src/spawn.rs` says *"That is advertisement, not
   reachability"* about the session bus, `org.a11y.Bus` is activated **on** that
   bus, and `RESERVED_ENV` holds five names of which none is a bus address — so
   `DBUS_SESSION_BUS_ADDRESS` is allow-listable and an operator running Firefox
   does allow-list it, handing that realm the host's a11y bridge. Phase 2's
   P2.1.10 exists precisely because the route is open. Published now as
   **advertised**, with the same missing-service-not-a-confinement sentence the
   portals bullet carries, and pinned by a `limits-check` evidence row on
   `spawn.rs` so the stronger word cannot creep back.
5. **The same draft published that sixteen launches spend the realm cap *"for
   good"* / *"for the life of the session"*** on `README.md` and
   `site/index.html`. `Realm::occupies_capacity` excludes the terminal state and
   the launch refusal in `session.rs` reads `capacity_used()`, not `len()`, so a
   slot returns when a realm's app exits — there is a shipped test named
   `capacity_counts_live_realms_and_forgets_exited_ones` asserting exactly that.
   `limits.md` was half-right and half-wrong on its own: it said *"a realm ends
   when its own app exits, and not otherwise"* and then, two sentences later,
   that fifteen launches *"permanently commit"* the remaining slots. #234's
   title is narrower than what all three surfaces said: it is about **no
   principal being able to end a realm**, never about the slot not returning.
   All three now publish both halves together, and `limits-check` anchors the
   qualifier as well as the number, because a gate that only looked for
   `16 realms` could not see this.
6. **The sweep's first draft omitted two limits its own source document
   enumerates**: the media/brightness keys that now reach an app which cannot
   act on them (§6 above calls it *"an honest half-fix rather than a fix"*), and
   the gesture that a consent card or the lock ends the wrong way. Both are
   published now. A sweep that silently drops an item from the list it was
   handed is the failure this issue exists to prevent, so the omission is
   recorded rather than repaired in silence.

**And one limit came out of the sweep that no source document had.** Reading
`shim/src/seat.c` to check the accessibility list's *"no repeat tuning"* turned
up a comment asserting *"the core repeats instead, for physical-origin presses
only"* — describing code that has never existed. The shim sets
`wlr_keyboard_set_repeat_info` to zero by a good decision (repeat is seat-wide
and this seat carries an agent's actuations), off a host libinput synthesizes no
repeat, and the compensating core-side repeat was never written, so **a held key
on `--drm` produces exactly one character**. The comment is corrected in place,
the limit is published on `limits.md` and `README.md`, and it is stated as
**unconfirmed on hardware** because it was found by reading rather than by
using: CI cannot hold a seat, and the one bare-metal session that drove a
terminal did not test for it.

And one dictated sentence was **refused rather than published**. §4.4's handoff
table dictates, for `README.md` and `site/index.html`, *"None of the
session-lifecycle behaviour has been confirmed on hardware"* — which was true
when it was written and is not true now. Its own constraint 4 anticipated
exactly this: *"if the numbers have landed by then it must cite the dated run
rather than dropping the sentence."* So both surfaces carry the dated run
instead of the blanket sentence, at the depth each register can hold:
`README.md` carries the counts actually recorded (2026-08-11: 10/10 VT switches,
4 of 5 suspend cycles, 2 of 5 lid cycles with one reaching sleep, blank at
61.2 s, no lock card), and `site/index.html` carries the dates, the two kernels
and the defect total only, because a landing page that lists six rung counts
loses the reader before the row that matters. Neither states a number the other
contradicts, which is the actual requirement.

The limit set follows.

<!-- limit-set: begin -->

- <!-- limit: no-accessibility -->
  **No accessibility of any kind.** No screen reader, magnifier, on-screen
  keyboard, sticky or slow keys, high-contrast or reduced-motion signal, and no
  AT-SPI2 bus *advertised* to a realm — **advertised, not reachable**, in the
  same register as the portals bullet and for the same reason: under D9 the host
  session bus, where `org.a11y.Bus` is activated, is still connectable and
  neither `DBUS_SESSION_BUS_ADDRESS` nor `AT_SPI_BUS_ADDRESS` is in
  `RESERVED_ENV`. #160 makes the absence real; P2.1.10's adversarial probe is the
  test that would prove it and does not exist. The semantic channel is **not** a
  substitute for AT-SPI — different consumer, different transport, under a grant
  a human approved — and it **does not make Orca work**. A daily driver with no
  screen reader is a real exclusion and is stated as one. **Published by
  WS-E.4.4/#224** as an *exclusion, not a deferral*, under its own `##` heading
  on `docs/book/src/limits.md` and in its own register on `README.md` and
  `site/index.html`. It has **no issue and is to have none**: an issue implies
  somebody intends to close it, PRD §5.3 puts human accessibility inside the
  horizon phase's support treadmill, and that phase's M4 entry gate is unmet on
  every threshold.
- <!-- limit: no-x11 -->
  **No X11**, so no Steam and no legacy application (pre-existing; *scoped and
  measured* by WS-E.4.1/#221). §4.2 carries the six-item list this hands to
  E3.2, the X11-only software actually measured on this machine, the classes
  deliberately excluded from that list, and the one contradiction the method
  could not resolve. The maintainer's interim is **a second session for
  X11-only software**, so *"I did not have to reboot into Hyprland"* is false
  for that set until E3.2 lands — a workaround he accepts and one that nothing
  in this stack confines, since the second session is another compositor with
  full access to the same devices and switching to it leaves the confined world
  entirely. Published in `docs/book/src/limits.md`, and the executed runs are in
  `docs/book/src/session-app-matrix.md`.
- <!-- limit: no-layer-shell -->
  **No bars, launchers, notifications or OSD** — there is no
  `zwlr_layer_shell_v1` and there will not be one at the app level; the
  replacements are core-owned surfaces.
- <!-- limit: shell-crash-loses-re-aim -->
  **A shell crash loses window management**, because the shell is a client and
  there is no core-side fallback. §3(3)'s invariant is right and this is its
  price. **Measured since WS-E.1.5/#211**, and the shape is narrower than the
  sentence suggests: killing the shell leaves both realms running and the realm
  it last focused still receiving the human's physical input, because the
  binding is core state. What is lost is the ability to *re-aim* — and the
  wedge is that recovering means running the shell again from a terminal which,
  in a real session, must already be the bound realm. Asserted by
  `tests/integration/test_shell.py`; published in `docs/book/src/limits.md`.
- <!-- limit: drm-has-no-ci-gate -->
  **The DRM backend cannot be tested by CI** — no runner has a DRM device or a
  seat — so it arrives with structurally weaker evidence than anything else in
  the tree. That is an asymmetry against D12 and it is published, not
  discovered.
- <!-- limit: one-output -->
  **The session drives exactly one output, and a second connected display is a
  startup refusal** (created by WS-E.3.2/#218; the hot-plug gap it leaves has no
  issue). Both of the session's cardinalities are in the contract rather than in
  the content — up to 16 realms, one output — so sixteen live realms do not buy a
  second panel. Coming up on two panels would light whichever connector
  enumerated first and leave a powered display dark with no message and no verb
  in the protocol that could move the output to it, so `--drm` refuses to start
  and names the connectors it found. **A laptop plus an external monitor, the
  most ordinary desktop arrangement there is, does not work here.** The refusal
  is a *startup* one: the backend enumerates connectors once and installs no udev
  monitor, so a panel plugged in mid-session is neither lit nor complained about.
  Published in `docs/book/src/limits.md`, and held by `limits-check`'s
  `one-output` claim. **Added to this enumeration by WS-E.4.4/#224**: all three
  published surfaces carried it and this section did not, which is precisely the
  divergence the set cross-check described above exists to catch — and it was
  found by writing that check rather than by reading.

- **Input classes on the wire: partly closed by WS-E.4.2 (#222), and the
  remainder is split three ways.** This bullet used to read *"no touch,
  gestures, tablet, switches or relative motion on the wire"* and to say the
  limit was *"unchanged and real"* until the work landed. **It landed**
  ([§4.3](#43-the-five-input-classes-the-seat-vocabulary-drops-a-verdict-each),
  [D-032](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)),
  and the old sentence survived into the commit that falsified it — recorded
  here because this is a published-limits section, where an error in the
  optimistic direction is the one that misleads a user. The five classes never
  shared a verdict and now share even less:
  - <!-- limit: pointer-extras-unproven-on-hardware -->
    **SERVED, and unproven on hardware.** `relative_motion` and four gesture
    events (`gesture_begin`, the two updates, `gesture_end`) on
    `vitrin_shim_seat`; a `pointer_constraint` ask-and-verdict pair on
    `vitrin_shim_session`; and three shim globals —
    `zwp_relative_pointer_manager_v1`, `zwp_pointer_gestures_v1` and
    `zwp_pointer_constraints_v1`. **No run has yet delivered any of them to a
    connected application.** CI has no touchpad and no DRM device, so the
    evidence behind this row is unit and component tests, not a mock-free gate;
    a named `tests/integration/test_real_*.py` rung on the target machine is
    owed and is not yet written. Two-finger scroll is **not** in this set: it
    has always worked, as a scroll axis.
  - <!-- limit: gesture-ends-wrong-way -->
    <!-- limit-not-on-page: pointer-lock-release-unobservable-in-ci -- no
    surface carries it: it is a statement about what CI cannot observe rather
    than about something a reader meets, and this bullet is its only record
    -->
    **SERVED, with two gaps named rather than smoothed over.** A pointer
    lock deactivates and the human's cursor sprite returns on every path the
    core knows about, but that property can only be observed on bare metal —
    nested and headless draw no human sprite at all — so it is the one
    behaviour in this workstream that CI is structurally unable to check. And
    an in-flight gesture is ended `cancelled` on a realm switch and a seat
    pause, but **not** when a consent card or the lock screen raises: those
    withhold the gesture's updates and then deliver the device's own end, so an
    app that was previewing a zoom is told the human completed what they in
    fact abandoned. Closing that is owed, and **WS-E.4.4/#224 published it** on
    `docs/book/src/limits.md` beside the touch/tablet paragraph rather than
    leaving it as an unpublished note here; it stays off `README.md` and
    `site/index.html` by the stated-subset decision, and #222 owns it.
  - <!-- limit: no-touch-no-tablet -->
    **NOT YET SERVED: touch, and tablet or stylus.** Neither has a wire event,
    and `wl_touch` stays out of the shim's seat capabilities (the comment
    heading is `TOUCH IS NOT YET SERVED`) because a class advertised with
    nothing behind it is worse than an absent one — a toolkit that sees TOUCH
    stops installing its pointer fallbacks. Both are deferrals with named
    reopening evidence, not permanent decisions: **touch** reopens on a
    touchscreen in the measured device set *together with* an application that
    needs it; **tablet** reopens on a pen or stylus device in that set, the
    application half of its evidence being already on record. This machine has
    neither device, which is a measurement of one laptop and not a property of
    the protocol. **Published in that register, with the reopening evidence
    named, by WS-E.4.4/#224** — on `docs/book/src/limits.md` and `README.md`,
    and deliberately not on `site/index.html`, where one line cannot carry the
    deferral-versus-refusal distinction the owner corrected #222 on.
  - <!-- limit-not-on-page: lid-switch-delegated-to-logind -- published with
    the recovery runbook, which is where the logind values it delegates to are
    printed; the limits page does not restate them -->
    **NOT A SEAT QUESTION: the lid switch**, handed to WS-E.4.3/#223 and
    **decided there** ([§4.4](#44-session-lifecycle-build-what-the-hardware-forces-delegate-the-rest),
    [D-033](20-decision-log.md#d-033--idle-blanks-the-screen-and-does-not-lock-it-suspend-is-detected-after-the-fact-or-not-at-all-and-the-recovery-path-is-sudo-only)):
    delegated to logind, no wire event, and the `logind.conf` values it depends
    on published with the recovery runbook because this repository does not own
    them. Wayland clients do not receive switch events at all — the compositor
    consumes them, and on this machine logind does — so a wire message for one
    would sit under something no application could use.

- <!-- limit: idle-blank-does-not-lock -->
  **Idle blanks the screen and does not lock it, so a dark panel is an
  *unlocked* session** (created by WS-E.4.3/#223, and **the owner's decision of
  2026-08-10** — [D-033](20-decision-log.md#d-033--idle-blanks-the-screen-and-does-not-lock-it-suspend-is-detected-after-the-fact-or-not-at-all-and-the-recovery-path-is-sudo-only)).
  `--blank-idle` turns the panel off after a period with no physical input; the
  session behind it stays unlocked, and any physical input restores it.
  Locking remains the human's manual `Ctrl-Alt-Delete`, and the two are
  deliberately **not coupled** — coupling them would have made a comfort feature
  into a security control nobody chose. **Anyone who walks up and touches a key
  gets the session, not a passphrase prompt**, which is worse than what this
  machine does under Hyprland today. Published unsoftened in
  `docs/book/src/limits.md`; the owner's trade, stated as one.

- <!-- limit: blank-does-not-stop-observation -->
  **A dark screen is not evidence that nothing is watching** (created by
  WS-E.4.3, and **the same policy as the lock**, not a second accident). An
  agent holding `observe` keeps capturing the realm while the panel is off,
  frame for frame — exactly as
  [D-025](20-decision-log.md#d-025--a-locked-screen-does-not-suspend-agent-observation-the-gap-is-published-not-papered-over)
  decided for a lock on 2026-08-08, on the same ground: the grant is the
  authority, not the human's gaze. It is published *citing* the lock entry
  rather than beside it, so the two read as one posture. The instrument for
  "stop everything" is unchanged and works in the dark for a structural reason:
  the dead-man switch detects in the router's unconditional `observe` tap, which
  no gate can suppress, so the very press that wakes the screen also starts the
  hold. Published in `docs/book/src/limits.md`.

- <!-- limit: blank-stops-the-frame-clock -->
  **A blank stops every realm's frame clock, so it halts every agent in the
  session** (created by WS-E.4.3, and the sharpest thing in this section).
  CRTC disabled → no vblank → `DrmState::on_vblank` never runs →
  `session::emit_presented` is never called → no `frame_done` is discharged →
  every paced client stops painting. So the sentence above is true and
  incomplete: the agent does not *keep seeing*, it is served **the pre-blank
  frame indefinitely, with no staleness signal and no refusal**.
  [D-030(6)](20-decision-log.md#d-030--the-trusted-band-asserts-only-about-the-screen-this-core-is-driving-and-the-session-colour-is-never-re-minted-a-paused-seat-raises-no-prompt-it-could-record-as-shown)
  already published this for a VT switch and called it *"worse than a stall"*;
  blanking makes it **routine and timer-driven** on an agent-first display
  server rather than something a human causes by leaving. The named fix — a
  software frame cadence for paused realms — is D-030's existing unscheduled
  deferral, inherited rather than re-filed. Published in
  `docs/book/src/limits.md`.

- <!-- limit: idle-inhibit-bounded -->
  **Idle inhibition is served, bounded three ways, and unproven on hardware**
  (created by WS-E.4.3 as a *not yet*; discharged by #306 and
  [D-042](20-decision-log.md), and re-published as a **bound** rather than
  deleted). `zwp_idle_inhibit_manager_v1` is advertised by the shim and relayed
  over `vitrin_shim_session.idle_inhibit`. The three bounds: only the realm the
  human's output is bound to can hold one; it suppresses the idle **blank** and
  never the idle **lock** (D-033(1)), so a film longer than `--lock-idle` still
  gets a lock screen over it; and **no run on real hardware has confirmed a
  video keeping a panel lit** — blanking needs a display controller, CI has
  none, and `shim/tests/acceptance/idle_inhibit.sh` is a component test against
  `mock_core.c`. **What would close the third bound:** a human watching a video
  under `--drm --blank-idle` and pasting the result, which is #223's own
  hardware-rung debt inherited rather than re-filed. Published in
  `docs/book/src/limits.md`, `README.md` and `site/index.html`.

- <!-- limit: media-keys-reach-an-app-that-cannot-act -->
  **The media keys reach an app that cannot act on them** (created by WS-E.4.3,
  and an honest half-fix rather than a fix). The XF86 rows stop those keys being
  dropped at intake with a trace line — but a delivered `XF86AudioRaiseVolume`
  reaches the focused realm's shim seat, and no confined app can open a mixer.
  So the human still presses volume and nothing happens; what changed is *where*
  it stops. **Volume actuation is deferred**, reopened by a shell client holding
  a named verb — which [D-039](20-decision-log.md) makes newly plausible — or by
  an explicit owner decision of the kind D-041 records for the backlight. There
  is no one-file sysfs equivalent for a mixer and every route to one runs through
  a sound server, a bus or socket client inside the TCB, which is the dependency
  D-033(4) refused for logind. **Published by WS-E.4.4/#224** on
  `docs/book/src/limits.md` beside the idle and blank entries, and as one bullet
  on `README.md`; omitted from `site/index.html` by the stated-subset decision
  above, because at one line it reads as a fix. The volume half has **no issue**,
  and the reason is that the decision it waits on has never been put to the owner
  rather than that nobody cares.

  > **THE BRIGHTNESS HALF CLOSED 2026-08-17** by D-041 and
  > [#303](https://github.com/vitrin-os/vitrin-os/issues/303), which is the only
  > issue this limit has ever had. On `--drm --backlight` the core consumes both
  > brightness keys and writes `/sys/class/backlight` itself — so the sentence
  > above is now about **volume**, and the anchor id is kept rather than renamed
  > because the limit it points at did not go away, it halved. What did **not**
  > change, and is published in the same words on both surfaces: it does nothing
  > for an external display, nothing on a session without the flag, nothing on
  > nested or headless, and it costs the two keys reaching apps at all. The
  > register above is deliberately **not** rewritten as "the media keys work
  > now"; that sentence would be wrong in three directions at once.
  >
  > It also **widens the core's future self-sandbox from read to write** — the
  > first write rule the **core's own** Landlock ruleset will owe — which is
  > recorded here, in `crates/vitrin-core/src/backlight.rs`'s module docs and in
  > the `status-strip-reads-sysfs` bullet of `docs/book/src/limits.md`, on
  > `battery.rs:32-39`'s three-surface precedent. That ruleset is
  > [#314](https://github.com/vitrin-os/vitrin-os/issues/314).
  > This sentence named `#187` until 2026-08-23 and that was wrong, not merely
  > out of date: #187 wrote the **realm's** ruleset in `vitrin-realm-init`,
  > which runs after a `pivot_root` away from `/sys`, and it never owned a rule
  > about `vitrind`'s own process at any point in its life. D-041 routed the
  > debt there; the correction is appended to that entry rather than written
  > over it.

- <!-- limit: no-key-repeat-on-drm -->
  **A held key does not repeat on `--drm`** (pre-existing since WS-E.3.1 and
  **found by this sweep rather than by using the session**, which is the honest
  provenance and the reason it is stated as unconfirmed). `shim/src/seat.c` sets
  `wlr_keyboard_set_repeat_info` to a rate and delay of zero — a good decision,
  because repeat is seat-wide and this seat carries an agent's actuations beside
  the human's, so a client-side timer would repeat an agent's held key — and its
  comment claimed *"the core repeats instead, for physical-origin presses only"*.
  **No such code has ever existed.** Off a host libinput synthesizes no repeat,
  so on bare metal a held key produces exactly one character; nested is
  unaffected because the host compositor repeats and the core forwards each
  event. The comment is corrected in place and the limit is **published by
  WS-E.4.4/#224** on `docs/book/src/limits.md` and `README.md`, stated as
  unconfirmed on hardware: CI cannot hold a seat, and the 2026-08-11 session
  that typed 399 keystrokes into alacritty did not test for it. It has **no
  issue**; D-028(5)/#217 is the decision whose second half is missing.

- <!-- limit-not-on-page: logind-settings-not-owned-here -- published in
  docs/book/src/recovery.md, because the values are read from the running
  system and a limits page cannot carry a number this checkout does not own
  -->
  **Behaviour now depends on files this repository does not own** (created by
  WS-E.4.3, and unavoidable given the delegation above). Lid, power-key and
  suspend-key policy is logind's, so *"suspend works"* is not reproducible from
  this checkout. The mitigation is publication, not ownership: the values are
  read from the running logind and printed in
  [the recovery runbook](../book/src/recovery.md#the-settings-this-depends-on-which-this-repository-does-not-own),
  with the note that `/etc/systemd/logind.conf` is empty on this machine so every
  one of them is a systemd default that can move without any change here. A run
  recorded under different values is a run of a different system.

- <!-- limit-not-on-page: session-mode-tcb-growth -- no surface carries it: it
  is a cost to this project's trusted computing base rather than something a
  reader of the limits page meets, and this bullet is its only record -->
  **TCB growth for zero differentiator, exactly as #223 predicted** (created by
  WS-E.4.3). An idle state machine, a cover surface, a fourth flip-gating term, a
  third `PromptVisibility` variant, an activity clock lifted out of the lock, and
  a wider invariant keysym table — all inside the trusted core, none of it
  anything a user would choose this project for. [PRD](../PRD.md) §5.3's warning
  about the support treadmill, paid in full and recorded as paid.

- <!-- limit: lifecycle-checklist-run-once -->
  **Suspend and lid have been exercised once, on 2026-08-11, short of the counts
  the rungs ask for** (created by WS-E.4.3; **narrowed, not closed**, by the
  first execution of `L1`–`L6`). Until that date this read *"never exercised by
  anyone, in any form"*, and it was true: the bring-up page's checklist runs
  7–15 and contains no suspend, lid or blank rung at all, so those rungs had to
  be *written* before they could be executed. They are `L1`–`L7` on
  [the recovery runbook](../book/src/recovery.md#the-hardware-checklist), and
  the owner ran `L1`–`L6` and pasted the numbers into #223. **It is not a clean
  pass.** `L1` 10/10; `L2` **4 of 5** cycles, all returning a working panel;
  `L3` **2 of 5** lid cycles of which only one ever reached sleep, so **one
  usable lid sample** and nothing established about a short lid close; `L4`
  blank at 61.2 s; `L5` no lock card; `L6` recovered in ~69 s **by a route that
  could not be reconstructed afterwards**, which leaves the rung's actual
  question unanswered. Route 3 (`/proc/sysrq-trigger`) is still documented and
  unexecuted and the advisory VKMS rung was never attempted. Four defects came
  out of the run: #257, #258, #259 and #260. **`L7` was written from #257 after
  the run, and was itself executed later the same day** at a 20 s timeout: the
  panel stayed lit on the return and the lock did not raise, so **#257 is
  settled on hardware** — but by eye, with no figure recorded, so the rung's
  own question (how long the panel stayed lit) is still unanswered. The fixes
  for **#258 and #259 remain unobserved**: they concern the wake's log line and
  recorder pair, and neither was read during that run. So a hardware criterion
  of #223 may now be cited **only at the count and scope actually recorded** —
  never as *"suspend works"* or *"lid works"*.
- <!-- limit: realm-cap-arithmetic -->
  **The cap is sixteen *simultaneously live* realms, and no principal can end
  one** (created by WS-E.1.2/#208, which raised the cap from one;
  [#234](https://github.com/vitrin-os/vitrin-os/issues/234) owns the second
  half). Both halves have to be published together or each becomes a lie:
  `Realm::occupies_capacity` excludes the terminal state and the launch refusal
  reads `capacity_used()` rather than `len()`, so a slot comes back when a
  realm's own app exits — and nothing else returns one, because revocation,
  disconnect and the dead-man switch all leave the process running. So the
  human's remedies for a realm they no longer want are the app's own quit path,
  killing the process from a terminal, or restarting `vitrind`; the display
  server offers none. Published in `docs/book/src/limits.md`, and held by
  `limits-check`'s `realm-cardinality` claim. **Added to this enumeration by
  WS-E.4.4/#224**, for the same reason as the one-output entry above.

- <!-- limit-not-on-page: capture-could-carry-a-siblings-pixels -- closed by
  WS-E.1.3; the limits page publishes the closed property instead, and a gap
  that is shut must not be republished as a live one -->
  ~~**Several realms run, one is visible, and a capture cannot tell them
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
- <!-- limit: every-realm-renders -->
  **Every realm renders, visible or not** (created by WS-E.1.3, no owner).
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
- <!-- limit: agent-cursor-visible-realm-only -->
  **The agent cursor is drawn only for the visible realm** (created by
  WS-E.1.3, fixed by a per-realm indicator nobody has scheduled). D-019 added
  the sprite so a human can see that an agent is acting. It is painted into
  the output, which shows one realm, so an agent actuating inside a hidden
  realm draws nothing — the exact defect D-019 exists to close,
  reintroduced for hidden realms. Published in `docs/book/src/limits.md`.
- <!-- limit-not-on-page: one-realm-actuable-rest-refused -- closed by
  WS-E.1.6/#212; kept struck through as a record, for the same reason as the
  entry above -->
  ~~**Only one realm can be actuated, and the rest are refused rather than
  misdelivered**~~ (created by WS-E.1.2, **closed by WS-E.1.6/#212**). The
  write-side twin of the item above, and the one the same review found
  second. There was one input router and one delivery target
  (`session::seat_target`), so an actuation admitted under a grant naming
  *another* realm would have been delivered into a sibling's app — an agent
  driving an app it holds no authority over, which is a **write**. WS-E.1.2
  refused it `internal` instead, deliberately choosing a stopgap over half a
  routing model; WS-E.1.6 deleted the guard **and** the placeholder, because
  a real router makes the comparison unnecessary rather than merely passing.
  With it went the cross-principal denial of service a `layout_focus` holder
  briefly had over every other principal's actuations.

  The router now keeps one seat's state per realm and follows two addressing
  rules that never move each other: physical input goes to the **bound**
  realm (the human's attention, which `layout_focus` moves), an agent's
  actuation goes to the realm its **grant** names. `PhysicalPresence` is per
  realm on the same grounds.

- <!-- limit: per-realm-presence-narrows-preempted -->
  **Per-realm presence narrows a blanket safety behaviour** (created by
  WS-E.1.6, no owner). The price of the item above, and it is a *loss* worth
  stating on its own: `preempted` used to mean "a human is touching something
  in this session", so a hand on the keyboard suspended every agent
  everywhere. It now means "a human is touching *this realm*". That is the
  correct reading of the IDL's own words ("physical human input owns **the
  target**") and it is also strictly less refusing than before, with no wire
  event to tell an agent — or a human — that the breadth is gone. Layout
  requests keep the old breadth, because they move what the human is looking
  at rather than being delivered into a realm. Published in
  `docs/book/src/limits.md`.

- <!-- limit: realm-switch-releases-held-input -->
  **A realm switch mid-gesture tells the app the human let go** (created by
  WS-E.1.6, no owner). A key or button held across a binding change is
  released into the realm being left, because the human's real release will
  be delivered to the realm they moved to. The app cannot distinguish that
  release from a real one. The alternative is a latched modifier or a wedged
  pointer grab in an app the human can no longer see, which is worse — it is
  the same trade `InputRouter::release_physical_keys` already made for host
  focus loss — but it now happens on every switcher keypress rather than only
  on alt-tab. Published in `docs/book/src/limits.md`.

- <!-- limit-not-on-page: physical-presence-fed-by-nothing -- closed by
  WS-E.1.6/#212 itself; same reason as the two entries above -->
  ~~**`PhysicalPresence` is still fed by nothing in production**~~
  (pre-existing, surfaced by WS-E.1.6, **closed by WS-E.1.6/#212 itself**).
  `PresenceHook` was an *optional* member of the router's hook stack and no
  shipping backend included it, so no build ever called
  `PhysicalPresence::note` and the `preempted` refusal could not fire at
  runtime while the book described it as live behaviour. #212's review deleted
  the hook: `InputRouter` holds the presence map itself, above the stack, writes
  it in `route_into`, and `Runtime::new` takes the kernel's map *out of the
  router* — so a router that does not feed presence, or a kernel whose presence
  is not its router's, is now unconstructible rather than a mistake nobody made
  on purpose. (This bullet described the state during #212 and was left
  uncorrected when it landed; corrected in place by WS-E.1.7, which had to read
  the hook stack to add to it.)

- <!-- limit: super-is-taken-everywhere -->
  **The core owns a second physical chord, and it eats Super** (created by
  WS-E.1.7/#232, no owner). Tapping Super opens a one-second, single-use window
  in which a layout holder is not refused `preempted`, and delivers an
  argument-free `attention` event to every layout holder. It closes the loop
  that made an in-realm shell unusable — the Enter that sends `focus editor` is
  the physical input that forbids it — and it costs the human that key
  **everywhere**: a nested compositor, a VM viewer or a remote-desktop client in
  a realm loses it with no pass-through and no way to ask for one. That is the
  cost `deadman.rs` refused to pay for Escape, paid here for a different key on
  the argument that every desktop already reserves it. The only remedy is
  `--attention-chord rsuper`, which is not a remedy. Published in
  `docs/book/src/limits.md`.

- <!-- limit: attention-window-is-session-wide -->
  **The attention window is session-wide, and the delivered-to set only narrows
  it** (created by WS-E.1.7, and unfixable here — D-023(2)). Any principal the
  `attention` event reached may claim the window; the core cannot tell which of
  two layout holders the human meant. Fixing it means choosing a shell, which is
  the window-management policy PRD §5.1 exiles from the core, and the mapping
  that would make it structural — "the principal that draws in the bound realm"
  — does not exist, because a realm and a principal connection have no binding
  (#211's decision 2 names the same gap). A second holder can consume a press the
  human aimed at the shell; the human's own switch then silently fails and the
  thief's lands. It is journaled with the claiming principal and the grant is
  revocable, and it is still a hole. Published in `docs/book/src/limits.md`.

- <!-- limit: clipboard-is-a-bounded-channel -->
  **A cross-realm channel exists now, with a stated bandwidth** (created by
  WS-E.2.1/#213, no owner). [PRD](../PRD.md) §15's first threat row used to say
  a malicious app cannot *"reach the session's real seat/clipboard/a11y bus"*.
  After #213 it can reach a **clipboard** — through a human gesture, one
  direction at a time, `text/plain;charset=utf-8` only, 60 KiB at a time. Two
  colluding realms can therefore move 60 KiB per human gesture pair. The honest
  statement is that bound, never *"there is no channel"*; the PRD row is edited
  rather than left standing, and the channel is published in
  `docs/book/src/limits.md`. See §4.1 for the bound's derivation.

- <!-- limit: tcb-stores-application-bytes -->
  **The TCB stores application-authored bytes for the first time** (created by
  WS-E.2.1, and the maintainer's own accepted cost — [D-024](20-decision-log.md)).
  Nothing else in the core does: it holds client *pixels* it never interprets and
  typed values it validated itself. A password copied from a manager now transits
  `vitrind` and rests in a slot with a lifetime, so a compromised core exposes
  whatever was copied last. The cap, the one-MIME allow-list, digest-only
  journaling, the idle timeout, the source-realm-death clear and the dead-man
  clear bound it; **none removes it**. Published in `docs/book/src/limits.md`.

- <!-- limit: clipboard-chords-taken -->
  **The core eats two more physical chords, and one of them is a paste key**
  (created by WS-E.2.1, no owner). Ctrl-Shift-Insert and Shift-Insert are
  consumed in every realm, unconditionally. Shift-Insert is the historical X11
  primary-paste chord, so an app that binds it loses it with no pass-through and
  no way to ask for one — the third time this workstream has paid that price
  (Escape refused it, Super paid it in D-023). What makes it affordable rather
  than merely paid: the loss is *inside* the realm only, both halves of each
  press are consumed so no app can even tell, and the gesture the human lost is
  the gesture they are being given across realms instead.

- <!-- limit: preempted-now-depends-on-hidden-state -->
  **`preempted` on the layout verbs is conditional on invisible core state**
  (created by WS-E.1.7, no owner). An agent reading its own journal can no longer
  reconstruct why one `focus` landed and an identical one did not without
  correlating the core's attention entries, which it cannot see. The refusal used
  to mean one thing. Published in `docs/book/src/limits.md`.

- <!-- limit: principal-cannot-draw -->
  <!-- limit: principal-has-no-hotkey -->
  **A principal cannot draw, and cannot receive physical input** (pre-existing,
  *surfaced and priced* by WS-E.1.5/#211, no owner). Neither is new and neither
  was written down as a user-facing limit until a switcher had to be built
  against them. `vitrin_view` is capture-only and there is no principal-facing
  surface interface in the IDL, so no client can put a pixel on the output —
  which is why the shipped switcher is a line-oriented host-side program and
  not a placeholder for a graphical one. There is no `observe_input` verb and
  none is designed, so no client has a hotkey; the core owns eight physical
  gestures and owns every one of them because they must not depend on a client
  (enumerated in `docs/book/src/limits.md` under `principal-has-no-hotkey`). The
  consequence for a daily driver is blunt: **every layout change starts as a
  line typed into a terminal that must be somewhere the human can reach.** The
  eventual shape #211's decision 2 names — the shell running *as a realm*,
  drawing through its own shim — needs no new protocol and does need that
  realm to reach the core socket, which is a confinement question nobody has
  answered. Published in `docs/book/src/limits.md`.

- <!-- limit-not-on-page: layout-arrange-is-single-holder -- no surface
  carries it: it is a restriction on a second tool rather than on the human's
  session, and this bullet is its only record -->
  **The shell holds `layout.arrange` for the whole output** (created by
  WS-E.1.5, and designed rather than accidental — D-018(4)). Arrangement is
  single-holder per output, checked at admission, so while the switcher lives a
  second tool that wants to arrange anything resolves `layout_held` before it
  reaches a prompt. The shipped shell therefore petitions arrangement over
  exactly one realm and names that realm on every `fullscreen`. It is the
  correct behaviour and it is also a restriction people will hit before they
  understand why.

- <!-- limit: lock-does-not-stop-agents -->
  **A locked screen an agent can still watch** (created by WS-E.2.2/#214,
  and **decided rather than deferred** — [D-025](20-decision-log.md#d-025--a-locked-screen-does-not-suspend-agent-observation-the-gap-is-published-not-papered-over)).
  The lock screen consumes every physical event and covers the output, and it
  does not touch a grant: an `observe` holder keeps capturing the realm across
  a lock and an `actuate_*` holder keeps acting. Correct against the IDL
  (observation is concurrent by design), argued, taken to the maintainer in
  plain terms on 2026-08-08, and genuinely surprising to a human — which is
  why it is on the lock card itself as well as in
  `docs/book/src/limits.md`, rather than in a code comment. The instrument for
  "stop everything" remains the dead-man chord, which fires while locked.
- <!-- limit: nested-lock-locks-a-window -->
  **In nested mode the lock screen locks a window** (created by WS-E.2.2, and
  a Stage-3 item by construction). `vitrind` is a client of the host
  compositor; the host owns the real session and anyone can alt-tab away.
  Stages 1–2 therefore ship a privacy cover, not an authentication boundary
  for the seat. Published.
- <!-- limit: no-vt-switch-inhibition -->
  **No protection against VT switching, and the fix is a worse trade**
  (created by WS-E.2.2, **decided by WS-E.3.3 / D-030**). On bare DRM
  `Ctrl-Alt-F<n>` walks past the lock unless the core inhibits it, and
  inhibiting it means a session a human cannot leave when the compositor
  wedges. §7's safety rule and this item pointed at the same Stage-3 decision,
  and D-030 took it: **no inhibition, and the trusted band is scoped to the
  screen this core is driving instead** — plus the answer to the question this
  bullet did not ask, which is that a switch away does *not* raise the lock
  (it would claim a protection the core does not have and charge the human a
  passphrase for using the escape hatch). Published, in the human's words, in
  `docs/book/src/limits.md`. **Still the default, and now one of three**
  (issue #246, D-034): `--lock-on-seat-change immediate` locks on the way out
  for an operator who wants that trade, `idle` charges the absence to the idle
  countdown, and `never` — what every session that says nothing gets — is the
  behaviour this bullet describes. No policy lowers a lock that is already up.
- <!-- limit: passphrase-is-not-headless -->
  **A passphrase is nested-only, because a headless backend has no keyboard** (created
  by WS-E.2.2, closed only by Stage 3 answering the keymap question). `--lock-passphrase-file`
  is refused at startup with `--headless`, naming the reason. Without it the
  lock is an unauthenticated privacy screen and the card says so. Growing an
  xkbcommon keymap was refused here rather than deferred quietly:
  `input/mod.rs:106-109` records that a real keymap moves key pairing from the
  keysym to the scancode — a router invariant the dead-man switch depends on —
  and that is Stage 3's decision, which a lock-screen issue must not pre-empt.
- <!-- limit-not-on-page: kdf-in-the-tcb -- no surface carries it as a limit:
  the limits page names Argon2id where it describes the passphrase path, but
  the four-crate dependency cost is recorded here and in
  crates/vitrin-core/Cargo.toml only -->
  **A KDF is now in the TCB, and it processes operator-supplied input**
  (created by WS-E.2.2, no owner). Four crates (`argon2`, `base64ct`,
  `blake2`, `subtle`), measured rather than estimated, inside the most
  privileged component. Unlike fontdue — whose justification turns on "the
  only bytes it parses are a compile-time constant" — this one really does
  process bytes an operator supplied, though never bytes that arrived over the
  wire. Issue #201 records that `deny.toml` and the `cargo-deny` job still do
  not exist, so `crates/vitrin-core/Cargo.toml`'s comment is the only place
  this budget is checked.
- <!-- limit: lock-chord-taken -->
  **A fourth gate in the input stack, and a fourth chord taken from every
  app** (created by WS-E.2.2). `deadman.rs` spends its module docs proving no
  gate bug can stop the off-switch; every gate added is a new chance for that
  proof to stop being true. The compensating controls are structural rather
  than documentary — the lock's policy implements `ConsumingGate`, which has
  no observation method, so the observe tap is forwarded by code in
  `crate::input` that has no notion a lock exists — plus an adversarial test
  through the real stack. What it also costs: `--dead-man-chord delete` is now
  refused on an otherwise default command line, because the default lock chord
  is `ctrl+alt+delete`.
- <!-- limit-not-on-page: lock-adds-resident-frame-path-state -- no surface
  carries it: it is a cost inside the compositor rather than a behaviour a
  reader meets -->
  **New always-resident core state on the frame path** (created by WS-E.2.2).
  A second `ConsentSurface`-shaped cost: one more `Option<LockContent>`, one
  more cached raster and one more generation counter per backend, and a raised
  lock forces the CPU compositing path exactly as a consent card does — which
  on the WS-E laptop means the zero-copy dmabuf branch is off for as long as
  the screen is locked.
- <!-- limit-not-on-page: top-strip-composite-order -- not a limit a reader
  meets: a design record, stated once in crates/vitrin-core/src/status/mod.rs,
  and kept here so the collision it resolved is not re-litigated -->
  **The top strip has now been designed as a whole** (created by WS-E.1.7,
  resolved by WS-E.2.3/#215, which landed third and therefore owned the
  collision). The dead-man hold bar, the attention marker and the status strip
  were each designed without the others. The composite order is now stated once,
  in `crates/vitrin-core/src/status/mod.rs`'s module docs and in
  `backend::human_visible_from_view`, and it is:

  ```text
  realm view -> consent overlay -> lock cover -> STATUS STRIP -> trusted band
             -> attention marker ... -> dead-man hold indicator (last of all)
  ```

  The decisions, each with its reason:
  - The **band** keeps rows `[0, 8)` alone, exactly one colour, and nothing —
    core-drawn or not — is composited into them. The strip's raster is the
    strip's own height and is blitted at `y = TRUST_BAND_HEIGHT`, so no
    coordinate expressible in the renderer lands in the band.
  - The **strip is drawn over the lock cover**, not under it. #215 left this to
    whoever landed second; #214 landed first, so #215 decided it. A clock is the
    one thing a human wants on a lock screen, and "the strip is always there"
    must have no exception. It leaks nothing the lock hides: every field is
    core-owned, and the realm id it names is one the lock card already prints.
  - The **attention marker keeps a lane**: the strip's content starts past
    `attention::MARKER_W`, read from that constant and never restated, with a
    `const` assertion that turns an overgrown marker into a compile error.
  - The **dead-man hold indicator is still composited last of all**, so nothing
    added here can hide a hold in progress.
- <!-- limit: status-strip-overdraws-the-view -->
  **The status strip is opt-in, and the realm view is still NOT inset**
  (created by WS-E.2.3). #215 asks for the app to be *configured* smaller by
  `TRUST_BAND_HEIGHT + STATUS_STRIP_HEIGHT`; that is unmet, and the strip
  therefore overdraws 20 rows of client content the way the band already
  overdraws 8. A correct inset needs one usable-view value reaching
  `scene::layout::place` from three paths (`Scene::compose`, the router's
  `surface_local`, `dmabuf::human_visible_frame`) that share no carrier for a
  second number, plus the `configure` size the shim is told — a `ViewGeometry`
  refactor of its own, and a half-done version (one path reserving rows the
  others do not) is strictly worse than none. `--status` is off by default so
  no session pays the overdraw without asking; the cost is published in
  `docs/book/src/limits.md`.
- <!-- limit: status-strip-reads-sysfs -->
  **A recurring filesystem read inside the TCB** (created by WS-E.2.3). Before
  it, the core read `realm.toml` and `principals.toml` once at startup and
  embedded its font at compile time; `crates/vitrin-core/src/status/battery.rs`
  now reads `/sys/class/power_supply` on a 30 s cadence. Bounded to one fixed
  root, at most 16 directory entries, and at most 16/32/32 bytes per attribute
  through `Read::take` — with every failure collapsing to an empty slot rather
  than a guess. **When E2.6/E2.7 put Landlock over the core's own process this
  becomes a rule the core must grant itself**, which is a real widening of that
  future sandbox and is recorded here so the ruleset's author does not have to
  rediscover it. That author has an issue as of 2026-08-23:
  [#314](https://github.com/vitrin-os/vitrin-os/issues/314),
  which carries this read rule together with the backlight write rule the
  brightness bullet above describes. `battery.rs`'s own module docs are
  deliberately left without that citation: D-041 quotes six line numbers from
  that file and inserting a paragraph into it would move every one of them.
  This is where a reader of the sysfs limit is pointed instead.

- <!-- limit: screenshots-are-world-readable-to-realms -->
  **A file on disk that is a picture of the human's screen** (created by
  WS-E.2.4/#216, no owner until E2.6/E2.7 confine the core). The core already
  writes the recorder's log and the `--capture-dump` diagnostic, so this is not
  its first descriptor; it is the first whose *contents* are the screen. Every
  screenshot is readable by every app in every realm, because everything runs
  as one uid (D9). The mode is `600`, which is about other users and not about
  the confined app. The directory is audited at startup and then **held open**
  for the process's life, so the path is resolved exactly once, before any
  client exists; until the core is confined, that audit is the whole of the
  enforcement of the operator's choice.
- <!-- limit: screenshot-cannot-show-a-prompt -->
  **A vitrin screenshot cannot show a consent prompt, and that is the design**
  (created by WS-E.2.4, and unfixable without giving up the trusted indicator).
  The file is the **realm view**: no trusted band, no card, no ring, no lock
  cover, no status strip, no agent cursor. "Send me a picture of that weird
  dialog" is the single most useful thing a screenshot does and this one cannot
  do it; the answer is a phone camera, which is worse than every other desktop
  offers.

  The reason is that the band's colour **is** the session secret
  (`consent/indicator.rs`: *"never written to any descriptor or file"*), and a
  same-uid app can read anything the core writes — so one screenshot of the
  human-visible composite would end the indicator's usefulness for the rest of
  the session. Two softer designs were examined by #216's implementation and
  both are worse, which is why this bullet is a limit rather than a TODO:
  - **Cropping the band's rows out does not close it.** A genuine consent card
    is framed in the same colour in the *middle* of the output, so a crop
    protects the secret only while no prompt is up — exactly when a screenshot
    is least wanted.
  - **Redacting by colour opens an oracle.** Replace every pixel equal to the
    secret and an app can paint a field of candidate colours, ask the human for
    a screenshot, and read back which of *its own* pixels were recoloured. A
    ~22-bit secret (`TrustedIndicator::generate` scales each channel into
    `[64, 255]`) falls in a handful of screenshots at 1080p. This is the sharper
    of the two findings and it is recorded here because it looks thorough.

  Published in `docs/book/src/limits.md` and in `--help`.
- <!-- limit: screenshot-chord-taken -->
  **A fifth core-owned chord, and the first one that pays a cost back**
  (created by WS-E.2.4). Ctrl-PrintScreen is consumed in every realm. It is a
  *chord* rather than #216's proposed bare `Print` for a structural reason —
  `crate::chord::ModChord` refuses a modifier-less chord, so a bare-key gesture
  would have meant a second matcher in the stack the off-switch lives in, which
  is the thing [D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing) exists to forbid — and the
  side effect is that **bare PrintScreen is still delivered**, so an app that
  binds it keeps it. That is the first time this workstream has taken a gesture
  without taking the key. The collision check `main.rs` runs is now over five
  chords rather than four.
- <!-- limit-not-on-page: png-encoder-in-the-tcb -- no surface carries it: a
  trusted-computing-base dependency cost, recorded here and in
  crates/vitrin-core/Cargo.toml -->
  **A PNG encoder is now TCB code** (created by WS-E.2.4). `crates/vitrin-png`
  is the golden harness's hand-rolled artifact encoder, promoted to a
  zero-dependency in-tree crate both it and `vitrin-core` depend on, because
  *"no image codec in the core, in any dependency class"* is stated twice in
  `crates/vitrin-core/Cargo.toml` and an external `png`/`image` crate would have
  brought a **decoder**. The mitigating fact is that there is no decoder here at
  all: it consumes a buffer the core composited and returns bytes the core
  writes, so it never parses attacker input — the same argument `Cargo.toml`
  already makes for fontdue. What it costs: ~130 lines written for test
  artifacts must now be reviewed to a TCB bar, and that argument holds only for
  as long as nothing else in the core feeds it bytes it did not itself produce.

<!-- limit-set: end -->

### Measurements taken for WS-E.2.3

Numbers stated in `crates/vitrin-core/src/status/` come from here, so a later
reader can re-run them rather than trust them.

- **Battery read cost.** `read_battery` against the real
  `/sys/class/power_supply` on the WS-E laptop (two devices: `ADP1` mains, `BAT1`
  battery), 10 000 iterations, 2026-08-08:
  **8.90 us/call in `--release`, 10.97 us/call in the debug profile.** At the
  30 s `BATTERY_INTERVAL` that is 0.30 us of work per second of session.
  Measured by timing a loop of `read_battery(Path::new(SYSFS_ROOT))` in a
  temporary `#[test]` in `status/battery.rs`; the test is not kept, because a
  timing assertion in CI is a flake generator.
- **Type metrics.** `Text::line_metrics(12.0)` on the bundled face reports
  `ascent = 11`, `height = 14`. `DEFAULT_HEIGHT = 20` is 14 + 3 + 3, and
  `MIN_HEIGHT = 16` is the smallest strip that holds the line box plus the
  bottom rule. Pinned by `the_height_flag_range_is_what_the_type_size_needs`,
  so a font change moves the constant rather than clipping a digit.
- **GPU-path cost.** One extra textured quad per presented frame, plus a
  re-upload of one `view_width x 20` RGBA texture whenever the strip's
  generation changes. At 2560x1600 that texture is `2560 * 20 * 4` =
  **200 KiB**, and the generation changes **once a minute** at steady state
  (the clock is `HH:MM` with no seconds; the battery is re-read every 30 s and
  moves a percent far more slowly) — i.e. **~3.4 KiB/s** of bus traffic,
  independent of the frame rate. At 240 Hz that is one re-upload per 14 400
  presented frames. The alternative #215 rejects — forcing the CPU path — would
  cost a full `2560*1600*4` = 16 MiB composite plus upload *per frame*, 3.9
  GB/s at 240 Hz, which is why the strip is a texture and not a fallback.
- **Wakeups.** A `--status` session arms one 1 s repeating timer
  (`session::STATUS_TICK`) so an otherwise idle session's clock can move; the
  repaint itself happens only on the minute that rolls over. A session without
  `--status` arms no timer and makes no clock or filesystem read at all.

## 7. Safety rule, non-negotiable

**A DRM backend takes DRM master and the seat. Running one from inside the live
session kills that session.** Every Stage-3 task runs on an isolated VT or a
second machine, never from inside the running desktop. This is the same hazard
class as injecting input into a live session, and it is written here so no task
has to rediscover it.

**The escape route is a Hyprland-side shell and an installer USB, not SSH — and
VT switching is back, but only because the core now implements it.** This rule
originally required an SSH session from a second machine;
[D-031 (the first of two entries with that number)](20-decision-log.md#d-031--the-drm-bring-up-escape-route-is-a-hyprland-side-shell-and-an-installer-usb-not-ssh-and-not-vt-switching)
(2026-08-09) records the maintainer's decision not to run an sshd, the route
that replaces it, and its cost — **a wedged DRM master with no live console is a
reboot rather than a command.** That entry was amended the same day by the first
bare-metal run, which found `Ctrl+Alt+F<n>` did nothing once `vitrind` held the
display and left the maintainer trapped on `tty3`;
[D-031 (the second)](20-decision-log.md#d-031--the-core-implements-ctrl-alt-fn-itself-because-refusing-to-is-what-trapped-the-human-d-030s-reasoning-stands-and-its-effect-was-its-own-opposite)
then made the core implement the VT chord itself. **Both are numbered D-031;
neither is renumbered, because the id is cited from landed code.** (This
paragraph cited them as `D-028` until 2026-08-10, which is the numbering
collision doing exactly the damage it was predicted to do — in a
safety rule, where the reader most needs the link to land.) The step-by-step
version, written against this machine's actual VTs, cards and connectors, is
[`docs/drm-bringup.md`](../drm-bringup.md) step 0. The rule above is unchanged;
only the recovery path is.

## 8. What this workstream is not

- **Not the horizon item** (§0), and not evidence toward M4.
- **Not a product.** [PRD](../PRD.md) §5.4 renounces displacing Wayland on
  today's human desktop as a project aim; nothing here changes that.
- **Not a reason to stop Phase 2.** WS-E's estimate is roughly Phase 2's
  remaining budget. D-021 records that as an unmitigated cost and a priority
  choice, not a solved problem.
