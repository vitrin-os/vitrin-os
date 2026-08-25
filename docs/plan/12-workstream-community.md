# Workstream C — Community, governance, licensing

The [PRD](../PRD.md) treats bus factor (§9, Q8) and communication opacity (§9, the Arcan lesson) as first-class project risks. This workstream is their standing mitigation plan, plus the mechanical setup that executes the licensing decision (D-005).

## 1. Bus factor (Q8) — standing mitigations

- **Spec-first artifacts** are the primary hedge: the protocol outlives the author (WS-A, [10-workstream-spec.md](10-workstream-spec.md)).
- **Design-doc-per-subsystem rule:** every epic's "design decisions" land as short documents (the `docs/protocol/` pages and epic sections in this tree), so the codebase stays enterable by a newcomer at any point.
- **Boring code in the core** recorded as a review norm — cleverness in the TCB is a liability twice over.
- **Funded second contributor** is the explicit budget goal of the first NLnet grant ([11-workstream-funding.md](11-workstream-funding.md) §1).
- **Succession note:** org/namespace access (GitHub org, npm scope, crates) held such that the project survives the individual — a named backup holder once one exists.

## 2. Licensing setup (executes D-005)

Planned for first public push; status tracked here. [D-016](20-decision-log.md) records the execution decisions and the root [`NOTICE`](../../NOTICE) is the normative path→license map.

- [x] **MPL-2.0** on the reference implementation — taken over the LGPL-3.0 fallback D-005 also recorded (D-016 gives the four reasons; the load-bearing one is that copyleft on the TCB makes "small and auditable" a license property rather than a README promise).
- [x] **Apache-2.0** (+ explicit patent-grant notice) on wire definitions, schemas, and SDKs — in force since first publication. D-016 extends it to everything *derived* from the protocol (the checked-in generated bindings, the generated C header, the codegen that emits them) and to the conformance instruments, which is the answer to the open question in issue #133.
- [x] **CC-BY-4.0** on spec prose — in force since first publication.
- [x] **SPDX headers** on first-party sources — every `.rs`, `.c`, `.h`, `.py`, `.sh` and `.js` file — plus a per-crate `license` field and no workspace-wide default, so a new crate cannot inherit the wrong half silently. Two scoping facts, stated rather than glossed: build manifests, the IDL and its schema, Markdown and fixture data carry **no** inline header and are covered by the `NOTICE` path map instead; and coverage is *not machine-checked* — there is no `reuse lint`-style CI gate, so it can rot. Adopting the REUSE layout is a deliberate follow-up, not something to assume done.
- [x] **Root license texts + `NOTICE` as the map** — this supersedes the planned "per-directory LICENSE files where the split changes", which D-016 rejected on the facts: `shim/` carries three licenses and `shim/include/` two, so a directory-level file would be *false* in both places. Per-file headers plus one map is the truthful spelling.
- [x] The pre-existing **GPL-3.0-only carve-out** (`shim/wlcs/`) is preserved and now has its license text in the tree, which GPL-3.0 §4 requires and which was missing.
- [x] **No patent filings; defensive publication instead** ([D-015](20-decision-log.md)) — recorded, with the 2026-07-26 landscape scan, its prior-art anchors, and its explicit limits (a public-source scan, *not* a freedom-to-operate opinion).
- [ ] **Join the Open Invention Network** (D-015) — free below $10M revenue and the third leg of that decision, but **not yet done**; the other two legs (publication, license patent grants) are already in force.
- [x] **DCO, not CLA** (D-012): sign-off on commits; a CLA deters the contributors a single-maintainer project needs most. **Executed**: [`CONTRIBUTING.md`](../../CONTRIBUTING.md) states the policy, `.github/workflows/dco.yml` enforces a `Signed-off-by:` trailer per commit on every pull request, and D-012 is `accepted, executed` in the log. This bullet said the opposite — "not executed", "no `CONTRIBUTING.md` yet", "nothing enforces sign-off" — for months after all three became false, which is exactly the drift [#172](https://github.com/vitrin-os/vitrin-os/issues/172) exists for; `cargo xtask limits-check` now holds this tick to that workflow's existence in both directions.

## 3. Repo-hygiene ladder

| When | What |
|---|---|
| **M0** (now) | LICENSE files, README, this plan tree public |
| **M1** (announcement) | CONTRIBUTING.md, CODE_OF_CONDUCT.md, **SECURITY.md** (a capability-security project *will* receive vulnerability reports — a disclosure policy at announcement is non-optional), issue templates, "good first issue" seeding |
| **3+ regular contributors** or first funded contributor | GOVERNANCE.md (see §5) |

## 4. Announcement policy (the §9 communication-opacity mitigation)

Quiet until **M1**. Announce with, in this order:

1. a runnable MVP with copy-paste instructions (the P1.9.5 README, verified on a clean machine) — **see "The first item, and the second beat" below, which is the standing record for this item and for the M2 beat, and restates them rather than replacing them**;
2. a ~3-minute demo recording;
3. one worked example in plain words ("an agent fills a form in Firefox inside a realm; the human takes over by touching the mouse; holding Esc revokes everything");
4. and only *then* the manifesto link.

Demo above prose — explicitly inverting Arcan's failure mode ("I still don't feel like I know what it is"). A second announcement beat lands at **M2**: the ransomware demo and the `ssh localhost` demo (E2.6/E2.7 exit artifacts) are the shareable security stories. **That beat now lands at M2a, not M2 (D-047 decision 3); see "The first item, and the second beat" below, which is the standing record for this sentence and restates it rather than replacing it.**

Channels: Show HN, LWN outreach, agent-infrastructure communities (the PRD §3 personas), Wayland/a11y circles (Newton/AccessKit adjacency).

### The first item, and the second beat

**REALIGNED 2026-08-25 BY D-047** (decisions 3 and 6). Both statements above are left
standing; this restates them.

**Spent: "verified on a clean machine."** It is false as written, and it is the *first*
thing the announcement offers, which is why D-047 treats restating it as derived rather
than optional. The Landlock admission floor refuses **Ubuntu 22.04 (ABI 1), Debian 12
(ABI 2) and Ubuntu 24.04's GA kernel (ABI 4)** — three of the five measured
distribution kernels — and Ubuntu additionally needs an AppArmor profile that
[#293](https://github.com/vitrin-os/vitrin-os/issues/293) records nothing installs. So
the demo holds on **two of five measured kernels**, and the failure mode on the other
three is not a degraded demo but a refusal to start.

This matters more here than anywhere else in the tree. The announcement policy exists
to invert Arcan's failure mode — *"I still don't feel like I know what it is"* — and
the mechanism it uses is that the reader can run the thing. A Show HN reader on a stock
LTS kernel who copy-pastes the README and gets a refusal has been handed the **worse**
failure mode: not "I don't know what it is" but "it doesn't work." So the honest
spelling of item 1 is **a runnable MVP with copy-paste instructions and its kernel
floor stated in the same breath**, exactly as the floor is stated on the project's own
limits surfaces. Disclosed, the floor argues for the project — the core refuses to
start below the isolation tier it can actually enforce. Discovered, it argues against
it.

**D-047 files the packaging that would restore the sentence and does not schedule it.**
Whether a container or image carrying its own kernel floor, plus the AppArmor
profile #293 names, is worth prioritising is a real choice, left open there and not
made here.
Until it is made, **this document must not restore the unqualified sentence**, and
[11-workstream-funding.md](11-workstream-funding.md) carries the same precondition on
the same artifact.

**Spent: "a second announcement beat lands at M2."** The two demos it names are not M2
gates. They are the two mock-free gates of the confinement/powerbox rung: **★P2.6.9**
([#193](https://github.com/vitrin-os/vitrin-os/issues/193), the ransomware demo — the
payload realm's measured write set) and **★P2.7.6**
([#200](https://github.com/vitrin-os/vitrin-os/issues/200), the `ssh localhost` demo —
five measured claims, each with a control), both labelled `M2.5` on the tracker.

**Replacement: the second beat lands at M2a**, the non-ambient realm (D-047 decision
3), which is the rung those two gates actually exit. This is a correction of fact, not
a deferral — it moves the beat **earlier**, not later: under the executed inversion
those demos are a handful of tasks from done, where the semantic chain that M2b carries
has no task issues at all. **M3 and M4 keep their numbers**, so nothing else in this
document's triggers moves; that is why the split is `a`/`b` rather than a renumbering.

One thing the move does not change: both gates are `★`, meaning mock-free against the
shipped binaries. A beat announced on a component-test result would be the exact class
of claim this document's own §2 discipline exists to prevent.

## 5. Governance

- **Now:** documented single-maintainer (BDFL). Stating it plainly beats pretending otherwise.
- **Trigger for GOVERNANCE.md** (decision process, maintainer-addition rules): 3+ regular contributors or the first funded contributor, whichever comes first.
- **Trust-root governance (Q14)** is cross-referenced here deliberately: "who operates the transparency log, and which issuers are default-trusted" (PRD §20.14) is a project-neutrality question, not just a technical one. It must be answered before any durable standing grant ships (E3.7), and the recommended default — per-deployment-configurable roots, federating with Sigstore public-good instances, no project-run log initially — is recorded in [03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md) E3.7.

## 6. Tracker visibility of WS-A, WS-B and WS-C

**Added 2026-08-25 as an in-place addition, and *not* one of D-047's
enumerated changes** — that entry's obligation on this document is one thing,
*"WS-C's second beat moved to M2a (3)"*, discharged in §4 above. This section stands
on the tracker facts measured below rather than on a decision entry, and the next
entry to touch this workstream should adopt or reject it — the same footing
[02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md)'s P2.6.11 row
records for itself.

Two of the five workstreams are tracker-visible and three are not. `gh label list`
carries exactly two `workstream:*` labels — `workstream:agent-integration` (WS-D) and
`workstream:session-mode` (WS-E) — and a search of every issue, open and closed, finds
**no issue titled for WS-A, WS-B or WS-C**, and none for a spec release, a grant
application, an announcement or governance. WS-C has exactly one issue-shaped item
anywhere on the tracker, [#159](https://github.com/vitrin-os/vitrin-os/issues/159)
(join the OIN, D-015's third leg), and it carries `track:ci-docs` and `known-limit`
rather than a workstream label. WS-A and WS-B have nothing at all.

**Recorded here rather than fixed by relabelling: WS-A, WS-B and WS-C are deliberately
untracked, and the reason is what their deliverables are.** A spec release, a grant
application, an announcement thread and a governance document are not engineering
deliverables with acceptance tests; they are single acts, most of them gated on a
milestone rather than on other work, and CLAUDE.md's rule is that *labels state
membership and dependency edges state order*. Three workstreams whose entire content is
"do this once, when that milestone lands" gain nothing from a membership label and
would gain a completeness bar that overstates what the tracker knows — the same
objection that keeps GitHub milestones out of this repo.

**Two standing exceptions, so this note is not a licence to leave real work
untracked.** An item in one of these three workstreams gets a tracker issue when either
holds:

- **it has a deliverable another person could pick up** — the OIN membership is exactly
  this, which is why #159 exists and why the pattern is already established; or
- **it is a precondition of something the tracker does schedule** — WS-A's
  per-connection version matrix and D-015's landscape re-check are both preconditions
  of the core spec 1.0-candidate rung, and both are currently held only by prose in
  [10-workstream-spec.md](10-workstream-spec.md) §2 and §4.

Those two are named here as candidates for the tracker pass. **This document creates no
labels and changes none**; D-047 files rather than schedules, and the tracker is not
this file's to edit.
