# Decision log

Registry of decisions (Part A) and open questions (Part B) for Vitrin OS. Entries are ADR-shaped and cited by short ID (`D-005`, `Q11`) from every other plan document. Once a decision is **accepted**, its entry is append-only: changes are recorded as a new superseding entry, never edits.

Convention: this stays a single file until it exceeds ~25 accepted entries or external contributors begin proposing decisions, at which point it splits into `docs/plan/adr/` (see D-013).

---

## Part A — Decided

Entries D-001 through D-011 are seeded from decisions the [PRD](../PRD.md) already made; the PRD section cited is the full rationale. D-012 onward originate in this plan tree.

### D-001 — Rust + Smithay for the trusted core
**Status:** accepted (PRD Doc 2 §9, §17)
**Decision:** the trusted core (`vitrind`: compositor, capability kernel, grant store, input router, journal, motion synthesis) is Rust on Smithay.
**Context/consequences:** the core is the entire TCB; memory safety eliminates vulnerability classes in the most privileged component. Smithay is production-proven (niri, COSMIC) at frame deadlines. Go rejected (GC pauses vs. frame deadlines); C/C++ rejected for the core (TCB memory-safety argument).

### D-002 — C + wlroots shims, outside the TCB
**Status:** accepted (PRD Doc 2 §4.2, §17)
**Decision:** the Wayland shim is C on wlroots; the later X11 shim is C/C++ (Xwayland-derived) with an embedded minimal WM. Shims are unprivileged and disposable.
**Context/consequences:** the legacy semantics we want live in wlroots, mature; reusing them *outside* the TCB is the point. See Phase-1 risk R2 for the held-in-reserve Rust-shim pivot option.

### D-003 — Wayland-style Unix-socket wire protocol with SCM_RIGHTS; Cap'n Proto rejected for the local hot path
**Status:** accepted (PRD Doc 2 §3.2)
**Decision:** local principals speak a Wayland-style binary protocol over Unix domain sockets with fd passing; handles are per-connection, sender-constrained. Cap'n Proto RPC's conceptual model (handles-as-capabilities, attenuation, pipelining) is adopted; its transport is not (no shared-memory transport in practice; buffers move as dmabuf fds outside the payload). Protobuf/gRPC and FlatBuffers rejected for the local path.

### D-004 — QUIC (quinn) for network sessions
**Status:** accepted (PRD Doc 2 §10)
**Decision:** remote sessions run over QUIC: multiplexed streams, TLS 1.3, connection migration; workload identity bound to the channel. Cap'n Proto remains optional for the network control plane (final call at E3.1 — see Q6 for the codec question).

### D-005 — Split licensing
**Status:** accepted (PRD §11)
**Decision:** protocol spec + wire definitions under Apache-2.0 (explicit patent grant), spec prose CC-BY-4.0; reference implementation under weak copyleft (MPL-2.0 preferred, LGPL-3.0 the fallback); client SDKs Apache-2.0.
**Consequences:** setup work (LICENSE files, SPDX headers) executes at first public push — see [12-workstream-community.md](12-workstream-community.md) §2.

### D-006 — Naming: Vitrin OS
**Status:** accepted (PRD Naming section)
**Decision:** project **Vitrin OS**, daemon `vitrind`, org `vitrin-os`, npm scope `@vitrin-os`, crates `vitrin-os`/`vitrind` (namespaces claimed 12 July 2026). Kavşak dropped (pronounceability), Torii recorded but not adopted.

### D-007 — Scope tiering: invariants / v1 / horizon / renounced
**Status:** accepted (PRD §5)
**Decision:** scope statements are kept in four distinct classes: permanent invariants (§5.1), v1 sequencing (§5.2), claimed-but-deferred horizon (§5.3), renounced non-goals (§5.4). Every plan document inherits this discipline; horizon items never silently migrate into a phase without an M4 gate review ([04-phase-4-horizon.md](04-phase-4-horizon.md)).

### D-008 — Pluggable identity verifier; no hard commitment to in-flight standards
**Status:** accepted (PRD Doc 2 §5.1, Caveats)
**Decision:** identity verification (SPIFFE SVID, OIDC, SSH certificates, MVP static identity) sits behind a pluggable `Verifier` abstraction. IETF AIMS, MCP authorization revisions, and OID4VC profiles are tracked, pinned, and re-checked — never hard-wired (see the liaison table in [10-workstream-spec.md](10-workstream-spec.md)).

### D-009 — Transparency log for provenance; deliberately not a blockchain
**Status:** accepted (PRD P14, Doc 2 §13)
**Decision:** app provenance uses Sigstore-style identity-bound short-lived signing certificates plus a Merkle transparency log (checkpoint + inclusion proof verified locally). No consensus, no tokens, no ledger on the grant-time hot path. DIDs acceptable as an identity *format* only.

### D-010 — Per-realm isolation dial
**Status:** accepted (PRD Doc 2 §4.5)
**Decision:** isolation strength is per-realm policy over one identical GUI protocol: default (namespaces + seccomp + Landlock), hardened (gVisor-class), paranoid (microVM). Every realm gets its own network/PID/IPC namespaces and UID regardless of tier. Security claims are stated per tier (only the microVM tier escapes shared-kernel escape classes).

### D-011 — v1 deployments: headless fleet + local nested; one Wayland shim first
**Status:** accepted (PRD §5.2, §8)
**Decision:** v1 targets the two deployments with no open incumbent (headless agent fleets; nested-in-a-desktop). The X11 shim, network sessions, and fleet mode follow in Phase 3. Session mode on bare DRM/KMS is horizon-tier.

### D-012 — DCO, not CLA
**Status:** proposed
**Decision:** contributions are accepted under the Developer Certificate of Origin (sign-off), not a Contributor License Agreement.
**Rationale:** a CLA is a contributor deterrent, and a single-maintainer project needs contributors more than it needs relicensing optionality; the licensing split (D-005) already secures the protocol/implementation boundary. Revisit only if a fiscal host requires otherwise.

### D-013 — Single-file decision log
**Status:** proposed
**Decision:** decisions and open questions live in this one file until ~25 accepted entries or external decision proposals arrive; then split into `docs/plan/adr/`.

### D-014 — Spec versions track the implementation (module-freeze ladder)
**Status:** proposed
**Decision:** the protocol spec is published early but versioned `0.x` and explicitly tracking the reference implementation; modules freeze on the ladder defined in [10-workstream-spec.md](10-workstream-spec.md) §2 (0.1 at M0, 0.2 at M1, core 1.0-candidate at M2, network profile at M3).
**Rationale:** the epoch/CAS mechanism is "a design claim, not a proven result" (PRD Caveats); freezing it before E2.3 measures it would enshrine guesswork. Running code before prose authority (the anti-Arcan posture, PRD §9).

---

## Part B — Open questions (PRD §20), with owners and decide-by gates

Each row: the PRD §20 question (kept verbatim there; summarized here), the epic(s) it blocks, and the milestone or moment by which a decision must exist. Epic IDs refer to [02-phase-2-semantic-epochs.md](02-phase-2-semantic-epochs.md) and [03-phase-3-network-x11-fleet.md](03-phase-3-network-x11-fleet.md); milestones to [00-roadmap.md](00-roadmap.md).

| Q | Question (short) | Blocks | Decide-by |
|---|---|---|---|
| Q1 | Epoch granularity vs. animation-heavy UIs | E2.3 | initial invalidation policy at E2.3 start; tuned empirically before spec 1.0-candidate (M2) |
| Q2 | Semantic node-addressing stability across SPA-style rebuilds | E2.2 | v0 strategy (fingerprint re-identification + explicit invalidation) at E2.2 start; revisit with the coverage matrix (M2) |
| Q3 | VLM fallback trust and confidence surfacing | E2.4 | E2.4 design; per-node confidence field frozen at spec 1.0-candidate |
| Q4 | Grant delegation-chain depth | protocol spec | interim cap (depth = 1) acceptable through Phase 2; must close before spec 1.0-candidate (M2) |
| Q5 | Portal-compat coverage on real apps | E3.5 | empirical; compat matrix published at M3 |
| Q6 | Network buffer codec | E3.1 | evaluation during Phase 2; decision at E3.1 start |
| Q7 | Identity-standard churn (AIMS, MCP auth) | none (mitigated by D-008) | standing review each milestone via the liaison table (WS-A) |
| Q8 | Bus factor | project survival; M4 gate input | ongoing (WS-C); "funded second contributor" is the first grant's explicit budget goal |
| Q9 | Standing-grant ergonomics for gesture-less software | E2.6 (v0), E3.7 (full) | v0 posture (non-durable rungs only) at E2.6; full answer with provenance at E3.7 |
| Q10 | Atomic-save patterns over FUSE synthetic paths | E3.6 | empirical; compat matrix at M3 |
| Q11 | Principal boundary vs. Unix-user boundary | **E2.6 + E2.7 spawn/namespace layout** | **Phase 2 start — the earliest hard gate in this log** |
| Q12 | Egress-designation ergonomics (browser realms) | E2.7 (v0), fleet UX | v0 posture (per-realm template allowlists, no blanket grants) at E2.7; full answer by M3 |
| Q13 | Human factors of the consent ladder | E2.6 prompts; E3.7 durable rungs | first prompt-design review at E2.6; mandatory re-review before durable rungs ship (E3.7) |
| Q14 | Trust-root governance (logs, issuers) | E3.7 durable rungs | before any durable grant ships; cross-references governance in WS-C |
