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
| **4 — long tail** | X11 (defers to E3.2 — **scoped, not built**, WS-E.4.1/#221: six requirements handed to E3.2, the X11-only software measured on this machine, and the interim, all in §4.2) · ~~seat vocabulary~~ (**landed in the tree, unproven on hardware**, WS-E.4.2/#222: `relative_motion` and four gesture events on `vitrin_shim_seat`, a `pointer_constraint` ask-and-verdict pair on `vitrin_shim_session`, three new shim globals, touch and tablet deferred against named reopening evidence, the lid handed to WS-E.4.3 — §4.3, [D-032](20-decision-log.md#d-032--relative-motion-and-pointer-gestures-grow-the-seat-vocabulary-pointer-constraints-grow-the-shim-session-instead-and-touch-and-tablet-are-deferred-against-named-evidence)) · session lifecycle · the honesty sweep | open |

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
- **Owed, and named rather than smoothed over:** cancelling an in-flight
  gesture when a consent card or the lock screen raises (above); and the
  hardware rung itself.

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
this workstream owns, not inherits:

- **No accessibility of any kind.** No screen reader, magnifier, high
  contrast, sticky or slow keys. The semantic channel is **not** a substitute
  for AT-SPI — it serves agents, not humans. A daily driver with no screen
  reader is a real exclusion and is stated as one.
- **No X11**, so no Steam and no legacy application (pre-existing; *scoped and
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
- **No bars, launchers, notifications or OSD** — there is no
  `zwlr_layer_shell_v1` and there will not be one at the app level; the
  replacements are core-owned surfaces.
- **A shell crash loses window management**, because the shell is a client and
  there is no core-side fallback. §3(3)'s invariant is right and this is its
  price. **Measured since WS-E.1.5/#211**, and the shape is narrower than the
  sentence suggests: killing the shell leaves both realms running and the realm
  it last focused still receiving the human's physical input, because the
  binding is core state. What is lost is the ability to *re-aim* — and the
  wedge is that recovering means running the shell again from a terminal which,
  in a real session, must already be the bound realm. Asserted by
  `tests/integration/test_shell.py`; published in `docs/book/src/limits.md`.
- **The DRM backend cannot be tested by CI** — no runner has a DRM device or a
  seat — so it arrives with structurally weaker evidence than anything else in
  the tree. That is an asymmetry against D12 and it is published, not
  discovered.
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
  - **SERVED, and unproven on hardware.** `relative_motion` and four gesture
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
  - **SERVED, with two gaps named rather than smoothed over.** A pointer
    lock deactivates and the human's cursor sprite returns on every path the
    core knows about, but that property can only be observed on bare metal —
    nested and headless draw no human sprite at all — so it is the one
    behaviour in this workstream that CI is structurally unable to check. And
    an in-flight gesture is ended `cancelled` on a realm switch and a seat
    pause, but **not** when a consent card or the lock screen raises: those
    withhold the gesture's updates and then deliver the device's own end, so an
    app that was previewing a zoom is told the human completed what they in
    fact abandoned. Closing that is owed.
  - **NOT YET SERVED: touch, and tablet or stylus.** Neither has a wire event,
    and `wl_touch` stays out of the shim's seat capabilities (the comment
    heading is `TOUCH IS NOT YET SERVED`) because a class advertised with
    nothing behind it is worse than an absent one — a toolkit that sees TOUCH
    stops installing its pointer fallbacks. Both are deferrals with named
    reopening evidence, not permanent decisions: **touch** reopens on a
    touchscreen in the measured device set *together with* an application that
    needs it; **tablet** reopens on a pen or stylus device in that set, the
    application half of its evidence being already on record. This machine has
    neither device, which is a measurement of one laptop and not a property of
    the protocol. Published in that register, with the reopening evidence
    named, by WS-E.4.4/#224.
  - **NOT A SEAT QUESTION: the lid switch**, handed to WS-E.4.3/#223. Wayland
    clients do not receive switch events at all — the compositor consumes them,
    and on this machine logind does — so a wire message for one would sit under
    something no application could use.
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
- ~~**Only one realm can be actuated, and the rest are refused rather than
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

- **Per-realm presence narrows a blanket safety behaviour** (created by
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

- **A realm switch mid-gesture tells the app the human let go** (created by
  WS-E.1.6, no owner). A key or button held across a binding change is
  released into the realm being left, because the human's real release will
  be delivered to the realm they moved to. The app cannot distinguish that
  release from a real one. The alternative is a latched modifier or a wedged
  pointer grab in an app the human can no longer see, which is worse — it is
  the same trade `InputRouter::release_physical_keys` already made for host
  focus loss — but it now happens on every switcher keypress rather than only
  on alt-tab. Published in `docs/book/src/limits.md`.

- ~~**`PhysicalPresence` is still fed by nothing in production**~~
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

- **The core owns a second physical chord, and it eats Super** (created by
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

- **The attention window is session-wide, and the delivered-to set only narrows
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

- **A cross-realm channel exists now, with a stated bandwidth** (created by
  WS-E.2.1/#213, no owner). [PRD](../PRD.md) §15's first threat row used to say
  a malicious app cannot *"reach the session's real seat/clipboard/a11y bus"*.
  After #213 it can reach a **clipboard** — through a human gesture, one
  direction at a time, `text/plain;charset=utf-8` only, 60 KiB at a time. Two
  colluding realms can therefore move 60 KiB per human gesture pair. The honest
  statement is that bound, never *"there is no channel"*; the PRD row is edited
  rather than left standing, and the channel is published in
  `docs/book/src/limits.md`. See §4.1 for the bound's derivation.

- **The TCB stores application-authored bytes for the first time** (created by
  WS-E.2.1, and the maintainer's own accepted cost — [D-024](20-decision-log.md)).
  Nothing else in the core does: it holds client *pixels* it never interprets and
  typed values it validated itself. A password copied from a manager now transits
  `vitrind` and rests in a slot with a lifetime, so a compromised core exposes
  whatever was copied last. The cap, the one-MIME allow-list, digest-only
  journaling, the idle timeout, the source-realm-death clear and the dead-man
  clear bound it; **none removes it**. Published in `docs/book/src/limits.md`.

- **The core eats two more physical chords, and one of them is a paste key**
  (created by WS-E.2.1, no owner). Ctrl-Shift-Insert and Shift-Insert are
  consumed in every realm, unconditionally. Shift-Insert is the historical X11
  primary-paste chord, so an app that binds it loses it with no pass-through and
  no way to ask for one — the third time this workstream has paid that price
  (Escape refused it, Super paid it in D-023). What makes it affordable rather
  than merely paid: the loss is *inside* the realm only, both halves of each
  press are consumed so no app can even tell, and the gesture the human lost is
  the gesture they are being given across realms instead.

- **`preempted` on the layout verbs is conditional on invisible core state**
  (created by WS-E.1.7, no owner). An agent reading its own journal can no longer
  reconstruct why one `focus` landed and an identical one did not without
  correlating the core's attention entries, which it cannot see. The refusal used
  to mean one thing. Published in `docs/book/src/limits.md`.

- **A principal cannot draw, and cannot receive physical input** (pre-existing,
  *surfaced and priced* by WS-E.1.5/#211, no owner). Neither is new and neither
  was written down as a user-facing limit until a switcher had to be built
  against them. `vitrin_view` is capture-only and there is no principal-facing
  surface interface in the IDL, so no client can put a pixel on the output —
  which is why the shipped switcher is a line-oriented host-side program and
  not a placeholder for a graphical one. There is no `observe_input` verb and
  none is designed, so no client has a hotkey; the core owns two physical
  chords and owns both because they must not depend on a client. The
  consequence for a daily driver is blunt: **every layout change starts as a
  line typed into a terminal that must be somewhere the human can reach.** The
  eventual shape #211's decision 2 names — the shell running *as a realm*,
  drawing through its own shim — needs no new protocol and does need that
  realm to reach the core socket, which is a confinement question nobody has
  answered. Published in `docs/book/src/limits.md`.

- **The shell holds `layout.arrange` for the whole output** (created by
  WS-E.1.5, and designed rather than accidental — D-018(4)). Arrangement is
  single-holder per output, checked at admission, so while the switcher lives a
  second tool that wants to arrange anything resolves `layout_held` before it
  reaches a prompt. The shipped shell therefore petitions arrangement over
  exactly one realm and names that realm on every `fullscreen`. It is the
  correct behaviour and it is also a restriction people will hit before they
  understand why.

- **A locked screen an agent can still watch** (created by WS-E.2.2/#214,
  and **decided rather than deferred** — [D-025](20-decision-log.md#d-025--a-locked-screen-does-not-suspend-agent-observation-the-gap-is-published-not-papered-over)).
  The lock screen consumes every physical event and covers the output, and it
  does not touch a grant: an `observe` holder keeps capturing the realm across
  a lock and an `actuate_*` holder keeps acting. Correct against the IDL
  (observation is concurrent by design), argued, taken to the maintainer in
  plain terms on 2026-08-08, and genuinely surprising to a human — which is
  why it is on the lock card itself as well as in
  `docs/book/src/limits.md`, rather than in a code comment. The instrument for
  "stop everything" remains the dead-man chord, which fires while locked.
- **In nested mode the lock screen locks a window** (created by WS-E.2.2, and
  a Stage-3 item by construction). `vitrind` is a client of the host
  compositor; the host owns the real session and anyone can alt-tab away.
  Stages 1–2 therefore ship a privacy cover, not an authentication boundary
  for the seat. Published.
- **No protection against VT switching, and the fix is a worse trade**
  (created by WS-E.2.2, **decided by WS-E.3.3 / D-030**). On bare DRM
  `Ctrl-Alt-F<n>` walks past the lock unless the core inhibits it, and
  inhibiting it means a session a human cannot leave when the compositor
  wedges. §7's safety rule and this item pointed at the same Stage-3 decision,
  and D-030 took it: **no inhibition, and the trusted band is scoped to the
  screen this core is driving instead** — plus the answer to the question this
  bullet did not ask, which is that a switch away does *not* raise the lock
  (it would claim a protection the core does not have and charge the human a
  passphrase for using the escape hatch). Published, in the human's words, in
  `docs/book/src/limits.md`.
- **A passphrase is nested-only, because a headless backend has no keyboard** (created
  by WS-E.2.2, closed only by Stage 3 answering the keymap question). `--lock-passphrase-file`
  is refused at startup with `--headless`, naming the reason. Without it the
  lock is an unauthenticated privacy screen and the card says so. Growing an
  xkbcommon keymap was refused here rather than deferred quietly:
  `input/mod.rs:106-109` records that a real keymap moves key pairing from the
  keysym to the scancode — a router invariant the dead-man switch depends on —
  and that is Stage 3's decision, which a lock-screen issue must not pre-empt.
- **A KDF is now in the TCB, and it processes operator-supplied input**
  (created by WS-E.2.2, no owner). Four crates (`argon2`, `base64ct`,
  `blake2`, `subtle`), measured rather than estimated, inside the most
  privileged component. Unlike fontdue — whose justification turns on "the
  only bytes it parses are a compile-time constant" — this one really does
  process bytes an operator supplied, though never bytes that arrived over the
  wire. Issue #201 records that `deny.toml` and the `cargo-deny` job still do
  not exist, so `crates/vitrin-core/Cargo.toml`'s comment is the only place
  this budget is checked.
- **A fourth gate in the input stack, and a fourth chord taken from every
  app** (created by WS-E.2.2). `deadman.rs` spends its module docs proving no
  gate bug can stop the off-switch; every gate added is a new chance for that
  proof to stop being true. The compensating controls are structural rather
  than documentary — the lock's policy implements `ConsumingGate`, which has
  no observation method, so the observe tap is forwarded by code in
  `crate::input` that has no notion a lock exists — plus an adversarial test
  through the real stack. What it also costs: `--dead-man-chord delete` is now
  refused on an otherwise default command line, because the default lock chord
  is `ctrl+alt+delete`.
- **New always-resident core state on the frame path** (created by WS-E.2.2).
  A second `ConsentSurface`-shaped cost: one more `Option<LockContent>`, one
  more cached raster and one more generation counter per backend, and a raised
  lock forces the CPU compositing path exactly as a consent card does — which
  on the WS-E laptop means the zero-copy dmabuf branch is off for as long as
  the screen is locked.
- **The top strip has now been designed as a whole** (created by WS-E.1.7,
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
- **The status strip is opt-in, and the realm view is still NOT inset**
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
- **A recurring filesystem read inside the TCB** (created by WS-E.2.3). Before
  it, the core read `realm.toml` and `principals.toml` once at startup and
  embedded its font at compile time; `crates/vitrin-core/src/status/battery.rs`
  now reads `/sys/class/power_supply` on a 30 s cadence. Bounded to one fixed
  root, at most 16 directory entries, and at most 16/32/32 bytes per attribute
  through `Read::take` — with every failure collapsing to an empty slot rather
  than a guess. **When E2.6/E2.7 put Landlock over the core's own process this
  becomes a rule the core must grant itself**, which is a real widening of that
  future sandbox and is recorded here so the ruleset's author does not have to
  rediscover it.

- **A file on disk that is a picture of the human's screen** (created by
  WS-E.2.4/#216, no owner until E2.6/E2.7 confine the core). The core already
  writes the recorder's log and the `--capture-dump` diagnostic, so this is not
  its first descriptor; it is the first whose *contents* are the screen. Every
  screenshot is readable by every app in every realm, because everything runs
  as one uid (D9). The mode is `600`, which is about other users and not about
  the confined app. The directory is audited at startup and then **held open**
  for the process's life, so the path is resolved exactly once, before any
  client exists; until the core is confined, that audit is the whole of the
  enforcement of the operator's choice.
- **A vitrin screenshot cannot show a consent prompt, and that is the design**
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
- **A fifth core-owned chord, and the first one that pays a cost back**
  (created by WS-E.2.4). Ctrl-PrintScreen is consumed in every realm. It is a
  *chord* rather than #216's proposed bare `Print` for a structural reason —
  `crate::chord::ModChord` refuses a modifier-less chord, so a bare-key gesture
  would have meant a second matcher in the stack the off-switch lives in, which
  is the thing [D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing) exists to forbid — and the
  side effect is that **bare PrintScreen is still delivered**, so an app that
  binds it keeps it. That is the first time this workstream has taken a gesture
  without taking the key. The collision check `main.rs` runs is now over five
  chords rather than four.
- **A PNG encoder is now TCB code** (created by WS-E.2.4). `crates/vitrin-png`
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
