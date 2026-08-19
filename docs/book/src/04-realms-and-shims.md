# Realms and shims

Grants control what a principal may *do*. Realms control what an application
can *see*. The two are independent, and the second one is structural.

## The idea

A legacy application never talks to the trusted core. It talks to its own
private Wayland compositor — a **shim** — which is itself an unprivileged
client of the core.

```
        ┌──────────────────────────────────────────┐
        │  vitrind — the trusted core              │
        │  capability kernel · grant store         │
        │  compositor · input router · consent     │
        └──────────────────────────────────────────┘
             ▲                          ▲
   frames up │ input down     frames up │ input down
   (dmabuf/  │ (origin-       (dmabuf/  │ (origin-
    shm fd)  │  tagged)        shm fd)  │  tagged)
             ▼                          ▼
    ┌─────────────────┐        ┌─────────────────┐
    │ realm-0         │        │ realm-1         │
    │  ┌───────────┐  │        │  ┌───────────┐  │
    │  │ vitrin-   │  │        │  │ vitrin-   │  │
    │  │ shim      │  │        │  │ shim      │  │
    │  └───────────┘  │        │  └───────────┘  │
    │        ▲        │        │        ▲        │
    │  WAYLAND_DISPLAY│        │  WAYLAND_DISPLAY│
    │        │        │        │        │        │
    │  ┌───────────┐  │        │  ┌───────────┐  │
    │  │ Firefox   │  │        │  │ a terminal│  │
    │  └───────────┘  │        │  └───────────┘  │
    └─────────────────┘        └─────────────────┘
```

Firefox in `realm-0` cannot enumerate the terminal in `realm-1`, cannot see
its surfaces, and cannot receive its input — not because a policy forbids
it, but because **its entire Wayland universe is a compositor that contains
only itself**. There is nothing to enumerate. Scoping is structural.

This is the gamescope and Qubes precedent, applied per-application rather
than per-session.

## Why the shim is a separate process in C

The shim is a real [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots)
compositor, written in C, built with Meson, deliberately outside the Cargo
workspace. That looks like an odd choice in a Rust project until you see
what it buys:

**Legacy complexity is exiled from the TCB.** Serving the full Wayland
protocol surface — `xdg-shell`, subsurfaces, buffer management, all the
quirks real toolkits rely on — is a large, messy job. None of it belongs in
a process that also holds the grant table. The shim absorbs that
complexity while being **untrusted**: the core assumes nothing about its
behaviour.

**It is disposable.** One shim per application. It crashes, that app dies,
and nothing else notices.

**Patching it costs you nothing.** The trademark policy makes this explicit:
modifying the trusted core means renaming your build, but `shim/` sits
outside the TCB, so patching a shim or writing a whole new one does not
change what the core enforces and does not cost you the name.

## What confines a realm today

Be precise here, because the honest answer is short.

When the core launches a realm's app it:

- forks a per-app shim and hands it **one end of a socketpair** as its
  identity — no credential, no handshake; holding the descriptor *is* being
  that realm's shim;
- gives it a private `0700` runtime directory;
- builds its environment **from nothing** — only names the operator
  allow-listed in `realm.toml`, plus a `WAYLAND_DISPLAY` pointing at that
  realm's own socket. `DISPLAY`, the host `WAYLAND_DISPLAY`,
  `WAYLAND_SOCKET`, `XAUTHORITY` and the host `XDG_RUNTIME_DIR` cannot reach
  the app;
- lets **no** unrelated descriptor cross the fork — not the agent listener,
  not the flight-recorder log, not other realms' sockets, not capture
  memfds — via a `close_range` sweep between `fork` and `execve`;
- resets signal dispositions, so the child does not inherit whatever the
  operator's shell was ignoring.

Those last two are enforced by the fork itself rather than by every other
module remembering to be careful. The full path is documented in
[`crates/vitrin-core/src/spawn.rs`](https://github.com/vitrin-os/vitrin-os/blob/main/crates/vitrin-core/src/spawn.rs).

**That is the complete list.** Read the next section before drawing
conclusions from it.

One prerequisite, because it decides whether any of the above happens at all:
the namespace set is built from an **unprivileged** `CLONE_NEWUSER`, so the
host has to let such a namespace carry its capabilities. Where a host permits
the `unshare` and then strips them, `vitrind --isolation=default` refuses to
start rather than running a weaker session — see
[the limits page](limits.md) for the requirement, the one measurement behind
it, and what is tracked to make the grant routine.

Since P2.6.3 there is a **second** such prerequisite, and it is a different
condition with a different remedy: the kernel must actually have Landlock —
**≥ 5.13**, built with `CONFIG_SECURITY_LANDLOCK=y`, and with `landlock` in the
active LSM list (`/sys/kernel/security/lsm`) — **and, since 2026-08-15, an ABI
at or above this build's declared floor** (`build.landlock_min_abi` from
`vitrind --print-floor`, **6** here). The ruleset below is part of the
confinement floor, so without all four the core refuses to start rather than
confining a realm one mechanism less than its own journal claims. The fourth is
the one a correctly configured kernel can still fail, and its remedy is a newer
kernel rather than any knob. The refusal
names the mechanism it could not get — `namespaces` for the paragraph above,
`landlock` for this one — and that word is the diagnosis: the two remedies do
not substitute for each other. `vitrind --print-isolation` answers both,
without spawning anything.

## What does *not* confine a realm

> **The sandbox is half-built.** Decisions D9, D-020, D-036. The shim and its
> app run in six namespaces with an identity uid/gid map, zero capabilities, a
> private mount table and — since P2.6.3 — a **Landlock ruleset** enforced
> before the shim's `execve`, whose read set is enumerated rather than granted
> at the realm root and whose write set reaches eight hierarchies (the four
> writable mounts in full, plus `WRITE_FILE` alone on `/proc`, `/dev`,
> `/dev/pts` and each render node — eight with one render node bound, one more
> for each additional one). What that ruleset requires of a kernel is published
> as a generated, CI-held table — [the Landlock ABI matrix](isolation-matrix.md)
> — and P2.6.3 closed on 2026-08-19 on **corrected** criteria rather than the
> ones it was written with, so read the closure narrowly: that table measures no
> kernel, the
> per-kernel one its criteria ask for exists on a page of its own —
> [which kernels this build starts on](isolation-kernels.md), five distribution
> kernels booted under QEMU with the shipped `vitrind`, two admitted and three
> refused `below-floor` — but every row there is a **kernel** reading taken in a
> bare initramfs and not a statement about the distribution that ships that
> kernel, so the number of *distributions* measured as such is still one, and
> the ABI floor narrowed
> the task rather than closing it.
> Since P2.6.4 there is also a **seccomp deny-list**, installed immediately
> before the shim's `execve` and inherited by every process the shim forks: it
> closes the 13 rows `vitrind --print-seccomp` prints, each naming the escape
> class it answers and the errno it returns, and leaves the rest of the
> kernel's syscall surface unenumerated. So the realm is
> filesystem-confined and filtered against a named list and not
> syscall-confined. At `--isolation=off` none of it
> applies and the paragraph below holds in full; `--landlock=off` turns off the
> ruleset alone, and both say so in every journal entry. **There is no
> `--seccomp=off`**: a kernel that cannot accept a filter refuses the session
> instead of running one unfiltered.

An application that ignores `WAYLAND_DISPLAY` and connects directly to a
path it already knows is not stopped by anything in this MVP.

**And an app's *own* sandbox no longer confines anything here.** A Landlock
domain denies every mount-topology change to a realm's app and its
descendants, unconditionally — mounting is not an access right, so no rule
grants it and widening the ruleset cannot restore it. A nested sandbox
therefore cannot be built inside a realm, and an app that decodes images in
one (GTK → `glycin` → `bwrap`) decodes them **unsandboxed** instead. A realm
additionally refuses nested user namespaces outright, which takes no
capability away — a namespace that cannot mount was already useless — and
turns that into the conventional refusal such libraries already handle rather
than an unexpected `mount(2)` failure. The measurement, and what it costs, are
on [the limits page](limits.md).

Two further specifics worth naming rather than leaving to be discovered:

**The session D-Bus is reachable at `--isolation=off`, and closed twice over at
`--isolation=default`.** The core advertises no `DBUS_SESSION_BUS_ADDRESS` and
redirects `XDG_RUNTIME_DIR` either way, so a well-behaved client finds no bus —
but advertisement is not reachability, and at `off` that is the whole of it:
`/run/user/<uid>/bus` is still on the filesystem, still connectable by any
process of that uid, and the abstract-socket namespace is still shared. Since
P2.6.2 the default closes both halves, and neither closure is this project's
cleverness: the mount namespace removes `/run/user/<uid>/bus` as a path — the
realm's `/run` holds one entry, `vitrin` — and the network namespace removes the
abstract-socket namespace the bus also listens on, because abstract sockets are
scoped to a network namespace. So an operator who allow-lists
`DBUS_SESSION_BUS_ADDRESS` in `realm.toml` at `off` turns an implicit hole into
an audited one, and the same line at `default` names something that is not
there. That closure is not the same claim as a measurement, though, and the
distinction is worth its own sentence: it is *derived from the mount table
rather than measured* — **no test asserts the absence of `/run/user`**, and
`tests/integration/test_real_confinement.py` lists "that a realm cannot reach
the session bus by other means" among the things it explicitly does *not*
prove. The adversarial probe that would attempt `org.a11y.Bus` activation on
every bus reachable from inside a realm has still not been written. Two
residuals survive that, and both are narrower than what closed:
`binds` names any absolute path outside `/` and `/home`, so an operator who
binds the host's runtime directory into a realm puts the bus socket back inside
it under a key that says nothing about buses; and the *designated-egress* half
of the network answer — reachability as a granted, host:port-scoped capability
rather than as nothing at all — is still P13's, unbuilt.

**Same-uid separation is not attempted.** The `0700` runtime directory bounds
other *users* on the machine, not other processes of this user, and the app runs
as the core's uid in either isolation mode. What the realm's `XDG_RUNTIME_DIR`
*names* stopped being the same thing at P2.6.2, though. At `--isolation=off` it
is `$XDG_RUNTIME_DIR/vitrin-0/<realm>`, one level below the directory holding the
core's own agent socket and the run's flight-recorder log — it names the control
plane as much as it hides it, and relocating the tree would not help, since a
child of the core's uid derives `/run/user/<uid>` from `getuid()` with or without
a variable pointing at it. At `--isolation=default` the value is the fixed
in-realm `/run/vitrin`, a bind of that same core-created directory, and `..`
resolves to the realm's own `/run`, where there is no `core.sock` and no recorder
log. The closure is the mount namespace's rather than the path's, and it is
checked rather than argued: both are canaries every confined spawn probes through
`/proc/<shim>/root`.

Environment hygiene confines the well-behaved. It does not contain the
hostile. Real sandboxing arrives with the Phase-2 powerbox (E2.6/E2.7).

## Configuring a realm

`realm.toml` names what a realm runs and what environment names may reach
it:

```toml
[[realm]]
id = "realm-0"
command = "/usr/bin/firefox-esr"
args = ["--no-remote", "--new-window", "about:blank"]
env_allow = [
    "HOME", "LANG", "XDG_SESSION_TYPE",
    "MOZ_ENABLE_WAYLAND", "GDK_BACKEND",
    "DBUS_SESSION_BUS_ADDRESS",   # see above -- an audited hole at --isolation=off
]
```

`env_allow` is an allow-list of **names**, and values are copied from
`vitrind`'s own environment. That is the only route by which a realm's
environment grows. [`examples/realm.toml`](https://github.com/vitrin-os/vitrin-os/blob/main/examples/realm.toml)
carries the security rules inline.

**A second realm is a second `[[realm]]` table**, up to 16, and one of them
must be `realm-0` — the one realm name a client can know without being told,
since there is still no way to enumerate realms on the wire. Each gets its
own shim, its own private runtime tree and its own socket, exactly as the
diagram above shows. Ids are otherwise free-form, with one refusal worth
knowing: a realm's lock file sits *beside* its directory as `<id>.lock`, so
a realm named `foo.lock` would collide with realm `foo`, and startup refuses
that naming both.

What a second realm does get is its own scene and its own capture: an
`observe` grant returns the pixels of the realm it names, hidden or not, and
a grant over a realm whose app has died refuses `no_surface` regardless of
what its siblings are doing. What it does *not* get is its own **output**:
the core composites one output from one realm's scene, so with several
realms running only the realm the output is bound to is visible — the first
one to attach. Which realm that is **is now somebody's to choose**: a client
holding the `layout.focus` verb moves the output, and the human's own keyboard
and pointer move with it — one act, because showing a realm and typing into it
must never come apart. Absent such a client the binding moves on the one event
nobody chooses: the bound realm's app exiting, after which the output follows to
the first realm still serving, and to no realm at all once none is. Every other
realm still
renders, and pays for it. Read [Known limits](limits.md) before configuring
more than one.

`--no-remote` in that example is load-bearing, not hygiene: without it, a
`firefox --new-window` on a machine already running Firefox hands the window
to the *existing* instance over its remoting protocol — never touching the
confined process at all, silently defeating the entire arrangement.

## The buffer path

Frames move shim→core as file descriptors over `SCM_RIGHTS`. Two paths:

- **shm** — universal, always available, one copy. CI runs entirely on it.
- **dmabuf** — zero-copy on a real GPU. Version 0 imports exactly
  `xrgb8888`/`argb8888` with the linear modifier implied; the allow-list is
  checked before any driver call. Failure produces an explicit
  `buffer_done(import_failed)` telling the shim to fall back to shm — never
  a silent black frame.

MVP success does not depend on zero-copy working, which is why CI can stay
GPU-free.

Next: [The wire protocol](05-the-wire-protocol.md).
