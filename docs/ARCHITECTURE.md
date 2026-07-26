# Architecture — crate/directory ↔ PRD map

This page is the R8 bus-factor artifact (PRD §9): it maps the repository's
actual crates, directories, and modules to the sections of
[`docs/PRD.md`](PRD.md) that motivate them, so a code review can cite design
intent instead of re-deriving it, and a new contributor can find "why does
this file exist" in one hop. It documents **what is built**, as of this PR;
for **what is planned**, see [`docs/plan/`](plan/), especially
[`01-phase-1-mvp.md`](plan/01-phase-1-mvp.md) (epics/tasks) and
[`00-roadmap.md`](plan/00-roadmap.md) (phase-level milestones).

Where this page and the code disagree, **the code wins** — this is a map,
not a spec. The protocol's own source of truth is
`protocol/vitrin-v0.xml` (see [CLAUDE.md](../CLAUDE.md)); this page does not
restate protocol semantics, only where each interface's implementation
lives.

## 1. Top-level layout

```
vitrin-os/
├── protocol/          # wire-protocol IDL (source of truth) + RELAX NG schema
├── docs/
│   ├── PRD.md         # PRD (Doc 1) + Technical Architecture (Doc 2) — canonical vision/design
│   ├── ARCHITECTURE.md  # this file
│   ├── protocol/      # prose, one page per interface, normative conventions in 00-conventions.md
│   ├── plan/          # phase/epic/task breakdown, decision log, roadmap
│   └── demo/          # demo screencast: recording plan + (once recorded) the published artifact
├── crates/            # the Cargo workspace — see §2 below
├── shim/              # the Wayland shim, C + wlroots, OUTSIDE the Cargo workspace — see §3
├── sdk/python/        # the Python agent SDK — see §4
├── examples/
│   ├── agent-demo/run_demo.py   # the Phase-1 demo agent (also the M1.5 integration test)
│   ├── realm.toml                # realm config template + security commentary
│   └── principals.toml           # identity-registry template
└── tests/
    ├── integration/    # drives the SHIPPED vitrind BINARY over a real socket — see §5
    └── golden/         # reference frames + the golden-regeneration entrypoint (`cargo xtask bless`)
```

## 2. `crates/` — the Cargo workspace (Rust)

| Crate | PRD mapping | What it is |
|---|---|---|
| [`vitrin-protocol`](../crates/vitrin-protocol) | Doc 2 §3.2–3.4 (wire format decision, capability handles, fd passing) | Generated message types + wire codec. Pure data: no sockets, no I/O. Consumed by both the core and (conceptually) any future Rust client; the Python SDK is an independent implementation of the same wire format by design (D8 — it keeps the protocol honest by having two implementations). |
| [`vitrin-scanner`](../crates/vitrin-scanner) | Doc 2 §17 (codegen, checked-in generated code) | The codegen: parses `protocol/vitrin-v0.xml`, emits the Rust types in `vitrin-protocol` and the C header `shim/include/vitrin-protocol.h`. Driven by `cargo xtask codegen`/`--check` (never run by hand). |
| [`vitrin-ipc`](../crates/vitrin-ipc) | Doc 2 §3.2–3.4; PRD plan E2 (P1.2) | The Unix-socket transport: length-prefixed framing, `SCM_RIGHTS` fd passing, `SO_PEERCRED` capture at accept, backpressure/misbehavior policy (kill a slow-reading or fd-bombing connection, never block the compositor loop — Doc 2 §9's "budgeted dependency" posture applied to the wire). Feature-split `server` (calloop glue, what the core links) vs. `client` (what a Rust client would link, with no compositor dependency pulled in). |
| [`vitrin-core`](../crates/vitrin-core) | Doc 2 §2 (trusted core & TCB boundary); the whole of Doc 2 §3–5, §8–9 | `vitrind` — the entire Trusted Computing Base. See §2.1 below for its internal module map. |
| [`vitrin-mock-shim`](../crates/vitrin-mock-shim) | PRD plan E3 (P1.3.4 acceptance: "a mock shim drives an animated surface") | A **fixture only** — a Rust test binary that speaks the shim-facing protocol without wlroots, used to exercise the core's shim server before/alongside the real C shim, and by `cargo xtask demo` today (see the root [README](../README.md#status) for the tracked gap this implies). Never the demo's real-app proof; that is `shim/` (§3) exercised by `tests/integration/`. |
| [`vitrin-golden`](../crates/vitrin-golden) | PRD plan E9 (P1.9.2, golden-frame harness) | Per-pixel + SSIM frame comparison (`vitrin-golden-cmp` binary) and PNG helpers, used by the golden tests and the real-app capture-fidelity integration gate. |
| [`xtask`](../crates/xtask) | Repo-wide tooling, not a PRD section | `cargo xtask codegen [--check]`, `cargo xtask bless [--filter S]`, `cargo xtask demo [--headless]` — see each subcommand's own doc comment in `crates/xtask/src/main.rs` for the full contract. |

### 2.1 Inside `vitrin-core` — the TCB's own modules

| Module | PRD mapping | Role |
|---|---|---|
| `main.rs` | Doc 2 §2 (TCB discipline: no policy, one enforcement chokepoint, budgeted deps) | Binary entry point; the module-level doc comment is the current, load-bearing statement of what is and is not in the TCB — read it before adding a dependency. |
| `backend/` (`winit.rs`, `headless.rs`, `band_witness.rs`) | Doc 2 §9 (rendering/compositing path); PRD plan E3 (P1.3.1–2) | The two presentation backends: `--nested` (a host Wayland client, real display) and `--headless --size WxH` (fixed virtual output, pixman software rendering, no GPU — the CI path, plan decision D3). Both drive the same `scene`/`session` runtime. `band_witness.rs` is not a third backend: it is the trusted-band witness (issue [#139](https://github.com/vitrin-os/vitrin-os/issues/139), refs #85), which counts per composite whether client content ever reached the trusted band's rows on the human-visible output. Compiled only under `cfg(any(test, feature = "consent-injector"))`, and wired into `headless.rs`'s composite only under the feature, so no shipping build computes or answers anything. |
| `scene/` (`mod.rs`, `layout.rs`) | Doc 2 §3.1 (object model: surfaces, views); Doc 2 §2's "layout policy stays out of the core long-term" invariant | Scene composition v0: single maximized client surface. `layout.rs` is doc-marked as *not* the core's permanent job — window-management policy moves to an unprivileged component later (the Nitpicker/Fuchsia-Scenic lesson Doc 2 §2 cites). |
| `shim.rs` | Doc 2 §3.4, §4 (fd/dmabuf passing; shim architecture) | The shim-facing protocol server: `vitrin_shim_session`/`_surface`/`_seat` over the inherited socketpair — accepts buffer attach/damage/commit, relays frame-done, and is the core-side half of every shim interaction in `docs/protocol/09`–`11`. |
| `dmabuf.rs` | Doc 2 §3.4, §9 (zero-copy dmabuf import) | The `DmabufImporter` trait seam, the GLES importer behind it, and `present_human_visible` — the one GPU presentation entry point, which always paints the trusted band (§5.3/issue #85), so a zero-copy frame is never composed only of pixels the confined client owns. **Threaded into the nested backend, and only that one** (PR #132): `--nested` builds a real `GlesDmabufImporter` over its live `GlesRenderer` for both shim dispatch and realm teardown, and presents retained GPU content straight into the host window. **Headless remains `importer: None`** — it has no GPU renderer, so every `kind=dmabuf` commit there resolves as the designed `buffer_done(import_failed)` shm fallback (D3), which is what CI runs on end to end. One MVP seam is open by design: while a consent prompt or a dead-man hold indicator is up, the nested backend falls back to the CPU compose path, and the dmabuf arm never commits into `Scene` — so the window shows stale CPU-side content for as long as the overlay lasts (argued in full at `backend/winit.rs`'s `try_redraw`). Real-GPU coverage is env-gated (`VITRIN_GPU_TESTS=1`), never CI; the mock-free integration gate for zero-copy is issue [#117](https://github.com/vitrin-os/vitrin-os/issues/117) and has not landed. |
| `capture.rs` | Doc 2 §3.1 (surface/view); PRD plan E3 (P1.3.6) | The sealed-memfd pixel path behind `vitrin_view.frame_ready`. Pure mechanics — the authority decision lives in `enforcement.rs`. |
| `identity.rs`, `principal.rs` | Doc 2 §5.1 (identity binding at connect); plan D5 | The pluggable `Verifier` trait + `StaticVerifier` (principal registry from `principals.toml`), and the `vitrin_principal` object — the authenticated root of a connection's authority chain. |
| `grants.rs`, `petitions.rs` | Doc 2 §5.2 (grant table schema), §3.3 (capability handle semantics) | The in-memory grant table (request → pending → consent → resolved; sender-constrained; expiry/revocation) and the petition state machine `request_grant` drives. |
| `enforcement.rs` | Doc 2 §2 ("one enforcement chokepoint") | The single function every `capture_frame` and every actuation passes through: connection → principal → grant → verbs → constraints. Grep-provable single-path by design. |
| `realm.rs`, `spawn.rs`, `lifecycle.rs` | Doc 2 §4.1 (spawn model); PRD plan E5 (P1.5) | Realm object v0 (`realm-0`); the fork/exec of the shim binary with a scrubbed, allow-listed environment and a private runtime dir (the security posture documented in the root README and `examples/realm.toml`); crash detection, exit propagation, shutdown ordering. |
| `consent/` (`mod.rs`, `render.rs`, `canvas.rs`, `text.rs`, `grab.rs`, `indicator.rs`, `injector.rs`) | Doc 2 §5.3 (consent surface); plan D4 (TCB rendering stack) | The core-rendered consent prompt — composited above the realm view, in human-visible output only, drawn with a hand-rolled canvas + `fontdue` (no GUI toolkit in the TCB) — plus its exclusive input grab while a prompt is shown, and the "this session is under vitrind's consent surface" trust indicator. `injector.rs` is the build- **and** invocation-gated `consent-injector` test channel (issue #138): present only under `cfg(any(test, feature = "consent-injector"))` and inert without `--consent-injector-fd N`, it feeds a decision into the same `ConsentGrab` a physical click reaches — never a second decision path. |
| `deadman.rs` | PRD P10 (dead-man switch), reduced to MVP scope in plan E7 (P1.7.3) | The hold-Esc revocation switch: tap-through/hold-swallow semantics, disarmed by the chord key returning, cancelled on host-window focus loss. |
| `cursor.rs` | PRD §20.15 / plan [D-019](plan/20-decision-log.md) (agent cursor drawing); Doc 2 §8's origin split | The agent cursor sprite: one geometry function (`agent_cursor_rects`) read by **both** human-visible presentation paths — the CPU composite and `dmabuf.rs`'s draw list — so they cannot paint different crosshairs, the way `trust_band_rect` already works for the band. Drawn from `input/`'s **agent-owned** position (`InputRouter::agent_pointer`, emulated origin only), never the shared one both origins write, and clipped below the trusted band because an agent picks its own coordinates. **Drawing only:** seat delivery to the shim is still one shared pointer position per realm view — D-017's per-principal *delivery* deferral to M2 is untouched. Human-visible output only, at the same output stage as the consent overlay, so no capture can contain it; nested always composites it, headless only under `--agent-cursor` (its human-visible framebuffer is measured against the realm view by `band_witness.rs`). |
| `input/` | Doc 2 §8 (input pipeline); plan requirement B2 (physical-vs-emulated origin tagging) | Input intake and the `origin` tag (`physical`/`emulated`) applied at the point of entry, carried through to every `vitrin_shim_seat` event — the structural hook later phases hang physically-originated consent on. |
| `session.rs` | Not a single PRD section — the runtime glue (plan P1.M1.1) | Binds the core socket, accepts principal connections, drives a `PrincipalServer` per connection against the shared capability kernel, services the realm's shim socketpair, and sweeps expired petitions/grants. This is what makes every module above reachable from a running binary. |
| `recorder.rs` | Plan requirement B1 (replay-ready log entries); PRD Doc 2's journal concept, reduced to an MVP seed | The flight-recorder: a JSON-lines log of handshakes, grant lifecycle, consent decisions, and actuations, with per-capture observation digests. Explicitly **not** the signed journal of a later phase. |
| `toml_subset.rs` | Not a PRD section — a security-motivated implementation detail | The deliberately strict TOML dialect `realm.toml`/`principals.toml` are parsed with (comments, `[[table]]` headers, string/string-array values only) — every file it accepts is valid TOML, but it accepts much less than a general parser would, so a config file cannot smuggle in a construct nobody reviewed. |
| `test_pattern.rs` | PRD plan E3 (P1.3.1 acceptance: "shows the test pattern") | The deterministic synthetic image the scene renders before any client surface has committed — what the earliest golden tests assert against. |

## 3. `shim/` — the Wayland shim (C + wlroots)

Outside the Cargo workspace by design (Doc 2 §17: C for the shim, to link
wlroots without pulling a Rust wlroots binding into the loop; Meson-built).
Maps to Doc 2 §4 (shim architecture) and PRD plan E6 (P1.6).

| Path | PRD mapping | Role |
|---|---|---|
| `include/vitrin-protocol.h` | Doc 2 §3.2 (wire format) | Generated C header (marshal/unmarshal helpers) — never hand-edited; regenerate with `cargo xtask codegen`. |
| `src/main.c`, `src/server.c` | Doc 2 §4.1 (one binary, N instances, spawn model); §4.2 (Wayland shim) | The wlroots **headless** backend compositor: one instance, one private socket, one app — spawned by the core's `spawn.rs`, never touches real hardware (the core owns the screen). |
| `src/upstream.c` | Doc 2 §3.4, §4.4 (buffer/input/damage paths) | The link to the core over the inherited socketpair: forwards the app's committed buffer + damage upstream, relays the core's `frame_done` back as the app's `wl_surface.frame` callback. |
| `src/seat.c` | Doc 2 §4.4 (input paths); plan requirement B2 | Virtual-seat replay: receives origin-tagged `vitrin_shim_seat` events and replays them into the app via the shim's own `wl_seat` — the dynamic-keymap technique for Unicode text delivery (plan D7). |
| `src/globals.c`, `src/probe-catalogue.h.in`, `docs/firefox.md` | Doc 2 §4.2 (Wayland shim: which globals it must offer) | The "globals an app touched" ledger and the probe-globals mechanism that made the v0 global set (including `wl_subcompositor`, added empirically for Firefox) an evidence-driven contract rather than a guess. |
| `src/xdg.c`, `src/output.c`, `src/wire.c`, `src/ledger.c` | Doc 2 §4.2 | `xdg_shell`/`wl_output` handling and the wire-protocol dispatch/logging glue. |
| `tests/` | PRD plan E9 (P1.9.1 shim CI job: no Rust toolchain) | C-side checks: the header compiles standalone; frames match the Rust codec byte-for-byte (golden frames); Firefox bring-up fixtures. |

See [`shim/README.md`](../shim/README.md) for the shim's own up-to-date
status (what globals are implemented, what Firefox needed, what popups
still lack).

## 4. `sdk/python/` — the agent SDK

Maps to Doc 2 §18 (API sketch) and PRD plan E8 (P1.8). Pure Python, stdlib
only (decision D8 — no C extension, so it is a genuinely independent
implementation of the wire protocol, not a binding to the Rust one).

| Path | Role |
|---|---|
| `src/vitrin_os/transport.py`, `wire.py` | Framing + `socket.recvmsg`/`sendmsg` `SCM_RIGHTS` fd passing — the transport half of Doc 2 §3.2/§3.4, independently implemented from `vitrin-ipc`. |
| `src/vitrin_os/protocol.py`, `messages.py` | The client-side message shapes (mirrors, not generated from, `vitrin-protocol` — see D8's rationale in the module docstrings). |
| `src/vitrin_os/client.py` | The blocking, synchronous client API: `connect` → `request_grant` → `grant.observe()` → `grant.actuate_pointer`/`actuate_text` (Doc 2 §18's pseudocode, made real). |
| `src/vitrin_os/errors.py` | Typed grant-error exceptions (`GrantExpired`, `Revoked`, `RateLimited`, …) mapped from the wire's `refused` codes. |
| `src/vitrin_os/png.py` | Capture ergonomics: `.to_png()` on an observed frame (Pillow optional; the raw path stays dependency-free). |
| `tests/` | Unit tests against a scripted mock server built from protocol test vectors — no `vitrind` involved; the live pairing is `tests/integration/`'s job. |

## 5. `tests/` and `examples/` — what proves the above wired together

| Path | PRD mapping | Role |
|---|---|---|
| `examples/agent-demo/run_demo.py` | Doc 2 §18; PRD plan E8 (P1.8.4 acceptance) | The demo agent: connect → grant → consent → capture → locate a UI feature by pixels → click → type → capture → assert the page changed. Doubles as the M1.5 integration test; launched by `cargo xtask demo`. |
| `examples/realm.toml`, `principals.toml` | Doc 2 §4.1 (spawn model); the spawn-path security notes in the root README | Config templates with the security rules (ownership/writability checks, environment allow-listing) spelled out inline. |
| `tests/integration/` | PRD plan E9 (P1.9.1/.6); the honesty rule in [#111](https://github.com/vitrin-os/vitrin-os/issues/111) | Drives the **shipped `vitrind` binary** over a real socket with a real forked realm (never an in-process runtime) — the only place startup-ordering regressions are visible. Hosts the M1.2–M1.5 real-app gates (`test_real_app.py`, `test_real_capture_fidelity.py`, `test_real_actuation.py`, `test_real_deadman.py`, `test_real_consent.py`, `test_demo.py`, plus the `test_real_firefox.py`/`test_real_gtk.py` rungs); `run.sh` names those gates in `MILESTONE_GATES` and fails if one is missing, since `unittest discover` cannot tell an absent gate from a green suite. See [`tests/integration/README.md`](../tests/integration/README.md) for the full entry-point contract. |
| `tests/golden/` | PRD plan E9 (P1.9.2) | Reference frames + the `cargo xtask bless` regeneration entrypoint for every golden test scattered across the workspace (consent-prompt ink map, SDK wire vectors, headless test pattern). |

## 6. What this map deliberately omits

Phase 2+ concepts named in the PRD (semantic trees/epochs, the powerbox,
the credential wallet, network sessions, the X11 shim, the mission-control
shell) have **no corresponding code yet** and so have no row above. See
[`docs/plan/02-phase-2-semantic-epochs.md`](plan/02-phase-2-semantic-epochs.md)
onward for where they are planned, and the root README's roadmap section
for the phase-level summary. Do not read their absence from this table as
an oversight — it is the honesty rule this whole docs pass is written
under (issue [#111](https://github.com/vitrin-os/vitrin-os/issues/111)).
