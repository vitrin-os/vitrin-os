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

## What does *not* confine a realm

> **The sandbox is half-built.** Decisions D9, D-020, D-036. The shim and its
> app run in six namespaces with an identity uid/gid map, zero capabilities and
> a private mount table — but with **no seccomp filter and no Landlock
> ruleset**, so the realm is path-confined and not syscall-confined. At
> `--isolation=off` none of it applies and the paragraph below holds in full.

An application that ignores `WAYLAND_DISPLAY` and connects directly to a
path it already knows is not stopped by anything in this MVP.

Two specifics worth naming rather than leaving to be discovered:

**The session D-Bus is reachable.** The core advertises no
`DBUS_SESSION_BUS_ADDRESS` and redirects `XDG_RUNTIME_DIR`, so a
well-behaved client finds no bus. But advertisement is not reachability:
`/run/user/<uid>/bus` is still on the filesystem, still connectable by any
process of that uid, and the abstract-socket namespace is still shared. In
practice, running Firefox means allow-listing `DBUS_SESSION_BUS_ADDRESS`
explicitly — which at least turns an implicit hole into an audited one.
Closing it properly (P13, Phase 2) means a loopback-only network namespace
and an empty mount namespace, so there is nothing to reach rather than
nothing advertised.

**Same-uid separation is not attempted.** The `0700` runtime directory
bounds other *users* on the machine, not other processes of this user. Note
what the realm's `XDG_RUNTIME_DIR` therefore is: `$XDG_RUNTIME_DIR/vitrin-0/<realm>`
sits one level below the directory holding the core's own agent socket and
the run's flight-recorder log. It names the control plane as much as it
hides it. Under D9 the app runs as the core's uid and can derive those paths
with or without a variable pointing at them.

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
    "DBUS_SESSION_BUS_ADDRESS",   # see above -- an audited hole
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
one to attach. Nothing *chooses* to move that binding, because who may choose
is a separate, unlanded authority question; the one thing that moves it is the
bound realm's app exiting, after which the output follows to the first realm
still serving, and to no realm at all once none is. Every other realm still
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
