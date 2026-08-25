# Phase 3 — Network + X11 + fleet

**Phase goal:** the fleet-and-remote phase — v1's headline deployment ([PRD](../PRD.md) user stories 1, 4, 5, 7, 8): authenticated remote sessions, legacy X11 apps in realms, N-realm headless boxes, replayable journals, wallet v0, and the first unprivileged supervision shell.

**Consumes:** Phase-2 exits (semantic chain, powerbox, network authority) plus Phase-1 artifacts A4 (flight recorder → signed journal) and A6 (headless mode → fleet). **Phase 2's exits are named here as prose and never by artifact id; see "Dependencies restated against named Phase-2 artifacts" below, which restates this sentence rather than replacing it, and A6's own restatement in [01-phase-1-mvp.md](01-phase-1-mvp.md) §1.**

**Phase exit = milestone M3** ([00-roadmap.md](00-roadmap.md)): 50-realm headless box, remote QUIC principal, X11 app in a realm, journal replay, wallet v0, mission-control shell v0. **M3 keeps its number** — D-047 decision 3 splits M2 into M2a/M2b precisely so this gate does not move. **The 50-realm half is restated in "E3.3, realigned" below against the cap the core actually enforces.**

**Internal dependency structure:** E3.1, E3.2, and E3.6 can start independently; E3.3 needs E3.1 + E3.2 late; E3.7 has the longest external-standards lead time (its tracking starts during Phase 2 via the WS-A liaison table); E3.8 consumes everything — it is the phase's integrating demo. **This sentence is about ordering *inside* Phase 3 and is unchanged; what it does not say is what each epic needs from *outside* it — see the section immediately below.**

---

## Dependencies restated against named Phase-2 artifacts

**REALIGNED 2026-08-25 BY D-047.** This document names **A1** and **A3**–**A6**
by id and cites both backward requirements **B1** and **B2** — and it names
**no C-artifact at all**. Phase 2's exports are referred to only as prose
("semantic chain, powerbox, network authority") and as epic numbers, so no epic
below states which *contract* it consumes. That reads like looseness and is
worse than that: a dependency named as prose cannot go stale, and it also cannot
be checked. Every per-epic **Dependencies** line below is left standing; this
table is the standing statement of what each epic consumes, and it is read
against **C1**–**C8** in
[02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) §1.

| Epic | The Phase-2 artifact it actually needs | Produced by | Decomposed into task issues? |
|---|---|---|---|
| **E3.1** | **C1** — the tree wire format *is* the "tree diffs are the payload that makes remote sessions cheap" this epic's own dependency line names; **C3** — a remote actuation carries `expected_epoch` or it is a race with a network in it | C1 ← E2.1 [#175](https://github.com/vitrin-os/vitrin-os/issues/175); C3 ← E2.2 [#176](https://github.com/vitrin-os/vitrin-os/issues/176) + E2.3 [#177](https://github.com/vitrin-os/vitrin-os/issues/177) | **No — all three epics have zero task issues.** |
| **E3.2** | **C1** + **C2** — AT-SPI2 is the semantic source for X apps and C2's store is where a tree lands; **C7** — the "documented XWayland-IME fallback" this epic depends on is C7's own exit criterion | #175, #176, E2.8 [#182](https://github.com/vitrin-os/vitrin-os/issues/182) | **No — zero task issues in all three.** |
| **E3.3** | **C4** — the confined spawn; C4's own stated limitation makes the per-UID provisioning packaging **E3.3's to own or to ship without**; **C5** for a fleet realm's network authority | E2.6 [#180](https://github.com/vitrin-os/vitrin-os/issues/180), E2.7 [#181](https://github.com/vitrin-os/vitrin-os/issues/181) | **Yes** — E2.6 (#185–#194) and E2.7 (#195–#200) are the only decomposed Phase-2 epics; the exceptions are the two rows added to E2.6 on 2026-08-25, P2.6.11 and P2.6.12, which have none. Only P2.6.12 is D-047's; P2.6.11's own row records it as an in-place correction and **not one of D-047's enumerated changes**. |
| **E3.4** | **A4** + **B1**, unchanged; plus **C1** — after Phase 2 what was observed includes a tree — and **C3**, because the epoch is the replay's clock | #175, #176, #177 | **No.** |
| **E3.5** | **C5** — the powerbox designation path this epic's drag-and-drop shares, and the fd/socket delivery — plus **A3** and D-024's already-shipped clipboard subset | #180, #181 | **Yes** — both producers are decomposed (#185–#194, #195–#200); the C5 tasks this row needs, P2.6.5–P2.6.7 (#189–#191) and P2.7.2–P2.7.4 (#196–#198), all exist and are all open. |
| **E3.6** | **C5** — "the fds exist; this adds the path view" is C5's delivery path by name — and **C4**, because the mount location is inside the realm's namespaces | #180, #181 | **Yes** — both producers are decomposed (#185–#194, #195–#200); C4's P2.6.1–P2.6.4 (#185–#188) are closed and C5's #189–#191 and #196–#198 open, but P2.6.11 — the shim-side Landlock stack added to E2.6 on 2026-08-25, as an in-place correction rather than one of D-047's enumerated changes — has no issue. |
| **E3.7** | **A3**, **B2**, and **C5** — durable designation grants multiply exactly the unrecallable-fd residue C5 exports — plus **C8**, since `node:` granularity is what a durable grant gets scoped to | #180, #181; C8 ← E2.2 #176 | Partly — **C8's producer has zero task issues.** |
| **E3.8** | **C2** and **C6** — the "E2.5 native semantic path" this epic's design decision names *is* C6 — plus **C3** | #175, #176, E2.5 [#179](https://github.com/vitrin-os/vitrin-os/issues/179) | **No.** |

**Phase 3 is further away than the ladder's single arrow suggests, and the
count is why.** Four of the eight epics above — **E3.1, E3.2, E3.4 and E3.8** —
depend entirely on contracts (C1, C2, C3, C6, C7) produced by epics that have
**no task issues at all**: #175, #176, #177, #179 and #182. D-047 decision 2
cuts E2.1's eleven now and re-cuts each remaining epic as it starts, so the rest
is a debt this tree names rather than pays; until then those five contracts name
producing tasks with neither an issue nor a line of code. **C5 is the trap in
the other direction:** D-047 splits it into landed *vocabulary* — the verb bits
and the `vitrin_powerbox`/`vitrin_egress` interfaces, which are on the wire —
and unbuilt *mediation*, which is the picker, the fd minting, the shim relay and
the egress proxy that E3.5, E3.6 and E3.7 would otherwise read as delivered.
And D-047 decision 3 splits M2 into **M2a** (the non-ambient realm: the
ransomware and `ssh localhost` demos) and **M2b** (the semantic realm: Firefox
by semantic tree under epoch/CAS, the first benchmark numbers, the core spec
1.0-candidate). That split is the honest statement of the gap here — **every
"No" row above waits on M2b, and M2b is the half with no issues.** M3 and M4
keep their numbers, which is why the split is a/b and not a renumbering.

---

## E3.1 — QUIC network sessions

- **Goal:** authenticated, multiplexed, capability-scoped remote sessions; workload identity (SVID/OIDC) bound to the TLS channel; capability handles established over the authenticated session; reconnection renegotiates dynamic state rather than losing it (PRD P9, Doc 2 §10).
- **Dependencies:** A1 protocol (the network profile of the same object model — the spec must state which semantics are transport-invariant → WS-A); E2.2 (tree diffs are the payload that makes remote sessions cheap).
- **Design decisions:** quinn-based transport (D-004); control-plane serialization — Cap'n Proto optional per D-003, decided here; **Q6** buffer codec for non-dmabuf sinks (evaluated during Phase 2, decided at epic start; the Arcan-a12 lesson "pixels as a last resort" is the prior art); connection-migration semantics for roaming agents; sender-constraint mapping for a channel that isn't a Unix socket (TLS channel binding replaces `SO_PEERCRED`).
- **Exit criteria:** PRD user story 5 — an agent on machine A drives a realm on machine B: identity bound at the transport, epoch/CAS intact end to end, server-side motion synthesis masking latency (measured); reconnect resumes without grant re-consent (restore-token path).

## E3.2 — X11 shim + embedded WM

- **Goal:** a per-app rootless X server with a minimal window manager *inside* the shim; X legacy fully outside the core (PRD P3, Doc 2 §4.3).
- **Dependencies:** A5 shim architecture as the template; E2.1 bridge (AT-SPI2 is the semantic source for X apps); E2.8's documented XWayland-IME fallback. **See "E3.2, realigned" below, which adds the six requirements WS-E.4.1 handed this epic and restates these rather than replacing them.**
- **Design decisions:** Xwayland-derived fork vs. driving stock Xwayland rootless with a companion WM process inside the same sandbox — **recommend the latter first** (less fork maintenance); a published, closed-ended minimal `_NET_WM_*` coverage list; gamescope's per-game-Xwayland pattern as prior art.
- **Exit criteria:** a legacy X app runs in a realm with correct map/focus semantics; **the anti-keylog test:** two X apps in two realms provably cannot observe each other's windows or input — the shared-XWayland hole (PRD Doc 2 §4.1) closed and demonstrated.

### E3.2, realigned

**REALIGNED 2026-08-25 BY D-047.** The three bullets above stand and none of
their design decisions is re-opened here. What they lack is the input WS-E.4.1
([#221](https://github.com/vitrin-os/vitrin-os/issues/221), closed) formally
handed this epic on **2026-08-10** — and **D-040** records that X11's deferral
*"**Reopens on:** E3.2 being scheduled, on its own criteria, in its own plan
document."* **This document is that instrument**, and until this block it
carried none of the list.

**The six requirements are imported by reference and are deliberately not
copied.** They are in [14-workstream-session-mode.md](14-workstream-session-mode.md)
§4.2, *"X11: what a daily driver needs from E3.2"* — six numbered items, each
naming the application that demands it, closed against one machine on one day,
with the method and its limits stated in the same section. D-040 refuses to copy
them and says why: *"A decision log that copies a scoped list acquires a second
copy that can drift from the first …"* That reasoning binds a plan document at
least as hard. **The list widens by executing #221's runbook and landing the
run, never by anyone adding a seventh line to it here.** §4.2 also carries the
X11-only software measured on the maintainer's machine, the classes that must
**not** be counted as X11 gaps (the waybar/rofi class is Stage 2's layer-shell
result), and the interim the owner accepts — a second session for X11-only
software — which is a workaround and not a mitigation.

**Two of the six are constraints this epic cannot discover from its own text,
and §4.2 says in terms that they belong to *this* core rather than to E3.2:**

1. **The scene holds at most one client surface** (§4.2 item 4). Whether an X
   client behind a rootless server arrives as one surface is E3.2's question;
   the one-surface model is not. It is `Scene`'s single `Option<SurfaceContent>`,
   and **D-040** makes growing it **one** change that layer-shell and tiling
   also wait on — tracked as
   [#307](https://github.com/vitrin-os/vitrin-os/issues/307) and **uncosted**: no
   estimate, no design sketch and no measurement behind it. E3.2 either fits one
   surface per X app or it pays for that change.
2. **The core already eats Shift-Insert in every realm, unconditionally**
   (§4.2 item 3). It is the core's clipboard *offer* chord
   (`crates/vitrin-core/src/clipboard.rs`, **D-024**), and the module records
   that the pair was chosen partly *because* Shift-Insert is the historical X11
   clipboard chord and `KEY_INSERT` is keymap-invariant where letters are not.
   **So an X client's own primary-paste gesture is gone before the X path
   exists.** That is a dependency on **E3.5/D-024**, not on anything inside
   E3.2, and it is recorded as one in E3.5's bullets below. E3.2 must not
   re-decide it.

**And one of the six is a warning about a property this epic does not have.**
§4.2 item 6: a realm's app is spawned from an environment built from nothing
(`spawn.rs`'s `env_clear` plus an allow-list), and `DISPLAY` and `XAUTHORITY`
are refused outright by `realm::RESERVED_ENV` — which is *why* the recorded
failure is `Can't open display` rather than a protocol error. **The environment
scrub is not the security property.** `spawn.rs` says in as many words that an
app which ignores what it was handed *"and connects directly to a path it
already knows is not stopped by anything in this file"*, and §4.2 records the
host's own `/tmp/.X11-unix/X0` as present and world-connectable when the
measurement was taken. An X path that hands realms a real X server must not be
built such that the scrub is what isolates — the anti-keylog exit criterion
above is the test, and a scrub cannot pass it.

**Gating:** per the table above, E3.2's Phase-2 dependencies are **C1**, **C2**
and **C7**, produced by #175, #176 and #182 — three epics with zero task issues.

## E3.3 — Headless multi-realm fleet

- **Goal:** N-realm headless operation (PRD user story 1's 50-realm box): virtual output + framebuffer per realm, resource accounting, and an SSH front-end terminating certificate principals into scoped protocol sessions — never a PTY with ambient authority (PRD Doc 2 §5.1).
- **Dependencies:** A6 headless mode; E3.1 (remote principals are how fleets are actually used); E2.6/E2.7 (realm density presumes cheap hardened namespaces). **See "E3.3, realigned" below — the cap, and the hard predecessor this line does not name.**
- **Design decisions:** SSH front-end shape — OpenSSH `ForceCommand` bridge first (smaller attack surface, faster to ship), a russh-based front-end later if needed; realm lifecycle API (create/suspend/destroy — the seed of a fleet control plane, but no hosted control plane per PRD §10); per-realm resource ceilings (cgroups) and a stated memory budget for the density target.
- **Exit criteria:** 50 realms on one reference box with measured per-realm memory/CPU overhead published (feeds the PRD §7 benchmark metric); PRD user story 8 — an SSH certificate principal gets exactly its granted realms, and `ssh localhost` inside any realm is inert; realm-7-cannot-see-realm-8 isolation re-verified at density. **Restated in "E3.3, realigned" below, which is the standing exit evidence for this epic and restates this rather than replacing it.**

### E3.3, realigned

**REALIGNED 2026-08-25 BY D-047.** The goal and exit criteria above plan a
**50-realm** headless box. **The shipped core serves 16**, and it enforces that
rather than documenting it: `MAX_REALMS = 16` in
`crates/vitrin-core/src/realm.rs`, checked in two places — a `realm.toml`
carrying more `[[realm]]` tables than that is refused at load, and
`vitrin_launcher.launch` is refused `capacity` once
`RealmRegistry::capacity_used()` has reached the number.

**The cap is not a constant somebody forgot to raise.** It is derived from a
per-realm memory accounting — `crates/vitrin-realm-init/src/lib.rs` records that
a fully populated session can pin **2.1 GiB** on that basis — and it is cited
out of code onto published surfaces: `crates/xtask/src/limits.rs` pins both the
declaration and the launcher's capacity comparison as needles, and
`examples/realm.toml` restates the number in operator-facing prose. **Raising it
is E3.3's work**, and the work is re-deriving the accounting, regenerating every
surface that carries the number, and re-measuring the density claim — not
editing a `usize`.

**Hard predecessor:
[#234](https://github.com/vitrin-os/vitrin-os/issues/234) — a launched realm has
no reclamation path.** Revocation, disconnect and the dead-man switch all leave
a launched realm running. **A fleet whose realms cannot be reclaimed reaches no
density number at all:** the box fills once and stays full, and "50 realms with
measured per-realm overhead" is not merely harder to hit, it is unmeasurable
while every launch is permanent. #234 is a predecessor of E3.3's exit criteria,
not an item inside them.

**The exit evidence and the metric row, restated.** 50 realms stays the target
and is **not** evidence M3 can cite while the cap is 16 and #234 is open. In
order, what E3.3 owes:

1. **#234 closed** — a launched realm is reclaimable, and revocation, disconnect
   and the dead-man switch each say what they do to one.
2. **`MAX_REALMS` raised**, its memory accounting re-derived, and every citing
   surface (`limits.rs`'s needles, the limits page, `examples/realm.toml`)
   regenerated rather than hand-edited.
3. **Then** the density run, whose published per-realm memory/CPU overhead is
   what feeds the PRD §7 benchmark metric. **A metric row measured below the cap
   is a measurement of the cap, not of the system**, so a run at 16 is a
   component result and must be labelled one.

The isolation half of the criteria — `realm-7-cannot-see-realm-8` re-verified at
density, and `ssh localhost` inert inside any realm — is unchanged, and is
downstream of **C4**, whose own exported limitation already makes the per-UID
provisioning packaging E3.3's to own or to ship without.

## E3.4 — Journal replay + training export

- **Goal:** the signed P6 journal proper, deterministic replay of what each principal observed and did (frame by frame), and trajectory export as agent training data (PRD user story 4).
- **Dependencies:** A4 — Phase 1's flight recorder with backward requirement B1 honored (observation digests + epoch-ready fields from day one; replay is unbuildable retroactively otherwise).
- **Design decisions:** replay fidelity — journal-of-record replay (replay what was *observed*; recommended for v1) vs. bit-exact re-composition (re-executing apps; rejected for v1); export schema aligned with what agent-training pipelines actually consume (OSWorld-style trajectory format as the reference); signature-chain verification tooling.
- **Exit criteria:** post-incident replay of a multi-principal session reconstructs the observe/act sequence with verified signatures; one session's export is consumed by an external fine-tuning/eval pipeline as a demonstration.

## E3.5 — Cross-realm mediators hardened

- **Goal:** the clipboard broker (two explicit gestures, Qubes model), drag-and-drop as designation, window activation, and a portal-compat layer proxying FileChooser/ScreenCast/RemoteDesktop into grant-checked operations (PRD P8, Doc 2 §11).
- **Dependencies:** A3 grants/consent; E2.6 (drag-and-drop shares the powerbox designation path); A5/E3.2 (the portal backend lives in shims).
- **Design decisions:** portal-backend coverage order — FileChooser first (it *is* the powerbox gesture), ScreenCast second, RemoteDesktop last; **Q5** resolved empirically: a compatibility matrix over a named top-N list of portal-using apps.
- **Exit criteria:** copy in realm A → paste in realm B requires two explicit user actions and is journaled; the portal-compat matrix is published with pass/fail per app; no shared clipboard exists anywhere (verified, not asserted).
- **A narrower clipboard already shipped, and it discharges none of the above.** WS-E.2.1 (#213, **[D-024](20-decision-log.md#d-024--the-cross-realm-clipboard-is-a-core-held-single-slot-pulled-by-the-core-on-two-human-gestures-that-delegate-nothing)**) built the *subset* this epic hardens: a core-held single slot for one **nested single-output** session, `text/plain;charset=utf-8` only at 60 KiB, filled and offered by two physical human chords, with **no agent-facing verb**, no drag-and-drop, no window activation and no portal. The first criterion above therefore *looks* discharged and is not — it is a fleet-scale claim over network sessions and the headless deployment [PRD](../PRD.md) §81 promises "no shared seat, no shared clipboard" for — and the third is a verification obligation over *every* deployment, which #213 made harder by adding a clipboard to one of them. What E3.5 inherits from D-024 and must not silently re-decide: the pull-only wire shape (`request_selection`/`selection`/`offer_selection`, and the absence of a `selection_changed` event), the MIME allow-list, and the byte cap — which lives in an immutable `(max N bytes)` token, so raising it is a new message rather than an edit.
- **One more thing E3.5 inherits from D-024: the offer chord *is* the X11 primary-paste gesture, and the core eats it in every realm.** Added 2026-08-25 as an in-place correction and **not one of D-047's enumerated changes** — it stands on the shipped code cited next rather than on a decision entry. `crates/vitrin-core/src/clipboard.rs`'s `DEFAULT_TRIGGER` is `insert`: `ctrl+shift+insert` promotes into the core's slot, `shift+insert` offers it to the focused realm. The module states the reasoning — `KEY_INSERT` is keymap-invariant where letters are not, so it is the only shape of the Qubes gesture that survives the keymap-less bare-metal backend, *"and Shift-Insert is the historical X11 clipboard chord, so the pair is familiar rather than invented"*. **WS-E.4.1 §4.2 item 3 hands the consequence to E3.2**: an X client in a realm has no route to the one cross-realm channel this project built, and its own paste gesture is consumed before the X path exists. **The decision is D-024's and the resolution is this epic's**, not E3.2's — either the chord moves, or an X selection is joined to the core-held slot, or the collision is published as a limit. E3.2 must not re-decide it, and this epic must not leave it to E3.2 by silence.

## E3.6 — FUSE synthetic paths

- **Goal:** path-expecting legacy apps see granted files at per-realm synthetic paths — the xdg-document-portal pattern, warts inherited knowingly (PRD P12, Doc 2 §12).
- **Dependencies:** E2.6 powerbox (the fds exist; this adds the path view); Rust `fuser` (PRD §17).
- **Design decisions:** **Q10** resolved empirically — atomic-save (rename-over), hardlink, and mmap patterns tested against a named list of most-used desktop apps; per-pattern wart strategy (emulate in FUSE vs. fall back to a per-app subtree standing grant); mount location inside the realm's namespaces.
- **Exit criteria:** the compatibility matrix is published (the PRD promises it publicly — §9 mitigation); a documented fallback path exists for each failing pattern; at least one stubborn real app (e.g. an office suite) saves successfully.

## E3.7 — Wallet v0 + provenance

- **Goal:** the out-of-core hardened wallet service — TPM2/FIDO2 key custody, sandboxed SD-JWT VC/mdoc parsing, presentation-as-grant, physically-originated-input consent — plus the provenance verifier (Sigstore-style identity-bound signing + transparency-log inclusion proofs, D-009) gating the durable consent-ladder rungs (PRD P11, P14, Doc 2 §13). One trust engine, three subjects.
- **Dependencies:** A3 grant table (presentation rows, `provenance_ref`); backward requirement B2 — the physical-vs-emulated input distinction preserved end-to-end since Phase 1; E2.6's deliberately blocked durable rungs unblock here.
- **Design decisions:** library choices (tss-esapi, ctap2, a sigstore-rs-class verifier); **Q14** trust-root governance — decided *before* durable rungs ship (recommended: per-deployment-configurable roots defaulting to federation with Sigstore public-good instances; a project-run log deferred for its governance cost — cross-reference [12-workstream-community.md](12-workstream-community.md) §5); **Q9** standing-grant ergonomics, full answer (grant templates + rate-audited defaults); **Q13** consent-prompt human factors — a prompt-design review is an explicit deliverable, not an afterthought; OID4VC/OID4VP revision pin (moving target per PRD Caveats — pin and track via WS-A).
- **Exit criteria:** PRD user story 7 end to end (presentation consented with physical input; the app receives a derived token; the credential never leaves the wallet; everything journaled); **the impersonation test** (PRD §15 row 7): a re-signed binary and a lookalike publisher both fail to inherit or obtain durable grants; injected input provably cannot approve a presentation.

## E3.8 — Mission-control shell v0

- **Goal:** the unprivileged supervision surface: realm grid, per-principal tinted cursors surfaced, grant/revocation panel (connected-apps view with last-used timestamps), live journal view (PRD P10; §5.2 requires the minimal version, §5.3 claims the full one).
- **Dependencies:** everything above — this is the phase's integrating demo. Standing invariant: **zero lines added to the TCB** (PRD §5.1 first invariant) — the epic's permanent acceptance test. **See "E3.8, realigned" below — a shell already exists and its architecture was decided elsewhere; these bullets are restated there rather than replaced.**
- **Design decisions:** the shell's own toolkit — eat the dogfood: build on the E2.5 native semantic path, making the shell the second native semantic client; the split of supervision affordances — consent prompts and trusted labels stay core-drawn (Qubes/Nitpicker), grids and panels are shell-drawn.
- **Exit criteria:** one human supervises N agent realms — watches live journals, revokes a grant from the panel with immediate transitive effect, hold-Esc dead-man verified from the shell context; a shell crash leaves the core and all realms unaffected (crash-only test).

### E3.8, realigned

**REALIGNED 2026-08-25 BY D-047.** The bullets above stand. Three of them
describe a shell this epic no longer gets to design from a blank page.

**A shell already exists, and it is not built on E2.5.**
`examples/shell/run_shell.py` — WS-E.1.5,
[#211](https://github.com/vitrin-os/vitrin-os/issues/211), closed **2026-08-08**
— is a line-oriented switcher and launcher that holds `layout.focus`,
`layout.arrange` and `realm.launch` through the same petition-and-consent path
every other client uses, **on the plain Python SDK and on no semantic path at
all**. It draws nothing and has no hotkey, and its own docstring states both as
authority arguments rather than missing polish: **a principal cannot draw**
(`vitrin_view` is capture-only and there is no principal-facing surface
interface anywhere in `protocol/vitrin-v0.xml`) and **a principal cannot receive
physical input** (there is no `observe_input` verb, and **D-039** decides that
none is designed). So this epic's design decision — *"build on the E2.5 native
semantic path, making the shell the second native semantic client"* — is a
choice about a **second** shell, and it must say what becomes of the first.

**The architecture was decided by D-038 and D-046, not by E3.8.** Both are the
owner's decisions, accepted and unbuilt:

- **D-038 (2026-08-17): the shell is a realm that draws.**
  One realm holds both a shim surface — so it draws like any other app — and a
  principal connection, so it holds the layout verbs through the ordinary grant
  path. The core stops drawing the status strip and those rows move **below the
  trust line** to a client. **Its security content is stated before its
  benefits and this epic inherits it unchanged: the core cannot verify the
  pairing.** *"The thing drawing the bar is the thing holding arrange"* is a
  deployment fact, not an invariant, and D-038 deliberately refuses to mint the
  realm↔principal binding that would make it one — choosing that rule is
  window-management policy the core does not have (PRD §5.1). What the human
  gets instead is the ordinary consent card, naming a principal and a realm.
- **D-046 (2026-08-24): that realm reaches the core socket
  through a descriptor the core mints and passes down the spawn path it already
  has** — no new protocol, no filesystem object, no third spawn path —
  authorised by an operator's declaration in `realm.toml` **and** a human's
  `while_running` consent, with what the connection may carry fenced
  structurally rather than by consent. #311's measurement is why a descriptor
  and not a path: of 14 computed routes to `core.sock` from inside a confined
  realm, both confined arms reached **0**.

**An explicit and uncosted predecessor: the N-surface scene.** **D-040** records
that layer-shell and tiling are **one** deferral because both need `Scene` to
hold more than one client surface, and that **nobody has costed that change** —
no estimate, no design sketch, no issue and no measurement *behind the claim that it
is one change rather than two*, only the reading of `scene/mod.rs`,
`scene/layout.rs` and the IDL recorded there. Read that list at its own scope: the
deferral does have a tracking issue —
[#307](https://github.com/vitrin-os/vitrin-os/issues/307), named in D-040's own
status line, still open, and carried by
[14-workstream-session-mode.md](14-workstream-session-mode.md) §4.5 — and an issue
is not a costing. A shell realm drawing the status
rows while an app realm draws below it **is** that scene. D-040 also names what
the change collides with: `scene/layout.rs`'s *"it must never grow"* comment,
which is the artifact that has kept window-management policy out of the core
through two pressures already. Note the boundary carefully, because it is easy
to blur: **a tiler inside `vitrind` is refused permanently on PRD §5.1; a tiler
outside it is what is deferred.** Treat the N-surface scene as a predecessor of
E3.8 with no number attached to it.

**Whether [#304](https://github.com/vitrin-os/vitrin-os/issues/304) is WS-E
residue or E3.8's first task is not answered here.** #304 is open and is the
issue D-038 was taken on. D-047 decision 1 gives WS-E a **Stage 5** owning
D-038, D-039, D-040 and D-046; which side of the WS-E/E3.8 line #304 falls on is
**Stage 5's** to decide, and this document deliberately does not decide it.

**One bullet above survives intact and should be read harder for it.** *"Zero
lines added to the TCB"* is this epic's permanent acceptance test, and
D-038's shell-as-a-realm is the shape that keeps it true: the alternative —
giving principals a surface interface — would make every connected client a
potential painter of the human's screen, which is a far larger change to the
TCB's attack surface than a shell needs.
