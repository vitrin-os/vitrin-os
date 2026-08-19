<!-- GENERATED FILE -- DO NOT EDIT.

Produced by `cargo xtask isolation-matrix` from the corpus in
crates/xtask/src/isolation_matrix.rs, the Landlock ladder parsed out of
crates/vitrin-realm-init/src/landlock.rs, and the floor and ceiling read
from crates/vitrin-realm-init/src/lib.rs. `cargo xtask isolation-matrix
--check` re-renders and compares byte-for-byte, and CI runs it, so a hand
edit to this file is a red build -- and so is moving a right to another
rung, or re-tuning the floor, without regenerating.
-->

# The Landlock ABI matrix

PRD §20 says Landlock coverage is kernel-dependent. This page is the table that
sentence is checkable against: **what this build requires of a kernel's Landlock,
and what each rung of the ABI buys the ruleset on the way.**

This build's two numbers, both read out of the source that declares them rather
than typed here:

- **floor — `build.landlock_min_abi` = 6.** A kernel reporting a lower
  Landlock ABI is refused at startup. It is not confined at a weaker rung.
- **ceiling — `build.landlock_max_rung` = 9.** A kernel reporting a higher
  ABI gets a rung-9 ruleset, journaled as
  `isolation.landlock.clamped_by_build`.

Both are printed by `vitrind --print-floor`.

The ladder below has **9 rung numbers naming 6 distinct enforced
domains** — that count is computed from the parsed ladder, not asserted, and the
rungs that collapse into one domain are named in the domain table.

## What this page is a fact about

**This build, not your kernel.** Nothing here probes anything. The generator runs
on a laptop and on a CI runner and must emit the same bytes on both, so it reads
the repository and never the machine — and the two machines this repository has
actually run report different Landlock ABIs (the development box 9,
the CI runner 7), which a probing generator could not have
reconciled into one checked-in page.

The machine half is a command you run:

```console
$ vitrind --print-isolation | grep landlock
$ vitrind --print-floor | grep landlock
```

The first prints what your kernel answers; the second prints the two build
numbers above. The next table says what this build does with each possible
answer. **Which kernel releases produce which answer is not stated anywhere on
this page, because it was not measured here** — that mapping is a fact about
mainline and about distributions, and this page probes neither. It is measured
on a page of its own: [which kernels this build starts on](isolation-kernels.md),
from boot rows checked in under `tests/kernel-matrix/rows/`.

## Read your own kernel against it

Every cell below is a property of this build's own code — `spawn::isolation`'s
`Report::mechanism` for the verdict and `landlock::apply_with` for the second
refusal — with the floor at 6 and the ceiling at 9.

| `vitrind --print-isolation` says | what this build does |
|---|---|
| `landlock.abi=N` with N at or above `build.landlock_min_abi` and at or below `build.landlock_max_rung` | Starts. The helper asks for rung N and journals the rung it obtained, the rung it asked for, and the ABI the kernel reported. |
| `landlock.abi=N` with N above `build.landlock_max_rung` | Starts, at the ceiling. The request is clamped down and the clamp is journaled as `isolation.landlock.clamped_by_build`. |
| `landlock.abi=N` with N at or above 1 and below `build.landlock_min_abi` | **Refuses to start** at `--isolation=default`, reporting `below-floor(abi=N,required=M)`. The remedy is a newer kernel, explicitly *not* a sysctl, an `lsm=` edit or a `CONFIG_` change — those are already correct on such a machine. |
| `landlock.abi=absent(errno=E)` | **Refuses to start**: the kernel does not implement the syscall. Check `CONFIG_SECURITY_LANDLOCK` and the kernel version. |
| `landlock.abi=restricted-by-policy(errno=E)` | **Refuses to start**: the kernel has Landlock and something above it said no — most often `landlock` missing from the active `lsm=` list. |
| any of the above, with `--landlock=off` on the command line | Starts with **no ruleset at all**, journaling `namespaces-only`. It is not a remedy for a kernel that could be upgraded, and no confinement claim on this page applies to such a session. |
| any of the above, with `--landlock=abi:N` on the command line | Pins the request to rung N, **including below the floor**, because it is the instrument every per-rung measurement in this repository is taken with. A session pinned below the floor warns that no published confinement claim applies to its realms. |

## The ladder, one row per ABI rung

`what it buys` is the right or facility the rung adds. `axis` is which field of
the request it moves, and it decides whether `--landlock=abi:N` can *simulate* a
kernel without the rung: the cap sets `handled_access_fs` and `scoped`, so it can;
it does not set the `landlock_restrict_self` flags word or `handled_access_net`,
so for those rungs there is nothing for a cap to take away.

| ABI | what it buys | axis | capping simulates it | this build asks for it | `handled_access_fs` | `scoped` | vs. this build's floor | published claim |
|---|---|---|---|---|---|---|---|---|
| 1 | the base access-mask bits — `EXECUTE`, `WRITE_FILE`, `READ_FILE`, `READ_DIR`, the `REMOVE_*` pair and the seven `MAKE_*` bits | `handled_access_fs` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0x1fff` | `0x0` | **below the floor** — a session refuses to start with `below-floor(abi=1,required=6)`; reachable only through `--landlock=abi:1`, which warns that no published confinement claim applies | `refer-makes-the-cap-a-dial`, `abi-floor-refuses-below-the-number`, `sub-floor-rungs-hold-the-dial-not-the-floor` |
| 2 | `LANDLOCK_ACCESS_FS_REFER` | `handled_access_fs` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0x3fff` | `0x0` | **below the floor** — a session refuses to start with `below-floor(abi=2,required=6)`; reachable only through `--landlock=abi:2`, which warns that no published confinement claim applies | `refer-makes-the-cap-a-dial`, `sub-floor-rungs-hold-the-dial-not-the-floor` |
| 3 | `LANDLOCK_ACCESS_FS_TRUNCATE` | `handled_access_fs` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0x7fff` | `0x0` | **below the floor** — a session refuses to start with `below-floor(abi=3,required=6)`; reachable only through `--landlock=abi:3`, which warns that no published confinement claim applies | `truncate-arrives-at-abi-3`, `sub-floor-rungs-hold-the-dial-not-the-floor` |
| 4 | `handled_access_net` — TCP bind/connect scoping by port | `handled_access_net` | **no** — not an access-mask bit | no — the realm's own network namespace carries that claim structurally and far more completely, since it covers UDP and raw sockets too | `0x7fff` | `0x0` | **below the floor** — a session refuses to start with `below-floor(abi=4,required=6)`; reachable only through `--landlock=abi:4`, which warns that no published confinement claim applies | `net-scoping-is-carried-by-the-namespace`, `nine-rungs-are-six-domains`, `sub-floor-rungs-are-not-all-exercised` |
| 5 | `LANDLOCK_ACCESS_FS_IOCTL_DEV` | `handled_access_fs` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0xffff` | `0x0` | **below the floor** — a session refuses to start with `below-floor(abi=5,required=6)`; reachable only through `--landlock=abi:5`, which warns that no published confinement claim applies | `ioctl-dev-does-not-close-the-render-node`, `sub-floor-rungs-are-not-all-exercised` |
| 6 | the `scoped` field — `SCOPE_ABSTRACT_UNIX_SOCKET` and `SCOPE_SIGNAL` | `scoped` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0xffff` | `0x3` | at or above the floor — a shipped session runs here | `scoped-is-defence-in-depth` |
| 7 | `landlock_restrict_self` log flags — `LOG_SAME_EXEC_OFF`, `LOG_NEW_EXEC_ON`, `LOG_SUBDOMAINS_OFF` | `landlock_restrict_self` flags | **no** — not an access-mask bit | no — the log flags are observability, not confinement, and no published claim depends on them; the one that is reachable at all is reachable only through the `VITRIN_LANDLOCK_AUDIT` diagnostic in vitrind's own environment | `0xffff` | `0x3` | at or above the floor — a shipped session runs here | `restrict-self-flags-are-not-mask-bits`, `nine-rungs-are-six-domains` |
| 8 | `landlock_restrict_self` `TSYNC` — apply the domain to every thread of the caller | `landlock_restrict_self` flags | **no** — not an access-mask bit | no — the helper is single-threaded by design and enforces the domain on the one thread that then `execve`s, so its shape already carries what `TSYNC` would buy | `0xffff` | `0x3` | at or above the floor — a shipped session runs here | `restrict-self-flags-are-not-mask-bits`, `nine-rungs-are-six-domains` |
| 9 | `LANDLOCK_ACCESS_FS_IOCTL_DEV`'s ladder successor `RESOLVE_UNIX` — `connect(2)` and addressed `sendmsg(2)` restricted to pathname UNIX sockets | `handled_access_fs` | yes — `--landlock=abi:N` reproduces its absence | **yes** | `0x1ffff` | `0x3` | at or above the floor — a shipped session runs here | `the-ladder-stops-at-the-build-ceiling` |
| 10 | not stated here — this build does not define ABI 10's rights, and nothing in this repository has measured them | not known to this build | **no** — not an access-mask bit | no — a build must not name a constant its own headers do not define; a kernel reporting ABI 10 or above is clamped down to this build's ceiling and the clamp is journaled | not requested by this build | not requested by this build | **above this build's ladder** — clamped down to rung 9, journaled as `clamped_by_build` | `the-ladder-stops-at-the-build-ceiling` |

The `handled_access_fs` column is the cumulative mask this build asks a kernel at
that rung for. It is parsed out of `handled_access_fs` in
`crates/vitrin-realm-init/src/landlock.rs` and cross-checked against the measured
table pinned in that crate's `the_rung_masks_pin_a_measured_table`; the two
readings disagreeing stops this page being emitted at all. The rights arrive in
this order: rung 2 → REFER, rung 3 → TRUNCATE, rung 5 → IOCTL_DEV, rung 6 → the `scoped` field, rung 9 → RESOLVE_UNIX.

**Which rungs are exercised is counted from this table, not asserted.** A rung is
counted here when a test in `crates/vitrin-realm-init/src/main.rs` **enters** a
Landlock domain at it and asserts the kernel's own answer — a syscall's outcome
inside the domain, or the kernel's verdict on the request. Building a ruleset at a
rung and never entering it does not count.

- **rung 1** — `a_realm_can_write_where_it_was_granted_and_nowhere_else`, `rung_one_forbids_reparenting_that_the_rung_above_permits`
- **rung 2** — `rung_one_forbids_reparenting_that_the_rung_above_permits`, `the_truncate_rung_is_measured_and_its_absence_is_measured_with_it`
- **rung 3** — `the_truncate_rung_is_measured_and_its_absence_is_measured_with_it`
- **rung 7** — `the_audit_log_flag_is_off_unless_asked_for_and_the_kernel_takes_it`

That is 4 of the 10 rungs on this page. Below the floor the tally is the one
`docs/book/src/limits.md` has to carry word for word:

> below the floor of 6, rungs 1, 2 and 3 are exercised and rungs 4 and 5 are not.

Every cell on an unexercised row is derived from this build's own source and
measured against nothing — keeping the sub-floor tests that exist and adding none
for the rest is decision D-043, not an oversight. Neither half is remembered:
every name above is looked up in that file before this page is emitted, and the
generator refuses to emit when the limits page does not carry that tally.

## What each rung does not buy

The column this table exists for. A ladder printed without it reads as though
every rung is pure gain, and three of the rows below say otherwise.

| ABI | what it buys | what it does **not** buy |
|---|---|---|
| 1 | the base access-mask bits — `EXECUTE`, `WRITE_FILE`, `READ_FILE`, `READ_DIR`, the `REMOVE_*` pair and the seven `MAKE_*` bits | `REFER`, and its absence makes a rung-1 domain **stricter**: it refuses `rename(2)` and `link(2)` across directories even inside the realm's own writable storage. Measured `EXDEV` at rung 1, success at rungs 2–9. |
| 2 | `LANDLOCK_ACCESS_FS_REFER` | a tightening of any kind. Handling `REFER` is what **permits** cross-directory rename, which is how GTK and Firefox write files; a ladder read as "higher is tighter" has this rung backwards. |
| 3 | `LANDLOCK_ACCESS_FS_TRUNCATE` | protection for a path outside every granted write hierarchy, which was never truncatable at any rung. What it adds is that a path the domain grants only `READ_FILE` on can no longer be emptied by `truncate(2)`, `creat(2)` or `O_TRUNC`. |
| 4 | `handled_access_net` — TCP bind/connect scoping by port | anything this build asks for. `handled_access_net` stays zero, so the enforced domain at rung 4 is byte-identical to rung 3 — and because the cap moves `handled_access_fs`, `--landlock=abi:3` cannot simulate a kernel without rung 4. |
| 5 | `LANDLOCK_ACCESS_FS_IOCTL_DEV` | closure of the published render-node limit. The bit is all-or-nothing per hierarchy and the app needs the node's ioctls, so the ruleset **grants** `IOCTL_DEV` on every bound render node and on `/dev/pts`. What the rung buys is denying `ioctl` on every *other* device node in the realm. |
| 6 | the `scoped` field — `SCOPE_ABSTRACT_UNIX_SOCKET` and `SCOPE_SIGNAL` | a claim that rests on it. Both halves are already carried structurally by the realm's namespaces — abstract sockets are per-netns, and the pid namespace already denies signalling outward — so this rung is defence in depth, and no published sentence would become false without it. |
| 7 | `landlock_restrict_self` log flags — `LOG_SAME_EXEC_OFF`, `LOG_NEW_EXEC_ON`, `LOG_SUBDOMAINS_OFF` | any access right — and because it is a **flag** rather than a mask bit, `--landlock=abi:6` cannot simulate a kernel without it. There is nothing for the cap to remove from a request that never asked. |
| 8 | `landlock_restrict_self` `TSYNC` — apply the domain to every thread of the caller | any access right, and — as at rung 7 — nothing a mask cap can take away. `--landlock=abi:7` and `--landlock=abi:8` request the same domain. |
| 9 | `LANDLOCK_ACCESS_FS_IOCTL_DEV`'s ladder successor `RESOLVE_UNIX` — `connect(2)` and addressed `sendmsg(2)` restricted to pathname UNIX sockets | a rung above it that this build knows how to ask for. It travels with every writable hierarchy, because a socket the realm creates for itself — the shim's `wayland-0` among them — must stay connectable to it. |
| 10 | not stated here — this build does not define ABI 10's rights, and nothing in this repository has measured them | anything, for this build. The clamp is asserted against a **constructed** ABI value rather than against a machine that reports one — nothing here has run on such a kernel. |

## The enforced domains

Two rungs enforce the same domain when this build's request is byte-identical at
both — `handled_access_fs`, `scoped` and the `landlock_restrict_self` flags word
together. The grouping below is computed from the parsed ladder. `applied_profile`
still spells every rung differently, so read that string as *which rung was
obtained*, never as *how much confinement*.

Each statement is published **verbatim** on the
[limits page](limits.md), so the two can be compared without anyone adjudicating
a paraphrase.

| domain | rungs | `handled_access_fs` | `scoped` | `restrict_self` flags | what this domain is |
|---|---|---|---|---|---|
| 1 (T1) | 1 | `0x1fff` | `0x0` | `0x0` | `handled_access_fs=0x1fff`, `scoped=0x0`: no `REFER`, so a realm capped at rung 1 cannot `rename(2)` across directories inside its own writable storage — the one rung that is stricter than the rung above it. |
| 2 (T2) | 2 | `0x3fff` | `0x0` | `0x0` | `handled_access_fs=0x3fff`, `scoped=0x0`: `REFER` arrives, and handling it is what permits cross-directory rename inside the realm's own storage. |
| 3 (T3) | 3, 4 | `0x7fff` | `0x0` | `0x0` | `handled_access_fs=0x7fff`, `scoped=0x0`: `TRUNCATE` arrives at rung 3; rung 4 buys `handled_access_net`, which this build leaves zero, so rungs 3 and 4 are one domain. |
| 4 (T4) | 5 | `0xffff` | `0x0` | `0x0` | `handled_access_fs=0xffff`, `scoped=0x0`: `IOCTL_DEV` arrives, and it does not close the render-node limit — the app needs the node's ioctls, so the ruleset grants them there. |
| 5 (T5) | 6, 7, 8 | `0xffff` | `0x3` | `0x0` | `handled_access_fs=0xffff`, `scoped=0x3`: rung 6 adds the `scoped` field; rungs 7 and 8 buy `landlock_restrict_self` flags rather than access-mask bits, so a mask cap cannot simulate their absence and rungs 6, 7 and 8 are one domain. |
| 6 (T6) | 9 | `0x1ffff` | `0x3` | `0x0` | `handled_access_fs=0x1ffff`, `scoped=0x3`: `RESOLVE_UNIX` arrives, and this is the highest rung this build requests — a kernel reporting a higher ABI is clamped here. |

The flags column is zero at every rung because a **shipped** session passes zero.
The one thing that moves it is `VITRIN_LANDLOCK_AUDIT=1` in vitrind's own
environment, which sets rung 7's `LOG_NEW_EXEC_ON` so the kernel keeps logging a
realm's denials past the shim's `execve`. It changes what the kernel writes down,
never what it permits, and it cannot be reached from `realm.toml` or a command
line — so under it rungs 6 and 7 stop being one domain **in the log flags only**.

## What the ruleset denies that the realm's mount table does not

A realm is confined by a mount table *and* a Landlock domain, and most of what
the domain refuses the mount table refuses too. Publishing the overlap as though
the ruleset earned it would be the flattering direction, so this table is only
the difference — and it is short.

| the denial | why the mount table does not already carry it | what has been measured | published claim |
|---|---|---|---|
| `execve(2)` anywhere under `/etc` | `/etc` is bound `MS_RDONLY`, `MS_NOSUID`, `MS_NODEV` and with **no** `noexec`, so the mount itself permits execution there. Everywhere else the two maps agree: the ruleset grants `EXECUTE` exactly where the mount table omits `noexec`. | **Nothing measures it.** No test in this repository exercises this denial, which makes it the one row here that is prose rather than measurement. Said plainly rather than left implied. | `execute-under-etc-is-the-rulesets-own-denial` |

## Every claim this table carries, and where it is published

A row with a right and no claim, or a claim with no row, stops the generator.
Each needle below is checked against the surface it names on every run, so a
published sentence cannot be deleted or reworded while this table still cites it.

| claim | what it says | published at |
|---|---|---|
| `abi-floor-refuses-below-the-number` | A kernel reporting a Landlock ABI below this build's floor is refused at startup rather than confined at a weaker rung, and the number is printed as `build.landlock_min_abi`. | `docs/book/src/limits.md` — “build.landlock_min_abi”; `README.md` — “build.landlock_min_abi”; `SECURITY.md` — “build.landlock_min_abi” |
| `sub-floor-rungs-hold-the-dial-not-the-floor` | Rungs below this build's floor are unreachable in production -- a kernel reporting one is REFUSED at startup rather than confined weakly -- so a behavioural test taken at one of them holds the `--landlock=abi:N` DIAL honest and not the floor. This row is a rung such a test enters a domain at: it describes no state a stock session can reach, and those tests are the only evidence that this part of the table is not fiction (decision D-043, 2026-08-19). | `docs/book/src/limits.md` — “hold the dial honest, not the floor” |
| `sub-floor-rungs-are-not-all-exercised` | This rung is below the floor AND no behavioural test enters a Landlock domain at it, so every cell on this row is derived from this build's own source and measured against nothing -- the sub-floor half of the ladder is exercised in part, not throughout. D-043 (2026-08-19) kept the sub-floor tests that exist and deliberately added none, so this row's status is a decision rather than an oversight. | `docs/book/src/limits.md` — “exercised in part, not throughout” |
| `refer-makes-the-cap-a-dial` | A domain denies cross-directory `rename(2)` unless its ruleset HANDLES `REFER`, so rung 1 is stricter about reparenting than rung 2 -- the rung cap is a dial, not a one-way weakening. | `docs/book/src/limits.md` — “The cap is a dial, not a one-way weakening”; `README.md` — “dial, not a one-way tightening” |
| `truncate-arrives-at-abi-3` | Below ABI 3 there is no `TRUNCATE` right, so a payload that cannot write a file can still empty it -- measured at rung 2 succeeding and rung 3 refusing. | `docs/book/src/limits.md` — “Below ABI 3 there is no `TRUNCATE` right” |
| `net-scoping-is-carried-by-the-namespace` | ABI 4 buys network scoping, which this build leaves zero because the realm's own network namespace carries that claim and covers UDP and raw sockets too. | `docs/book/src/limits.md` — “ABI 4 is network scoping” |
| `ioctl-dev-does-not-close-the-render-node` | ABI 5's `IOCTL_DEV` is one all-or-nothing bit per hierarchy and the app needs the render node's ioctls, so the ruleset grants them there and the published render-node limit survives the rung intact. | `docs/book/src/limits.md` — “It does not close the render-node limit below.”; `README.md` — “the ruleset grants it there and this cost is unchanged”; `SECURITY.md` — “the app needs the node, so the ruleset grants it there” |
| `scoped-is-defence-in-depth` | ABI 6's `scoped` field is defence in depth rather than the mechanism behind any published claim: the realm's network namespace already isolates abstract UNIX sockets and its pid namespace already denies signalling outward. | `docs/book/src/limits.md` — “ABI 6's `scoped` field is defence in depth rather than the mechanism behind either published claim” |
| `restrict-self-flags-are-not-mask-bits` | ABI 7 and ABI 8 buy `landlock_restrict_self` FLAGS rather than access-mask bits, so `--landlock=abi:N` cannot simulate their absence and those rungs are prose-backed rather than measurable here. | `docs/book/src/limits.md` — “`landlock_restrict_self` *flags*”; `README.md` — “byte-identical at rungs 3 and 4 and again at rungs 6, 7 and 8” |
| `nine-rungs-are-six-domains` | Rung numbers and enforced domains are not the same count: rungs that buy nothing this build requests collapse into their predecessor's domain, while `applied_profile` still spells every rung differently. | `docs/book/src/limits.md` — “Nine rung numbers name six different domains” |
| `the-ladder-stops-at-the-build-ceiling` | This build's ladder stops at its ceiling and a newer kernel is clamped down to it, journaled per realm as `isolation.landlock.clamped_by_build`; nothing here has run on such a kernel. | `docs/book/src/limits.md` — “This build's ladder stops at rung 9” |
| `execute-under-etc-is-the-rulesets-own-denial` | `/etc` is bound read-only with no `noexec`, so the mount permits execution there and only the Landlock ruleset refuses it -- the one filesystem denial this layer contributes that the mount table does not already carry. | `docs/book/src/limits.md` — “only this ruleset refuses it, and no test in this repository measures that denial yet” |

## What is NOT on this page

- **A per-kernel measurement.** `docs/plan/02-phase-2-semantic-epochs.md`'s
  restated criteria for P2.6.3 ask for a table generated "from `vitrind
  --print-isolation` output on each kernel in the CI matrix, one row per ABI
  actually reported". That is not what this is, and it is not a step towards it
  that was left half-taken: a table carrying the ABI of the machine that
  generated it cannot be byte-stable across two machines, so it cannot be the
  thing CI holds. The plan carries that as Correction 5.
- **Which kernels clear the floor — measured elsewhere, not here.** This page
  still probes nothing. Since 2026-08-16 the per-kernel measurement exists as its
  own artefact: [the kernel page](isolation-kernels.md), generated from boot logs
  checked in under `tests/kernel-matrix/rows/`. Read the two together and do not
  confuse them — this page says what the *build* requires, that one says what five
  *kernels* answered and which of them the floor of 6 admits. Two live machines
  are also on record: this repository's
  development box at Landlock ABI 9 on 2026-08-15, and the runner its CI uses
  at ABI 7 on 2026-08-14. The runner's number was read out of a CI job log that
  archives nothing, and it is corroborated but **not** replaced by the kernel page:
  booting the runner's own `6.17.0-1020-azure` in a bare initramfs answers ABI 7
  too, which is a fact about that kernel and not about that runner.
- **Any statement that P2.6.3's criteria were all met as written.** The task
  (issue #187) closed on 2026-08-19, on its *corrected* criteria and on decision
  D-043 — not on the row the plan first wrote. What landed with this page is a
  generated ladder of what this build requires, held by CI. A per-kernel row set
  landed separately on 2026-08-16 — five kernels, on [the kernel
  page](isolation-kernels.md) — and it is a row per *kernel*, not the "one row per
  ABI actually reported" the criteria ask for, a clause no byte-stable checked-in
  page can satisfy (the plan carries that as Correction 5). Four things did not
  become true when the task closed: five kernels answered five ABIs, and four of
  the nine rungs are reported by none of them; every row on that page is a
  **kernel** reading taken in a bare initramfs rather than a distribution; the
  behavioural per-rung tests this page's numbers rest on still live in
  `vitrin-realm-init`'s own suite, on one box; and the sub-floor half of those
  tests is evidence about the `--landlock=abi:N` dial rather than about any state a
  stock session reaches.
- **The realm's grant table.** Which hierarchies get which rights is
  [the limits page](limits.md)'s two-tier grant list, not a per-rung fact. The
  only grant-table row here is the one denial the mount table does not carry.

## Runbook

**To change what this page says, change the code or the published prose — never
this file.**

```console
$ cargo xtask isolation-matrix          # regenerate in place, then review `git diff`
$ cargo xtask isolation-matrix --check  # what CI runs; reads only, writes nothing
```

The generator refuses to emit anything when:

1. a pinned line of `crates/vitrin-realm-init/` or `crates/vitrin-core/` source
   is gone, so a cell here would describe code that no longer exists;
2. `LANDLOCK_MIN_ABI` or `LANDLOCK_BUILD_MAX_RUNG` cannot be read, or the ladder
   in `landlock.rs` cannot be parsed — a shape the parser does not recognise is an
   error, never a rung silently dropped;
3. the parsed ladder disagrees with the measured mask table pinned in
   `the_rung_masks_pin_a_measured_table`;
4. a rung row names no published claim, or a published claim is named by no row;
5. a claim's needle is no longer on the surface it cites;
6. a domain has no tier statement, or a tier statement is not on the limits page
   verbatim.

Adding a rung is therefore not an edit to this page: move the right in
`landlock.rs`, publish what the rung is worth, add the row and its claim to
`crates/xtask/src/isolation_matrix.rs`, and regenerate.
