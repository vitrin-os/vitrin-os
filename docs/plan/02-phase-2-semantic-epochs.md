# Phase 2 — Semantic + epochs

**Phase goal:** turn the pixel MVP into the *semantic, race-free, non-ambient* system — the differentiator phase. Agents act on versioned semantic trees under epoch/CAS; realms lose ambient filesystem and network authority.

**Consumes Phase-1 artifacts** ([01-phase-1-mvp.md](01-phase-1-mvp.md) §1): A1 protocol core, A2 spawn model, A3 grants+consent, A5 shim, plus backward requirements B1/B2 already honored in Phase 1.

**Phase exit = milestone M2** ([00-roadmap.md](00-roadmap.md)): agent drives Firefox by semantic tree + epoch/CAS; the ransomware demo (E2.6) and the `ssh localhost` demo (E2.7) run scripted; first benchmark numbers published.

**Internal dependency structure:** E2.1 → E2.2 → E2.3 is the semantic critical path; E2.4 and E2.5 hang off E2.1's node schema; E2.6/E2.7 are independent of the semantic chain and share a spawn-hardening sub-task; E2.8 is independent. **Q11** (realm vs. Unix-user boundary, [20-decision-log.md](20-decision-log.md)) is the phase's first decision gate — it shapes the E2.6/E2.7 namespace/UID layout and must be decided at phase start. §3 below records one inversion the roadmap does not anticipate: the confinement track should run **first**, not merely early.

Epic template: Goal / Dependencies / Design decisions / Exit criteria, then a task table.

---

## 1. Exported-artifact contract (what Phase 3 may depend on)

Phase 3 cites these as its Phase-2 dependencies, exactly as Phase 1 exported A1–A6; anything not listed here is an internal detail free to change. Each artifact travels **with its limitations** — an exported capability whose caveat is left in the epic that built it is an artifact Phase 3 will over-read.

- **C1 — Semantic node schema and the tree wire format.** The AccessKit-derived node model pinned to one exact crate version with a reserved Vitrin extension namespace; `node_role`/`node_state`/`tree_flags` enums; the length-prefixed, canonically-ordered serialization carried over an fd; `vitrin_shim_surface.push_tree` (double-buffered, applied at `commit`) and `vitrin_view.observe_tree`/`tree_ready`; **protocol version 2**, with the core implementing versions 1 and 2 simultaneously. Includes the role/state mapping table, the pin-drift check, and the golden wire vectors holding `crates/vitrin-protocol` and the Python SDK to the same bytes (D8). Produced by P2.1.1/P2.1.2; consumed by E3.2, E3.4 (B1), E3.5. **Not exported:** the AT-SPI vocabulary, the D-Bus types and the collector's normalization tables stay inside the shim by the epic's normalization split — Phase 3 must not reach for them.
- **C2 — Core tree store and per-observer delta service.** `crates/vitrin-core/src/semantic/`: one canonical tree per surface with a **source tag** (`a11y` / `native` / `synthetic`) answerable in the journal and invisible in the agent's API; exactly one write path from the shim side and one from the principal side; served through the existing single enforcement chokepoint, so revocation, expiry, rate ceilings and `consent_held` apply to trees with zero new authority code; a bounded retained history (stated version count **and** stated byte budget) with normative resync triggers; always a copy into a fresh sealed memfd. Produced by P2.1.6/P2.2.2/P2.2.4; consumed by E3.2, E3.3, E3.4. **Guarantee Phase 3 may rely on:** a client cannot grow the core by never observing.
- **C3 — Epoch / CAS action semantics.** One monotonic counter per view, defined by D-018(5) over (frame content + view geometry + stacking), never reused within a session; per-node epochs derived from it, not a second clock; `expected_epoch` on node-targeted actuation; the `stale` refusal with `retry_after_epoch`; `in_transition` as a node state; the comparison at exactly one grep-provable site, ordered **authority first, then staleness**, so a stale actuation from a revoked grant reports `revoked` and never leaks that the node changed. Produced by P2.2.3/P2.3.1–P2.3.3; consumed by E3, E3.4, E3.7. **Exported with its measurement:** the false-reject rate and the zero-false-accept count from P2.3.7, without which D-014 forbids freezing the mechanism in the spec.
- **C4 — Confined realm spawn.** The `vitrin-realm-init` helper and its namespace set (user + mount + PID + IPC + UTS + net), the uid_map/gid_map/setgroups sync-pipe protocol, the realm's mount table with a **reserved fixed in-realm path for the private accessibility bus socket**, the Landlock ruleset applied before `execve` (inherited by every descendant including the app the shim forks), the seccomp deny-list with one comment per row naming the escape class it closes, and the runtime isolation preflight with its refuse-to-start floor and generated per-kernel tier matrix. Produced by P2.6.1–P2.6.4 and P2.7.1; consumed by E3.2, E3.3, E3.6. **Two limitations Phase 3 must not over-read:** the per-UID isolation tier needs install-time provisioning (subuid ranges plus a `newuidmap`-class helper) that no packaging exists for, so E3.3 must own that packaging or the fleet ships on the weaker intra-user tier; and only the microVM tier escapes shared-kernel escape classes (D-010).
- **C5 — Powerbox and egress: vocabulary, mediation, and the fd/socket delivery path.** Verb bits `designate_file` and `egress`; resource prefixes `file:`, `dir:` and `net:` with a wildcard-free `host:port` grammar; the `vitrin_powerbox` facet minted structurally on `vitrin_grant`; the core-drawn picker on the existing consent stack with `openat2 RESOLVE_NO_SYMLINKS` resolution from a directory fd; `SCM_RIGHTS` delivery and the shim's per-realm relay; the out-of-core egress proxy that asks the enforcement chokepoint per connection and holds no grant of its own; DNS resolved only in the proxy with addresses pinned into the grant row. Produced by P2.6.5–P2.6.7 and P2.7.2–P2.7.4; consumed by E3.5, E3.6, E3.7. **The one limitation that will be misread if unstated:** a delivered fd is kernel authority the core cannot recall, so PRD P2's "revocation is immediate and transitive" is **false for designations already made** — revocation stops future designations and kills the grant row; the payload keeps the fd until the realm dies. E3.7 must know this before designing durable designation grants, which multiply exactly this residue.
- **C6 — The native tree path and its reference client.** `protocol/vitrin-semantic-v1.xml` (wayland-scanner dialect, **never** validated against `vitrin-v0.rng`; protocol-derived, so `NOTICE` places it under Apache-2.0 per D-016), the shim-side implementation relaying a native tree verbatim while normalizing an a11y-sourced one into the same bytes, the conformance page, and `shim/tests/native_tree_minimal.c` — a client written **only** from that page, sharing no code with `examples/native-demo/`, whose existence is the evidence the page is sufficient. Produced by P2.5.1/P2.5.2/P2.5.6; consumed by Phase 4's toolkit backends and WS-A's freeze ladder. **The load-bearing property** is byte-equality of the two producers' upstream output — that is what makes "one node model" a test result rather than a claim.
- **C7 — IME plumbing and the compatibility matrix.** `vitrin_shim_text_input` (a 1:1 mirror of `text-input-v3`, so the shim relays rather than translates), `zwp_text_input_v3` toward the app, the core's separate IME socket with its one-connection-per-session rule and its suspension under `ConsentGrab`, the candidate-popup positioning path with the trusted-band clip, and the **generated** compatibility matrix whose committed-cell count CI asserts. Produced by P2.8.1–P2.8.6; consumed by E3.2, whose XWayland-IME fallback page this epic's exit criterion explicitly feeds. **Exported with a trust statement, not a capability statement:** the IME sees every keystroke destined for the focused realm. It is a keylogger by construction and a genuine extension of the trust boundary beyond the TCB; it belongs in PRD Doc 2 §15 and in `docs/book/src/limits.md`, and no capability in this design constrains what it does with what it sees.
- **C8 — Sub-realm authority granularity (`node:`).** The `ResourceRef` variants and subtree containment in `covers`, pixel **redaction** (not cropping — the agent receives a full-size frame with everything outside the granted subtree replaced by a constant, so its coordinate space is unchanged and geometry leaks nothing), actuation clipping at the chokepoint, and `observe_tree` scoped to the granted subtree. Produced by P2.2.6, closing #161; consumed by E3.7 and E3.3. **Exported with the risk it creates, which is new in Phase 2 and must travel with it:** once a node id names a grant's resource, a wrong re-identification is not an ergonomics bug but an **authority redirection** — a grant scoped to the `name` field silently covering the `card number` field. That is why identity is core-owned and never shim-supplied, why ambiguous matches fail closed by invalidating rather than picking, and why P2.2.8 asserts the false-identification count as an exact zero rather than as a rate.

---

## 2. Epics and tasks

Task rows: ID · task · key decisions · dependencies · acceptance criteria · owning track. **★ marks a mock-free integration gate** under the definition-of-done rule (§4). Rows are the plan-tree granularity; the full acceptance detail lives in each task's tracking issue.

Task IDs are `P2.<epic>.<task>` — `P2.6.1` is E2.6's first task. Every task carries exactly one `track:*`, naming the **owner of the deliverable**, not the diff's footprint.

### E2.1 — Semantic bridge (AccessKit / AT-SPI2)

- **Goal:** every shim surface carries a normalized, live semantic tree sourced from real apps' accessibility stacks, pushed (not pulled) into the core's scene node (PRD P4, Doc 2 §6.1).
- **Dependencies:** A5 (the bridge lives inside the Wayland shim); A1 (scene/surface objects); the node-schema decision below — which also gates E2.2/E2.4/E2.5, making it the phase's critical-path decision.
- **Design decisions:**
  - Node schema: adopt AccessKit's schema with extensions (recommended, given Newton/COSMIC gravity) vs. an independent superset. Record the PRD caveat that Newton's protocols are unfinalized: pin the schema by version and track it as a moving dependency (WS-A liaison table).
  - Private per-shim AT-SPI bus mechanics: an own session-bus instance inside the shim's namespace — never the session bus (closing the AT-SPI backdoor by construction). Interacts with E2.7: the bus socket must exist *inside* the realm's namespaces.
  - Normalization split: the shim normalizes to the schema; the core only stores/serves — parser code stays out of the TCB.
- **Exit criteria:** agent SDK `find(role, name)` works against live Firefox; a coverage matrix (checked cells, not prose) published for Firefox, Chromium/Electron, GTK4, Qt6; a test — not an assertion — proves no path from a realm to the host session a11y bus.

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.1.1** | Decide and pin the semantic node schema: adopt AccessKit's node model as the normative baseline pinned to one exact crate version, add a reserved Vitrin extension namespace, and land it as a new normative prose page docs/protocol/12-semantic-nodes.md plus node_role/node_state enums in protocol/vitrin-v0.xml, with the pin recorded as a moving… | AccessKit schema with extensions over an independent superset, on Newton/COSMIC gravity, with the PRD Doc 2 §6.1 dependency-risk caveat restated on the prose page rather than paraphrased away. AccessKit is the schema, AT-SPI2 is the Phase-2 source: named explicitly so nobody reads 'AccessKit bridge' as a transport claim. | A1 | A checked-in machine-readable mapping table maps every role and every state of the pinned AccessKit version to exactly one Vitrin role/state or to an explicitly enumerated unmapped list — a CI lint fails if any AccessKit entry is in neither set. | `protocol` |
| **P2.1.2** | Add the tree transport to the wire as paired IDL+prose: shim-facing `vitrin_shim_surface.push_tree(fd, size, version, format)` and principal-facing `vitrin_view.observe_tree(since_version)` → `vitrin_view.tree_ready(fd, size, version, flags)`, all since="2", and bump protocol/vitrin-v0.xml's root @version from 1 to 2. | `push_tree` is **double-buffered surface state applied at `commit`**, beside `attach`/`damage` — E2.5's shape, adopted here so a tree can never describe pixels that are not on screen. | P2.1.1 | `xmllint --relaxng` passes; `cargo xtask codegen --check` idempotent and green; the scanner's append-only opcode lint passes (messages appended, requests and events numbered separately). Version-matrix test, four cells, all against the shipped binary: a v1 client handshakes and completes a capture unchanged | `protocol` |
| **P2.1.3** | Give each shim its own accessibility bus: spawn a private dbus-daemon into the shim's runtime dir from shim/src/main.c, export AT_SPI_BUS_ADDRESS/DBUS_SESSION_BUS_ADDRESS pointing only at it when the shim execs the app, register the shim as that bus's accessibility registry, and remove GTK_A11Y=none / NO_AT_BRIDGE=1 from the environment… | An own bus instance inside the shim's runtime dir, never the host session bus — this is what closes the AT-SPI backdoor (PRD §1.3) by construction rather than by policy. The core injects no DBUS_SESSION_BUS_ADDRESS today (crates/vitrin-core/src/spawn.rs module docs say so explicitly); that stays true | P2.6.2, P2.6.1, P2.1.2 | Component test in shim/tests/acceptance/: the shim starts, the private bus socket exists at mode 0700 inside the shim's runtime dir, and a probe execed with the app's exact composed environment reaches that bus and only that bus. | `c-shim` |
| **P2.1.4** | Build the in-shim AT-SPI2 tree collector (new shim/src/a11y.c): walk the app's accessible tree at surface map over the private bus, subscribe to object:children-changed / object:state-changed / object:text-changed / object:property-change, and maintain a live mirror keyed by AT-SPI accessible path. | Push, not poll, following Newton's model (PRD Doc 2 §6.2) — the collector reacts to signals and never round-trips the bus on an agent's observe, so agent latency is not a function of D-Bus round trips. Signal storms are coalesced into one push per shim dispatch round, on the same argument that bounds compositor composites in D-019. | P2.1.3 | Convergence test against a repo-authored fixture app under shim/tests/ that mutates its own accessible tree on command: after each of >=200 scripted mutations spanning all four subscribed signal classes, the mirror is compared against a fresh full walk of the same bus | `c-shim` |
| **P2.1.5** | Normalize and push: map AT-SPI roles/states/attributes onto the P2.1.1 schema inside the shim, serialize the tree into a memfd, and emit `vitrin_shim_surface.push_tree` alongside the surface's buffer commit in shim/src/upstream.c. | The shim normalizes and the core only stores/serves — no AT-SPI vocabulary, no D-Bus types and no parser code cross the TCB boundary, which is the epic's stated normalization split and R7 (TCB dependency creep) carried into Phase 2. | P2.1.2, P2.1.4 | Golden normalization fixtures: recorded AT-SPI dumps from real Firefox ESR, Chromium, a GTK4 app and a Qt6 app are checked in under tests/golden/ and normalize byte-identically to checked-in expected trees, regenerated only via `cargo xtask bless`. | `c-shim` |
| **P2.1.6** | Store and serve in the core: a new crates/vitrin-core/src/semantic/ module holding one canonical tree per surface, populated by `push_tree` in shim.rs and served through `observe_tree`/`tree_ready` behind the existing single enforcement chokepoint in enforcement.rs, copied into a fresh sealed memfd exactly as capture.rs does for pixels. | The core stores and serves and does nothing else to the tree in E2.1 — no parsing, no interpretation, no role logic; structural work arrives in E2.2. Serving a tree is an `observe`-verb use through the same chokepoint as `capture_frame`, so revocation, expiry, rate ceilings and consent_held apply to trees with zero new authority code, and P1.4.4's… | P2.1.2, P2.1.5 | Component test with vitrin-mock-shim pushing a synthetic tree: `observe_tree` under a live grant returns it byte-identically; under a revoked grant returns `refused(observe, revoked)` and never a stale tree; under a rate-limited grant returns `refused(observe, rate_limited)` with retry_after_ms > 0. | `rust-core` |
| **P2.1.7** | Land `find(role, name)` in the Python SDK: a new `sdk/python/src/vitrin_os/tree.py` deserializing the P2.1.1 format independently of `crates/vitrin-protocol` (D8), exposing `obs.tree`, `find`/`find_all` with fail-closed ambiguity, and carrying the `accept_synthetic` option defaulted off so E2.4 populates a slot rather than flipping a default. | The SDK deserializes independently of crates/vitrin-protocol, per D8 — two independent implementations of the wire is the mechanism that keeps the format honest, and P2.1.2's golden vectors are what pins them together. `find` returning nothing is a return value, not an exception | P2.1.6 | Unit tests against the P2.1.5 fixtures cover role-only, name-only, role+name, nth-match, no-match and unknown-role lookups. The Verb parity check fails when protocol/vitrin-v0.xml gains a verb bit the SDK does not mirror — demonstrated by adding a bit in the test and watching it go red. | `sdk` |
| **P2.1.8** | Build the semantic coverage matrix as generated output, not prose: a harness (tests/integration/test_semantic_coverage.py) that boots Firefox ESR, Chromium/Electron, a GTK4 app and a Qt6 app each in a realm and emits a checked-cell table into docs/book/src/semantic-coverage.md. | Cells are measured, never asserted: each (app x capability) cell carries a number or an explicit 'not tested', on a fixed capability axis — tree-present, roles-mapped fraction, names-present fraction, actionable-nodes-present, live-update latency. | P2.1.5, P2.1.11 | The published page is byte-identical to the harness's output (CI regenerates and diffs; any drift fails). Every cell is either a measured number with its measurement definition or the literal string 'not tested' with a reason — a lint rejects free prose in a cell. | `ci-docs` |
| **P2.1.9** ★ | The epic's semantic gate: tests/integration/test_real_semantic_firefox.py drives the shipped `vitrind` binary + real `vitrin-shim` + real Firefox ESR + the real Python SDK over a real socket, calls `find(role, name)` for a named element, and proves the returned node is that element by actuating at its geometry and reading the app's own visible… | `find` returning a node is not evidence that the node is the right one — the P1.9.8 lesson (a metric's name is not evidence about the metric) applied to semantics. The gate therefore closes the loop through pixels: locate by tree, act at the node's geometry, require a visible response only that element produces. | P2.1.7, P2.1.8 | Green against real software on every seam it claims: shipped `vitrind` (not an in-process runtime), real `vitrin-shim` booted with an explicit `shim=str(self.shim_bin)` path (never the bare `Core()` that defaults to vitrin-mock-shim), real Firefox ESR at the pin, real dbus-daemon on the private bus, real Python SDK over a real Unix socket. | `sdk` |
| **P2.1.10** ★ | Prove the AT-SPI backdoor is closed, adversarially: tests/integration/test_real_a11y_isolation.py runs a hostile probe as the realm's app against the shipped binaries and attempts every route from inside the realm to the host session's accessibility bus. | This is the epic's security exit criterion and it is a test, not an assertion — the epic says so in those words. Routes probed: the composed environment, /proc/self/environ, the well-known XDG_RUNTIME_DIR session-bus path, the abstract-socket namespace, org.a11y.Bus activation on any reachable bus, and the host's a11y registry name on the private bus. | P2.1.3, P2.1.9 | The probe reports zero successful reaches on every enumerated route, and the test fails if the route list shrinks (route count pinned, so deleting a probe cannot make the gate pass). | `c-shim` |
| **P2.1.11** | Build and pin the coverage matrix's missing fixtures: a GTK4 probe (`shim/meson.build` has `gtk4` optional and `gtk_entry_probe.c` is GTK3 by design), a Qt6 fixture (none exists), and a Chromium/Electron pin with the accessibility-enable flag it needs. | The epic's exit criterion asks for a matrix published *for* four toolkits; a matrix reading "not tested" in half its columns satisfies a lint but not the criterion. Fixtures are pinned by version like the Firefox ESR pin, so a matrix cell names software a reader can reproduce. | — | Each of GTK4, Qt6 and Chromium/Electron builds or resolves in CI and reaches a mapped, non-empty tree through the real shim; every matrix column has a pinned version string; a fixture that fails to build fails the matrix generation rather than silently emitting "not tested". | `c-shim` |

### E2.2 — Tree versioning, diffing, stable addressing

- **Goal:** atomic, epoch-stamped tree updates; KB-scale deltas on the wire; node IDs stable across redraws or explicitly invalidated (PRD Doc 2 §6.2).
- **Dependencies:** E2.1 (a tree to version); A1 wire protocol (new messages — feeds WS-A spec extraction).
- **Design decisions:** wire diff format (structural ops vs. per-node patches); stable-ID strategy under SPA-style full rebuilds → **Q2** (v0: best-effort re-identification by role+name+position fingerprint, explicit invalidation otherwise, honest degradation documented); full-tree resync triggers.
- **Exit criteria:** measured median delta size over a Firefox browsing session (target: KB-scale — the PRD's headline claim; this number goes straight into the M2 benchmark and the NLnet report); a node reference survives a page's dynamic updates or raises explicit invalidation, demonstrated in a test harness.

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.2.1** | Give Q2 its v0 posture in code: core-owned node identity in crates/vitrin-core/src/semantic/identity.rs — prefer the source's own stable id where the toolkit provides one, fall back to a role+name+structural-position fingerprint for re-identification across a full rebuild, and emit explicit invalidation when neither resolves. | Q2's v0 answer as the epic prescribes: best-effort fingerprint re-identification, explicit invalidation otherwise, honest degradation documented as a tier table. Node ids are assigned and owned by the CORE, not the shim, even though the shim holds the source ids | P2.1.6 | Measured against a checked-in corpus of >=20 recorded mutation traces spanning three classes (in-place text update, sibling insert/remove, full SPA re-render), captured from real pages and replayed offline: report per-class re-identification rate and, separately, false-identification count. False-identification count must be 0 across the whole corpus | `rust-core` |
| **P2.2.2** | Specify the wire diff format as paired IDL+prose: a hybrid of structural ops (insert / remove / reparent / reorder) and per-node property patches, carried in the same fd payload as a full tree and distinguished by a `full_tree` bit in the `tree_ready` flags field reserved at P2.1.2, with the full-resync trigger conditions written normatively. | Hybrid over pure per-node patches: an SPA re-render is a structural event, and expressing it as N property patches is exactly how a KB-scale claim becomes an MB-scale reality. Resync triggers are normative and enumerated, not heuristic | P2.2.1 | Property test over fuzzed tree pairs: apply(diff(old, new), old) == new, 10k cases, plus an adversarial set of hand-built pathological pairs (whole-tree reparent, id-reuse attempt, cycle attempt) that must be rejected or resynced rather than mis-applied. `xmllint --relaxng` green and protocol/test-mutations.sh extended. | `protocol` |
| **P2.2.3** | Land the epoch counter and atomic tree swap in crates/vitrin-core/src/semantic/epoch.rs: one monotonic counter per view, bumped on tree push, on view geometry change and on stacking change, with an observer structurally unable to read a half-applied tree. | ONE counter, not a correlated frame/tree pair — the epic lists that choice as open and D-018(5) closes it: the epoch MUST be defined over (frame content + view geometry + stacking), so a single per-view counter is the only shape that satisfies it, and a separate tree epoch would let a layout move invalidate coordinates without advancing the number an agent… | P2.2.1 | Interleaving test: 10k randomized interleavings of push and observe assert every observed tree is internally consistent (every referenced parent exists, no dangling child) and every observer's epochs are strictly non-decreasing. | `rust-core` |
| **P2.2.4** | Compute per-observer deltas in the core: retain a bounded history of tree versions per view, serve `observe_tree(since_version)` as the delta from that observer's last-served version, and fall back to a full tree with the `full_tree` flag when P2.2.2's resync triggers fire. | Per-observer, because two agents at different versions need different answers and the core is the only party that knows both. Retained history is bounded by a stated version count and a stated byte budget, and exceeding either forces a resync rather than growing | P2.2.2, P2.2.3 | Unit tests: a `since_version` older than the retained window returns `full_tree`; equal to current returns an empty delta, not a full tree; from the future is fatal `invalid_argument` (a client cannot have seen a version the server never emitted). | `rust-core` |
| **P2.2.5** | Make the SDK's tree incremental: a version-tracking cache in sdk/python/src/vitrin_os/tree.py that applies deltas, handles `full_tree` resync, and raises a new typed `NodeInvalidated` from sdk/python/src/vitrin_os/errors.py when a held node reference no longer resolves. | A stale node reference raises, it never silently resolves — an SDK that quietly re-binds node #42 to whatever now sits at that position is the exact failure the stable-addressing exit criterion forbids, and it would be invisible to every test that only checks that a call succeeded. | P2.2.4 | The SDK replays P2.2.2's golden delta vectors and its reconstructed tree hashes equal the core's canonical tree hash at every step. A held node whose element was removed raises `NodeInvalidated` on next use; a held node whose element merely moved resolves and reports its new geometry. | `sdk` |
| **P2.2.6** | Serve the `node:` resource prefix: add the `ResourceRef` variants crates/vitrin-core/src/grants.rs already reserves in its own doc comment, implement subtree containment in `ResourceRef::covers`, redact pixels outside the granted subtree's bounds on `observe`, clip actuation outside it at the enforcement chokepoint, and scope `observe_tree` to the… | Closes #161 with the semantic naming its own body defers to Phase 2 ('build the chokepoint's sub-surface scoping against geometry now; Phase 2 swaps the naming to node: without touching enforcement') | P2.2.1, P2.2.4 | Adversarial suite: under a grant naming `node:<id>`, every pixel outside the subtree's bounds equals the redaction constant (per-pixel assertion, not sampled); an actuation outside the subtree refuses `not_granted` and delivers nothing to the app, verified by the app's own receipt rather than the core's log | `rust-core` |
| **P2.2.7** ★ | Measure the headline number: tests/integration/test_real_semantic_delta_size.py drives shipped `vitrind` + real `vitrin-shim` + real Firefox ESR through a scripted browsing session over a pinned, checked-in, offline page corpus and publishes the delta-size distribution to docs/benchmarks/semantic-deltas.md. | The session is a checked-in offline corpus served from a local static server, never the live web, because a number that cannot be reproduced in two years is not a benchmark — and this number goes into the NLnet report and the M2 benchmark set. Reported quantities: p50, p90 and max delta bytes; the full-tree serialization baseline for the same session | P2.1.5, P2.2.5 | Median (p50) delta <= 8 KB — single-digit KB is what the PRD's 'KB-scale deltas, not MB screenshots' claim means, and the task fails if it is not met rather than reporting a larger number as a success. Full distribution and the full-tree baseline published, with the ratio reported. | `ci-docs` |
| **P2.2.8** ★ | The epic's correctness gate: tests/integration/test_real_node_stability.py holds a node reference across each of the three mutation classes against shipped `vitrind` + real `vitrin-shim` + real Firefox ESR and requires that the reference either resolves to the same element or raises `NodeInvalidated`. | Ground truth comes from the app, not from the core: the corpus pages render a receipt band naming which element last received an event (the form-target technique from P1.9.8), so 'the reference still points at the same element' is proved by acting through it and reading the app's own answer rather than by the core agreeing with itself. | P2.2.7 | Zero silent mis-bindings across the whole corpus — a single one fails the gate. Per-class re-identification rates reported and matching P2.2.1's offline numbers within a stated tolerance (a large divergence means the offline corpus is unrepresentative, which is itself a finding). | `rust-core` |

### E2.3 — Epoch / CAS action semantics

- **Goal:** observe returns an epoch; actions carry `expected_epoch`; the server rejects stale targets; `in_transition` handling for animated nodes (PRD P5, Doc 2 §7).
- **Dependencies:** E2.2 (tree epochs); A1 input router (rejection path).
- **Design decisions:** invalidation granularity → **Q1** — start from PRD §7's target-invalidating list, instrument false-reject/false-accept rates, tune empirically. The PRD flags epoch/CAS as "a design claim, not a proven result"; this epic is where the claim gets its test, and the measurement is part of the exit. Also: retry-hint semantics ("retry after epoch N"); whether frame epoch and tree epoch are one counter or a correlated pair (spec-relevant → WS-A); delegation-depth interim cap (**Q4**, depth = 1 until spec 1.0-candidate).
- **Exit criteria:** a harness demonstrating that a mutation between observe and act yields `StaleEpoch` (the WebDriver stale-element idea, generalized and enforced server-side); false-reject rate measured on an animation-heavy app against a stated threshold; the PRD Doc 2 §18 API sketch steps 4–5 run verbatim.

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.3.1** | Give Q1 its v0 posture in code: derive a per-node epoch in crates/vitrin-core/src/semantic/epoch.rs from PRD Doc 2 §7's target-invalidating list — the target's geometry, role, enabled/visible state, text content for a text target, and its removal or reparenting | Q1's starting policy is PRD 7's list taken literally, not a guess, and the counters exist because the epic's instruction is to tune empirically before spec 1.0-candidate — a policy with no instrumentation cannot be tuned, only re-guessed. | P2.2.3 | Table-driven test enumerating every class named in PRD Doc 2 §7 with a constructed mutation for each, asserting bump for the five invalidating classes and no-bump for the three non-invalidating ones — eight cells, none skipped. A layout-change case asserts a global bump. | `rust-core` |
| **P2.3.2** | Put CAS on the wire as paired IDL+prose: since="2" sibling requests `vitrin_actuator_pointer.click_node(node_id, expected_epoch)` and `vitrin_actuator_text.type_into(node_id, text, expected_epoch)`, plus the since="2" `vitrin_grant.stale(verb, node_id, current_epoch, retry_after_epoch)` event that 00-conventions.md Appendix A already names as the… | Siblings, never changed signatures — `move`/`button`/`scroll`/`type` are immutable forever (7.4), and Appendix A already commits to this exact shape for both halves ('drag intents' for pointer siblings, 'epoch-staleness refusal sibling' for the event, with the stated reason that `retry_after_ms` cannot express an epoch). | P2.3.1, P2.2.2 | `xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml` passes; protocol/test-mutations.sh gains negative cases for the new messages; the append-only opcode lint passes and `cargo xtask codegen --check` is idempotent. A test asserts a stale actuation leaves the connection ALIVE and delivers `stale` | `protocol` |
| **P2.3.3** | Enforce CAS at the single chokepoint: extend crates/vitrin-core/src/enforcement.rs so every node-targeted actuation compares `expected_epoch` against the target node's current epoch before anything is acted on, refusing with `stale` and never partially applying. | One site, grep-provable, exactly as P1.4.4 established — a second epoch-check location anywhere else is the defect this task exists to prevent, so the grep proof is part of acceptance rather than a code-review habit. | P2.3.2 | Deterministic race harness: a mutation injected between observe and act on 1000 iterations yields `stale` every time — zero false accepts, which is the property, not a rate. `rg` proves exactly one epoch-comparison site in crates/vitrin-core, in the same style as the existing single-chokepoint proof. | `rust-core` |
| **P2.3.4** | Handle animated nodes: propagate `in_transition` from the collector through to the core and refuse actuation on a transitioning node with `stale` carrying a `retry_after_epoch` naming the epoch at which the core expects it to settle. | v0 REFUSES rather than waits. PRD Doc 2 §7 offers both ('wait for settle or are rejected with a retry hint'); waiting means holding an actuation inside the TCB for an unbounded interval on the compositor loop, which is the posture P1.2.3 forbade for slow readers | P2.3.3, P2.1.4 | Against a checked-in CSS-animation page in real Firefox: actuation on an animating node refuses with `stale` carrying a `retry_after_epoch`, and retrying at that epoch succeeds — measured over >=100 trials with the success-on-first-retry rate reported, not merely demonstrated once. Settle-latency distribution (p50/p99) published. | `rust-core` |
| **P2.3.5** | Close Q4: fix the grant delegation-chain depth at 1 as a normative statement in protocol/vitrin-v0.xml's `vitrin_grant` description and docs/protocol/04-vitrin_grant.md, and make it structural in crates/vitrin-core/src/grants.rs against the `parent_grant_id` field the Phase-1 grant table already carries present-but-null. | Q4 must close before spec 1.0-candidate (M2 = Phase 2 exit) and no Phase-2 epic builds `attenuate` — so it closes as a cap enforced before the mechanism exists, which is cheap now and impossible to retrofit once a chain is on the wire. | P2.3.2 | Unit test constructs a two-level parent chain directly in the grant table and asserts the third grant is refused; a one-level chain is accepted. `xmllint --relaxng` green after the description edit. The decision-log entry exists and Part B's Q4 row is marked CLOSED with a link to it, matching how Q15/Q16 were closed by D-017/D-018. | `protocol` |
| **P2.3.6** | Land the CAS loop in the SDK: `grant.actuate(click(node.id), expected_epoch=obs.epoch)` plus a `StaleEpoch` exception in sdk/python/src/vitrin_os/errors.py, and a checked-in executable file reproducing PRD Doc 2 §18 steps 4-5 verbatim as the reference client for the epoch API. | The SDK does NOT retry automatically. An automatic retry loop inside the SDK hides exactly the number P2.3.7 must measure and turns a false-reject rate into a latency figure nobody sees; the SDK raises `StaleEpoch` carrying the current epoch and the retry hint, and the agent decides — which is what the PRD's own pseudocode does. | P2.3.3, P2.2.5 | The checked-in PRD-18-steps-4-5 file executes against the real chain and its source is diffed against the PRD's own code block by a CI check, so a divergence between document and running code fails the build in whichever direction it happens. Unit tests: `StaleEpoch` exposes `current_epoch` and `retry_after_epoch` | `sdk` |
| **P2.3.7** ★ | Test the design claim: tests/integration/test_real_epoch_cas.py drives shipped `vitrind` + real `vitrin-shim` + real Firefox ESR against a checked-in animation-heavy page and measures the false-reject and false-accept rates of P2.3.1's invalidation policy over >=500 scripted observe-then-act cycles, publishing the numbers to… | This is the task that turns 'a design claim, not a proven result' into a number, and D-014 makes it a gate on freezing the mechanism in the spec — so a red result blocks the core 1.0-candidate rather than being noted and shipped. | P2.3.4, P2.3.6, P2.2.8 | False-accept count == 0 over >=500 cycles — one instance fails the gate. False-reject rate <= 5%, reported with per-invalidation-class attribution from P2.3.1's counters so a miss is actionable. The PRD Doc 2 §18 steps 4-5 file from P2.3.6 runs verbatim inside this gate and completes. | `rust-core` |

### E2.4 — VLM fallback pipeline

- **Goal:** an out-of-TCB service synthesizes trees for treeless surfaces (games, canvas, custom GUIs), cached and damage-invalidated, unified into the same node model (PRD Doc 2 §6.3).
- **Dependencies:** **E2.1**'s node schema (P2.1.1 — not E2.2's; the schema is the phase's critical-path decision and reserves this epic's `confidence`/`synthetic` slots) plus E2.2's epoch stamping; A1 capture path — the parser consumes the same observed frames agents do, no privileged tap.
- **Design decisions:** parser choice (OmniParser-class, pluggable); confidence surfacing → **Q3** (v0: per-node confidence attribute + tree-level `synthetic: true`, agents opt in); cache keying/invalidation on damage regions; deployment shape (sidecar service, never in-core — PRD §17).
- **Exit criteria:** a canvas-only surface yields an actionable synthetic tree through the *same* SDK calls; misclick behavior at low confidence characterized and documented (the honest-degradation posture of PRD §9).

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.4.1** | Extend E2.2's tree vocabulary in `protocol/vitrin-v0.xml` plus its prose page with the synthetic-publish path: a `vitrin_tree_publisher` facet minted by `vitrin_realm.request_grant` under a new `vitrin_grant.verb` bit `publish_tree` (0x40) carrying `publish_tree(surface, tree, observed_epoch)`; a tree-level `synthetic` flag | Q3's v0 posture becomes wire vocabulary, not sidecar convention — a convention cannot bind a lying publisher. Confidence is a per-node byte rather than a float: it must survive the codec unambiguously and be comparable across backends. `accept_synthetic` is a request flag, NOT a verb | P2.1.1, P2.1.2 | `xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml` green and `cargo xtask codegen --check` idempotent; the two existing verb tripwires go red until a human classifies the new bit — `crates/vitrin-protocol/tests/decode_errors.rs`'s `Verb::VALID_MASK == 575` pin and `crates/vitrin-core/src/consent/render.rs`'s unserved-set catalogue test | `protocol` |
| **P2.4.2** | Serve `publish_tree` in the core: admit the verb at `crates/vitrin-core/src/enforcement.rs`'s single chokepoint, store the published tree on the scene node beside the shim-sourced one in `crates/vitrin-core/src/scene/mod.rs`, stamp it with the core's own tree epoch, tag it with the publishing principal's identity, and refuse a publish whose… | A synthetic tree confers zero authority: actuation still passes the same chokepoint against the ACTING agent's own verbs, so a lying publisher can only mis-aim a click that agent was already permitted to make — stated in the interface prose so node addressing is never read as an authority path. | P2.4.1, P2.1.6 | Unit tests: publish without `observe` on the resource refuses at mint; publish with a stale `observed_epoch` refuses recoverably and never kills the connection (fatal-vs-recoverable razor, docs/protocol/00-conventions.md) | `rust-core` |
| **P2.4.3** | Build the sidecar process itself under a new `services/vlm-parser/` (does not exist today): a Python service that connects to the core as the ordinary principal `vitrin://local/service/vlm-parser`, holds `observe + publish_tree` on a realm under its own consented grant, loops capture → parse → publish, and dies cleanly when its grant is revoked or… | Deployment shape is a sibling process, never in-core and never a child of `vitrind` (PRD 17: the parser's memory-unsafety must be irrelevant to the TCB). It uses `sdk/python/src/vitrin_os/client.py` unmodified — if the sidecar needs an SDK change, that is evidence the agent-facing API is wrong, not that the sidecar is special. | P2.4.2, P2.6.1 | `pstree` in an integration run shows the sidecar as a sibling of the agent under the harness, never a descendant of `vitrind`; `grep -rniE 'vlm\|omniparser\|sidecar\|parser' crates/vitrin-core/src` returns nothing (the TCB does not know it exists) | `sdk` |
| **P2.4.4** | Define a pluggable `Parser` interface in `services/vlm-parser/` and ship two implementations behind it — a deterministic classical-CV reference backend that runs in CI with no model download and no network, and an OmniParser-class model backend selected by env | Pluggability is the epic's stated design decision, so the CONTRACT TEST, not either implementation, is the deliverable. The deterministic backend exists because a model-dependent CI job would make every number in P2.4.5/P2.4.6 unreproducible; it is explicitly the floor, not the recommendation. | P2.4.3, P2.1.1 | A checked-in fixture corpus (`services/vlm-parser/tests/corpus/`, at least 12 frames covering canvas, WebGL, game and custom-widget surfaces) with hand-labelled ground-truth control rectangles in JSON | `sdk` |
| **P2.4.5** | Add the parse cache to `services/vlm-parser/`, keyed on the exact SHA-256 of the observed frame's bytes, with the measurement harness that reports hit rate and miss cost over a scripted session. | v0 keys on the whole-frame content digest, not on damage regions, because `vitrin_view.frame_ready` carries no damage rectangles at all (D6's poll model, checked in the IDL) and inventing region keying would need an observe-side extension this epic does not own. The choice buys one real property | P2.4.4 | Measured and published in `docs/book/src/limits.md`: cache hit rate over a scripted 200-frame session for each corpus class (static canvas, animated canvas, idle app), and p50/p95 miss cost in ms per backend. A test constructs two byte-identical frames and asserts one parse; a test mutates one pixel and asserts a second parse. | `sdk` |
| **P2.4.6** | Run and publish the misclick experiment the exit criterion demands: over the labelled corpus, bin every synthetic node by confidence decile and measure, per decile, hit rate (node centroid inside its ground-truth rectangle), MIS-TARGET rate (centroid inside a different control's rectangle) and void rate (centroid on no control) | Mis-target is separated from void deliberately and is the number that matters: a click landing on nothing wastes a turn, a click landing on the WRONG control does something the agent did not intend, and averaging them into one accuracy figure hides exactly the failure the honest-degradation posture (PRD 9) exists for. | P2.4.4; P2.4.5 | A decile table per backend (10 rows x hit / mis-target / void, with N per bin) checked in under `docs/book/src/limits.md` and regenerated by one command from the corpus; the chosen `min_confidence` default is a constant in the Python SDK derived from that table in a comment naming the decile and its measured mis-target rate | `ci-docs` |
| **P2.4.7** ★ | Land `tests/integration/test_real_synthetic_tree.py`: shipped `vitrind` → shipped `vitrin-shim` → real Firefox ESR on a canvas-only page whose a11y tree is proved to contain zero actionable nodes inside the canvas rect → the real sidecar publishing over a real socket → the Python SDK agent calling the SAME `find(role, name)` plus node-addressed… | Every seam carries real software: shipped core binary, shipped C shim, pinned real Firefox ESR (third-party), the real sidecar as a separate process, the real Python SDK over a real Unix socket — no `vitrin-mock-shim`, no `shim/tests/mock_core.c`, per D12. | P2.4.3, P2.4.5, P2.3.3 | Green in CI with no mock on any seam; watched failing on at least four constructed breakages, each red on a different assertion: sidecar not started (find returns nothing); sidecar publishing with `synthetic` unset (the agent's `accept_synthetic` path no longer matches and the run refuses) | `sdk` |

### E2.5 — Native semantic demo app

- **Goal:** one application pushes trees natively (`scene.push_tree(surface, tree, epoch)`), proving the toolkit-backend path end to end (PRD §5.2; seeds the Phase-4 backends).
- **Dependencies:** E2.1's node schema and wire (P2.1.1/P2.1.2 — `vitrin_shim_surface.push_tree` is owned there, not here), plus E2.2/E2.3 so the native path produces spec-conformant versioned trees; a new app-facing surface-commit extension.
- **Design decisions:** toolkit — egui or iced (Rust, days-scale embedder work, same language as the core) over Flutter (heavier embedder, claimed for Phase 4); stabilize only the minimum native protocol that makes the demo honest (`push_tree` + buffer-commit pairing).
- **Exit criteria:** an agent completes a form-fill task on the demo app with zero a11y-bridge involvement; the demo doubles as the spec's reference client for the native tree API (WS-A input).

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.5.1** | Author the app-facing native semantic extension as a NEW artifact — `protocol/vitrin-semantic-v1.xml` in the wayland-scanner dialect (never validated against `vitrin-v0.rng`) plus its prose page and its `NOTICE` path→license entry. The `vitrin-v0.xml` half is **not** here: `vitrin_shim_surface.push_tree` is owned by P2.1.2. | Two carriers, one encoding. The app-facing half is a Wayland extension the SHIM advertises — not a core connection — because an app holding a core socket would break the spawn model and the no-app-rewrites invariant in one stroke. | P2.1.2 | `xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml` green; `wayland-scanner client-header` and `server-header` both succeed on `protocol/vitrin-semantic-v1.xml` with the pinned scanner; `cargo xtask codegen --check` idempotent with `shim/include/vitrin-protocol.h` diff-clean | `protocol` |
| **P2.5.2** | Implement the app-facing extension in the shim: new `shim/src/semantic.c` plus `shim/include/semantic.h`, global registered in `shim/src/globals.c`, relayed upstream from `shim/src/upstream.c` on the surface's commit, with the meson entry and the probe-catalogue/ledger bookkeeping updated. | The shim RELAYS a native tree verbatim and NORMALIZES an a11y-sourced one (E2.1) — two producers, one encoding — so the byte-equality test below is the load-bearing check that 'one node model' is true rather than claimed. | P2.5.1; A5 | A new C test client under `shim/tests/` attaches a tree plus a buffer; against `shim/tests/mock_core.c` (a COMPONENT test, never a gate) assert exactly one upstream `push_tree` per commit whose tree changed, zero for commits whose tree did not change, and correct ordering relative to `attach`/`damage`. | `c-shim` |
| **P2.5.3** | Accept, validate and serve native trees in the core: handle `push_tree` in `crates/vitrin-core/src/shim.rs`, store it on the scene node in `crates/vitrin-core/src/scene/mod.rs`, stamp the tree epoch, and serve it on the same observation path the a11y bridge feeds — `synthetic=false`, confidence saturated. | The core stores and serves and does not parse: validation is structural only (node-count cap, depth cap, string-length cap, acyclic parent links, UTF-8), and a violation is a recoverable refusal to the shim plus a recorder entry, never an abort — a confined app must not be able to kill the core by pushing a bad tree. | P2.5.2, P2.1.6 | `fuzz/fuzz_targets/` gains a tree-decode target, 4 h clean before the gate is cited; cap tests — a 10^6-node tree and a 10^5-deep tree each refuse with measured peak-RSS delta under 8 MB and the core survives; a cycle in parent links refuses rather than looping | `rust-core` |
| **P2.5.4** | Build the demo app as a new Rust crate `examples/native-demo/` (egui): a three-field form plus a submit button, rendering as a plain Wayland client of its shim, binding the `vitrin-semantic-v1` global and pushing a tree alongside every buffer commit. | egui over iced, and over Flutter which PRD §5.3 claims for Phase 4 — egui already emits AccessKit `TreeUpdate`s, the same schema E2.1 normalizes toward, so the demo consumes one schema instead of hand-rolling a second. THIS MUST BE VERIFIED AT TASK START against the pinned egui version's actual role/property coverage for the demo's four controls | P2.5.2; P2.5.1; E2.2 (stable-id rules the pushed tree must satisfy) | The app runs against a bare `vitrin-shim` with no core and pushes a well-formed tree (checked via the shim's ledger); `cargo xtask` gains a launcher; the pushed tree's four controls have non-empty roles and names, and node ids are byte-stable across 100 consecutive repaints with no content change | `sdk` |
| **P2.5.5** ★ | Land `tests/integration/test_real_native_tree.py`: shipped `vitrind` → shipped `vitrin-shim` → the real `native-demo` binary → the real Python SDK, where the agent completes the form-fill entirely by `find(role, name)` plus node-addressed actuation under `expected_epoch`, with the a11y bridge both disabled AND proved to have contributed zero nodes. | 'Zero a11y-bridge involvement' is proved twice on purpose, because the P1.9.8 lesson is that a wiring claim is not a discriminating one: the run starts with the bridge disabled AND a core-side counter reports zero bridge-sourced nodes reached the served tree. The agent's path contains no pixel coordinate at all — grep-provable in the gate's own source | P2.5.4, P2.3.3 | Green in CI, no mock on any seam, registered in `tests/integration/run.sh`'s gate list so an absent file cannot read as a green suite. Watched failing per D12 item (4) on at least five constructed breakages, each red on a different assertion: `push_tree` suppressed in the app (find returns nothing); one node's name corrupted (find fails, not the click) | `sdk` |
| **P2.5.6** | Turn the demo into the spec's reference client (WS-A input): write the native-path conformance page under `docs/protocol/`, and prove it sufficient by building `shim/tests/native_tree_minimal.c` — a client of at most 300 lines written ONLY from that page and `protocol/vitrin-semantic-v1.xml`, without reading `examples/native-demo/`. | The exit criterion asks for a reference client, and a document nobody has re-implemented from is not one — so the deliverable is a second, independently derived implementation, not prose. The 'written only from the spec' rule is enforceable in review by the fact that the C client and the Rust app share no code and no helper | P2.5.2; P2.5.4 | `native_tree_minimal.c` builds in the shim's Meson job (which links no Rust toolchain) and passes the SAME push/commit ordering test P2.5.2 defines, unchanged; every spec ambiguity found while writing it lands as a `docs/protocol/` edit in the same PR and is listed in the PR body | `ci-docs` |

### E2.6 — Filesystem powerbox v0

- **Goal:** realms spawn with an empty mount namespace + Landlock; the core-owned picker returns already-open fds over `SCM_RIGHTS`; subtree grants; basic (non-durable) standing grants (PRD P12, Doc 2 §12).
- **Dependencies:** A2 realm spawn (this epic extends it — closing Phase 1's documented D9 gap); A3 consent surface (the picker is core-rendered); **Q11 decided at epic start** (the realm-vs-Unix-user boundary shapes the namespace/UID layout).
- **Design decisions:**
  - Landlock ABI floor + degradation ladder for older kernels (PRD caveat: documented per tier, never silently weakened).
  - Realm private-storage layout.
  - Picker shape: core-drawn UI vs. a core-owned separate process with core-drawn chrome.
  - Consent-ladder subset: **only `once` / `while-running` rungs ship in v0**; durable rungs (`until-revoked`, `always`) are structurally blocked until provenance exists (E3.7) — stated explicitly so the ladder is never shipped ungated (**Q9** v0 posture; **Q13** first prompt-design review happens here).
- **Exit criteria:** **the ransomware demo** (PRD user story 6): a payload realm can write exactly its designated fds + realm storage, verified by an adversarial test attempting home-directory reach, path races, and picker spoofing; every designation journaled; the demo is scripted and reproducible (it is a WS-B/WS-C asset).

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.6.1** | Decide Q11 in a new decision-log entry and land crates/vitrin-core/src/spawn/isolation.rs: a runtime preflight that probes CLONE_NEWUSER/NEWNS/NEWPID/NEWIPC/NEWUTS/NEWNET availability, the Landlock ABI via landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION), seccomp availability, and the distro-level userns restrictions… | Q11 v0 answer, recommended: a realm is INTRA-USER by default — one host uid for the whole session — with the per-realm uid/gid PRD Doc 2 §4.5 promises delivered inside an unprivileged user namespace (single-id map written by the parent, setgroups=deny), alongside per-realm mount/PID/IPC/UTS/net namespaces. | A2, A3 | An isolation matrix generated (not transcribed) by running vitrind --print-isolation on the CI kernel plus at least three distro kernels spanning a >=6.12, a 6.1-class LTS and a 5.15-class LTS — every cell is the value the kernel reported, and CI fails if the checked-in table is stale, the same posture as cargo xtask codegen --check. | `rust-core` |
| **P2.6.2** | Replace spawn.rs's environment-only confinement with a real one: clone the realm child with CLONE_NEWUSER\|NEWNS\|NEWPID\|NEWIPC\|NEWUTS, write uid_map/gid_map/setgroups from the parent while the child blocks on a sync pipe, and exec a new core-owned vitrin-realm-init helper that builds the realm's mount namespace | A helper binary rather than growing spawn.rs's pre_exec closure. That closure's documented discipline is 'syscalls and no Rust', async-signal-safe because the core is multi-threaded (Smithay's EGL/GL stacks spawn threads), and mount-namespace construction cannot be squeezed into it without allocating | P2.6.1, A2 | A mock-free tests/integration/test_real_confinement.py against the shipped binaries: from inside the realm, open() on a canary file in $HOME fails ENOENT AND the same run proves that canary is reachable under --isolation=none (the positive control §5's absence rule demands — an absence over a path nothing proved reachable is satisfied by no path at all) | `rust-core` |
| **P2.6.3** | Apply a Landlock ruleset in vitrin-realm-init immediately before execve — read+write+truncate on the realm's private storage, read+exec on the read-only runtime paths the shim and app need, read-write on the realm runtime dir — and publish the per-ABI degradation ladder as a generated table. | ABI 1 is the floor for --isolation=default; each rung above buys a named right whose absence is a MEASURED weakening, never a silent one. Two rungs matter enough to name in the plan: without LANDLOCK_ACCESS_FS_TRUNCATE (ABI < 3) a designated read-only fd can still be truncated to zero, which is directly ransomware-relevant and must appear in the matrix and… | P2.6.1, P2.6.2 | **ACCEPTED 2026-08-19 ON THE CORRECTED CRITERIA, AND TWO OF THESE ARE WRONG ON THE KERNEL'S OWN TERMS — see "P2.6.3, corrected" below, which is the standing acceptance record and restates them rather than replacing them; Correction 7 there is the acceptance. That date is this document's acceptance record and D-044; issue #187's own closure timestamp is whatever GitHub writes when the merge lands, and no surface in this repository asserts a value for it.** As written: the ladder table is generated by cargo xtask isolation-matrix on each kernel in the CI matrix, one row per ABI actually reported, and CI goes red if the checked-in table is stale. Behavioural per-rung tests, not prose: on the highest ABI available, ftruncate on a designated read-only fd fails EACCES | `rust-core` |
| **P2.6.4** | Install a seccomp-bpf filter with NO_NEW_PRIVS in vitrin-realm-init before execve, covering the shim and every descendant, as a reviewed data table with one comment per row naming the escape class each entry closes. | Deny-list (bwrap/Flatpak shape), not allow-list, for v0: Firefox's syscall surface is not enumerable at this stage and an allow-list would fail closed against the project's own acceptance app, so the honest v0 posture is to close the named classes from PRD Doc 2 §15 | P2.6.2 | A table-driven tests/integration rung runs a repo-authored probe client inside the realm attempting every denied syscall and asserting the exact errno per row — a row added to the filter without a case fails the test, so the table and its proof cannot drift. | `rust-core` |
| **P2.6.5** | Paired IDL+prose edit adding the powerbox vocabulary: verb bit designate_file (64), resource prefixes file: and dir:, a since-gated get_powerbox facet mint on vitrin_grant, a new vitrin_powerbox interface with request_file(mode)/request_dir() plus designated(fd, ...) and refused events, and a vitrin_shim_session.designation event carrying exactly… | The facet is a STRUCTURAL MINT on vitrin_grant, not a sixth new_id on request_grant, because that request's five new_id arguments are frozen forever and vitrin_grant's own description already documents this growth seam. One fd per message (P1.1.2's no-fd-arrays rule) means one designation event per file | A1, P2.6.1 (the tier decides whether the deployment can serve the verb at all) | xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml green; cargo xtask codegen idempotent and --check green; the round-trip property test in crates/vitrin-protocol/tests/roundtrip.rs covers the new messages including the fd-bearing ones | `protocol` |
| **P2.6.6** | Build the core-drawn file picker and the fd-minting path: navigate/select/confirm chrome on the existing consent stack, race-free resolution with openat2 RESOLVE_NO_SYMLINKS from a directory fd, delivery over SCM_RIGHTS, subtree designation as a directory fd, and one journal record per designation. | Picker shape: core-drawn IN-CORE, reusing the consent renderer (tiny-skia + cosmic-text per D4), not a core-owned separate process — a separate process needs its own trusted-path story from scratch, while the consent surface already has ConsentGrab's exclusive input grab, the trusted indicator and the trust band, so picker spoofing reduces to the… | P2.6.5, P2.6.2, P2.6.3, A3 | A path-race test: a repo-authored racer swaps a component of the chosen path between the human's confirm and the open, and the delivered fd's fstat (st_dev, st_ino) must equal the entry the picker displayed; where openat2 is unavailable the picker REFUSES to designate rather than falling back to a racy resolve, asserted. | `rust-core` |
| **P2.6.7** | Shim-side designation relay: the shim receives the designation fd from the core over fd 3 and serves a per-realm designation.sock inside the realm runtime dir, handing fds to a powerbox-aware app over SCM_RIGHTS and accepting nothing else. | The shim relays rather than the core serving the app directly: the app is the least-trusted process in the system, and giving it a socket into the TCB adds an attacker-facing surface to the core for no gain, while the shim is already the app's confinement peer and holds nothing it did not already hold. Legacy path-expecting apps are explicitly OUT of v0 | P2.6.5, P2.6.6, A5 | A shim acceptance script against shim/tests/mock_core.c (a component test, explicitly labelled as one, never gate evidence) plus the mock-free rung inside P2.6.9; an fd-leak test holding the open-fd count flat across 1000 designations | `c-shim` |
| **P2.6.8** | Make the durable consent rungs structurally unshippable, and run Q13's first prompt-design review across **every verb Phase 2 serves** — `designate_file`, `egress` and `publish_tree` — each shipping admitted-but-refused `unsupported` until its human-readable copy exists, the same staging `observe_cursor` still uses and `layout_*` used until WS-E.1.4 served both. | Q9 v0 posture: only once and while_running ship. The block is made STRUCTURAL rather than documented — admitting a durable rung requires a ProvenanceRef value whose only constructor lives behind a provenance cargo feature that no build in the tree enables, so 'ship the ladder ungated' becomes a compile-time impossibility rather than a review discipline. | P2.6.6, P2.4.1, P2.7.2 | A test enumerating all four persistence entries shows exactly two admissible and the other two resolving unsupported; a CI feature-matrix check proves the durable path is unreachable in every configuration the tree can build | `rust-core` |
| **P2.6.9** ★ | Land the ransomware demo as a scripted, reproducible adversarial suite — tests/integration/test_real_ransomware.py plus a repo-authored payload realm — covering home-directory reach, path races and picker spoofing, and report the payload's measured write set. | The payload is repo-authored, following the click-target/form-target precedent, and the disclosure is made in the same breath rather than left to be noticed — no third-party program will cooperatively enumerate its own write attempts. | P2.6.2, P2.6.3, P2.6.4, P2.6.6, P2.6.7, P2.6.8 | The run reports the payload's actual write set and the gate asserts it equals exactly {designated fds} union {realm private storage} — a measured set equality, not an assertion that confinement is implemented. Every designation appears in the journal with a matching (dev, ino). | `rust-core` |
| **P2.6.10** | Deliver the standing grant as the object PRD Doc 2 §12 describes — first-class in the grant table, enumerable in a connected-apps surface, revocable one-by-one, journaled and rate-audited — or state in the epic that A3's `while_running` already is it and nothing new ships. | Q9's v0 posture makes the durable rungs unshippable (P2.6.8); that is the *negative* half. This is the positive half, and the two are separately deliverable. If the answer is "`while_running` suffices", that must be written down, because the epic's goal line otherwise claims something Phase 1 already shipped. | P2.6.6 | A human can enumerate every live designation and standing grant for a realm and revoke one without revoking the others; each appears in the flight recorder with its designation journaled; the enumeration is asserted against a run that made three designations across two realms. | `rust-core` |

#### P2.6.3, corrected

The row above is the plan as written before the work started. Landing it found
that **half of it is undone and two of its acceptance criteria are false on the
kernel's own terms.** The original text is left standing above, because a plan
document whose wrong criteria are quietly rewritten is a document nobody can
audit; the correction lives here, and this subsection is the standing
acceptance record for P2.6.3.

**What landed (issue [#187](https://github.com/vitrin-os/vitrin-os/issues/187)):**
the ruleset itself — `crates/vitrin-realm-init/src/landlock.rs`, enforced on
the realm's PID 1 after `PR_SET_NO_NEW_PRIVS` and `pivot_root` and before the
shim's `execve`, with a rung descent that steps down on `EINVAL`/`E2BIG` until
the kernel accepts one, a `--landlock=highest|abi:N|off` flag, the obtained
rung in `applied_profile` and in the journal's `isolation.landlock` object, a
WARN per spawn when the obtained rung is below the request or below the
kernel's ABI, and Landlock in the startup floor so a kernel without it refuses
the session instead of degrading.

**What landed second, and how far short of the criterion it falls:** the
**generated** ladder table and its staleness gate — `cargo xtask
isolation-matrix`, `docs/book/src/isolation-matrix.md`, and the `--check` step
in `.github/workflows/ci.yml`. It is a table of **what this build requires of a
kernel's Landlock**, derived rather than transcribed: the rung ladder is parsed
out of `landlock.rs`, the floor and ceiling out of `vitrin-realm-init`'s
`lib.rs`, the domain grouping is computed from the parsed ladder, and every row
must name a published claim whose sentence still exists on
`docs/book/src/limits.md`, `README.md` or `SECURITY.md`. **It measures no
kernel**, which is Correction 5 below.

**What did not land inside P2.6.3, and has since landed elsewhere:** the
*multi-kernel per-ABI ladder*, one row per ABI actually reported. It was
recorded here as DEFERRED-not-delivered and handed to
[#281](https://github.com/vitrin-os/vitrin-os/issues/281); **#281 has since
delivered it**, and the details are Correction 6 below — five distribution
kernels booted under QEMU with the shipped `vitrind`, reporting ABI 1, 2, 4, 6
and 7, rendered into `docs/book/src/isolation-kernels.md` and held by
`cargo xtask kernel-matrix --check`. Read this paragraph *with* Correction 6,
never instead of it: the deferral it records is discharged, and which kernel
releases clear the floor is now a measured, checked-in answer rather than an
open question.

What has **not** changed is the axis this section was careful about. Those are
**kernel** rows taken in a bare initramfs, so the number of *distributions*
measured as such is still one; the *values* in the per-rung *behavioural*
statements were recorded on one box on one date (the tests that take them run
here and on the CI runner, which declares `VITRIN_REQUIRE_LANDLOCK_ABI=7` so a
skip is a panic, and on no third machine); and nobody other than the collector's
author has re-run its failure levers. **Nothing may describe P2.6.3 as complete** on the strength of
the kernel table alone, and `docs/book/src/limits.md` states the same three
residuals in the same terms.

**Correction 1 — `ftruncate` on a read-only fd is `EINVAL`, not `EACCES`, and
Landlock has nothing to do with it.** The row asks for "on the highest ABI
available, `ftruncate` on a designated read-only fd fails `EACCES`". That test
would pass on a kernel with Landlock compiled out, at every rung, and with no
ruleset at all: `ftruncate(2)` on a descriptor opened `O_RDONLY` is refused
`EINVAL` by the VFS before any LSM hook is reached, because the fd is not open
for writing. It is a vacuous criterion — the exact class this repository's own
rules forbid. **The real vector `LANDLOCK_ACCESS_FS_TRUNCATE` (ABI 3) gates is
`truncate(2)` on a *path* the domain grants `READ_FILE` on**, plus `creat(2)`
and `O_TRUNC`, none of which needs a writable descriptor. That is what the
per-rung measurement must exercise, and what the measurement quoted on the
limits page does exercise: at rung 2 a `truncate(2)` on a read-granted file
succeeds and the file goes to zero; at rung 3 the same call fails `EACCES`.

**Correction 2 — "a designated read-only fd" cannot be carved out of a
read-write grant, because Landlock has no deny rules.** The row's framing
assumes a hierarchy can be granted read-write while one file inside it stays
read-only. Landlock cannot express that: rules are *allow* rules on a path
hierarchy, rights are the union of every rule matching a path's ancestry, and
there is no rule that subtracts. A read-only file inside a writable hierarchy
is therefore only achievable by **not granting the hierarchy** and granting the
narrower paths instead — which is what `grants` does, and why the read set is
enumerated rather than rooted. The ransomware-relevant property the criterion
was reaching for survives, restated as something the mechanism can actually
deliver: *a path outside every granted write hierarchy cannot be truncated,
and at ABI < 3 it can be truncated even where it cannot be written.*

**Correction 3 — three rungs of the published ladder cannot be measured by
capping, and two of them the plan did not anticipate.** `--landlock=abi:N` caps
`handled_access_fs`, so it can only simulate the absence of a rung that *moves*
that mask. ABI 4 (network scoping) and ABI 7/8 (`landlock_restrict_self`
flags: audit-log control and `TSYNC`) do not, and a shipped session requests
none of the three — `handled_access_net` is zero by design and the flags word
is zero. The consequence is that the enforced domain is **byte-identical at
rungs 3 and 4**, and **byte-identical at rungs 6, 7 and 8**: nine rung numbers,
six distinct domains, while `profile_for` still renders nine distinct strings.
Those five rung numbers are three ladder rows, and every surface that publishes
the ladder now says so.

One qualification, added when the audit diagnostic landed: ABI 7's
`LOG_NEW_EXEC_ON` **is** reachable, through `VITRIN_LANDLOCK_AUDIT=1` in
vitrind's own environment, which the core forwards into the helper and nothing
else can set. Under it rungs 6 and 7 differ by that one bit. It is a
measurement instrument for the read/write set P2.6.9 owes — it decides what the
kernel logs, never what it permits — so the byte-identity above holds of every
run that is not a measurement, which is every run that ships. See
`tests/integration/landlock-denials.sh`, which is what consumes it.

**Correction 4 — the degradation ladder is replaced by a declared ABI floor,
and that NARROWS this task rather than completing it** (owner's decision,
2026-08-15).

The plan's key-decisions column says "ABI 1 is the floor for
`--isolation=default`; each rung above buys a named right whose absence is a
MEASURED weakening, never a silent one." The first clause is now false by
decision and the second is what made it false: **measuring every rung's absence
requires the generated per-kernel table that did not land**, and nothing in this
repository is going to hold nine rungs honest by review. Between publishing a
spectrum nobody measures and declaring which kernels this build serves, the
owner chose the second.

What that means concretely:

- `vitrin_realm_init::LANDLOCK_MIN_ABI` is **6** — it was 7 for one day, and the
  owner lowered it on 2026-08-16 — and a kernel reporting less is
  **refused at startup** — by `spawn::isolation::admit`, which reports
  `below-floor(abi=N,required=M)`, and again by the helper's `landlock::apply`
  if the two ever disagree. `vitrind --print-floor` prints the number as
  `build.landlock_min_abi`.
- **Why 6, and why the move down cost nothing:** 6 is the *lowest* rung at which
  the domain this build enforces is unchanged. The enforced triple —
  `handled_access_fs`, `scoped`, and the `landlock_restrict_self` flags word —
  is identical at rungs 6, 7 and 8, because rungs 7 and 8 buy only
  `landlock_restrict_self` *flags* (audit logging, `TSYNC`) and every shipped
  run passes flags = 0. Rung 5 differs (`scoped` arrives at 6) and rung 9
  differs (it adds `RESOLVE_UNIX`), so **no page says the domain is identical
  from 6 to 9**; the floor decides *admission* and never which rung is applied,
  which stays `min(kernel ABI, build ceiling)`. All three facts are asserted by
  `the_floor_costs_nothing_because_the_domain_is_flat_from_six_to_eight` in
  `crates/vitrin-realm-init/src/main.rs`. The immediate reason for the move was a
  measured row: Debian 13 (`6.12.101+deb13-amd64`) reports ABI 6 and was being
  refused for nothing.
- **Which kernel releases the floor excludes is now measured** (2026-08-16,
  issue #281). Five distribution kernels booted under QEMU with the shipped
  binary: `5.15.0-191-generic` (ABI 1), `6.1.0-50-amd64` (ABI 2) and
  `6.8.0-139-generic` (ABI 4) are refused; `6.12.101+deb13-amd64` (ABI 6) and
  `6.17.0-1020-azure` (ABI 7) start. The rows are checked in under
  `tests/kernel-matrix/rows/` and published as
  `docs/book/src/isolation-kernels.md`. They are **kernel** rows, not
  distribution rows — the same `6.17.0-1020-azure` under Ubuntu userspace
  reports different policy cells — so the *distribution* half of the runner
  measurement is still a transcription from a CI job log that archives nothing,
  and the page says so.
- The one-rung descent still exists in `create_ruleset` — a kernel may report
  ABI N and refuse rung N's mask — and now bottoms out at the floor instead of
  at rung 1.
- `--landlock=abi:N` **keeps** its full range, including below the floor,
  because it is the instrument every per-rung measurement in this repository is
  taken with. A session pinned below the floor warns that no confinement claim
  this build publishes applies to its realms.

**PRD §20's "coverage is kernel-dependent" caveat is deferred, not answered.**
The floor removes the *unmeasured* half of the dependence by refusing the
kernels it was about; it does not produce a per-kernel table, and the restated
criteria below are unchanged by it. (Correction 5 below records what a
generated table could and could not be after this decision, and what landed.)
Nothing may read this correction as P2.6.3 closing.

*Scope note added 2026-08-19.* That sentence stands exactly as written —
Correction 4 did not close P2.6.3 and nothing here claims it did. **Correction 7
below is what closed it**, on criteria this correction is one of the
restatements of. The pointer is added here rather than by editing the sentence,
because this document corrects by appending; without it a reader meets the
forbidding sentence and never learns that the thing it forbids has since
happened for other reasons.

**Restated acceptance criteria for the undone half**, so the next task has a
target rather than the false one: a checked-in ladder table **generated** by a
subcommand (`cargo xtask isolation-matrix` or its successor) from
`vitrind --print-isolation` output on each kernel in the CI matrix, one row per
ABI actually reported, with CI red on a stale checked-in copy — the same
posture as `cargo xtask codegen --check`; the table's rows distinguishing rungs
that move the enforced domain from rungs that do not, rather than implying nine
distinct confinements; and behavioural per-rung tests driving `truncate(2)` on
a read-granted path (not `ftruncate` on a read-only fd) and `rename(2)` across
directories for `REFER`, each watched failing at the adjacent rung.

**Correction 5 — "one row per ABI actually reported, on each kernel in the CI
matrix" and "CI red on a stale checked-in copy" cannot both be satisfied by one
checked-in page, and the second is the half worth keeping** (2026-08-15,
landed with `cargo xtask isolation-matrix`).

The two clauses of the restated criterion above are in tension, and the tension
is not a wording problem. A page whose rows are *what a kernel reported* carries
the ABI of the machine that generated it. This repository has two machines and
they disagree — the development box reports Landlock ABI 9, the runner CI uses
reports 7 — so such a page is stale on one of them by construction, and a
`--check` step holding it would be red on every pull request. It would be
measuring the runner, not the repository.

What was built instead, and what it is honestly worth:

- **`cargo xtask isolation-matrix [--check]`**, emitting
  `docs/book/src/isolation-matrix.md`, wired into `.github/workflows/ci.yml`
  beside `codegen --check` and `session-matrix --check`.
- It **derives** rather than transcribes. The rung ladder is parsed out of
  `crates/vitrin-realm-init/src/landlock.rs` (bit constants, base mask, each
  `if rung >= N { mask |= RIGHT; }`) and cross-checked against the *measured*
  mask table pinned in that crate's `the_rung_masks_pin_a_measured_table`; the
  floor and ceiling are read out of `lib.rs`. Moving a right to another rung,
  or re-tuning `LANDLOCK_MIN_ABI`, makes the checked-in page stale and CI red —
  which matters because four published surfaces print that number.
- **It satisfies the criterion's second clause** — rows distinguishing rungs
  that move the enforced domain from rungs that do not — *by computation*: the
  domain grouping is derived from the parsed ladder, so "nine rung numbers, six
  distinct domains" is a number the generator produces, not one a human
  maintains.
- **Each row names the right it buys and the published claim it carries**, and
  #187's rule is a real failure mode rather than a convention: a rung row with
  no claim refuses to render, a published claim no row carries refuses to
  render, and a claim whose sentence has been deleted from
  `docs/book/src/limits.md`, `README.md` or `SECURITY.md` refuses to render.
  Each of those is covered by its own test.
- The per-domain statements are published **verbatim** on
  `docs/book/src/limits.md` and compared byte-for-byte after whitespace
  normalization, so a later cross-check of table against page needs nobody to
  adjudicate a paraphrase.

**What it does not do, stated plainly:** it probes nothing. It is not evidence
about any kernel and it does not close PRD §20's caveat. The
machine half stays `vitrind --print-isolation`, which the page tells a
reader how to read against the table. **Nothing may read this correction as
P2.6.3 being accepted either** — and, as at Correction 4, that sentence is
about *this* correction and is not superseded: what accepted P2.6.3 is
**Correction 7 below**
(2026-08-19), on a dated owner's decision rather than on anything here. The two behavioural per-rung tests the criterion asks
for do exist (`the_truncate_rung_is_measured_and_its_absence_is_measured_with_it`,
`rung_one_forbids_reparenting_that_the_rung_above_permits`) and the values they
pin were recorded on one box on one date. The first clause of the restated criterion — a row per ABI
a kernel actually reported — was taken up separately; see Correction 6.

**Correction 6 — the per-kernel rows exist, they are taken under QEMU rather
than in the CI matrix, and the staleness gate splits in two** (2026-08-16,
landed with `tests/kernel-matrix/` and `cargo xtask kernel-matrix`).

Correction 5's tension was real and its resolution was to build the *build*
table. What it did not consider is that the tension dissolves if the machine is
not the runner: a kernel booted deliberately, under a userspace this repository
controls, produces an answer that is the same on every machine that boots it —
so a page rendered from those answers **is** byte-stable and **can** be the
thing CI holds.

What landed:

- **`tests/kernel-matrix/collect.sh`** boots each kernel in
  `tests/kernel-matrix/kernels.manifest` under QEMU with the **shipped**
  `vitrind` in a minimal initramfs (`tests/kernel-matrix/init.c`, a static PID
  1), and writes one checked-in row per kernel holding `--print-isolation` and
  `--print-floor` verbatim plus the startup verdict. Each row carries its
  provenance: package URL and sha256, vmlinuz member and sha256, the QEMU
  command line, the userspace, the `vitrind` version, the schema version and a
  collection date.
- **Five kernels**, chosen as machines somebody might be refused on rather than
  to fill rungs: ABI 1, 2, 4, 6 and 7. Four of the nine rungs are reported by
  none of them, and the page says so — this is not the ABI sweep the criterion's
  first clause imagined, and it does not pretend to be.
- **`cargo xtask kernel-matrix [--check]`** renders
  `docs/book/src/isolation-kernels.md` from those rows and is wired into
  `codegen-diff` beside the other three regeneration diffs.
- **The staleness gate is two gates, and which is which is published.** `cargo
  xtask kernel-matrix --check` (every PR, no QEMU) holds the PAGE to the ROWS.
  `collect.sh --check` (`.github/workflows/kernel-matrix.yml`, scheduled and on
  demand) holds the ROWS to the KERNELS by re-booting all five, and goes red on
  a row older than its age limit. A green pull request proves the first and
  nothing about the second, which is why the rows carry dates.

**What this still is not.** It is a **kernel** measurement and never a
distribution one, and the cross-validation that proves the distinction is
published: booting the CI runner's own `6.17.0-1020-azure` in a bare initramfs
reproduces its `landlock.abi=7` exactly and *disagrees* with it on every policy
cell (`apparmor_restrict_unprivileged_userns` 0 vs 1, `mount.in_userns`
available vs `restricted-by-policy`, `tier` intra-user vs none). So the
distribution row still has to come from the distribution, the runner's own
reading remains a transcription from an expiring job log, and PRD §20's caveat
is still not closed.

**Correction 7 — P2.6.3 is accepted, on the corrected criteria and on a dated
decision about the ladder's lower half** (2026-08-19, [D-044](20-decision-log.md#d-044--the-sub-floor-landlock-rung-tests-are-kept-and-what-they-are-evidence-about-is-published-beside-them-the---landlockabin-dial-never-the-floor)).

The two sentences above that forbid reading Corrections 4 and 5 as this task
being accepted **stand exactly as written and are not being walked back**: the
floor decision did not accept it and neither did the build-side ladder table.
What accepts it is the whole set — the ruleset, the generated ladder with its
`--check` gate and its claim anchoring, the measured kernel page Correction 6
records, both behavioural per-rung tests in their corrected form, and the one
judgement that was left open after all of those had landed.

**That judgement, and the answer.** With the floor at 6, rungs 1–5 are
unreachable in production: a kernel reporting one is refused outright, not
confined weakly. Three behavioural tests in
`crates/vitrin-realm-init/src/main.rs` nevertheless enforce a domain at rung 1,
2 or 3, so they measure no state an operator can reach — the shape this
repository calls a check that stopped checking. The owner's answer is to
**keep them and publish why**: they hold the `--landlock=abi:N` *dial* honest
rather than the floor; they are the only evidence that any part of the ladder
table's lower half is not fiction — rungs 1, 2 and 3 of it, rungs 4 and 5 being
entered by no test at all — that page being derived from source and observing no
kernel answering; and the `REFER` result — rung 1 being *stricter* than rung 2 —
cannot be read off the mask column at all, which is why two tests asserting the
opposite invariant were replaced when it was measured. The reasoning is
published beside the tests, on `docs/book/src/limits.md`, and as a
generator-held **pair** of claims: `sub-floor-rungs-hold-the-dial-not-the-floor`
on the sub-floor rungs a test enters a domain at, and
`sub-floor-rungs-are-not-all-exercised` on the sub-floor rungs nothing enters
one at — rungs 4 and 5. Every sub-floor row names exactly one of the two, and
which one is decided by that row's list of behavioural tests rather than by an
editor, so the ladder's lower half cannot go back to having an unexplained
status — or a *wrong* explanation — while CI is green. The option of *adding*
tests for the four rungs no measured kernel reports was offered and **not**
taken; two of those four are not testable by this mechanism at all, since rungs
7 and 8 buy `landlock_restrict_self` flags rather than mask bits.

**Why this is a correction and not a silent close.** This task's *previous*
narrowing — "target recent kernels for now" — was settled by attrition and
reached four published pages as a deferral nobody could date. The decision
above is smaller and would have been easier still to leave implicit.

**What closing it does not make true**, and every one of these is published in
the same words on `docs/book/src/limits.md`: five kernels answered five ABIs and
four of the nine rungs are reported by none of them; those are **kernel** rows
in a bare initramfs, so the number of distributions measured as such is still
one; nobody but the collector's author has re-run its failure levers; the suite
itself has still only ever run on two machines; and the values in the per-rung
behavioural statements are one box on one date, their tests running on this
repository's development box and on the CI runner and nowhere else; those behavioural tests are **not** in
`tests/integration/` and are **not** table-driven off the ladder data the matrix
is generated from, which is a clause of the original task list that no
correction above restates as replaced and that closing this task does not meet;
and no mock-free gate measures the *write* denial at all — the shipped default's
mock-free measurement is a **read**, its probe opening `O_RDONLY`, so the
write criterion rests on a component test in `vitrin-realm-init`'s own suite;
that gap is **owed to P2.6.9** ([#193](https://github.com/vitrin-os/vitrin-os/issues/193)),
whose payload realm reports every write it attempted with the errno each got,
and it is not closed by anything that lands before it.
**PRD §20's caveat is answered for those
five kernels and for no others** — the sentence above stands.

### E2.7 — Network authority v0

- **Goal:** per-realm loopback-only network namespace plus own PID/IPC/UID (completing the container-per-realm baseline, PRD Doc 2 §4.5); egress as a designated host:port-scoped, journaled grant via a mediating proxy (PRD P13).
- **Dependencies:** A2 spawn; A3 grant table/consent; Q11 (shared with E2.6 — the two epics share a spawn-hardening sub-task and are planned as siblings).
- **Design decisions:**
  - Egress-proxy mechanism: per-realm proxy socket injected into the netns (recommended for v0 — simpler, no routing in the TCB) vs. veth + transparent redirect.
  - DNS mediation: resolve in the proxy, grants are host-name-scoped, resolved addresses pinned.
  - Egress rows in the grant schema.
  - **Q12** v0 posture: no blanket grants; browser-realm ergonomics deferred behind an interim per-realm template allowlist; the full answer is a decide-by-M3 item.
- **Exit criteria:** **the `ssh localhost` demo** (PRD §1.8, §15 threat row): inside a realm, host loopback unreachable, abstract sockets confined, path sockets absent — an adversarial test suite, not prose; one designated egress (host:443) works, expires, and revokes immediately; a realm with no grant emits zero outbound packets (verified by capture).

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.7.1** | Add CLONE_NEWNET to the realm clone and bring lo up inside the new network namespace, completing the container-per-realm baseline of PRD Doc 2 §4.5 on top of P2.6.2's namespace set. | Loopback-only: no veth, no bridge, no NAT, no routing state anywhere in the TCB (PRD Doc 2 §12). lo is brought up because apps expect it to exist, and that is safe precisely because nothing is listening on it — 'ssh localhost reaches the realm's own empty loopback' is a statement about what is bound, not about what is routable. | P2.6.1, P2.6.2 | Inside the realm the interface set is exactly {lo}, enumerated from the netns. connect() to a host loopback port THE TEST ITSELF OPENED fails ENETUNREACH or ECONNREFUSED, while the same run proves that port reachable from outside the realm (positive control — a refusal against a port nothing was listening on proves nothing). | `rust-core` |
| **P2.7.2** | Paired IDL+prose edit adding the egress vocabulary: verb bit egress (128), resource prefix net: with a host:port grammar, request_connect(host, port) plus a connected-socket delivery event on the vitrin_powerbox facet, and the pinned-address column the grant row needs. | One facet, not two: vitrin_powerbox carries request_file/request_dir and request_connect, making egress literally 'the socket analog of request_file' as PRD Doc 2 §18 step 9 sketches it. The grant row grows a pinned_addrs column holding the addresses resolved at grant time, so the pin is grant-table state rather than proxy state and survives a proxy restart. | P2.6.5, A1 | xmllint --relaxng green; cargo xtask codegen --check green; a proptest over generated selectors asserting every accepted string round-trips to exactly one (host, port) and that no accepted string contains a wildcard, a CIDR or a comma — checked by generation rather than by inspection | `protocol` |
| **P2.7.3** | Build the per-realm egress proxy: a listening socket created inside the realm's netns by setns-then-bind-then-setns-back, served by an out-of-core sidecar speaking SOCKS5 and HTTP CONNECT, which asks the core's enforcement chokepoint per connection and never decides anything itself. | Mechanism = a socket injected into the netns, not veth plus transparent redirect — the recommended v0 in the plan, and the reason is that it gives the TCB a listener instead of a router: no NAT, no nftables state, no packet path in the core. | P2.7.1, P2.7.2, A3 | A designated egress to an HTTP origin the harness starts itself works end to end from inside the realm through real curl. Revocation is MEASURED, not asserted: the latency from revoke to the first refused connection is reported and must be within one round-trip, and live connections are torn down… | `rust-core` |
| **P2.7.4** | Resolve DNS in the proxy, pin the resolved addresses into the grant row, and refuse any CONNECT to an address the pin does not contain — including literal-IP CONNECTs under a name-scoped grant. | There is no resolver inside the realm: no /etc/resolv.conf in the mount namespace, no route to a DNS server in the netns. Name resolution is therefore available ONLY through the proxy (SOCKS5h / CONNECT by name), which makes DNS mediation structural rather than a policy the app could route around. | P2.7.3 | A DNS server under the harness's control returns address A at grant time and address B afterwards: the connection to B is refused and journaled with the reason, and the same test proves A still works (positive control). A literal-IP CONNECT to an address the pin does not contain is refused even when the name grant would have covered it after the rebind. | `rust-core` |
| **P2.7.5** | Land the Q12 interim answer: per-realm egress templates in realm.toml — named, fully enumerated host:port sets rendered in the consent prompt as an enumeration — and measure the prompt count they save. | Q12 v0 posture: no blanket grants (already inexpressible after P2.7.2), browser-realm ergonomics deferred behind a per-realm template allowlist, full answer decide-by M3. A template is a PETITION SHORTCUT, never a pre-approval: it still raises exactly one prompt and the human still chooses from the two-rung ladder. | P2.7.3, P2.6.8 | A measurement, reported as a number into the M2 benchmark set: prompt count for a scripted 10-minute Firefox browsing session with and without the template (the consent-fatigue datum Q12 needs and nobody has). A parse test refusing any entry containing a wildcard, a CIDR, a port range, or more hosts than the cap. | `rust-core` |
| **P2.7.6** ★ | Land the ssh-localhost demo as a scripted, reproducible adversarial suite — tests/integration/test_real_ssh_localhost.py — proving host loopback unreachable, abstract sockets confined, path sockets absent, zero outbound packets without a grant by capture, and one designated egress that works, expires and revokes. | Third-party binaries are used here deliberately: E2.6's gate has a repo-authored payload and discloses it, so this gate buys the independence back where it is cheap — ssh from OpenSSH, curl, ss/ip are ubiquitous and nobody here wrote them. | P2.7.1, P2.7.3, P2.7.4, P2.7.5, P2.6.2, P2.6.4 | Five measured claims. (1) ssh -o BatchMode=yes localhost fails to connect, while the harness proves the same sshd is reachable from outside the realm in the same run. (2) An abstract socket bound on the host is unreachable from the realm and reachable from outside it. (3) find / -type s inside the realm yields exactly the realm's own sockets | `rust-core` |

### E2.8 — IME workstream (begins)

- **Goal:** land PRD Doc 2 §14's strategy as running code where cheap and documented plan where not: the agent `text` actuator (Unicode-direct, IME-bypassing) ships; the human IME path works for one reference combination.
- **Dependencies:** A5 shim seat model; core surface layer (candidate popups as core-owned surfaces).
- **Design decisions:** fcitx5-first (maintained, Wayland-native), IBus as documented fallback; candidate-window routing (core-owned surface positioned by the core, immune to nesting offsets); scope discipline — this is a known tarpit (PRD §9), so the epic carries an explicit **effort cap**: everything beyond the reference combination is a compatibility-matrix entry, not a commitment.
- **Exit criteria:** agent text entry into a CJK-locale app works with no IME involved; a human types Japanese into Firefox-in-realm via fcitx5 with correctly positioned candidates; the XWayland-IME fallback strategy is documented (consumed by E3.2).

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.8.1** | Add the `vitrin_shim_text_input` interface to `protocol/vitrin-v0.xml` plus its prose page: shim-to-core `enable`/`disable`/`set_surrounding_text`/`set_content_type`/`set_cursor_rectangle`/`commit_state`, core-to-shim `preedit_string`/`commit_string`/`delete_surrounding_text`/`done`. | The vocabulary MIRRORS text-input-v3 1:1 rather than inventing a better one, so the shim is a relay and not a translator — a translator is where version-skew bugs live, and fcitx5's own docs warn that client and compositor must agree on the protocol version. | A1; A5 | `xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml` green; `cargo xtask codegen --check` idempotent with `shim/include/vitrin-protocol.h` diff-clean; decode-error seeds added to `fuzz/corpus` for every new message; a test that no `vitrin_actuator_text.type` request emits any message on this interface | `protocol` |
| **P2.8.2** | Implement `zwp_text_input_v3` toward the app in the shim (new `shim/src/text_input.c`, registered in `shim/src/globals.c`), relay its state upstream over `vitrin_shim_text_input`, remove `zwp_text_input_manager_v3` from `shim/docs/firefox-refused-globals.txt` with the reason written up in `shim/docs/firefox.md`, and route the agent `text` actuator… | Preferring `commit_string` for agent text when the app has an active text-input is a genuine improvement over D7, not a workaround: `commit_string` IS 'deliver this Unicode string', which is what `vitrin_actuator_text` has always meant, so the codepoint-per-key keymap churn disappears for IME-aware apps. | P2.8.1; A5; P1.6.3's existing keymap path in shim/src/seat.c | `shim/tests/acceptance/firefox_bringup.sh` check D stays green with the interface removed from the refused allowlist (the ledger already shows Firefox binding it — shim/docs/globals-touched-firefox-140.12.0esr.log) and goes red if the global is advertised while the allowlist still names it | `c-shim` |
| **P2.8.3** | Add the core's IME connection: advertise `zwp_input_method_v2` on a SEPARATE core-owned socket (not the realm's, not the principal socket), bridge the focused realm's `vitrin_shim_text_input` state to it and its preedit/commit back, and refuse any connection that does not pass the IME trust check. | The IME sees every keystroke destined for the focused realm — it is a keylogger by construction, and that is a trust extension beyond the TCB that must be named in the threat model (PRD Doc 2 §15) and in docs/book/src/limits.md rather than discovered. | P2.8.2, P2.6.1 | An integration test proves a realm cannot reach the IME socket (path absent from the realm's runtime dir; a connection attempt from the realm's uid/pid is refused and journaled); a second IME connection is refused with a named reason | `rust-core` |
| **P2.8.4** | Composite the IME's `zwp_input_popup_surface_v2` as a core-owned surface positioned by the core at the app-reported cursor rectangle mapped through the realm view, at the same output stage as the consent overlay and agent cursor, clipped below the trusted band and never above the consent surface. | The core POSITIONS but does not DRAW the candidates — the input method renders its own popup and the core places it. That is the cheapest reading of PRD Doc 2 §14(2) that still delivers the property ('positioned correctly regardless of the app's nesting') and it keeps a candidate renderer out of the TCB, where D4 already refused to put a toolkit. | P2.8.3; A6; the P1.7.1 overlay stage in crates/vitrin-core/src/consent/mod.rs and… | A headless test measures the composited popup rectangle against the app-reported cursor rectangle within 1 px across three different realm-view offsets and two view sizes — nesting offsets being the exact failure this task exists to kill, so one offset would not discriminate | `rust-core` |
| **P2.8.5** ★ | Land `tests/integration/test_real_ime.py`: shipped `vitrind` → shipped `vitrin-shim` → **real Firefox ESR in a realm** as the committed combination the exit criterion names, with the GTK probe as the supporting rung — real fcitx5, Japanese typed by a human-principal path, candidates positioned at the app-reported cursor rect. | Real software on every seam INCLUDING the input method: fcitx5 and its engine are third-party, which gives this gate more independence than E2.5's. Headless has no physical keyboard for the human half, so a test-gated key injector stands in for the human's romaji | P2.8.4 | Green in CI, or explicitly skipped with a named reason when fcitx5 or the engine is absent — never silently passed — and registered in `tests/integration/run.sh`'s gate list. Human half: the popup's composited rectangle sits at the app-reported cursor rectangle within 1 px, and the committed text read back by the agent's own observe() and by the app's… | `rust-core` |
| **P2.8.6** | Make the effort cap structural: publish an IME compatibility matrix at `docs/book/src/ime-matrix.md` GENERATED by a runner that can only emit a cell it actually executed, with exactly one committed cell (fcitx5 + the one engine + the one toolkit) and every other cell marked untested/community | This is where the epic visibly stops, and the stopping point is enforced by a generator rather than promised in prose: no hand-editable cell exists, and CI asserts the committed-cell count is exactly 1, so widening the commitment requires running a combination and landing its evidence. IBus is a DOCUMENTED FALLBACK | P2.8.5 | `docs/book/src/ime-matrix.md` is produced by one command and is diff-clean against the checked-in copy in CI (so a hand edit fails the build); CI asserts exactly one cell carries the committed status and that the cell names the fcitx5 version, engine, toolkit and locale actually executed by test_real_ime.py in that run | `ci-docs` |

### E2.9 — Phase exit: publication, freeze, and the dated deferrals

- **Goal:** close the M2 obligations that [00-roadmap.md](00-roadmap.md) and [20-decision-log.md](20-decision-log.md) already carry but no epic owned — the benchmark the project's headline claim rests on, the core spec 1.0-candidate freeze, and the per-principal cursor delivery D-017/D-019 dated to this exact milestone.
- **Dependencies:** the measurement gates whose numbers it publishes (P2.3.7, P2.2.7, P2.2.8) and the version-2 bump it rides (P2.1.2).
- **Design decisions:**
  - **This epic adds no scope.** Every item is an existing obligation with no owner: roadmap §5 attaches the OSWorld-style benchmark and the ≥3-external-reviews metric to M2; roadmap §1 makes "core spec 1.0-candidate" M2's exit evidence; D-017 and D-019 both defer per-principal cursor delivery *to M2 in as many words*; roadmap §3 schedules Q6's evaluation during Phase 2. Recording them as an epic is what makes them closeable. Per [README.md](README.md)'s change rule, adding an epic is a substantive plan change and takes a decision-log entry.
  - The benchmark is **not** P2.2.7. Delta size compares a semantic delta against a full tree; the M2 benchmark compares Vitrin against the Xvfb + screenshot + xdotool incumbent on token cost, success rate and wall-clock (PRD §1.6). Both are needed and neither substitutes for the other — which is exactly the confusion that let the obligation go unowned.
  - The freeze follows the implementation (D-014), so it cannot precede P2.3.7's number: **a red epoch/CAS result blocks the freeze rather than being noted and shipped.**
  - One owner for the honesty surfaces. Nine tasks across the phase write into `docs/book/src/limits.md`; the sweep proves the set is complete and mutually consistent, and is a `blocked_by` on the phase closing.
- **Exit criteria:** benchmark numbers published against a real incumbent baseline with every input pinned; spec 1.0-candidate tagged with Q1/Q2/Q4 revisits recorded; `observe_cursor` served and the `ConsentGrab` shared-cursor workaround deleted rather than bypassed; every `known-limit` Phase 2 closes enumerated across all published surfaces (#172 closes); Q6 evaluated or explicitly moved to E3.1 with the reason.

**Tasks**

| ID | Task | Key decisions | Depends on | Acceptance criteria | Track |
|---|---|---|---|---|---|
| **P2.9.1** ★ | Build the OSWorld-style benchmark against the Xvfb + screenshot + xdotool baseline (PRD §1.6): a named task set, the baseline harness, and `docs/benchmarks/screenshot-baseline.md`. | This is the benchmark roadmap §5 attaches to M2 — ≥10× token-cost reduction, success rate ≥ parity, wall-clock reported. P2.2.7's delta-size number is a *different* comparison (semantic delta vs. full tree) and says nothing about tokens or task success. Both are needed; neither substitutes. | P2.3.6, P2.2.7 | N named tasks run against both stacks; token cost, success rate and wall-clock published with the Firefox pin, corpus hash and task list; CI fails when any input moved without the numbers being regenerated. | `ci-docs` |
| **P2.9.2** | Perform the core spec 1.0-candidate freeze (D-014, WS-A §2): the pre-freeze sweep of `docs/protocol/00-conventions.md` (§7.3's version prose no longer degenerates to an exact match; the scope note "Semantic trees — no accessibility/DOM-like node graph" becomes false at version 2), Q1's and Q2's empirical revisits, Q4's closure, and the logged… | D-014 freezes the spec **behind** the measured implementation, so the freeze cannot precede P2.3.7's false-reject number — a red result blocks the freeze rather than being noted and shipped. Q1's tuning pass and Q2's coverage-matrix revisit are conditions ON this task, which is why they have no separate owner. | P2.3.7, P2.2.8, P2.3.5, P2.5.6 | Spec 1.0-candidate tagged; every scope note that version 2 falsified is rewritten rather than deleted; Q1/Q2/Q4 each carry a recorded revisit; ≥3 substantive external reviews logged (roadmap §5's M2 row) or the row explicitly marked unmet rather than silently unchecked. | `protocol` |
| **P2.9.3** | Serve the D-017/D-019 M2 deferrals: `since="2"` sibling `vitrin_shim_seat` events naming the principal (each still ending with `origin`, so B2 holds), the `crates/vitrin-core/src/input/` delivery half, and `observe_cursor` served rather than resolving `unsupported`. | Both decision entries defer this to M2 in as many words, and M2 *is* this phase's exit — the cleanest orphan in the set, dated in an accepted entry rather than inferred. It rides P2.1.2's version-2 bump. It also deletes the `ConsentGrab` shared-cursor workaround D-017 promises it would. | P2.1.2, P2.3.2 | Two principals' cursors are delivered and distinguishable to the app with `origin` preserved end to end; `observe_cursor` returns a real position under grant and refuses without one; the `ConsentGrab` workaround is deleted, not bypassed; `sdk/python/tests/test_verb_parity.py` and the unserved-set catalogue test both move. | `protocol` |
| **P2.9.4** | Sweep the honesty surfaces once, at phase exit: every `known-limit` Phase 2 closes, enumerated across `docs/book/src/limits.md`, `README.md`, `NOTICE`, `crates/vitrin-core/src/spawn.rs`'s D9 section, the book's realm/shim pages and the project site. | Nine tasks across three clusters write into `limits.md` and none owns it; that is exactly how a stale gap claim ships, which is what open issue #172 exists to prevent. Individual tasks still write their own tier statements — this one proves the set is complete and mutually consistent. | P2.6.9, P2.7.6, P2.1.10, P2.8.6 | A cross-check fails when a gap is described differently on two surfaces or named on one and absent from another; #160's and #161's `known-limit` labels close with every surface enumerated; #172 closes. | `ci-docs` |
| **P2.9.5** | Evaluate Q6 (network buffer codec) during Phase 2 as the roadmap's long-lead pre-study schedules, producing a recommendation E3.1 decides on — or record that it moves wholesale to E3.1. | `20-decision-log.md` Part B and roadmap §3 both say evaluation happens during Phase 2 with the decision at E3.1 start. It is small and cheap to lose; silence is the one outcome that is not allowed. | — | A written codec comparison against the realm-view frame characteristics measured in this phase, or an accepted decision-log line moving the evaluation to E3.1 with the reason. | `ci-docs` |

---

## 3. Dependency graph, build order, walking skeleton

```
PHASE 2 — dependency graph   (★ = named mock-free gate · [M2.x] = rung closes)

CONFINEMENT TRACK (independent of the schema; starts at phase open)

  Q11 ──► P2.6.1 preflight ──► P2.6.2 empty-authority spawn (#160)
                                   │        │         │
                     ┌─────────────┘        │         │
                     ▼                      ▼         ▼
              P2.6.3 Landlock       P2.6.4 seccomp   P2.7.1 netns
                     │                                │
  P2.6.5 powerbox IDL ─► P2.6.6 picker ─► P2.6.7 relay ─► P2.6.8 Q9/Q13
  P2.7.2 egress IDL ──► P2.7.3 proxy ──► P2.7.4 DNS pin ─► P2.7.5 Q12
                     │                                │
                     └─► ★P2.6.9 ransomware   ★P2.7.6 ssh-localhost   [M2.5]

SEMANTIC SPINE (critical path)

  P2.1.1 schema (Q3 slots) ─► P2.1.2 wire v2 ─┬─► P2.1.6 core store
        freeze #1                 freeze #2   │          │
                                              │          ▼
  P2.1.3 private bus ◄── mount path ──────────┘   P2.1.7 SDK find
        │  (needs P2.6.2)                                │
        ▼                                                ▼
  P2.1.4 collector ─► P2.1.5 normalize ──────────► P2.1.8 matrix
                                        P2.1.11 fixtures ─┘
        ★P2.1.9 find-in-Firefox        ★P2.1.10 a11y-isolation      [M2.2]

  P2.2.1 identity(Q2) ─► P2.2.2 diff fmt ─► P2.2.4 per-observer deltas
        │                P2.2.3 epoch ──────────►│           │
        └──► P2.2.6 node: (#161)                 │           ▼
                                                 │    P2.2.5 SDK cache
        ★P2.2.7 delta size            ★P2.2.8 node stability        [M2.3]

  P2.3.1 policy(Q1) ─► P2.3.2 CAS wire ─► P2.3.3 chokepoint ─► P2.3.4 anim
                            └─► P2.3.5 Q4 cap        └─► P2.3.6 SDK CAS
                                          ★P2.3.7 false-reject rate

PRODUCER TRACKS (fork from P2.1.2; gates need E2.3's expected_epoch)

  P2.4.1 publish vocab ─► P2.4.2 core publish ─► P2.4.3 sidecar
                          P2.4.4 parsers ─► P2.4.5 cache ─► P2.4.6 deciles
                                          ★P2.4.7 canvas synthetic tree
  P2.5.1 native ext ─► P2.5.2 shim relay ─► P2.5.3 core accept ─► P2.5.4 app
                                          ★P2.5.5 native form-fill
                                            P2.5.6 reference client       [M2.4]

IME TRACK (independent; needs only A5 + the P1.7.1 overlay stage)

  P2.8.1 IME IDL ─► P2.8.2 text-input-v3 ─► P2.8.3 IME conn ─► P2.8.4 popup
                                          ★P2.8.5 fcitx5 ─► P2.8.6 matrix

PHASE EXIT (E2.9 — obligations already carried by the roadmap/decision log)

  ★P2.9.1 benchmark   P2.9.2 spec 1.0-cand   P2.9.3 cursor delivery
  P2.9.4 honesty sweep   P2.9.5 Q6 pre-study                          [M2.6]
```

### Parallel tracks

Phase 2 has **four** genuinely parallel tracks, not Phase 1's two.

- **Track C — confinement** (`rust-core` → `c-shim` → `ci-docs`): `Q11 → P2.6.1 → P2.6.2 → {P2.6.3, P2.6.4, P2.7.1} → powerbox/egress → ★P2.6.9, ★P2.7.6`. Consumes only A2/A3 and touches no semantic code.
- **Track A — semantic spine** (`protocol` → `rust-core` → `sdk`): `P2.1.1 → P2.1.2 → P2.1.6 → P2.1.7 → E2.2 → E2.3`. The phase's critical path, and the only track whose gates are strictly serial.
- **Track B — shim a11y** (`c-shim`): `P2.1.3 → P2.1.4 → P2.1.5`. Runs beside Track A after P2.1.2 fixes the wire, but is *blocked at its head* by Track C's P2.6.2.
- **Track D — IME** (`protocol` → `c-shim` → `rust-core` → `ci-docs`): fully independent, touching only A5's seat model and the P1.7.1/D-019 overlay compositing stage.

The two producer tracks (E2.4 sidecar, E2.5 native app) fork from Track A at P2.1.2 and rejoin only at their gates, which need E2.3's `expected_epoch`.

### Hard serialization points

Phase 1 had exactly one (the IDL freeze). Phase 2 has **three**:

1. **P2.1.1 — the node-schema freeze.** The epic doc names it the phase's critical-path decision. It gates E2.2, E2.4, E2.5 and every SDK surface. Everything Q3 needs (`confidence`, `synthetic`) and everything E2.3 needs (`in_transition`) must be a reserved slot here, or it costs a `since`-gated schema extension after golden delta vectors have shipped.
2. **P2.1.2 — the protocol 1→2 bump and every `since="2"` signature.** The sharper of the two. A signature is immutable forever ([00-conventions.md](../protocol/00-conventions.md) §7.4), so every argument any later Phase-2 epic needs must exist at this landing: `flags` on `tree_ready`, `since_version` **and** a reserved `options` word on `observe_tree` (E2.4's `accept_synthetic`), and the `observed_epoch` shape E2.4's publisher names. Miss one and that epic buys a sibling request forever. **Nothing may bump to version 3:** P2.1.2 owns the single bump for the whole phase, and E2.4–E2.8 all land at `since="2"`.
3. **Q11 → P2.6.2 — the empty-authority spawn.** Serializes E2.6, E2.7 *and* — via the in-realm bus path — E2.1's Track B behind one large, kernel-dependent change. It is the phase's biggest single schedule risk, which is why its preflight P2.6.1 must land as early as anything in the phase.

### Fully serialized validity (R8), and one inversion

The plan stays valid fully serialized for a single maintainer in rung order M2.1 → M2.6 — **with one inversion [00-roadmap.md](00-roadmap.md) §3 does not anticipate.** The roadmap says E2.6/E2.7 *may* start early because they need only A2/A3. Under this decomposition they *should* go first:

- P2.6.2 owns the mount table P2.1.3's private bus socket lives in, so building the bus first means bind-mounting it in afterwards and re-doing the confinement claim;
- P2.6.4's seccomp deny-list and P2.6.3's Landlock ruleset must be brought up against a realm that already runs `dbus-daemon` plus the dconf/gsettings machinery Firefox drags in with accessibility enabled — discovering that later means widening a policy under schedule pressure, which is how a deny-list rots;
- R2.9 (unprivileged user namespaces restricted on the target distro) is the one risk that can invalidate two whole epics, and it retires only by running the preflight on real kernels.

Recommended serial order: **P2.6.1 → P2.6.2 (+P2.6.3/P2.6.4/P2.7.1) → P2.1.1 → P2.1.2 → Track A/B to ★P2.1.9 → E2.2 → E2.3 → powerbox/egress to ★P2.6.9/★P2.7.6 → E2.4/E2.5 → E2.8 → E2.9.** Track D (IME) is the only track deferrable wholesale without stalling anything else, and it carries an explicit effort cap — so it is the correct schedule shock absorber, never the semantic spine.

One consequence no epic states on its own: **E2.1 reverses `GTK_A11Y=none` and `NO_AT_BRIDGE=1`**, pinned in `shim/docs/firefox.md`, `shim/tests/acceptance/firefox_bringup.sh`, `shim/tests/acceptance/seat_input_replay.sh` and `crates/xtask/src/main.rs` precisely because a11y activation cost ~20 s. Every existing mock-free gate that boots Firefox or GTK inherits that change. Re-running and re-timing the full `MILESTONE_GATES` set belongs inside P2.1.3's acceptance; a timeout that must move is a named `ci-docs` change with the measurement attached, not CI flake absorbed later.

### Walking skeleton

**M2.1 — a tree the agent can find a node in, with no accessibility stack anywhere.** `vitrind` runs headless serving protocol **versions 1 and 2 simultaneously**. `vitrin-mock-shim` pushes one checked-in, hand-authored tree over `push_tree`. The core stores it as one canonical tree per surface in `crates/vitrin-core/src/semantic/` and serves it through `observe_tree` → `tree_ready` at the **existing** enforcement chokepoint, copied into a fresh sealed memfd exactly as `capture.rs` does for pixels. The Python SDK deserializes it independently and `find(role="button", name="Search")` returns the node. A version-1 client on the same core still completes a Phase-1 `capture_frame` unchanged; a version-1 client sending the `observe_tree` opcode dies with fatal `invalid_opcode`.

It exercises every seam that is *new invention* — the node schema and its canonical serialization, the fd-carried transport, the 1→2 bump and the two-version matrix the core must now implement, per-surface tree storage in the TCB, `observe` as a verb over trees (so revocation, expiry, rate ceilings and `consent_held` apply with **zero new authority code**, and P1.4.4's grep-provable single-path property must survive), the flight recorder's tree digest (B1), and the SDK's second independent implementation pinned to golden vectors (D8).

It defers everything that is *known-hard integration*: AT-SPI2 and D-Bus, Firefox's ~20 s accessibility activation, user namespaces and Landlock and seccomp, the VLM parser, fcitx5, egui. Those are the five places Phase 2 can lose weeks, and none of them can teach you whether the schema, the wire and the chokepoint hold.

**It has no mock-free gate, and says so** — on exactly the M1.1 carve-out: at this rung the only tree producer in existence is `vitrin-mock-shim`, so there is nothing mock-free to *be* mock-free about. Every test on M2.1 is a component test and is labelled one in [tests/integration/README.md](../../tests/integration/README.md). **The carve-out is spent here and is unavailable to M2.2 onward.**

---

## 4. Milestones within Phase 2

`M2.1`–`M2.6` are Phase-2-internal rungs, exactly as `M1.1`–`M1.5` were Phase-1-internal. The roadmap defines only the external **M2**, which M2.6 is.

| Milestone | Statement of done | Contains | Named exit gate |
|---|---|---|---|
| **M2.1 — "Tree on the wire"** (walking skeleton) | `vitrind` serves versions 1 and 2 simultaneously; a hand-authored tree pushed over `push_tree` is stored per surface, served through `observe_tree` at the existing chokepoint into a fresh sealed memfd, deserialized independently by the Python SDK, and `find(role, name)` returns the node; a v1 client still completes a Phase-1 `capture_frame` unchanged | P2.1.1, P2.1.2, P2.1.6, P2.1.7 (partial) | **none, deliberately** — walking skeleton, on the M1.1 carve-out (see §3) |
| **M2.2 — "Agent finds a real element in real Firefox"** | Every shim surface carries a live, normalized tree from the confined app's own accessibility stack over a private per-shim bus; `find(role, name)` locates a named element in real Firefox ESR and the node is proved to *be* that element by actuating at its geometry and reading the app's own visible response; the four-app coverage matrix is generated (not written); no route from inside a realm to the host session a11y bus is reachable | P2.1.3–P2.1.5, P2.1.8, P2.1.11, remainder of P2.1.6/P2.1.7 | **★P2.1.9** (`test_real_semantic_firefox.py`) **+ ★P2.1.10** (`test_real_a11y_isolation.py`) |
| **M2.3 — "Versioned trees: KB deltas and addresses that do not lie"** | Tree updates are atomic and epoch-stamped over one monotonic per-view counter; per-observer deltas are served against `since_version` with enumerated normative resync triggers; a held node reference across a real page's dynamic updates either resolves to the same element or raises `NodeInvalidated`, with silent re-binding measured at exactly zero; median delta size over a pinned corpus is published with its full-tree baseline and ratio | P2.2.1–P2.2.6 | **★P2.2.8** (`test_real_node_stability.py`, correctness) **+ ★P2.2.7** (`test_real_semantic_delta_size.py`, measurement) |
| **M2.4 — "Race-free action, one node model from three producers"** | Observation returns an epoch; node-targeted actuation carries `expected_epoch`; the single chokepoint rejects stale targets with `stale` before anything is acted on; animated nodes refuse with `retry_after_epoch`; false accepts are zero and the false-reject rate is measured against a threshold stated *before* the run; PRD Doc 2 §18 steps 4–5 run verbatim as checked-in executable code; a canvas-only surface and a native egui app both yield trees agents reach through the **same** `find()` + node-addressed actuation | P2.3.1–P2.3.6, P2.4.1–P2.4.6, P2.5.1–P2.5.4, P2.5.6 | **★P2.3.7** (`test_real_epoch_cas.py`, primary) **+ ★P2.4.7** (`test_real_synthetic_tree.py`) **+ ★P2.5.5** (`test_real_native_tree.py`) |
| **M2.5 — "The non-ambient realm"** | A realm spawns with a user+mount+PID+IPC+UTS+net namespace set, a Landlock ruleset and a seccomp filter, at an isolation tier the core measured and refuses to start below; the picker returns already-open fds resolved race-free from a directory fd; a payload realm's measured write set equals exactly {designated fds} ∪ {realm private storage} against attempted home-directory reach, path races and picker spoofing; host loopback unreachable, abstract sockets confined, path sockets absent, a grantless realm emits zero outbound packets by capture, and one designated `host:443` egress works, expires and revokes with both latencies measured | P2.6.1–P2.6.10, P2.7.1–P2.7.5 | **★P2.6.9** (`test_real_ransomware.py`) **+ ★P2.7.6** (`test_real_ssh_localhost.py`) |
| **M2.6 — "IME reference combination + phase exit"** (= roadmap **M2**) | Agent `text` entry into a CJK-locale app works with the IME running and provably untouched (zero bytes across `vitrin_shim_text_input`, with the same counter shown nonzero in the same run); a human types Japanese into **Firefox**-in-realm via real fcitx5 with candidates positioned at the app-reported cursor rect within 1 px across multiple realm-view offsets; the effort cap is structural. **And** the publication obligations are met: benchmark-vs-screenshot numbers, delta-size and false-reject numbers, the core spec 1.0-candidate freeze, and the D-017/D-019 cursor deferrals served | P2.8.1–P2.8.6, P2.9.1–P2.9.5 | **★P2.8.5** (`test_real_ime.py`, IME half) **+ ★P2.9.1** (benchmark half) |

### The definition-of-done rule, restated for Phase 2

[01-phase-1-mvp.md](01-phase-1-mvp.md) §5's **D12** carries forward unchanged: a rung closes only when its named gate passes green with **no mock on any seam that rung claims**, and `vitrin-mock-shim` / `shim/tests/mock_core.c` may never be the evidence source. Phase 2 adds seams Phase 1 did not have, and each is a new place a mock can hide:

| New seam | What must be real |
|---|---|
| App → accessibility stack | The app's own AT-SPI2 implementation. A repo-authored fixture that emits a hand-built tree is a component test, never a gate — the whole claim is that *unmodified* apps expose structure. |
| Shim → private bus | A real `dbus-daemon` in the realm. Asserting the host bus is "not advertised" is the exact D9-shaped error #160 exists to close; P2.1.10 must attempt reachability and carry a **positive control** proving the attempt would have succeeded unconfined. |
| Core → VLM sidecar | A real out-of-process sidecar over the same `observe` path an agent uses. A parser called in-process would prove nothing about the TCB boundary the design rests on. |
| Realm → kernel confinement | Real namespaces, real Landlock, real seccomp, on a stated kernel. M2.5 is almost entirely *absences*, so **every negative claim needs a positive control in the same run** — an obligation Phase 1's gates did not carry. |
| Realm → egress proxy | A real proxy process and real packet capture. "No grant, zero packets" is a capture assertion, not a code-path assertion. |
| Human → IME | Real fcitx5. A synthesized `text-input-v3` sequence tests the shim, not the IME path. |

Two further obligations, both inherited from the P1.9.8 gate-integrity pass: a gate must have been **watched failing** on constructed breakages before it may be cited, and where a gate asserts on repo-authored content inside third-party software (R2.7), that reduction in independence is stated in the gate's own docstring rather than left for a reader to find.

---

## 5. Phase-2 allocation registry and decision gates

Three clusters decomposing in parallel independently claimed the same verb bit, the same prose-page number and the same message name — each an **immutable** choice once landed ([00-conventions.md](../protocol/00-conventions.md) §7.4, and D-017's note that `deprecated-since` marks but never removes). The allocations below are therefore made **once, here, before any of the tasks open**, and belong in a `20-decision-log.md` entry rather than in whichever task lands first.

**Verb bits** (`Verb::VALID_MASK` is **575** today — `1|2|4|8|16|32|512`, the six original bits plus `realm_launch`; it read 63 until WS-E.1.1 landed the 512 row below). Re-pin the mask **once per epic**, never once per task, in its three sites: `crates/vitrin-protocol/tests/decode_errors.rs`, `crates/vitrin-core/src/consent/render.rs`'s unserved-set catalogue test, and `sdk/python/tests/test_verb_parity.py`.

**Serving an already-allocated bit allocates nothing and still moves two of those three.** WS-E.1.4 served `layout_arrange` (16) and `layout_focus` (32) without adding a bit, so `VALID_MASK` did not move and `decode_errors.rs` did not fire — but the served/unserved *split* did move, which turned the catalogue test's unserved-set pin and `test_verb_parity.py`'s served-set constant red. That is the tripwire pair working with no new allocation: a verb cannot become served without a human classifying it and writing its consent-prompt copy.

| Bit | Verb | Epic | Task |
|---|---|---|---|
| 64 | `designate_file` | E2.6 (earliest-starting) | P2.6.5 |
| 128 | `egress` | E2.7 | P2.7.2 |
| 256 | `publish_tree` | E2.4 | P2.4.1 |
| 512 | `realm_launch` | **WS-E** ([14-workstream-session-mode.md](14-workstream-session-mode.md)) | WS-E.1.1 (#207) |

**The registry is repo-wide, not Phase-2-only.** The 512 row is the proof: WS-E is
a workstream, not a Phase-2 epic, and its first task drafted `realm_launch` at
**64** — already `designate_file`'s. A verb value is immutable once landed
([00-conventions.md](../protocol/00-conventions.md) §7.4), so a parallel
workstream allocating against a stale reading of the IDL is exactly the collision
this table exists to stop, and it caught one. Anything that adds a verb bit
allocates it here first, whatever document schedules the work.

**The 1→2 version bump is owned by whoever lands first, not by P2.1.2 by name.**
§3's second serialization point says P2.1.2 owns "the single bump for the whole
phase". WS-E.1.1 needs version 2 to carry a new request and may land before Track
A opens. If it does, it performs the bump and P2.1.2 rides it; the invariant that
actually matters is unchanged — **one bump, and every later addition at
`since="2"`** — because the "everything at once" rule binds the *arguments within
a signature*, which are immutable, not the calendar order of two additive
landings. Nothing may bump to version 3.

**Prose pages.** When this table was written `docs/protocol/` ended at
`11-vitrin_shim_seat.md` and three tasks had each claimed 12 — the collision it
exists to stop. WS-E.1.1 then landed page **16**, skipping the four numbers
allocated below rather than taking one, which is the rule working: 12–15 stay
reserved for tasks that have not opened yet. WS-E.1.4 landed **17** and **18**
on the same rule, so `docs/protocol/` now ends at
`18-vitrin_layout_arrange.md` with the same deliberate gap. **Two** pages
rather than one, because `interface/@verb` is one value per interface and the
two layout verbs must stay independently attenuable (D-022(2)) — a row that
had anticipated a single "layout facet" would have understated it.

| Page | Content | Task |
|---|---|---|
| `12-semantic-nodes.md` | Node schema + `vitrin_tree_publisher` folded in as a second interface | P2.1.1 / P2.4.1 |
| `13-vitrin_powerbox.md` | Powerbox facet | P2.6.5 |
| `14-vitrin_shim_text_input.md` | IME | P2.8.1 |
| `15-vitrin-semantic-v1.md` | Native app-facing extension | P2.5.1 |
| `16-vitrin_launcher.md` | Realm-launch facet (**landed**) | WS-E.1.1 (#207) |
| `17-vitrin_layout_focus.md` | Focus facet (**landed**) | WS-E.1.4 (#210) |
| `18-vitrin_layout_arrange.md` | Arrangement facet (**landed**) | WS-E.1.4 (#210) |

**Two rules the merge had to settle, both normative for the phase:**

- **One epoch, one definition.** One monotonic counter **per view**, defined by D-018(5) over (frame content + view geometry + stacking), stamped by the core — not a correlated frame/tree pair. Consequences that must be written into tasks rather than left inferable: a synthetic publisher's `observed_epoch` names that counter; a native app never sees an epoch and never re-pushes on a geometry change (the core re-stamps the stored tree); and the consent card, the picker and the trust band — human-visible output only, never in a capture — **must not bump it**, or a picker raised for an unrelated realm invalidates every agent's cached epoch and contaminates P2.3.7's false-reject measurement with the powerbox.
- **A gate's track is the track that owns the capability the gate proves**, not the track that owns `tests/integration/`. Phase 1's precedent is mixed (#105 `ci-docs`, #107/#108/#110 `sdk`, #109 `rust-core`), so the rule is stated rather than inferred. `ci-docs` keeps the harness, `run.sh`'s gate lists, and the published-number pages.

**Decision gates.** [00-roadmap.md](00-roadmap.md) §4's decide-by column, resolved to owning tasks:

| Q | Decide-by | Owner |
|---|---|---|
| Q11 realm vs. Unix-user boundary | **Phase 2 start** | P2.6.1 — and it must answer three things, not one: the realm's UID model; that the private a11y bus is a first-class member of the realm's private resource set; and that host-level sidecars (VLM parser, egress proxy) are outside every realm and therefore outside every realm's confinement. **That last clause has a consequence nobody else owns: the VLM sidecar has unmediated host network access unless something says otherwise, which makes E2.7's headline claim locally true and globally false if left unstated.** |
| Q2 node addressing | v0 at E2.2 start; revisit at M2 | P2.2.1 (v0) → P2.9.2 (revisit) |
| Q1 epoch granularity | v0 at E2.3 start; tuned before spec 1.0-cand | P2.3.1 (v0) → P2.3.7 (measure) → P2.9.2 (tune) |
| Q3 VLM confidence | E2.4 design; field frozen at spec 1.0-cand | P2.1.1 (reserved slots) → P2.4.1 (allocate) |
| Q4 delegation depth | before spec 1.0-candidate | P2.3.5 |
| Q9 standing-grant ergonomics | v0 at E2.6 | P2.6.8 (negative half) + P2.6.10 (positive half) |
| Q12 egress ergonomics | v0 at E2.7; full by M3 | P2.7.5 |
| Q13 consent-ladder human factors | review at E2.6 | P2.6.8, widened to **every verb Phase 2 serves** — each new verb ships admitted-but-refused `unsupported` until its copy exists, the staging `observe_cursor` still uses and `layout_*` used until WS-E.1.4 served both |
| Q6 network codec | evaluated during Phase 2 | P2.9.5 |

---

## 6. Phase-2 risks

Phase-1 risk IDs `R1`–`R8` are not reused; these are `R2.n`.

| ID | Risk | Mitigation |
|---|---|---|
| **R2.1** | **AT-SPI2 is the path we are trying to leave, and the path Phase 2 runs on.** AccessKit has no consumption transport for third-party apps yet (that is Newton, unfinalized), so "AccessKit bridge" in Phase 2 means AccessKit's *schema* over AT-SPI2's D-Bus. | P2.1.1 states the split so no document implies a transport we do not have; the collector is signal-driven so D-Bus cost never lands on an agent's `observe`; the schema is version-pinned and tracked in the WS-A liaison table. Residual: if Newton finalizes differently, the normalizer changes and the golden vectors regenerate — bounded, not free. |
| **R2.2** | **The KB-scale delta claim is a property of the differ, not the source.** AT-SPI change signals are per-property; a naive collector produces near-full-tree churn on any structural change, and an SPA-heavy corpus could measure MB-scale medians — falsifying the PRD's headline claim in public. | The hybrid structural+patch format (P2.2.2) exists for the re-render case; P2.2.7 measures on a pinned reproducible corpus; and the failure posture is written down in advance — **publish the number and the cause, never relax the threshold after seeing it.** |
| **R2.3** | **Epoch/CAS may not clear its own threshold.** The PRD calls it a design claim; an animation-heavy real app is exactly where a per-node invalidation policy over-fires. | P2.3.1's per-class counters make a miss attributable rather than mysterious; the two directions carry different thresholds (false accepts **zero**, false rejects a budget) because they are different kinds of wrong; D-014 already blocks a spec freeze on a red result rather than shipping behind one. |
| **R2.4** | **New structural code in the TCB, against R7.** Phase 2 adds a tree store, a per-observer differ, an epoch counter and node re-identification to `vitrin-core`. | None of it parses an application format (the shim normalizes); the serialization is a fixed-layout blob with a round-trip property test rather than a parser; the `cargo-deny` budget is re-reviewed at the epic boundary; and the retained-history bound is a stated byte budget, so a client cannot grow the core by never observing. |
| **R2.5** | **Node ids become authority.** Once `node:` is a served grant resource, a wrong re-identification is an **authority redirection**, not an ergonomics bug — a grant scoped to `name` silently covering `card number`. | Identity is core-owned and never shim-supplied; ambiguous matches fail closed by invalidating rather than picking; the false-identification count is asserted as an exact **zero** in both the offline corpus and the real-app gate; the node-scoped grant test includes a re-identification-drift case at the authority boundary. |
| **R2.6** | **The private bus adds a process to the realm before the realm is confined**, if E2.1 runs before E2.6. | The build order inverts to put Track C first (§3). Where the tiers still overlap, P2.1.10's claim publishes **as a tier** — `limits.md` names which routes are closed structurally and which only by environment hygiene — and the weaker tier is retired *inside* E2.6, never quietly upgraded. |
| **R2.7** | **Three repo-authored gate artifacts in one phase.** E2.2/E2.3's gates rest on repo-authored page content inside third-party Firefox; E2.5's demo app and E2.4's canvas fixture are ours too. M1.5 already conceded this shape once for `form-target`; doing it three more times erodes the mitigation that made the first acceptable. | State it in each gate's own docstring rather than letting a reader find it; keep Firefox, the shim, the transport and the chokepoint third-party or shipped; keep E2.1's third-party a11y rungs green in CI; and require a receipt-frozen breakage in each gate's watched-failing list, so a page that lies by not updating turns the gate **red** instead of passing it. |
| **R2.8** | **E2.1's environment change perturbs every Phase-1 gate** by reversing `GTK_A11Y=none`/`NO_AT_BRIDGE=1`, which exist because a11y activation cost ~20 s. | Re-run and re-time the full `MILESTONE_GATES` set inside P2.1.3 and publish the new figure; a timeout that must move moves as a named `ci-docs` change with the measurement attached, never as absorbed CI flake. |
| **R2.9** | **Unprivileged user namespaces are restricted or disabled on major distros.** No userns → no netns → no confinement at all. This is the one risk that can invalidate two whole epics. | P2.6.1's preflight runs on real kernels as early as anything in the phase; the core measures its tier and **refuses to start below a floor** rather than degrading silently; the per-kernel tier matrix is generated, not asserted. |
| **R2.10** | **A delivered fd is authority the core cannot recall**, so PRD P2's "revocation is immediate and transitive" is false for designations already made. | Stated as an exported limitation on C5 rather than discovered by E3.7; revocation kills the grant row and stops future designations; the residue ends when the realm does. |
| **R2.11** | **The synthetic tree is an unmodelled cross-principal channel.** The sidecar (principal A) publishes structure derived from a realm; an agent (principal B) reads it. The grant table models A→realm and B→realm, but nothing models A→B. | Attribution on every stored tree, the `accept_synthetic` opt-in defaulted off, and P2.4.2's journal entry — plus a plain statement in `limits.md` that cross-principal flow through the tree store is **unmodelled in v0** and is a decide-by-M3 item, rather than letting the attribution field imply it is solved. |
| **R2.12** | **The IME is a keylogger by construction.** It sees every keystroke destined for the focused realm — a genuine extension of the trust boundary beyond the TCB that no capability in this design constrains. | Stated as a trust statement on C7, in PRD Doc 2 §15's threat model and in `limits.md`. It is not mitigated by the architecture and must not be described as if it were. |

---

## 7. Limits this phase creates, and where they are published

Phase 2 is the phase that makes a realm *actually* confined, which is exactly
when a reader starts trusting the word. Each limit below is a published
`known-limit` this phase **creates**, not one it inherits — and each is
enumerated here rather than in
[`14-workstream-session-mode.md`](14-workstream-session-mode.md) §6, which is
titled *"Limits this workstream creates"* and scopes every entry to a limit
*"this workstream owns, not inherits"*. Filing a confinement limit under WS-E
would send the next sweep that closes it to the wrong document's surface
table, which is the failure an enumeration exists to prevent.

### The surfaces, enumerated once

`CLAUDE.md`'s `known-limit` rule ("enumerate every surface when closing one")
needs the set of surfaces to exist in one place per body of work. Here is
Phase 2's. It is the only copy.

| Surface | Register it is written in | What it carries | What holds it |
|---|---|---|---|
| [`docs/book/src/limits.md`](../book/src/limits.md) | An argument, at length, hard on this project. **This is the governing surface**: where it and any other disagree, it wins. | Every limit in this section, with its evidence, the bound on that evidence, and what it does *not* claim. | `cargo xtask limits-check` — the anchored claims, **and the set cross-check against this section** — plus human review |
| [`README.md`](../../README.md), §"Security notes" | A contributor's summary — dense, linked, no argument. Sits with the other confinement bullets, because a reader who accepted those will otherwise assume the thing starts. | One bullet, linking onward. | `cargo xtask limits-check`, anchored claims only |
| [`SECURITY.md`](../../SECURITY.md), §"Known gaps that are not findings" | A reporter's triage list. | That a refusal to start is designed behaviour and not a vulnerability, so nobody spends a weekend on it. | `cargo xtask limits-check`, anchored claims only |
| [`site/index.html`](../../site/index.html) | A landing page's warning, in a reader's words. | Both host requirements with the one-runner bound on the measured one, **and** the nested-sandbox cost — a reader deciding whether to run this is entitled to know that an app's own sandbox stops working inside a realm, which is the one item here that changes what running it *does* rather than whether it starts. Nothing else. | `cargo xtask limits-check` — the two host-requirement claims only; the nested-sandbox paragraph is **not anchored** on this surface and is held by review |
| [`docs/book/src/01-run-the-demo.md`](../book/src/01-run-the-demo.md) | A quickstart. | The symptom a first-time reader will actually hit, in the "If it fails" table. | Human review only — **not anchored** |
| [`docs/book/src/04-realms-and-shims.md`](../book/src/04-realms-and-shims.md) | A concept page. | Both host requirements, where it says what confines a realm today; and the nested-sandbox cost under "what does *not* confine a realm", which is where a reader learns that an app's own sandbox stops being one inside a realm. | Human review only — **not anchored** |
| [`tests/integration/README.md`](../../tests/integration/README.md) | A harness contract. | What a contributor must have before the confinement gates can run at all. | Human review only — **not anchored** |
| [`NOTICE`](../../NOTICE) | Normative path→license map. | **Nothing from this section.** Checked: no limit here is licensing-relevant and no file moved across a license boundary. Named so the next sweep does not have to re-derive that it is out of scope. | — |

**A claim that is not in `limits-check`'s table is not held**, and every "not
anchored" mark above says so rather than leaving a reader to assume coverage.
Three surfaces are unheld entirely — the quickstart, the concept page and the
harness contract — and one is held in part: `site/index.html`'s two
host-requirement paragraphs carry anchors and its nested-sandbox paragraph does
not, because `landlock-breaks-nested-image-sandboxes` has no row in
`limits-check`'s claim table on any surface. All of it is published because a
reader meets it there; the unanchored part is held by review, which is weaker,
and that is the honest description.

### The mechanism, and why this section exists at all

`cargo xtask limits-check` compares the set of limits enumerated by the plan
documents against the set published on `docs/book/src/limits.md` — **the set,
never the wording**, since the two registers are deliberately different. Each
limit carries an invisible marker comment holding a kebab-case id, here and
beside the matching entry on the limits page; a pair of comments reading
`limit-set: begin` and `limit-set: end` bounds the set below. Inside it, every
top-level bullet carries either `limit: <id>` (the limits page publishes it
under that id) or `limit-not-on-page: <id> -- why` (it deliberately does not,
and here is the reason in writing). The delimiters are written here without
their comment brackets on purpose: the gate reads this file, and a literal
delimiter in prose would move where it thinks the set starts.

The full argument for the mechanism is in
[`14-workstream-session-mode.md`](14-workstream-session-mode.md) §6, which
built it. What is new here is only that **there is more than one enumerating
document**, and the gate holds three further rules because of it: a registered
document must enumerate at least one limit (a region added to make a marker
legal is refused), an id is declared by exactly one document (two claims on one
limit is how a sweep closes it in one place and leaves the other standing), and
the every-bullet rule runs over this section exactly as it does over §6.
`crates/xtask/src/limits.rs`'s `ENUMERATORS` carries that argument in full,
including what a carve-out here would have looked like and why this is not one.

The limit set follows.

<!-- limit-set: begin -->

- <!-- limit: host-must-permit-unprivileged-userns -->
  **A host must let an unprivileged user namespace actually carry its
  capabilities, or `vitrind --isolation=default` refuses to start** — and one
  measured mainstream image does not. What D-020/D-037 build is an
  unprivileged `CLONE_NEWUSER` plus a mount namespace inside it; a host may
  permit the `unshare` and still strip the capabilities that namespace is
  supposed to confer, which is why the startup preflight probes the mount and
  not only the unshare — the two answers differ on such a host. Where its
  `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)` fails, the session is
  refused before any realm is spawned. **The refusal is correct** — D-020(6)
  forbids silent degradation, and the message already names the knobs this core reads. The
  limit is that until #286 nothing published said a host might need to be
  granted anything at all. **Evidence: one data point.** A GitHub
  `ubuntu-latest` runner, kernel `6.17.0-1020-azure`, measured 2026-08-14:
  `kernel.apparmor_restrict_unprivileged_userns=1` on that stock image, read
  from the runner's own sysctl before CI granted itself the remedy,
  AppArmor permits the unshare and then confines the process to a profile that
  denies the capabilities inside it, the first mount answers `EACCES`, and the
  matrix reads `ns.all=available`,
  `mount.in_userns=restricted-by-policy(errno=13)`, `tier=none`. That is one
  CI image on one kernel on one date and **not a distribution survey**; #281
  owns the survey and collects it from the diagnostic step that runs *before*
  CI grants itself the remedy. Packaging that makes the grant routine — an
  AppArmor profile attempted first and CI-gated — is #286; the wording on every
  surface describes the **requirement** rather than a blessed remedy, so it
  does not have to be rewritten when that lands. `--isolation=off` is not the
  answer: it starts an unconfined session and every confinement claim stops
  applying to it.

- <!-- limit: landlock-breaks-nested-image-sandboxes -->
  **A Landlock domain denies every mount, so a nested sandbox cannot be built
  inside a realm — and an app whose image decoding runs in one therefore
  decodes UNSANDBOXED.** The mechanism is not an access right and no rule
  reaches it: `security/landlock`'s mount hooks refuse for any process in a
  domain. Measured on this repo's box (Arch, kernel `7.1.8-arch1-3`, ABI 9,
  2026-08-15) with the granted rights held constant at *everything on `/`* and
  only the handled mask varied — `handled_access_fs = 0` returns 0 from
  `mount(MS_REC|MS_SLAVE, "/")`, `EXECUTE` alone returns `EPERM`, the full
  rung-9 mask with every right granted on `/` still returns `EPERM`.
  **Widening cannot repair it**, and that too is measured: a domain granting
  *every* rung-9 right on `/` — more than the enumerated read set, and more
  than the realm-root grant #187 declined — fails the identical decode.
  **What this bullet published until 2026-08-15 and no longer does**: that the
  shipped default takes `test_real_actuation.py`'s typing rung (#108, M1.4's
  actuation half), `test_real_gtk.py` and `test_real_firefox.py` red. Since
  `vitrin-realm-init` writes `0` to the realm's own
  `/proc/sys/user/max_user_namespaces` (K9b), `bwrap` fails at
  `unshare(CLONE_NEWUSER)` with a message `glycin` recognises instead of at its
  first `mount(2)` with one it does not, `glycin` takes the fallback it already
  ships, and all three gates pass at the shipped default — re-measured on the
  same box, 2026-08-15. That is a change in *which error* a nested sandbox
  receives, not a capability restored: mounting is still denied, so the decode
  still runs with no sandbox around it, which is what the entry now publishes.
  **Which hosts it bites was measured on one point per side and no more**: this
  box's `gdk-pixbuf 2.44.7` has one in-process loader and both `glycin` and
  `bwrap` installed; an `ubuntu:24.04` container carrying what
  `shim/ci/install-deps.sh` installs has `libpixbufloader-svg.so` in process,
  no `libglycin`, and no `bwrap`. No gate was run inside that container, so
  nothing claims a CI result. No gate is skipped and no app is exempted; the
  refusal of nested user namespaces applies to every realm at the shipped
  default. `VITRIN_LANDLOCK=off bash tests/integration/run.sh` remains the
  no-ruleset control — announcing itself as a control, since it is evidence for
  no milestone.

- <!-- limit: host-must-have-landlock -->
  **The host must actually have Landlock, or `--isolation=default` refuses to
  start.** P2.6.3 put `Mechanism::Landlock` in `spawn/isolation.rs`'s `FLOOR`,
  which is a **startup behaviour change by design**: a kernel answering the ABI
  query with `ENOSYS` used to start a session confined by mount table alone and
  now refuses, because D-020(6) forbids a session whose realms are confined one
  mechanism less than its own journal says. Three host facts are required, and
  the refusal names all three in the order worth checking: kernel ≥ 5.13
  (Landlock's arrival), `CONFIG_SECURITY_LANDLOCK=y`, and `landlock` present in
  the active LSM list — a kernel can carry the code and leave it out of `lsm=`.
  `vitrind --print-isolation` answers all three as `landlock.abi=N` without
  spawning anything. **A fourth condition arrived with the ABI floor** (owner's
  decision, 2026-08-15, "P2.6.3, corrected" above): the reported ABI must be at
  or above `build.landlock_min_abi` (**6**), and this is the one condition a
  correctly configured, working Landlock can still fail. Its remedy is different
  from the other three — nothing is misconfigured, no knob moves the number, the
  answer is a newer kernel — so the refusal renders as
  `below-floor(abi=N,required=M)` and every surface carries that discriminator
  rather than repeating the three checks. **Nobody here has surveyed which
  distributions ship the
  third unset, or a kernel below the fourth**; #281 owns that survey, the same
  way it owns the userns one
  above. One distribution *is* named on the surfaces, and only under a stated
  bound: Ubuntu 24.04's 6.8-series kernel is well past 5.13 and, by Landlock's
  mainline ABI history, reports ABI 4 — which the floor refuses — published as
  arithmetic over release
  notes rather than as a measurement, because no Ubuntu machine has been asked
  here and conditions (2), (3) and (4) are facts no version implies. It is
  there for one purpose: to show the two host requirements are **independent**,
  which is the thing an operator gets wrong. Both refusals stop the same
  command and look alike, so every surface carries the same discriminator — the
  refusal names the mechanism it could not get (`namespaces` or `landlock`) —
  and says the remedies do not substitute: no userns sysctl makes a kernel
  report a Landlock ABI, and adding `landlock` to `lsm=` restores no capability
  a user namespace was stripped of. `--landlock=off` is not the remedy for a
  configurable kernel either: it builds no ruleset, so every published claim
  about the read set, the write set and the rung ladder stops applying to that
  session.

- <!-- limit: seccomp-is-a-deny-list -->
  **The seccomp filter is a deny-list, so "seccomp" here is a named-class claim
  and never a completeness one.** P2.6.4 put `Mechanism::Seccomp` and
  `Mechanism::NoNewPrivs` in `spawn/isolation.rs`'s `FLOOR` — the third and
  last scheduled floor move, after which `FLOOR` *equals* `Report::tier`'s base
  predicate — so a kernel without `CONFIG_SECCOMP_FILTER` now refuses
  `--isolation=default`, and there is deliberately no `--seccomp=off` to waive
  it with. What the filter closes is the row set `vitrind --print-seccomp`
  prints; what it leaves open is everything else, **unenumerated**. An
  allow-list is the right long-term shape and belongs to a phase that has a
  measured syscall trace of the acceptance apps to build one from; built now it
  would fail closed against Firefox, which is this project's own standing
  acceptance app. Three things the plan must not let a later reader
  over-read. **First**, of PRD Doc 2 §15's eight actor rows the filter answers
  two (*Compromised shim*, *Malicious app in a shim*) and part of a third
  (*Reachable-service lateral escape*) — and that third only at two services
  §4.5's own enumeration missed, the kernel keyring and `AF_VSOCK`, both
  measured reachable from inside a realm on 2026-08-16. §4.5's "there is simply
  nothing to reach" sentence has been corrected in the PRD rather than left
  standing. **Second**, whether a given row is *demonstrated* is a property of
  the kernel and not of the table: `bpf` and `userfaultfd` are already denied by
  `kernel.unprivileged_bpf_disabled` and `vm.unprivileged_userfaultfd` on a
  hardened box, so `tests/integration/test_real_seccomp.py` runs a positive
  control per row and reports those as *not demonstrated* rather than counting
  them — 11 of the 13 denied syscall rows demonstrated on the box this was
  measured on. **Third**, the
  table costs two things that are published rather than implied: a realm cannot
  execute a foreign-ABI (32-bit) binary, because syscall numbers are per-ABI and
  the filter kills a foreign `arch` rather than passing it unfiltered; and the
  `ptrace` row breaks the pinned Firefox's crash reporter, a path
  `test_real_firefox.py` does not exercise because it sets
  `MOZ_CRASHREPORTER_DISABLE=1` — so that gate's green tick is not evidence for
  that row, and the limits page says so.

<!-- limit-set: end -->
