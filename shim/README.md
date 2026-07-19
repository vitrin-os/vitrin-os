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

## Status — P1.6.3 (virtual-seat input replay)

What exists today: a headless compositor that stands up the v0 Wayland
environment, composites its app's window, **forwards every composed frame up
to the trusted core** over the socketpair it inherited at fork (`attach` →
`damage` per rectangle → `commit`, with the core's `frame_done` relayed back
as the app's `wl_surface.frame` callbacks), and **replays the core's
origin-tagged `vitrin_shim_seat` events into the app through its own
`wl_seat`** — pointer motion, buttons, high-resolution scroll, keys, and
Unicode text. **Not yet**: popups are not in the scene graph (an
`xdg_shell.new_popup` listener is missing, so a menu is neither rendered nor
clickable — the input path already routes by hit-testing the scene, so it
needs no change when they land), and `text-input-v3` / IME is the Phase-2
workstream (E2.8) that retires the synthesized keymap below.

### Input replay, in one pass

| Step | Where |
|---|---|
| Seat event off the wire, routed by object id 3 | `src/upstream.c` |
| Decode + bind the origin tag (B2), dispatch | `src/seat.c` |
| Keysym → keycode in the dynamic keymap (D7) | `src/seat.c` |
| View → surface-local by scene hit test (D10) | `src/seat.c` |
| Pointer focus on first motion, keyboard focus at map | `src/seat.c`, `src/xdg.c` |
| One `wl_pointer.frame` per drained wire batch | `src/wire.c` → `src/seat.c` |

Four decisions this task settled (issue #35 listed the first two as open):

- **Keymap caching — three regions, and regeneration only for genuinely new
  codepoints.** The keycode space splits into a *modifier* region (fixed for
  the process's life, so a held chord survives every regeneration), a *warm*
  region (all printable ASCII plus every layout-invariant editing /
  navigation / function keysym, bound **once at startup, before the app can
  connect**), and a *dynamic ring* (bind-on-demand, FIFO, never evicting a
  keycode whose key is held). So the app's *first* keymap read already covers
  the whole human key path and all-ASCII agent text — which is what makes the
  known failure mode, an app that reads the keymap once and never re-reads
  it, degrade gracefully. Binding is **additive**: a keycode already in the
  app's copy never changes meaning until the ring wraps, so a stale reader
  loses characters it never had rather than typing the wrong ones. Measured:
  an all-ASCII run costs **zero** regenerations, and `héllo→世界` costs
  exactly **one**, then zero on every repeat.
- **Pointer batching — one `wl_pointer.frame` per drained wire batch.** A
  frame marks a group of pointer events that belong together, and the group
  the protocol cares about is the one the core routed from a single cause (a
  diagonal scroll is two `scroll` events for one wheel motion). The core
  emits those back-to-back on a `SOCK_STREAM` connection, so they normally
  land in one `recvmsg` and the transport's batch boundary *is* the grouping;
  `wire.c` reports the boundary, `seat.c` closes the frame there. A frame per
  event would tell the app those were unrelated.

  The grouping is a **best-effort refinement, never a correctness
  requirement**: the core issues one `sendmsg` per event and it is the kernel
  that coalesces them, so a scheduling accident can split a group. That costs
  precision and nothing else — more frames means a finer grouping, which is
  always protocol-legal, and a frame is closed after *every* drain, so a
  group is never left unterminated. Both halves are asserted (acceptance
  check **K**): the same two scrolls share one frame when they arrive
  together and take one each when they do not. `mock_core.c` writes
  consecutive script events in one socket write so "arrived together" is a
  fact in the test rather than a race with the scheduler.
- **What the generated keymap suppresses, and what `text` refuses to send.**
  The normative modifier-suppression rule ("modifiers already applied are
  never applied twice") needs **two** xkbcommon mechanisms neutralised, and
  the obvious fix only covers one. Every generated key carries a single key
  type that (a) maps **only Level1**, so no modifier can select a different
  level — that is the Shift case, the classic VNC double-shift bug — and (b)
  **consumes `Lock`**, because libxkbcommon capitalises the *resolved keysym*
  after level selection whenever Caps is effective and the key's type does
  not consume it. A `modifiers = none` type consumes nothing, so it does not
  suppress (b) at all; with it, a human tapping Caps Lock in the host window
  silently upper-cases every later agent payload — `hello` delivered as
  `HELLO`, the R5 gate string as `HÉLLO→世界`. `Lock` and nothing else is
  consumed: consuming `Control` would stop Ctrl+C producing U+0003, and
  consuming anything hides it from toolkit accelerator masks. Modifier keys
  keep their real `SetMods`/`LockMods` actions, so chords and locked state
  still reach the app. Both mechanisms are asserted separately (checks
  **E**), because passing the Shift case proves nothing about the Caps one.

  Relatedly, `actuate.text` is "deliver this Unicode string", never "press
  these keys": the IDL names exactly two control characters that become
  keystrokes (`\n` → Return, `\t` → Tab) and **every other one is dropped**.
  That cannot be delegated to xkbcommon — not one C0 codepoint comes back as
  `NoSymbol`; U+0008 is `BackSpace`, U+001B is `Escape`, U+007F is `Delete`.
  A stray byte in a scraped or pasted payload must not erase the user's text
  or dismiss their dialog. Refusals are counted (`unmappable=`) and the
  delivery is traced as `partial` (check **I**).
- **The origin tag in C (B2).** The core makes the tag unforgeable through
  the type system. C is weaker, so the tag is defended four ways: replay is
  reachable *only* by decoding a wire event (every per-event replay function
  is `static`, so no untagged path can be named, let alone called); the tag
  is stored **biased by one**, so the all-zeroes value that `= {0}` and
  `calloc` produce is `unset` rather than `physical`; the field is `const`,
  which C enforces, so a constructed tag cannot be overwritten; and it is a
  **mandatory by-value parameter** of every replay function, so dropping it
  breaks every call site. Delivery is traced one line per event —
  `seat-replay: seq=N event=… origin=physical|emulated … delivered=0|1`.

**Where the tag is observable, and the limit of that claim.** The acceptance
criterion asks for the tag "observable in the flight-recorder trace of the
delivery". The flight recorder (`crates/vitrin-core/src/recorder.rs`) is
**core-side**, and `vitrin_shim_seat` has *zero requests* — there is no
shim → core input acknowledgement anywhere in v0 — so the core structurally
cannot record what the shim *delivered*, only what it *sent*. As written the
criterion is therefore **not satisfiable without a protocol change**. What is
proven instead is the strongest available substitute: the core's record of
what it emitted and the shim's record of what it delivered are projected to
`(event, origin)` and compared **one-to-one, in order**, by two programs that
share no code. Closing the gap properly needs one of (a) an `origin` field on
the recorder's `use_decision` entries — rust-core track, and the recorder
records no origin at all today — or (b) a v2 shim → core delivery
acknowledgement carrying the tag, which is a protocol-track change.

**A global was added, empirically.** `wl_data_device_manager` joined the v0
set here because GDK treats it as a prerequisite for having a seat at all:
GTK 4 refuses to open the display without it and GTK 3 opens the display but
never constructs a `GdkSeat`, so **no GTK app can receive keyboard input**.
It grants nothing across the realm boundary — one client per shim means both
ends of any transfer are the same app, and v0 has no clipboard message on the
wire at all.

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
| `wl_seat` | `wlr_seat_create` (+ the virtual keyboard, `src/seat.c`) |
| `wl_output` | `wlr_output_create_global` |
| `wl_data_device_manager` | `wlr_data_device_manager_create` — **P1.6.3**, see above |
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

[`tests/acceptance/seat_input_replay.sh`](tests/acceptance/seat_input_replay.sh)
— the P1.6.3 criteria, asserted by **three independent witnesses**: the core
(`mock_core.c --input SCRIPT`, which plays a scripted run of seat events and
traces each one), the shim (its own `seat-replay:` delivery trace), and the
app ([`tests/input_echo_client.c`](tests/input_echo_client.c), a real Wayland
client that resolves every key through the keymap it was sent, with
xkbcommon, exactly as a toolkit does). The R5 gate — *"héllo→世界 arrives
intact in a GTK text field"* — additionally drives a real `GtkEntry`
([`tests/gtk_entry_probe.c`](tests/gtk_entry_probe.c)) and reads the bytes
back out, because the toolkit's input-method layer is what the criterion is
actually about and no keysym stream can stand in for it. That probe is built
only where GTK is installed; the check reports `SKIP` on a machine without
it, and **fails** rather than skipping under `CI` (declare a deliberate gap
with `VITRIN_SKIP_GTK_GATE=1`).

```bash
BUILD_DIR=./build bash tests/acceptance/seat_input_replay.sh
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
