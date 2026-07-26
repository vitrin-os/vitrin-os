# Vitrin OS — execution plan

This tree is the planning layer over [`docs/PRD.md`](../PRD.md). The PRD is normative for **what** the project is and **why** (vision, pillars, architecture, threat model); this plan is normative for **when and in what order** (phases, epics, tasks, decision gates, workstreams). PRD pillar IDs (P1–P14) and sections are cited from here, never restated.

## Reading order

1. [`../PRD.md`](../PRD.md) — the vision and architecture (if you read one thing, read its two TL;DRs).
2. [`00-roadmap.md`](00-roadmap.md) — milestone ladder M0–M4, swimlanes, sequencing rules, metrics.
3. The phase you care about: [`01-phase-1-mvp.md`](01-phase-1-mvp.md) (deep, task-level) · [`02-phase-2-semantic-epochs.md`](02-phase-2-semantic-epochs.md) · [`03-phase-3-network-x11-fleet.md`](03-phase-3-network-x11-fleet.md) · [`04-phase-4-horizon.md`](04-phase-4-horizon.md) (gated).
4. [`20-decision-log.md`](20-decision-log.md) for anything cited as `D-00n` or `Qn`.
5. The workstreams, as needed: [`10-workstream-spec.md`](10-workstream-spec.md) · [`11-workstream-funding.md`](11-workstream-funding.md) · [`12-workstream-community.md`](12-workstream-community.md) · [`13-workstream-agent-integration.md`](13-workstream-agent-integration.md).

## File numbering

- `0x` — sequenced execution (roadmap, then phases in order).
- `1x` — parallel workstreams (no order among themselves).
- `2x` — registries (decision log).

## Conventions

- **Epic template:** Goal / Dependencies / Design decisions / Exit criteria. Phase 1 additionally decomposes epics into tasks with IDs, dependencies, and acceptance criteria.
- **Cross-reference syntax:** `P1.3.6` (Phase-1 task) · `E2.3` / `E3.7` (Phase-2/3 epics) · `A1`–`A6` (Phase-1 exported artifacts) · `B1`/`B2` (backward requirements on Phase 1) · `M0`–`M4` (roadmap milestones; `M1.x` are Phase-1-internal) · `D-00n` (project decision) · `D1`–`D11` (Phase-1-local decisions) · `R1`–`R8` (Phase-1 risks) · `Qn` (PRD §20 open question) · `WS-A/B/C/D` (workstreams spec/funding/community/agent-integration).
- **Status legend** for epics/tasks as work begins: `planned | active | done | deferred`.
- **Scope discipline** (D-007): invariants, v1 scope, horizon, and renounced items never migrate between classes without a decision-log entry; Phase 4 opens only through the M4 gate.

## Changing this plan

Plan documents are living and edited in place — except [`20-decision-log.md`](20-decision-log.md), where accepted decisions are append-only (superseded, never edited). Substantive scope changes (an epic added/dropped, a milestone redefined) get a decision-log entry.

## Non-goals of this tree

No issue-tracker duplication, no code scaffolding, no sprint-level task lists — Phase 1's task tables are the finest granularity kept in-repo. When implementation starts, tasks may be mirrored into an issue tracker; this tree remains the source of truth for structure and sequencing.
