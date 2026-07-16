# Phase 4 — Horizon

Deliberately bullet-level. Horizon items are *claimed, not renounced* ([PRD](../PRD.md) §5.3) — but each drags a support treadmill that would consume the differentiator if attempted early, so this phase is gated, not scheduled.

## Entry gate

Phase 4 opens only on the **M4 gate review** ([00-roadmap.md](00-roadmap.md)) passing thresholds tied to the PRD §7 metrics:

- ≥1 independent implementer's written statement of intent (spec metric);
- a sustained contributor base beyond the author (≥2 regular non-author contributors — Q8);
- grant funding secured (NLnet MoU signed, [11-workstream-funding.md](11-workstream-funding.md));
- the benchmark vs. the screenshot baseline published (M2/M3 exit artifact).

Per PRD §8: "entered only when Phase 1–3 adoption metrics justify the support treadmill." If the gate fails, Phase 4 items stay claimed and deferred — a deferred ambition and a renounced one are different promises (PRD §5).

## Horizon items (in intended order)

- **Toolkit semantic backends.** Flutter-embedder and Rust-toolkit (iced/egui) backends generalizing the E2.5 demo path — weeks-scale each per PRD §5.3. First in order because they compound the semantic differentiator.
- **Capability-remoting protocol hardening.** Third-party-client-grade spec + conformance suite for the E3.1 network layer — "share exactly one window, capability-scoped, audited, revocable." The *protocol* is claimed; the consumer remoting *product* remains renounced (§5.4) — third parties build "Parsec for realms."
- **EUDI / OID4VC conformance.** Formal conformance testing of the E3.7 wallet against settled eIDAS 2.0 profiles, once the standards churn (PRD Caveats) settles.
- **Session mode on bare DRM/KMS.** The same core as a daily-driver display server, every app in a shim. Architecturally emergent — no new design, only the support treadmill (hardware matrix, HDR, color management, fractional scaling, human accessibility, IME-for-everyone). Explicitly the **last** item to start; its product face is the consumer security desktop (bounded ransomware, powerbox files, wallet login) — the *point* of the horizon per §5.3, not an accident.
- **Capability shell (research).** A terminal whose shell passes designated fds instead of ambient authority (Capsicum/CloudABI lineage). Research track: publishable output, not a shippable component.

## Non-goals (restated so horizon never silently absorbs them)

Per PRD §5.4, renounced outright: a full application toolkit; a consumer remote-desktop *product*; displacing Wayland on today's human desktop as a project aim.
