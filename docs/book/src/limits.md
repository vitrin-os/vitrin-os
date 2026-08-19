# Where this is honest about its limits

Phase 1 is complete. That is a statement about a defined slice closing on
named, mock-free gates — not a statement that this is ready for anything
real. This page is the whole list, in one place, so you never have to
discover an item on it yourself.

## Do not deploy this yet

**The sandbox is half-built, and which half you have depends on a flag.**
Decision D9, then D-020 and D-036. Since P2.6.2 a realm is spawned into **six
namespaces** — user, mount, PID, IPC, UTS and network — with an identity
uid/gid map, **zero capabilities**, and a private mount table the app cannot
reshape. The core verifies all of that from **outside**, by reading the
kernel's answer about the child, and refuses the spawn when it cannot.

Since P2.6.3 the realm gets a **Landlock ruleset**, enforced immediately before
the shim's `execve` and inherited by every process the realm ever runs. The
task's second deliverable — a *generated* ladder table with a CI staleness gate
— now exists too: [the Landlock ABI matrix](isolation-matrix.md), emitted by
`cargo xtask isolation-matrix` and held byte-for-byte by CI. **It is not the
per-kernel table the task's restated criteria describe**, and the difference is
not a detail: it publishes what *this build* requires of a kernel's Landlock,
one row per ABI rung, and it probes nothing. The row-per-ABI-*actually-reported*
half now exists separately, and is **measured**: [which kernels this build
starts on](isolation-kernels.md) carries five distribution kernels booted under
QEMU with the shipped `vitrind`, reporting ABI 1, 2, 4, 6 and 7 — three refused
below the floor, two admitted. Read the two pages as what they each are: one
says what this build requires of a kernel, the other says what five kernels
answered.

What has **not** grown is the number of machines on which the suite itself has
ever run, which is still exactly two. A kernel row is one binary printing its
isolation report in a minimal initramfs; it is not a run of `run.sh`, and it is
not a statement about the distribution that ships that kernel. The remaining
gap is narrower than it was and is its own entry below.

The grant set is two tiers, and one sentence about "the write set" gets it
wrong in the flattering direction — an earlier draft of this page said "its
write set is exactly the four hierarchies the mount table already publishes",
and that was false. Read from `crates/vitrin-realm-init/src/landlock.rs`'s
`grants`, which is the only authority:

- **Full write authority — create, delete, rename, truncate — on exactly the
  four hierarchies the mount table publishes as writable**: `/run/vitrin`,
  `/vitrin/home`, `/tmp` and `/dev/shm`.
- **`WRITE_FILE` and nothing else on four more**: `/proc` (real software writes
  its own `/proc/self/*`), `/dev` and `/dev/pts` (writing *through* a device
  node, never creating one — `/dev` is a read-only mount by the time the
  ruleset is built), and every bound render node. So the ruleset requests
  `WRITE_FILE` on **eight** hierarchies, not four — and eight is the count on a
  host with **one** render node, since `render_nodes` is a list and each entry
  is its own rule, so a two-GPU host is nine. None of those four carries
  `TRUNCATE`, any `MAKE_*`, any `REMOVE_*` or `REFER` — nothing there can be
  created, deleted, renamed or emptied — but "the write set is the four
  writable mounts" is still the wrong sentence, and the difference is exactly
  the kind a reader is entitled to have stated rather than to find in the
  source.
- **The read set is enumerated** rather than granted at the realm root, and the
  **execute** half is narrower than the read half. Read+execute: `/usr`, the
  `/bin`-class compatibility names, the shim binary, the app's own directory,
  every `binds` entry, and `/tmp` (whose mount is deliberately not `noexec`).
  Read only, **no execute**: `/etc` and `/sys`. Read+write, no execute:
  `/proc` and `/dev`. `/dev/pts` additionally carries `IOCTL_DEV`, and each
  bound render node carries read+write+`IOCTL_DEV` (see below). Nothing else is
  granted at all — a read of a path the mount table happened to leave reachable
  but nothing granted now fails `EACCES` rather than succeeding. **Three** of
  those rows are a *file* rather than a hierarchy — the shim binary, any
  `binds` entry naming a file, and every render node (a character device) — and
  they are granted **without** the directory-list right, because the kernel
  refuses a rule that names `LANDLOCK_ACCESS_FS_READ_DIR` on anything that is
  not a directory. Refuses the rule, not merely the right: measured `EINVAL`
  here (ABI 9, 2026-08-14 for the regular file, 2026-08-15 for the render
  node), and since every one of those rules is required and the helper fails
  closed, granting any of the three the wrong rights takes **every realm on the
  box** down at the shipped default — which is how both were found.
- **`/etc` is the one place the ruleset denies something the mount table does
  not.** Everywhere else the two maps agree on execution: the ruleset grants
  `EXECUTE` exactly where the mount table omits `noexec`. `/etc` is bound
  `MS_RDONLY|MS_NOSUID|MS_NODEV` with **no** `noexec`, so the mount permits
  `execve(2)` there and only this ruleset refuses it, and no test in this
  repository measures that denial yet. It is the only row of the [ABI
  matrix](isolation-matrix.md)'s "what the ruleset denies that the mount table
  does not" table, and that page prints the same unmeasured status beside it —
  an unmeasured denial is prose, and is published as prose.

**And "nothing else" is a small set — which is the honest half of that
sentence, and it is published because the enumeration is the stronger-sounding
claim.** Measured on this repo's box (Arch, kernel `7.1.8-arch1-3`, Landlock ABI
9, 2026-08-14) with `solid-client --probe` over **31 distinct in-realm paths**,
in batches of eight, each batch run twice — shipped default and
`--landlock=off`, byte-identical `realm.toml` asserted between the pair, and at
least one path per batch reachable in both runs so a report from an app that
could open nothing cannot satisfy a denial. **Eight** paths were refused
`EACCES` at the default and opened at `--landlock=off`:

| denied at the default, reachable at `--landlock=off` | what it is |
|---|---|
| `/` | the realm root |
| `/run`, `/vitrin` | the parents of `/run/vitrin`, `/vitrin/home` and the shim binary |
| `/home`, `/home/<user>`, `…/projects`, `…/projects/vitrin`, `…/vitrin/shim` | the parents of this development tree's app-directory and shim-library binds |

Every one of the eight is a directory the realm's **own** mount table created on
its root tmpfs purely to hold a bind target beneath it, and each holds nothing
but the next component of that path. Most of the `/home` chain is an artefact of
running from a build tree: with the app relocated to a directory already inside
a granted hierarchy, a second run's probed denials were `/`, `/run`, `/vitrin`
and `/home` — the deeper components stayed only because *this tree's shim*
needed its `subprojects/wlroots` directory bound, which a packaged install would
not. Every other probed
path answered identically at both settings: `/usr`, `/etc`, `/proc`, `/sys`,
`/tmp`, `/dev` and its nodes, `/dev/shm`, `/dev/pts`, `/dev/dri`,
`/vitrin/home`, the shim binary, `/run/vitrin`, the `/bin`-class symlinks and
files inside them all opened in both; `/dev/tty` answered `ENXIO` in both, which
is a realm with no controlling terminal and not a ruleset denial.

**Two things this measurement's spellings predate, neither of which moves the
boundary it reports.** Issue [#283](https://github.com/vitrin-os/vitrin-os/issues/283)
(a) renamed the shim's bind target from `/vitrin/shim` to
`/vitrin/vitrin-shim`, which changes a leaf's name and no grant, and (b) removed
the shim-library bind the `/home` chain's deeper components existed for, by
linking anything the shim vendors statically instead. So a re-run today should
deny four paths rather than eight when the app is relocated, and should mint no
`…/projects/vitrin/shim` chain at all. **That is a prediction, not a
measurement: the table above is what was actually probed on 2026-08-14 and it
has not been re-collected since.**

So what the enumeration buys over the realm-root grant #187 declined is, on this
host, that the realm cannot **list its own root** and cannot list the handful of
empty directories its mount table had to mint. It denies **no path that
carries data**, because the mount table already puts host content nowhere but at
a bind target and every bind target is granted. That is a narrower thing than
"the read set is enumerated" sounds like, and it is the measured one. It is not
nothing — `/` is exactly the directory a nested sandbox enumerates before
binding, and `test_real_confinement.py` holds that denial as a gate — but a
reader sizing residual risk should size it at a handful of empty directories,
not at a filesystem.

<!-- limit: seccomp-is-a-deny-list -->
**Since P2.6.4 ([#188](https://github.com/vitrin-os/vitrin-os/issues/188))
there is a seccomp filter, and it is a DENY-LIST — a
named-class claim, never a completeness claim.** `vitrin-realm-init` installs
a classic-BPF program immediately before the shim's `execve`, so the shim and
every process it forks inherit it and cannot remove it. What it closes is the
list `vitrind --print-seccomp` prints: **13 denied syscall rows** today, each
naming the PRD Doc 2 §15 escape class it answers and the errno it returns. What it leaves
open is **everything else, unenumerated** — this build does not know Firefox's
syscall surface, and an allow-list built without a measured trace would fail
closed against the project's own acceptance app. So a realm is now
*syscall-filtered against a named list* and is **not** "syscall-confined": the
residual surface is the kernel's whole surface minus 13 denied syscall rows,
and nobody here has counted what that leaves.

Read that beside the Landlock sentence above, because the two are the same
shape and P2.9.4's cross-check compares them: **Landlock's read set is
enumerated and denies a handful of empty directories; seccomp's deny set is
enumerated and denies thirteen syscalls.** Neither is a boundary around
"everything an app might do". Both are lists, and a list is exactly as large
as it is.

Four things about that filter that a reader must not have to infer:

- **It answers two of §15's eight actor rows, and part of a third.** The two
  are *Compromised shim* — the only §15 row that names seccomp at all — and
  *Malicious app in a shim*, at its kernel-attack-surface half. The third is
  *Reachable-service lateral escape*, and it is answered at **two services PRD
  Doc 2 §4.5's own "there is simply nothing to reach" sentence does not
  cover**: the operator's kernel keyring, and `AF_VSOCK`. The remaining five
  rows — ransomware, hijacked agent, malicious agent client, malicious relying
  app, impersonating publisher — are answered by mechanisms that are not
  seccomp, and by nothing in this filter.
- **11 of the 13 denied syscall rows are DEMONSTRATED on the kernel this was
  measured on; two are not.** `tests/integration/test_real_seccomp.py` runs
  the same probe binary inside a realm and at `--isolation=off`, and a row
  whose syscall already fails outside a realm is reported *not demonstrated*
  rather than counted as confinement. `bpf` and `userfaultfd` land there on a
  box with `kernel.unprivileged_bpf_disabled` or
  `vm.unprivileged_userfaultfd` set — the denial is real, and on that machine
  it is not confinement *this filter* adds. Which rows are demonstrated is a
  property of the kernel, so it is measured per run and printed, never
  declared.
- **A realm cannot execute a foreign-ABI binary.** Syscall numbers are
  per-ABI, so a process running under i386 or x32 on an x86-64 kernel meets a
  table whose numbers mean other syscalls. The filter kills it
  (`SECCOMP_RET_KILL_PROCESS`) rather than passing it unfiltered. A 32-bit app
  in a realm dies with `SIGSYS` on its first syscall.
- **The crash reporter is the acceptance app's casualty, and the gate does not
  cover it.** The `ptrace` row denies the pinned Firefox's minidump writer.
  `test_real_firefox.py` sets `MOZ_CRASHREPORTER_DISABLE=1`, so that gate goes
  green *without exercising the path this row breaks*. The green tick is not
  evidence for that row, and this bullet exists so nobody reads it as one.

**Timing, measured rather than asserted (R2.8).** Three acceptance gates, 7
runs each, on Arch `7.1.8-arch1-3`, 2026-08-16, against a control build
identical except that it does not install the filter: `test_real_app.py`
2.139 s vs 2.154 s, `test_real_gtk.py` 1.722 s vs 1.717 s,
`test_real_firefox.py` 2.943 s vs 2.947 s (medians). Every difference is
smaller than the run-to-run spread of either arm, so the honest statement is
**no change measurable at this resolution** — not "negligible", which is a
claim about magnitude this measurement cannot make. Note also what is *not*
measured here: installing a seccomp filter enables the kernel's speculative
store bypass mitigation for the process unless `SECCOMP_FILTER_FLAG_SPEC_ALLOW`
is passed, which this build does not pass. On hardware where that mitigation
costs, the cost is real and no gate here would see it.

What that ruleset is, and what it is not, stated rather than left to be
inferred from the word "Landlock":

- **There is a declared ABI floor, and it is not a ladder** (owner's decision,
  2026-08-15; the number lowered a rung on 2026-08-16). This build targets
  recent kernels: a kernel reporting a Landlock
  ABI below `build.landlock_min_abi` — printed by `vitrind --print-floor`, and
  **6** in this build — is **refused at startup**, with a refusal that names the
  number it found, the number it needed, and the fact that no sysctl, LSM list
  or boot parameter changes either. It does not fall back to a lower rung: a
  realm confined by a weaker domain than the session's own journal names is the
  silent degradation D-020(6) exists to forbid. That is the fourth host
  requirement in the `host-must-have-landlock` entry below.
  **Why 6, and why moving it down from 7 gave up no enforcement.** The floor was
  7 for one day. It is 6 because 6 is the *lowest* rung at which the domain this
  build actually enforces is unchanged: the enforced triple —
  `handled_access_fs`, `scoped`, and the `landlock_restrict_self` flags word —
  is **identical at rungs 6, 7 and 8**, because the only thing rungs 7 and 8 buy
  is flags (audit logging, `TSYNC`) and every shipped run passes flags = 0.
  Rung 5 differs (it is below where `scoped` arrives), which is why the floor
  cannot go lower without giving something up, and rung 9 differs too (it adds
  `RESOLVE_UNIX`) — so *no page here says the domain is identical from 6 to 9*.
  The floor decides **admission**, never which rung is applied: the rung a realm
  gets is still `min(kernel ABI, build ceiling)`, so a machine that supplied
  rung 9 before supplies rung 9 now. All three facts are asserted, not narrated,
  by `the_floor_costs_nothing_because_the_domain_is_flat_from_six_to_eight` in
  `crates/vitrin-realm-init/src/main.rs`.
  **Which kernel releases the floor excludes IS measured now**, and it is
  measured on kernels rather than inferred from mainline changelogs — five
  distribution kernels were booted under QEMU with the shipped binary and their
  answers are checked in: Ubuntu 22.04's `5.15.0-191-generic` at ABI 1, Debian
  12's `6.1.0-50-amd64` at ABI 2 and Ubuntu 24.04's GA `6.8.0-139-generic` at
  ABI 4 are **refused**; Debian 13's `6.12.101+deb13-amd64` at ABI 6 and the
  azure kernel this repository's CI runners boot at ABI 7 **start**.
  See [the kernel page](isolation-kernels.md) for the rows, their provenance,
  and why they are kernel rows and not distribution rows. Two live machines are
  also on record and are a different kind of evidence: this repository's
  development box (Arch, `7.1.8-arch1-3`) answers `landlock.abi=9`, and the
  GitHub runner its CI uses answered `landlock.abi=7` on 2026-08-14 —
  **that second number lives only in a job log**, read out of CI's own
  diagnostic step, and no checked-in artefact carries that *runner* measurement;
  the `host-must-have-landlock` entry below states the bound. (The kernel page
  boots the same kernel release and also reads 7, which corroborates the number
  without making it a fact about that runner — the policy cells differ.)
  What a checked-in file does carry, since issue
  [#288](https://github.com/vitrin-os/vitrin-os/issues/288), is the *claim*:
  `.github/workflows/ci.yml`'s `rust` job sets `VITRIN_REQUIRE_LANDLOCK_ABI:
  "7"`, which turns every Landlock rung measurement at or below 7 from a test
  that may skip into a test that must run. That does not re-take the
  measurement and must not be read as one — it asserts it, so a runner image
  that dropped below 7 would turn the job red instead of skipping five
  measurements quietly, which is what those five did before. Note the two
  numbers are now deliberately different: the *build* floor is 6 and the *CI
  require-variable* is 7, because the second is a statement about the runner's
  kernel and not about what this build needs.
  **This narrows P2.6.3 rather than completing it**: PRD §20's
  "coverage is kernel-dependent" caveat is *deferred*, not answered. Five
  kernels reported five ABIs and four of this build's nine rungs are reported by
  none of them, so the per-rung table the task asks for is still generated from
  source rather than measured on machines. The plan document carries that
  correction in as many words.
- **The rung matters, and the rung *obtained* is what is published.** A
  Landlock ABI rung is which access rights the kernel will police at all. The
  helper asks for the highest rung this build knows that the kernel accepts,
  and journals the rung it got, the rung the session asked for, and the ABI the
  kernel reported. The one-rung-at-a-time descent still exists for a kernel
  that reports ABI N and then refuses rung N's mask, and it **bottoms out at
  the floor** rather than walking to rung 1. `applied_profile` names
  the rung **obtained** — `namespaces+landlock-abi9` on this repo's own box —
  so a session that landed a rung below what it asked for cannot render like
  one that did not, and the core logs a WARN per spawn whenever the obtained
  rung is below
  the request or below the kernel's own ABI, naming both numbers and the rights
  that moved between them. Below ABI 3 there is no `TRUNCATE` right, so a
  payload that cannot *write* a file can still destroy it — measured, not
  asserted: at rung 2 a `truncate(2)` on a read-granted file succeeds and the
  file goes to zero; at rung 3 the same call fails `EACCES` and the file
  survives. `--landlock=abi:N` pins a session to rung N so each rung's absence
  can be measured on a modern kernel — for the rungs that move the mask, which
  is not all of them; see the rung-4/7/8 bullet below — and warns at startup.
  **The cap may still be set below the floor, and that is deliberate**: it is
  the instrument every per-rung measurement on this page is taken with, and a
  kernel that reported the same rung would be refused. A session pinned there
  warns in as many words that no confinement claim this build publishes applies
  to its realms. `--landlock=off` builds no ruleset at all and journals
  `namespaces-only`.
- **The cap is a dial, not a one-way weakening, and rung 1 is *stricter* about
  reparenting.** `--landlock=abi:N` reproduces exactly what this build asks for
  on an ABI-N kernel, which includes reproducing that kernel's strictness — but
  read that sentence with the exception two bullets down, because it is *not*
  the same as reproducing an ABI-N kernel. A Landlock domain denies reparenting —
  `rename(2)` and `link(2)` across directories — whenever its ruleset does not
  *handle* `REFER`, and no ruleset below ABI 2 can. So a realm capped at rung 1
  cannot move a file between two directories **inside its own writable
  storage**, and every rung above it can. Measured on this repo's box (kernel
  `7.1.8-arch1-3`, Landlock ABI 9, 2026-08-14) with the realm's whole writable
  set granted on one hierarchy: rung 1 answers `EXDEV`, rungs 2–9 succeed, and
  a same-directory rename succeeds at every rung. Practically, `--landlock=abi:1`
  breaks every app that writes by rename-into-place (GTK, Firefox). Do not read
  the ladder as "higher is always tighter"; read it as "rung N is ABI N".
- **This build's ladder stops at rung 9, and a newer kernel is clamped to it.**
  ABI 10 exists in mainline and this build does not request it. A kernel
  reporting more than 9 gets a rung-9 ruleset, and that is journaled per realm
  as `isolation.landlock.clamped_by_build`; the constant it is measured against
  is printed by `vitrind --print-floor` as `build.landlock_max_rung`. Nothing
  here has been run on such a kernel — the clamp is asserted against a
  constructed ABI value, not against a machine that reports one.
- **Nine rung numbers name six different domains, and the profile string does
  not say so.** Three rungs buy facilities this build never requests: ABI 4 is
  network scoping (`handled_access_net`, deliberately zero — the realm's own
  network namespace carries that claim), and ABI 7 (audit-log control) and ABI
  8 (`TSYNC`) are `landlock_restrict_self` *flags*, which this build passes as
  **zero in every shipped run**. (The one thing that moves the flags word is a
  diagnostic, `VITRIN_LANDLOCK_AUDIT=1` in vitrind's own environment, which
  sets ABI 7's `LOG_NEW_EXEC_ON` so the kernel keeps logging a realm's denials
  past the shim's `execve`. It changes what the kernel *writes down* and
  nothing about what it permits, it cannot be reached from `realm.toml` or from
  a command line, and under it rungs 6 and 7 stop being byte-identical — in the
  log flags only.) Since none of the three moves anything the helper asks for,
  the
  enforced domain — `handled_access_fs`, `scoped` and the flags word together —
  is **byte-identical at rungs 3 and 4**, and **byte-identical at rungs 6, 7
  and 8**. `--landlock=abi:4` and `--landlock=abi:7` are nevertheless accepted
  and journal `namespaces+landlock-abi4` and `namespaces+landlock-abi7`:
  distinct strings for domains that are not distinct. Read a profile as *which
  rung was requested and obtained*, never as *how much confinement*, and read
  those five rung numbers as **two** rows of the ladder rather than five. Two
  consequences follow, and both cut against this page: capping the mask cannot
  simulate the absence of a facility the build never asked for, so **rungs 4, 7
  and 8 are prose-backed rather than measurable here**; and the per-rung
  measurements quoted on this page are for the rungs that do move the mask (1,
  2, 3, 5, 6, 9). Why neither flag is requested by a shipped session is in
  `crates/vitrin-realm-init/src/landlock.rs`: the helper is single-threaded, so
  its shape already carries what `TSYNC` would buy, and no published claim here
  depends on the log flags — they are pure observability, which is why the one
  of them that is reachable at all is reachable only as a diagnostic.
- **It does not close the render-node limit below.** `IOCTL_DEV` (ABI 5) is one
  all-or-nothing bit per granted hierarchy, and an app that cannot `ioctl` its
  render node cannot render, so the ruleset **grants** it there. What the rung
  buys is denying `ioctl` on every *other* device node in the realm — the
  read-write render node, and everything the next bullet says about it, is
  unchanged.
- **One rung is requested and carries no claim of its own.** ABI 6's `scoped`
  field is defence in depth rather than the mechanism behind either published
  claim it touches: a realm's abstract UNIX sockets are already isolated by its
  own network namespace, and its pid namespace already denies signalling
  outward. The ruleset asks for it anyway, because asking costs nothing and the
  namespaces could one day be relaxed; but no sentence on this page would
  become false if the kernel refused it, and the [ABI
  matrix](isolation-matrix.md) says so in that rung's row rather than implying
  the rung is load-bearing.
- **A realm's app can no longer mount anything, and that breaks nested
  sandboxes.** A Landlock domain denies *every* mount-topology change to the
  process and its descendants, unconditionally — it is not an access right, so
  no rule grants it and widening the ruleset cannot restore it. Measured on
  this box (2026-08-15) with the granted rights held constant at *everything on
  `/`* and only the handled mask varied: with `handled_access_fs = 0` the
  `mount(NULL, "/", NULL, MS_REC|MS_SLAVE, NULL)` returns 0; with `EXECUTE`
  alone handled it returns `EPERM`; with the full rung-9 mask handled and every
  one of those rights granted on `/` it still returns `EPERM`. So a realm's app
  is confined by *this* system's boundary and cannot build a second one inside
  it — the practical casualty is **bubblewrap**, which GTK's `glycin` image
  loaders spawn to decode an SVG.
- **So the realm refuses nested user namespaces outright, and that is a
  hardening rather than a workaround.** Since a domain forbids `mount(2)`
  unconditionally, a user namespace created *inside* a realm can build no mount
  and was already useless; `vitrin-realm-init` writes `0` to the realm's own
  `/proc/sys/user/max_user_namespaces` (step K9b) so that the refusal arrives
  at `unshare(CLONE_NEWUSER)` instead of much later, at the first `mount(2)`.
  Nothing an app could do becomes impossible. What changes is **which error it
  receives**: the conventional "this host does not allow unprivileged user
  namespaces" answer every sandbox library already has a branch for, rather
  than an opaque `EPERM` from deep inside its own setup. It also removes real
  attack surface, nested user-namespace creation being a recurring source of
  kernel CVEs and a realm having no legitimate use for one. Measured mock-free
  from inside a real realm by `tests/integration/test_real_confinement.py`
  (`RealConfinementNestedUserns`): the app's forked `unshare(CLONE_NEWUSER)`
  fails `ENOSPC` at the shipped default **and** at `--landlock=off` — so the
  refusal is the realm's ucount limit, not the ruleset — and succeeds at
  `--isolation=off`, which is the positive control that makes the two negatives
  mean anything.
- **Nested image sandboxes still do not work inside a realm; apps that want one
  now degrade instead of aborting.** That distinction is the entry below, and
  it is a narrower claim than "fixed" in both directions.
- **The rung number is child-asserted; the denial is not.** The namespace
  inodes, the realm's root device and the canary set are read by the core from
  `/proc`. The Landlock **rung** cannot be: no `/proc` file names a process's
  Landlock domain, so the number in the journal is one the helper reported and
  a substituted helper could report anything. What such a helper cannot forge
  is the realm's *behaviour*, and that is measured — mock-free, from inside a
  real realm — by `tests/integration/test_real_confinement.py`, which opens a
  path the mount table leaves reachable and the ruleset does not grant
  (`/vitrin`, the directory holding the realm's own shim and storage). Under
  the default it fails `EACCES`; under `--landlock=off`, same core, same mount
  table, same argv, it succeeds. Neither half is evidence without the other.
  What is still *not* measured that way is any particular rung's rights inside
  a real realm — those are measured in `vitrin-realm-init`'s own suite, where a
  forked child can enforce a capped domain and try the syscall.
- **There is a ladder table now, and it is a table about this build — not
  about kernels. P2.6.3 is still not finished and this page will not say
  otherwise.** The task's own acceptance criteria
  (`docs/plan/02-phase-2-semantic-epochs.md`, P2.6.3) ask for two deliverables:
  the ruleset, which landed, **and** a per-ABI ladder table *generated* on each
  kernel in the CI matrix with CI going red when the checked-in copy is stale.
  What now exists is `cargo xtask isolation-matrix`, which emits
  [the Landlock ABI matrix](isolation-matrix.md), and a `--check` step in
  `.github/workflows/ci.yml` that goes red when the checked-in page is stale.
  **The per-kernel half has since been delivered by
  [#281](https://github.com/vitrin-os/vitrin-os/issues/281)**, and it is no
  longer correct to call it deferred: [which kernels this build starts
  on](isolation-kernels.md) is rendered from five checked-in boot rows under
  `tests/kernel-matrix/rows/`, each holding `vitrind --print-isolation` and
  `--print-floor` verbatim from a QEMU boot of that kernel, with
  `cargo xtask kernel-matrix --check` going red when the page and the rows
  disagree. It stayed a *separate* page rather than becoming a column on the
  ladder, for the reason that generator probes nothing: it parses the rung
  ladder out of the helper's own source and the ABI floor out of the crate that
  declares it, because a page carrying the ABI of the machine that produced it
  could not be byte-identical on this repository's two machines (development
  box: `landlock.abi=9`; CI runner: `landlock.abi=7`) and so could not be the
  thing CI holds. **Which kernel releases clear the floor is now stated, and
  measured** — 6.12 and 6.17 start; 5.15, 6.1 and 6.8 are refused
  `below-floor`. Three things about it are still true and still limits: every
  one of those rows is a **kernel** reading taken in a bare initramfs, so the
  number of *distributions* measured as such is still one; nobody other than
  the author has re-run the collector's own failure levers, which needs QEMU on
  a second machine; and five kernels is five kernels, not a spectrum. What holds
  the *build* half of those rows is a gate rather than anybody's memory: `cargo
  xtask kernel-matrix --check` reads each row's own recorded `floor.mechanism=`
  and `applies.*` lines and holds them to the sets
  `crates/vitrin-core/src/spawn/isolation.rs` declares, so the page goes **red
  the day the floor moves out from under them** and names the mechanism that
  moved. That is worth stating because it has already failed once: P2.6.4 grew
  the floor by two mechanisms and every gate stayed green, because the check
  compared the page against the rows and both were stale together. Read its
  scope narrowly, though — it **re-boots nothing**, so a green pull request says
  the rows describe this build and says nothing whatever about whether these
  kernels still answer this way. Only `tests/kernel-matrix/collect.sh --check`
  re-takes that half; it needs QEMU, no pull request runs it, and every row
  carries the date it was last taken on. PRD §20's
  "coverage is kernel-dependent" caveat is answered for those five and for no
  others. The per-rung *behavioural* statements quoted above
  (the `TRUNCATE` pair, the `REFER` pair) are held by `vitrin-realm-init`'s own
  tests on one box; everything else about a rung is now generated and gated,
  which is a narrower promise than "measured". Do not read "P2.6.3" anywhere in
  this repository as a finished task. The plan document carries the
  corrections, and two of the criteria written there were **wrong on the
  kernel's own terms**; they are restated with the correction visible rather
  than deleted.

### The six enforced domains, stated once so a generated table can be compared

`crates/xtask/src/isolation_matrix.rs` emits one row per distinct enforced
domain and prints the statement below in that row, **byte for byte**. The
generator refuses to emit the page at all when a statement here and the one it
would print differ, so the two cannot drift and nobody has to decide whether a
paraphrase still means the same thing. The domain count is derived from the
parsed ladder rather than typed, which is why "nine rung numbers, six domains"
above is a computed sentence rather than a remembered one.

- **T1 — rung 1.** `handled_access_fs=0x1fff`, `scoped=0x0`: no `REFER`, so a
  realm capped at rung 1 cannot `rename(2)` across directories inside its own
  writable storage — the one rung that is stricter than the rung above it.
- **T2 — rung 2.** `handled_access_fs=0x3fff`, `scoped=0x0`: `REFER` arrives,
  and handling it is what permits cross-directory rename inside the realm's own
  storage.
- **T3 — rungs 3 and 4.** `handled_access_fs=0x7fff`, `scoped=0x0`: `TRUNCATE`
  arrives at rung 3; rung 4 buys `handled_access_net`, which this build leaves
  zero, so rungs 3 and 4 are one domain.
- **T4 — rung 5.** `handled_access_fs=0xffff`, `scoped=0x0`: `IOCTL_DEV`
  arrives, and it does not close the render-node limit — the app needs the
  node's ioctls, so the ruleset grants them there.
- **T5 — rungs 6, 7 and 8.** `handled_access_fs=0xffff`, `scoped=0x3`: rung 6
  adds the `scoped` field; rungs 7 and 8 buy `landlock_restrict_self` flags
  rather than access-mask bits, so a mask cap cannot simulate their absence and
  rungs 6, 7 and 8 are one domain.
- **T6 — rung 9.** `handled_access_fs=0x1ffff`, `scoped=0x3`: `RESOLVE_UNIX`
  arrives, and this is the highest rung this build requests — a kernel
  reporting a higher ABI is clamped here.

Every hexadecimal number above is the mask **this build asks a kernel at that
rung for**, parsed out of `crates/vitrin-realm-init/src/landlock.rs` and
cross-checked against the measured table pinned in that crate's
`the_rung_masks_pin_a_measured_table`. They are not statements that a kernel at
that ABI enforces nothing else — they are statements about the request.

<!-- limit: landlock-breaks-nested-image-sandboxes -->
**Inside a realm, a nested sandbox cannot be built, so an app that decodes
images in one decodes them UNSANDBOXED.** That is the whole of what this entry
now claims, and the wording matters in both directions: nothing here says the
nested sandbox works, and nothing here says an app dies for wanting one.

**What this entry said until 2026-08-15, and why it no longer says it.** It
published that the shipped default took three of this repository's own real-app
gates red — `test_real_actuation.py`'s typing rung (M1.4's actuation half,
#108), `test_real_gtk.py` and `test_real_firefox.py` — with a two-column table
of shipped-default failures against `--landlock=off` passes. **That is no
longer true, and a false published limit is as damaging as a missing one.** The
realm now refuses nested user namespaces (`vitrin-realm-init`'s K9b, the bullet
above), so `bwrap` fails at `unshare(CLONE_NEWUSER)` rather than at its first
`mount(2)`, and `glycin` — which decides bwrap's availability by matching its
stderr against a fixed list of namespace-refusal strings — takes the graceful
fallback it already ships. Re-measured on this repo's box (Arch, kernel
`7.1.8-arch1-3`, Landlock ABI 9, 2026-08-15), each gate run at the **shipped
default** with no flag changed and no app exempted:

| gate | shipped default, 2026-08-14 | shipped default, 2026-08-15 |
|---|---|---|
| `test_real_actuation.py` — typing rung (**M1.4, actuation half, #108**) | **FAIL** | pass |
| `test_real_gtk.py` (supporting — M1.2 render half) | **FAIL** | pass |
| `test_real_firefox.py` (supporting — M1.2 render half) | **FAIL** | pass |

The right-hand column is not an assertion; it is what a whole
`bash tests/integration/run.sh` on that box reported — `Ran 118 tests`, `OK`,
**0 failures and 0 skips**, ending on `full suite: no skips, every named gate
ran`.

One qualification on that sentence, because a page about honesty may not cite
a green suite as if it were a reproducible constant. When it was written the
suite carried a **flake in `tests/integration/test_multi_realm.py`** — unrelated
to Landlock, reproduced on `main` in a clean worktree — which took the whole run
red often enough that "0 failures" was a run that happened rather than a state
the suite returned to.

**That flake was root-caused and fixed under
[#292](https://github.com/vitrin-os/vitrin-os/issues/292)**, and the numbers are
worth stating because they are the only thing that distinguishes a fix from a
re-run. It was two independent races, *both in the test's observation and
neither in the core*: one test asserted on the runtime tree the instant the
core's socket appeared, which is a median 8.4 ms before the last of three realms
is forked; the other pinned a death cause the core's own module documentation
calls nondeterministic in as many words. Measured on this box, that module run
back-to-back as its own process: **19 red out of 60 before, 0 red out of 100
after**. Each fix was reverted separately to confirm it was load-bearing — the
first brought the failure back at 15/60, the second at 8/100 — and the second
race's member ran 100 more times green with its fix restored.

What that does **not** license is reading a green suite as a constant. Since the
fix the whole suite has run **13 consecutive times green on this one box**, each
reporting `Ran 118 tests`, `OK`, 0 skips. Thirteen runs on one machine is
thirteen runs on one machine: it is enough to say the *known* flake is gone —
the pre-fix rate would have reddened roughly four of them — and it is not enough
to say the suite has no others. **The three-gate claim
above does not rest on the whole-suite line either way:** each gate was also run
individually at the shipped default, and the per-gate lines below are what
actually carry the column.

Each gate's own line: `test_real_gtk.py` captured a 640×480 frame
(196 distinct colour values in that run, 192 in a separate one the same day —
the count is not a fixture and is quoted only to show a real frame arrived),
`test_real_actuation.py`'s typing rung received `héllo→世界` intact with 4324
pixels changed, and `test_real_firefox.py` painted `#0000ff` over 78% of a
1024×768 frame. The `--landlock=off` column that used to sit beside these has
been **deleted rather than carried**: it was measured on 2026-08-14, it was
never re-run, and with the left-hand column no longer failing it compared
nothing.

**The mechanism, read from a realm's own log rather than inferred.** Run
`bwrap` itself as a realm's app (`command = "/usr/bin/bwrap"`, `args =
["--unshare-all", "--die-with-parent", "--ro-bind", "/usr", "/usr", "--dev",
"/dev", "--tmpfs", "/tmp", "--", "/usr/bin/true"]`) and read
`<runtime>/vitrin-0/realm-0/realm.log`. Both halves were measured on this box
on 2026-08-15, one code change apart and nothing else:

```text
before K9b:  bwrap: Failed to make / slave: Operation not permitted
after  K9b:  bwrap: Creating new namespace failed: nesting depth or
             /proc/sys/user/max_*_namespaces exceeded (ENOSPC)
```

`Creating new namespace failed` is on `glycin`'s known-string list (read out of
`strings /usr/lib/libglycin-2.so.0`: `Creating new namespace failed`, `No
permissions to create a new namespace`, `bwrap: setting up uid map: Permission
denied`, …); `Failed to make / slave` is not. That single string is the whole
difference between `bwrap sandboxing available: true` — followed by a loader
that dies sandboxed, and a GTK 3.24 `gtkiconhelper.c:495` `g_error` that turns
a failed icon load into `SIGABRT` — and `bwrap syscalls not available: STDERR
contains known string` → `WARNING: Glycin running without sandbox.`

**What was NOT the cause, each measured and each worth keeping so nobody spends
the day again.** The enumerated read set is not missing a grant: a domain
handling the whole rung-9 mask and granting *every one of those rights on `/`*
— strictly more than the enumeration, and more than the realm-root grant #187
declined — failed the identical decode. `gdk-pixbuf`'s loader cache and the
mime database are not corrupted or shadowed: inside a confined realm at the
shipped default, `sha256sum` matches the host byte-for-byte for
`loaders.cache`, `mime.cache`, `image-missing.svg` and the loader binary, and
the loader binary executes — GTK's "pixbuf loaders or the mime database could
not be found" is a generic message and a red herring.
`MOZ_DISABLE_CONTENT_SANDBOX=1` does not help Firefox: it died at the same GTK
abort while drawing its own chrome, before any content process was spawned.

**What is still true, and is what this entry publishes.** A realm's app cannot
build a sandbox inside the realm — `mount(2)` is denied to any process in a
Landlock domain and no rule reaches it — so on a host whose image decoding is a
nested-sandbox spawn, **that decoding happens with no sandbox around it**.
`glycin` prints `WARNING: Glycin running without sandbox.` for a reason, and
trading a nested sandbox away is a real loss, even though it is the loss every
host without `bwrap` already takes. An image decoder is a large attack surface
fed untrusted bytes; inside a realm it is contained by the realm's own
boundary and by nothing else.

**Which hosts this bites is a property of the host's image decoders, and one
point on each side was measured.** On this box `gdk-pixbuf 2.44.7` carries a
single in-process loader (`io-wmf.so`) with `glycin 2.1.5` and `bwrap` both
installed, so every other decode is a nested-sandbox spawn. In an
`ubuntu:24.04` container holding exactly what `shim/ci/install-deps.sh`
installs, `gdk-pixbuf 2.42.10` ships `libpixbufloader-svg.so` **in process**,
carries no `libglycin` at all, and has no `bwrap` on `PATH` — so the
nested-sandbox path is not reachable there. **What was not measured is any gate
on that image.** Two decoder inventories were measured; no gate was run inside
a container, and nothing here says these three gates pass on CI.

**Nothing routes around this.** No gate is skipped for it, no app is exempted
from the ruleset, and the ruleset was not widened to the realm root — per the
measurement above, that would not have worked either. The one thing that
changed is *when* a nested sandbox is refused, and that change is applied to
every realm at the shipped default rather than to the apps whose gates were
red. `VITRIN_LANDLOCK=off bash tests/integration/run.sh` still exists as the
no-ruleset control; that run announces itself as a control, is not the shipped
default, and is evidence for no milestone.

Three things survive the namespaces, and each is published rather than left
to be found:

- **The realm keeps your supplementary groups.** `video`, `render`, `input`,
  `docker` — whatever the invoking user has. This is not an oversight: an
  unprivileged `setgroups(0, NULL)` and an unprivileged single-id `gid_map`
  require *disjoint* windows, so there is no moment at which the groups can be
  dropped. Measured, both orderings return `EPERM`. The mount table is
  therefore the only thing standing between a realm and any device those
  groups would open, and each spawn journals the count as
  `supplementary_groups_retained`.
- **The GPU render node is bound read-write.** Never `card*` or `controlD*`,
  but the render node's ioctl surface and the cross-realm GPU-memory side
  channels it carries are real and unaddressed. Read-only was tried and
  rejected: it disables the node rather than restricting it, which would
  silently break every accelerated app in every realm. P2.6.3's Landlock rung 5
  does not change this, for the same reason in a different mechanism: its
  `IOCTL_DEV` right is one bit per hierarchy, not a per-command filter, so the
  ruleset grants it on the node.
- **`--isolation=off` is exactly the old, unconfined path**, and it exists so
  the confinement gates can run their positive controls. It has to be named
  on the command line; nothing selects it implicitly, and a session running
  that way says so on the panel.

Environment hygiene confines the well-behaved; it does not contain the
hostile. **Do not run untrusted applications, or untrusted agents, against
this yet** — a realm that can issue any syscall it likes is not a boundary you
should stake anything on, whatever its filesystem view and whatever its
Landlock ruleset denies.

<!-- limit: host-must-permit-unprivileged-userns -->
**And on some hosts you do not get that far: `vitrind --isolation=default`
refuses to start unless the host lets an unprivileged user namespace actually
carry its capabilities.** Everything above is built out of an unprivileged
`CLONE_NEWUSER` and a mount namespace inside it. A host can permit the
`unshare` and still strip the capabilities that new namespace is supposed to
confer — which the startup preflight now finds out by trying it, because
creating the namespace and mounting inside it are two different answers on such
a host. Where the probe's `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)`
fails, `vitrind` **stops**, before a realm is ever spawned. It does not quietly
start a weaker session: silent degradation is the one outcome D-020(6) forbids,
so a machine that cannot confine is told so up front, with the knobs the core
actually read named in the refusal.

Stated as a requirement on the host rather than as a list of distributions
that fail:

> Creating an unprivileged user namespace must succeed, **and** a
> `MS_REC|MS_PRIVATE` remount of `/` inside it must succeed.
> `vitrind --print-isolation` answers both, for the machine in front of you,
> without spawning anything.

**The evidence behind that sentence is one data point, and it is worth exactly
one.** On a GitHub `ubuntu-latest` runner — kernel `6.17.0-1020-azure`,
measured 2026-08-14 — `kernel.apparmor_restrict_unprivileged_userns` is `1`,
read from the runner's own sysctl before CI changed anything, so it is the
value that stock image ships. (Calling it *the distribution's* default is one
step further than this reaches — the runner's `/etc/os-release` was never
opened, and one image is not a distribution.) AppArmor permits the unshare and
then
confines the process to a profile denying the capabilities the new user
namespace should have conferred, so the first mount answers `EACCES`. The
matrix reads `ns.all=available`,
`mount.in_userns=restricted-by-policy(errno=13)`, `tier=none`, and no realm
starts. That is **one CI image, on one kernel, on one date — not a
distribution survey**. Nobody here has run one. Do not read this as "one
distribution is broken and the rest are fine"; read it as "one mainstream
default was measured and it was the unhappy answer". Collecting the matrix
across kernels is [#281](https://github.com/vitrin-os/vitrin-os/issues/281).

**What is missing is packaging, not a fix to the refusal.** The refusal is the
behaviour this project wants, and its message already tells an operator where
to look. What nothing published said, until this entry, was that a host may
need to be granted something *at all* before the default isolation will run —
so an operator met a stop rather than a prerequisite. Saying it, and shipping a
profile that makes the grant, was
[#286](https://github.com/vitrin-os/vitrin-os/issues/286), which is closed.
Making it *routine* — having an installation of this project put the profile
and the binaries where the profile expects them — is
[#293](https://github.com/vitrin-os/vitrin-os/issues/293), and until that lands
nothing here installs anything: a build outside
`/usr/lib/vitrin/` is not attached to the profile at all.
`--isolation=off` is **not** that arrangement: it starts an unconfined session,
and every confinement claim on this page stops applying to it.

**There is an AppArmor profile in the tree, and as of 2026-08-15 it has been
loaded and measured — on one kernel, on one CI image.**
`packaging/apparmor/vitrind` is the per-binary grant Ubuntu ships a mechanism
for — the same shape the `chrome`, `firefox` and `flatpak` profiles in Ubuntu
24.04's own `apparmor` package already use, chosen over telling operators to
weaken a system-wide default. It was *written* on a machine with AppArmor
**compiled out** (`/sys/module/apparmor/parameters/enabled` reads `N`), so for
its first day here it had not been parsed, loaded, attached or observed to
grant anything, and this page said exactly that. The `apparmor-profile` job now
reports otherwise, and the numbers are below rather than a paraphrase of them.

Two different kinds of claim are in play here and this page keeps them apart,
because an earlier draft did not. The profile's **behaviour** is now measured —
that is the table below. The profile's **form** is cited: every structural
choice in it is copied from a profile Ubuntu actually ships, and the file's own
header carries a provenance block naming the URL each one was fetched from and
the date. An earlier draft named `bubblewrap` in that list from memory; the
`bwrap-userns-restrict` profile is not in 24.04's `apparmor` package at all,
and the claim is gone rather than softened. If you are checking this page
against reality, check the header's URLs — that is what they are there for.

The instrument is the `apparmor-profile` job in `.github/workflows/ci.yml`. It
runs on a `ubuntu-latest` runner it does **not** modify — the only job in that
workflow that never touches `kernel.apparmor_restrict_unprivileged_userns` —
and it **fails rather than skips** if that knob is not `1` when it starts, so
it cannot quietly measure nothing. It re-reads the knob after setup and fails
if it moved, because installing the `apparmor` package would load the distro's
own profiles and grant what this profile is meant to grant
(`parser_present=stock`: the parser is already on the image, so no install
happens). It installs the profile, loads it, spawns a real realm, runs the
real-app confinement gate, then **removes the profile and requires the spawn to
fail again**.

What it reported on kernel `6.17.0-1022-azure` with
`apparmor_restrict_unprivileged_userns=1` and no sysctl touched:

| | baseline | with the profile |
|---|---|---|
| `apparmor.label` | `unconfined` | `vitrind (unconfined)` |
| `mount.in_userns` | `restricted-by-policy(errno=13)` | `available` |
| `tier` | `none` | `per-uid` |
| realm spawn | `refused-as-expected` | `ok` |

with `realapp=pass` over 8 executed confinement assertions, and the lever in
the same run: `lever_without=refused`, `lever_restored=ok`. That lever is what
distinguishes this profile working from Ubuntu's own fallback
`unprivileged_userns` profile, which carries `audit deny capability,` beside
`allow userns,` and therefore fails with the *identical* `EACCES=13` signature
— a job that only asked "did it spawn?" could not tell a wrong profile from no
profile.

**Read the boundary as narrowly as it is written: one kernel, one image, one
distribution.** Nobody has loaded this profile on an installed Ubuntu system,
and this repository has never measured a second AppArmor-carrying
distribution.

One question decided whether the profile was worth anything, and the job was
built around it. `vitrind` does not create the user namespace itself — it
`execve`s `vitrin-realm-init`, which does. If an AppArmor grant did not survive
that exec, the profile would fix the core's startup and **not** the realm's
spawn, which is worse than shipping nothing because the refusal moves somewhere
less legible. The profile is written to make that question moot — one
attachment glob over `/usr/lib/vitrin/{vitrind,vitrin-realm-init}`, so the exec
is same-label and performs no transition at all, rather than betting on
fallback semantics that `PR_SET_NO_NEW_PRIVS` restricts. **A realm spawning
under the profile is the measurement that answers it.** The shim and the app
deliberately get nothing further: `vitrin-realm-init` writes
`max_user_namespaces=0` inside the realm (K9b), so a nested user namespace is
refused by design.

**And the profile has a security cost, which is published here rather than
buried in the file — but it is conditional, and an earlier draft of this page
stated the condition backwards.** A profile of this shape —
`flags=(unconfined)` carrying a `userns` rule — is a name any local user may
try to borrow: `aa-exec -p vitrind -- <anything>` asks to run an arbitrary
program under a profile that grants a user namespace and restricts nothing
else. Installing this file adds one entry to the set of names that can be asked
for. It is the same cost Ubuntu already accepted for `chrome`, `firefox` and
`flatpak`, which is company rather than a justification.

Whether the ask *succeeds* depends on a second knob,
`kernel.apparmor_restrict_unprivileged_unconfined`, and **it has now been
measured: `0`.** Recorded by the `apparmor profile` CI job as
`RESULT unconfined_knob=0` on a stock `ubuntu-latest` (kernel
`6.17.0-1022-azure`, 2026-08-15), on the same machine and in the same run that
`apparmor_restrict_unprivileged_userns` read `1`.

**So the cost is real and unmitigated.** At `0`, `aa-exec -p vitrind` borrows
this profile's name and the borrower is genuinely unconfined — any local user
can obtain an unprivileged user namespace by naming a profile they do not own.
That is the price of installing this file, and it does not depend on vitrin
being installed or running.

This page asserted that knob twice before measuring it, wrongly in both
directions — first `0` for the wrong reason, then `1` on the strength of the
AppArmor project's [userns-restriction wiki page][aa-userns] describing what
upstream intends `/usr/lib/sysctl.d/10-apparmor.conf` to contain, which is not
the same as reading what Ubuntu ships. The measurement happens to agree with
the first guess. It was still a guess, and the second correction was confidently
wrong, which is why the job now records this knob on every run rather than
leaving it to prose.

Had it read `1`, the [unconfined-restriction page][aa-unconf] describes
`change_profile` — what `aa-exec -p` performs — as stacking rather than
transitioning, so the borrow would shed nothing. That is the branch this page
does **not** get to claim, on this runner.

So: the cost is real where an operator has set that knob to `0`, and is
mitigated by the stacking behaviour where Ubuntu's shipped `1` is in force.
Neither half has been measured by this project — the correction above is a
citation, not an experiment — which is why the `apparmor-profile` CI job
records the knob's value on its runner and refuses to report a verdict without
it. `vitrind --print-isolation` reports the same knob as
`policy.apparmor_restrict_unprivileged_unconfined`, so you can read your own
machine's answer, and its own AppArmor label as `apparmor.label` — the row that
tells "no profile attached" apart from "a profile attached and granted
nothing", which are otherwise the same errno.

[aa-userns]: https://gitlab.com/apparmor/apparmor/-/wikis/unprivileged_userns_restriction
[aa-unconf]: https://gitlab.com/apparmor/apparmor/-/wikis/unprivileged_unconfined_restriction

**This is not the only host requirement, and the two are easy to confuse.** The
entry immediately below is a second one — the kernel must actually have
Landlock — which stops the same command with the same shape of message and has
a completely different remedy. Check which mechanism the refusal names before
following anything here: this entry is the one that says `namespaces`.

<!-- limit: host-must-have-landlock -->
**And there is a second host requirement, added by P2.6.3 and just as capable
of stopping a session before any realm exists: the kernel must actually have
Landlock.** Since #187 the Landlock ruleset is part of this build's confinement
*floor*, not an optimisation on top of it — so a kernel that answers the ABI
query with `ENOSYS` no longer starts a weaker session, it refuses to start at
all. That is the same D-020(6) trade as above, made deliberately: the
alternative is a session whose realms are confined one mechanism less than its
own journal claims.

Stated as a requirement on the host, in the order an operator should check it:

> 1. **Kernel ≥ 5.13**, which is where Landlock arrived. `uname -r`.
> 2. **`CONFIG_SECURITY_LANDLOCK=y`** in the running kernel's config.
>    `zgrep CONFIG_SECURITY_LANDLOCK /proc/config.gz`, or the matching file
>    under `/boot`.
> 3. **`landlock` present in the active LSM list** — the kernel can carry the
>    code and still not enable it. `cat /sys/kernel/security/lsm`; if it is
>    absent, add `landlock` to the `lsm=` boot parameter, **keeping every name
>    already there**.
> 4. **The reported ABI must be at or above this build's floor**, which is
>    `build.landlock_min_abi` from `vitrind --print-floor` — **6** in this
>    build. This is a *build* requirement rather than a kernel-configuration
>    one, and it is the only one of the four that a correctly configured,
>    working Landlock can still fail. [The kernel
>    page](isolation-kernels.md) lists five measured kernels and which side of
>    this line each falls on.
>
> `vitrind --print-isolation` answers (1)–(3) for the machine in front of you,
> as `landlock.abi=N`, without spawning anything; hold that number against
> `--print-floor`'s for (4).

**Requirement (4) is an owner's decision (2026-08-15, re-tuned 2026-08-16), and
its remedy is different from the other three.** Nothing is misconfigured on such
a machine — Landlock is present, enabled and answering — so no sysctl, LSM list
or boot parameter moves the number and the refusal says so rather than handing
the operator the three checks above. The remedy is a newer kernel. The refusal
carries both numbers, as `below-floor(abi=N,required=M)`. The reasoning is in the
ladder bullet above; the short form is that 6 is the *lowest* rung at which this
build's enforced domain is unchanged, so the floor sits at the point where
refusing fewer machines costs no confinement.

Two things this entry does **not** say. It does not say which distributions
ship (3) unset — nobody here has surveyed that, and
[#281](https://github.com/vitrin-os/vitrin-os/issues/281) owns it alongside the
namespace survey. (Which kernels fall below (4) **is** now measured, on five of
them; see [the kernel page](isolation-kernels.md).) And `--landlock=off` is not
the remedy for a
kernel that could be configured: it starts realms with **no ruleset at all**,
so every sentence on this page about the enumerated read set, the write set and
the rung ladder stops applying to that session. It exists for a machine that
genuinely cannot have Landlock, and for the control runs this page's own
measurements are taken against.

**These are two requirements, not one, and their remedies must not be
crossed.** Both stop the same command with the same shape of message, so the
first thing to read is *which mechanism the refusal names*: `namespaces` is the
entry above, `landlock` is this one. `vitrind` walks its confinement floor in
order and refuses on the first mechanism whose probe failed, naming that one —
so the word in the message is the diagnosis, not a heading.

| the refusal names | what the host is missing | what fixes it | what does **nothing** |
|---|---|---|---|
| `namespaces` | an unprivileged user namespace that carries its capabilities | the sysctl / policy the refusal quotes (`kernel.apparmor_restrict_unprivileged_userns`, `user.max_user_namespaces`, …) | adding `landlock` to `lsm=`; rebuilding the kernel |
| `landlock` | Landlock: too old a kernel, `CONFIG_SECURITY_LANDLOCK=n`, or `landlock` absent from `lsm=` | the three checks above, two of which need a reboot | any userns sysctl; `apparmor_restrict_unprivileged_userns=0` |
| `landlock`, as `below-floor(abi=N,required=M)` | nothing — Landlock works; the kernel is older than this build's declared ABI floor | a newer kernel | all of the above, including the three checks: they are already satisfied |

**The two conditions are independent, and the one machine measured here shows
it.** The namespace refusal was measured on a kernel `6.17.0-1020-azure`
runner — four years past the 5.13 where Landlock arrived — so that machine
failed the first requirement while being nowhere near failing the second. That
same runner answered `landlock.abi=7`, which clears requirement (4) with a rung
to spare.

**That runner reading is still a transient observation, and it is worth being
precise about what has and has not changed.** It was printed by CI's own `What
confinement this runner actually grants (diagnostic, never fails)` step —
`--print-isolation` on an unmodified runner — and read out of the job log for run
[31776579437](https://github.com/vitrin-os/vitrin-os/actions/runs/31776579437),
integration job, 2026-08-14. **No file in this repository records that
runner's own output.** It is not archived as a CI artefact, is not asserted by
any test, and GitHub expires job logs, so the *distribution* half of it — the
policy rows, the `tier`, the `mount.in_userns` refusal — survives only as long
as that log does. What [#281](https://github.com/vitrin-os/vitrin-os/issues/281)
**did** close, on 2026-08-16, is the kernel half: the same kernel release
(`6.17.0-1020-azure`) is now booted under QEMU with the shipped binary and its
answer is a checked-in artefact reporting `landlock.abi=7`. That corroborates
the ABI without turning it into a fact about the runner, because the same boot
reads `apparmor_restrict_unprivileged_userns=0` where the runner reads `1`. See
[the kernel page](isolation-kernels.md), which states that distinction as the
reason it is a kernel page and not a distribution page.

For the distribution people will ask about: Ubuntu 24.04's own GA kernel is the
6.8 series, and **this is now measured rather than inferred** —
`6.8.0-139-generic`, booted with the shipped binary, reports `landlock.abi=4`,
which is below requirement (4) and is refused with
`below-floor(abi=4,required=6)`. An earlier version of this page reached the
same number by arithmetic over mainline release notes and labelled it as
arithmetic; it is a row now. What that row still does not settle is
requirements (2) and (3) on an arbitrary 24.04 install, or anything about that
distribution's *userspace* — see the next entry, and the kernel page's section
on why these are kernel rows. Note also that `ubuntu-latest` carries an Azure
kernel nine releases newer than 24.04's own, so the runner above says nothing
about a stock 24.04 in either direction.

**One interaction that is easy to miss, because it spans two refusals.** PR #290
shipped an AppArmor profile aimed at the *namespace* requirement on Ubuntu
24.04 (issue #286). That profile is measured, on the Azure kernel the runner
carries — see the entry above for what it reported. This paragraph holds
either way, and that was deliberate when it was written: the point is what the
kernel rows add regardless. 24.04's GA kernel is the ABI-4 row above, so on a
**stock**
24.04 the Landlock floor refuses the session at the next gate even if the
profile grants everything it is meant to grant. The two remedies are disjoint —
no AppArmor policy changes the number a kernel reports for its Landlock ABI — so
a working profile there would change which refusal you get, not whether you get
one, and the remedy for the second is a newer kernel. Only a 24.04 running a
newer HWE or cloud kernel — `6.17.0-1020-azure` is one — is a machine where the
profile is the only thing standing between it and a session.

**One note on this repository's own CI, because it is easy to over-read.** The
integration job takes the printed remedy and modifies the runner before it runs
a single confinement gate. Everything after that step is evidence about a
machine that was granted what it needed, never about a default install — the
measurement above is read from the diagnostic step *before* it, and the
ordering in `.github/workflows/ci.yml` is deliberate for exactly that reason.

**And when that sandboxing does arrive, it will not close this next gap, so
the gap is published before the feature rather than after it.** Host-level
sidecars sit outside every realm and therefore outside every realm's
confinement — the VLM parser and the egress proxy are ordinary host processes
with principal identities, deliberately not realm members, because the
parser's memory-unsafety must stay irrelevant to the TCB and the proxy must
hold a listener inside a realm's network namespace without being confined by
it. The consequence, stated rather than left to be discovered:

> The VLM sidecar has unmediated host network access. E2.7's headline claim is
> therefore *"a realm with no egress grant emits zero outbound packets"* — a
> statement about the realm — and is **not** a statement that realm content
> cannot leave the machine.

It can leave, through a sidecar the realm's network namespace says nothing
about. Constraining the sidecars themselves is a decide-by-M3 item; it is not
solved by attribution metadata and must not be described as if it were. See
D-020(5) in `docs/plan/20-decision-log.md`.

**On bare metal at `--isolation=off`, a realm's app can plausibly open the real
keyboard and read every key you type — including into other realms, and
including a passphrase.** This is the sandbox gap above, pointed at the one
device the whole architecture is built to mediate. `logind` ACLs
`/dev/input/event*` to the user owning the active seat session; an unconfined
app runs as the core's **own uid** with the core's full filesystem view, so
nothing stops it from opening those nodes directly. On this project's own
target machine the maintainer is additionally a member of the `input` group,
which grants that access independently of any seat — so this is concrete
rather than theoretical.

**At `--isolation=default` this is closed, and by exactly one mechanism.** The
realm's `/dev` is built from scratch and contains six nodes — `null`, `zero`,
`full`, `random`, `urandom`, `tty` — plus render nodes. **`/dev/input` is not
among them**, and the realm cannot mount, so it cannot put it there. Note what
is *not* doing the work: the `input` **group membership survives** into the
realm (see the supplementary-groups limit above), so the app still holds the
credential that would open those nodes. It is the mount namespace alone that
denies it the path. That is a single point of failure, stated as one.

What that bypasses is not a feature but the premise: `vitrind`'s input router,
the origin tag that distinguishes a human from an agent, the per-realm routing,
the consent grab that makes a prompt unspoofable, and the lock screen are all
*downstream* of a device the app reached around. An app doing this is not
observed by the journal, is not refused `preempted`, and does not appear in any
capture.

**This entry was published ahead of the code and has now been overtaken twice,
in opposite directions.** Both corrections are recorded rather than quietly
edited, because the pair is the honest history of the hole.

First it got *worse*: the sentence "it is not reachable today — there is no
DRM/KMS backend" was true when written and stopped being true when WS-E.3.2
landed the bare-metal backend, which has since run on real hardware many times.
Under `--nested` the host compositor is still the only reader of those devices,
so the exposure was always bare metal only.

Then it got *better*: P2.6.2's mount namespace closes it at
`--isolation=default`, as described above — and the same task gave
`spawn/isolation.rs` its first real enforcement, so the module that used to
probe this and enforce nothing now refuses a session below the floor. What
remains open is `--isolation=off`, where every word of the original paragraph
still holds, and the single-mechanism caveat: the credential survives, only the
path is gone.

## Testing gaps

**The 24-hour fuzz soak has never been run**
([#156](https://github.com/vitrin-os/vitrin-os/issues/156))**.** `fuzz/` ships
two cargo-fuzz targets with a checked-in corpus that CI replays on every PR,
plus a short per-PR burst. The 24-hour clean run the plan asks for is a documented manual
procedure, not a scheduled job, and nobody has executed it end to end.

**wlcs conformance is advisory and mostly red**
([#157](https://github.com/vitrin-os/vitrin-os/issues/157))**.** The
2026-07-25 run, against wlcs 1.6.1-1:
`total=180 passed=3 failed=145 skipped=32`. The version is part of the number
and not a footnote — the same shim scores 8/49 against wlcs 1.7.0 with no shim
change in between.

That number needs its context, and the context is not an excuse. wlcs tests
a general-purpose desktop compositor. The shim deliberately serves a narrow
surface — no touch, no full `xdg-shell` policy, no decoration protocols — so
most failures are "no such global" rather than misbehaviour, and the
excluded touch tests are excluded for a structural absence rather than an
expected failure. `shim/wlcs/README.md` separates the two categories
honestly. But it is still the real number, it has **not** been re-measured
since that date, and a partial run's `failed=` count is a floor rather than
a tally. It never gates a PR and is never built by default.

**dmabuf zero-copy is proven by an env-gated test, not by CI.** The path is
implemented and wired on the nested backend. The zero-memcpy assertion needs
a real GPU (EGL + a DRM render node) and runs only under
`VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests --
--ignored dmabuf`. CI is GPU-free and exercises the shm path exclusively.

**Four `#[test]` functions in this repository run in no CI job at all, and
here they are by name.** Issue
[#288](https://github.com/vitrin-os/vitrin-os/issues/288) made this a checked
number rather than a sentence: `cargo xtask skip-scan` parses every `#[test]`
in the tree, works out which CI step compiles *and* selects it, and fails
until each one that no step runs is listed in `UNRUN_TESTS`
(`crates/xtask/src/test_census.rs`) with a reason and either a pointer to
this page or the name of a test that executes it as a child process. **The
sentence you have just read is generated from that table and matched against
this page**, so adding a fifth gap is a red build until somebody writes both
the fifth bullet *and* the word "Five" above it — the first version of this
paragraph checked the names and left the number to prose, which is the exact
shape of overclaim this page exists to refuse. Twenty-four tests were selected
by no CI run when that check first ran, and the arithmetic is meant to be
checkable: nineteen were wired into a job instead — which is what the check
pushes toward — four are the bullets below, and the twenty-fourth is the
two-process case described after them, which does execute. (Those historical
numbers are prose; nothing re-derives them.)

- `dmabuf::gpu_tests::real_gpu_dmabuf_frames_are_zero_copy_end_to_end` — needs
  a real GPU whose renderer imports XRGB8888+LINEAR dmabufs.
- `dmabuf::gpu_tests::real_gpu_probe_accepts_dmabuf_and_kills_memfd_lie` —
  needs an EGL device and a DRM render node; a GitHub runner has neither.
- `dmabuf::gpu_tests::real_gpu_oversized_dmabuf_center_crops_the_full_view` —
  the same GPU, plus the same per-driver import reality (plan risk R3).
- `screenshot::tests::measure_encode_cost_at_a_real_panel_size` — not a
  hardware gap at all: it is a *measurement*, timing a 2560×1600 PNG encode
  and printing the number. There is no assertion in it for CI to fail, and a
  shared runner's timing would not be a number anybody could act on. It is
  listed here so the count stays honest, not because a machine is missing.

What this list does **not** claim is that everything absent from it is
well-tested. It measures whether a test runs, never whether it asserts
anything — and it covers Rust `#[test]` functions only. The C shim's Meson
suites, the Python integration ladder and the SDK's pytest suite each carry
their own collection floors (`tests/integration/run.sh`,
`sdk/python/tests/conftest.py`), and those are separate machinery with
separate bounds.

One further test — `spawn::isolation::tests::probe_under_ignored_sigchld` —
is selected by no CI run either, and is deliberately **not** on the list
above, because it does execute: it is the child half of a two-process test,
re-executed by name under an ignored `SIGCHLD` by
`a_launcher_that_ignores_sigchld_still_measures`, which CI does run. The
check holds that claim to the source, so a rename that broke the chain would
be red rather than quiet.

<!-- limit: drm-has-no-ci-gate -->
**The DRM/KMS backend will never have a green gate behind it, and that is the
weakest evidence in this repository.** Every other claim on this page closes on
a named, mock-free test. This one cannot, and the reasons are structural rather
than budgetary. Eight of them, named rather than summarised:

- **No DRM device in CI.** A GitHub runner has no display controller. Nothing
  there can set a mode, commit a frame or receive a page flip.
- **No seat in CI.** No `logind` session, no `seatd`, nothing for `libseat` to
  open a card through. The backend cannot even reach the point of failing
  usefully.
- **A compile check, and its own name in CI says `COMPILE ONLY`.** This bullet
  has now been wrong in **both** directions and the page keeps both corrections,
  because a limits page that quietly acquires the right words teaches nothing
  about how it got the wrong ones. It first said, in the present tense, that a
  CI rung runs `cargo clippy … --features drm-backend` when neither the rung nor
  the feature existed. It was then corrected to *"no such rung exists and no
  such feature exists — the backend itself is unwritten (#218)"* — and **#218
  landed, so that correction is now the stale half**. What is true today:
  `.github/workflows/ci.yml` carries a job named
  `drm-compile-check (COMPILE ONLY - no display controller is touched)`, which
  installs the graphics dev stack, runs
  `cargo clippy -p vitrin-core --all-targets --features drm-backend -- -D warnings`,
  asserts that smithay's soft-failing gbm probe actually ran, and runs the
  backend's device-free unit tests. **It proves the code type-checks against the
  smithay API and nothing whatsoever about behaviour.** It sets no mode, commits
  no frame and delivers no key. A green tick in a repository whose readers are
  trained to trust green ticks is exactly how a compile check gets cited as a
  functional one, which is why the job's own name shouts the qualifier and why
  this page quotes the name rather than paraphrasing it.
- **`vkms-advisory` does not close this, and must never be read as if it did.**
  There is an advisory job that attempts `sudo modprobe vkms` and, when the
  module is available, reports what it found. **What the job actually does is
  narrower than the device's capabilities**, and the distinction matters: it
  opens the node, reads mode-setting resources, and probes GBM/EGL/GLES up to
  locking a front buffer. It deliberately never calls `drmSetMaster`, never sets
  a mode and never flips a page — so it says nothing about mode setting, atomic
  commit or the page-flip loop, whatever a vkms device is capable of in
  principle. Whether it exercises the **GBM + GLES scanout path at all** is
  *unmeasured* — vkms exposes no render node, so the GLES half would need a
  software renderer and may not import into a vkms scanout buffer. The job
  measures and publishes that answer on each run; until it has run, this
  sentence is the honest state of it. It is advisory, it never gates a PR, and
  it is never to be named without the word *advisory*.
- **One machine, one GPU, one panel, one kernel.** The evidence that this
  backend works is one person executing
  [`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md)
  on one laptop: a single Intel-driven `eDP-1` at 2560x1600, scale 1, on one
  Arch kernel and one mesa version. It says nothing about any other GPU, panel,
  kernel or mesa. The PRD names "hardware matrix" as the first item of the
  support treadmill that consumed prior alternative display servers; this closes
  none of it and must not read as if it does.

  Say the second GPU precisely, because the loose word is the misleading one.
  That laptop has a second DRM device — `/dev/dri/card2`, `nvidia`, with
  `nvidia_drm` loaded and all four of its connectors disconnected, which is how
  [`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md)'s
  hazard H1 records it. What this project has exercised is **one device node at
  a time, whichever one the seat's primary GPU resolves to**: the backend
  resolves its card through `udev::primary_gpu(&seat_name)` and opens exactly
  that one — and **that selection has now been observed twice with two different
  answers**, `card1` on 2026-08-09 and `/dev/dri/card2` on the step-13a run of
  2026-08-13, from the same machine. An earlier version of this paragraph said
  "nothing here has ever opened `card2`"; that is retracted, and it was already
  false when this page last said it, which is the more useful half of the
  correction. What opening `card2` established is narrow: that session lit a
  2560x1600 output and carried six touchpad rungs through it, so the node the
  selection chose worked. What it did **not** establish is which device sat
  behind the node — the record names the node and not the driver, so whether
  the selection took the NVIDIA GPU or took the iGPU under a renumbered node is
  **not recorded either way**, and nothing here is evidence that this backend
  drives an NVIDIA card. Hard-code neither node. Nothing about a *multi-GPU*
  path changed either: there is no PRIME path, no multi-GPU renderer and no
  buffer import between devices anywhere in this repository, so no result from
  one node generalises to two. "Untested" would imply a multi-device path exists
  that nobody exercised; none exists. **No issue tracks a hardware matrix**, and
  none should: a matrix is a support treadmill the PRD names as the thing that
  consumed prior alternative display servers, not a defect.
- **The trusted band has an automated witness, and it covers one backend — not
  the one you would daily drive.** `backend/band_witness.rs` measures the
  negative half of the band's unspoofability property: that a confined app's own
  rendering can never reach the band's rows on the human-visible frame, in
  numbers a harness can hold without ever holding the session secret. It is
  wired into `backend/headless.rs` and into nothing else. Grep the DRM backend
  for `band_witness` and there are no hits, because a witness needs a
  framebuffer a test process can read and a bare-metal session's is a scanout
  buffer behind DRM master. So the property the whole trust story rests on is
  machine-checked on the backend CI runs and **asserted, not checked, on the
  backend a human looks at**. Nothing was weakened to make that true and nothing
  restores it; the alternative would be a witness on a backend no runner can
  reach, which is not a check. [#173](https://github.com/vitrin-os/vitrin-os/issues/173) tracks the *human* half nobody has
  evidence for; **the DRM half has no issue**, because there is nothing a CI
  change could do about it.
- **The bring-up runbook has been executed twice in full, both on 2026-08-09**,
  and it carries a dated record block for each. Neither was a clean pass: three
  defects came out of the first, one of which was that the page's own first line
  of recovery did not exist. **A third, partial execution followed on
  2026-08-13**: step 13a, the touchpad-class rung, which carries its own
  "Record block — EXECUTED 2026-08-13" and whose six sub-rungs came back five
  PASS and one defect —
  [#275](https://github.com/vitrin-os/vitrin-os/issues/275), a gesture
  interrupted by a VT switch that ends `completed` where it must say
  `cancelled`. Read that as **one rung, not a third pass of the runbook**:
  steps 12a, 16 and 17 are still marked NOT YET RUN on that page, and #220's
  frame-cadence field was never captured in fps. A runbook nobody has executed
  is a plan, and the wlcs number above is this repository's standing example of
  how a manual result ages once it is taken.
- <!-- limit: lifecycle-checklist-run-once -->
  **The session-lifecycle checklist has been executed twice, on 2026-08-11 and
  2026-08-13 — plus a 2026-08-12 re-read of one rung — and neither full run was
  a clean pass.** Blanking, suspend, lid handling,
  deliberate-wedge recovery and returning from another VT are rungs `L1`–`L7`
  in [Getting out of a wedged session](recovery.md#the-hardware-checklist),
  where the dated records now live. What they establish, at the counts the rungs
  themselves ask for: `L1` 10 of 10 VT switches with a stable band colour; `L2`
  **5 of 5** suspend/resume cycles, four on 2026-08-11 and the fifth on
  2026-08-13, each returning a working panel — with **liveness proven on the
  second run only**, because the first had no keymap passed and therefore no way
  to tell an idle app from a frozen one; `L3`
  **5 of 5** lid cycles, two on 2026-08-11 of which only one ever reached sleep,
  and three more on 2026-08-13 that all did, each with the same
  typed-after-resume liveness proof — plus a fourth close reopened inside one
  second that correctly never suspended at all, which is the short-lid-close
  case a single sample could never have established; `L4`
  blank at 61.2 s with the panel returning on physical input, and `L5` no lock
  card. **`L6`'s answer was lost once and is now recovered**: the 2026-08-11
  wedge came back in ~69 s by a route that could not be reconstructed afterwards
  — not from the journal, not from either flight recorder, not from the process
  tree — and 2026-08-13 settled it, `kill -CONT` against a 163.8 s `SIGSTOP`
  wedge, recovering in the next logged millisecond, with route 1's chord found
  to have been *queued* rather than defeated. `L7` is now a measurement rather
  than an impression: **61.214 s lit, counted from the seat's return** against a
  60 s timeout on 2026-08-13. **The rungs filed four defects (#257–#260)**, one
  of them that the recovery page's own published command was wrong — and a
  **fifth, #268, came out of the same 2026-08-11 session**, from driving alacritty and nautilus
  rather than from any rung, so a reader counting defects against that date
  should count five. The generated
  [session app matrix](session-app-matrix.md) is where that fifth one is
  recorded; understating a defect count is the direction this page holds to be
  the more corrosive one. **The second run filed a sixth,
  [#277](https://github.com/vitrin-os/vitrin-os/issues/277)**, and it is #260's
  class again: `kill -TERM`, the command route 2 published, is inert against a
  `SIGSTOP`ed core, so the recovery page has now been wrong twice about its own
  central instruction — which is the reason to read it sceptically rather than a
  reason to leave the count at five. **`L4` is therefore not a
  clean pass**: [#257](https://github.com/vitrin-os/vitrin-os/issues/257),
  [#258](https://github.com/vitrin-os/vitrin-os/issues/258) and
  [#259](https://github.com/vitrin-os/vitrin-os/issues/259) — the panel blanking
  ~1.5 s after a return from another VT, a silent unblank, and neither
  transition reaching the flight recorder — all came out of that session, and
  **#257's fix has since been observed on hardware** — rung `L7` was written
  from it and run later the same day, at a 20 s timeout: the panel stayed lit on
  the return and the lock did not raise, so both symptoms are gone on the
  machine that produced them. That pass was **by eye and produced no figure**;
  the figure exists now, from the 2026-08-13 `L7` run at a 60 s timeout, and it
  is the 61.214 s above rather than anything the 20 s pass could distinguish
  from 17 s. **#258 and #259 have
  since been observed on hardware too** — a second `L4` execution on 2026-08-12
  read the log and the recorder rather than only the panel, and found the wake
  line and the `screen_blanked`/`screen_woke` pair carrying
  `outcome: flip_landed`, and 2026-08-13's `L7` run re-observed that same log
  line and recorder pair in passing. One caveat travels with it: the failed-wake
  `WARN` has still never been emitted on hardware, since no wake has failed
  there. **Still unexecuted, and named rather than implied:** the SysRq route
  (route 3) and route 4, both still careful predictions; `L7`'s second pass,
  which was attempted on 2026-08-13 and caught no absence to measure; and step
  12a's `immediate` and `idle` seat policies — only `never` has ever run on
  hardware, and `idle` is the branch that would return you *locked*. The
  advisory VKMS rung is no longer "never attempted" and is worse than that: CI
  attempts it on every pull request and it **currently covers nothing** — read
  on 2026-08-13, the module loads and no card node appears behind it, so no
  connector enumeration, no mode set and no GBM/EGL probe run at all. Two runs
  on one laptop are a report about that laptop and nothing more.

This is a recorded decision with a scheduled closure in the sense that page's
last section means: the closure is a dated human run, not a job. The alternative
— a green check proving compilation, read as proving function — is strictly
worse, and is precisely the honesty gap this page exists to prevent.

## Model gaps

**The trusted indicator is unforgeable within one VT, and not necessarily
noticed.** There is a rigorous gate proving a client cannot counterfeit the
band. There is no evidence that a human *notices* when it is wrong — that needs
user research nobody has done. The plan explicitly adjudicated unspoofability
out of M1.4's criteria for exactly this reason. Do not cite the milestone as
evidence for the human half. The second qualifier is the VT: the band says
nothing about any screen other than the one this core is driving, which is
spelled out with the VT-switch entry below.

**Several realms run; only one is visible.** A `realm.toml` may now declare
up to 16 realms, and each gets its own shim process, its own private runtime
tree, its own Wayland socket and — since the output binding landed — its own
scene, its own capture and its own seat state. What it does *not* get is its
own **output**: the core composites one output from one realm's scene, so with two realms
running only the realm the output is bound to is on screen. Which realm that
is **is now somebody's to choose**: a client holding the `layout.focus` grant
verb moves the output, and the human's own keyboard and pointer move with it —
one act, because showing a realm and typing into it must never come apart. An
**agent's** actuation does not follow the output at all — it follows the realm
its own grant names, so an agent works in a realm nobody is looking at.
Absent such a client the output binds to the first realm to attach, and moves
on one event nobody chooses: the bound realm's app exiting, after which the
output follows to the first realm still serving, and to no realm at all once
none is serving. Treat a multi-realm configuration as "several apps running,
one of them on screen".

<!-- limit: one-output -->
**And there is exactly one output, by contract — a second connected display is
refused at startup rather than half-served.** Those are the session's two
cardinalities and they do not move: up to **16 realms**, one output. The
singularity is in the contract rather than in the content, which is why sixteen
live realms do not buy a second panel: `Presenter::view_size` is one size for the
whole session and every realm's shim is configured with it once before the first
fork, `RealmScenes::bound` is a single `Option<RealmId>` that the human's seat
target and the agent cursor's coordinate space both resolve through, and the
status strip has one caption. Coming up on two panels anyway would light
whichever connector enumerated first and leave a powered display dark with **no
message and no verb in the protocol that could ever move the output to it**, so
on `--drm` the backend refuses to start and names the connectors it found.

Two consequences, neither closed. **A laptop plus an external monitor — the most
ordinary desktop arrangement there is — does not work here**, and the refusal
tells you to unplug one. And the refusal is a *startup* one: this backend
enumerates connectors once and installs no udev monitor, so a panel plugged in
mid-session is neither lit nor complained about, and unplugging the only panel
leaves the session compositing into a surface nobody sees. That gap is
deliberately unowned rather than absorbed into the seat-pause handling, because a
paused session still has its panel and is told when it gets it back, while an
unplugged one has no event promising a return — holding a consent card and a lock
for a screen that no longer exists is a different decision with a different
failure mode, and nobody has taken it. The refusal came with WS-E.3.2
([#218](https://github.com/vitrin-os/vitrin-os/issues/218)); **the hot-plug gap has no issue**, and is a numbered item in
that workstream's runbook instead.

**Layout is two requests, and the absences are deliberate.** A holder can
focus a realm and choose whether it fills the output or keeps its own size.
There is no `place`, no `resize`, no `raise` and no stacking — not requests
that refuse, but no requests at all, because a scene showing one unstacked
realm cannot honour them and a verb that silently does less than its name is
worse than one with no request. Do not plan a tiling shell against this yet.

<!-- limit: principal-cannot-draw -->
**A principal cannot draw, so nothing a client builds can be on screen.**
`vitrin_view` is capture-only and there is **no principal-facing surface
interface anywhere in the IDL** — a grant can read a realm's pixels and can put
none back. So the switcher this project ships
([`examples/shell/run_shell.py`](https://github.com/vitrin-os/vitrin-os/blob/main/examples/shell/run_shell.py))
is a line-oriented program in a host terminal, and that is not a placeholder
for a graphical one: no amount of client work reaches the output. The intended
eventual shape is the shell running **as a realm**, drawing through its own
shim like any other app while holding the layout verbs through the ordinary
grant path — which needs no new protocol, but does need that realm to reach the
core socket, and that is a confinement question nobody has answered. Until
then, anything you would call a desktop shell — a bar, a launcher, an OSD, a
window-switcher overlay — cannot exist on this display server, and the
replacements are core-owned surfaces (the trusted band, the consent card, the
attention marker, the lock screen and the status strip) that no client can add
to.

<!-- limit: no-layer-shell -->
**No client status bar is possible, and the core's `--status` strip is the
whole of the replacement.** `zwlr_layer_shell_v1` is not in the shim's global
contract, and that was measured rather than assumed: waybar connects, binds six
globals, and never maps a surface; rofi and wofi are the same class. So
`vitrind --status` draws the strip itself, in reserved rows immediately below
the trusted band, and it shows **three facts**: the focused realm's name, the
battery, and a clock. There is no tray, no notifications, no workspace
switcher, and no click targets — it is not interactive at all, because a
principal cannot receive physical input (above) and the core does not want
another core-owned gesture for a status bar (it already owns eight; they are
enumerated under `principal-has-no-hotkey` below). Four further limits belong with it:

- **The strip is unspoofable in pixels but is not self-authenticating.** It
  always wins the composite, so a confined app cannot cover it — but an app
  *can* paint a convincing fake strip one row lower. The band above it is the
  anchor, and the rule is **"trusted content is everything above the coloured
  line"**. That is strictly weaker than the band's own guarantee: the band
  proves itself, the strip only inherits position from it. This makes the
  indicator story three rules where there was one, and a human who cannot state
  the rule cannot apply it.
- <!-- limit: status-strip-overdraws-the-view -->
  **Every app loses rows while the strip is on.** The realm view is *not* inset
  — the app is not configured smaller, its top rows are overdrawn, exactly as
  the band's 8 rows already are. Issue #215 asks for the inset and it is
  unimplemented; `--status` is off by default so no session pays for a strip it
  did not ask for.
- **The clock is UTC unless you say otherwise, and there is no DST.** The core
  carries no timezone database — a `tzfile` parser and a recurring read of
  `/usr/share/zoneinfo` is authority the TCB is not taking for a cosmetic field
  — so `--status-utc-offset +09:00` states a fixed offset and the strip always
  labels the zone it is showing. A session running across a DST boundary shows
  an hour that is wrong until the operator changes the flag.
- <!-- limit: status-strip-reads-sysfs -->
  **The strip is a recurring filesystem read inside the TCB.** The battery
  comes from `/sys/class/power_supply`, re-read every 30 s, bounded to one fixed
  root, 16 directory entries and 16–32 bytes per attribute, with every failure —
  no battery, a desktop, a machine mid-suspend — collapsing to an **empty slot**
  rather than a guess. When Landlock over the core's own process lands, this
  becomes a rule the core must grant itself, i.e. this widens that future
  sandbox. **And `--backlight` widens it further, in the direction that
  matters**: the brightness keys described below are a *write* to
  `/sys/class/backlight/*/brightness`, bounded the same way (one fixed root,
  names sorted before the 16-entry cap is applied, and the cap bounds the
  auto-pick — a device you name with `--backlight-device` is matched against
  the whole directory, or the flag would silently stop working on a class with
  seventeen entries in it — 24 bytes per read, every failure a no-op), which
  makes it the first rule in that future ruleset with a write bit in it.
  Recorded here rather than left for whoever writes the ruleset
  ([#187](https://github.com/vitrin-os/vitrin-os/issues/187)) to discover. It is
  **not** a `--status` fact and does not need the strip; the two are listed
  together because they are the two sysfs *class* trees the core walks, and
  because the second is the one that turns a read-only future ruleset into a
  read-write one. They are **not** the only sysfs paths the trusted core
  touches. There are four, and the other two are single files read once rather
  than directories walked on a timer: `/sys/class/tty/tty0/active`, which the
  bare-metal backend reads to learn which VT it is on, and
  `/sys/module/apparmor/parameters/enabled`, which the spawn path reads to
  decide whether an AppArmor label means anything on this kernel — the same
  file this page already cites in the confinement section above.

<!-- limit: principal-has-no-hotkey -->
**A principal cannot receive physical input either, so no client has a
hotkey.** There is no `observe_input` verb and none is designed. The core owns
**eight** physical gestures — the dead-man chord, the attention chord, the two
clipboard chords (Ctrl-Shift-Insert and Shift-Insert), the lock chord, the
screenshot chord, and, on a `--drm` session started with `--backlight` and only
then, the two brightness keys — and owns them *precisely because* the human's
off-switch, the human's attention gesture, a cross-realm transfer, the act of
locking a screen, a picture of one's own screen and a panel you can actually
read must not depend on a client being alive and correct. (On `--drm` the core
also consumes the twelve `Ctrl-Alt-F<N>` chords, which is a different thing
again: they hand the seat to another console rather than doing anything inside
this session, so they are not on that list and are described under VT switching
below.) The count was **five** before D-041 and that was already one short of
its own list; it is written out in full here so the next entry has to change a
number as well as add a clause. A
convenience hotkey is not in that class and must not borrow that warrant, so
"Super+Tab switches windows" is not a missing feature: it would mean the core
reserving a chord on behalf of whichever client asked first, which is
window-management policy the core deliberately does not have. What follows for
a user is concrete: **every layout change starts as a line you type into a
terminal**, and the terminal has to be somewhere you can reach.

<!-- limit: no-touch-no-tablet -->
**The seat serves a pointer and a keyboard: there is no touch and no tablet.**
`wl_touch` is deliberately absent from the shim's advertised seat capabilities —
`shim/src/globals.c` says `TOUCH IS NOT YET SERVED` in those words — and a
tablet or stylus has neither a shim global nor a wire event. The absence is
deliberate rather than a smaller version of support: a class advertised with
nothing behind it is **worse** than an absent one, because a toolkit that sees
`TOUCH` stops installing its pointer fallbacks and you get an application that
responds to nothing at all.

Both are **deferrals with named reopening evidence, not refusals**, and the
difference is the whole reason they are stated this way. Touch reopens on a
touchscreen appearing in the measured device set *together with* an application
that needs it; tablet reopens on a pen or stylus in that set, its application
half already being on record. The measured machine has neither device, and that
is a measurement of one laptop rather than a property of the protocol — a wire
protocol that intends to be permanent may not foreclose a device class because
one machine lacks one.

<!-- limit: pointer-extras-unproven-on-hardware -->
**What *is* served is relative motion, pointer gestures and pointer
constraints** ([#222](https://github.com/vitrin-os/vitrin-os/issues/222)) —
**and they are landed in the tree and unproven on hardware.** No run has yet
delivered any of them to a connected application, because CI has no touchpad and
no DRM device, so what stands behind them is unit and component tests rather
than a mock-free gate.

<!-- limit: gesture-ends-wrong-way -->
**A gesture that a consent card or the lock interrupts is ended the wrong
way, and that is owed rather than argued for.** The router ends an in-flight
gesture `cancelled` on a realm switch and on a seat pause, for the stated
reason that a begin with no end leaves the losing app accumulating a gesture
forever. A consent card or the lock screen raising mid-gesture takes a
different path on purpose — the gate withholds the gesture's *updates* and
keeps delivering its end, because the router only ever delivers an end for a
begin it delivered — but what then arrives is **the device's own end**, so an
app that was previewing a pinch-zoom when a card came up is told the human
*completed* what they in fact abandoned. Nothing wedges and nothing leaks; the
app's state is simply wrong in a way the human did not choose. Owned by
[#222](https://github.com/vitrin-os/vitrin-os/issues/222).

<!-- limit: no-key-repeat-on-drm -->
**And on the daily-driver backend a held key does not repeat at all.** The shim
sets `wlr_keyboard_set_repeat_info` to a rate and delay of zero, so no
application in a realm ever runs its own repeat timer, and there is no repeat
implementation anywhere in the core — grep `crates/vitrin-core/src` for one and
there is nothing but comments about *filtering* a host's autorepeat out.
Nested, that is invisible: the host compositor repeats and the core forwards
each repeated event individually, so a held key behaves. On `--drm` there is no
host, libinput synthesizes no repeat, and holding a key therefore produces
exactly one character. The refusal to turn the shim's timer back on is a real
decision and a good one — repeat is **seat-wide**, this seat carries an agent's
actuations beside the human's, and the repeat machinery cannot see the
per-event `origin` tag, so a client-side timer would repeat an agent's held key
— but the compensating core-side repeat that decision assumes **was never
written**. Read this as an unimplemented half of D-028(5), not as a design: no
run has confirmed it at a prompt, because CI cannot, and the one bare-metal
session that drove a terminal (2026-08-11) did not test for it. It has **no
issue**, because it was found by reading the tree during this sweep rather than
by using the session.

<!-- limit: shell-crash-loses-re-aim -->
**If the shell dies, you keep the session and lose the ability to re-aim it.**
The switcher is a client (PRD §5.1, D-021(4)), so there is no core-side
fallback — that is the price of the invariant, paid rather than argued away.
Kill it and both realms keep running, their shims and apps being children of
`vitrind` rather than of the shell, and the realm it last focused **keeps
receiving your keyboard and pointer**, because the output binding is core state
and nothing revokes it when the principal that set it disappears. What you
cannot do is move it. Recovery is running the shell again, which re-petitions
from zero and raises a fresh prompt per realm — **and in a real session the
terminal you would restart it from must already be the bound realm.** If the
shell died while the output was pointed at a realm with no terminal in it,
nothing on screen can start it, and every remedy is outside Vitrin: an SSH
session from another machine, a VT switch, or restarting `vitrind`. This is a
genuine wedge; it is documented, asserted
([`tests/integration/test_shell.py`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/integration/test_shell.py)),
and not solved.

**`set_fullscreen` is a no-op whenever the output and the realm are the same
size.** The two modes differ only in whether the realm's view size tracks the
output's, so while they are equal — which is the ordinary case for a realm
spawned into an output that has not resized — switching between them changes
nothing you can see. It is honest and it is surprising; the IDL says so in as
many words.

**Captures do tell realms apart, and that is enforced rather than
incidental.** An agent's capture is of the realm its **grant names**, never
of whatever is on the output: the compositor keeps one composed frame per
realm and the chokepoint resolves the frame from the grant's realm id, on
the same line it judges that realm's liveness. A grant over a realm whose
app has died refuses `no_surface` however busy its siblings are; a grant
over a live but hidden realm returns that realm's own pixels. There was a
window — between the realm cap being raised and the output being bound —
where this was not true and a capture could carry a live sibling's pixels;
it is closed.

<!-- limit: every-realm-renders -->
**Every realm renders, whether or not you are looking at it — and whether or
not any agent is connected.** A hidden realm keeps receiving frame callbacks
paced by the output's composites, and keeps having its view composed. That is
not generosity: a Wayland client throttles on frame callbacks, so a realm that
stopped being paced would stop repainting and its capture would go stale —
which the protocol forbids outright (`no_surface` is documented as "never a
stale frame"). The second half is the one this page used to leave out: the
compositor also does **not** ask whether anything will read a composed view.
With no agent connected, no `--capture-dump` and no `--screenshot-dir`, a
realm that is painting still has its view composed and cached every round.
Gating that on whether a grant happens to exist would make what a capture
returns depend on when the grant appeared, and that trade was declined.

What the compositor does skip is a realm whose scene has not changed since the
last composite: the cached view is then already byte-for-byte what recomposing
would produce, so nothing about what any reader is served depends on it. That
saves the idle case and the sibling case — one realm painting no longer costs
its fifteen neighbours a composite each — and saves nothing at all for a
single realm whose app is busy painting. The rest of the cost is real and is
not traded away: on a laptop, up to sixteen apps compositing at the output's
rate with nobody watching fifteen of them, plus roughly
`2 x width x height x 4` bytes of core-side pixels per realm (~590 MiB
resident at sixteen realms on a 2560x1600 panel, measured).

<!-- limit: agent-cursor-visible-realm-only -->
**The agent cursor is drawn only for the visible realm.** The core paints a
small crosshair where an agent is pointing, so a human can see that an agent
is acting. It is painted into the output, which shows one realm — so an
agent actuating inside a **hidden** realm draws no sprite, and the human
loses that signal entirely for everything happening off-screen. This
reintroduces, for hidden realms, exactly the defect the sprite was added to
close, and per-realm input routing makes it more likely to bite rather than
less: agents can now actually work in hidden realms, so there is more going on
that nothing draws. The fix is a per-realm indicator in the trusted band and
it is not built.

<!-- limit: per-realm-presence-narrows-preempted -->
**A human's hand no longer stops agents in other realms — and that is a
narrowing of a blanket safety behaviour.** The core refuses an agent's
actuation `preempted` while physical human input owns the target, and "the
target" used to be the whole session: touching the keyboard suspended every
agent everywhere for half a second, whatever realm each was working in. It is
now judged **per realm**. Typing in realm A suspends agents acting on realm A
and leaves agents acting on realm B alone, which is what "several apps running
concurrently" has to mean — but if you were relying on the old breadth as a
crude session-wide "hands off while I work", you no longer have it, and no
wire event tells you so. Layout requests (`focus`, `set_fullscreen`) still
yield to a hand anywhere the human is: those move what you are looking at
rather than being delivered into a realm, so they are judged against the realm
your input is following.

**You cannot switch realms in the same half-second you typed — unless you tap
Super first.** Because layout requests yield to a hand, any physical input marks
the realm you are in as yours for 500 ms, and a `focus` or `set_fullscreen`
arriving inside that window is refused `preempted`. That is correct for the case
the rule was written for — an agent must not move the output out from under
someone mid-keystroke — but it lands hardest on the most ordinary human action
there is: type `focus editor` into a shell and press Enter, and the Enter is
itself the physical input that preempts the request the Enter just sent.

The core therefore owns a second key, **Super** (configurable to right-Super,
and to nothing else). Tapping it opens a one-second, single-use window in which
a layout request from a principal holding layout authority is not refused
`preempted`, and it sends those principals a one-bit `attention` event so they
know to send the request they had staged. The key is **consumed**: no app in any
realm ever sees it, which is also why it cannot be used as a keystroke-timing
oracle. **It delegates nothing** — everything the client does afterwards it
could already do; what you withdrew was a courtesy the core was extending to
your own typing.

What it costs you:

- <!-- limit: super-is-taken-everywhere -->
  **The core eats Super, everywhere.** A nested compositor, a VM viewer, or a
  remote-desktop client running in a realm loses that key with no pass-through
  and no way to ask for one. The only remedy is `--attention-chord rsuper`,
  which is not really a remedy.
- <!-- limit: attention-window-is-session-wide -->
  **The window is session-wide.** If two clients hold layout authority, either
  of them may consume the press — the core cannot know which one you meant, and
  choosing would be window-management policy it deliberately does not have. Your
  own switch then silently fails and the other one lands. The claim is journaled
  with the principal that took it, and the grant is revocable, and it is still a
  hole.
- **A client can ask you to press it.** A shell printing "press Super to apply"
  is doing the right thing; a malicious one printing the same string and banking
  the timing is indistinguishable from it. What bounds this is that the press
  confers no authority the client did not already hold — but a human who learns
  "press Super when the screen tells me to" has learned a habit an attacker can
  invoke.
- <!-- limit: preempted-now-depends-on-hidden-state -->
  **`preempted` on the layout verbs is now conditional on core state you cannot
  see.** An agent reading its own journal can no longer reconstruct why one
  `focus` landed and an identical one did not.
- **Other principals lose a guarantee nobody tells them they lost.** "A human
  typing means nobody moves the output" was true for 500 ms at a time and is now
  suspendable by a gesture no wire event announces to anyone but layout holders.

While the window is open the core draws a small marker just below the trusted
band — never inside it, because the band has exactly one correct appearance and
that is the whole of its value. **A focus change that happened with no marker up
was not yours.**

<!-- limit: realm-switch-releases-held-input -->
**Switching realms mid-gesture releases what you were holding, into the realm
you left.** A key or pointer button you are physically holding when the output
binding moves is released to the app you are leaving, because your actual
release will be delivered to the realm you moved to. The app cannot tell that
release from a real one — it is told you let go when you did not. The
alternative is worse (a modifier latched down forever, or a wedged pointer
grab, in an app you can no longer see), and it is the same trade the core
already makes when the nested window loses host keyboard focus; the difference
is that it now happens on every switcher keypress rather than only on alt-tab.
An **agent's** held keys are not touched — its grant still reaches that realm,
so it can release them itself.

**Resizing the nested window does not resize the apps.** `vitrind --nested`
runs inside a host compositor's window, and that window can be dragged to any
size. The core announces the view size to a realm's shim exactly once, when
the shim session starts, and never re-announces it — so an app keeps its
startup size and the composite centres it, 1:1, in whatever the window has
become: background bars around it when the window grows, a centre crop when it
shrinks. Nothing is lost and no capture is wrong (a capture is composed at the
same view size), it simply looks wrong. This is not the *per-realm* resize
WS-E.1.3 declined; it is the one output changing size with nothing told about
it. `--headless` has a fixed virtual output and cannot reach this, which is
also why no CI gate can see it — `shim/docs/nested-multi-realm.md` carries it
as a manual step.

**No realm enumeration on the wire — but do not read that as unguessable.**
A client cannot ask what realms exist; `realm-0` is the one name it can know
without being told, and `vitrin_launcher.launched` hands back the ids of realms
it started itself. What that does *not* buy is secrecy of the others: instance
ids are `<template>.<n>` with a small session-global counter, so a client that
knows or guesses a template name can guess live instance ids cheaply, and
petition admission answers differently for a realm that exists than for one
that does not. So the id space is a **naming** scheme, not a capability — the
grant is what confers authority, and knowing a name gets you a petition the
human still has to approve. Treat any design that leans on an id being secret
as broken. Multi-realm *fleet* mode — a 50-realm headless box — is Phase 3 and
is a different thing again.

**Runtime launch exists, and it is a real reduction in what the core
guarantees.** `realm_launch` is served: a principal holding it over a realm
template can make the trusted core fork a process, repeatedly, for as long as
its grant lives. Until this landed, the *only* thing that could make `vitrind`
fork was startup reading a file the operator had hardened. That property is
gone. What replaces it is weaker than "impossible" and is stated as such: a
human's approval on a card naming the template's program, an expiry,
revocation, the grant's rate ceiling, a cap of 16 live realms (refused
`capacity`), and a journal entry naming the principal and grant behind every
spawn.

**And little bounds what a launched app then does.** A launch grant is
authority to start a process confined exactly as much as the session is — at
`--isolation=off`, an *unconfined* one with the core's own uid and filesystem
view; at `--isolation=default`, one that is path-confined and **filtered
against a named list of thirteen syscalls, which is not the same as
syscall-confined**. P2.6.2, P2.6.3 and P2.6.4 each narrowed this and none
closed it: the launched process still gets the kernel's whole syscall surface
minus that list, the operator's supplementary groups and a read-write render
node, so the confinement limits above apply to it unchanged, one authority
level up.

**A launched realm cannot be closed, by anybody, ever.** This is the sharper
half of the point below and it is worth stating on its own: there is no wire
request that ends a realm, and nothing in the core reclaims one. Revoking the
launch grant does not close what it started; nor does closing the connection
that asked; nor does the dead-man switch, which revokes every *grant* and
leaves every *process* running. A realm ends when its own app exits, and not
otherwise. So one approved `realm_launch` grant, exercised 15 times before the
human revokes it, commits every remaining slot of the 16-realm cap for as long
as those fifteen apps keep running, and revocation will not get one back.

<!-- limit: realm-cap-arithmetic -->
**State the cap's arithmetic precisely, because the loose version overstates
it.** The cap counts *live* realms, not launches: `Realm::occupies_capacity`
excludes the terminal state and `capacity_used` — not `len` — is what a launch
is refused against, so when a realm's app exits its slot returns and the
session can launch again. Sixteen *simultaneously live* realms is the limit,
not sixteen launches per session. Both halves have to be published together or
each becomes a lie: **no principal and no wire request can end a realm** —
revocation, disconnect and the dead-man switch all leave the process running
([#234](https://github.com/vitrin-os/vitrin-os/issues/234)) — **and a slot
comes back only when the realm's own app exits.** So the human's remedies for a
realm they no longer want are the app's own quit path, killing the process from
a terminal, or restarting `vitrind`; the display server offers none. Revocation
bounds *future* launches and nothing else; read it that way when deciding
whether to approve one.

**Launched realms accumulate for the life of a session.** An exited realm
keeps its row so `unavailable` keeps meaning *not ever*, so a session that
launches continuously grows a table of dead names it never frees. It costs no
process, no descriptor and no pixels — a name and a spawn config — and it is
bounded only by the grant's rate ceiling and expiry, not by a count. A
long-lived session driven by an agent launching on a timer will grow that
table without limit.

<!-- limit: clipboard-is-a-bounded-channel -->
**A human can now move text between two realms, and that is a channel with a
stated bandwidth.** Copy-paste between realms exists as of WS-E.2.1: pressing
Ctrl-Shift-Insert asks the realm you are looking at for its selection and puts
it in a single core-held slot; pressing Shift-Insert in another realm offers
that slot to *that* realm's app, which you then paste into with the app's own
paste key. Two gestures, one direction each, and **no client can trigger,
force or observe either** — the core asks, and there is no message by which an
app or a shim can announce a copy.

Read the rest as a bound rather than as an absence, because that is what it is:

- `text/plain;charset=utf-8` only. No images, no rich text, no file paths.
- **60 KiB** at a time (61 440 bytes), measured against real file sizes and
  against the wire's own 64 KiB frame ceiling — a larger cap is not expressible
  without handing the trusted core a shim-controlled memory mapping.
- The slot is cleared after two minutes, when the realm its contents came from
  dies, and whenever the dead-man switch fires. Nothing tells you it was
  cleared; a gesture that finds an empty slot simply does nothing.
- **Two colluding realms can therefore move ~60 KiB per human gesture pair.**
  Qubes accepts the same bound. The honest statement is the bound, never
  "there is no channel"; the PRD's threat-model row was edited rather than left
  standing.

<!-- limit: tcb-stores-application-bytes -->
**The trusted core now stores bytes an application authored.** Nothing else in
it does — it holds client *pixels* it never interprets and typed values it
validated itself. A password copied from a manager transits `vitrind` and rests
in that slot until one of the three clearing rules fires. The cap, the
one-type allow-list, the digest-only journaling (the flight recorder records a
length and a BLAKE3 digest, never content) and the three clears bound it; none
removes it. This was decided deliberately, with that cost stated, and it is the
first time this project has made that trade.

<!-- limit: clipboard-chords-taken -->
**Two more keys are taken from every app.** Ctrl-Shift-Insert and Shift-Insert
are consumed by the core in every realm, with no pass-through and no way to ask
for one. Shift-Insert is the historical X11 primary-paste chord, so an app that
binds it loses it. `--clipboard-key` moves both to another key, which is not a
remedy so much as a different loss.

<!-- limit: lock-does-not-stop-agents -->
**The lock screen does not lock out agents, and this is the single most
surprising thing on this page.** As of WS-E.2.2 there is a lock screen, and
**three** things raise it: Ctrl-Alt-Delete, `--lock-idle SECS` of no physical
input, and — only if you asked for it — a VT switch away under
`--lock-on-seat-change immediate`, which is described with the other two seat
policies further down this page and which a session that never names that flag
can never produce. Whichever one raised it, it covers the output with a
core-drawn card and takes **every physical event** away from every realm until
you type your passphrase. What it does not do is touch a grant. An agent
holding `observe` **keeps capturing the realm across a lock**, frame for frame,
exactly as if you were sitting there; one holding `actuate_pointer` or
`actuate_text` keeps acting.

That is a decision, not a gap somebody forgot to close. Observation is
concurrent by design in the wire protocol (`vitrin_view`), so `preempted` and
`consent_held` never refuse a capture, and a lock takes away **your** input, not
an agent's authority. Three alternatives were considered and rejected: a new
refusal code (a v0 wire-semantic change, which belongs to the protocol track);
blanking the realm view so agents receive black frames (a lie by omission — the
agent is never told why it sees black); and routing the lock through the
enforcement chokepoint as a synthetic human principal (which invents a wire
principal the identity layer does not have).

The instrument for "stop everything" is unchanged and still works while locked:
**hold the dead-man chord**, which revokes every grant in the session, denies
every pending petition and clears the clipboard slot. The lock card says all of
this on the card itself, in the same words, because a human who locks a screen
and walks away should not learn it from a documentation page.

**On bare metal, that same continued observation holds across a VT switch too —
with one difference that is worse and is not softened here.** The subject here
is the agent's access, *not* the paragraph immediately above: your dead-man
chord is a **physical** gesture, and physical input is suspended for the whole
time you are on another VT, so the emergency stop that still works while locked
does **not** work while you are switched away. From another VT your only stop is
a shell and a signal. An agent holding `observe` keeps being served
its realm's capture while you are on another VT — the capture is composed from
the realm's scene, which a VT switch does not touch, so the request keeps
succeeding — and one holding `actuate_pointer` or `actuate_text` keeps acting on
the app. Keeping both is right for the reason above: the grant is the authority,
not your gaze. **But the pixels stop changing.** While the seat holds the
devices no page flip lands, so no `frame_done` is issued, so every app that
paces on it stops painting; the agent is served the same frame it had when you
switched away, with no staleness signal and no refusal. That is *not* what
happens across a lock, where the frame clock keeps running — so read the
sentence above as true of a lock and this one as true of a VT switch. The net
effect is the uncomfortable one: **across a VT switch an agent can still act and
cannot see the consequences, and neither can you.** Giving realms a software
frame cadence while the seat is away would fix the observation half; it is not
scheduled, and the human's half needs a mission-control shell (E3), which is
also not in this workstream.

One more thing that gets quietly wider while you are away: `preempted` — the
refusal that stops an agent acting where your hands are — is judged against
recent *physical* input, and physical input is suspended for the whole switch.
So the moment you leave is the moment agent actuation stops being refused
`preempted`. That is correct (you really are absent) and it means agent
authority is at its widest exactly when your view of it is at its narrowest.

<!-- limit: nested-lock-locks-a-window -->
**In nested mode the lock screen locks a window, not a session.** `vitrind`
runs as a client of your real compositor, which is above it and owns the actual
session: anyone can alt-tab away from the locked window, and the host's own
screen lock is still the thing protecting the machine. Treat the nested lock as
what it is — a privacy cover over the realms `vitrind` is showing — and not as
an authentication boundary for the seat.

<!-- limit: no-vt-switch-inhibition -->
**`vitrind` never inhibits VT switching, and on bare metal it has to
*implement* `Ctrl-Alt-F<n>` for it to work at all.** On the nested backend the
chord is the host compositor's business and outside this project's reach. On
bare metal it is `vitrind`'s, and there is no third option: **once a process
holds the display, the kernel stops handling that chord**, so a display server
that does not implement it is one you cannot leave.

An earlier release of this page said the opposite — that the chord was left
alone on purpose, because a display server that traps you on its own VT is one
you cannot leave when it wedges. **The reasoning was right and the effect was
its own opposite.** That code was run on a real panel for the first time on
2026-08-09 and the human could not leave: `Ctrl-Alt-F1` and `Ctrl-Alt-F2` did
nothing, and the session ended only because it was killed from another shell.
The words are being changed, not quietly swapped: the decision that was written
to keep the escape hatch open is what welded it shut.

So, in this release:

- **`Ctrl-Alt-F1` … `Ctrl-Alt-F12` switch virtual terminal**, exactly as they do
  under every other Linux compositor. `vitrind` never switches your VT for any
  other reason — not on a timer, not on an agent's request, not to bring you
  back. Only your own hands can move it, and no principal on the wire can,
  whatever it holds.
- **They work while the screen is locked.** That is deliberate, and it is
  argued rather than assumed: being trapped is worst in the state where you
  cannot dismiss what is in front of you. It is **never a way past the lock** —
  the lock stays up and still wants your passphrase when you come back. What
  someone standing at your locked laptop gains by pressing it is a login prompt
  on another terminal, which they could have reached before you started
  `vitrind` or by power-cycling the machine. It is strictly less than what they
  can already do: the dead-man chord revokes every grant in your session and
  fires through the lock on purpose.
- **Twelve keys are taken from every confined app on bare metal.** The chords
  are consumed in every realm and never delivered. Same as every other Linux
  compositor; stated because this project states what it takes. `f1`…`f12` are
  also no longer available to `--dead-man-chord`, `--lock-chord`,
  `--clipboard-key` or `--screenshot-chord` under `--drm`, and a command line
  that asks for one is refused at startup rather than silently rearming your
  off-switch every time you leave the terminal.
- **Know your own VT number before you start.** The startup banner logs it.
  A human who can leave and cannot come back is only half rescued.
- **If a switch fails, you will see it on the panel**, in a red band that names
  what happened and what you can still do. A log line is worth nothing to
  somebody who cannot leave the screen to read it. If that band ever appears,
  the session is trapped: record it and treat it as serious.

**The first of those five bullets is confirmed on hardware; the other four are
not.** No test in this project can take DRM master or a seat, so whether
`Ctrl-Alt-F2` really puts a tty on your panel is knowable only by running the
runbooks on the one machine that has the hardware — and it has been: 5 chorded
switches on 2026-08-09 and 10 of 10 on 2026-08-11, with the band the same colour
on every return. **Every one of those chords was pressed against a healthy
compositor**, which is a narrower claim than it sounds like: the one deliberate
wedge on record defeated the chord, because a stopped compositor cannot run the
code that switches the VT. So the chord is proven as a *feature* and unproven as
an *escape*, and [the recovery page](recovery.md) is where that distinction is
argued out. Nothing on hardware has yet exercised the chord under a lock, the
twelve consumed keys, the startup banner's VT number, or the red band. The four
`vt_switch_refused already_here` events on 2026-08-09 are not a sighting of that
band: that path returns before `raise_trapped_notice`, deliberately, because
chording the VT you are already on is not a failure. **The band that tells you
the session is trapped has never been drawn on a real panel.**

**The trusted band covers this screen and this process, and nothing else.** The
coloured strip means one thing: everything above the line on *this* display was
drawn by the `vitrind` you started, not by an app running inside it. It makes no
claim about any other virtual terminal. While you are away, `vitrind` cannot see
that screen, cannot draw on it, and cannot tell you afterwards what was on it.

What *is* checkable when you come back is the colour. It is minted once per
`vitrind` process and **never rotated** — not on a VT switch, not on resume, not
for any reason — so **the same colour means the same core, and a different
colour means the core you left is not the core you came back to.** Treat
everything on screen as untrusted until you know why it restarted.
**Photograph the band before you switch and compare side by side on return**,
rather than trusting your memory of an arbitrary colour: this page already says
nobody has evidence a human reliably notices a wrong band, and a memory test is
not a check.

<!-- limit: idle-blank-does-not-lock -->
**Your screen now goes dark on its own — and a dark screen is not a locked
session.** With `--blank-idle SECS` on bare metal, `vitrind` turns the panel off
after that long with no physical input from you. **The session behind it stays
unlocked.** Any physical input brings it back, and what comes back is your
session exactly as you left it — not a passphrase prompt.

Say the consequence rather than the feature: **anyone who walks up to your dark
laptop and touches a key is inside your session.** That is worse than what most
desktops do, where the screen blanking and the screen locking are the same
timer. Here they are deliberately not coupled — locking is `Ctrl-Alt-Delete`, or
`--lock-idle SECS`, and it is a separate thing you have to ask for. If you want
a dark screen to mean a locked screen, set `--lock-idle` to a value you are
comfortable with; nothing in the blank will do it for you, and the two timers do
not know about each other beyond sharing the answer to *"when did a human last
touch this?"*.

<!-- limit: idle-inhibit-bounded -->
Two smaller things that come with it. **Idle inhibition is served now, with
three bounds worth knowing before you rely on it** (issue
[#306](https://github.com/vitrin-os/vitrin-os/issues/306), D-042). An app that
says "don't blank, I'm playing a film" over `zwp_idle_inhibit_manager_v1` is
relayed to the core, and the core holds the blank off — but only while your
output is on *that* realm, so a video in a realm you are not looking at stops
holding anything the moment you look away; it holds off the **blank** and never
the **lock**, so a film longer than your `--lock-idle` still gets a lock screen
over it, exactly as it would with no inhibit at all; and **nobody has yet
watched a video on real hardware and confirmed the panel stayed lit.** The
core-side guard, the shim relay and the cleanup on realm death are all tested,
but a blank needs a display controller and CI has none — so the claim you can
rely on today is "the ask reaches the core", not "your film will not be
interrupted". And **`--blank-idle` is refused on `--nested`**: a `vitrind`
running inside your real compositor's window would be painting a black rectangle
and calling it a dark screen, which asserts something about a display it does
not own.

<!-- limit: media-keys-reach-an-app-that-cannot-act -->
**The volume keys still reach an app that cannot act on them. The brightness
keys now work, on one backend, behind a flag, on the internal panel only.** The
keymap fallback learned the `XF86` media and brightness rows, so none of these
keys is dropped at intake — but for the media half, what changed is *where they
stop*, not what they do: a delivered `XF86AudioRaiseVolume` lands on the focused
realm's shim seat, and a confined application cannot open a mixer, so **the
human presses volume and nothing happens**. State it that way rather than
reporting that the media keys were fixed. Volume actuation stays **deferred,
with named reopening evidence**: a shell client holding a verb for it, which
WS-E Stage 2 sketched and did not build, or an explicit owner decision. There is
no one-file sysfs equivalent for a mixer and every route to one runs through a
sound server — a bus or socket client inside the TCB, which is exactly the
dependency this core refuses for logind. **No issue tracks the volume half.**

The **brightness** half closed, and it closed *narrowly*. On `--drm` only, and
only when the session was started with `--backlight`, the core consumes
`XF86MonBrightnessUp`/`Down` and writes `/sys/class/backlight` itself, one step
of 5% of that device's `max_brightness` per press — rounded *up*, and never
smaller than one raw unit, so a ceiling of 10 moves by 1 rather than by nothing
(D-041, issue
[#303](https://github.com/vitrin-os/vitrin-os/issues/303)). Five things about
that are limits rather than features, and all five are permanent until somebody
files work against them:

- **It does nothing for an external display.** The write reaches the internal
  panel this machine exposes under `/sys/class/backlight` and nothing else, so
  the behaviour now *varies by which screen you are looking at* — which is a
  worse thing to learn than the uniform nothing it replaces.
- **It is off unless you ask, and it is off on nested and headless entirely**,
  where the flag is a startup error rather than a silent no-op.
- **The two keys stop reaching your applications.** That is a reversal of what
  the previous release shipped: a nested compositor, a VM viewer or a
  remote-desktop client inside a realm loses both keys, with no pass-through and
  no way to ask for one. The core takes them because an app that both cannot act
  on the key and can *time* the human's presses is worse than an app that never
  sees it.
- **Whether it works at all is a property of your machine, not of this
  checkout.** The write is reachable through a `video`-group membership or a
  logind/udev tag this project does not own. Every failure — no device, an
  unreadable value, a file this uid cannot open — is the key doing nothing, said
  once at startup and journalled on every press, and never a startup refusal.
- **The core now has a second way to change what your panel shows, and the
  blank state machine knows about one of them.** Blanked-but-bright and
  unblanked-at-an-illegible-brightness are both reachable, and nothing makes the
  two paths agree. The mitigation is one-sided and stated as such: this core
  will never write a brightness below 5% of the device's maximum — the
  percentage is rounded up, so that is a floor and not an approximation of one
  — because a
  black panel is indistinguishable from a blanked one — but that bounds the
  accidental case and not a buggy one.

**No agent can touch any of it.** There is no verb, no wire message and no
request: the write happens on a physically-originated key press or not at all,
so an agent has nothing to ask for and nothing to be refused. **It also is not
the blanking mechanism** — `--blank-idle` powers the panel down through the
display controller, this only dims it, and the two paths do not know about each
other.

<!-- limit: blank-does-not-stop-observation -->
**A dark screen is not evidence that nothing is watching, either — and this is
the same decision as the lock screen, not a second accident.** An agent holding
`observe` **keeps capturing the realm while your panel is off**, exactly as it
does across a lock. Read the lock-screen entry above and read this as the same
sentence: a lock, and now a blank, takes away **your** input and **your** view;
neither touches an agent's authority, because the grant is what confers it and
your gaze never did. The instrument for "stop everything" is unchanged and still
works in the dark: **hold the dead-man chord**. It fires through a blank for a
structural reason rather than a lucky one — the switch watches an input tap no
gate can suppress, so the very press that wakes your screen is also the first
press of the hold.

<!-- limit: blank-stops-the-frame-clock -->
**But it is worse than that, and the honest version is uncomfortable: a blank
stops every realm's frame clock.** With the display powered off there are no
vertical blanks, so nothing tells the compositor a frame landed, so no
application is given permission to draw the next one. Every app in the session
stops painting for as long as the screen is dark. An agent holding `observe`
therefore does not "keep seeing" — **it is served the frame from just before the
blank, over and over, indefinitely, with no signal that the picture is stale and
no refusal to tell it something is wrong.** This is the same effect this page
already describes for a VT switch, where the project's own notes call it *"worse
than a stall"*; the difference is that a VT switch is something you do
deliberately and a blank happens on a timer. So on a `--blank-idle` session,
**an agent can still act and cannot see the consequences, on a schedule, without
anybody choosing it.** The fix — giving realms a software frame cadence while
the display is off — is named and is not scheduled. If you are running agents
unattended, this is the entry to read twice, and the shortest honest advice is
that a blank timeout and unattended agent work do not currently mix.

**And `vitrind` still cannot see a panel that went dark for any other reason.**
It knows about the darkness it caused itself, and that is all: your monitor's own
power button, and the backlight controls your laptop exposes outside the display
server, remain beyond it. **And since D-041 the core is one of the things that
can dim your panel without the blank knowing** — `--backlight` writes
`/sys/class/backlight` from a path that has no idea whether a cover is up, so
blanked-but-bright and unblanked-at-a-brightness-you-cannot-read are both
reachable. The core will never write below 5% of the device's maximum (rounded
up, so the published number is the floor rather than a truncation of it), which
bounds the accidental case and not a buggy one. So a consent card can still in principle be raised —
and recorded as shown to you — while you are looking at a screen something *else*
turned off. What `vitrind` does hold back is a prompt while its own blank is up,
and a prompt while the *seat* is taken away from it, which is what happens when
you switch to another VT and is the common case by a wide margin. **An earlier
release of this page said `vitrind` "never turns your screen off and has no way
to tell that something else did".** The first half stopped being true with this
release and the second half never covered the case it is now narrowed to; the
sentence is corrected here rather than quietly replaced, because a limits page
that acquires the right words without saying how it had the wrong ones is a page
you cannot check.

**The blanking behaviour above was confirmed on hardware across three dated
sessions — 2026-08-11, 2026-08-12 and 2026-08-13 — and the confirmation is
still narrower than the section.** No test in this project can
take DRM master, a seat, an ACPI event or a backlight, so all of it is knowable
only by a human running
[the recovery runbook's checklist](recovery.md#the-hardware-checklist) on the one
machine that has the hardware, and one human did. What that first run settled:
the panel does go dark on the timeout (61.2 s against `--blank-idle 60`), it does
come back on ordinary physical input, and the wake leaves the session as you
left it with **no lock card** — idle blank and idle lock are uncoupled in fact
and not only in design. What it did **not** settle, and what the third run did:
suspend ran 4 of the 5
cycles the rung asks for and lid ran 2 of 5, of which only one suspended at all,
so 2026-08-11 left a single usable lid sample and no basis at all for a claim
about a short lid close; 2026-08-13 took both rungs to **5 of 5**, added the
typed-after-resume liveness the first run had no keymap to prove, and observed
the short-lid-close case directly — a close reopened inside one second correctly
never reached sleep. The first run also found the defects that are the honest
headline here — returning to a paused session blanks the panel in ~1.5 s (#257),
the unblank is silent so success and failure look identical (#258), and blank and
unblank leave no flight-recorder event (#259). **All three are since fixed, and
all three fixes have since been observed on this same panel** — #257 by the L7
run on 2026-08-11 and again, measured rather than eyeballed, by the 2026-08-13
L7 run that timed the panel at 61.214 s lit from the seat's return against a
60 s timeout; #258 and #259 by a second L4 execution on 2026-08-12 that
read the log and the recorder instead of only the screen. Treat this section as
confirmed to that depth and no further: the frame-clock halt, the agent's
indefinitely stale frame and the prompt-suppression rules were **not** observed
on a panel and remain claims about code.

**That check runs in one direction only, and you should know which.** A
*different* colour on return is a sound alarm: the core you left is not the core
you came back to. A *matching* colour is **not** proof that it is. Anyone who
photographed your band — the very exposure the next paragraph describes — can
reproduce it exactly, so photograph-and-compare catches a restarted or
substituted core and does not catch a patient one. It is worth doing because the
first case is the common one, not because it closes the second.

The cost of never rotating is real and is the price of the property. A colour
observed once — a camera pointed at the panel, which is newly plausible when the
panel is physically in the room — is observed for the whole session, and there
is no rotation path to reach for. Rotation was refused because it would destroy
what it appears to protect: a human who cannot tell a legitimate change from a
forgery has no check left. What still holds is that **a forged card gets no
input grab**, so a replica cannot mint a grant; the harm is deception, not
authority. And the fix for a compromised colour is ending the session — which
means leaving `vitrind`'s VT and killing the process. **The dead-man chord is
not that fix**: it revokes every grant and denies every petition, which is the
right instrument for "stop everything" and does nothing for the trust colour,
because the process and its colour keep running. Note too that the chord is a
*physical* gesture and physical input is suspended for the whole time you are on
another VT, so from there your only stop is a shell and a signal.

**By default a VT switch does not raise the lock screen, and a consent prompt
raised while you are away is not recorded as shown.** Switching away is not
treated as walking away: it costs no passphrase, because making the escape hatch
expensive to come back from would erode the reason it is open. **The idle timer
is also stopped while you are away**, and the countdown restarts when you come
back — so with `--lock-idle` a switch away does not lock the session either,
however long it lasts. **The cost of that is plain: a session you switched away
from eight hours ago is unlocked when you switch back to it.**

That is the default and it is unchanged, but it is no longer the only behaviour
available: `--lock-on-seat-change immediate|idle|never` picks one, on `--drm`
only, and `never` is what you get if you say nothing.

| Policy | What leaving does |
|---|---|
| `immediate` | The lock goes up as you leave, so coming back always costs a passphrase (or an Enter, with no `--lock-passphrase-file`). |
| `idle` | The idle countdown keeps running across the absence, so a long switch-away comes back to a locked screen and a short one does not. Needs `--lock-idle`; with no countdown there is nothing to keep running. |
| `never` | **The default, described above.** The countdown freezes for the absence and restarts when you return. |

Two things no policy changes. **A lock already up is untouched** — a VT switch is
never a way past a lock screen, under any of the three. And **none of them
suspends an agent**: a locked screen does not stop observation or actuation
(above), so `immediate` buys you a passphrase prompt and not a pause. What
`vitrind` will not do is put a consent prompt on a screen it does not own: a
petition that arrives while you are on another VT stays pending, is never
journalled as shown, and times out on the ordinary sweep, which reaches the
agent as a refusal. You will experience that as the system being obstructive.
It is the fail-closed answer, and the flight recorder carries the reason
(`petition_resolved{timed_out}` with no `consent_transition{shown}` before it).

<!-- limit: passphrase-is-not-headless -->
**Without `--lock-passphrase-file` the lock is a privacy screen, and it says
so.** Enter dismisses it, with no authentication of any kind. The passphrase
path exists (Argon2id, one digest per session, one journal entry per attempt
including the failures) and it is refused at startup with `--headless`, for a
reason worth stating plainly: **a headless core holds no keymap and has no
keyboard**. Letters and digits reach a nested core only because the host
compositor interprets the layout; with no host and no device there is nothing
to type with at all, so a passphrase would be unenterable and a session that
came up that way would be locked out rather than locked.

**That sentence used to say "the core holds no keymap", full stop, and it was
about to stop being true.** WS-E.3.1
([D-028](https://github.com/vitrin-os/vitrin-os/issues/217)) puts an xkb keymap
inside the core for the bare-metal backend — behind an off-by-default build
feature, so a nested or headless `vitrind` links no `libxkbcommon` at all and
this paragraph still describes it exactly. Two things follow that are worth
knowing before that backend exists. The keymap is a **pre-compiled file an
operator points the core at**, never a layout name: libxkbcommon's name
resolution searches `~/.config/xkb` before the system path, and a realm's app
runs as the core's own uid, so a name-resolved keymap would be an app-writable
file the trusted core parses. And the core will link 383 KB of C it does not
link today, which is a real increase in the trusted computing base, stated here
rather than in a changelog.

**Turkish, and every other layout whose letters are not Latin-1, is where this
gets sharp.** The lock passphrase reads a keysym as a codepoint, and
libxkbcommon reports `ğ ş ı İ` as *legacy* keysyms whose number is not their
codepoint — while `ö ç ü` are Latin-1 and are. The core normalises both into
one convention so all of them type, but the failure it is avoiding is worth
naming: some of your letters working and some of them silently vanishing looks
like a typo, not a bug, and it would be discovered at a lock screen.

<!-- limit: lock-chord-taken -->
**A fourth chord is now taken from every app, and it constrains the other
three.** Ctrl-Alt-Delete is consumed in every realm. It also means
`--dead-man-chord delete` is refused at startup on an otherwise default command
line: the dead-man switch detects in the router's unconditional observe tap, so
a lock chord sharing its key would arm your off-switch every single time you
locked your screen. `--lock-chord` moves it, which — as with the clipboard — is
a different loss rather than a remedy.

<!-- limit: screenshot-cannot-show-a-prompt -->
**A vitrin screenshot shows the realm, not what you saw — and it cannot show a
consent prompt.** As of WS-E.2.4 there is a screenshot key: with
`--screenshot-dir PATH`, Ctrl-PrintScreen writes one PNG of the focused realm's
view into that directory. No grant is involved at any point — a human
photographing their own screen is not an agent capability — and the core mints
the filename itself, so nothing a client controls reaches a path component.

What the file contains is the limit, and it is a deliberate one: **the realm's
view only.** No trusted band, no consent card, no trusted ring, no lock screen,
no status strip, no agent cursor. So the single most useful thing a screenshot
does — "send me a picture of that weird dialog" — is the thing this one cannot
do. The correct answer today is a phone camera, and that is worse than every
other desktop offers.

The reason is the trusted band, whose colour **is** this session's secret. The
confined realm runs as the core's own uid, so any file the core writes is a
file any app can read: a screenshot carrying the band would hand a forger the
one thing that distinguishes a genuine consent prompt from a painted replica,
permanently, on the first press of the key. Two softer designs were examined
and both are worse. Cropping the band's rows out does not close it — a genuine
consent card is framed in the same colour, in the middle of the output, so a
crop protects the secret only while no prompt is up, which is exactly when you
want the screenshot. Replacing every pixel equal to the secret hands the app an
**oracle**: an app paints a field of candidate colours, you take a screenshot,
and it reads back which of its own pixels were recoloured — a ~22-bit secret
falls in a handful of screenshots at 1080p.

Four more things belong with it:

- <!-- limit: screenshots-are-world-readable-to-realms -->
  **The screenshots are readable by every app in every realm at
  `--isolation=off`.** They are files written as your uid, and the file mode is
  `600`, which keeps them from *other users* and does nothing whatsoever about
  an unconfined app running as you. This page creates no new hole — that is D9
  — but this feature creates the files.

  **At `--isolation=default` the screenshot directory is not in the realm's
  mount table at all**, so no app in any realm can name it. The narrowing is
  real, and so is its shape: it is a *path* denial, not a permission one. The
  mode is still `600` and the app still runs as your uid, so anything that ever
  puts that directory back inside a realm — a `binds` entry in `realm.toml`,
  a future designation — hands it over in full. Do not read the confinement as
  having changed what the files are.
- <!-- limit: screenshot-chord-taken -->
  **A fifth chord is taken from every app.** Ctrl-PrintScreen is consumed in
  every realm. It is a *chord* rather than a bare PrintScreen deliberately, and
  that is the one cost this feature pays back: bare PrintScreen is still
  delivered, so an app that binds it keeps it. `--screenshot-chord` moves the
  gesture, and — as with the clipboard and the lock — that is a different loss
  rather than a remedy. It may not share a key with any of the other four
  gestures; startup refuses it if it does.
- **A wrong `--screenshot-dir` puts pictures of your screen somewhere you did
  not intend, and nothing below the core enforces the choice.** The directory
  is audited at startup (absolute, existing, a directory, not a symlink, not
  group- or world-writable) and then held open for the process's life, so no
  later rename or planted symlink can redirect a write. Until E2.6/E2.7 confine
  the core, that audit is the whole of the enforcement.
- **The screenshot key does not work through a lock or a consent prompt** —
  each of those consumes *all* physical input while it is up, so the chord never
  reaches the screenshot hook. The lock one is deliberate rather than
  incidental: a person standing at your locked machine must not be able to write
  the session behind it to a file.
- **Pressing it costs the compositor about 4 ms, and the encode no longer
  happens there at all.** It used to be about 70 ms: 71.7 ms in a release build
  to encode one 2560x1600 frame into a 12.3 MB PNG, synchronously on the
  event-loop thread — roughly seventeen dropped frames at 240 Hz, on every
  press. Since issue #240 the encode runs on a worker thread that owns the
  screenshot directory, and what the press pays on the compositor thread is one
  copy of the frame out of the capture cache: **4.2 ms** for the same
  2560x1600 frame, measured in the same release build (73.9 ms for the encode
  itself, unchanged, on the same run). That is one frame at 240 Hz rather than
  seventeen. The remaining cost is the copy, and it scales with pixel count the
  same way; the queue is bounded at two presses behind the one being encoded,
  and a press past that is refused and journalled (`encoder_busy`) rather than
  queued, because each job is a whole frame of memory.

  Both numbers are CPU measurements on the development machine, in a release
  build — **not** a measurement of a session driving a real panel. What a
  screenshot does to a bare-metal session's frame timing is knowable only from
  a run of `docs/drm-bringup.md`, which needs hardware CI does not have.
- **It DOES work during a dead-man hold, and that is deliberate.** An earlier
  version of this page said otherwise and was simply wrong: `DeadManHook::gate`
  consumes only its own chord's key and delivers every other, so Ctrl+Print
  reaches the screenshot hook mid-hold and a file is written. The behaviour is
  the intended one — the off-switch destroys *authority*, and a human
  photographing their own screen is not authority — but it means the dead-man
  chord is not a way to stop a screenshot you have already started, and a
  screenshot taken during a hold captures the session as it was before the
  revocation landed. Since the encode moved to a worker thread this is literal:
  a hold does not cancel an encode already accepted, and the end of the session
  waits for it — but only for five seconds. Past that the core stops waiting,
  the process exits over the top of the worker, and that last file may be
  truncated. The wait is bounded on purpose and in both halves (the outcome
  *and* the thread), because a screenshot directory on a mount that has stopped
  answering must not be able to make `SIGTERM` do nothing.

**Identities are static tokens.** Listed in `principals.toml`. The IDL is
shaped for SPIFFE/OIDC credentials; the machinery is not here yet.

**No semantic layer.** Agents work on pixels. The AccessKit/AT-SPI2 bridge,
versioned and diffable semantic trees, and epoch/CAS action semantics are
all Phase 2 — which is to say the token-hungry screenshot loop this project
criticises is still what an agent does against it today. The difference so
far is authorization, not efficiency.

**No portals, because a realm is advertised no session bus — and that absence is
a missing service, not a confinement.** There is no `xdg-desktop-portal` here:
nothing in the core or the shim starts one, talks to one, or advertises one. The
core injects no `DBUS_SESSION_BUS_ADDRESS` and points `XDG_RUNTIME_DIR` at the
realm's own private directory, so a well-behaved application looking for a
session bus finds nothing. What that costs a desktop user is concrete and larger
than it sounds: **no portal file chooser** (you get whatever dialog the toolkit
draws itself, which cannot reach a file the application could not already open),
**no screen sharing**, **no notifications**, and no "open this link in a
browser" — a click that would hand a URL to another application does nothing.

Read the next sentence as the whole point of this entry. **This is not a
security property, and it must never be cited as one — in either mode.** Where
a realm is confined, the confinement is the kernel's, not the missing portal: no
version of "we serve no bus" is a boundary, and the paragraph below is about
what is reachable, not about what is advertised. At `--isolation=off` there is
no sandbox at all: `/run/user/<uid>/bus` is still on the filesystem and still
connectable by any process of this uid, and the abstract-socket namespace is
shared, so a determined application connects to the host session bus with no
help from anybody. In practice an operator running Firefox allow-lists
`DBUS_SESSION_BUS_ADDRESS` in `realm.toml`, which turns the implicit hole into an
audited one — and hands that realm the **host's** bus, with whatever services the
host happens to be running on it, entirely outside anything this project
mediates. What a toolkit then does with a host portal from inside a realm is
**unmeasured**; nobody has run it. What has changed since this entry was written
is the default, and it changed the reachability half only: P2.6.2's mount
namespace removes `/run/user/<uid>/bus` as a path — the realm's `/run` holds one
entry, `vitrin` — and its network namespace removes the abstract-socket namespace
the bus also listens on, because abstract sockets are scoped to a network
namespace, so at `--isolation=default` the same allow-list line names something
that is not there. That is the half Phase-2 confinement
([#160](https://github.com/vitrin-os/vitrin-os/issues/160), E2.6/E2.7) named,
delivered by the kernel; it makes the bus unreachable and it does not make the
unserved portal a confinement. Read that closure as *derived from the mount
table rather than measured*, because that is a different claim from "the kernel
did it": **no test asserts the absence of `/run/user`**, and
`tests/integration/test_real_confinement.py` lists "that a realm cannot reach
the session bus by other means" among the things it explicitly does *not*
prove, saying in as many words that a full escape survey is not what it does.
One residual is narrower than the closure and survives it: `binds` names any
absolute path outside `/` and `/home`, so an operator who binds the host's
runtime directory into a realm puts the bus socket back inside it at
`--isolation=default`, under a key that says nothing about buses. Serving
portals *properly* — a core-mediated file chooser under a grant — is the Phase-2 powerbox's job and is a
different thing again from restoring the toolkit's. **Serving portals has no
issue and appears in no plan document**, so read this as an absence nobody has
scheduled rather than as work in a queue.

<!-- limit: no-x11 -->
**No X11 shim.** Wayland only. Per-app X11 with an embedded WM is Phase 3.
There is no X server anywhere in this stack — not in the core, not among the
globals a shim advertises, not as a process anything here ever starts — and a
realm's app is handed no `DISPLAY` at all, because `DISPLAY` and `XAUTHORITY`
are refused outright by the environment the core builds for it. So `xterm` in a
realm dies before it draws anything, and the failure this project recorded is
`Can't open display`. That is the fragment the run wrote down; the full line
`xterm` emitted was not captured, and this page does not reconstruct it.

For anyone thinking of this as a desktop, the consequence is not a footnote. On
the one machine that has been measured, `xterm`, `feh`, `xsel` and
`nvidia-settings` are X11-only, as are the X11 window manager, compositor, bar,
launcher and screen locker installed on it. None of them can run here. The
maintainer's interim is **a second session, on another virtual terminal, for
X11-only software** — so "I did not have to go back to my old compositor" is
false for that set of programs. That is a workaround he accepts, not something
this project offers or confines: the second session is another compositor with
full access to the same devices, nothing here knows it exists, and switching to
it leaves the confined world entirely. It is also one person's arrangement on
one machine, and it is not advice.

What *has* been run, with the observable each run actually checked, is
[the session app matrix](session-app-matrix.md). Read it before assuming
anything else works; it is deliberately shorter than the list of things people
expect a desktop to run.

**The protocol will break.** v0 is frozen for Phase 1, not forever.

<!-- limit: no-accessibility -->
## No accessibility of any kind

This project builds an accessibility-derived semantic tree for **agents** and
provides **none at all for humans**. Somebody was going to write that sentence
about this project eventually; it is better here, in our own words, than as an
external finding.

Concretely, and this is the whole list rather than a sample:

- **No screen reader.** Nothing here speaks, and nothing here can be spoken to.
- **No magnifier.** No zoom, no lens, no focus-follows-magnifier.
- **No on-screen keyboard.** There is no `input-method`/`text-input` support at
  all — the same absence that stops you composing text in any non-Latin script —
  so there is nothing for one to type through even if one existed.
- **No sticky keys, no slow keys, no bounce keys, and no repeat tuning** — on
  the daily-driver backend, no key repeat at all (see the entry below, which
  publishes that as its own limit rather than as an accessibility footnote).
  The input router forwards what the device reports.
- **No high-contrast signal and no reduced-motion signal.** A confined app has
  no way to ask what the human needs and no way to be told.
- **No AT-SPI2 bus is advertised to a realm** — and read that word exactly, in
  the register the portals entry above uses, because the stronger word is the
  one this project must not use about itself. There is no accessibility bridge,
  bus or client in the core, the shim, the wire protocol or the SDK; the core
  injects no `DBUS_SESSION_BUS_ADDRESS` and points `XDG_RUNTIME_DIR` at the
  realm's private directory, so a well-behaved toolkit looking for
  `org.a11y.Bus` finds nothing, and the shim's own acceptance runs *disable* the
  bridge a toolkit would otherwise start — `GTK_A11Y=none` and `NO_AT_BRIDGE=1`,
  for the stated reason *"neither exists here"*.

  **That is advertisement, not reachability, and it is a missing service rather
  than a confinement.** `crates/vitrin-core/src/spawn.rs` says *"That is
  advertisement, not reachability"* about the session bus in exactly those
  words; the missing-service framing is this page's. And `org.a11y.Bus` is
  activated *on* that bus: at `--isolation=off` `/run/user/<uid>/bus` is still
  on the filesystem, still connectable by any process of this uid, and neither
  `DBUS_SESSION_BUS_ADDRESS` nor `AT_SPI_BUS_ADDRESS` is in `RESERVED_ENV`, so
  either can be allow-listed in `realm.toml`. In practice an operator running
  Firefox allow-lists `DBUS_SESSION_BUS_ADDRESS` — which hands that realm the
  **host's** accessibility bridge along with everything else on that bus.
  At `--isolation=default` the mount and network namespaces P2.6.2 landed close
  the reachability half — the realm's `/run` holds one entry, `vitrin`, and
  abstract sockets are scoped to a network namespace — so the same allow-list
  line names a bus that is not there; that is the half
  [#160](https://github.com/vitrin-os/vitrin-os/issues/160) (E2.6/E2.7) named,
  delivered by the kernel, and it makes the bus unreachable rather than making
  the unserved bridge a confinement — with the same residual the portals entry
  names, since an operator who binds the host's runtime directory in with
  `binds` puts the socket back at `--isolation=default` too. That closure is
  *derived from the mount table rather than measured*: **no test asserts the
  absence of `/run/user`**, and the test that would *prove* it — P2.1.10's adversarial
  probe, which attempts `org.a11y.Bus` activation on every reachable bus from
  inside a realm — **does not exist yet**. It is scheduled for what it settles
  in both modes: the route is open today at `--isolation=off`, and at
  `--isolation=default` nothing has yet measured the closure from inside a
  realm. Grep the core, the shim, the wire protocol and the
  SDK for `AT-SPI` and there are no hits, and `cargo xtask limits-check` holds
  that absence; the only mentions anywhere in this repository are prose about
  the backdoor this project exists to close, and the gate that holds this
  sentence.

**The semantic tree does not make Orca work, and reading it as accessibility is
the misreading this section exists to prevent.** The AccessKit/AT-SPI2 bridge
([#175](https://github.com/vitrin-os/vitrin-os/issues/175), Phase 2) is
*derived* from accessibility technology and serves a different consumer over a
different transport under a different authority: it hands **an agent** a
versioned tree over the Vitrin wire protocol, only where a **human has approved
a grant** that names the realm. An assistive technology on this machine is a
program running as the human, expecting a D-Bus bus name it can talk to without
asking anybody's permission, for a person who is not going to answer a consent
card in order to read their own screen. Nothing in the agent path becomes the
human path by being pointed at a different reader. If Phase 2 ships in full,
Orca still does not work here.

**This is an exclusion, not a deferral, and the distinction is deliberate.**
"Deferred" implies a schedule and there is none. [PRD](https://github.com/vitrin-os/vitrin-os/blob/main/docs/PRD.md)
§5.3 places human accessibility inside the support treadmill that the horizon
phase carries — *"hardware matrix, HDR, color management, fractional scaling,
human accessibility, IME for every user"* — and that phase opens only on the
**M4 gate**, whose thresholds (an independent implementer's statement of intent,
two regular non-author contributors, grant funding signed, a published
benchmark) are **unmet**, every one of them. There is no issue tracking this,
and that is on purpose: an issue would imply somebody intends to close it, and
nobody has said so.

The reasons, stated so this does not read as indifference: there is no
assistive-technology stack in this project and building one is not weeks of
work; there is no session bus inside a realm for an existing stack to attach to
(see the portals entry above), and the sandbox that would make that absence
meaningful is itself unbuilt; and there is one maintainer. None of those is an
argument that the exclusion is acceptable. **A daily driver with no screen
reader excludes people, and the honest thing to publish is the exclusion, not a
promise.** If that reads badly, it is supposed to.

## Project gaps

**One maintainer.** Governance is a documented BDFL. Bus factor is tracked
as a first-class project risk rather than waved away; the standing
mitigations are spec-first artifacts, a design-doc-per-subsystem rule, and a
review norm against cleverness in the TCB.

**No OIN membership yet**
([#159](https://github.com/vitrin-os/vitrin-os/issues/159))**.** The project
files no patents and relies on defensive publication plus the Apache-2.0 §3
and MPL-2.0 §2.1(b) grants, which are in force today. Joining the Open Invention Network is decided and
not yet done. None of this is a freedom-to-operate opinion.

**SPDX header coverage is not machine-checked**
([#155](https://github.com/vitrin-os/vitrin-os/issues/155))**.** There is no
`reuse lint`-style CI gate, so a new file without a header will not be caught
automatically.

## What holds this page to the others, and what it does not

This page, [the README](https://github.com/vitrin-os/vitrin-os/blob/main/README.md),
[`SECURITY.md`](https://github.com/vitrin-os/vitrin-os/blob/main/SECURITY.md)
and the project site state the same gaps in four different registers, and
`cargo xtask limits-check` fails the build when they stop agreeing. How much of
that is machine-held is worth writing down, because *"there is a check"* and
*"this page is checked"* are different sentences, and only the second one is
what a reader is really asking.

**What it holds.** Every claim in its table has to appear on each surface that
carries it **and** still be true of the code — a page that overstates a gap
fails as loudly as one that hides it, and both directions have caught real
drift here. Every value with a single canonical definition — the Landlock ABI
floor, the advisory wlcs counts, the wlcs release they were measured against,
the kernel the AppArmor run was taken on, the size of the booted-kernel set —
has to appear in **every** place each surface renders it, not merely somewhere
on the page, so a surface cannot contradict itself the way this project's own
site once did. Constants duplicated between two files under a comment promising
they mirror have to still mirror. The plan documents that enumerate this
project's limits have to enumerate the same set as this page. And the tables
themselves are held to a written roll of ids, so **losing coverage is a red
build rather than a smaller number in a log nobody reads**.

**What it does not hold, listed rather than left to be inferred from a green
build:**

- **Claims about the world.** Dates, hardware, *"the runbook has been executed
  twice"*, *"the suite has only ever run on two machines"*. No program can
  check those. They are published with their date and their one machine named,
  and a human repeating the run is the only check there is.
- **Whether the wlcs numbers are still true of the shim.** The gate holds every
  surface to the same four counts and the same wlcs release; nothing re-runs
  wlcs, because the advisory job commits no artefact to compare against. That
  is [#157](https://github.com/vitrin-os/vitrin-os/issues/157).
- **A paragraph that states a held value in a register the table does not
  know.** The check finds every occurrence of the registers it is given; it
  cannot find one nobody told it about. That gap closes by adding a row, and
  the same is true of any claim on this page with no row at all — most of this
  page is argued prose, and only the named subset is machine-held.
- **A published page nobody added to the table.** Coverage is per page and per
  claim: a page the table does not name is unheld entirely, however many of
  these claims it repeats. `docs/ARCHITECTURE.md` and the Phase-2 plan document
  both restate the five-kernel figure today and neither is held; a page added
  tomorrow inherits the same gap on the day it ships.
- **Text a reader never sees.** The check reads the file's bytes, not the page a
  browser draws, so a block commented out or fenced still satisfies every
  anchor in it. The gate would report agreement across surfaces that had
  stopped publishing the claim at all — which is the understating direction,
  the one this page cares about most.
- **That this page and the issue tracker describe the same set.** They do not,
  by policy and on purpose: many gaps here are permanent decisions with no
  issue, and the README promises exactly that. What runs on every pull request
  is the narrower, offline direction — every issue a held claim names must be
  cited on one of that claim's own surfaces, so a reader who meets a gap can
  find what tracks it without leaving the page. The other direction needs the
  GitHub API and is a scheduled advisory report
  (`.github/workflows/honesty-tracker.yml`), never a gate, because a build that
  goes red when somebody else opens an issue is a build people learn to delete.

The tables, their rolls and the full argument for each of these bounds are in
`crates/xtask/src/limits.rs`, and the gate prints what it compared on every run.

## Why this page exists

From the project's own security notes: *a half-believed confinement claim is
worse than an honest gap.* Every item above is a recorded decision with a
scheduled closure, not an oversight — see
[the decision log](https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md).

If you find something true that belongs on this page and is not here, that
is a bug worth reporting, and it will be treated as one.
