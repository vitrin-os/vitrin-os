# Vitrin OS — Project Guide

**Vitrin OS** is an open-source, agent-first display server: a small trusted
core (`vitrind`) speaking a new capability-native wire protocol, with every
legacy X11/Wayland app confined to its own per-app nested shim, so humans and
AI agents can concurrently observe and operate GUIs under granular, revocable,
capability-scoped authorization. Full vision, architecture, and roadmap:
**`docs/PRD.md`** (Document 1 = PRD, Document 2 = Technical Architecture).

## Repo layout

| Path | What it is |
|---|---|
| `docs/PRD.md` | PRD + Technical Architecture — the canonical vision/design doc |
| `docs/ARCHITECTURE.md` | Maps every crate/directory below to the PRD section it implements — read this before "why does this file exist" |
| `protocol/vitrin-v0.xml` | The wire protocol IDL — **source of truth** for every interface |
| `protocol/vitrin-v0.rng` | RELAX NG schema for the IDL dialect |
| `docs/protocol/00-conventions.md` | Normative protocol conventions (wire format, object ids, error taxonomy, versioning, dialect/schema) |
| `docs/protocol/NN-vitrin_*.md` | One prose page per interface, cross-linked from `00-conventions.md` |
| `crates/vitrin-core/` | `vitrind` — the trusted core (compositor, capability kernel, grant store, realms, consent, dead-man switch) |
| `shim/` | The wlroots-based per-app Wayland shim (C + Meson, outside the Cargo workspace) |
| `sdk/python/` | The pure-Python agent SDK; `examples/agent-demo/run_demo.py` is the demo agent (`cargo xtask demo`) |
| `tests/integration/` | Drives the shipped `vitrind` binary + real shim + real apps over a real socket — see its own README for the entry-point contract |

Where prose and IDL disagree, **the IDL's `<description>` text wins** — prose
pages restate it, they don't override it.

## Phase & tracking model

Work is tracked as GitHub issues in `vitrin-os/vitrin-os`. Phase 1 is split
into 9 epics, each carrying exactly one `track:*` label:

| Track | Epics | Scope |
|---|---|---|
| `track:protocol` | E1 (P1.1) | IDL (`protocol/vitrin-v0.xml`/`.rng`) + prose (`docs/protocol/*.md`) |
| `track:rust-core` | E2–E5, E7 (P1.2–P1.5, P1.7) | `vitrind` trusted core: transport, compositor, capability kernel, realms, consent |
| `track:c-shim` | E6 (P1.6) | wlroots-based per-app Wayland shim |
| `track:sdk` | E8 (P1.8) | Python agent SDK + demo agent |
| `track:ci-docs` | E9 (P1.9) | CI, test harnesses, repo-wide docs |

Milestones `M1.1`–`M1.5` sequence the work within Phase 1 (see PRD §8 for the
phase-level roadmap: Phase 0 spec, Phase 1 MVP slice, Phase 2 semantic+epochs,
Phase 3 network+X11+fleet, Phase 4 horizon).

## Conventions observed in this repo (follow them, don't reinvent)

- **Branch naming**: `p<phase>.<epic>.<task>-slug`, e.g. `p1.1.1-protocol-idl`.
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/),
  `type(scope): summary` — e.g. `feat(protocol): add vitrin_grant.attenuate`.
  Scope is the track (`protocol`, `rust-core`, `c-shim`, `sdk`, `ci-docs`) or
  `root` for repo-wide changes. Types: `feat`, `fix`, `docs`, `refactor`,
  `perf`, `test`, `build`, `ci`, `chore`, `revert`. Reference the tracking
  issue in the footer (`Closes #10` / `Refs #10`) — the issue title already
  carries the phase/epic/task number (e.g. "P1.1.1 — ..."), so the commit
  header no longer needs to repeat it.
  **Supersedes** the ad-hoc `P<phase>.<epic>.<task>: summary` header used in
  this repo's earliest commits (`P1.1.1: author protocol/vitrin-v0.xml ...`)
  — adopted so changelogs can be generated straight from commit history by
  type. Do not rewrite that history; the new format applies going forward.
- **Language**: English only, everywhere — code, docs, commits, issues, PRs.

## Milestone definition-of-done (hard requirement)

A milestone `M1.2`–`M1.5` (see `docs/plan/01-phase-1-mvp.md` §5, D12) is
**done only when its named integration-gate issue passes green with no mock
on any seam that milestone claims**: `M1.2` = #105, `M1.3` = #107, `M1.4` =
#108 + #109, `M1.5` = #110. `vitrin-mock-shim` and `shim/tests/mock_core.c`
are component/unit-test scaffolds — useful, kept, but **never** the evidence
source for a milestone's definition of done. Tests that use them are
component tests, not milestone acceptance; see `tests/integration/README.md`
for the current split. Don't claim a milestone is met on a component-test
result — say so explicitly if a real, mock-free gate hasn't landed yet.

## Protocol authoring rule (hard requirement)

A new or changed interface requires **paired** edits, never one alone:

1. `protocol/vitrin-v0.xml` (and `protocol/vitrin-v0.rng` if the *dialect
   itself* changes, not just an interface).
2. The matching `docs/protocol/NN-vitrin_name.md` prose page.

Validate with:

```bash
xmllint --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml
```

Before touching the protocol, read `docs/protocol/00-conventions.md` — it
defines the wire format, object-id rules, the fatal-vs-recoverable error
razor, and the Wayland-style growth rules every interface must follow. The
`protocol-idl` skill (below) has a distilled cheat-sheet.

## Agents & skills

This repo ships Claude Code agents (`.claude/agents/`) and skills
(`.claude/skills/`). Invoke them as the work demands — Claude delegates based
on each subagent's `description`, or ask explicitly ("use the protocol
subagent to add a new interface").

- **Track agents**: `protocol` (IDL + prose), `rust-core` (`vitrind` core),
  `c-shim` (wlroots/X11 shims), `sdk` (Python agent SDK), `ci-docs` (CI, test
  harnesses, repo-wide docs).
- **Utility agent**: `pr-opener` (draft/open PRs following this repo's
  conventions).
- **Skills**: `protocol-idl` (wire format / error taxonomy / growth-rules
  cheat-sheet), `github-conventions` (branch/commit/PR/issue conventions and
  the epic↔track↔milestone taxonomy).

All nine epics (E1–E9) now have landed code on `main` — `rust-core`,
`c-shim`, and `sdk` each have real, reviewable patterns to follow (see
`docs/ARCHITECTURE.md` for the crate/directory map). Domain-specific skills
for those tracks (Rust/Smithay conventions, wlroots idioms, Python SDK
packaging) can be added once a recurring pattern across PRs in that track
justifies its own cheat-sheet; not every track needs one yet.

## Milestone definition-of-done (mock-free acceptance)

A milestone (`M1.1`–`M1.5`) closes only on a **named, mock-free** integrated
acceptance test against the shipped binaries — never against
`vitrin-mock-shim` or an in-process test harness alone (see
`docs/plan/01-phase-1-mvp.md` §5 for the exact gate per milestone and
`tests/integration/README.md` for why `tests/integration/` exists
specifically to drive the shipped `vitrind` binary rather than an
in-process runtime). Tests built on `vitrin-mock-shim` remain valuable as
fast **component** tests, but citing one as a milestone's proof is the
exact class of honesty gap this repo's docs are written to avoid — state
mock-based coverage as what it is, and cite the real-app gate
(`tests/integration/test_real_*.py`) as the actual milestone evidence.

## Licensing (D-005, executed per D-016)

The split is executed. **The root `NOTICE` is the normative path→license
map — read it, not this summary, before adding or moving a file.** The
per-file `SPDX-License-Identifier` header wins for that file; the per-crate
`license` field in each `Cargo.toml` wins for that crate.

The boundary is drawn by **derivation, not by directory**:

- **MPL-2.0** — original implementation expression: `crates/vitrin-core`,
  `crates/vitrin-ipc`, and the shim's own C sources, hand-written headers
  and test fixtures under `shim/`.
- **Apache-2.0** — anything derived from the protocol, plus everything a
  third party needs: `protocol/`, `crates/vitrin-protocol` (**including its
  checked-in generated code**), `shim/include/vitrin-protocol.h` (generated
  from the same IDL, so it is Apache-2.0 *despite living under `shim/`* —
  writing a C client must never require touching copyleft code),
  `crates/vitrin-scanner` and `crates/xtask`, the conformance instruments
  (`crates/vitrin-golden`, `crates/vitrin-mock-shim`, `fuzz/`),
  `sdk/python/`, `tests/integration/`, `examples/`.
- **CC-BY-4.0** — spec prose: `docs/PRD.md`, `docs/protocol/`, `docs/plan/`.
- **GPL-3.0-only** — `shim/wlcs/` only, the advisory WLCS module. Unchanged
  by the split; never built by default, never installed, never linked into
  `vitrin-shim`.

Three rules that are easy to break by accident:

1. **Never add MPL Exhibit B** to any file. `shim/wlcs/` compiles MPL-2.0
   shim sources into a GPL-3.0-only module, which stays lawful only because
   MPL keeps GPL-3.0 as a Secondary License. Exhibit B would break it.
2. **Never hand-edit the SPDX line in generated files.** It comes from the
   templates in `crates/vitrin-scanner/`; hand-editing turns
   `cargo xtask codegen --check` red.
3. **A new crate must state its own `license`.** The workspace-wide default
   was deliberately deleted so nothing can inherit the wrong half silently.

`shim/include/` is a mixed directory on purpose, so per-directory LICENSE
files were rejected — see D-016 for the reasoning and the accepted costs.
