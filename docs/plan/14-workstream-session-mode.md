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
| **4 — long tail** | X11 (defers to E3.2) · seat vocabulary for touch/gestures/lid · session lifecycle · the honesty sweep | open |

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
- **No X11**, so no Steam and no legacy application.
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
  and **decided rather than deferred** — [D-025](20-decision-log.md#d-025)).
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
  is the thing [D-024](20-decision-log.md#d-024) exists to forbid — and the
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

**The escape route is VT switching and an installer USB, not SSH.** This rule
originally required an SSH session from a second machine;
[D-028](20-decision-log.md#d-028--the-drm-bring-up-escape-route-is-vt-switching-and-an-installer-usb-not-ssh)
(2026-08-09) records the maintainer's decision not to run an sshd, the route
that replaces it, and its cost — **a wedged DRM master with no live console is a
reboot rather than a command.** The step-by-step version, written against this
machine's actual VTs, cards and connectors, is
[`docs/drm-bringup.md`](../drm-bringup.md) step 0. The rule above is unchanged;
only the recovery path is.

## 8. What this workstream is not

- **Not the horizon item** (§0), and not evidence toward M4.
- **Not a product.** [PRD](../PRD.md) §5.4 renounces displacing Wayland on
  today's human desktop as a project aim; nothing here changes that.
- **Not a reason to stop Phase 2.** WS-E's estimate is roughly Phase 2's
  remaining budget. D-021 records that as an unmitigated cost and a priority
  choice, not a solved problem.
