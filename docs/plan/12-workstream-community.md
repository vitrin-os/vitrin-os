# Workstream C — Community, governance, licensing

The [PRD](../PRD.md) treats bus factor (§9, Q8) and communication opacity (§9, the Arcan lesson) as first-class project risks. This workstream is their standing mitigation plan, plus the mechanical setup that executes the licensing decision (D-005).

## 1. Bus factor (Q8) — standing mitigations

- **Spec-first artifacts** are the primary hedge: the protocol outlives the author (WS-A, [10-workstream-spec.md](10-workstream-spec.md)).
- **Design-doc-per-subsystem rule:** every epic's "design decisions" land as short documents (the `docs/protocol/` pages and epic sections in this tree), so the codebase stays enterable by a newcomer at any point.
- **Boring code in the core** recorded as a review norm — cleverness in the TCB is a liability twice over.
- **Funded second contributor** is the explicit budget goal of the first NLnet grant ([11-workstream-funding.md](11-workstream-funding.md) §1).
- **Succession note:** org/namespace access (GitHub org, npm scope, crates) held such that the project survives the individual — a named backup holder once one exists.

## 2. Licensing setup (executes D-005)

At first public push:

- **MPL-2.0** on the reference implementation (LGPL-3.0 the recorded fallback);
- **Apache-2.0** (+ explicit patent-grant notice) on wire definitions, schemas, and SDKs;
- **CC-BY-4.0** on spec prose;
- SPDX headers throughout; per-directory LICENSE files where the split changes.
- **DCO, not CLA** (D-012): sign-off on commits; a CLA deters the contributors a single-maintainer project needs most.

## 3. Repo-hygiene ladder

| When | What |
|---|---|
| **M0** (now) | LICENSE files, README, this plan tree public |
| **M1** (announcement) | CONTRIBUTING.md, CODE_OF_CONDUCT.md, **SECURITY.md** (a capability-security project *will* receive vulnerability reports — a disclosure policy at announcement is non-optional), issue templates, "good first issue" seeding |
| **3+ regular contributors** or first funded contributor | GOVERNANCE.md (see §5) |

## 4. Announcement policy (the §9 communication-opacity mitigation)

Quiet until **M1**. Announce with, in this order:

1. a runnable MVP with copy-paste instructions (the P1.9.5 README, verified on a clean machine);
2. a ~3-minute demo recording;
3. one worked example in plain words ("an agent fills a form in Firefox inside a realm; the human takes over by touching the mouse; holding Esc revokes everything");
4. and only *then* the manifesto link.

Demo above prose — explicitly inverting Arcan's failure mode ("I still don't feel like I know what it is"). A second announcement beat lands at **M2**: the ransomware demo and the `ssh localhost` demo (E2.6/E2.7 exit artifacts) are the shareable security stories.

Channels: Show HN, LWN outreach, agent-infrastructure communities (the PRD §3 personas), Wayland/a11y circles (Newton/AccessKit adjacency).

## 5. Governance

- **Now:** documented single-maintainer (BDFL). Stating it plainly beats pretending otherwise.
- **Trigger for GOVERNANCE.md** (decision process, maintainer-addition rules): 3+ regular contributors or the first funded contributor, whichever comes first.
- **Trust-root governance (Q14)** is cross-referenced here deliberately: "who operates the transparency log, and which issuers are default-trusted" (PRD §20.14) is a project-neutrality question, not just a technical one. It must be answered before any durable standing grant ships (E3.7), and the recommended default — per-deployment-configurable roots, federating with Sigstore public-good instances, no project-run log initially — is recorded in [03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md) E3.7.
