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

### D-015 — No patent filings; defensive publication instead
**Status:** accepted (2026-07-26)
**Decision:** Vitrin OS files no patents. Protection rests on three things instead: **defensive publication** (the PRD and the IDL published with a timestamp are themselves the prior art), the **patent grants already carried by the licenses** (Apache-2.0 §3 on the protocol and SDKs, MPL-2.0 §2.1(b) on the reference implementation — see D-016), and **Open Invention Network membership**, to be joined (free below $10M revenue; OIN 2.0 launched January 2026; 4,000+ members cross-licensing 3M+ patents royalty-free within the Linux System definition, which covers the graphics stack).
**Rationale:** the economics do not work and the posture is wrong. A patent family costs roughly EUR 10–30k per jurisdiction and takes 2–5 years to grant, and Europe's absolute-novelty rule would force filing *before* publication — delaying the one act that actually protects the project. Publishing the design is the protection: it blocks anyone else from patenting these claims, immediately and for free. It is also the only posture consistent with the project's own argument: a capability-security system asking to be trusted is stronger defensive-permissive than behind a patent wall, and D-005's whole point is that the protocol is a commons.
**Consequences:** a public-source patent-landscape scan was performed on **2026-07-26** and is recorded here so a later reader can tell what was and was not checked.

- *Nearest neighbour.* Anthropic's portfolio, acquired via Adept AI Labs. **US20250299023A1** (filed 2024-03-20, pending): claim 1 covers generating **training data** through an intermediary that intercepts user actions and translates them into actuation commands — no mention of permission, authorization, consent, capability, revocation, or a display server/compositor. The sibling **US 12,430,150** covers a client-side interface-automation language at runtime. Neither reads on a capability-scoped display protocol; the overlap is the *setting* (an intermediary between an agent and a GUI), not the claimed mechanism.
- *Prior-art anchors that protect Vitrin's own mechanisms.* Trusted path and the attention key: **US 4,918,653**, "Trusted path mechanism for an operating system" (expired), and the TCSEC / Secure Attention Key lineage behind it. The unforgeable trust band: **Qubes OS**'s dom0-drawn coloured window borders (2010, published). The nested per-app compositor and per-app security context: **Sommelier** and Wayland **`security-context-v1`**, both published. Capability security itself: **Dennis & Van Horn (1966)**, KeyKOS, EROS, seL4, Capsicum.
- *Limits, stated plainly.* This was a **public-source landscape scan, not a freedom-to-operate opinion**. No FTO has been obtained and none is budgeted. Pending claims get amended and continuations get filed, so the Anthropic applications above are an explicit **re-check item before 1.0**, not a settled result.
- *Residual risk.* The realistic exposure is a non-practising entity in the RPA space (UiPath's portfolio; Microsoft via the Softomotive acquisition), and that risk materialises with **revenue**, not with publication — which is another reason publishing early costs nothing here.

### D-016 — D-005's weak-copyleft half executed as MPL-2.0
**Status:** accepted (2026-07-26) — executes D-005, closes the open question in issue #133
**Decision:** the reference implementation ships under **MPL-2.0**, not the LGPL-3.0 fallback D-005 recorded. The boundary between the copyleft and permissive halves is drawn by **derivation, not by directory**: content transliterated from the Apache-2.0 protocol artifacts stays Apache-2.0, original implementation expression goes MPL-2.0. Concretely — the trusted core and the IPC layer (`crates/vitrin-core`, `crates/vitrin-ipc`) and the shim's own C sources, hand-written headers and fixtures go MPL-2.0; `crates/vitrin-protocol` **including its checked-in generated code**, the generated C header under `shim/include/`, the IDL scanner and its `xtask` driver, the conformance instruments (`crates/vitrin-golden`, `crates/vitrin-mock-shim`, `fuzz/`), the SDKs, `tests/integration/` and `examples/` all stay Apache-2.0. `shim/wlcs/` remains the GPL-3.0-only carve-out it already was. Mechanism: per-file `SPDX-License-Identifier` headers plus a per-crate `license` field, with the root [`NOTICE`](../../NOTICE) as the normative path→license map — **not** per-directory LICENSE files.
**Rationale:** MPL-2.0 over LGPL-3.0 on four grounds, one of which is the real one.

- **Security is the core argument.** Vitrin's central claim is a *small, auditable* trusted core. Copyleft on the TCB makes that a property of the license rather than a promise in a README: a modified capability kernel, grant store or consent path cannot be shipped as a black box, and security-relevant fixes come back.
- **File-level scope is the right shape here.** MPL's copyleft attaches to files, not to the process image, so an app running inside a shim is never derivative and the Larger Work allowance (§3.3) makes linking MIT wlroots and MIT Smithay clean rather than an argument.
- **No patent regression.** MPL-2.0 §2.1(b) carries an explicit patent grant and §5.2 a termination-on-litigation clause, so moving a file from Apache-2.0 to the copyleft half does not lose the patent protection D-005 chose Apache-2.0 for (and D-015 leans on).
- **GPL compatibility is load-bearing, not theoretical.** MPL names GPL-3.0 a Secondary License (§1.12) and permits distributing the Larger Work under GPL terms (§3.3) — which is what keeps the advisory `shim/wlcs/` module lawful now that it compiles MPL-2.0 shim sources. **Hard constraint: Exhibit B ("Incompatible With Secondary Licenses") must never be added to any file in this repo.**
- **LGPL-3.0 rejected on deployment reality.** Its anti-tivoization / Installation Information terms (GPLv3 §6) are a known deterrent to embedded and kiosk vendors — and kiosk/embedded panels are a genuine deployment target for a display server, not a hypothetical one.

**Consequences:**

- **Adoption is unaffected by design:** nobody has to touch an MPL-2.0 file to write a client, build an alternate compositor, or ship an integration, because the protocol, the generated bindings, the codegen and the SDKs are Apache-2.0. Copyleft binds the people who modify the trusted core, and only them.
- The workspace-wide `license` default is **removed** rather than changed, so a crate added later cannot silently inherit the wrong half; every crate states its own.
- Root license texts + `NOTICE` + per-file headers, because per-directory LICENSE files would be *false*: `shim/` carries three licenses and `shim/include/` two.
- The repo has shipped a GPL-3.0-only file since P1.9.4 with no GPLv3 text anywhere in the tree, which GPL-3.0 §4 requires. Pre-existing gap; closed by this pass.
- `LICENSE-CC-BY-4.0` had been a 28-line human-readable *summary* that said of itself it was "not a substitute for the license" — so one of the four licenses the map names had no legal text in the tree. Replaced with the verbatim CC BY 4.0 legal code, matching what this pass did for MPL-2.0 and GPL-3.0-only. Another pre-existing gap, closed here rather than noted and left.
- **Inline SPDX headers cover sources only** — every first-party `.rs`, `.c`, `.h`, `.py`, `.sh` and `.js` file. Build manifests, the IDL and its schema, Markdown and fixture data deliberately carry none: `Cargo.toml` states the license in its own field, and the rest is covered by `NOTICE`'s path map, which now includes a catch-all clause so no tracked path is left unassigned. A file without an inline header is not an unlicensed file, and `NOTICE` says so explicitly rather than leaving a reader to infer it.
- The `license: 'MIT'` assertion in the shim's build file — and the MIT prose claims that grew around it — are retired. No MIT license text has ever existed in this repository, so that assertion was never backed by anything. `shim/wlcs/integration.c`'s `SPDX-License-Identifier: GPL-3.0-only` tag is preserved exactly; only the surrounding prose about "the rest of the repository" changes.
- Generated files take their SPDX line from the scanner's templates, never by hand, or `cargo xtask codegen --check` goes red in CI.
- **Known cost, accepted knowingly:** GitHub's license detector reads the root `LICENSE` and will keep labelling the repository "Apache-2.0", which now understates the copyleft half. The clean fix is the REUSE Specification layout plus a `reuse lint` CI check — which would also stop header coverage rotting — and is deliberately deferred to a follow-up rather than folded in here.
- Third-party carve-outs are untouched: the OFL-1.1 font vendored under `crates/vitrin-core/assets/` and the wlroots/wayland trees under `shim/subprojects/` keep their own licenses.
- Two placements are the arguable ones and are recorded as such so a future reversal is cheap: `crates/vitrin-ipc` (MPL-2.0, though its `client` feature is SDK-facing) and `crates/xtask` (Apache-2.0, though it also drives core dev tasks). Each is a one-field change that disturbs nothing else in the map.

### D-017 — Per-principal cursors: the model is decided and on the wire; delivery is deferred to M2
**Status:** accepted (2026-07-26) — closes Q15 / PRD §20.15, issue #147
**Decision:** five answers, one wire change.

1. **Cursor identity needs no new object.** Each principal has exactly one virtual pointer per realm it holds pointer authority over, and `vitrin_actuator_pointer` **is** that pointer's name on the wire. `move` already meant "this principal's virtual pointer"; the model was implicit and is now normative. No new interface, no cursor object, no cursor id.
2. **Cursorless is by construction, not by declaration.** A principal that never petitions for `actuate_pointer` has no cursor. The headless-fleet case therefore costs zero wire vocabulary. There is deliberately **no request** to declare, disown, or hide a cursor — and the missing *hide* is itself the decision: a visually distinct agent cursor is a human override (PRD P10), so visibility is never the actuating principal's own choice.
3. **Visibility is a relation, it is settled on the observation side, and it is asymmetric.** A captured frame contains no cursor except the human principal's, and that one only for a grant holding `observe_cursor` (4) — otherwise the same rule the consent overlay already obeys. The asymmetry is the decision, not an oversight: **agent→agent is closed outright** and purchasable at no verb set, ever (a side channel revealing what another principal is doing), while **agent→human is closed by default** and opens only through a verb. What is refused throughout is a *per-pair toggle matrix*: the only "toggle" that exists is the one grant verb, visible on a consent prompt and revocable with the grant. Human→agent visibility, including the per-agent toggles a human supervising thirty agents needs, is a shell and core concern: the human has no wire presence in v0.
4. **Agent→human cursor visibility is a grant verb**, not a display preference: new bit `observe_cursor` (0x8) on `vitrin_grant.verb`. It attaches to the grant, not to the view facet, because it is authority over what a capture *contains*, and it has no facet interface of its own — it widens `frame_ready`, it does not add a request. It is **meaningful only alongside `observe`**: a petition naming it without `observe` names no capture to widen and resolves `unsupported`, rather than granting a bit that changes nothing (the same rule that forbids granting a verb the deployment does not enforce). Seeing *another agent's* cursor is not purchasable by this or any verb.
5. **Cursors are core-composited.** A realm may never supply the pointer bitmap; `vitrin_shim_surface` has no cursor-surface role and will not gain one. A realm drawing its own could paint a **decoy cursor** and mislead the human about where input is going — the spoofing class the consent surface exists to exclude (issue #85). An app-painted pointer image is ordinary realm content, never the composited pointer.

**Why this simplifies the core rather than growing it:** `ConsentGrab` carries a defensive rule — never let emulated motion relocate the position the human's hit test reads — that exists *only* because one cursor is shared between origins. With structurally distinct pointers the human's hit test follows the human's pointer and the special case is deleted. It also clarifies preemption: with one pointer, "physical input preempts agent input" is a contention rule; with N+1 pointers there is nothing to contend for on the pointer axis, and preemption is purely about focus and actuation ordering.

**DEFERRED, explicitly:** **per-principal cursor *delivery*** — to **M2** (spec 1.0-candidate). Version 1 delivers **one shared pointer position** per realm view to the shim and composites **no cursor at all** (in nested operation the host desktop draws the human's cursor, outside the realm view entirely). Serving `observe_cursor` is likewise deferred: v1 refuses it `unsupported`. The correction is purely additive because the agent-facing half is already principal-relative — only `vitrin_shim_seat` delivery is shared, and it grows `since`-gated sibling events that name the principal, each still ending with `origin` so the schema's B2 rule holds.

**Costs, stated:**
- One verb bit (0x8) is burned on authority nothing implements. Enum values are immutable and `deprecated-since` marks but never removes, so if the model changes the bit is dead weight forever.
- A verb defined but unserved is a claim the core must keep honest at exactly one place; the guard is `grants::UNSERVED_VERB_BITS` (derived from `Verb::VALID_MASK`, so a newly appended verb is unserved by default — forgetting to classify one fails closed at *runtime*) plus the admission refusal and its unit test. Because that constant is derived, "served ∪ unserved = the wire bitfield" is an identity and cannot fail; the *tripwires* that actually go red when a verb is appended to the IDL are two explicit pins — `vitrin-protocol/tests/decode_errors.rs` on `Verb::VALID_MASK == 63`, and `consent/render.rs`'s catalogue test on the unserved set's exact bits, which forces a human to classify the new verb as served (with prompt copy) or unserved. Stated because the first draft of that doc comment claimed a tripwire the derivation had removed.
- **The Python SDK's hand-transcribed `Verb` mirror is out of sync as of this landing.** `sdk/python/src/vitrin_os/protocol.py` still carries only `observe`/`actuate.pointer`/`actuate.text` (mask 7 against the IDL's 63), and nothing fails: `IntFlag` accepts the undefined bits and no golden vector covers a verb bit. That file's own docstring says the IDL wins and this file must be fixed, so the divergence is a debt, not a design. It is `track:sdk`-owned and out of `track:protocol`'s file set; it is recorded here so it cannot be lost, and the dotted names to add are `observe.cursor`, `layout.arrange`, `layout.focus` (the existing convention maps `actuate_pointer` → `actuate.pointer`). Whoever closes it should add a parity check against `protocol/vitrin-v0.xml` rather than a second hand-transcription.
- The shared-cursor defect and its `ConsentGrab` workaround **survive through M1**. The security argument for per-principal cursors is therefore recorded, not yet realised, and the prose says so rather than implying otherwise.
- The human→agent toggle surface is unanswered here because the human is not on the wire; it lands with the mission-control shell (E3) and D-018's arrangement model.

### D-018 — Layout is a capability: grant-governed arrangement, core-enforced ordering
**Status:** accepted (2026-07-26) — closes Q16 / PRD §20.16, issue #149
**Decision:** five answers, two wire bits.

1. **Layout is grant-governed; the shell holds no ambient layout authority.** Layout verbs are attenuable, revocable, journaled verbs on a grant, exactly like `observe`/`actuate_pointer`/`actuate_text`. *"The shell is trusted"* was not available: PRD §5.1 makes it an invariant that window-management policy lives outside the core, so trusting the shell would move exactly the code that invariant exiles back into the TCB, defeating the Nitpicker/Scenic argument the design rests on.
2. **Core-enforced, unpurchasable ordering invariants** (no grant buys these at any verb set, ever): (a) the consent surface and the trust indicator composite above every principal's content; (b) the core's own hit test — never a client's claimed stacking — decides which surface an input event reaches; (c) no arrangement may occlude, fullscreen over, or resize away the consent surface; (d) no **agent** principal's cursor is composited into another principal's captured frame — the one cursor a capture may ever contain is the human's, and only for a grant holding `observe_cursor` (shared with D-017(3)/(4), and the single carve-out). The shell gets *arrangement*; the core keeps *ordering*.
3. **`focus` is a separate verb from placement.** New bits: `layout_arrange` (0x10) covering place/resize/raise/fullscreen, and `layout_focus` (0x20). Focus theft is at once the sharpest attack and the most legitimate need, so it must be attenuable alone. `raise`/`move`/`resize`/`fullscreen` are deliberately **not** split further: with ordering held by the core (2), each stops being an attack primitive, and splitting them would be attenuation theatre — more bits, no more safety.
4. **One layout-holder per output; no arbitration in the core.** Arbitration between two principals holding layout grants is window-management *policy*, which §5.1 forbids inside the TCB. At most one live grant carries `layout_arrange` per output. The refusal code a second holder receives is settled with the shell's design (E3); the additive mechanism is an appended `outcome` entry, since outcome is a plain enum and entries append. Version 1 refuses **every** layout petition `unsupported`, so the contention cannot arise yet.
5. **Epoch interaction (E2.3) is a binding constraint, not a sequencing note.** A layout change invalidates an agent's cached coordinates exactly as a repaint does, so **E2.3's epoch MUST be defined over (frame content + view geometry + stacking), not content alone**, and a layout mutation MUST bump it. Third-party arrangement is also the reason the epoch must be a compare-and-swap *on actuation*, not merely a token returned by observation: between an agent's capture and its click, a shell holding `layout_arrange` may have moved the target. The two are designed together, per Q16's own instruction.

**DEFERRED, explicitly:** **layout enforcement in its entirety** — to **E3** (mission-control shell), gated on M2's epoch work per (5). Version 1 has no window manager; both verbs resolve `unsupported` at petition admission. The layout **facet** (a `since`-gated `get_layout` structural mint on `vitrin_grant`, since `request_grant`'s five `new_id` arguments are frozen) is not added now — an unserved verb needs no facet.

**Costs, stated:**
- Two more immutable bits burned on unimplemented authority, with the same dead-weight risk as D-017's.
- The single-holder-per-output escape is a real restriction, not a neutral simplification: two shells, or a shell plus a tiling agent, cannot both arrange the same output. That is the price of keeping arbitration policy out of the core, and it is revisitable only by an additive design that puts arbitration *outside* the TCB.
- Choosing the refusal code for a second layout holder is left open. Naming one now would either widen an existing `outcome` entry's meaning (a wire-semantics change the growth rules forbid) or append an entry for a mechanism that does not exist.
- The consent surface is unspoofable today partly because nothing outside the core does layout at all. This decision does not make it *more* spoofable — v1 still serves no layout — but it does commit the project to invariants (2) as *invariants*, and their current standing is uneven: (a) holds because the core composites the consent prompt above the realm view; (b) and (d) hold vacuously, since no client can state a stacking order and no cursor is composited at all; (c) has nothing to be true of, because there is no arrangement mechanism. None of the four is tested *as an invariant* against a client trying to violate it, and none can be until something outside the core can arrange realms. That test is E3's, and D-018 is the reason it must exist.

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
| Q15 | Per-principal cursors + the cursor-visibility relation; absent from the IDL (`cursor` appears 0× in `protocol/vitrin-v0.xml`, and the core records "v0 shares one cursor between origins") | **protocol spec (E1)**; input routing (E2.x); mission-control shell (E3) | **CLOSED by [D-017](#d-017--per-principal-cursors-the-model-is-decided-and-on-the-wire-delivery-is-deferred-to-m2)** (2026-07-26): model decided, vocabulary landed before v0 freeze. **Residual, tracked there, not here:** per-principal cursor *delivery* and serving `observe_cursor` — M2 |
| Q16 | Is layout a capability? `raise`/`move`/`focus`/`fullscreen` are authority over the human's attention, and the consent surface is unspoofable today only because nothing outside the core does layout at all | **mission-control shell (E3)**; protocol spec (E1); epoch/CAS interaction (E2.3) | **CLOSED by [D-018](#d-018--layout-is-a-capability-grant-governed-arrangement-core-enforced-ordering)** (2026-07-26): posture, verbs, and the unpurchasable ordering invariants decided. **Residual, tracked there:** layout enforcement — E3; the epoch definition D-018(5) binds — E2.3; the second-holder refusal code — E3 |
