# Security policy

## Status first: pre-1.0, and no security guarantees yet

Vitrin OS is a capability-security display server, so it is worth being
blunt about where it actually is. Per decision **D-014**
([`docs/plan/20-decision-log.md`](docs/plan/20-decision-log.md)) the
protocol spec is published early, versioned `0.x`, and explicitly tracks
the reference implementation; every crate is `0.1.0`, the Python SDK is
`0.1.0a0`, and the repository is in late Phase 1. There has been no
release, no external audit, and no third-party security review.

**The project makes no security guarantees at this stage.**
[`docs/PRD.md`](docs/PRD.md) describes properties the *design* intends to
deliver. The README's
[Security notes — what the MVP does and does not confine](README.md#security-notes--what-the-mvp-does-and-does-not-confine)
describes what the *shipped code* does today, which is considerably less.
Where the two differ, the README is the one telling the truth. Do not run
Vitrin OS anywhere a real adversary is on the other side of it.

None of that makes reports unwelcome — the opposite. A capability kernel
is exactly the kind of thing where a design flaw found at `0.1.0` is worth
an order of magnitude more than the same flaw found after 1.0.

## Reporting a vulnerability

Use **GitHub private security advisories**:

<https://github.com/vitrin-os/vitrin-os/security/advisories/new>

That is the only private channel. The project deliberately publishes no
security email address and no PGP key: an unmonitored inbox or a key
nobody rotates is worse than not having one at all. If the advisory form
is unavailable to you, open an ordinary public issue saying that you have
a report and asking for a channel — and put **no details in it**.

A report is easiest to act on when it names the commit on `main`, the
binary and backend involved (`vitrind --headless` vs `--nested`, the C
shim, the Python SDK), the `realm.toml`/`principals.toml` in play, what
the attacker is assumed to control at the start, and a reproduction —
a failing test under [`tests/integration/`](tests/integration/) is the
gold standard, but a prose walkthrough is fine.

## What to expect, honestly

This is an unfunded single-maintainer project — governance is stated
plainly as such in
[`docs/plan/12-workstream-community.md`](docs/plan/12-workstream-community.md)
§5. There is no on-call rotation and no security team, so no 24-hour SLA
is offered, because one would be a lie.

| Stage | Realistic expectation |
|---|---|
| Acknowledgement that the report arrived | usually within 7 days |
| First assessment — is it a bug, is it in scope, rough severity | within 30 days |
| A fix | depends on severity and on the maintainer's week; no promise |

If you have heard nothing after 14 days, ping the advisory thread, or open
a public issue saying only that a report is waiting. Silence is a dropped
ball, not a policy — treat it as one.

There is **no bug bounty**: no money, no swag, nothing to claim. Credit in
the published advisory unless you would rather not be named.

## Supported versions

Only `main`. There are no releases, no tags carrying fixes, and no
branches receiving backports — "fixed" means "merged to `main`". This
section changes when the project cuts its first release.

## Scope

### The trust boundary is the scope boundary

The entire security argument of this project is that the trusted core is
small and everything else is disposable and confined
([`docs/PRD.md`](docs/PRD.md) §15, Doc 2 §2). The scope of this policy is
the same line.

**In scope:**

- [`crates/vitrin-core/`](crates/vitrin-core) — the whole TCB. The
  enforcement chokepoint (`enforcement.rs`), grant lifecycle
  (`grants.rs`, `petitions.rs`), the consent surface and its input grab
  (`consent/`), the dead-man switch (`deadman.rs`), input origin tagging
  (`input/`), the capture path (`capture.rs`), realm spawn (`spawn.rs`,
  `realm.rs`), and the shim-facing server (`shim.rs`).
- [`crates/vitrin-ipc/`](crates/vitrin-ipc) — framing, `SCM_RIGHTS` fd
  passing, `SO_PEERCRED` capture, and the backpressure / oversized-frame /
  fd-bomb termination policy. Anything that lets a connection stall or
  crash the compositor loop belongs here.
- [`crates/vitrin-protocol/`](crates/vitrin-protocol) — decoders reachable
  from untrusted input.
- [`protocol/vitrin-v0.xml`](protocol/vitrin-v0.xml) — design-level
  authority flaws in the protocol itself, not just implementation bugs.
  A message that lets a principal name authority it should not be able to
  name is a protocol bug and is very much wanted.

Findings that would be especially valuable: a path to `capture_frame` or
an actuation that does **not** pass through `enforcement.rs`; a grant that
survives its expiry, its revocation, or its sender constraint; a client
that can draw over, occlude, or spoof the core-rendered consent prompt, or
escape its exclusive input grab; input that reaches an app tagged
`physical` when it did not originate physically; a capture that returns
another realm's pixels; a descriptor or environment variable that survives
the spawn fork it should not.

**Out of scope — because the shim is untrusted by design:**

- [`shim/`](shim/) is deliberately *outside* the TCB. PRD §15 already
  lists "compromised shim" as an actor that **can** lie about its own
  app's buffers, misbehave toward its own app, and corrupt or crash
  itself. That is the threat model working as intended, not a
  vulnerability. "I patched the shim and it sent the core a malformed
  frame" is an expected input, not a finding.
- The version of that report which *is* in scope is the crossing: if a
  hostile shim can affect **another** realm, reach into the core, forge a
  realm identity the core assigned, or make the core misbehave for anyone
  but itself, that is a core bug in `shim.rs` or `vitrin-ipc` and is fully
  in scope. The boundary is the trust boundary, not the directory.
- Test scaffolds and dev tooling that ship in no artifact:
  `crates/vitrin-mock-shim`, `crates/vitrin-golden`, `crates/xtask`,
  `fuzz/`, `tests/`. A crash in a mock is a test bug — file an ordinary
  issue.
- [`sdk/python/`](sdk/python) is a client library that trusts the core it
  connects to. Robustness bugs there are welcome as ordinary issues; they
  are not TCB findings.
- Third-party dependencies — wlroots, Smithay, Mesa, the kernel. Report
  those upstream; open an issue here if you want help routing one.

### Known gaps that are not findings

These are documented, decided, and tracked. They are listed so a reporter
does not spend a weekend on something the project already says out loud:

- **The sandbox is half-built** (plan decisions D9,
  [`docs/plan/01-phase-1-mvp.md`](docs/plan/01-phase-1-mvp.md); D-020 and
  D-036, [`docs/plan/20-decision-log.md`](docs/plan/20-decision-log.md)).
  Since P2.6.2 the shim and its app run in **six namespaces** with an identity
  uid/gid map, **zero capabilities** and a private mount table, verified by the
  core from outside; a realm that cannot be verified is not spawned. An app can
  no longer read `$HOME` or reach a socket by a path it already knows, because
  neither is in its mount table.

  **What is still missing is the syscall boundary**: no seccomp filter
  (P2.6.4), no Landlock ruleset (P2.6.3). Treat a realm as *path-confined but
  not syscall-confined*. Three residues are published in full on the
  [limits page](docs/book/src/limits.md): the invoking user's supplementary
  groups survive into the realm because the kernel gives no window to drop
  them, the GPU render node is bound read-write with its ioctl surface intact,
  and `--isolation=off` restores the fully unconfined path for anyone who names
  it. **Do not treat a realm as a security boundary against hostile code yet.**
- **The session D-Bus is reachable.** The core advertises no
  `DBUS_SESSION_BUS_ADDRESS`, but `/run/user/<uid>/bus` is still on the
  filesystem and still connectable, and the abstract-socket namespace is
  still shared. Closed by P13 in Phase 2.
- **Same-uid separation is not attempted.** The `0700` runtime directory
  bounds other *users* of the machine, not other processes of this user.
- **Realm identity is possession of a file descriptor.** The core hands
  the shim one end of a socketpair at fork; holding that descriptor *is*
  being that realm's shim. No credential, no handshake. Deliberate for
  the MVP.
- **The flight recorder is a plain JSON-lines log** (`recorder.rs`),
  explicitly *not* the signed, append-only journal the PRD describes for a
  later phase. It is not tamper-evident and nothing in the tree claims it
  is.
- **Consent occlusion has no mock-free gate.**
  [#109](https://github.com/vitrin-os/vitrin-os/issues/109)'s consent half
  is proven only by an in-process Rust test, not against the shipped
  binary — which under this repo's own definition of done (plan D12) means
  the property is **not proven** for `vitrind` as shipped. Treat it as an
  open question rather than an established guarantee. A concrete
  occlusion demonstrated against the shipped binary is genuinely valuable
  and should go through the advisory channel.
- **The nested backend falls back to CPU compose while an overlay is up**,
  so the host window can show stale content for as long as a consent
  prompt or dead-man indicator lasts (argued in full at
  `crates/vitrin-core/src/backend/winit.rs`, `try_redraw`). Zero-copy's
  own mock-free gate,
  [#117](https://github.com/vitrin-os/vitrin-os/issues/117), has not
  landed.
- **Everything from Phase 2 onward does not exist.** Semantic trees,
  epoch/CAS staleness rejection, the powerbox, the credential wallet,
  network sessions, the X11 shim, the mission-control shell — no code, and
  therefore no vulnerabilities. See
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §6.

Two things that list does **not** mean. If a documented gap turns out to
be materially worse than documented — a wider blast radius, an easier
trigger, a consequence nobody wrote down — that is a finding. And if the
documentation itself overclaims, that is also a finding. The rule this
repo is written under is that a half-believed confinement claim is worse
than an honest gap; a report proving one of these documents wrong is doing
precisely the job.

## Coordinated disclosure

Report privately, a fix lands on `main`, an advisory is published. The
default embargo is 90 days from the acknowledgement, negotiable in both
directions: if a fix lands in a week the advisory goes out in a week, and
if the fix turns out to be architectural, say so and a longer date can be
agreed. Nothing already public will be treated as embargoed.

Given that the project is pre-1.0 with no deployments to protect, the bias
is toward publishing quickly and plainly rather than sitting on anything.
If a finding is best explained as a documentation change — "the README
claims X and here is why X is false" — that is a perfectly good outcome
and will be credited the same way.
