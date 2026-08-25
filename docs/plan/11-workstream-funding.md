# Workstream B — Funding & sustainability

Posture per [PRD](../PRD.md) §10, verbatim: open-source first; no business model centered; a hosted fleet-management control plane noted as a later, optional layer and out of this plan's scope. Funding milestones are tied to engineering milestones ([00-roadmap.md](00-roadmap.md)) because every application below is categorically stronger with a runnable demo than with a manifesto — the PRD's own §9 argument, applied to funders.

## 1. NLnet / NGI Zero (primary)

Precedent: NLnet funded Arcan-A12 — exactly this class of infrastructure.

- **Timing:** apply to the **first call after M1**, milestone-relative: the application is drafted during the Phase-1 endgame and submitted with the M1 demo linkable. NLnet runs calls on a roughly two-month cadence, so no calendar date is needed — the demo is the gating artifact. **See "The gating artifact carries a precondition" below, which is the standing record for this bullet and restates it rather than replacing it: the demo runs on two of five measured kernels.**
- **Ask shape:** NLnet negotiates an MoU with payment-per-milestone against a concrete plan and itemized budget (typical envelope €5k–50k). The application's project plan is **Phase 2, nearly verbatim**: the fundable core is the semantic chain **E2.1–E2.4 plus powerbox E2.6** ([02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md)), each epic mapping 1:1 to an MoU milestone with a budget line. This is a deliberate design property of this plan tree: the Phase-2/3 epic structure is written to be submittable. **See "The fundable core, re-pointed" below, which is the standing record for this bullet and restates it rather than replacing it: E2.1–E2.4 have no scoped work to price.**
- **Artifact checklist:**
  - demo video + copy-paste run instructions (M1 exit artifact, P1.9.5);
  - a comparison-with-existing-efforts section (condensed from PRD §1);
  - licensing statement (D-005);
  - milestone/budget table (from the epic structure);
  - sustainability paragraph (this document).
- **Second application** targeting the Phase-3 fleet slice (E3.1/E3.3/E3.4) after M2. **M2 is now M2a + M2b (D-047 decision 3), and this "M2" reads M2b: [00-roadmap.md](00-roadmap.md) §1 makes every existing reference to M2 in this tree denote the Phase-2 exit, which M2b is, and names WS-C's second announcement beat as the one trigger that moves to M2a. No choice is left open here.**
- **Budget priority:** the first grant's explicit goal is a **funded second contributor** (Q8, bus factor — see [12-workstream-community.md](12-workstream-community.md) §1).

### The fundable core, re-pointed

**REALIGNED 2026-08-25 BY D-047** (decision 2's stated cost, and decision 6). The two
bullets above are left standing; this restates them.

**Spent: "the fundable core is the semantic chain E2.1–E2.4 plus powerbox E2.6 …
each epic mapping 1:1 to an MoU milestone with a budget line."** The 1:1 mapping is
still the right shape. What is not true today is that E2.1–E2.4 contain anything to
price. Verified on the tracker, 2026-08-25:

| Epic | Tracker | Task issues | State |
|---|---|---|---|
| E2.1 semantic bridge | [#175](https://github.com/vitrin-os/vitrin-os/issues/175) | **none** | epic issue only |
| E2.2 tree versioning / addressing | [#176](https://github.com/vitrin-os/vitrin-os/issues/176) | **none** | epic issue only |
| E2.3 epoch / CAS | [#177](https://github.com/vitrin-os/vitrin-os/issues/177) | **none** | epic issue only |
| E2.4 VLM fallback | [#178](https://github.com/vitrin-os/vitrin-os/issues/178) | **none** | epic issue only |
| E2.6 filesystem powerbox | [#180](https://github.com/vitrin-os/vitrin-os/issues/180) | **10** (#185–#194) | #185–#188 closed; six open |
| E2.7 network authority | [#181](https://github.com/vitrin-os/vitrin-os/issues/181) | **6** (#195–#200) | all open |

Those sixteen are **every** Phase-2 task issue that exists, and all sixteen sit in
E2.6/E2.7. An MoU line is a priced deliverable with an acceptance test; four of the
five epics this bullet names have neither. **D-047 decision 2 cuts E2.1's eleven task
issues now and re-cuts each later epic as it starts** — deliberately, because writing
fifty issues before a line of code is how Phase 1's tables drifted — so E2.2–E2.4 stay
unpriceable for as long as they stay unstarted. Decision 2 names this cost against
this document by name and does not pay it.

**Replacement, and the two ways out. Only the first is available now.**

1. **Re-point the first application at the slice that is decomposed and closest to
   done: the confinement/powerbox core, E2.6 + E2.7.** Sixteen scoped tasks, four
   already landed, each with a written acceptance criterion, and two mock-free gates
   that are shareable security stories in their own right —
   **★P2.6.9** ([#193](https://github.com/vitrin-os/vitrin-os/issues/193), the
   ransomware demo, a measured write set) and **★P2.7.6**
   ([#200](https://github.com/vitrin-os/vitrin-os/issues/200), the `ssh localhost`
   demo, five measured claims each with a control). Under D-047 decision 3 both are
   **M2a**, which is also the rung WS-C's second announcement beat now sits on. The
   budget-priority bullet above is unaffected: a funded second contributor is still
   the ask.
2. **Or wait for the semantic chain to be cut.** Legitimate, and it is what the
   original bullet assumed. It costs at least the decomposition of E2.1, and per
   decision 2 the rest arrive epic by epic rather than as one pass — so this route has
   no date, and this document may not invent one.

**Neither route is a re-scoping of Phase 2.** D-047 refused that explicitly: conceding
the differentiator phase on a bookkeeping argument is not an outcome anyone wants, and
the semantic chain remains what the project is for. Option 1 changes what the *first*
grant application prices, not what the phase delivers.

### The gating artifact carries a precondition

**REALIGNED 2026-08-25 BY D-047** (decision 6). "The demo is the gating artifact" is
correct and stays. What it silently assumed — that the demo runs anywhere — does not.

**"Demo runs from README on a clean machine" is false as written**, and
[00-roadmap.md](00-roadmap.md)'s M1 row and metrics row carry the restatement. The
Landlock admission floor refuses **Ubuntu 22.04 (ABI 1), Debian 12 (ABI 2) and Ubuntu
24.04's GA kernel (ABI 4)** — three of the five measured distribution kernels — and
Ubuntu additionally needs an AppArmor profile that
[#293](https://github.com/vitrin-os/vitrin-os/issues/293) records nothing installs. So
**the demo runs on two of five measured kernels**, and a reviewer who tries it on a
stock LTS box does not get a partial result; they get a refusal to start.

The consequences for this document, stated rather than smoothed:

- The **artifact checklist's first item** ("demo video + copy-paste run instructions")
  is two artifacts with different exposure. The recording holds unconditionally; the
  run instructions hold only on a kernel that clears the floor, and an application that
  ships them without saying so is inviting the one reviewer experience that cannot be
  recovered from.
- The honest spelling is to **state the floor in the application** rather than to hope
  the reviewer's kernel is new enough. The floor is a deliberate, measured refusal —
  the core will not start below the isolation tier it can actually enforce — which is
  an argument *for* the project when it is disclosed and an embarrassment when it is
  discovered.
- **D-047 leaves the packaging question open on purpose.** Whether to prioritise the
  work that makes the original sentence true again — a container or image that carries
  its own kernel floor, plus the AppArmor profile #293 names — is a real choice, filed
  and not scheduled. It is filed here too: this workstream is one of the two documents
  keyed to that claim, and it does not get to decide the other's schedule.

## 2. Sovereign Tech Fund (secondary, later)

Precedent: STF funds Newton via the GNOME Foundation (PRD §1.3, §10).

- **Fit argument:** STF funds critical public-interest infrastructure with *demonstrated* relevance. A pre-adoption greenfield project is a weak fit; the strong fit is the **AccessKit/AT-SPI bridge and a11y-adjacent infrastructure (E2.1/E2.2)** framed as shared ecosystem plumbing, plus post-M2 adoption evidence.
- **Timing:** earliest credible submission after **M2** (benchmark + coverage matrix as evidence); realistic target after M3, with fleet users to point at. **The evidence this bullet names is the semantic rung's, not M2a's** — D-047 decision 3 puts "the first benchmark numbers" in M2b, and the coverage matrix is P2.1.8's generated output inside the semantic epic E2.1 ([02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md)), which M2a does not contain — **so this trigger reads M2b, mechanically and without a further choice.** M3 is unaffected: it keeps its number.
- **Application needs:** usage/dependency evidence; a maintenance plan (cross-reference the bus-factor mitigations in WS-C); team/contractor structure.

## 3. Other rails (minimal)

- **GitHub Sponsors / OpenCollective:** switched on at public announcement (M1) — zero-effort standing infrastructure.
- **Corporate agent-infra sponsorship:** deliberately deferred until the M3 fleet demo exists — that is the artifact such sponsors buy into.

## 4. Metric hook

"A funded grant (NLnet/NGI Zero)" is a PRD §7 community metric; its status (drafted / submitted / MoU signed) is tracked in the [00-roadmap.md](00-roadmap.md) metrics table.
