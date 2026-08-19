# Security policy

## Status first: pre-1.0, and no security guarantees yet

Vitrin OS is a capability-security display server, so it is worth being
blunt about where it actually is. Per decision **D-014**
([`docs/plan/20-decision-log.md`](docs/plan/20-decision-log.md)) the
protocol spec is published early, versioned `0.x`, and explicitly tracks
the reference implementation; every crate is `0.1.0`, the Python SDK is
`0.1.0a0`, and Phase 1 is complete while Phase 2 has only begun: its
confinement track has landed through the seccomp deny-list of P2.6.4 — P2.6.3
itself is not finished — and nothing of the powerbox, the egress path or the
semantic work exists. The other work since has been the maintainer
session-mode workstream WS-E, which **D-021** scopes as dogfooding rather than
as a phase. There has been no
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
- [`crates/vitrin-realm-init/`](crates/vitrin-realm-init) — the confinement
  helper, and the second trusted binary in this tree. At
  `--isolation=default` the core `execve`s it in the shim's place, and it is
  what unshares the six namespaces, builds the mount table, `pivot_root`s,
  enforces the Landlock ruleset (`landlock.rs`) and installs the seccomp
  deny-list (`seccomp.rs`) before the shim's own `execve`; at
  `--isolation=off` no helper runs at all. Much of what it does is not taken
  on its word: the core writes `setgroups` and the uid/gid map itself and
  reads all three back, reads the supervisor's namespace inodes, proves the
  pid the helper names really is the supervisor's child and init of exactly
  one nested pid namespace, and probes the realm's filesystem from outside
  through `/proc/<pid>/root` — refusing the spawn wherever it cannot. A path
  *past* that outside verification, or a confinement reported as applied and
  not applied where no outside read can catch it, is a core finding and is
  wanted; the Landlock rung is the named case of the latter, and the known
  gap below says why.
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

  Since P2.6.3 the helper also enforces a **Landlock ruleset** before the
  shim's `execve`, so a path the mount table happened to leave reachable is not
  therefore readable. State its grants precisely, because the short version
  understates them: full write authority (create, delete, rename, truncate) on
  the four hierarchies the mount table publishes as writable; `WRITE_FILE`
  alone on `/proc`, `/dev`, `/dev/pts` and each render node, which makes it
  **eight** hierarchies carrying a write right, not four — eight with one
  render node bound, one more for each additional one; an enumerated read
  set whose execute half is narrower still (`/etc` and `/sys` get no
  `EXECUTE`); and nothing else. Size "nothing else" from the measurement rather
  than from the phrase before reporting a read as a breach: probing 31 in-realm
  paths at the default and at `--landlock=off` found eight refused, all of them
  empty directories the realm's own mount table minted to hold a bind target
  beneath them, and **no path carrying data**. The rung is journaled per realm and is
  **child-asserted**: no `/proc` file names a process's Landlock domain, so the
  core cannot corroborate it the way it corroborates the namespace inodes. Note
  also that P2.6.3 is **not finished**: the ruleset landed, and so did a
  generated ladder table with a CI staleness gate — the
  [Landlock ABI matrix](docs/book/src/isolation-matrix.md), which states what
  this build requires of a kernel — but it measures **no** kernel. The
  per-kernel table the task's criteria ask for is a separate artefact,
  [which kernels this build starts on](docs/book/src/isolation-kernels.md):
  five distribution kernels booted under QEMU with the shipped `vitrind`,
  reporting ABI 1, 2, 4, 6 and 7, three of them refused below the floor. Those
  are **kernel** readings taken in a bare initramfs — the number of
  *distributions* whose policy this repository has measured is still one, and
  the suite itself has still only ever run on two machines. Each row records the
  build it was taken with beside the kernel's answers, and `cargo xtask
  kernel-matrix --check` holds that half to this tree — going **red the day the
  floor moves out from under them**, so a row cannot go on describing an older
  binary in silence. That check **re-boots nothing**: re-taking the kernel half
  is `tests/kernel-matrix/collect.sh --check`, which needs QEMU.

  One consequence is worth knowing before you report it as a bug: a Landlock
  domain denies **every mount-topology change** to the process and its
  descendants, unconditionally, so an app that decodes images in a *nested*
  sandbox (`bwrap`, as GTK's `glycin` loaders do) cannot have one and decodes
  **unsandboxed** instead — a real loss of defence in depth, published rather
  than worked around, and widening the ruleset was measured not to repair it.
  A realm additionally refuses nested user namespaces
  (`/proc/sys/user/max_user_namespaces = 0`, written inside the realm's own
  namespace), which takes no capability away — a nested namespace could not
  have mounted anything either — and makes such a sandbox fail at
  `unshare(CLONE_NEWUSER)`, the refusal sandbox libraries already handle,
  rather than at a `mount(2)` they do not expect to fail. See the
  [limits page](docs/book/src/limits.md).

  **The syscall boundary is a DENY-LIST, which is not the same as a
  boundary**: since P2.6.4
  ([#188](https://github.com/vitrin-os/vitrin-os/issues/188))
  `vitrin-realm-init` installs a seccomp-bpf filter
  immediately before the shim's `execve`, inherited by every process the shim
  forks and removable by none of them. It closes the 13 denied syscall rows
  `vitrind --print-seccomp` prints — each naming the PRD Doc 2 §15 escape class
  it answers and the errno it returns — and leaves the rest of the kernel's
  syscall surface **unenumerated**. So treat a realm as *path-confined,
  filesystem-rights-confined and filtered against a named list, but **not**
  syscall-confined*. 11 of the 13 denied syscall rows are demonstrated against
  a positive control on the kernel this was measured on; two (`bpf`, `userfaultfd`) are
  already denied by a sysctl there and are reported *not demonstrated* rather
  than counted. Three residues are published in full on the
  [limits page](docs/book/src/limits.md): the invoking user's supplementary
  groups survive into the realm because the kernel gives no window to drop
  them, the GPU render node is bound read-write with its ioctl surface intact
  (Landlock's `IOCTL_DEV` right is one bit per hierarchy and the app needs the
  node, so the ruleset grants it there), and `--isolation=off` -- or
  `--landlock=off`, which turns off the ruleset alone -- restores a weaker path
  for anyone who names it. **Do not treat a realm as a security boundary against hostile code yet.**
- **`--isolation=default` refusing to start is designed behaviour, not a
  vulnerability** ([#286](https://github.com/vitrin-os/vitrin-os/issues/286)).
  The confinement above needs a host that lets an unprivileged user namespace
  carry its capabilities. Where a host permits the `unshare` and then strips
  them, the first mount inside the namespace fails, and the startup preflight
  **refuses the session** rather than starting a weaker one — D-020(6) forbids
  silent degradation. Measured on a GitHub `ubuntu-latest` runner (kernel
  `6.17.0-1020-azure`, 2026-08-14), where
  `kernel.apparmor_restrict_unprivileged_userns` is `1` on that stock image —
  read from the runner's own sysctl before CI granted itself the remedy — and
  no realm can start. **That is one CI image, not a distribution
  survey** — [#281](https://github.com/vitrin-os/vitrin-os/issues/281) owns the
  cross-kernel matrix. Report a *silent* degradation here; a loud refusal is
  the feature. Note that anything reproduced under `--isolation=off` is a
  finding about an explicitly unconfined session and is triaged as such.
  **`packaging/apparmor/vitrind` is the per-binary grant for such a host, and
  the `apparmor-profile` CI job measures it working** — on kernel
  `6.17.0-1022-azure` it took `mount.in_userns` from
  `restricted-by-policy(errno=13)` to `available` and `tier` from `none` to
  `per-uid`, spawned a real realm, and failed again with the profile removed.
  That is **one kernel on one CI image**; nobody has loaded it on an installed
  Ubuntu system, and nothing in the repository installs it
  ([#293](https://github.com/vitrin-os/vitrin-os/issues/293)) — a build not at
  the paths the profile names is not covered by it. Two things about
  it are worth a reporter's attention rather than a report. First, a profile of
  that shape (`flags=(unconfined)` carrying a `userns` rule) is a **name any
  local user may try to borrow** with `aa-exec -p vitrind`. Whether that borrow
  yields an unrestricted user namespace depends on
  `kernel.apparmor_restrict_unprivileged_unconfined`: at `0` it does, and at
  `1` AppArmor stacks the borrowed profile with `unconfined` instead of
  transitioning to it, so the restriction is retained. **Measured `0` on a
  stock `ubuntu-latest` (2026-08-15), so on that machine the borrow works and
  the cost is real and unmitigated.** Either way it is a known, published property of the mechanism
  Ubuntu ships, shared with the `chrome`, `firefox` and `flatpak` profiles, and
  is not a vulnerability in this project. **Do send us a report if you can show
  the borrow succeeding on a host where that knob reads `1`** — that would
  contradict the cited behaviour rather than restate it, and the limits page
  spells out exactly which sentence it would falsify. Second, a session that
  came up **only** because that profile was borrowed is not a confined session
  this project claims anything about.
- **A missing Landlock also refuses the session, and that is designed
  behaviour too.** Since P2.6.3 the ruleset is in this build's confinement
  *floor*, so a kernel that answers the ABI query with `ENOSYS` no longer
  starts a session confined by mount table alone — it stops, on the same
  D-020(6) reasoning as the bullet above. Stated positively, the host must
  provide all three of: a **kernel ≥ 5.13**, **`CONFIG_SECURITY_LANDLOCK=y`**,
  and **`landlock` in the active LSM list** (`/sys/kernel/security/lsm` — a
  kernel can carry the code and leave it out of `lsm=`). `vitrind
  --print-isolation` answers all three as `landlock.abi=N`. Since 2026-08-15
  there is a **fourth** requirement and it is not a host misconfiguration: the
  reported ABI must be at or above this build's declared floor
  (`build.landlock_min_abi` from `vitrind --print-floor`, **6** here — it was 7
  until 2026-08-16, when it was lowered to the lowest rung at which this build's
  enforced domain is unchanged). A working
  Landlock on an older kernel is refused with `below-floor(abi=N,required=M)`
  rather than confined at a weaker rung; the remedy is a newer kernel and no
  knob substitutes. Which kernels that admits is measured on five of them —
  see [the kernel page](docs/book/src/isolation-kernels.md). A refusal on any of
  the four is designed behaviour, not a finding.
  **The two refusals must not be confused, because their remedies are
  disjoint**: the message names the mechanism it could not get, `namespaces`
  for the bullet above and `landlock` for this one. No userns sysctl makes a
  kernel report a Landlock ABI, and adding `landlock` to `lsm=` restores no
  capability a user namespace was stripped of. The conditions are independent:
  the runner where the namespace refusal was measured ran a 6.17 kernel, four
  years past Landlock's 5.13. Which distributions ship the third requirement
  unset has **not been surveyed here** — that is
  [#281](https://github.com/vitrin-os/vitrin-os/issues/281). `--landlock=off`
  is not a remedy for a kernel that could be configured; it starts realms with
  no ruleset at all, and every grant described above stops applying to that
  session.
- **The session D-Bus is reachable at `--isolation=off`, and not at
  `--isolation=default`.** The core injects no `DBUS_SESSION_BUS_ADDRESS` and
  points `XDG_RUNTIME_DIR` at the realm's private directory in either mode.
  Unconfined that is advertisement rather than reachability:
  `/run/user/<uid>/bus` is still on the filesystem, still connectable by any
  process of this uid, and the abstract-socket namespace is still shared —
  and an operator who allow-lists `DBUS_SESSION_BUS_ADDRESS` in `realm.toml`
  there hands that realm the host's own bus. Since P2.6.2 the closure is the
  kernel's rather than this project's cleverness: the realm's mount table
  publishes a `/run` holding one entry, `vitrin`, so the bus has no path, and
  `CLONE_NEWNET` — in the same six-flag `unshare` as the rest
  (`crates/vitrin-realm-init/src/main.rs`, `CLONE_FLAGS`) — takes the
  abstract-socket namespace with it, because abstract sockets are scoped to a
  network namespace. That hedge is a mechanism hedge, and the evidentiary one
  belongs beside it rather than instead of it: the closure is read off the
  realm's mount table and the namespace inodes the core verifies at spawn, not
  off any probe. **No test in the tree attempts the bus from inside a realm** —
  `tests/integration/test_real_confinement.py` says in as many words that it
  asserts nothing about a realm reaching the session bus by other means, and
  P2.1.10's adversarial probe, which would attempt every route from inside a
  realm to the host session's bus, does not exist (E2.1,
  `docs/plan/02-phase-2-semantic-epochs.md`). On a page about reporting
  vulnerabilities that distinction is the whole point: "the kernel closed it"
  and "nobody has tried to open it" are different claims, and only the first is
  being made here. So a report here has to name the isolation mode it was taken
  under — and naming `default` is not sufficient on its own. The `binds` key
  names any absolute path outside `/` and `/home`, so an operator who binds the
  host's runtime directory into a realm puts the bus socket back inside it
  under a key that says nothing about buses; name the `realm.toml` `binds` list
  too, because a `default` realm carrying that bind is not the configuration
  this bullet describes as closed. What P13 still owes is not the namespaces but **designated
  egress**: the per-realm proxy, host:port-scoped grants, DNS resolution
  pinned at grant time, and the scripted `ssh localhost` adversarial gate
  (E2.7, `docs/plan/02-phase-2-semantic-epochs.md`) — none of which has
  landed, and there is no `tests/integration/test_real_ssh_localhost.py` in
  the tree.
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
- **The consent gate proves occlusion, not the physical click.**
  [#138](https://github.com/vitrin-os/vitrin-os/issues/138) closed the half
  [#109](https://github.com/vitrin-os/vitrin-os/issues/109) left open:
  `tests/integration/test_real_consent.py` drives the shipped
  `target/debug/vitrind` over a real socket against a real app, shows the
  exported footprint to *be* a raster of the core's own card on exactly the
  rectangle the core named — accent ring on all four edges, its exact
  perimeter count, body, buttons — *before* the absence of the app's pixels is
  read out of it, and shows the capture path moving zero pixels while the
  agent's own mid-prompt `observe()` still carries the live app. What it does
  **not** prove is enumerated under "What the consent gate still does not
  prove" in [`tests/integration/README.md`](tests/integration/README.md), and
  the largest item is this one: headless has no pointer for a human to click
  with, so the click is stood in for by a build-gated injector socket, and
  while the injected decision is drained by the same `service_consent_round`
  and `resolve_human` a real click reaches, it **bypasses `judge` entirely**.
  The hit test, the 500 ms guard interval, the press-arms/release-commits
  ladder and the origin check that stops an agent answering its own prompt are
  held only by `crates/vitrin-core/src/consent/grab.rs`'s own tests and by a
  human at a mouse (`shim/docs/firefox.md` §9). Nor does the gate say anything
  about whether the card is framed in a colour a confined app cannot forge —
  that is [#139](https://github.com/vitrin-os/vitrin-os/issues/139)'s half of
  [#85](https://github.com/vitrin-os/vitrin-os/issues/85), adjudicated as not
  an M1.4 criterion. A concrete occlusion demonstrated against the shipped
  binary is still genuinely valuable and should go through the advisory
  channel.
- **The nested backend falls back to CPU compose while an overlay is up**,
  so the host window can show stale content for as long as a consent
  prompt or dead-man indicator lasts (argued in full at
  `crates/vitrin-core/src/backend/winit.rs`, `try_redraw`). Zero-copy's
  own mock-free gate,
  [#117](https://github.com/vitrin-os/vitrin-os/issues/117), has not
  landed.
- **Everything from Phase 2's *semantic* half onward does not exist.**
  Semantic trees, epoch/CAS staleness rejection, the powerbox, the credential
  wallet, network sessions, the X11 shim, the mission-control shell — no code,
  and therefore no vulnerabilities. **Phase 2's confinement track is the
  exception, and is very much code**: P2.6.1 landed as
  `crates/vitrin-core/src/spawn/isolation.rs`, and P2.6.2–P2.6.4 plus P2.7.1
  as the core-owned `crates/vitrin-realm-init` helper, so this sentence must
  not be read as covering them — what they do and do not confine is the first
  bullet of this list rather than a concept awaiting an implementation. See
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §6, which records the same
  two exceptions to the same omission rule, for the same reason — that
  paragraph had already gone stale once.

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
