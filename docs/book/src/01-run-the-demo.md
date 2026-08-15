# Run the demo in five minutes

At the end of this you will have watched an agent connect over a real Unix
socket, petition for a capability, capture a real application's pixels,
click into it, type into it, and prove the text landed — with the trusted
core mediating every step.

Nothing here is mocked. `cargo xtask demo` fails loudly rather than
substituting a stand-in.

## What you need

Linux, and:

```sh
# Debian/Ubuntu
sudo apt-get install -y libxkbcommon-dev libpixman-1-dev weston xmlstarlet

# Arch
sudo pacman -S --needed libxkbcommon pixman weston meson
```

`weston` is there for `weston-terminal`, the real application the headless
demo drives. The Rust toolchain pins itself — `rust-toolchain.toml` makes
rustup install the right version on your first `cargo` command.

## Build it

```sh
git clone https://github.com/vitrin-os/vitrin-os.git
cd vitrin-os

# The Rust side: vitrind, xtask, and the test fixtures.
cargo build --workspace

# The C side: the per-app wlroots shim. It lives outside the Cargo
# workspace by design and needs its own dependency step.
bash shim/ci/install-deps.sh
meson setup shim/build shim && meson compile -C shim/build
```

The shim is not optional. `cargo xtask demo` looks for it at
`shim/build/vitrin-shim` (or wherever `VITRIN_C_SHIM_BIN` points) and stops
with the exact `meson` command above if it is missing.

## Run it

```sh
cargo xtask demo --headless
```

Expect output ending in `xtask demo: PASS`, plus paths to the run's flight
recorder (`flight.jsonl`) and its captured frames.

## What just happened

```
cargo xtask demo --headless
   │
   ├─ boots vitrind --headless          the trusted core, software-rendered
   │    │
   │    ├─ fork/execs vitrin-shim       a real wlroots compositor, one per app,
   │    │     │                         with a scrubbed environment and its own
   │    │     │                         private runtime dir
   │    │     └─ fork/execs weston-terminal
   │    │           WAYLAND_DISPLAY points only at the shim's own socket, so
   │    │           the app's entire universe is that shim
   │    │
   │    └─ listens on a Unix socket for agent principals
   │
   └─ runs examples/agent-demo/run_demo.py
        connect → request_grant → await consent → settle → capture
        → click → type → capture → assert the typed text landed
```

The two captures are not compared naïvely. An earlier version of this gate
asked only for 24 changed pixels between them, which `weston-terminal`'s own
startup paint clears without any agent involvement — it passed whether or
not the click and keystrokes reached anything. It now settles the app,
watches it idle at least as long as it later polls, and demands a change
*shaped* like a typed line: enough changed pixels **and** a densely inked
run of them along one scanline.

That detail is in this getting-started page on purpose. It is the difference
between a demo and a test.

## Look at the evidence

The flight recorder journals every decision the core made:

```sh
# The path is printed at the end of the run.
jq -c 'select(.event | test("grant|consent|refus"))' /path/to/flight.jsonl
```

You will see the petition arrive, the consent decision resolve, and each
actuation checked at the chokepoint — with the grant it was checked against.

## The full integration suite

The demo is one test. To run every named milestone gate:

```sh
VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" bash tests/integration/run.sh
```

That drives the shipped `vitrind` binary against real applications —
`weston-terminal`, a GTK entry probe, and Firefox ESR — over a real socket.
[`tests/integration/README.md`](https://github.com/vitrin-os/vitrin-os/blob/main/tests/integration/README.md)
maps each test to the milestone it closes, and is explicit about which tests
are component tests that close nothing.

## Nested mode

Drop `--headless` and the core draws a real window on your own Wayland
session, with Firefox ESR in the realm:

```sh
cargo xtask demo
# If your Firefox is not at firefox-esr:
VITRIN_DEMO_FIREFOX=/usr/bin/firefox cargo xtask demo
```

This needs a running compositor (GNOME, Hyprland, …) and a browser
installed. It is never a CI dependency — nested mode has no headless
equivalent by design.

Nested mode is also the only way to experience the two properties the
headless run can only simulate: clicking **Allow** on a consent prompt the
core drew itself, and physically holding Escape to watch a live grant die
mid-actuation.

## If it fails

| Symptom | Cause |
|---|---|
| `vitrin-shim not found` | The `meson` step above did not run, or `VITRIN_C_SHIM_BIN` points somewhere stale. |
| Hangs before any capture | `weston-terminal` is not installed, so the realm has nothing to draw. |
| `AuthFailed` at connect | The demo identity is not in the `principals.toml` the core booted with. |
| Nested mode opens nothing | No Wayland session — check `echo $WAYLAND_DISPLAY`. |
| `vitrind` exits at startup naming `landlock` as the mechanism it could not get | This kernel has no usable Landlock, which since P2.6.3 is a startup requirement rather than an optimisation. Check three things, in order: `uname -r` ≥ 5.13, `zgrep CONFIG_SECURITY_LANDLOCK /proc/config.gz`, and `cat /sys/kernel/security/lsm` for `landlock` — a kernel can carry the code and leave it out of `lsm=`. `vitrind --print-isolation` prints all three as `landlock.abi=N`. A fourth condition is a build one, not a host misconfiguration: the number must be at or above `build.landlock_min_abi` from `vitrind --print-floor` (**7** here), and a working Landlock below it is refused as `below-floor(abi=N,required=M)` with a newer kernel as the only remedy. `--landlock=off` starts realms with **no ruleset at all** and is the wrong answer to a kernel that could be configured. **This is not the row below**: that one is about user namespaces and its remedy does nothing here — no sysctl makes a kernel report a Landlock ABI. The word the refusal prints (`landlock` vs `namespaces`) is the diagnosis. |
| A realm's log says `WARNING: Glycin running without sandbox.` | Expected, and it is a published cost rather than a bug. A Landlock domain denies **every** mount, so `glycin` cannot build the `bwrap` sandbox it decodes images in; a realm makes that refusal arrive early and legibly by refusing nested user namespaces (`/proc/sys/user/max_user_namespaces = 0` inside the realm), so `bwrap` fails at `unshare(CLONE_NEWUSER)` with a message `glycin` recognises and `glycin` takes the no-sandbox fallback it already ships. The decode then runs inside your realm with no second boundary around it. Read `landlock-breaks-nested-image-sandboxes` on the limits page before deciding whether that is acceptable for what you are opening. |
| A GTK app in a realm aborts on startup with `Gtk:ERROR:…gtkiconhelper.c:495` and `Loader process exited early with status '1'` | **This was the shipped behaviour until 2026-08-15 and should no longer happen**; if it does, your `glycin` does not recognise the refusal above. The abort is `glycin` concluding its `bwrap` sandbox is available, spawning a loader that then dies, and GTK treating the failed icon load as fatal. `glycin` classifies the availability probe by matching its stderr against a list of namespace-refusal strings; check the realm's log for what `bwrap` actually printed, and see `landlock-breaks-nested-image-sandboxes` on the limits page for the full measurement. Check whether this path applies to you at all with `ldd /usr/lib/libgdk_pixbuf-2.0.so.0 \| grep glycin` and `command -v bwrap`. |
| `vitrind` exits at startup naming an isolation mechanism it could not get | Your host does not let an unprivileged user namespace carry its capabilities, so the default confinement cannot be built and the core refuses rather than running unconfined. Run `vitrind --print-isolation` to see the same probe on its own. On the one machine where this was measured — a GitHub `ubuntu-latest` runner, kernel `6.17.0-1020-azure`, 2026-08-14 — the knob was `kernel.apparmor_restrict_unprivileged_userns`, set to `1`. That is one runner on one date, and **that machine's `/etc/os-release` was never opened**, so do not read it as "Ubuntu does this"; the refusal names the knobs *your* machine answered with. Packaging so this is arranged for you is [#286](https://github.com/vitrin-os/vitrin-os/issues/286), and the limits page states the requirement and the bound on that measurement. **If the mechanism it names is `landlock`, this is the wrong row** — see the row above, whose remedy is a kernel build and not a sysctl. |

Still stuck? Open an issue with the `flight.jsonl` attached; it is usually
the single most useful artifact.

Next: [Your first agent](02-your-first-agent.md).
