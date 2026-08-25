# Roadmap

The master sequencing view: four engineering phases and five parallel workstreams, hung on a six-rung milestone ladder. The [PRD](../PRD.md) (§8, §19) is normative for *what and why*; this document owns *when and in what order*.

**No calendar dates.** Sequencing is milestone-relative only — with a single maintainer through at least Phase 1 (risk R8), dates would be fiction. The one external cadence that matters (NLnet call cycles) is handled milestone-relatively in [11-workstream-funding.md](11-workstream-funding.md).

## 1. Milestone ladder

| Milestone | Meaning | Exit evidence |
|---|---|---|
| **M0 — Spec draft & plan published** (Phase 0 exit) | PRD + this plan tree committed; protocol object-model draft (spec 0.1) published; namespaces claimed (already done, PRD Naming) | this commit contributes; spec 0.1 per [10-workstream-spec.md](10-workstream-spec.md) |
| **M1 — MVP demo** (Phase 1 exit) | Firefox in a realm; agent observes pixels + injects scoped input under one core-rendered consent grant; nested + headless | [01-phase-1-mvp.md](01-phase-1-mvp.md) M1.5; demo runs from README on a clean machine. *Load-bearing for the NLnet application (WS-B) and the public announcement (WS-C)* — **"on a clean machine" is spent; see "M1's exit evidence" below, which is the standing statement of this row and restates it rather than replacing it** |
| **M2 — Semantic realm** (Phase 2 exit) — **split by D-047; this row is left standing as the record of the single gate it was, and is no longer the rung anything exits on. See the M2a and M2b rows below and "The M2 split" after this table** | Agent drives Firefox by semantic tree + epoch/CAS; ransomware demo (E2.6); `ssh localhost` demo (E2.7); first benchmark numbers | [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) exits; core spec 1.0-candidate |
| **M2a — The non-ambient realm** (a Phase-2 rung, not a phase exit) | A realm with no ambient filesystem or network authority, demonstrated: the ransomware demo (★P2.6.9, [#193](https://github.com/vitrin-os/vitrin-os/issues/193)) and the `ssh localhost` demo (★P2.7.6, [#200](https://github.com/vitrin-os/vitrin-os/issues/200)). **Ships first** | [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) M2.5's two named gates — `test_real_ransomware.py` **and** `test_real_ssh_localhost.py`, mock-free per D12. *WS-C's second announcement beat lands here, moved from M2 by D-047(3)* |
| **M2b — The semantic realm** (Phase 2 exit) | Agent drives Firefox by semantic tree under epoch/CAS; first benchmark numbers | [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) exits (M2.6); core spec 1.0-candidate |
| **M3 — Fleet** (Phase 3 exit) | 50-realm headless box; remote QUIC principal; X11 app in a realm; journal replay; wallet v0; mission-control shell v0 | [03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md) exits; network spec 1.0-candidate |
| **M4 — Horizon gate review** (not a delivery) | Adoption-metric checkpoint deciding Phase 4 entry (PRD §8: "only when Phase 1–3 adoption metrics justify the support treadmill") | gate thresholds in [04-phase-4-horizon.md](04-phase-4-horizon.md) |

#### M1's exit evidence — REALIGNED 2026-08-25 BY D-047

**Spent: *"demo runs from README on a clean machine."*** The sentence is false, and **D-047**(6) restates it rather than deleting it because it is the claim WS-B and WS-C were both keyed to.

The reason is an *admission* floor, not a degradation: `--isolation=default` refuses to start below `build.landlock_min_abi`, which is **6**. Of the five distribution kernels [`docs/book/src/isolation-kernels.md`](../book/src/isolation-kernels.md) actually booted and measured, **three are refused** — Ubuntu 22.04 LTS (`5.15.0-191-generic`, ABI 1), Debian 12 bookworm (`6.1.0-50-amd64`, ABI 2) and Ubuntu 24.04 LTS's GA kernel (`6.8.0-139-generic`, ABI 4), each reporting `below-floor(abi=N,required=6)` — and **two start**: Debian 13 trixie (`6.12.101+deb13-amd64`, ABI 6) and the azure kernel this repository's CI runners boot (`6.17.0-1020-azure`, ABI 7). All five report `ns.all=available` and `mount.in_userns=available`, so the Landlock floor is the only thing separating the two groups, and its remedy is a newer kernel rather than a sysctl, an `lsm=` edit or a `CONFIG_` change. On an AppArmor-carrying distribution there is a **second, independent** precondition: `packaging/apparmor/vitrind` has to be loaded, and [#293](https://github.com/vitrin-os/vitrin-os/issues/293) is open on exactly the fact that nothing in the tree installs it or the binaries it names.

**What replaces the claim.** M1's exit evidence is [01-phase-1-mvp.md](01-phase-1-mvp.md) M1.5 plus: *the demo runs from the README on a machine whose kernel reports Landlock ABI ≥ 6, and on an AppArmor-carrying distribution only after `packaging/apparmor/vitrind` is installed by hand.* That is a claim about a **prepared** machine rather than a clean one, and it holds on two of the five kernels anyone has measured. M1.5 itself carries two criteria still open — [#171](https://github.com/vitrin-os/vitrin-os/issues/171) and [#156](https://github.com/vitrin-os/vitrin-os/issues/156) — which **D-047**(6) requires be named as exceptions on the milestone in `01-phase-1-mvp.md` rather than left unlabelled; #171 is recorded there as **code-blocked by [#253](https://github.com/vitrin-os/vitrin-os/issues/253)**, not hardware-blocked.

**The packaging is filed, not scheduled.** #293 is open, no rung in any phase or workstream document schedules it, and **D-047**(6) leaves prioritising it explicitly open rather than deciding it. Nothing here schedules it either. This block records what the exit claim is worth today; §5 below carries the same restatement against the PRD metric it also appears as.

#### The M2 split — REALIGNED 2026-08-25 BY D-047

**Spent: M2 as one external exit gate.** It stood over two tracks in incomparable states — a semantic chain **D-047**(3) records as having zero task issues, against a confinement/powerbox track the same decision counts as **four of sixteen built** (the per-epic row counts behind that are **D-047**(2)'s). They cannot exit together, so **D-047**(3) splits the gate where the executed inversion had already cut it (§3 below), and the ladder above names the honest rungs rather than one gate that could only be reached twice over.

- **M2a — the non-ambient realm, and it ships first**, because it is what the executed inversion actually built toward. Its content is the Phase-2 document's **M2.5** and its evidence is that rung's two named mock-free gates.
- **M2b — the semantic realm, and it is the Phase-2 exit.** Its content is that document's **M2.1** (the walking skeleton) → **M2.2** → **M2.3** → **M2.4** → **M2.6**: Firefox driven by semantic tree under epoch/CAS, the first benchmark numbers, and the core spec 1.0-candidate.

**M3 and M4 keep their numbers**, which is the whole reason this is an `a`/`b` split rather than a renumbering: WS-B and WS-C both key triggers to milestone ids — [11-workstream-funding.md](11-workstream-funding.md) §2's STF application and [12-workstream-community.md](12-workstream-community.md) §4's announcement beats — so renumbering M3 would move a funding artifact and an announcement beat for no gain.

**What an existing reference to "M2" now means: M2b.** Every "M2" elsewhere in this tree and in [20-decision-log.md](20-decision-log.md) Part B denotes the Phase-2 *exit*, which M2b is; §4 and §5 below are read that way and are deliberately not rewritten. Two references are written out rather than left to that reading, for different reasons. **Exactly one thing moves: WS-C's second announcement beat, from M2 to M2a** (**D-047**(3)), because its two named demos live there — and under the inversion they are four tasks from done rather than sixty. And [04-phase-4-horizon.md](04-phase-4-horizon.md)'s entry-gate bullet, which keys the published benchmark to an "M2/M3 exit artifact", now names **M2b** in place: nothing moves and no threshold changes — the benchmark is still the Phase-2 exit's — but an M4 gate-review threshold is the wrong place to make a reader resolve which half of a split gate was meant.

**Where the rungs behind this split are cut, and it is not here.** **D-047** obliged [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) §4 to re-cut its M2.1–M2.6 rungs against this split, and that has landed: its block headed "REALIGNED 2026-08-25 BY D-047 — the rungs re-cut against M2a/M2b" leaves the M2.1–M2.6 table standing and adds a mapping table over it — **M2a** = **M2.5** entire, **M2b** = **M2.1** (skeleton) → **M2.2** → **M2.3** → **M2.4** → **M2.6** — each row naming the mock-free gates that close it. That document owns the mapping; this table is the ladder's view of it, and the two are to be read together.

## 2. Swimlanes and coupling points

```
             M0 ─────────► M1 ─────────► M2a ────────► M2b ─────────► M3 ─────────► M4 (gate)
Engineering  plan tree     Phase 1       Phase 2       Phase 2        Phase 3       Phase 4
             spec draft    MVP demo      non-ambient   semantic       network+X11+  (if gate
                                         realm         realm          fleet+wallet  passes)
WS-A spec    spec 0.1 ──── spec 0.2 ────────────────── core 1.0-cand  net 1.0-cand  wallet profile
WS-B funding               NLnet app ───────────────── STF app ────── NLnet #2 /    sponsors
                           (with demo)                 (a11y framing) (fleet slice)
WS-C comm.   licensing ─── ANNOUNCE ──── 2nd beat ─────────────────── GOVERNANCE.md
             files, quiet  (demo-first)  (security demos)             trigger zone
```

The M2 column became two per **D-047**(3) — see "The M2 split" in §1. WS-C's second beat is drawn at **M2a**, moved there by that decision; WS-A's core 1.0-candidate and WS-B's STF application stay at the Phase-2 exit, which is **M2b**.

**WS-D and WS-E have no lane above, and that is a statement about them rather than an omission.** Both were opened after M1 closed — [13-workstream-agent-integration.md](13-workstream-agent-integration.md) on 2026-07-26, [14-workstream-session-mode.md](14-workstream-session-mode.md) by [D-021](20-decision-log.md#d-021--session-mode-is-scheduled-as-a-maintainer-dogfooding-workstream-ws-e-and-that-is-not-the-horizon-item) on 2026-08-06 — so neither has an M0→M1 history to draw, and neither is sequenced milestone-relatively here. WS-D's ordering lives in its §5, whose first item, an MCP server, is unbuilt. **WS-E is no longer in flight, and that — not motion — is now why it has no lane:** its tracking epic [#206](https://github.com/vitrin-os/vitrin-os/issues/206) closed on **2026-08-13** with all nineteen of its task sub-issues closed. D-021(2) independently forbids reading any WS-E deliverable as evidence toward M4, so it could never have acquired an M4 lane in any case.

**What a closed WS-E owed the phase tree, and where that debt was discharged.** Two things, both **D-047**(1)'s, and both are now written in [14-workstream-session-mode.md](14-workstream-session-mode.md) rather than here. First, a numbered **E-series exported-artifact block** on the A1–A6 / C1–C8 model: that document had no exported-artifact section at all, so under the plan tree's own rule — *"anything not listed here is an internal detail free to change"* ([01-phase-1-mvp.md](01-phase-1-mvp.md) §1, [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) §1) — the DRM/KMS backend, multi-realm spawn, the served layout and launch verbs, the cross-realm clipboard, pointer constraints, idle inhibit, gestures, relative motion, the `attention` event, backlight actuation, the lock and idle-blank policy, VT handling and the `ViewGeometry` inset were all internal details later phases may not rely on, which is false because Phase 3 already reaches for several. That document's **§9** now carries **E1–E9**, each entry giving *"what the artifact is, the WS-E task and issue that produced it, who may consume it, and the limitations it travels with"*, and saying in its own opening that it is a contract written after the fact. Second, a **Stage 5** owning the four decisions WS-E produced that are accepted, unbuilt and scheduled nowhere: **D-038** (the shell as a realm), **D-039** (hotkeys as named actions), **D-040** (the N-surface scene, and the layer-shell/tiling deferral resting on it) and **D-046** (the principal socket) — all in [20-decision-log.md](20-decision-log.md). It is now the fifth row of that document's §4 stage table, with §4.5 behind it, and it gives them a home while re-deciding no clause of any of them.

Coupling arrows that drive sequencing:

- **M1 demo → NLnet application** (an application with a runnable demo is categorically stronger — WS-B §1).
- **E2.2 tree schema → core spec module freeze** (the spec follows the measured implementation, D-014).
- **M2 benchmark + coverage matrix → STF application** (WS-B §2).
- **M1 → public announcement** (quiet until a runnable MVP exists — WS-C §4).
- **E3.7 provenance → durable consent-ladder rungs anywhere** (E2.6 ships them structurally blocked).

## 3. Sequencing rules

- **Phase 2 epics that may start before M1 fully closes:** E2.6 and E2.7 depend only on the realm-spawn model (A2) and consent (A3), not on semantics — they can begin as soon as those artifacts stabilize. E2.8 (IME) is parallel by design. **This rule was a permission, it was overridden by name on 2026-08-06, and the override has since been executed — see "The executed order" below, which is the standing sequencing rule and restates this bullet rather than replacing it.**
- **Phase 3 long-lead pre-study during Phase 2:** E3.7's standards tracking (OID4VC/eIDAS pins) and E3.1's codec evaluation (Q6) both start early via the WS-A liaison table.
- **The earliest hard decision gate was Q11** (realm vs. Unix-user boundary) at Phase 2 start — it shaped E2.6/E2.7's namespace/UID layout, and it was **closed by [D-020](20-decision-log.md#d-020--the-realm-boundary-is-a-namespace-boundary-intra-user-by-default-in-namespace-uidgid-and-a-residue-that-lives-outside-every-realm)** (2026-08-06). [D-037](20-decision-log.md#d-037--the-realms-namespaces-are-built-by-a-helper-that-execs-first-and-unshares-second-the-core-proves-the-confinement-from-outside-before-it-commits-and-the-measured-ceiling-and-the-selected-policy-are-two-vocabularies-whose-bottoms-may-never-share-a-word) executes D-020(1)(4)(6) in P2.6.2; D-020(3)'s per-UID provisioning (E3.3), D-020(5)'s host-level sidecar residue (M3) and [D-010](20-decision-log.md#d-010--per-realm-isolation-dial)'s hardened and paranoid tiers (unscheduled) remain open, and R2.9 is not retired.
- **Within Phase 1**, the only hard serialization point is the IDL freeze; see the dependency graph in [01-phase-1-mvp.md](01-phase-1-mvp.md) §4.

#### The executed order — REALIGNED 2026-08-25 BY D-047

**Spent: *"epics that may start before M1 fully closes."*** A permission is the wrong instrument for an order that has already been taken. [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) §3 has overridden this bullet **by name** since 2026-08-06 — *"the roadmap says E2.6/E2.7 may start early because they need only A2/A3. Under this decomposition they **should** go first"* — and gives three reasons this document never carried: P2.6.2 owns the mount table P2.1.3's private bus socket lives in, so building the bus first means bind-mounting it in afterwards and re-doing the confinement claim; P2.6.3's Landlock ruleset and P2.6.4's seccomp deny-list have to be brought up against a realm that already runs `dbus-daemon` and the dconf/gsettings machinery Firefox drags in with accessibility enabled, and discovering that later means widening a policy under schedule pressure; and R2.9 is the one risk that can invalidate two whole epics and retires only by running the preflight on real kernels.

**The order as executed, stated as an order.** The confinement track ran first. Q11 closed at Phase-2 start by **D-020** (2026-08-06); the confinement and powerbox work then landed against **D-037**, **D-044** and **D-045**, and **D-047**(3) counts that track **four of sixteen built** against a semantic chain with **zero** task issues. The recommended serial order is that document's §3, not this one's: **P2.6.1 → P2.6.2 (+P2.6.3/P2.6.4/P2.7.1) → P2.1.1 → P2.1.2 → Track A/B to ★P2.1.9 → E2.2 → E2.3 → powerbox/egress to ★P2.6.9/★P2.7.6 → E2.4/E2.5 → E2.8 → E2.9** — with Track D (IME) the only track deferrable wholesale, and therefore the schedule shock absorber rather than the spine. This is also why **M2a** ships before **M2b** (§1): the split follows the order, it does not create it.

**Three keyings to the superseded order are named here rather than edited**, because each belongs to a document **D-047** hands to a different owner. [11-workstream-funding.md](11-workstream-funding.md) §1 calls the fundable core *"E2.1–E2.4 plus powerbox E2.6"* and §2 keys the STF fit to *"the AccessKit/AT-SPI bridge and a11y-adjacent infrastructure (E2.1/E2.2)"* — epics **D-047**(2)'s audit found with no task issues at all (E2.1 11 rows, E2.2 8, E2.3 7, E2.4 7; only E2.1's eleven are cut by that decision), which is why it says in as many words that **"WS-B's claim that the fundable core is submittable is not true of E2.1–E2.4"** and WS-B must re-point at the work that *is* decomposed, or wait. [13-workstream-agent-integration.md](13-workstream-agent-integration.md) §5 makes its item 7 (set-of-marks / semantic nodes) depend on *"Phase 2 (E2.1, E2.4)"*, the same slice. Nothing in this section edits either file.

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
| Phase-1 MVP end-to-end | demo runs on a clean machine from README instructions — **spent; see "The Phase-1 MVP metric" below** | M1 |
| Phase-2 semantic + epochs | E2.3 StaleEpoch harness demo + measured tree-diff sizes | M2 |
| Phase-3 network + X11 + fleet | 50-realm box + cross-machine drive + anti-keylog test, all scripted | M3 |
| Contributors beyond the author | ≥2 non-author contributors with merged non-trivial PRs | M2→M3 (WS-C) |
| Funded grant | NLnet MoU signed | post-M1 (WS-B) |
| Citations in agent-infra / capability-security discussions | tracked list; ≥5 independent mentions | M4 gate (soft) |
| Benchmark vs. screenshot baseline | OSWorld-style subset (named N tasks): ≥10× token-cost reduction, success rate ≥ parity, wall-clock improvement reported — vs. the Xvfb+screenshot+xdotool baseline (PRD §1.6) | first cut at M2 (Firefox realm); full at M3 (fleet + X11) |

Honest note: the citation and implementer-intent metrics are not fully controllable; they are M4 gate *inputs*, not epic exit criteria.

#### The Phase-1 MVP metric — REALIGNED 2026-08-25 BY D-047

**Spent: *"demo runs on a clean machine from README instructions."*** This is the same claim as M1's exit evidence in §1 and it is false in the same way; the five measured kernels, the ABI-6 admission floor and the AppArmor precondition are set out there and are not repeated. The measurable form that holds today: **the demo runs from the README on a machine whose kernel reports Landlock ABI ≥ 6, and on an AppArmor-carrying distribution only after `packaging/apparmor/vitrind` is installed by hand** — two of the five kernels [`docs/book/src/isolation-kernels.md`](../book/src/isolation-kernels.md) measured.

**Two downstream artifacts are keyed to the spent form, which is why D-047(6) restates it rather than deleting it.** [11-workstream-funding.md](11-workstream-funding.md) §1 makes the runnable demo the gating artifact of the NLnet application — the first coupling arrow in §2 above, *"an application with a runnable demo is categorically stronger"*. [12-workstream-community.md](12-workstream-community.md) §4 makes *"a runnable MVP with copy-paste instructions (the P1.9.5 README, verified on a clean machine)"* the **first** item of the announcement, in stated order, ahead of the recording and the manifesto. Both therefore rest on a claim that currently holds on two kernels, and both documents are owned by **D-047**'s enumeration rather than by this section.

**Whether to prioritise the packaging that would make the original sentence true again is left open by D-047(6) and is not decided here.** [#293](https://github.com/vitrin-os/vitrin-os/issues/293) is filed; nothing schedules it.
