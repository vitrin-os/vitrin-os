# Workstream B — Funding & sustainability

Posture per [PRD](../PRD.md) §10, verbatim: open-source first; no business model centered; a hosted fleet-management control plane noted as a later, optional layer and out of this plan's scope. Funding milestones are tied to engineering milestones ([00-roadmap.md](00-roadmap.md)) because every application below is categorically stronger with a runnable demo than with a manifesto — the PRD's own §9 argument, applied to funders.

## 1. NLnet / NGI Zero (primary)

Precedent: NLnet funded Arcan-A12 — exactly this class of infrastructure.

- **Timing:** apply to the **first call after M1**, milestone-relative: the application is drafted during the Phase-1 endgame and submitted with the M1 demo linkable. NLnet runs calls on a roughly two-month cadence, so no calendar date is needed — the demo is the gating artifact.
- **Ask shape:** NLnet negotiates an MoU with payment-per-milestone against a concrete plan and itemized budget (typical envelope €5k–50k). The application's project plan is **Phase 2, nearly verbatim**: the fundable core is the semantic chain **E2.1–E2.4 plus powerbox E2.6** ([02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md)), each epic mapping 1:1 to an MoU milestone with a budget line. This is a deliberate design property of this plan tree: the Phase-2/3 epic structure is written to be submittable.
- **Artifact checklist:**
  - demo video + copy-paste run instructions (M1 exit artifact, P1.9.5);
  - a comparison-with-existing-efforts section (condensed from PRD §1);
  - licensing statement (D-005);
  - milestone/budget table (from the epic structure);
  - sustainability paragraph (this document).
- **Second application** targeting the Phase-3 fleet slice (E3.1/E3.3/E3.4) after M2.
- **Budget priority:** the first grant's explicit goal is a **funded second contributor** (Q8, bus factor — see [12-workstream-community.md](12-workstream-community.md) §1).

## 2. Sovereign Tech Fund (secondary, later)

Precedent: STF funds Newton via the GNOME Foundation (PRD §1.3, §10).

- **Fit argument:** STF funds critical public-interest infrastructure with *demonstrated* relevance. A pre-adoption greenfield project is a weak fit; the strong fit is the **AccessKit/AT-SPI bridge and a11y-adjacent infrastructure (E2.1/E2.2)** framed as shared ecosystem plumbing, plus post-M2 adoption evidence.
- **Timing:** earliest credible submission after **M2** (benchmark + coverage matrix as evidence); realistic target after M3, with fleet users to point at.
- **Application needs:** usage/dependency evidence; a maintenance plan (cross-reference the bus-factor mitigations in WS-C); team/contractor structure.

## 3. Other rails (minimal)

- **GitHub Sponsors / OpenCollective:** switched on at public announcement (M1) — zero-effort standing infrastructure.
- **Corporate agent-infra sponsorship:** deliberately deferred until the M3 fleet demo exists — that is the artifact such sponsors buy into.

## 4. Metric hook

"A funded grant (NLnet/NGI Zero)" is a PRD §7 community metric; its status (drafted / submitted / MoU signed) is tracked in the [00-roadmap.md](00-roadmap.md) metrics table.
