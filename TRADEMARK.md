# Trademark policy

**Short version: the code is free, the name is not** — and that split is
deliberate, because the name is what tells someone which build actually
enforces the security claims.

## The marks

"Vitrin OS"; "Vitrin" where it refers to this project; `vitrind`, the
trusted-core daemon; and the project namespaces claimed on 12 July 2026
under decision **D-006** (the GitHub org `vitrin-os`, the npm scope
`@vitrin-os`, and the crates `vitrin-os` and `vitrind`).

No registration exists. This is an unregistered claim, written down here
so that it is at least stated rather than assumed. There is no ®, no
counsel, and no legal entity behind the project today.

## Why a name policy exists alongside free licenses

The licenses do not cover this, on purpose. Apache-2.0 §6 grants no
trademark rights; MPL-2.0 §2.3 closes with the same sentence about
trademarks, service marks, and logos. Neither is an oversight. **The
license is the tool that keeps the code free; trademark is the tool that
keeps the name meaningful.** They do different jobs, and it is a good
thing they are separate.

Here the name has one specific job. Vitrin OS's whole pitch is a small
trusted core: one enforcement chokepoint, a consent surface a client
cannot draw over, a grant table that expires and revokes. Those claims are
checkable — against [`crates/vitrin-core`](crates/vitrin-core), this tree,
this code, with [`SECURITY.md`](SECURITY.md) stating what is and is not
yet proven. If a modified core could ship under the same name, "Vitrin OS"
would stop being a statement about what is enforced, and every security
claim in [`docs/PRD.md`](docs/PRD.md) and [`README.md`](README.md) would
become unverifiable in practice. Protecting the name is protecting
someone's ability to know what they are running.

## The model: Firefox and Iceweasel

Mozilla's trademark policy required approval for builds carrying the
Firefox name with non-trivial patches. Debian and Mozilla could not agree
on terms, so from 2006 Debian shipped the same browser under the name
Iceweasel — and in 2016, once the two sides settled, Debian went back to
shipping it as Firefox.

Both halves of that story are the model here. The rename is a remedy, not
a punishment: the code kept shipping the entire time, users kept getting
the browser, and the disagreement ended in an agreement rather than a
lawsuit. That is the outcome this policy aims at.

## What you can do without asking

- **Say the name.** "Compatible with Vitrin OS", "built on Vitrin OS", "a
  Vitrin OS agent", "we evaluated Vitrin OS and chose something else, here
  is why". Blog posts, talks, papers, benchmarks, comparisons, criticism —
  all fine, including the unflattering ones. Nominative use is not
  something this policy tries to reach.
- **Redistribute an unmodified build under the name.** Distro packaging of
  this tree is the normal, welcome case.
- **Package it the way distributions package things.** Build-system fixes,
  backported upstream patches, packaging metadata, path adjustments —
  changes that do not alter what the trusted core enforces keep the name.
- **Fork it under a different name.** That is what the licenses are for
  and nobody needs permission. Saying what it is derived from — "a fork of
  Vitrin OS" — is nominative use and is fine.
- **Name your own project so it reads as yours.** `vitrin-os-metrics`,
  `foo-for-vitrin-os`, `vitrin-agent-toolkit`: a name that reads as
  third-party is fine. A name that reads as official, or that sits inside
  the project's own namespaces above, is not.

## What needs a different name, or a conversation first

- **A modified trusted core distributed under the Vitrin OS name.** The
  trigger is precise, and it is the same line the security model draws: if
  your change alters what [`crates/vitrin-core`](crates/vitrin-core) — or
  [`crates/vitrin-ipc`](crates/vitrin-ipc) beneath it — enforces (the
  enforcement chokepoint, the grant lifecycle, the consent surface, the
  dead-man switch, input origin tagging), then rename, or ask.

  Note what is deliberately *not* on that list: [`shim/`](shim/) is
  untrusted by design and sits outside the TCB. Patching a shim, or
  writing an entirely new one, does not change what the core enforces and
  does not cost you the name.
- **Using the name for your company, product, or domain**, or in any way
  that implies this project endorses, maintains, reviewed, or vouches for
  something it does not.
- **Merchandise, logos, and event names.**

## How to ask

Open an issue in
[vitrin-os/vitrin-os](https://github.com/vitrin-os/vitrin-os/issues)
describing what you want to ship. The default answer is yes. The purpose
of this document is to stop someone being confused about which build
enforces the security claims — not to control who talks about the project,
and not to make packaging harder than it needs to be.

## Standing of this document

This is a statement of the maintainer's intent, not a contract. Governance
is a documented single maintainer
([`docs/plan/12-workstream-community.md`](docs/plan/12-workstream-community.md)
§5) and there is no legal entity behind the project; if that changes —
a foundation, a fiscal host — this document gets revisited with it.

**Nothing here restricts any right granted by the licenses.** If this
policy and a license disagree about what you may do with the *code*, the
license wins. This document concerns the *name* only; see
[`NOTICE`](NOTICE) for the license map.
