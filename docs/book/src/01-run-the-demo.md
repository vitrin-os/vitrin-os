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

Still stuck? Open an issue with the `flight.jsonl` attached; it is usually
the single most useful artifact.

Next: [Your first agent](02-your-first-agent.md).
