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

## Status — P1.6.2 (upstream link)

What exists today: a headless compositor that stands up the v0 Wayland
environment, composites its app's window, and **forwards every composed
frame up to the trusted core** over the socketpair it inherited at fork —
`attach` → `damage` (per rectangle) → `commit` — relaying the core's
`frame_done` back to the app as its `wl_surface.frame` callbacks. **Not
yet**: virtual-seat input replay (P1.6.3; the seat object is minted at
startup and its events are logged and dropped until then).

### The upstream link, in one pass

| Step | Where |
|---|---|
| Adopt fd 3, re-arm `FD_CLOEXEC`, go non-blocking | `src/wire.c` |
| One synchronous read of `configure` (realm + view geometry) | `src/upstream.c` |
| `create_surface` (id 2) and `get_seat` (id 3) | `src/upstream.c` |
| Bind the private Wayland socket — only now | `src/main.c` |
| App window → scene, single-maximized at the view size | `src/xdg.c` |
| Composite → copy into a pooled memfd → `attach`/`damage`/`commit` | `src/output.c`, `src/upstream.c` |
| `frame_done` → the app's frame callbacks; `buffer_done` → recycle | `src/upstream.c` |

**Identity is the descriptor.** The shim presents no credential and performs
no handshake: holding fd 3 *is* being that realm's shim. It therefore
**refuses to start without it** — pass `--no-upstream` for standalone
development, which is what the P1.6.1 globals test uses.

Two decisions this task settled (issue #34 listed both as open):

- **Damage granularity — per rectangle, verbatim.** The compositor's own
  damage region travels one `damage` request per rectangle, never
  pre-coalesced to the full surface, so a small repaint is visible as one in
  the core's log (the IDL asks for exactly this). Past 32 rectangles the tail
  folds into a bounding box, the same over-approximation the core applies
  past its own cap.
- **memfd reuse from a bounded pool, not fresh-per-frame.** Two slots, sealed
  `F_SEAL_SHRINK | F_SEAL_GROW`, rewritten only after `buffer_done` returns
  ownership. Two rather than one because the dmabuf passthrough regime
  (P1.3.5) defers `released` until the *next* commit replaces the retained
  buffer, which a one-slot pool cannot construct; two rather than more
  because the frame path admits one frame at a time, so a third slot could
  only buffer frames nobody asked for.

**Backpressure** is applied *before* compositing: while a commit is awaiting
its `frame_done`, an output frame tick is skipped outright. Nothing is lost
(the scene's damage ring keeps the pixels for the next composite that runs)
and nothing is queued, so the shim's footprint is O(1) buffers no matter how
far behind the core falls. Measured: an app committing 900 frames while the
core presents 50 leaves the shim's descriptor count and RSS exactly flat.

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
./build/vitrin-shim [--socket NAME] [--dmabuf] [--no-upstream] [--width W] [--height H]
```

Normally the core runs it — there is nothing to run it against by hand,
since it exits unless fd 3 is a core connection. `--no-upstream` drops that
requirement for development; `--width`/`--height` matter only in that mode,
because with a core the `configure` geometry wins.

The socket name resolves as `--socket` > `$WAYLAND_DISPLAY` > `vitrin-shim-0`;
a relative name is created under `$XDG_RUNTIME_DIR` and an absolute one is
used as-is (which is what the core passes). Force pure-software headless
operation with `WLR_BACKENDS=headless WLR_RENDERER=pixman
WLR_RENDERER_ALLOW_SOFTWARE=1`.

## Acceptance tests

Both are GPU-free, headless, and need no Rust toolchain.

[`tests/acceptance/shim_globals_and_client.sh`](tests/acceptance/shim_globals_and_client.sh)
— the P1.6.1 criteria: `wayland-info` lists exactly the expected globals, and
`weston-terminal` runs without crashing or hanging the shim. Needs
`wayland-info` (`wayland-utils`) and `weston-terminal` (`weston`).

```bash
SHIM_BIN=./build/vitrin-shim bash tests/acceptance/shim_globals_and_client.sh
```

[`tests/acceptance/upstream_frame_path.sh`](tests/acceptance/upstream_frame_path.sh)
— the P1.6.2 criteria, asserted against the wire by
[`tests/mock_core.c`](tests/mock_core.c), a C reimplementation of the core's
side that spawns the shim exactly as the core does and applies the real
core's validation rules to everything it sends.
[`tests/damage_client.c`](tests/damage_client.c) supplies the app: known
32×32 repaints and a frame-callback counter, so "damage-only" and "paced by
the core" are measured, not eyeballed.

```bash
BUILD_DIR=./build bash tests/acceptance/upstream_frame_path.sh
```

### Conformance against the real core

The mock core can only prove the shim satisfies *a* reading of the spec. The
cross-check against the real `vitrind` shim server — its `F_GET_SEALS` probe,
its 128-bit geometry arithmetic, its watermark and `bad_order` rules — is a
Rust test, opt-in because `shim/` is outside the Cargo workspace:

```bash
meson compile -C shim/build
VITRIN_C_SHIM_BIN=$PWD/shim/build/vitrin-shim cargo test -p vitrin-core c_shim
```

Opt-in locally, mandatory in CI: the `conformance` job is the only one that
holds both toolchains, and it is where this runs. The test refuses to skip
itself silently under `CI` — a job that neither points `VITRIN_C_SHIM_BIN` at
a built shim nor sets `VITRIN_C_SHIM_CONFORMANCE_SKIP` fails rather than
reporting a green result it did not earn.
