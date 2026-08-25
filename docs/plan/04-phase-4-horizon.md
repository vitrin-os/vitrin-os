# Phase 4 — Horizon

Deliberately bullet-level. Horizon items are *claimed, not renounced* ([PRD](../PRD.md) §5.3) — but each drags a support treadmill that would consume the differentiator if attempted early, so this phase is gated, not scheduled.

## Entry gate

Phase 4 opens only on the **M4 gate review** ([00-roadmap.md](00-roadmap.md)) passing thresholds tied to the PRD §7 metrics:

- ≥1 independent implementer's written statement of intent (spec metric);
- a sustained contributor base beyond the author (≥2 regular non-author contributors — Q8);
- grant funding secured (NLnet MoU signed, [11-workstream-funding.md](11-workstream-funding.md));
- the benchmark vs. the screenshot baseline published (**M2b**/M3 exit artifact — M2 split into M2a and M2b by **D-047**(3), and the first benchmark numbers are M2b's; see [00-roadmap.md](00-roadmap.md) §1).

Per PRD §8: "entered only when Phase 1–3 adoption metrics justify the support treadmill." If the gate fails, Phase 4 items stay claimed and deferred — a deferred ambition and a renounced one are different promises (PRD §5).

## Horizon items (in intended order)

- **Toolkit semantic backends.** Flutter-embedder and Rust-toolkit (iced/egui) backends generalizing the E2.5 demo path — weeks-scale each per PRD §5.3. First in order because they compound the semantic differentiator.
- **Capability-remoting protocol hardening.** Third-party-client-grade spec + conformance suite for the E3.1 network layer — "share exactly one window, capability-scoped, audited, revocable." The *protocol* is claimed; the consumer remoting *product* remains renounced (§5.4) — third parties build "Parsec for realms."
- **EUDI / OID4VC conformance.** Formal conformance testing of the E3.7 wallet against settled eIDAS 2.0 profiles, once the standards churn (PRD Caveats) settles.
- **Session mode on bare DRM/KMS.** The same core as a daily-driver display server, every app in a shim. Architecturally emergent — no new design, only the support treadmill (hardware matrix, HDR, color management, fractional scaling, human accessibility, IME-for-everyone). Explicitly the **last** item to start; its product face is the consumer security desktop (bounded ransomware, powerbox files, wallet login) — the *point* of the horizon per §5.3, not an accident. **Two of this bullet's three claims are spent — see "Session mode on bare DRM/KMS" below, the REALIGNED block, which is the standing statement of what this tier holds and restates the bullet rather than replacing it.**
- **Capability shell (research).** A terminal whose shell passes designated fds instead of ambient authority (Capsicum/CloudABI lineage). Research track: publishable output, not a shippable component.

### Session mode on bare DRM/KMS — REALIGNED 2026-08-25 BY D-047

**This block reverses a published scope-tiering decision, so the bullet above is left standing rather than rewritten.** [D-007](20-decision-log.md) makes horizon membership something only a decision-log entry may change, and **D-021** (accepted 2026-08-06, [20-decision-log.md](20-decision-log.md)) is that entry. It names this document by name: *"D-007 says horizon items never migrate class without a decision-log entry, and `04-phase-4-horizon.md` names session mode the **last** horizon item behind the M4 gate. This entry is that instrument."* The bullet is therefore the record of what this tier held before that date; this block says what D-021(2) left in it.

**Two of the bullet's three claims are spent.**

- ***"Explicitly the last item to start"* — it started first.** [14-workstream-session-mode.md](14-workstream-session-mode.md) opened with D-021 on 2026-08-06 and its tracking epic [#206](https://github.com/vitrin-os/vitrin-os/issues/206) closed on **2026-08-13** with all nineteen task sub-issues closed. The other four items in the list above remain unstarted behind an entry gate the project has not approached.
- ***"Architecturally emergent — no new design"* — it was not.** The work produced **twelve decision-log entries under eleven numbers** (D-031 is used twice, an unresolved collision the log itself flags): **D-028**–**D-034** — the keyboard interpreted inside the core from a pre-compiled keymap file with key pairing moved to the scancode; a bare-metal backend that drives exactly one output and refuses to start otherwise, compositing the human's cursor the IDL said it never composites; what the trusted band may assert across a VT switch; the DRM bring-up escape route; `Ctrl-Alt-F<n>` implemented in the core; relative motion and pointer gestures on the seat with pointer constraints on the shim session; idle blanking that does not lock; and the lost-seat lock policy — plus **D-039**–**D-042**: hotkeys as named actions the core resolves, the N-surface scene that layer-shell and tiling both wait on, backlight actuation, and idle inhibit as a property of the focused realm. Every one is a design decision this bullet said would not be needed.

**What D-021(2) actually left in the horizon tier.** In its own words the horizon item is *"a display server other people can run"* — against WS-E's *"one maintainer's one laptop"*, a single Intel-driven eDP output at scale 1. What stays here is the treadmill D-021 deferred explicitly: the **hardware matrix**; **HDR**; **color management**; **fractional scaling** (WS-E's target panel runs at scale 1, *"which is why this is affordable"*); **human accessibility** — D-021 defers *"no accessibility of any kind"*; and **IME for every user**, WS-E carrying nothing beyond E2.8's single reference combination. **D-040** adds one more, which the original bullet did not name at all:

- **Multi-output.** **D-029** makes the bare-metal backend drive exactly one output and *refuse to start otherwise*, so this is not a gap left open by accident. D-040 parks reopening it **behind the M4 gate** rather than inside the workstream, because *"reopening it is not a scene change at all: it is an **M4 hardware-matrix commitment**"* — and its trigger is deliberately narrow: a second output on the maintainer's own desk **together with** a decision to spend budget on it, or the M4 gate being approached deliberately, never as a side effect of a shell issue. Multi-output *arrangement* is separately D-018's residual and stays in Phase 3.

**D-021(2)'s prohibition, restated so no WS-E result is ever read as M4 gate evidence.** *"No WS-E deliverable may be cited as evidence toward M4."* It stands, D-040 explicitly declines to disturb it, and it is the reason this block exists rather than a rewritten bullet: the two targets differ by an order of magnitude of effort and by their entire audience, and a dogfooding success on one panel clears none of the thresholds under "Entry gate" above. Every WS-E number was measured on hardware chosen for being easy — one machine, one GPU, one output, the discrete GPU out of the display path — and D-021 states in as many words that *"none of it generalizes, and stating a WS-E result as a portability claim would be false."*

**Unchanged by this block:** the bullet's product face. The consumer security desktop — bounded ransomware, powerbox files, wallet login — is still what this item is *for* per PRD §5.3, and nothing above narrows it.

## Non-goals (restated so horizon never silently absorbs them)

Per PRD §5.4, renounced outright: a full application toolkit; a consumer remote-desktop *product*; displacing Wayland on today's human desktop as a project aim.
