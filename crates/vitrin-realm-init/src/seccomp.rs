// SPDX-License-Identifier: MPL-2.0
//! K13b -- the seccomp-bpf deny-list the realm's shim and app inherit
//! (P2.6.4, issue #188).
//!
//! # The table is the deliverable; the BPF is a compilation of it
//!
//! [`ROWS`] is the artefact a reviewer reads. Every row carries the **escape
//! class it closes**, named from PRD Doc 2 §15's actor table and typed as
//! [`EscapeClass`] rather than written as prose, so a row cannot name a class
//! the PRD does not have. [`every_class_label_is_in_the_prd`] holds the eight
//! labels to `docs/PRD.md` verbatim, which is what stops a §15 rewording from
//! silently orphaning this file. [`compile`] turns the table into classic BPF
//! and nothing else in this module decides policy.
//!
//! # Deny-list, not allow-list, and that is v0's whole posture
//!
//! Firefox's syscall surface is not enumerable at this stage, and an
//! allow-list would fail closed against the project's own acceptance app
//! (`tests/integration/test_real_firefox.py`). So this closes the **named**
//! classes and says plainly that the residual surface is unenumerated. A
//! reader who sees "seccomp filter" and reads "syscall-confined" has been
//! misled, which is why `docs/book/src/limits.md` states the posture in the
//! same paragraph as the mechanism.
//!
//! # What is deliberately NOT here, and why the absences are the argument
//!
//! Under #188's positive-control rule a row that denies something a realm
//! already cannot do is not confinement -- it is a longer table. Measured
//! inside the full realm ladder on Arch `7.1.8-arch1-3` (Landlock ABI 9,
//! 2026-08-16), the following are **already impossible before any filter
//! exists**, so none of them is a row:
//!
//! - `mount`, `umount2`, `pivot_root`, `open_tree`, `move_mount`, `fsopen`,
//!   `fsconfig`, `fsmount`, `fspick`, `mount_setattr`, `chroot` -- the
//!   Landlock domain (mount) and the K10 capability drop (the rest). The
//!   control run is decisive: with namespaces and capabilities but **no**
//!   Landlock domain, `mount` and `chroot` return `0`; with the domain they
//!   return `EPERM`. Flatpak's entire "subnamespace setups" and "new mount
//!   manipulation APIs" blocks would be fourteen rows no positive control can
//!   demonstrate.
//! - `unshare(CLONE_NEWUSER)` and `clone(CLONE_NEWUSER)` -- `ENOSPC`, from
//!   K9b's `user.max_user_namespaces = 0`.
//! - `clone3(2)`. Flatpak and Firefox's own filter both deny it (`ENOSYS`)
//!   because seccomp cannot read `struct clone_args`, so a `clone(2)` flag
//!   predicate is bypassable through it. **This build has no `clone` flag
//!   predicate to seal**, because the escape such a predicate would close is
//!   K9b's already. Denying `clone3` would therefore buy no confinement and
//!   cost the highest acceptance-app risk in the candidate table -- every
//!   `fork` in the realm goes through it on glibc >= 2.34. Omitted, and this
//!   paragraph is the record of the decision.
//! - `syslog`, `acct`, `quotactl`, `uselib`, `kexec_load`, `init_module`,
//!   `finit_module`, `delete_module`, `swapon`, `swapoff`, `ioperm`, `iopl`,
//!   `open_by_handle_at` -- every one gated on a capability K10 drops
//!   (measured `EPERM`, except `uselib`, which this kernel answers `ENOSYS`
//!   itself, and `quotactl`, `EINVAL`). `settimeofday`, `clock_settime` and
//!   `reboot` were deliberately **not** run -- a probe that succeeded would
//!   move the operator's clock or reboot his machine -- and are reasoned from
//!   the same capability model.
//! - `ioctl(TIOCSTI)` -- `main.rs`'s K10 `setsid()` leaves the realm with no
//!   controlling terminal, so there is no descriptor to issue it on.
//! - `socket(AF_PACKET)` -- already `EPERM` without `CAP_NET_RAW`. The
//!   socket-family row below is demonstrated against `AF_VSOCK` and `AF_ALG`,
//!   never against `AF_PACKET`, and `tests/integration/test_real_seccomp.py`
//!   says so.
//! - `migrate_pages` -- needs `CAP_SYS_NICE`, and answers `EINVAL` on this
//!   box before any filter. Dropped rather than carried as a labelled
//!   not-demonstrated row.
//!
//! # Raw syscalls, and no seccomp crate
//!
//! `Cargo.toml` records a **one-dependency budget** for this binary against
//! plan risk R7, and `landlock.rs` already spells three syscall numbers and
//! two `#[repr(C)]` structs by hand for the same reason. `libseccomp` is a C
//! library with its own allocator behaviour; `seccompiler` and `syscallz` are
//! third-party crates in the process that *builds* a realm's confinement.
//! Classic BPF is four opcodes and one struct, and the assembler below is
//! sixty lines with a unit test that disassembles its own output.
//!
//! # Where this runs, and why the position is not negotiable
//!
//! Inside `main.rs`'s `exec_shim` -- the BINARY half of this crate, which is
//! why this module lives in the library half instead: `vitrind` needs the same
//! table to answer `--print-seccomp` -- on PID 1 of the realm's pid namespace:
//!
//! - **after** `PR_SET_NO_NEW_PRIVS`, which K10 already sets -- this module
//!   [`verify_no_new_privs`]es rather than setting it a second time, per
//!   #188's "unconditionally and first" read as a property to check;
//! - **after** the Landlock ruleset (K12b) and after K13's `close_range`,
//!   because `landlock_restrict_self` and `close_range` are themselves
//!   syscalls, and a filter installed before them would have to carry rows
//!   permitting them;
//! - **immediately before** the `execve`, because a seccomp filter can never
//!   be removed and everything after it runs under it -- including `execve`
//!   itself, which is why `execve` is not a row;
//! - in the **helper**, not the shim: the shim is this binary's direct
//!   `execve` target, so §15's "compromised shim" row is answered by
//!   construction rather than by the shim cooperating.
//!
//! # A foreign syscall ABI is killed, not passed
//!
//! Syscall numbers are per-ABI. A process executing under i386 or x32 on an
//! x86-64 kernel would meet a table whose numbers mean other syscalls, so the
//! filter answers a non-native `arch` with `SECCOMP_RET_KILL_PROCESS` rather
//! than letting it through unfiltered. The cost is stated rather than hidden:
//! **a realm cannot execute a 32-bit binary on a 64-bit host under this
//! build.** Passing it would be a bypass, and returning an errno for every
//! syscall would include `exit_group`.

// ---------------------------------------------------------------------------
// PRD Doc 2 §15 -- the escape classes, as a closed vocabulary
// ---------------------------------------------------------------------------

/// One actor row of PRD Doc 2 §15.
///
/// An enum and not a `&'static str` field, because #188 asks for "a lint or
/// test asserts no row has an empty class field" and a non-empty free-text
/// string is trivially satisfied by a row naming nothing real. A row cannot
/// name a class §15 does not have, and [`every_class_label_is_in_the_prd`]
/// asserts every label here still appears verbatim in `docs/PRD.md`.
///
/// All eight are carried even though this table uses two, because the two
/// this filter answers are only meaningful against the six it does not --
/// which is the sentence `docs/book/src/limits.md` publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeClass {
    /// §15 row 1. Answered **in part**: the kernel-surface and keyring rows
    /// bite here, and nothing in this table touches that row's "Cannot do"
    /// column, which is namespaces, the compositor and the grant model.
    MaliciousAppInAShim,
    /// §15 row 2. **Not answered by any row.** Encryption is ordinary
    /// read/write on already-granted fds; this is Landlock's row and the
    /// powerbox's, end to end.
    RansomwarePayloadInARealm,
    /// §15 row 3. Answered at **two services §4.5's own enumeration misses**
    /// -- the operator's kernel keyring and `AF_VSOCK` -- and at none of the
    /// six it names, every one of which is closed by construction.
    ReachableServiceLateralEscape,
    /// §15 row 4. **Not answered.** An agent is a wire client on the far side
    /// of the socket; it executes no syscall inside a realm.
    HijackedPromptInjectedAgent,
    /// §15 row 5. **Not answered.** Credential validation, at the transport.
    MaliciousAgentClient,
    /// §15 row 6. **Not answered.** Consent and claim subsetting.
    MaliciousRelyingApp,
    /// §15 row 7. **Not answered.** Identity verification and TOFU pinning.
    ImpersonatingDaemon,
    /// §15 row 8. **This filter's real class** -- the only §15 row that names
    /// seccomp, and the one whose parenthetical "unprivileged,
    /// seccomp-confined" was an overclaim until this task landed.
    CompromisedShim,
}

/// Every §15 actor, in the PRD's own order. The table's exhaustiveness check
/// and the PRD cross-check both iterate this.
pub const ESCAPE_CLASSES: &[EscapeClass] = &[
    EscapeClass::MaliciousAppInAShim,
    EscapeClass::RansomwarePayloadInARealm,
    EscapeClass::ReachableServiceLateralEscape,
    EscapeClass::HijackedPromptInjectedAgent,
    EscapeClass::MaliciousAgentClient,
    EscapeClass::MaliciousRelyingApp,
    EscapeClass::ImpersonatingDaemon,
    EscapeClass::CompromisedShim,
];

impl EscapeClass {
    /// The actor's name **exactly as PRD Doc 2 §15's first column spells it**,
    /// bold markers stripped. Held to the PRD by a test.
    pub const fn actor(self) -> &'static str {
        match self {
            EscapeClass::MaliciousAppInAShim => "Malicious app in a shim",
            EscapeClass::RansomwarePayloadInARealm => "Ransomware payload in a realm",
            EscapeClass::ReachableServiceLateralEscape => "Reachable-service lateral escape",
            EscapeClass::HijackedPromptInjectedAgent => "Hijacked / prompt-injected agent",
            EscapeClass::MaliciousAgentClient => "Malicious agent client (bad credential)",
            EscapeClass::MaliciousRelyingApp => "Malicious relying app (wallet)",
            EscapeClass::ImpersonatingDaemon => "Impersonating daemon / lookalike publisher",
            EscapeClass::CompromisedShim => "Compromised shim",
        }
    }
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// How a row decides whether a call is denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Predicate {
    /// The syscall number alone. Every call is denied.
    Always,
    /// Denied unless argument `arg` is one of `allowed`.
    ///
    /// The comparison is over the **full 64-bit** register: a call whose high
    /// word is non-zero is denied even if its low word is an allowed value,
    /// because the kernel truncates to `int` and an allow-list that ignored
    /// the high word would be a hole rather than a compatibility measure.
    /// Denying there is the fail-closed direction -- the worst it can do is
    /// refuse a caller who wrote `AF_UNIX` in a strange way.
    ArgNotIn { arg: u8, allowed: &'static [u32] },
}

/// One denied syscall, or syscall+argument predicate.
#[derive(Clone, Copy)]
pub struct Row {
    /// The stable key the probe client and the integration gate agree on.
    /// Lower-case, no parentheses; this is what appears in the probe report.
    pub name: &'static str,
    /// This architecture's syscall number.
    pub nr: libc::c_long,
    /// The §15 actor row this row closes. Typed, never free text.
    pub class: EscapeClass,
    /// The errno the filter returns. **Part of the contract**: `EPERM` and
    /// `ENOSYS` are behaviourally different, and the integration gate asserts
    /// the exact value rather than "not zero".
    ///
    /// That gate reads the expected errno out of *this field*, though, so it
    /// cannot see this field change. What holds a row's errno still is the
    /// literal beside it in `tests::PINNED`, which every row has and which is
    /// the only copy in the repository not derived from the table.
    pub errno: i32,
    /// The spelling of [`Self::errno`], for the printed table. Held to
    /// `libc` by [`every_errno_name_matches_libc`].
    pub errno_name: &'static str,
    pub predicate: Predicate,
    /// Why this row exists, in one paragraph, naming what it closes and what
    /// it does not. #188: "a row whose comment cannot name a class from §15
    /// is a row that has not justified its cost."
    pub why: &'static str,
    /// A **non-seccomp** mechanism that already denies this syscall on some
    /// kernels, or `None`.
    ///
    /// This is documentation, never a verdict: whether a row is *demonstrated*
    /// is measured per run by the positive control in
    /// `tests/integration/test_real_seccomp.py`, because the answer differs
    /// per kernel and per sysctl. A row that carries a shadow can still be
    /// demonstrated on a kernel whose sysctl permits the call.
    pub shadow: Option<&'static str>,
}

/// `AF_UNSPEC`, `AF_UNIX`, `AF_INET`, `AF_INET6`, `AF_NETLINK` -- the five
/// families a realm keeps, and the reason each one stays.
///
/// `AF_UNIX` is the wire protocol and the Wayland socket; `AF_INET`/`AF_INET6`
/// are Firefox's (the realm's netns is loopback-only, so this is not egress);
/// `AF_NETLINK` is glib's and GTK's network monitor. `AF_UNSPEC` is what
/// `socket(2)` callers pass when probing. All five measured reachable inside
/// a realm on 2026-08-16 and **must stay so** -- the negative controls in
/// `test_real_seccomp.py` fail if any of them starts returning an errno.
const ALLOWED_SOCKET_FAMILIES: &[u32] = &[
    libc::AF_UNSPEC as u32,
    libc::AF_UNIX as u32,
    libc::AF_INET as u32,
    libc::AF_INET6 as u32,
    libc::AF_NETLINK as u32,
];

/// `PER_LINUX` and the query form `0xffffffff`.
///
/// Flatpak denies every value other than the process's own persona, which
/// takes `personality(0xffffffff)` -- the *read* glibc and `setarch` issue --
/// with it. Keeping the query costs nothing and closes the same thing:
/// `ADDR_NO_RANDOMIZE` (0x0040000) is not in this list.
const ALLOWED_PERSONALITIES: &[u32] = &[0x0000_0000, 0xffff_ffff];

/// How many rows [`ROWS`] holds, spelled as a literal.
///
/// **A second copy of a number, deliberately, and it is the smaller of two
/// evils.** `ROWS.len()` is the real answer and this constant is checked
/// against it by [`the_published_row_count_is_the_tables_own`] -- so it cannot
/// drift from the table. What it buys is a canonical source `cargo xtask
/// limits-check` can *read*: the size of this table is published on six
/// surfaces (`docs/book/src/limits.md`, `README.md`, `SECURITY.md`,
/// `site/index.html`, `examples/realm.toml` and the Phase-2 plan), because "a
/// deny-list" is a claim whose size is half its meaning, and #172's derived
/// -value gate can only hold six surfaces to one number if the number exists
/// as a literal somewhere. Without this, adding a fourteenth row would leave
/// six pages saying thirteen and nothing would be red.
pub const ROW_COUNT: usize = 13;

/// **The filter table.** One row per denied syscall or syscall+argument
/// predicate; the BPF below is its compilation and adds nothing.
pub const ROWS: &[Row] = &[
    Row {
        name: "keyctl",
        nr: libc::SYS_keyctl,
        class: EscapeClass::ReachableServiceLateralEscape,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "The kernel keyring is namespaced by nothing `vitrin-realm-init` unshares, so a \
              realm inherits possession of the OPERATOR's session keyring and gets its \
              possessor-permission bits. Measured on 2026-08-16 from inside the full ladder: a \
              key planted on the operator's session keyring was found by NAME with \
              KEYCTL_SEARCH and read back in PLAINTEXT with KEYCTL_READ. That is §15's \
              `Attempt to reach a privileged local service to borrow its ambient authority`, at \
              a service PRD Doc 2 §4.5's `there is simply nothing to reach` sentence does not \
              enumerate. EPERM and not ENOSYS: a kernel without CONFIG_KEYS does not exist on \
              any target, so ENOSYS would be a lie a library could act on. Flatpak denies the \
              whole keyring group with EPERM for the same reason.",
        shadow: None,
    },
    Row {
        name: "add_key",
        nr: libc::SYS_add_key,
        class: EscapeClass::ReachableServiceLateralEscape,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "The write half of the same finding, measured in the same run: from inside the \
              realm, `add_key` against KEY_SPEC_SESSION_KEYRING landed a key the host-side \
              cleanup then found on the operator's own session keyring. A realm can plant a key \
              the operator's session will later resolve by description -- a confused-deputy \
              primitive against anything on the host that looks keys up by name.",
        shadow: None,
    },
    Row {
        name: "request_key",
        nr: libc::SYS_request_key,
        class: EscapeClass::ReachableServiceLateralEscape,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "The third keyring entry point, and the one that reaches the upcall path: \
              `request_key` can instantiate a key through /sbin/request-key and searches the \
              same unnamespaced keyrings the other two reach. Denying two of three would leave \
              the class open, which is why Flatpak denies them as a group. NOTE the \
              behavioural change the gate pins: unfiltered this syscall RUNS and answers ENOKEY \
              for a key that is absent; under the filter it answers EPERM before searching.",
        shadow: None,
    },
    Row {
        name: "io_uring_setup",
        nr: libc::SYS_io_uring_setup,
        class: EscapeClass::CompromisedShim,
        errno: libc::ENOSYS,
        errno_name: "ENOSYS",
        predicate: Predicate::Always,
        why: "Load-bearing for the REST of this table rather than only for itself: io_uring \
              submits work through a shared ring instead of through syscalls, so an operation \
              issued through it is never seen by a seccomp filter and IORING_OP_OPENAT would \
              hole every other row here. It is also the largest unprivileged kernel-attack \
              surface of the last five years -- §15's `Escape its sandbox into the core \
              (unprivileged, seccomp-confined)`. Denying `setup` alone suffices: \
              `io_uring_enter` and `io_uring_register` need a ring fd, and after K13's \
              `close_range` the only descriptor the shim inherits is its core connection. \
              ENOSYS and NOT Docker's blanket EPERM: ENOSYS is the pre-5.1 answer every \
              liburing consumer already handles, and EPERM is the errno that produced the \
              real-world breakage in moby/moby#47532, where apps read a denied ring as a \
              transient failure rather than an absent feature.",
        shadow: Some(
            "kernel.io_uring_disabled=2 denies it kernel-wide on kernels that have \
                      that sysctl",
        ),
    },
    Row {
        name: "socket_family",
        nr: libc::SYS_socket,
        class: EscapeClass::ReachableServiceLateralEscape,
        errno: libc::EAFNOSUPPORT,
        errno_name: "EAFNOSUPPORT",
        predicate: Predicate::ArgNotIn {
            arg: 0,
            allowed: ALLOWED_SOCKET_FAMILIES,
        },
        why: "AF_VSOCK is the concrete finding, and it is the second reachable service §4.5's \
              enumeration misses: vsock addressing is NOT scoped by the network namespace, so \
              on the project's declared ABI-6 deploy target -- a Debian 13 VPS -- a realm's own \
              netns does not close a path to the HYPERVISOR. AF_ALG is the kernel crypto API, a \
              recurring CVE surface with no use in a display realm, and is the second half of \
              the positive control. AF_PACKET is deliberately NOT the demonstration: it is \
              already EPERM without CAP_NET_RAW, which K10 drops. EAFNOSUPPORT is the strongest \
              errno argument in the table -- it is exactly what a kernel built without that \
              family returns, so every socket-using library already has the branch. Flatpak \
              ships the same choice.",
        shadow: Some(
            "AF_PACKET alone is already EPERM without CAP_NET_RAW; AF_VSOCK and AF_ALG \
                      are not",
        ),
    },
    Row {
        name: "ptrace",
        nr: libc::SYS_ptrace,
        class: EscapeClass::CompromisedShim,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "The classic sandbox-escape lever -- attach to a less-confined sibling and rewrite \
              its registers. Inside a realm the honest claim is `the shim cannot ptrace its own \
              app, and the app cannot ptrace the shim`: realm-to-core and realm-to-realm ptrace \
              were already impossible at #186, because the PID namespace means a realm cannot \
              NAME a pid outside itself. \
              **RISK STATED RATHER THAN DISCOVERED BY A USER:** the pinned Firefox ESR \
              140.12.0's libxul.so does contain ptrace users -- minidump-writer's ptrace_dumper \
              -- in its CRASH REPORTER, and `tests/integration/test_real_firefox.py` sets \
              MOZ_CRASHREPORTER_DISABLE=1, so the acceptance gate goes green WITHOUT exercising \
              the path this row breaks. Treat that green tick as non-evidence for this row. A \
              blanket denial also takes PTRACE_TRACEME, which is how sanitizers and debuggers \
              self-attach.",
        shadow: Some(
            "kernel.yama.ptrace_scope=3 denies it process-wide, and the PID namespace \
                      already denies every cross-realm target",
        ),
    },
    Row {
        name: "perf_event_open",
        nr: libc::SYS_perf_event_open,
        class: EscapeClass::CompromisedShim,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "A long CVE history and no use in a display realm; Flatpak's precedent is explicit \
              (`perf has been the source of many CVEs`). EPERM matches what \
              kernel.perf_event_paranoid already returns where it denies, so no caller learns a \
              new error. Measured with a real `struct perf_event_attr` (a software CPU-clock \
              counter on the calling process): it SUCCEEDS unfiltered on a box with \
              perf_event_paranoid=2, so this row is demonstrable there rather than shadowed -- \
              an earlier hand-rolled probe reported EACCES and was wrong.",
        shadow: Some("kernel.perf_event_paranoid=3 denies it for unprivileged callers"),
    },
    Row {
        name: "bpf",
        nr: libc::SYS_bpf,
        class: EscapeClass::CompromisedShim,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "The eBPF verifier is among the most productive unprivileged-LPE surfaces of the \
              last five years and a realm has no legitimate BPF use. EPERM deliberately matches \
              what kernel.unprivileged_bpf_disabled already returns, so software probing for \
              unprivileged BPF sees the answer it expects; ENOSYS would falsely claim a kernel \
              without BPF at all. **Demonstrability is per-kernel**: on a box with \
              unprivileged_bpf_disabled=2 the positive control fails and the gate reports this \
              row NOT DEMONSTRATED rather than counting it.",
        shadow: Some("kernel.unprivileged_bpf_disabled=1 or 2"),
    },
    Row {
        name: "userfaultfd",
        nr: libc::SYS_userfaultfd,
        class: EscapeClass::CompromisedShim,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::Always,
        why: "userfaultfd lets userspace stall the kernel mid-copy, which is what turns many \
              kernel race conditions into reliable exploits; it is the standard heap-grooming \
              primitive in kernel exploit chains and has no display-stack use. EPERM matches \
              what vm.unprivileged_userfaultfd=0 already returns. **Demonstrability is \
              per-kernel**, exactly as for `bpf`.",
        shadow: Some("vm.unprivileged_userfaultfd=0"),
    },
    Row {
        name: "personality",
        nr: libc::SYS_personality,
        class: EscapeClass::CompromisedShim,
        errno: libc::EPERM,
        errno_name: "EPERM",
        predicate: Predicate::ArgNotIn {
            arg: 0,
            allowed: ALLOWED_PERSONALITIES,
        },
        why: "personality(ADDR_NO_RANDOMIZE) disables ASLR for the process and everything it \
              execs -- a free step in nearly every memory-corruption chain, and therefore part \
              of §15's `Escape its sandbox into the core`. The predicate rather than a blanket \
              denial is Flatpak's shape and the right one: personality(0xffffffff) is the \
              ordinary QUERY glibc and setarch issue, and denying it would break them for \
              nothing.",
        shadow: None,
    },
    Row {
        name: "mbind",
        nr: libc::SYS_mbind,
        class: EscapeClass::CompromisedShim,
        errno: libc::ENOSYS,
        errno_name: "ENOSYS",
        predicate: Predicate::Always,
        why: "Flatpak's `Scary VM/NUMA ops` group: real but modest kernel-surface reduction. \
              ENOSYS and NOT Flatpak's EPERM, on the authority of the project's own acceptance \
              app -- Firefox's security/sandbox/linux/SandboxFilter.cpp returns ENOSYS for the \
              NUMA group under the comment `Required by libnuma for FFmpeg`, in three separate \
              policy classes, because libnuma probes and falls back on ENOSYS and treats EPERM \
              as NUMA being broken.",
        shadow: None,
    },
    Row {
        name: "set_mempolicy",
        nr: libc::SYS_set_mempolicy,
        class: EscapeClass::CompromisedShim,
        errno: libc::ENOSYS,
        errno_name: "ENOSYS",
        predicate: Predicate::Always,
        why: "The same group and the same errno argument as `mbind`. `get_mempolicy` is \
              DELIBERATELY absent from this table: Firefox's filter returns Allow() for it \
              under the same libnuma comment, so denying it would break the acceptance app in \
              exchange for nothing. `tests/integration/test_real_seccomp.py` asserts \
              get_mempolicy still succeeds inside a realm.",
        shadow: None,
    },
    Row {
        name: "move_pages",
        nr: libc::SYS_move_pages,
        class: EscapeClass::CompromisedShim,
        errno: libc::ENOSYS,
        errno_name: "ENOSYS",
        predicate: Predicate::Always,
        why: "The third of the NUMA group, same errno argument. `migrate_pages` is not a row: \
              it needs CAP_SYS_NICE, which K10 drops, and answers EINVAL before any filter -- a \
              row no positive control can demonstrate.",
        shadow: None,
    },
];

/// Syscalls this filter **must never deny**, and the reason each one is a
/// standing negative control rather than a comment.
///
/// A seccomp filter is inherited and unremovable, so anything denied here is
/// denied to Firefox's own sandbox too. `tests/integration/test_real_seccomp.py`
/// probes every one of these *inside* a realm and fails if any is refused.
pub const NEVER_DENIED: &[(&str, &str)] = &[
    (
        "seccomp",
        "Firefox and Chromium install their own filters for content processes. It succeeds \
         inside a realm today precisely because K10 already set PR_SET_NO_NEW_PRIVS, which is \
         what makes nested filtering legal for an unprivileged process. A deny here breaks the \
         acceptance app's entire sandbox and would present as an unrelated startup failure.",
    ),
    (
        "get_mempolicy",
        "Firefox's own filter Allow()s it in three policy classes under `Required by libnuma \
         for FFmpeg`, while returning ENOSYS for the setters beside it. The asymmetry is \
         deliberate upstream and is copied deliberately here.",
    ),
    (
        "socket_unix",
        "The wire protocol and the Wayland socket. If this ever fails inside a realm the \
         socket-family predicate has regressed and nothing else in the stack works either.",
    ),
    (
        "socket_inet",
        "Firefox. The realm's netns is loopback-only, so keeping AF_INET is not egress.",
    ),
    (
        "socket_inet6",
        "The same, and named separately rather than folded into `socket_inet` because the two \
         are different values of the predicate's argument and a filter can lose one without \
         losing the other.",
    ),
    ("socket_netlink", "glib and GTK's network monitor."),
    (
        "personality_query",
        "`personality(0xffffffff)` is the READ form glibc and setarch issue, and the reason \
         this row is an argument predicate rather than Flatpak's blanket denial. If it ever \
         fails the predicate has collapsed into a blanket deny and the row is breaking callers \
         for nothing.",
    ),
    (
        "fork",
        "Process creation. This build denies neither `clone` nor `clone3`, so a realm forks \
         exactly as it did before the filter -- see this module's docs for why `clone3` is not \
         a row.",
    ),
];

// ---------------------------------------------------------------------------
// Classic BPF
// ---------------------------------------------------------------------------

/// `struct sock_filter`. Spelled here rather than taken from `libc` for the
/// same reason `landlock.rs` spells its structs: this file is the whole
/// surface, and a `#[repr(C)]` of four fields is cheaper to audit than a
/// dependency's version matrix.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insn {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// `struct sock_fprog`.
#[repr(C)]
struct Fprog {
    len: u16,
    filter: *const Insn,
}

// BPF opcodes. `BPF_LD | BPF_W | BPF_ABS`, `BPF_JMP | BPF_JEQ | BPF_K`,
// `BPF_JMP | BPF_JGE | BPF_K`, `BPF_JMP | BPF_JA`, `BPF_RET | BPF_K`.
const LD_ABS_W: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JGE_K: u16 = 0x35;
const JA: u16 = 0x05;
const RET_K: u16 = 0x06;

// `struct seccomp_data` field offsets. Little-endian: a 64-bit argument's low
// word is at its own offset and the high word four bytes later.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
const OFF_ARGS: u32 = 16;

/// `SECCOMP_RET_KILL_PROCESS`, `SECCOMP_RET_ERRNO`, `SECCOMP_RET_ALLOW`.
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_DATA: u32 = 0x0000_ffff;

/// `SECCOMP_SET_MODE_FILTER`.
const SET_MODE_FILTER: libc::c_ulong = 1;

/// `AUDIT_ARCH_*` for the architecture this build's syscall numbers belong to.
///
/// A `compile_error!` and not a fallback on any other architecture: a filter
/// carrying x86-64 numbers on a machine that numbers syscalls differently
/// would deny the wrong calls, which is worse than not building.
#[cfg(target_arch = "x86_64")]
const NATIVE_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const NATIVE_ARCH: u32 = 0xc000_00b7;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
    "vitrin-realm-init's seccomp table carries per-architecture syscall numbers and an \
     AUDIT_ARCH constant for x86_64 and aarch64 only. Port both before building here: a filter \
     with the wrong numbers denies the wrong syscalls."
);

/// `__X32_SYSCALL_BIT`. On x86-64 the x32 ABI reports `AUDIT_ARCH_X86_64` and
/// sets this bit in the syscall number, so an `arch` check alone does not
/// separate the two.
#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Where a jump goes. Every target is in the program's tail, which is emitted
/// last, so every jump is forward and the offsets are known once the body's
/// length is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Label {
    /// The next instruction.
    Fall,
    /// `n` instructions past the next one -- a within-block skip whose length
    /// the emitter knows because it emits the block.
    Skip(u8),
    Allow,
    Kill,
    /// The tail's `ret ERRNO(e)` for this errno.
    Errno(i32),
}

struct Draft {
    code: u16,
    jt: Label,
    jf: Label,
    k: u32,
}

/// Why a table could not be compiled. Both arms are unreachable for [`ROWS`]
/// and are checked by [`the_shipped_table_compiles_within_the_jump_limits`];
/// they exist so a future row cannot silently produce a malformed filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileError {
    /// A conditional jump offset does not fit the 8 bits classic BPF gives it.
    /// Reachable only from a table large enough that a row is more than 255
    /// instructions from the tail.
    JumpTooFar,
    /// The program exceeds `BPF_MAXINSNS` (4096).
    TooLong,
}

/// Compile [`ROWS`] into a classic-BPF program.
///
/// Allocates, which is allowed here: this binary is single-threaded by design
/// (see `main.rs`'s module docs) and the program is built **before** the
/// install, so [`install`] itself is allocation-free and safe to call from a
/// forked child of a multi-threaded test harness.
pub fn compile(rows: &[Row]) -> Result<Vec<Insn>, CompileError> {
    // The prologue. A foreign ABI is killed rather than passed: this table's
    // numbers mean other syscalls under i386 or x32.
    let mut body: Vec<Draft> = vec![
        Draft {
            code: LD_ABS_W,
            jt: Label::Fall,
            jf: Label::Fall,
            k: OFF_ARCH,
        },
        Draft {
            code: JEQ_K,
            jt: Label::Fall,
            jf: Label::Kill,
            k: NATIVE_ARCH,
        },
        Draft {
            code: LD_ABS_W,
            jt: Label::Fall,
            jf: Label::Fall,
            k: OFF_NR,
        },
    ];
    // x32 reports `AUDIT_ARCH_X86_64` and sets a bit in the syscall number
    // instead, so the `arch` check above does not separate the two.
    #[cfg(target_arch = "x86_64")]
    body.push(Draft {
        code: JGE_K,
        jt: Label::Kill,
        jf: Label::Fall,
        k: X32_SYSCALL_BIT,
    });

    // Simple rows first, while the accumulator still holds the syscall number
    // and no reload is needed between them.
    for row in rows {
        if matches!(row.predicate, Predicate::Always) {
            body.push(Draft {
                code: JEQ_K,
                jt: Label::Errno(row.errno),
                jf: Label::Fall,
                k: row.nr as u32,
            });
        }
    }

    // Then the argument predicates. Each block reloads the syscall number
    // itself, so the blocks are independent and their order does not matter.
    let mut first_block = true;
    for row in rows {
        let Predicate::ArgNotIn { arg, allowed } = row.predicate else {
            continue;
        };
        if !first_block {
            body.push(Draft {
                code: LD_ABS_W,
                jt: Label::Fall,
                jf: Label::Fall,
                k: OFF_NR,
            });
        }
        first_block = false;
        // `JEQ nr` falls into the block or skips the whole of it. The block
        // after this instruction is: LD hi, JEQ 0, LD lo, JEQ x N, JA.
        let rest = u8::try_from(allowed.len() + 4).map_err(|_| CompileError::JumpTooFar)?;
        body.push(Draft {
            code: JEQ_K,
            jt: Label::Fall,
            jf: Label::Skip(rest),
            k: row.nr as u32,
        });
        let base = OFF_ARGS + u32::from(arg) * 8;
        body.push(Draft {
            code: LD_ABS_W,
            jt: Label::Fall,
            jf: Label::Fall,
            k: base + 4,
        });
        body.push(Draft {
            code: JEQ_K,
            jt: Label::Fall,
            jf: Label::Errno(row.errno),
            k: 0,
        });
        body.push(Draft {
            code: LD_ABS_W,
            jt: Label::Fall,
            jf: Label::Fall,
            k: base,
        });
        for value in allowed {
            body.push(Draft {
                code: JEQ_K,
                jt: Label::Allow,
                jf: Label::Fall,
                k: *value,
            });
        }
        // Nothing in the allow list matched, so the call is denied. A `JA`
        // and not a conditional, because there is nothing left to compare:
        // classic BPF's unconditional jump takes a 32-bit `k` rather than an
        // 8-bit offset, so the resolver reads its destination from `jt` and
        // writes the distance into `k`.
        body.push(Draft {
            code: JA,
            jt: Label::Errno(row.errno),
            jf: Label::Fall,
            k: 0,
        });
    }

    // The tail. `ALLOW` first so a program that falls off the body allows,
    // which is the deny-list's whole shape.
    let mut errnos: Vec<i32> = Vec::new();
    for row in rows {
        if !errnos.contains(&row.errno) {
            errnos.push(row.errno);
        }
    }
    let body_len = body.len();
    let allow_at = body_len;
    let errno_at = |e: i32| body_len + 1 + errnos.iter().position(|x| *x == e).expect("listed");
    let kill_at = body_len + 1 + errnos.len();

    let resolve = |label: Label, at: usize| -> Result<u8, CompileError> {
        let target = match label {
            Label::Fall => return Ok(0),
            Label::Skip(n) => return Ok(n),
            Label::Allow => allow_at,
            Label::Kill => kill_at,
            Label::Errno(e) => errno_at(e),
        };
        u8::try_from(target - at - 1).map_err(|_| CompileError::JumpTooFar)
    };

    let mut out: Vec<Insn> = Vec::with_capacity(body_len + errnos.len() + 2);
    for (at, draft) in body.iter().enumerate() {
        // A `JA`'s `k` is an instruction count, not a comparison operand, and
        // it is 32 bits wide -- so it is resolved from `jt` and never clamped.
        let k = if draft.code == JA {
            let target = match draft.jt {
                Label::Allow => allow_at,
                Label::Kill => kill_at,
                Label::Errno(e) => errno_at(e),
                Label::Fall | Label::Skip(_) => at + 1,
            };
            (target - at - 1) as u32
        } else {
            draft.k
        };
        out.push(Insn {
            code: draft.code,
            jt: if draft.code == JA {
                0
            } else {
                resolve(draft.jt, at)?
            },
            jf: if draft.code == JA {
                0
            } else {
                resolve(draft.jf, at)?
            },
            k,
        });
    }
    out.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: RET_ALLOW,
    });
    for errno in &errnos {
        out.push(Insn {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: RET_ERRNO | (*errno as u32 & RET_DATA),
        });
    }
    out.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: RET_KILL_PROCESS,
    });
    if out.len() > 4096 {
        return Err(CompileError::TooLong);
    }
    Ok(out)
}

/// `PR_GET_NO_NEW_PRIVS` must already read 1.
///
/// #188 asks for `PR_SET_NO_NEW_PRIVS` "unconditionally and first". K10's
/// [`crate::drop_all_capabilities`] already sets it -- setting it a second
/// time here would make two sites responsible for one property and would pass
/// whether or not the first one ran. So this **checks** instead: without it
/// `seccomp(2)` refuses `EACCES` for an unprivileged caller anyway, and a
/// refusal that names the missing bit is worth more than a second `prctl`.
fn verify_no_new_privs() -> Result<(), i32> {
    // SAFETY: a prctl with no out-parameter.
    let set = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
    if set != 1 {
        eprintln!(
            "vitrin-realm-init: PR_GET_NO_NEW_PRIVS read {set}, not 1. K10 sets it before the \
             capability drop; a realm that reaches the seccomp filter without it is a realm \
             whose earlier steps did not run, and an unprivileged seccomp(2) would refuse \
             anyway."
        );
        return Err(libc::EACCES);
    }
    Ok(())
}

/// Install a compiled program. **Allocation-free**, so a forked child of a
/// multi-threaded test harness may call it.
///
/// Returns the kernel's errno on failure. `seccomp(2)` is 3.17 and this build
/// requires far newer, but the `prctl` fallback costs two lines and is the
/// difference between a refusal and a wrong diagnosis on an exotic kernel.
///
/// # Safety
///
/// `prog` must outlive the call. The filter is **irreversible**: after this
/// returns `Ok`, this process and every descendant carry it for life.
pub fn install(prog: &[Insn]) -> Result<(), i32> {
    let fprog = Fprog {
        len: prog.len() as u16,
        filter: prog.as_ptr(),
    };
    // SAFETY: `fprog` describes `prog`, which the caller keeps alive.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SET_MODE_FILTER,
            0 as libc::c_ulong,
            &fprog as *const Fprog,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let first = errno();
    if first != libc::ENOSYS {
        return Err(first);
    }
    // SAFETY: same argument; `PR_SET_SECCOMP` takes the same `sock_fprog`.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &fprog as *const Fprog,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// The last syscall's errno. Spelled here rather than imported so this module
/// is a leaf of the crate and can live in the library half, where
/// `vitrind --print-seccomp` can reach the table.
fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// What K13b reports to the core: how many rows were compiled, and how many
/// instructions that came to.
pub struct Enforced {
    pub rows: u32,
    pub instructions: u32,
}

/// K13b. Verify `NO_NEW_PRIVS`, compile [`ROWS`], install.
///
/// **There is no `--seccomp=off`.** Landlock has one because a kernel can lack
/// Landlock entirely and an operator on such a machine still needs a session;
/// seccomp filter mode has been mandatory in every kernel this build's floor
/// admits, so a switch would only ever be a way to run a session that reads as
/// confined and is not.
pub fn apply() -> Result<Enforced, i32> {
    verify_no_new_privs()?;
    let prog = compile(ROWS).map_err(|e| {
        eprintln!("vitrin-realm-init: the seccomp table did not compile: {e:?}");
        libc::E2BIG
    })?;
    let instructions = prog.len() as u32;
    install(&prog).inspect_err(|e| {
        eprintln!("vitrin-realm-init: seccomp(SECCOMP_SET_MODE_FILTER) failed: errno {e}");
    })?;
    Ok(Enforced {
        rows: ROWS.len() as u32,
        instructions,
    })
}

/// The `vitrind --print-seccomp` payload: the table, machine-readably.
///
/// A **build** constant like `--print-floor`, never a measurement -- it says
/// what this binary would install, not what any kernel did. The integration
/// gate reads it to learn the row set, which is what makes "a row added
/// without a case fails the test" true without a second hand-written list.
pub fn render_table() -> String {
    let mut out = String::from("vitrin-seccomp 1\n");
    out.push_str(&format!("table.rows={}\n", ROWS.len()));
    match compile(ROWS) {
        Ok(prog) => out.push_str(&format!("table.instructions={}\n", prog.len())),
        Err(e) => out.push_str(&format!("table.instructions=uncompilable ({e:?})\n")),
    }
    out.push_str(&format!("table.arch={NATIVE_ARCH:#010x}\n"));
    for row in ROWS {
        let predicate = match row.predicate {
            Predicate::Always => "always".to_string(),
            Predicate::ArgNotIn { arg, allowed } => format!(
                "arg{arg}-not-in:{}",
                allowed
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("|")
            ),
        };
        out.push_str(&format!(
            "row name={} nr={} errno={} class={} predicate={} shadow={}\n",
            row.name,
            row.nr,
            row.errno_name,
            row.class.actor(),
            predicate,
            match row.shadow {
                Some(_) => "yes",
                None => "no",
            },
        ));
    }
    for (name, _) in NEVER_DENIED {
        out.push_str(&format!("never-denied name={name}\n"));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// #188: "a lint or test asserts no row has an empty class field."
    ///
    /// The enum already makes an empty class unrepresentable, so the check
    /// that earns its keep is the one behind it: every class label is a
    /// **verbatim** row of PRD Doc 2 §15, and every row's `why` is long enough
    /// to be an argument rather than a placeholder.
    #[test]
    fn every_row_names_a_class_and_argues_for_itself() {
        assert!(!ROWS.is_empty(), "a deny-list with no rows is not a filter");
        for row in ROWS {
            assert!(!row.name.is_empty(), "{} has no probe key", row.nr);
            assert!(
                ESCAPE_CLASSES.contains(&row.class),
                "{} names a class outside §15",
                row.name
            );
            assert!(
                !row.class.actor().is_empty(),
                "{} has an empty class label",
                row.name
            );
            assert!(
                row.why.len() > 200,
                "{}'s justification is {} characters. #188: a row whose comment cannot name a \
                 class from §15 is a row that has not justified its cost",
                row.name,
                row.why.len()
            );
            assert!(row.errno > 0, "{} returns errno {}", row.name, row.errno);
        }
        // Probe keys are the join between this table, the probe client and the
        // integration gate, so they must be unique.
        for (i, row) in ROWS.iter().enumerate() {
            for other in &ROWS[i + 1..] {
                assert_ne!(row.name, other.name, "two rows share a probe key");
            }
        }
    }

    /// The class vocabulary is held to the PRD, so a §15 rewording cannot
    /// silently orphan the whole table.
    #[test]
    fn every_class_label_is_in_the_prd() {
        let prd = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/PRD.md"),
        )
        .expect("docs/PRD.md is in this repository");
        for class in ESCAPE_CLASSES {
            assert!(
                prd.contains(class.actor()),
                "PRD Doc 2 §15 no longer contains the actor row `{}`. Either the table renamed \
                 a class -- in which case this vocabulary has to follow it -- or a row of the \
                 actor table was deleted, in which case every seccomp row naming it has lost \
                 its justification",
                class.actor()
            );
        }
        // Non-vacuity: a substring that is NOT a §15 actor must not pass, or
        // the assertion above would be satisfied by any common word.
        assert!(
            !prd.contains("Benevolent app in a shim"),
            "the needle test itself is broken"
        );
    }

    #[test]
    fn every_errno_name_matches_libc() {
        for row in ROWS {
            let expected = match row.errno_name {
                "EPERM" => libc::EPERM,
                "ENOSYS" => libc::ENOSYS,
                "EAFNOSUPPORT" => libc::EAFNOSUPPORT,
                other => panic!(
                    "{} uses errno name {other}, which this test does not know. Add \
                                 it here rather than deleting the check: the printed table is \
                                 what `tests/integration/test_real_seccomp.py` asserts against",
                    row.name
                ),
            };
            assert_eq!(
                row.errno, expected,
                "{}'s errno and errno_name disagree",
                row.name
            );
        }
    }

    /// The two rows every reviewer will look for, pinned by name so a table
    /// edit that drops the keyring finding cannot pass quietly.
    #[test]
    fn the_measured_findings_are_still_rows() {
        let names: Vec<&str> = ROWS.iter().map(|r| r.name).collect();
        for needed in ["keyctl", "add_key", "request_key", "socket_family"] {
            assert!(
                names.contains(&needed),
                "{needed} is the row that answers a reachable service PRD Doc 2 §4.5's `there \
                 is simply nothing to reach` sentence does not cover. Removing it means \
                 rewriting that sentence too"
            );
        }
        // And the ones deliberately NOT denied, so a later edit cannot add
        // them without meeting this test first.
        let denied_numbers: Vec<libc::c_long> = ROWS.iter().map(|r| r.nr).collect();
        for (nr, why) in [
            (
                libc::SYS_clone3,
                "every fork in the realm goes through clone3 on glibc >= 2.34, and the escape \
                 it seals is K9b's already",
            ),
            (
                libc::SYS_get_mempolicy,
                "Firefox's own filter Allow()s it under `Required by libnuma for FFmpeg`",
            ),
            (
                libc::SYS_seccomp,
                "Firefox and Chromium install their own filters for content processes",
            ),
            (
                libc::SYS_mount,
                "a Landlock domain already denies every mount-topology change, measured; a row \
                 here would be a longer table and no more confinement",
            ),
        ] {
            assert!(
                !denied_numbers.contains(&nr),
                "syscall {nr} was added to the table: {why}"
            );
        }
    }

    /// The compiled program, disassembled by hand and checked against the
    /// table it came from.
    #[test]
    fn the_shipped_table_compiles_within_the_jump_limits() {
        let prog = compile(ROWS).expect("the shipped table compiles");
        assert!(prog.len() <= 4096, "BPF_MAXINSNS");
        // Exactly three distinct return values plus one per distinct errno.
        let rets: Vec<u32> = prog
            .iter()
            .filter(|i| i.code == RET_K)
            .map(|i| i.k)
            .collect();
        assert!(
            rets.contains(&RET_ALLOW),
            "a deny-list must allow by default"
        );
        assert!(
            rets.contains(&RET_KILL_PROCESS),
            "a foreign syscall ABI must be killed, not passed"
        );
        for row in ROWS {
            let want = RET_ERRNO | (row.errno as u32 & RET_DATA);
            assert!(
                rets.contains(&want),
                "{} returns {} and the program has no such return",
                row.name,
                row.errno_name
            );
        }
        // Every jump lands inside the program. Classic BPF's verifier refuses
        // anything else, and an out-of-range jump is the one way this
        // assembler could produce a filter the kernel rejects at install time
        // -- on the shim's last step before `execve`, where the failure is a
        // realm that will not start.
        for (at, insn) in prog.iter().enumerate() {
            if insn.code == RET_K {
                continue;
            }
            if insn.code == JA {
                assert!(
                    at + 1 + (insn.k as usize) < prog.len(),
                    "instruction {at} jumps past the end"
                );
                continue;
            }
            assert!(
                at + 1 + (insn.jt as usize) < prog.len()
                    && at + 1 + (insn.jf as usize) < prog.len(),
                "instruction {at} jumps past the end"
            );
        }
        // The last instruction is a return: classic BPF must not fall off.
        assert_eq!(prog.last().expect("non-empty").code, RET_K);
    }

    /// **Non-vacuity for the assembler**: a table whose predicate list is long
    /// enough to overrun an 8-bit jump is refused rather than truncated.
    #[test]
    fn a_table_that_cannot_be_assembled_is_refused() {
        // 300 simple rows put the last of them more than 255 instructions from
        // the tail, which is exactly the overflow `JumpTooFar` names.
        static MANY: [Row; 300] = {
            let template = Row {
                name: "synthetic",
                nr: 0,
                class: EscapeClass::CompromisedShim,
                errno: libc::EPERM,
                errno_name: "EPERM",
                predicate: Predicate::Always,
                why: "",
                shadow: None,
            };
            [template; 300]
        };
        assert_eq!(compile(&MANY), Err(CompileError::JumpTooFar));
    }

    /// The negative-control list is not decoration: nothing on it may also be
    /// a denied row.
    #[test]
    fn nothing_is_both_denied_and_never_denied() {
        assert!(!NEVER_DENIED.is_empty());
        let denied: Vec<&str> = ROWS.iter().map(|r| r.name).collect();
        for (name, why) in NEVER_DENIED {
            assert!(
                !why.is_empty(),
                "{name} is a negative control with no reason"
            );
            assert!(
                !denied.contains(name),
                "{name} is both a denied row and a standing negative control"
            );
        }
    }

    /// One measurement inside a forked child that has installed the shipped
    /// program: `(rc, errno)` for one closure.
    ///
    /// The child is **allocation-free** -- [`compile`] runs in the parent and
    /// [`install`] allocates nothing -- because the cargo test harness is
    /// multi-threaded and a forked child inherits locks it can never release.
    /// The same discipline `landlock.rs`'s tests hold, for the same reason.
    /// A filter is irreversible, so an in-process install would confine the
    /// whole test binary and every test after it.
    fn under_the_filter(f: extern "C" fn() -> libc::c_long) -> (libc::c_long, i32) {
        let prog = compile(ROWS).expect("the shipped table compiles");
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(
            unsafe { libc::pipe(fds.as_mut_ptr()) },
            0,
            "the harness could not make a pipe"
        );
        // SAFETY: the child calls only async-signal-safe primitives.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                libc::close(fds[0]);
                // NO_NEW_PRIVS is the helper's K10; a test process has to set
                // it itself before an unprivileged `seccomp(2)` is legal.
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                    libc::_exit(90);
                }
                if install(&prog).is_err() {
                    libc::_exit(91);
                }
                let rc = f();
                let err = if rc < 0 { *libc::__errno_location() } else { 0 };
                let msg = [rc, libc::c_long::from(err)];
                libc::write(fds[1], msg.as_ptr().cast(), std::mem::size_of_val(&msg));
                libc::_exit(0);
            }
        }
        unsafe { libc::close(fds[1]) };
        let mut msg = [0 as libc::c_long; 2];
        let got = unsafe {
            libc::read(
                fds[0],
                msg.as_mut_ptr().cast::<libc::c_void>(),
                std::mem::size_of_val(&msg),
            )
        };
        unsafe { libc::close(fds[0]) };
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(
            got as usize,
            std::mem::size_of_val(&msg),
            "the child died before reporting (status {status}); 90 means PR_SET_NO_NEW_PRIVS \
             failed and 91 means the kernel refused the program itself"
        );
        (msg[0], msg[1] as i32)
    }

    extern "C" fn call_keyctl() -> libc::c_long {
        // KEYCTL_GET_KEYRING_ID on the process keyring, creating it. Succeeds
        // unfiltered and leaves nothing behind: a process keyring dies with
        // the process.
        unsafe { libc::syscall(libc::SYS_keyctl, 0, -2, 1) }
    }
    extern "C" fn call_add_key() -> libc::c_long {
        // `KEY_SPEC_PROCESS_KEYRING` (-2), never the SESSION keyring the row's
        // `why` describes: a key on the process keyring dies with the process,
        // and this probe runs in the ordinary test binary rather than in a
        // realm. The row's real finding was measured against the operator's
        // session keyring once, by hand; re-taking it on every `cargo test`
        // would leave keys on the developer's own session.
        unsafe {
            libc::syscall(
                libc::SYS_add_key,
                c"user".as_ptr(),
                c"vitrin-seccomp-unit-test".as_ptr(),
                c"x".as_ptr(),
                1usize,
                -2,
            )
        }
    }
    extern "C" fn call_request_key() -> libc::c_long {
        // A description nothing can have instantiated. Unfiltered this
        // SEARCHES and answers `ENOKEY`; under the filter it answers `EPERM`
        // before searching, which is the behavioural change the row's `why`
        // names and the reason "not zero" would be no assertion at all here.
        unsafe {
            libc::syscall(
                libc::SYS_request_key,
                c"user".as_ptr(),
                c"vitrin-seccomp-absent-key".as_ptr(),
                std::ptr::null::<libc::c_char>(),
                -2,
            )
        }
    }
    extern "C" fn call_io_uring_setup() -> libc::c_long {
        let params = [0u8; 120];
        unsafe { libc::syscall(libc::SYS_io_uring_setup, 1, params.as_ptr()) }
    }
    extern "C" fn call_socket_vsock() -> libc::c_long {
        unsafe { libc::syscall(libc::SYS_socket, 40, libc::SOCK_STREAM, 0) }
    }
    extern "C" fn call_ptrace() -> libc::c_long {
        // `PTRACE_ATTACH` against pid -1, and NOT `PTRACE_TRACEME`, which is
        // the request the row's `why` discusses. TRACEME would make this child
        // a tracee of the test binary if it ever ran unfiltered, and a traced
        // child changes what its parent's `waitpid` returns -- so the one
        // probe in this file that could hang the harness is the one that is
        // deliberately not used. Attaching to a pid that cannot exist has no
        // effect in either direction: `EPERM` under the filter, `ESRCH`
        // without it.
        unsafe { libc::syscall(libc::SYS_ptrace, libc::PTRACE_ATTACH, -1, 0, 0) }
    }
    extern "C" fn call_perf_event_open() -> libc::c_long {
        // A zeroed `struct perf_event_attr`. The filter denies on the syscall
        // number, so the kernel never reads it; unfiltered this is `EINVAL`
        // (size 0), which is why the demonstration that this row is *not*
        // shadowed on a `perf_event_paranoid=2` box lives in the integration
        // gate with a real attribute rather than here.
        let attr = [0u8; 128];
        unsafe {
            libc::syscall(
                libc::SYS_perf_event_open,
                attr.as_ptr(),
                0,
                -1,
                -1,
                0 as libc::c_ulong,
            )
        }
    }
    extern "C" fn call_bpf() -> libc::c_long {
        // `BPF_MAP_CREATE` with a zeroed union. Denied on the number under the
        // filter; `EINVAL` without it, since map type 0 is unspecified.
        let attr = [0u8; 128];
        unsafe { libc::syscall(libc::SYS_bpf, 0, attr.as_ptr(), 128usize) }
    }
    extern "C" fn call_userfaultfd() -> libc::c_long {
        // Returns a descriptor if it ever ran unfiltered; the child `_exit`s
        // immediately after reporting, so there is nothing to close.
        unsafe { libc::syscall(libc::SYS_userfaultfd, 0) }
    }
    extern "C" fn call_mbind() -> libc::c_long {
        unsafe {
            libc::syscall(
                libc::SYS_mbind,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0,
                std::ptr::null::<libc::c_ulong>(),
                0 as libc::c_ulong,
                0 as libc::c_uint,
            )
        }
    }
    extern "C" fn call_move_pages() -> libc::c_long {
        unsafe {
            libc::syscall(
                libc::SYS_move_pages,
                0,
                0 as libc::c_ulong,
                std::ptr::null::<*mut libc::c_void>(),
                std::ptr::null::<libc::c_int>(),
                std::ptr::null_mut::<libc::c_int>(),
                0 as libc::c_int,
            )
        }
    }
    extern "C" fn call_socket_unix() -> libc::c_long {
        unsafe { libc::syscall(libc::SYS_socket, libc::AF_UNIX, libc::SOCK_STREAM, 0) }
    }
    extern "C" fn call_socket_netlink() -> libc::c_long {
        unsafe { libc::syscall(libc::SYS_socket, libc::AF_NETLINK, libc::SOCK_RAW, 0) }
    }
    extern "C" fn call_personality_norandomize() -> libc::c_long {
        unsafe { libc::syscall(libc::SYS_personality, 0x0004_0000) }
    }
    extern "C" fn call_personality_query() -> libc::c_long {
        unsafe { libc::syscall(libc::SYS_personality, 0xffff_ffff_u32 as libc::c_ulong) }
    }
    extern "C" fn call_get_mempolicy() -> libc::c_long {
        let mut mode: libc::c_int = 0;
        unsafe {
            libc::syscall(
                libc::SYS_get_mempolicy,
                &mut mode as *mut libc::c_int,
                std::ptr::null_mut::<libc::c_ulong>(),
                0,
                std::ptr::null_mut::<libc::c_void>(),
                0,
            )
        }
    }
    extern "C" fn call_set_mempolicy() -> libc::c_long {
        unsafe {
            libc::syscall(
                libc::SYS_set_mempolicy,
                0,
                std::ptr::null::<libc::c_ulong>(),
                0,
            )
        }
    }
    extern "C" fn call_fork_and_exit() -> libc::c_long {
        // `fork(2)` is glibc's `clone`, and neither `clone` nor `clone3` is a
        // row. If this ever returns -1 the table has grown a row that breaks
        // every process the realm creates.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            unsafe { libc::_exit(0) };
        }
        if pid > 0 {
            let mut status = 0;
            unsafe { libc::waitpid(pid, &mut status, 0) };
            return 0;
        }
        -1
    }

    /// **One pinned expectation per row of [`ROWS`], written as a literal.**
    ///
    /// # Why a second copy of the errno, when the table already states it
    ///
    /// Because everything else that reads an errno reads it *from the table*,
    /// and a check whose expectation comes from the thing under test cannot
    /// see that thing change. [`compile`] takes each row's `errno` and emits
    /// `SECCOMP_RET_ERRNO` for it; `render_table` prints the same field;
    /// `tests/integration/test_real_seccomp.py` parses that print-out and
    /// asserts the realm returned it. Change `ptrace` from `EPERM` to `ENOSYS`
    /// in [`ROWS`] and every one of those moves with it -- the filter returns
    /// `ENOSYS`, the printed table says `ENOSYS`, the integration gate compares
    /// `ENOSYS` to `ENOSYS`, and the whole ladder stays green over a
    /// **behavioural** change: a library that probes `ptrace` and falls back on
    /// `ENOSYS` now silently proceeds where it used to abort, and one that
    /// treats `EPERM` as fatal no longer sees it.
    ///
    /// The table's own doc comment says errno is "part of the contract". This
    /// array is what makes that sentence enforceable: it is the only place in
    /// the repository where a row's errno is written down independently of the
    /// row, so changing a row's errno is exactly a two-file edit and a re-read.
    /// [`the_errno_of_every_row_is_pinned_and_measured`] holds it to [`ROWS`]
    /// in **both** directions, so a new row cannot be added without an entry
    /// here and an entry cannot outlive its row.
    ///
    /// The fourth field is the probe. Every one of them runs **only under the
    /// installed filter** (see [`under_the_filter`]), so each is denied before
    /// the kernel reads any argument -- which is why several pass pointers to
    /// zeroed structures that would be rejected on their own merits. What a
    /// row does *without* the filter is the integration gate's positive
    /// control and is deliberately not measured here.
    #[allow(clippy::type_complexity)]
    const PINNED: &[(&str, i32, &str, extern "C" fn() -> libc::c_long)] = &[
        ("keyctl", libc::EPERM, "EPERM", call_keyctl),
        ("add_key", libc::EPERM, "EPERM", call_add_key),
        ("request_key", libc::EPERM, "EPERM", call_request_key),
        (
            "io_uring_setup",
            libc::ENOSYS,
            "ENOSYS",
            call_io_uring_setup,
        ),
        (
            "socket_family",
            libc::EAFNOSUPPORT,
            "EAFNOSUPPORT",
            call_socket_vsock,
        ),
        ("ptrace", libc::EPERM, "EPERM", call_ptrace),
        (
            "perf_event_open",
            libc::EPERM,
            "EPERM",
            call_perf_event_open,
        ),
        ("bpf", libc::EPERM, "EPERM", call_bpf),
        ("userfaultfd", libc::EPERM, "EPERM", call_userfaultfd),
        (
            "personality",
            libc::EPERM,
            "EPERM",
            call_personality_norandomize,
        ),
        ("mbind", libc::ENOSYS, "ENOSYS", call_mbind),
        ("set_mempolicy", libc::ENOSYS, "ENOSYS", call_set_mempolicy),
        ("move_pages", libc::ENOSYS, "ENOSYS", call_move_pages),
    ];

    /// **The errno contract, for every row, held to a literal and measured.**
    ///
    /// Two assertions per row and they fail on different mistakes:
    ///
    /// 1. the row's declared `errno` equals [`PINNED`]'s literal -- a *table*
    ///    edit, caught without running anything;
    /// 2. the errno the compiled program actually returns equals that same
    ///    literal -- an *assembler* bug, caught by issuing the syscall under
    ///    the installed filter.
    ///
    /// Before this existed, five of the thirteen rows were measured against a
    /// literal and the other eight against the table itself, so eight rows
    /// could have their errno changed with every gate in the repository green.
    #[test]
    fn the_errno_of_every_row_is_pinned_and_measured() {
        // Both directions of the correspondence, first, so a row added without
        // a pin fails here rather than going unmeasured -- and a pin whose row
        // was deleted fails too, instead of quietly testing nothing.
        let pinned: Vec<&str> = PINNED.iter().map(|(name, ..)| *name).collect();
        let rows: Vec<&str> = ROWS.iter().map(|r| r.name).collect();
        for name in &rows {
            assert!(
                pinned.contains(name),
                "row `{name}` has no entry in PINNED. Add one -- with the errno written as a \
                 LITERAL, not read from the row -- or the row's errno can be changed with every \
                 check in this repository green, and `EPERM` versus `ENOSYS` is behavioural: \
                 libraries fall back on ENOSYS and abort on EPERM"
            );
        }
        for name in &pinned {
            assert!(
                rows.contains(name),
                "PINNED names `{name}` and the table has no such row, so that entry measures \
                 nothing"
            );
        }
        assert_eq!(pinned.len(), ROWS.len(), "PINNED and ROWS differ in size");

        for (name, want, want_name, call) in PINNED {
            let row = ROWS
                .iter()
                .find(|r| r.name == *name)
                .expect("checked above");
            // (1) The table, against the literal.
            assert_eq!(
                row.errno, *want,
                "row `{name}` declares errno {} ({}) and this test pins {want} ({want_name}). \
                 Errno is part of this table's contract, so changing one is a deliberate \
                 two-file edit: change it here too, and re-read the row's `why` -- a caller that \
                 branches on ENOSYS and aborts on EPERM sees a different program",
                row.errno, row.errno_name
            );
            // (2) The compiled filter, against the same literal.
            let (rc, err) = under_the_filter(*call);
            assert_eq!(rc, -1, "row `{name}` SUCCEEDED under the filter");
            assert_eq!(
                err, *want,
                "row `{name}` returned errno {err} under the installed filter, not the {want} \
                 ({want_name}) its row promises. The table and the program it compiles to \
                 disagree, which is an assembler bug rather than a policy one"
            );
        }
    }

    /// **The filter, measured rather than described.** The program the shipped
    /// table compiles to is installed in a child, and the syscalls the table
    /// promises to leave alone are issued against it.
    ///
    /// The denied half moved to
    /// [`the_errno_of_every_row_is_pinned_and_measured`], which covers all
    /// thirteen rows; what stays here is the non-vacuity half, which is the
    /// one that would not survive being folded into a table-driven loop.
    ///
    /// This is a *component* test and never the milestone evidence: it runs in
    /// this process's own namespaces, so it says what the BPF does and nothing
    /// about a realm. `tests/integration/test_real_seccomp.py` is the gate
    /// that measures the shipped helper, with a positive control per row.
    #[test]
    fn the_compiled_filter_denies_the_table_and_nothing_else() {
        // **Non-vacuity, and it is the half that matters.** Without these,
        // `the_errno_of_every_row_is_pinned_and_measured` would pass for a
        // filter that denied EVERYTHING with the right errnos -- which is
        // exactly what a mis-assembled jump would produce, and is the shape
        // no per-row assertion can see from inside its own row.
        for (name, call) in [
            (
                "socket(AF_UNIX)",
                call_socket_unix as extern "C" fn() -> libc::c_long,
            ),
            ("socket(AF_NETLINK)", call_socket_netlink),
            ("personality(0xffffffff)", call_personality_query),
            ("get_mempolicy", call_get_mempolicy),
            ("fork", call_fork_and_exit),
        ] {
            let (rc, err) = under_the_filter(call);
            assert!(
                rc >= 0,
                "{name} failed with errno {err} under the filter. It is a standing negative \
                 control: a filter that denies it is a filter that breaks the acceptance apps"
            );
        }
    }

    /// The published number and the table it describes.
    ///
    /// Non-vacuity is structural here: the assertion compares a literal to a
    /// `len()`, so it can only pass when they agree. What it protects is six
    /// published pages, which `cargo xtask limits-check` holds to
    /// [`ROW_COUNT`] and which nothing holds to [`ROWS`].
    #[test]
    fn the_published_row_count_is_the_tables_own() {
        assert_eq!(
            ROWS.len(),
            ROW_COUNT,
            "ROW_COUNT is what six published surfaces are held to by `cargo xtask \
             limits-check`, and the table now has a different number of rows. Change the \
             constant and re-read every page: `docs/book/src/limits.md`, `README.md`, \
             `SECURITY.md`, `site/index.html`, `examples/realm.toml` and \
             `docs/plan/02-phase-2-semantic-epochs.md` all state the size of this deny-list, \
             because the size is half of what `deny-list` means"
        );
    }

    #[test]
    fn the_printed_table_carries_every_row() {
        let rendered = render_table();
        assert!(rendered.starts_with("vitrin-seccomp 1\n"));
        for row in ROWS {
            assert!(
                rendered.contains(&format!("row name={} ", row.name)),
                "{} is missing from --print-seccomp",
                row.name
            );
            assert!(rendered.contains(row.errno_name));
        }
        for (name, _) in NEVER_DENIED {
            assert!(rendered.contains(&format!("never-denied name={name}")));
        }
        assert!(rendered.contains(&format!("table.rows={}", ROWS.len())));
    }
}
