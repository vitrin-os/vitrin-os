# Phase 2 — Semantic + epochs

**Phase goal:** turn the pixel MVP into the *semantic, race-free, non-ambient* system — the differentiator phase. Agents act on versioned semantic trees under epoch/CAS; realms lose ambient filesystem and network authority.

**Consumes Phase-1 artifacts** ([01-phase-1-mvp.md](01-phase-1-mvp.md) §1): A1 protocol core, A2 spawn model, A3 grants+consent, A5 shim, plus backward requirements B1/B2 already honored in Phase 1.

**Phase exit = milestone M2** ([00-roadmap.md](00-roadmap.md)): agent drives Firefox by semantic tree + epoch/CAS; the ransomware demo (E2.6) and the `ssh localhost` demo (E2.7) run scripted; first benchmark numbers published.

**Internal dependency structure:** E2.1 → E2.2 → E2.3 is the semantic critical path; E2.4 and E2.5 hang off E2.2's node schema; E2.6/E2.7 are independent of the semantic chain (parallelizable — they need only A2/A3) and share a spawn-hardening sub-task; E2.8 is independent. **Q11** (realm vs. Unix-user boundary, [20-decision-log.md](20-decision-log.md)) is the phase's first decision gate — it shapes the E2.6/E2.7 namespace/UID layout and must be decided at phase start.

Epic template: Goal / Dependencies / Design decisions / Exit criteria.

---

## E2.1 — Semantic bridge (AccessKit / AT-SPI2)

- **Goal:** every shim surface carries a normalized, live semantic tree sourced from real apps' accessibility stacks, pushed (not pulled) into the core's scene node (PRD P4, Doc 2 §6.1).
- **Dependencies:** A5 (the bridge lives inside the Wayland shim); A1 (scene/surface objects); the node-schema decision below — which also gates E2.2/E2.4/E2.5, making it the phase's critical-path decision.
- **Design decisions:**
  - Node schema: adopt AccessKit's schema with extensions (recommended, given Newton/COSMIC gravity) vs. an independent superset. Record the PRD caveat that Newton's protocols are unfinalized: pin the schema by version and track it as a moving dependency (WS-A liaison table).
  - Private per-shim AT-SPI bus mechanics: an own session-bus instance inside the shim's namespace — never the session bus (closing the AT-SPI backdoor by construction). Interacts with E2.7: the bus socket must exist *inside* the realm's namespaces.
  - Normalization split: the shim normalizes to the schema; the core only stores/serves — parser code stays out of the TCB.
- **Exit criteria:** agent SDK `find(role, name)` works against live Firefox; a coverage matrix (checked cells, not prose) published for Firefox, Chromium/Electron, GTK4, Qt6; a test — not an assertion — proves no path from a realm to the host session a11y bus.

## E2.2 — Tree versioning, diffing, stable addressing

- **Goal:** atomic, epoch-stamped tree updates; KB-scale deltas on the wire; node IDs stable across redraws or explicitly invalidated (PRD Doc 2 §6.2).
- **Dependencies:** E2.1 (a tree to version); A1 wire protocol (new messages — feeds WS-A spec extraction).
- **Design decisions:** wire diff format (structural ops vs. per-node patches); stable-ID strategy under SPA-style full rebuilds → **Q2** (v0: best-effort re-identification by role+name+position fingerprint, explicit invalidation otherwise, honest degradation documented); full-tree resync triggers.
- **Exit criteria:** measured median delta size over a Firefox browsing session (target: KB-scale — the PRD's headline claim; this number goes straight into the M2 benchmark and the NLnet report); a node reference survives a page's dynamic updates or raises explicit invalidation, demonstrated in a test harness.

## E2.3 — Epoch / CAS action semantics

- **Goal:** observe returns an epoch; actions carry `expected_epoch`; the server rejects stale targets; `in_transition` handling for animated nodes (PRD P5, Doc 2 §7).
- **Dependencies:** E2.2 (tree epochs); A1 input router (rejection path).
- **Design decisions:** invalidation granularity → **Q1** — start from PRD §7's target-invalidating list, instrument false-reject/false-accept rates, tune empirically. The PRD flags epoch/CAS as "a design claim, not a proven result"; this epic is where the claim gets its test, and the measurement is part of the exit. Also: retry-hint semantics ("retry after epoch N"); whether frame epoch and tree epoch are one counter or a correlated pair (spec-relevant → WS-A); delegation-depth interim cap (**Q4**, depth = 1 until spec 1.0-candidate).
- **Exit criteria:** a harness demonstrating that a mutation between observe and act yields `StaleEpoch` (the WebDriver stale-element idea, generalized and enforced server-side); false-reject rate measured on an animation-heavy app against a stated threshold; the PRD Doc 2 §18 API sketch steps 4–5 run verbatim.

## E2.4 — VLM fallback pipeline

- **Goal:** an out-of-TCB service synthesizes trees for treeless surfaces (games, canvas, custom GUIs), cached and damage-invalidated, unified into the same node model (PRD Doc 2 §6.3).
- **Dependencies:** E2.2 node schema + epoch stamping; A1 capture path — the parser consumes the same observed frames agents do, no privileged tap.
- **Design decisions:** parser choice (OmniParser-class, pluggable); confidence surfacing → **Q3** (v0: per-node confidence attribute + tree-level `synthetic: true`, agents opt in); cache keying/invalidation on damage regions; deployment shape (sidecar service, never in-core — PRD §17).
- **Exit criteria:** a canvas-only surface yields an actionable synthetic tree through the *same* SDK calls; misclick behavior at low confidence characterized and documented (the honest-degradation posture of PRD §9).

## E2.5 — Native semantic demo app

- **Goal:** one application pushes trees natively (`scene.push_tree(surface, tree, epoch)`), proving the toolkit-backend path end to end (PRD §5.2; seeds the Phase-4 backends).
- **Dependencies:** E2.2/E2.3 (the native path must produce spec-conformant versioned trees); a native-protocol surface-commit extension.
- **Design decisions:** toolkit — egui or iced (Rust, days-scale embedder work, same language as the core) over Flutter (heavier embedder, claimed for Phase 4); stabilize only the minimum native protocol that makes the demo honest (`push_tree` + buffer-commit pairing).
- **Exit criteria:** an agent completes a form-fill task on the demo app with zero a11y-bridge involvement; the demo doubles as the spec's reference client for the native tree API (WS-A input).

## E2.6 — Filesystem powerbox v0

- **Goal:** realms spawn with an empty mount namespace + Landlock; the core-owned picker returns already-open fds over `SCM_RIGHTS`; subtree grants; basic (non-durable) standing grants (PRD P12, Doc 2 §12).
- **Dependencies:** A2 realm spawn (this epic extends it — closing Phase 1's documented D9 gap); A3 consent surface (the picker is core-rendered); **Q11 decided at epic start** (the realm-vs-Unix-user boundary shapes the namespace/UID layout).
- **Design decisions:**
  - Landlock ABI floor + degradation ladder for older kernels (PRD caveat: documented per tier, never silently weakened).
  - Realm private-storage layout.
  - Picker shape: core-drawn UI vs. a core-owned separate process with core-drawn chrome.
  - Consent-ladder subset: **only `once` / `while-running` rungs ship in v0**; durable rungs (`until-revoked`, `always`) are structurally blocked until provenance exists (E3.7) — stated explicitly so the ladder is never shipped ungated (**Q9** v0 posture; **Q13** first prompt-design review happens here).
- **Exit criteria:** **the ransomware demo** (PRD user story 6): a payload realm can write exactly its designated fds + realm storage, verified by an adversarial test attempting home-directory reach, path races, and picker spoofing; every designation journaled; the demo is scripted and reproducible (it is a WS-B/WS-C asset).

## E2.7 — Network authority v0

- **Goal:** per-realm loopback-only network namespace plus own PID/IPC/UID (completing the container-per-realm baseline, PRD Doc 2 §4.5); egress as a designated host:port-scoped, journaled grant via a mediating proxy (PRD P13).
- **Dependencies:** A2 spawn; A3 grant table/consent; Q11 (shared with E2.6 — the two epics share a spawn-hardening sub-task and are planned as siblings).
- **Design decisions:**
  - Egress-proxy mechanism: per-realm proxy socket injected into the netns (recommended for v0 — simpler, no routing in the TCB) vs. veth + transparent redirect.
  - DNS mediation: resolve in the proxy, grants are host-name-scoped, resolved addresses pinned.
  - Egress rows in the grant schema.
  - **Q12** v0 posture: no blanket grants; browser-realm ergonomics deferred behind an interim per-realm template allowlist; the full answer is a decide-by-M3 item.
- **Exit criteria:** **the `ssh localhost` demo** (PRD §1.8, §15 threat row): inside a realm, host loopback unreachable, abstract sockets confined, path sockets absent — an adversarial test suite, not prose; one designated egress (host:443) works, expires, and revokes immediately; a realm with no grant emits zero outbound packets (verified by capture).

## E2.8 — IME workstream (begins)

- **Goal:** land PRD Doc 2 §14's strategy as running code where cheap and documented plan where not: the agent `text` actuator (Unicode-direct, IME-bypassing) ships; the human IME path works for one reference combination.
- **Dependencies:** A5 shim seat model; core surface layer (candidate popups as core-owned surfaces).
- **Design decisions:** fcitx5-first (maintained, Wayland-native), IBus as documented fallback; candidate-window routing (core-owned surface positioned by the core, immune to nesting offsets); scope discipline — this is a known tarpit (PRD §9), so the epic carries an explicit **effort cap**: everything beyond the reference combination is a compatibility-matrix entry, not a commitment.
- **Exit criteria:** agent text entry into a CJK-locale app works with no IME involved; a human types Japanese into Firefox-in-realm via fcitx5 with correctly positioned candidates; the XWayland-IME fallback strategy is documented (consumed by E3.2).
