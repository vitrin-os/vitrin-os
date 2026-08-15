<!-- GENERATED FILE -- do not edit by hand.
     Source: crates/xtask/src/kernel_matrix.rs, rendered from the boot rows in
     tests/kernel-matrix/rows/. Regenerate with `cargo xtask kernel-matrix`.
     `cargo xtask kernel-matrix --check` fails if this file and those rows disagree. -->

# Which kernels this build starts on, measured

Each row below is a distribution kernel — 5 of them — booted under QEMU with the
**shipped `vitrind`** in a minimal initramfs, and this page is their answers. Every cell below is a line that
binary printed on that kernel — not a lookup table of when a Landlock ABI landed in
mainline, and not a claim about the distributions those kernels come from.

The rows live in `tests/kernel-matrix/rows/`, one file per kernel, holding
`vitrind --print-isolation` and `vitrind --print-floor` verbatim plus the startup
line. This page is rendered from them; it measures nothing itself.

**This build's floor is Landlock ABI 6** — `build.landlock_min_abi`, printed by
`vitrind --print-floor`. A kernel below it is refused at startup rather than confined
at a weaker rung. What each rung of the ABI buys, and why the floor sits where it
does, is [the isolation matrix](isolation-matrix.md) — a **build**-static page that
probes no machine. Do not read the two as one document: that page says what this
build requires, this one says what these 5 kernels answered.

## The measured table

| kernel `release` | shipped by | `landlock.abi` | `ns.all` | `mount.in_userns` | `tier` | `--isolation=default` |
|---|---|---|---|---|---|---|
| `5.15.0-191-generic` | Ubuntu 22.04 LTS | 1 | `available` | `available` | `intra-user` | **refused** — `below-floor(abi=1,required=6)` |
| `6.1.0-50-amd64` | Debian 12 (bookworm) | 2 | `available` | `available` | `intra-user` | **refused** — `below-floor(abi=2,required=6)` |
| `6.8.0-139-generic` | Ubuntu 24.04 LTS (GA kernel) | 4 | `available` | `available` | `intra-user` | **refused** — `below-floor(abi=4,required=6)` |
| `6.12.101+deb13-amd64` | Debian 13 (trixie), current stable | 6 | `available` | `available` | `intra-user` | **starts** |
| `6.17.0-1020-azure` | Ubuntu (azure kernel) — what this repository's CI runners boot | 7 | `available` | `available` | `intra-user` | **starts** |

So of the five: **2 start and 3 are refused**, and the boundary is exactly the floor.

Admitted:

- `6.12.101+deb13-amd64` (Debian 13 (trixie), current stable) — `landlock.abi=6`, at or above the floor of 6.
- `6.17.0-1020-azure` (Ubuntu (azure kernel) — what this repository's CI runners boot) — `landlock.abi=7`, at or above the floor of 6.

Refused:

- `5.15.0-191-generic` (Ubuntu 22.04 LTS) — `landlock.abi=1`, below the floor of 6. The remedy is a newer
  kernel, and specifically **not** a sysctl, an `lsm=` edit or a `CONFIG_` change:
  this kernel's Landlock is present, enabled and answering.
- `6.1.0-50-amd64` (Debian 12 (bookworm)) — `landlock.abi=2`, below the floor of 6. The remedy is a newer
  kernel, and specifically **not** a sysctl, an `lsm=` edit or a `CONFIG_` change:
  this kernel's Landlock is present, enabled and answering.
- `6.8.0-139-generic` (Ubuntu 24.04 LTS (GA kernel)) — `landlock.abi=4`, below the floor of 6. The remedy is a newer
  kernel, and specifically **not** a sysctl, an `lsm=` edit or a `CONFIG_` change:
  this kernel's Landlock is present, enabled and answering.

All 5 rows report `ns.all=available` and `mount.in_userns=available`, so the
Landlock floor is the only thing separating the two groups. **4 of this
build's 9 rungs are reported by none of these kernels** — these are 5 machines
somebody might be refused on, not a sweep of the ABI ladder, and no row here says
anything about a kernel that is not in the table.

## These are KERNEL rows. They are not distribution rows

Each boot loads a distribution's unmodified `vmlinuz` and then runs a **minimal
initramfs of this repository's own making**: a static PID 1 (`tests/kernel-matrix/init.c`),
`/vitrind` and its library closure, `/proc` `/sys` `/dev` `/run` mounted, uid 0 with a
full capability set — and **no distribution userspace at all**. No AppArmor or SELinux
policy is loaded, no `/etc/subuid` exists, no sysctl file from `/etc/sysctl.d` has been
applied, no container runtime has adjusted anything.

That is a deliberate choice and it decides what the rows mean. A kernel is the same
kernel wherever it boots, so `landlock.abi` and the namespace rows are properties of
these bytes. The **policy** rows are not: they are properties of a running system, and
this one is bare.

**The cross-validation that settles it.** The last row is the same kernel release the
runners this repository's CI uses report. Booted here it agrees with that runner on
the kernel facts and disagrees with it on the policy facts. The left column is read
out of the checked-in row; the right column is transcribed from a CI job log:

| cell | this harness, bare initramfs | the CI runner, Ubuntu userspace |
|---|---|---|
| `landlock.abi` | `7` | `7` |
| `ns.all` | `available` | `available` |
| `policy.apparmor_restrict_unprivileged_userns` | `0` | `1` |
| `mount.in_userns` | `available` | `restricted-by-policy(errno=13)` |
| `tier` | `intra-user` | `none` |
| `provisioning.subuid` | `absent` — an initramfs has no `/etc/subuid` | whatever the image ships |

Same kernel, same binary, two different answers — and the ones that moved are exactly
the policy cells. So this method can produce a kernel row and can never produce a
distribution row. **A distribution row has to come from that distribution**, which for
this repository means the runner's own `--print-isolation` output, printed by the
`What confinement this runner actually grants` step in
[`.github/workflows/ci.yml`](https://github.com/vitrin-os/vitrin-os/blob/main/.github/workflows/ci.yml).
The runner column above is that step's reading, transcribed from a job log GitHub
expires; it is not an artefact in this tree, and [the limits page](limits.md)
publishes that bound.

This is also why `tests/kernel-matrix/kernels.manifest` has no policy-variant record.
Booting one of these kernels with an AppArmor sysctl flipped on the command line would
produce a row that *looks* like a distribution row while still being a kernel row with
one knob moved, which is the confusion this whole section exists to prevent.

## Ubuntu 24.04 needs the AppArmor profile *and* is refused anyway

This one is not obvious and it undercuts something this repository shipped, so it is
stated plainly rather than left to be inferred from the table.

PR #290 added an AppArmor profile for `vitrind` (`packaging/apparmor/vitrind`), because
Ubuntu 24.04 denies the capabilities `vitrind` needs *inside a user namespace it has
already granted* — issue #286. **Whether that profile works has not been measured**:
[the limits page](limits.md) records that it has never been loaded by anyone who wrote
it, and the `apparmor-profile` CI job is the instrument that will say. Nothing below
assumes it does work.

**What this page adds is that the profile cannot be sufficient on Ubuntu 24.04's own
GA kernel, whatever the job reports.** That kernel is `6.8.0-139-generic`, measured
above at `landlock.abi=4` — below this build's floor of 6. So on a stock 24.04,
even granting the profile everything it is meant to grant:

1. the profile is aimed at the **namespace** refusal, and then
2. the isolation preflight refuses the session at the next gate anyway, on the
**Landlock floor**, with `below-floor(abi=4,required=6)`.

The two refusals are independent and their remedies are disjoint — no AppArmor policy
changes the number a kernel reports for its Landlock ABI. So on a stock 24.04 a working
profile would change *which* refusal you get, not *whether* you get one; the remedy for
the second is a newer kernel. Only a 24.04 running a **newer HWE or cloud kernel** —
the `6.17.0-1020-azure` row above is one, at ABI 7 — is a machine where the profile is
the only thing standing between it and a session.

That is a real qualification on what PR #290 bought, and it belongs beside every
description of the profile rather than only here. [The limits page](limits.md) carries
it in the `host-must-have-landlock` entry.

## Why each kernel is in the set

- **`5.15.0-191-generic`** — Ubuntu 22.04 LTS (5.15-lts). The oldest kernel with Landlock at all that a reader might still be running. It is the bottom of the range: if this build refused nothing else, it would refuse this.
- **`6.1.0-50-amd64`** — Debian 12 (bookworm) (6.1-lts). The previous Debian stable, and the row that shows the floor is not satisfied by "a recent LTS" — 6.1 is a long-term kernel and it is still four rungs short.
- **`6.8.0-139-generic`** — Ubuntu 24.04 LTS (GA kernel) (6.8-lts). The kernel PR #290's AppArmor profile was written for, and the reason that profile cannot be sufficient on its own: the profile is aimed at the namespace refusal, and this kernel is refused at the next gate anyway, on the Landlock floor. The page's "AppArmor profile" section is about this row.
- **`6.12.101+deb13-amd64`** — Debian 13 (trixie), current stable (6.12-stable). The row the floor was lowered for (owner's decision, 2026-08-16). Under the previous floor of 7 this kernel was refused; it reports ABI 6, and the domain this build enforces at rung 6 is identical to the one it enforces at rung 7.
- **`6.17.0-1020-azure`** — Ubuntu (azure kernel) — what this repository's CI runners boot (ci-runner-kernel). The cross-validation row. It is the same kernel release the CI runner reports, so booting it here says whether this harness reproduces that machine — and, on the policy cells, whether it does not.

## Provenance, per row

A row is only worth as much as its ability to be re-taken. Each block below carries
the bytes that were booted, where they came from, and the command that booted them.
The **sha256 is the identity and the URL is only where those bytes were found on the
collection date**: distribution pools prune superseded kernels, so a dead URL is
expected eventually and the remedy is another mirror serving the same checksum — never
a re-measurement against whatever the pool holds now.

### `5.15.0-191-generic` — Ubuntu 22.04 LTS

| | |
|---|---|
| row | [`tests/kernel-matrix/rows/ubuntu-22.04-5.15.row`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/kernel-matrix/rows/ubuntu-22.04-5.15.row) |
| collected | 2026-08-15 (UTC) |
| `vitrind` version | 0.1.0 |
| `--print-isolation` schema | `vitrin-isolation 3` |
| package | `http://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux-image-unsigned-5.15.0-191-generic_5.15.0-191.201_amd64.deb` |
| package sha256 | `240e407cd863d86dc2eefcf3a085b232849afae7a8c911461369e8c0a02e3f67` |
| vmlinuz | `./boot/vmlinuz-5.15.0-191-generic` |
| vmlinuz sha256 | `e14e87b3c53124b655207647f695908cafc942cd28871c913ffa4aab712eba93` |
| boot | `qemu-system-x86_64 -accel <accel> -m 512 -smp 1 -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append "console=ttyS0 panic=1 quiet"` (`<accel>` was `tcg`) |
| userspace | minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row. |

The startup line this kernel produced, verbatim:

```text
ERROR vitrind: fatal: this build's isolation floor requires `landlock` and this machine reports `below-floor(abi=1,required=6)`. This kernel has Landlock and reports ABI 1; this build's floor is ABI 6 (owner's decisions of 2026-08-15 and 2026-08-16: declare a floor rather than publish a multi-rung ladder nothing measures, and set it at the lowest rung that gives up no enforcement). Nothing is misconfigured here and no sysctl, LSM list or boot parameter will change the number -- the remedy is a newer kernel. `uname -r` says 5.15.0-191-generic, and `vitrind --print-floor` prints the required number as `build.landlock_min_abi`. This build will not fall back to a lower rung: a realm confined by a weaker domain than the session's own journal names is the silent degradation D-020(6) exists to forbid. `--landlock=off` starts a session whose realms get NO ruleset at all -- it is the positive control this repository's confinement gates run against, not a way to run on an older kernel. Pass `--isolation=off` to start an UNCONFINED session anyway, or -- for a Landlock refusal specifically -- `--landlock=off` to start a session whose realms have namespaces and no ruleset. Both are weaker than what was asked for and both say so in every journal entry. `vitrind --print-isolation` shows every row behind this answer and `vitrind --print-floor` what this build requires.
```

### `6.1.0-50-amd64` — Debian 12 (bookworm)

| | |
|---|---|
| row | [`tests/kernel-matrix/rows/debian-12-6.1.row`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/kernel-matrix/rows/debian-12-6.1.row) |
| collected | 2026-08-15 (UTC) |
| `vitrind` version | 0.1.0 |
| `--print-isolation` schema | `vitrin-isolation 3` |
| package | `https://deb.debian.org/debian/pool/main/l/linux/linux-image-6.1.0-50-amd64-unsigned_6.1.176-1_amd64.deb` |
| package sha256 | `439422e41d2dbb840b60b81e3bf5e5955bfa56b7a8c268aec83c9ff882d421e6` |
| vmlinuz | `./boot/vmlinuz-6.1.0-50-amd64` |
| vmlinuz sha256 | `653421d9774c0de27502ca010d572323b52a5d7141d067b9b04214bd24baca3a` |
| boot | `qemu-system-x86_64 -accel <accel> -m 512 -smp 1 -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append "console=ttyS0 panic=1 quiet"` (`<accel>` was `tcg`) |
| userspace | minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row. |

The startup line this kernel produced, verbatim:

```text
ERROR vitrind: fatal: this build's isolation floor requires `landlock` and this machine reports `below-floor(abi=2,required=6)`. This kernel has Landlock and reports ABI 2; this build's floor is ABI 6 (owner's decisions of 2026-08-15 and 2026-08-16: declare a floor rather than publish a multi-rung ladder nothing measures, and set it at the lowest rung that gives up no enforcement). Nothing is misconfigured here and no sysctl, LSM list or boot parameter will change the number -- the remedy is a newer kernel. `uname -r` says 6.1.0-50-amd64, and `vitrind --print-floor` prints the required number as `build.landlock_min_abi`. This build will not fall back to a lower rung: a realm confined by a weaker domain than the session's own journal names is the silent degradation D-020(6) exists to forbid. `--landlock=off` starts a session whose realms get NO ruleset at all -- it is the positive control this repository's confinement gates run against, not a way to run on an older kernel. Pass `--isolation=off` to start an UNCONFINED session anyway, or -- for a Landlock refusal specifically -- `--landlock=off` to start a session whose realms have namespaces and no ruleset. Both are weaker than what was asked for and both say so in every journal entry. `vitrind --print-isolation` shows every row behind this answer and `vitrind --print-floor` what this build requires.
```

### `6.8.0-139-generic` — Ubuntu 24.04 LTS (GA kernel)

| | |
|---|---|
| row | [`tests/kernel-matrix/rows/ubuntu-24.04-6.8.row`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/kernel-matrix/rows/ubuntu-24.04-6.8.row) |
| collected | 2026-08-15 (UTC) |
| `vitrind` version | 0.1.0 |
| `--print-isolation` schema | `vitrin-isolation 3` |
| package | `http://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux-image-unsigned-6.8.0-139-generic_6.8.0-139.139_amd64.deb` |
| package sha256 | `6c5e6049c195ac7f8b4cbd2dd96dca4f8cfdd2c0627250c5688fe1ba9caf930f` |
| vmlinuz | `./boot/vmlinuz-6.8.0-139-generic` |
| vmlinuz sha256 | `b500ae87509c77cde64aa9867804e901b410efa3eea41f62d3c766f8c0ee9ab6` |
| boot | `qemu-system-x86_64 -accel <accel> -m 512 -smp 1 -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append "console=ttyS0 panic=1 quiet"` (`<accel>` was `tcg`) |
| userspace | minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row. |

The startup line this kernel produced, verbatim:

```text
ERROR vitrind: fatal: this build's isolation floor requires `landlock` and this machine reports `below-floor(abi=4,required=6)`. This kernel has Landlock and reports ABI 4; this build's floor is ABI 6 (owner's decisions of 2026-08-15 and 2026-08-16: declare a floor rather than publish a multi-rung ladder nothing measures, and set it at the lowest rung that gives up no enforcement). Nothing is misconfigured here and no sysctl, LSM list or boot parameter will change the number -- the remedy is a newer kernel. `uname -r` says 6.8.0-139-generic, and `vitrind --print-floor` prints the required number as `build.landlock_min_abi`. This build will not fall back to a lower rung: a realm confined by a weaker domain than the session's own journal names is the silent degradation D-020(6) exists to forbid. `--landlock=off` starts a session whose realms get NO ruleset at all -- it is the positive control this repository's confinement gates run against, not a way to run on an older kernel. Pass `--isolation=off` to start an UNCONFINED session anyway, or -- for a Landlock refusal specifically -- `--landlock=off` to start a session whose realms have namespaces and no ruleset. Both are weaker than what was asked for and both say so in every journal entry. `vitrind --print-isolation` shows every row behind this answer and `vitrind --print-floor` what this build requires.
```

### `6.12.101+deb13-amd64` — Debian 13 (trixie), current stable

| | |
|---|---|
| row | [`tests/kernel-matrix/rows/debian-13-6.12.row`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/kernel-matrix/rows/debian-13-6.12.row) |
| collected | 2026-08-15 (UTC) |
| `vitrind` version | 0.1.0 |
| `--print-isolation` schema | `vitrin-isolation 3` |
| package | `https://deb.debian.org/debian/pool/main/l/linux/linux-image-6.12.101+deb13-amd64-unsigned_6.12.101-1_amd64.deb` |
| package sha256 | `56ae4e2dbf5f07214ef5d07f515698f36c62fb63eee97831333f32b659afab66` |
| vmlinuz | `./boot/vmlinuz-6.12.101+deb13-amd64` |
| vmlinuz sha256 | `8f6144580ef6e34459ea0c0819919c292dc605acbd6eac074d8bfef84505a055` |
| boot | `qemu-system-x86_64 -accel <accel> -m 512 -smp 1 -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append "console=ttyS0 panic=1 quiet"` (`<accel>` was `tcg`) |
| userspace | minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row. |

The startup line this kernel produced, verbatim:

```text
INFO vitrind: realms will be confined: each gets its own user, mount, PID, IPC, UTS and network namespace, an identity uid/gid map, zero capabilities, and a Landlock ruleset enforced before the shim's execve. No `applied_profile` is printed here on purpose: it names the rung a realm OBTAINED, and no realm exists yet -- the ladder's landing is per-spawn. Whatever it says, it is not a tier name, because `intra-user` means namespaces PLUS Landlock PLUS seccomp and the seccomp filter (P2.6.4) is not applied by this build isolation=default landlock=highest kernel=6.12.101+deb13-amd64
```

### `6.17.0-1020-azure` — Ubuntu (azure kernel) — what this repository's CI runners boot

| | |
|---|---|
| row | [`tests/kernel-matrix/rows/ubuntu-azure-6.17.row`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/kernel-matrix/rows/ubuntu-azure-6.17.row) |
| collected | 2026-08-15 (UTC) |
| `vitrind` version | 0.1.0 |
| `--print-isolation` schema | `vitrin-isolation 3` |
| package | `http://archive.ubuntu.com/ubuntu/pool/main/l/linux-azure/linux-image-unsigned-6.17.0-1020-azure_6.17.0-1020.20_amd64.deb` |
| package sha256 | `0b6fc9fd94bf375fad1f83fe0672ab72152def473d1fd93c50b07b7452942faa` |
| vmlinuz | `./boot/vmlinuz-6.17.0-1020-azure` |
| vmlinuz sha256 | `04d18f600df726d196e6dffd22338657f6702f25a4b3512b400b77640d148c59` |
| boot | `qemu-system-x86_64 -accel <accel> -m 512 -smp 1 -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append "console=ttyS0 panic=1 quiet"` (`<accel>` was `tcg`) |
| userspace | minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row. |

The startup line this kernel produced, verbatim:

```text
INFO vitrind: realms will be confined: each gets its own user, mount, PID, IPC, UTS and network namespace, an identity uid/gid map, zero capabilities, and a Landlock ruleset enforced before the shim's execve. No `applied_profile` is printed here on purpose: it names the rung a realm OBTAINED, and no realm exists yet -- the ladder's landing is per-spawn. Whatever it says, it is not a tier name, because `intra-user` means namespaces PLUS Landlock PLUS seccomp and the seccomp filter (P2.6.4) is not applied by this build isolation=default landlock=highest kernel=6.17.0-1020-azure
```

## One cell is normalized, and it is named

`policy.max_user_namespaces` is derived from guest memory and tracks the size of the
compressed initramfs, so it moves whenever `vitrind`'s binary changes size — which is
most commits, including ones that touch nothing here. Publishing it raw would make
every unrelated change look like a kernel behaving differently. The rows therefore
replace that one value with a placeholder in the compared body and keep the raw
reading in the row's own header, where `--check` ignores it. It is the only cell that
gets this treatment, and every other line of every row is compared byte-for-byte.

## Runbook

**No pull request boots a kernel.** Collecting the rows needs QEMU, roughly 220 MiB of
downloaded kernel packages and about fifteen seconds of emulation, and wiring that
into per-PR CI would buy a check on something that changes when a *distribution* ships
a kernel — not when this repository changes. It is a command a person runs, and the
rows carry a collection date so a reader can see how stale the answer is.

```console
# Re-measure every kernel and rewrite tests/kernel-matrix/rows/. Needs qemu.
$ cargo build --release --bin vitrind
$ tests/kernel-matrix/collect.sh

# Re-measure and DIFF against what is checked in; writes nothing. Red if a kernel
# now answers differently, or if a row is older than 180 days.
$ tests/kernel-matrix/collect.sh --check

# One kernel only. Says loudly that it is a partial run.
$ tests/kernel-matrix/collect.sh --only debian-13-6.12

# Re-render THIS PAGE from the checked-in rows. No qemu, no network.
$ cargo xtask kernel-matrix
$ cargo xtask kernel-matrix --check   # what CI runs on every pull request
```

The two `--check`s prove different things and neither substitutes for the other:
`cargo xtask kernel-matrix --check` holds this **page** to the **rows**, and
`collect.sh --check` holds the **rows** to the **kernels**. A green pull request means
the first one passed. It says nothing about whether these kernels still answer this
way.

A failed boot never produces a row. The collector requires its init's sentinels, a
zero exit from each probe, a schema-tagged and complete reading, exactly one startup
verdict, and a `kernel.release` equal to the one the manifest pins — and it prints
`FAIL:` and exits nonzero on any of them. There is no branch in it that emits an empty
cell, a default, or an "unmeasured" that reads like an answer.

## What is NOT on this page

- **A distribution matrix.** See the section above: every row here ran with no
  distribution policy loaded. "Does vitrind run on Ubuntu 24.04" is not answered by
  the `6.8.0-139-generic` row alone — that row answers "does this build's floor admit
  Ubuntu 24.04's GA kernel", and the answer is no.
- **One row per ABI rung.** Five kernels reported five distinct ABIs; four of this
  build's nine rungs are reported by none of them. The per-rung behaviour table is
  [the isolation matrix](isolation-matrix.md), which is generated from source and
  measures no machine.
- **A claim that these rows are current.** Each carries a collection date. Nothing in
  a pull request re-boots them, which is a deliberate scope decision and not an
  oversight; `collect.sh --check` is how the claim gets re-taken, and it goes red on a
  row older than 180 days when somebody runs it.
- **Anything about non-x86-64, or about kernels not in the table.** No row here says
  where between ABI 5 and ABI 6 a given mainline release sits. That mapping
  is a fact about mainline, and this page publishes measurements rather than lookups.

