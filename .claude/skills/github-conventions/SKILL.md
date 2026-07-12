---
name: github-conventions
description: GitHub conventions for Vitrin OS — repo, gh CLI usage, branch/commit naming, PR/issue body templates, and the epic/track/milestone label taxonomy. Use when creating or editing issues, PRs, branches, or commits.
---

# GitHub conventions (Vitrin OS)

- **Repo**: `vitrin-os/vitrin-os`.
- **Tooling**: the `gh` CLI for all GitHub operations.

## Branch naming

`p<phase>.<epic>.<task>-slug`, e.g. `p1.1.1-protocol-idl`
(phase 1, epic 1 = E1/track:protocol, task 1 = issue #10).

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope):
summary`. Adopted specifically so a changelog can be generated straight from
commit history, grouped by type.

- **Types**: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`,
  `chore`, `revert`.
- **Scope**: the track (`protocol`, `rust-core`, `c-shim`, `sdk`, `ci-docs`)
  or `root` for repo-wide changes.
- **Footer**: reference the tracking issue, e.g. `Closes #10` or `Refs #10`.
  The issue title already carries the phase/epic/task number (e.g.
  "P1.1.1 — ..."), so the commit header doesn't need to repeat it.

Example:

```
feat(protocol): author v0 interfaces and error taxonomy

protocol/vitrin-v0.xml — 11 interfaces, 29 messages, 14 enums. See
docs/protocol/00-conventions.md for the normative rules this implements.

Closes #10
```

**Supersedes** the ad-hoc `P<phase>.<epic>.<task>: summary` header used in
this repo's earliest commits (e.g. `P1.1.1: author protocol/vitrin-v0.xml
...`) — that format predates this convention and is not rewritten
retroactively; Conventional Commits applies going forward.

## Epic → track → milestone taxonomy

Phase 1 is 9 epics (E1–E9), each with exactly one `track:*` label, sequenced
by milestones `M1.1`–`M1.5`:

| Epic | Phase task | Track label | Scope |
|---|---|---|---|
| E1 | P1.1 | `track:protocol` | IDL + prose |
| E2 | P1.2 | `track:rust-core` | Transport (`vitrin-ipc`) |
| E3 | P1.3 | `track:rust-core` | Core compositor skeleton |
| E4 | P1.4 | `track:rust-core` | Capability kernel & grant store v0 |
| E5 | P1.5 | `track:rust-core` | Realm & spawn manager |
| E6 | P1.6 | `track:c-shim` | Wayland shim (C + wlroots) |
| E7 | P1.7 | `track:rust-core` | Consent surface |
| E8 | P1.8 | `track:sdk` | Agent SDK (Python) + demo |
| E9 | P1.9 | `track:ci-docs` | Testing, CI & release hygiene |

Every issue also carries `phase-1` and, for epics, the `epic` label. Other
repo labels: `bug`, `documentation`, `duplicate`, `enhancement`,
`good first issue`, `help wanted`, `invalid`, `question`, `wontfix`.

## Issue body template

```markdown
## Summary

{1-2 sentence description}

## Tasks

- [ ] Task 1
- [ ] Task 2

## Acceptance Criteria

- [ ] Criterion 1
- [ ] Criterion 2

## References

- docs/PRD.md - {relevant section}
- docs/protocol/00-conventions.md - {relevant section, if protocol work}
```

## PR body template

Lead with the issue link so it auto-closes on merge:

```markdown
Closes #10

- Bullet point of change 1
- Bullet point of change 2

Acceptance criteria (issue #10): <restate and confirm each is met>
```

Use `Fixes #N` for bug fixes, `Closes #N` for features/tasks.

## Related agents/skills

- `pr-opener` agent — opens PRs following these conventions.
- `protocol-idl` skill — wire-protocol authoring rules for `track:protocol`
  work specifically.
