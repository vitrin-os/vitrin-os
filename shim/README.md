# `shim/` — Vitrin OS Wayland shim (C + wlroots)

The per-app **Wayland shim**: a tiny wlroots compositor that serves exactly
one legacy Wayland app over one private socket, so the app runs confined
outside the trusted core (`vitrind`). It uses the **headless** backend and
never touches real hardware — the core owns the screen. See
[`docs/PRD.md`](../docs/PRD.md) §17 (C+wlroots rationale) and
[`docs/plan/01-phase-1-mvp.md`](../docs/plan/01-phase-1-mvp.md) §3 E6.

This directory is **outside the Cargo workspace** (C, Meson-built). It consumes
the checked-in, generated C wire header
[`include/vitrin-protocol.h`](include/vitrin-protocol.h) (from the protocol
track, P1.1.2) — regenerate it with `cargo xtask codegen`, never by hand.

## Status — P1.6.1 (skeleton)

What exists today: a headless compositor that stands up the v0 Wayland
environment and runs a client blind. **Not yet**: the upstream link to the
core (buffer/damage forwarding, P1.6.2) or virtual-seat input replay
(P1.6.3).

Globals advertised in v0 (a contract, not a floor — additions are driven
empirically by the future "globals touched" log, P1.6.4):

| Global | Source |
|---|---|
| `wl_compositor` | `wlr_compositor_create` |
| `wl_shm` | `wlr_renderer_init_wl_shm` |
| `xdg_wm_base` | `wlr_xdg_shell_create` |
| `wl_seat` | `wlr_seat_create` |
| `wl_output` | `wlr_output_create_global` |
| `zxdg_decoration_manager_v1` | `wlr_xdg_decoration_manager_v1_create` (declines SSD) |
| `zwp_linux_dmabuf_v1` | `wlr_linux_dmabuf_v1_create_with_renderer` — **only with `--dmabuf`** |

Server-side decorations are declined (clients always draw their own). Per
**D3**, shm is the mandatory buffer path; `linux-dmabuf-v1` is opt-in.

## Building

**D11:** wlroots is pinned to **0.19.3** and vendored as a Meson subproject
(`subprojects/wlroots.wrap`); `wayland-protocols` likewise. The build is
system-first — an installed `wlroots-0.19` is used when present and the wrap
stays dormant; otherwise (e.g. CI) the pinned checkout is built from source.
The wrap is the source of truth for the version; budget one wlroots upgrade
task per phase.

```bash
meson setup build            # uses system wlroots-0.19 if available
ninja -C build
meson test -C build          # header-compiles

# Build the vendored wlroots from source (e.g. CI, or no system wlroots-0.19),
# taking wlroots' own dependencies from the system:
meson setup build --force-fallback-for=wlroots-0.19,wlroots
```

The build needs no Rust toolchain — only the checked-in generated header.

## Running

```bash
./build/vitrin-shim [--socket NAME] [--dmabuf] [--width W] [--height H]
```

The socket name resolves as `--socket` > `$WAYLAND_DISPLAY` > `vitrin-shim-0`,
created under `$XDG_RUNTIME_DIR`. Force pure-software headless operation with
`WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_RENDERER_ALLOW_SOFTWARE=1`.

## Acceptance test

[`tests/acceptance/shim_globals_and_client.sh`](tests/acceptance/shim_globals_and_client.sh)
proves the two P1.6.1 criteria: `wayland-info` lists exactly the expected
globals, and `weston-terminal` runs blind without crashing or hanging the
shim. It needs `wayland-info` (`wayland-utils`) and `weston-terminal`
(`weston`) installed.

```bash
SHIM_BIN=./build/vitrin-shim bash tests/acceptance/shim_globals_and_client.sh
```
