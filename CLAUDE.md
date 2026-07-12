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
| `protocol/vitrin-v0.xml` | The wire protocol IDL — **source of truth** for every interface |
| `protocol/vitrin-v0.rng` | RELAX NG schema for the IDL dialect |
| `docs/protocol/00-conventions.md` | Normative protocol conventions (wire format, object ids, error taxonomy, versioning, dialect/schema) |
| `docs/protocol/NN-vitrin_*.md` | One prose page per interface, cross-linked from `00-conventions.md` |

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

`rust-core`, `c-shim`, and `sdk` currently have no code to work against yet
(Phase 1 tracks not yet started) — their agent definitions are grounded in the
Technical Architecture sections of `docs/PRD.md` rather than existing code
patterns. Domain-specific skills for those tracks (Rust/Smithay conventions,
wlroots idioms, Python SDK packaging) should be added once each track lands
real, reviewable code — not speculatively now.
