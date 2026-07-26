<!--
Lead with the issue link so it auto-closes on merge.
Use `Fixes #N` for bug fixes, `Closes #N` for features/tasks, `Refs #N` if
this only moves an issue along.
-->

Closes #

## What changed

-
-

## Acceptance criteria

<!-- Restate each criterion from the issue and confirm it is met. If one is
     not met, say so here rather than leaving it silently unticked. -->

- [ ]

## Evidence

<!-- How do you know it works? Name the test, paste the run, or say plainly
     that this is unverified.

     If this claims to close a milestone (M1.x): the gate must be mock-free
     against the shipped binaries (decision D12). A test built on
     vitrin-mock-shim or shim/tests/mock_core.c is a COMPONENT test and is
     never milestone evidence -- say which kind yours is. -->

## Checklist

- [ ] Every commit is signed off (`git commit -s`) — DCO, see [CONTRIBUTING.md](../CONTRIBUTING.md)
- [ ] Commits follow Conventional Commits (`type(scope): summary`)
- [ ] New first-party source files carry an `SPDX-License-Identifier` header
- [ ] A new crate declares its own `license` field

**If this touches the protocol** — all of these, or none of them:

- [ ] `protocol/vitrin-v0.xml` and the matching `docs/protocol/NN-*.md` page changed **together**
- [ ] `xmllint --noout --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml` passes
- [ ] `cargo xtask codegen` run, regenerated output committed here
- [ ] No generated file hand-edited (its SPDX line comes from the scanner templates)
