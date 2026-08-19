# Roadmap

The master sequencing view: four engineering phases and five parallel workstreams, hung on a five-rung milestone ladder. The [PRD](../PRD.md) (§8, §19) is normative for *what and why*; this document owns *when and in what order*.

**No calendar dates.** Sequencing is milestone-relative only — with a single maintainer through at least Phase 1 (risk R8), dates would be fiction. The one external cadence that matters (NLnet call cycles) is handled milestone-relatively in [11-workstream-funding.md](11-workstream-funding.md).

## 1. Milestone ladder

| Milestone | Meaning | Exit evidence |
|---|---|---|
| **M0 — Spec draft & plan published** (Phase 0 exit) | PRD + this plan tree committed; protocol object-model draft (spec 0.1) published; namespaces claimed (already done, PRD Naming) | this commit contributes; spec 0.1 per [10-workstream-spec.md](10-workstream-spec.md) |
| **M1 — MVP demo** (Phase 1 exit) | Firefox in a realm; agent observes pixels + injects scoped input under one core-rendered consent grant; nested + headless | [01-phase-1-mvp.md](01-phase-1-mvp.md) M1.5; demo runs from README on a clean machine. *Load-bearing for the NLnet application (WS-B) and the public announcement (WS-C)* |
| **M2 — Semantic realm** (Phase 2 exit) | Agent drives Firefox by semantic tree + epoch/CAS; ransomware demo (E2.6); `ssh localhost` demo (E2.7); first benchmark numbers | [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) exits; core spec 1.0-candidate |
| **M3 — Fleet** (Phase 3 exit) | 50-realm headless box; remote QUIC principal; X11 app in a realm; journal replay; wallet v0; mission-control shell v0 | [03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md) exits; network spec 1.0-candidate |
| **M4 — Horizon gate review** (not a delivery) | Adoption-metric checkpoint deciding Phase 4 entry (PRD §8: "only when Phase 1–3 adoption metrics justify the support treadmill") | gate thresholds in [04-phase-4-horizon.md](04-phase-4-horizon.md) |

## 2. Swimlanes and coupling points

```
             M0 ──────────► M1 ──────────► M2 ──────────► M3 ──────────► M4 (gate)
Engineering  plan tree      Phase 1        Phase 2        Phase 3        Phase 4
             spec draft     MVP demo       semantic+      network+X11+   (if gate
                                           powerbox       fleet+wallet   passes)
WS-A spec    spec 0.1 ───── spec 0.2 ───── core 1.0-cand ─ net 1.0-cand ─ wallet profile
WS-B funding                NLnet app ──── STF app ─────── NLnet #2 / sponsors
                            (with demo)    (a11y framing)  (fleet slice)
WS-C comm.   licensing ──── ANNOUNCE ───── 2nd beat ────── GOVERNANCE.md
             files, quiet   (demo-first)   (security demos) trigger zone
```

**WS-D and WS-E have no lane above, and that is a statement about them rather than an omission.** Both were opened after M1 closed — [13-workstream-agent-integration.md](13-workstream-agent-integration.md) on 2026-07-26, [14-workstream-session-mode.md](14-workstream-session-mode.md) by [D-021](20-decision-log.md#d-021--session-mode-is-scheduled-as-a-maintainer-dogfooding-workstream-ws-e-and-that-is-not-the-horizon-item) on 2026-08-06 — so neither has an M0→M1 history to draw, and neither is sequenced milestone-relatively here. WS-D's ordering lives in its §5, whose first item, an MCP server, is unbuilt. WS-E's lives in its §4 stage table, where Stage 4 is still estimated *open* and Stage 1's shell client is not yet struck through; D-021(2) additionally forbids reading any WS-E deliverable as evidence toward M4, so it can never acquire an M4 lane.

Coupling arrows that drive sequencing:

- **M1 demo → NLnet application** (an application with a runnable demo is categorically stronger — WS-B §1).
- **E2.2 tree schema → core spec module freeze** (the spec follows the measured implementation, D-014).
- **M2 benchmark + coverage matrix → STF application** (WS-B §2).
- **M1 → public announcement** (quiet until a runnable MVP exists — WS-C §4).
- **E3.7 provenance → durable consent-ladder rungs anywhere** (E2.6 ships them structurally blocked).

## 3. Sequencing rules

- **Phase 2 epics that may start before M1 fully closes:** E2.6 and E2.7 depend only on the realm-spawn model (A2) and consent (A3), not on semantics — they can begin as soon as those artifacts stabilize. E2.8 (IME) is parallel by design.
- **Phase 3 long-lead pre-study during Phase 2:** E3.7's standards tracking (OID4VC/eIDAS pins) and E3.1's codec evaluation (Q6) both start early via the WS-A liaison table.
- **The earliest hard decision gate was Q11** (realm vs. Unix-user boundary) at Phase 2 start — it shaped E2.6/E2.7's namespace/UID layout, and it was **closed by [D-020](20-decision-log.md#d-020--the-realm-boundary-is-a-namespace-boundary-intra-user-by-default-in-namespace-uidgid-and-a-residue-that-lives-outside-every-realm)** (2026-08-06). [D-037](20-decision-log.md#d-037--the-realms-namespaces-are-built-by-a-helper-that-execs-first-and-unshares-second-the-core-proves-the-confinement-from-outside-before-it-commits-and-the-measured-ceiling-and-the-selected-policy-are-two-vocabularies-whose-bottoms-may-never-share-a-word) executes D-020(1)(4)(6) in P2.6.2; D-020(3)'s per-UID provisioning (E3.3), D-020(5)'s host-level sidecar residue (M3) and [D-010](20-decision-log.md#d-010--per-realm-isolation-dial)'s hardened and paranoid tiers (unscheduled) remain open, and R2.9 is not retired.
- **Within Phase 1**, the only hard serialization point is the IDL freeze; see the dependency graph in [01-phase-1-mvp.md](01-phase-1-mvp.md) §4.

## 4. Open-question cross-reference (compact)

Full entries with rationale live in [20-decision-log.md](20-decision-log.md) Part B.

| Q | Blocks | Decide-by |
|---|---|---|
| Q1 epoch granularity | E2.3 | tuned before core spec 1.0-candidate (M2) |
| Q2 node addressing | E2.2 | v0 at E2.2 start |
| Q3 VLM confidence | E2.4 | field frozen at spec 1.0-candidate |
| Q4 delegation depth | spec | before spec 1.0-candidate (M2) |
| Q5 portal coverage | E3.5 | matrix at M3 |
| Q6 network codec | E3.1 | at E3.1 start (evaluated during Phase 2) |
| Q7 identity churn | — (D-008) | standing review per milestone |
| Q8 bus factor | M4 gate input | ongoing (WS-C) |
| Q9 standing-grant ergonomics | E2.6 / E3.7 | v0 at E2.6; full at E3.7 |
| Q10 atomic-save over FUSE | E3.6 | matrix at M3 |
| Q11 realm vs. Unix-user boundary | E2.6 + E2.7 | **Phase 2 start** — closed there by D-020 (2026-08-06) |
| Q12 egress ergonomics | E2.7 | v0 at E2.7; full by M3 |
| Q13 consent-ladder human factors | E2.6 / E3.7 | review at E2.6; re-review before durable rungs |
| Q14 trust-root governance | E3.7 | before any durable grant ships |

## 5. Success metrics (PRD §7) → measurable exit criteria

| PRD §7 metric | Measurable form | Attached to |
|---|---|---|
| Published, versioned protocol spec | spec 0.1 tagged | M0 (WS-A) |
| External review/commentary | ≥3 substantive external reviews logged against the spec | M2 (WS-A) |
| Independent implementer intent | ≥1 written statement of intent | M3–M4 gate (WS-A) |
| Phase-1 MVP end-to-end | demo runs on a clean machine from README instructions | M1 |
| Phase-2 semantic + epochs | E2.3 StaleEpoch harness demo + measured tree-diff sizes | M2 |
| Phase-3 network + X11 + fleet | 50-realm box + cross-machine drive + anti-keylog test, all scripted | M3 |
| Contributors beyond the author | ≥2 non-author contributors with merged non-trivial PRs | M2→M3 (WS-C) |
| Funded grant | NLnet MoU signed | post-M1 (WS-B) |
| Citations in agent-infra / capability-security discussions | tracked list; ≥5 independent mentions | M4 gate (soft) |
| Benchmark vs. screenshot baseline | OSWorld-style subset (named N tasks): ≥10× token-cost reduction, success rate ≥ parity, wall-clock improvement reported — vs. the Xvfb+screenshot+xdotool baseline (PRD §1.6) | first cut at M2 (Firefox realm); full at M3 (fleet + X11) |

Honest note: the citation and implementer-intent metrics are not fully controllable; they are M4 gate *inputs*, not epic exit criteria.
