# Phase 3 — Network + X11 + fleet

**Phase goal:** the fleet-and-remote phase — v1's headline deployment ([PRD](../PRD.md) user stories 1, 4, 5, 7, 8): authenticated remote sessions, legacy X11 apps in realms, N-realm headless boxes, replayable journals, wallet v0, and the first unprivileged supervision shell.

**Consumes:** Phase-2 exits (semantic chain, powerbox, network authority) plus Phase-1 artifacts A4 (flight recorder → signed journal) and A6 (headless mode → fleet).

**Phase exit = milestone M3** ([00-roadmap.md](00-roadmap.md)): 50-realm headless box, remote QUIC principal, X11 app in a realm, journal replay, wallet v0, mission-control shell v0.

**Internal dependency structure:** E3.1, E3.2, and E3.6 can start independently; E3.3 needs E3.1 + E3.2 late; E3.7 has the longest external-standards lead time (its tracking starts during Phase 2 via the WS-A liaison table); E3.8 consumes everything — it is the phase's integrating demo.

---

## E3.1 — QUIC network sessions

- **Goal:** authenticated, multiplexed, capability-scoped remote sessions; workload identity (SVID/OIDC) bound to the TLS channel; capability handles established over the authenticated session; reconnection renegotiates dynamic state rather than losing it (PRD P9, Doc 2 §10).
- **Dependencies:** A1 protocol (the network profile of the same object model — the spec must state which semantics are transport-invariant → WS-A); E2.2 (tree diffs are the payload that makes remote sessions cheap).
- **Design decisions:** quinn-based transport (D-004); control-plane serialization — Cap'n Proto optional per D-003, decided here; **Q6** buffer codec for non-dmabuf sinks (evaluated during Phase 2, decided at epic start; the Arcan-a12 lesson "pixels as a last resort" is the prior art); connection-migration semantics for roaming agents; sender-constraint mapping for a channel that isn't a Unix socket (TLS channel binding replaces `SO_PEERCRED`).
- **Exit criteria:** PRD user story 5 — an agent on machine A drives a realm on machine B: identity bound at the transport, epoch/CAS intact end to end, server-side motion synthesis masking latency (measured); reconnect resumes without grant re-consent (restore-token path).

## E3.2 — X11 shim + embedded WM

- **Goal:** a per-app rootless X server with a minimal window manager *inside* the shim; X legacy fully outside the core (PRD P3, Doc 2 §4.3).
- **Dependencies:** A5 shim architecture as the template; E2.1 bridge (AT-SPI2 is the semantic source for X apps); E2.8's documented XWayland-IME fallback.
- **Design decisions:** Xwayland-derived fork vs. driving stock Xwayland rootless with a companion WM process inside the same sandbox — **recommend the latter first** (less fork maintenance); a published, closed-ended minimal `_NET_WM_*` coverage list; gamescope's per-game-Xwayland pattern as prior art.
- **Exit criteria:** a legacy X app runs in a realm with correct map/focus semantics; **the anti-keylog test:** two X apps in two realms provably cannot observe each other's windows or input — the shared-XWayland hole (PRD Doc 2 §4.1) closed and demonstrated.

## E3.3 — Headless multi-realm fleet

- **Goal:** N-realm headless operation (PRD user story 1's 50-realm box): virtual output + framebuffer per realm, resource accounting, and an SSH front-end terminating certificate principals into scoped protocol sessions — never a PTY with ambient authority (PRD Doc 2 §5.1).
- **Dependencies:** A6 headless mode; E3.1 (remote principals are how fleets are actually used); E2.6/E2.7 (realm density presumes cheap hardened namespaces).
- **Design decisions:** SSH front-end shape — OpenSSH `ForceCommand` bridge first (smaller attack surface, faster to ship), a russh-based front-end later if needed; realm lifecycle API (create/suspend/destroy — the seed of a fleet control plane, but no hosted control plane per PRD §10); per-realm resource ceilings (cgroups) and a stated memory budget for the density target.
- **Exit criteria:** 50 realms on one reference box with measured per-realm memory/CPU overhead published (feeds the PRD §7 benchmark metric); PRD user story 8 — an SSH certificate principal gets exactly its granted realms, and `ssh localhost` inside any realm is inert; realm-7-cannot-see-realm-8 isolation re-verified at density.

## E3.4 — Journal replay + training export

- **Goal:** the signed P6 journal proper, deterministic replay of what each principal observed and did (frame by frame), and trajectory export as agent training data (PRD user story 4).
- **Dependencies:** A4 — Phase 1's flight recorder with backward requirement B1 honored (observation digests + epoch-ready fields from day one; replay is unbuildable retroactively otherwise).
- **Design decisions:** replay fidelity — journal-of-record replay (replay what was *observed*; recommended for v1) vs. bit-exact re-composition (re-executing apps; rejected for v1); export schema aligned with what agent-training pipelines actually consume (OSWorld-style trajectory format as the reference); signature-chain verification tooling.
- **Exit criteria:** post-incident replay of a multi-principal session reconstructs the observe/act sequence with verified signatures; one session's export is consumed by an external fine-tuning/eval pipeline as a demonstration.

## E3.5 — Cross-realm mediators hardened

- **Goal:** the clipboard broker (two explicit gestures, Qubes model), drag-and-drop as designation, window activation, and a portal-compat layer proxying FileChooser/ScreenCast/RemoteDesktop into grant-checked operations (PRD P8, Doc 2 §11).
- **Dependencies:** A3 grants/consent; E2.6 (drag-and-drop shares the powerbox designation path); A5/E3.2 (the portal backend lives in shims).
- **Design decisions:** portal-backend coverage order — FileChooser first (it *is* the powerbox gesture), ScreenCast second, RemoteDesktop last; **Q5** resolved empirically: a compatibility matrix over a named top-N list of portal-using apps.
- **Exit criteria:** copy in realm A → paste in realm B requires two explicit user actions and is journaled; the portal-compat matrix is published with pass/fail per app; no shared clipboard exists anywhere (verified, not asserted).

## E3.6 — FUSE synthetic paths

- **Goal:** path-expecting legacy apps see granted files at per-realm synthetic paths — the xdg-document-portal pattern, warts inherited knowingly (PRD P12, Doc 2 §12).
- **Dependencies:** E2.6 powerbox (the fds exist; this adds the path view); Rust `fuser` (PRD §17).
- **Design decisions:** **Q10** resolved empirically — atomic-save (rename-over), hardlink, and mmap patterns tested against a named list of most-used desktop apps; per-pattern wart strategy (emulate in FUSE vs. fall back to a per-app subtree standing grant); mount location inside the realm's namespaces.
- **Exit criteria:** the compatibility matrix is published (the PRD promises it publicly — §9 mitigation); a documented fallback path exists for each failing pattern; at least one stubborn real app (e.g. an office suite) saves successfully.

## E3.7 — Wallet v0 + provenance

- **Goal:** the out-of-core hardened wallet service — TPM2/FIDO2 key custody, sandboxed SD-JWT VC/mdoc parsing, presentation-as-grant, physically-originated-input consent — plus the provenance verifier (Sigstore-style identity-bound signing + transparency-log inclusion proofs, D-009) gating the durable consent-ladder rungs (PRD P11, P14, Doc 2 §13). One trust engine, three subjects.
- **Dependencies:** A3 grant table (presentation rows, `provenance_ref`); backward requirement B2 — the physical-vs-emulated input distinction preserved end-to-end since Phase 1; E2.6's deliberately blocked durable rungs unblock here.
- **Design decisions:** library choices (tss-esapi, ctap2, a sigstore-rs-class verifier); **Q14** trust-root governance — decided *before* durable rungs ship (recommended: per-deployment-configurable roots defaulting to federation with Sigstore public-good instances; a project-run log deferred for its governance cost — cross-reference [12-workstream-community.md](12-workstream-community.md) §5); **Q9** standing-grant ergonomics, full answer (grant templates + rate-audited defaults); **Q13** consent-prompt human factors — a prompt-design review is an explicit deliverable, not an afterthought; OID4VC/OID4VP revision pin (moving target per PRD Caveats — pin and track via WS-A).
- **Exit criteria:** PRD user story 7 end to end (presentation consented with physical input; the app receives a derived token; the credential never leaves the wallet; everything journaled); **the impersonation test** (PRD §15 row 7): a re-signed binary and a lookalike publisher both fail to inherit or obtain durable grants; injected input provably cannot approve a presentation.

## E3.8 — Mission-control shell v0

- **Goal:** the unprivileged supervision surface: realm grid, per-principal tinted cursors surfaced, grant/revocation panel (connected-apps view with last-used timestamps), live journal view (PRD P10; §5.2 requires the minimal version, §5.3 claims the full one).
- **Dependencies:** everything above — this is the phase's integrating demo. Standing invariant: **zero lines added to the TCB** (PRD §5.1 first invariant) — the epic's permanent acceptance test.
- **Design decisions:** the shell's own toolkit — eat the dogfood: build on the E2.5 native semantic path, making the shell the second native semantic client; the split of supervision affordances — consent prompts and trusted labels stay core-drawn (Qubes/Nitpicker), grids and panels are shell-drawn.
- **Exit criteria:** one human supervises N agent realms — watches live journals, revokes a grant from the panel with immediate transitive effect, hold-Esc dead-man verified from the shell context; a shell crash leaves the core and all realms unaffected (crash-only test).
