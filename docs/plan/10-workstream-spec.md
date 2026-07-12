# Workstream A — Spec & standards

The protocol outlives any implementation ([PRD](../PRD.md) §9, bus-factor mitigation). This workstream turns the PRD's Doc 2 into a published, versioned, reviewable protocol specification — on a schedule that follows running code rather than preceding it (D-014 in [20-decision-log.md](20-decision-log.md)).

## 1. Artifact split

- Spec prose: **CC-BY-4.0**; wire/IDL definitions and schemas: **Apache-2.0 with explicit patent grant** (D-005).
- Eventually a separate repo, `vitrin-os/protocol` — the licensing boundary and the "protocol outlives the implementation" argument both want a clean cut. Until the split is worth its overhead (first external reviewer or implementer, whichever comes first), the spec lives in-tree at `docs/protocol/` (created in Phase 1, task P1.9.5) with the licensing split marked per-directory.

## 2. Module-freeze ladder (the key sequencing call)

The spec is published early — PRD §8 Phase 0 requires an object-model and wire-protocol draft — but versioned `0.x` and explicitly tracking the implementation. Rationale: the anti-Arcan posture (§9: running code before prose authority) and the PRD's own caveat that epoch/CAS is "a design claim, not a proven result" — freezing it before E2.3 measures it would enshrine guesswork.

| Spec module | Version event | At milestone | Precondition |
|---|---|---|---|
| Object model, handshake, grant/consent, observe/actuate | **spec 0.1** (as designed) | M0 | this plan tree committed |
| Same, corrected against running code | **spec 0.2** | M1 | Phase-1 demo end-to-end |
| Semantic tree, diff format, epoch/CAS semantics | **core 1.0-candidate** | M2 | Q1 and Q4 closed after E2.3's empirical tuning |
| Network profile (QUIC session, transport-invariant semantics) | **network 1.0-candidate** | M3 | Q6 closed at E3.1 |
| Wallet/provenance profile | stays **0.x** until Phase 4 | M4+ | EUDI/OID4VC churn settles (PRD Caveats) |

## 3. Review process

- RFC-style: numbered change proposals as PRs against the spec, a `CHANGES` log, a stated request-for-comment window per proposal.
- Named external reviewers recruited from adjacent projects — the natural reviewers already appear in the PRD's references: AccessKit, Newton, wlroots, Smithay circles.
- Two PRD §7 spec metrics become tracked counters here: substantive external reviews logged, and independent-implementer statements of intent (targets in the [00-roadmap.md](00-roadmap.md) metrics table).

## 4. Standards-liaison table

Operationalizes Q7 and the PRD Caveats: every moving external dependency is pinned, with a named re-check cadence. Reviewed at each milestone.

| Dependency | What we pin | Why it moves | Re-check |
|---|---|---|---|
| AccessKit schema | schema version adopted at E2.1 | Newton/COSMIC evolution | each milestone; before core 1.0-candidate |
| GNOME Newton protocols | prototype status noted; no hard dependency | "not yet finalized" (PRD §1.3) | each milestone |
| IETF AIMS (`draft-klrc-aiagent-auth`) | none — behind the pluggable verifier (D-008) | Security Considerations still "TODO" | each milestone |
| MCP authorization spec | revision referenced in consent design (PRD Doc 2 §5.3) | active revision cycle | each milestone |
| OID4VC / OID4VP / eIDAS 2.0 EUDI | revision pinned at E3.7 | member-state wallet timelines, revision churn | Phase-2 onward, quarterly |
| Wayland staging protocols (`wp_security_context_v1`, `ext-transient-seat-v1`, libei/EIS) | versions consumed by shim/core | staging-protocol churn | with each wlroots/Smithay upgrade task (D11) |
