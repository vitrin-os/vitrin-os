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

## Status — P1.6.4 (Firefox in the realm)

What exists today: a headless compositor that stands up the v0 Wayland
environment, composites its app's window, **forwards every composed frame up
to the trusted core** over the socketpair it inherited at fork (`attach` →
`damage` per rectangle → `commit`, with the core's `frame_done` relayed back
as the app's `wl_surface.frame` callbacks), and **replays the core's
origin-tagged `vitrin_shim_seat` events into the app through its own
`wl_seat`** — pointer motion, buttons, high-resolution scroll, keys, and
Unicode text. **Firefox runs in it**: the pinned ESR renders, repaints,
scrolls under injected input and navigates from a URL typed into its URL bar,
all headless with software WebRender and no GPU. Every global an app touches
is now recorded by a **permanent ledger**, and an interface an app wants but
does not get is recorded too. **Popups map too** — a menu, tooltip or combo
box gets its own `xdg_surface.configure` on its initial commit and its own
scene node under its parent's, so it is drawn where its positioner put it and
is clickable through the same scene hit test the rest of the input path uses.
**Not yet**: `text-input-v3` / IME is the Phase-2 workstream (E2.8) that
retires the synthesized keymap below.

**A bug the popup path was hiding, and why "not yet" was the wrong words for
it.** This paragraph used to say popups were "neither rendered nor clickable"
— as if a menu were missing from an otherwise working app. Measured against
the shipped binary, the app **died**: with no `xdg_shell.new_popup` listener
the shim never configured a popup at any moment, so the popup's first buffer
commit is `xdg_surface` error 3 (`unconfigured_buffer`, "xdg_surface has
never been configured") and the client is disconnected. wlroots does not send
that configure on the compositor's behalf — for popups it only rejects a
parentless one — which is the kind of asymmetry with the toplevel path that a
reading of the spec alone will not catch. Both halves of the fix are in
[`src/xdg.c`](src/xdg.c) (`on_new_popup`), and the configure half is asserted
from the client's side by the conformance test below, which was red on the
tree before it.

### The "globals touched" ledger, in one pass

The v0 global set is "a contract, not a floor", which is only honest if
additions are driven by evidence. [`include/ledger.h`](include/ledger.h) is
that evidence, and [`docs/firefox.md`](docs/firefox.md) is what it produced.

| Step | Where |
|---|---|
| Every `wl_registry.global` we send and `bind` we receive | `src/ledger.c` (one `wl_display` protocol logger) |
| Probe globals: advertised, not implemented, so demand is visible | `src/ledger.c` + `src/probe-catalogue.h.in` |
| Inert-but-well-formed probe resources | `wl_resource_set_dispatcher`, `src/ledger.c` |
| Contract drift check (advertised set vs. the list in code) | `src/ledger.c`, at teardown |
| Records: `globals-offer/bind/demand/touched/summary` | shim log, plus `--globals-log PATH` |

**The crux is that a global we never advertise generates no bind at all.**
Wayland discovery is push: a client learns what exists from the `global`
events the server chooses to send, and binds only what it was told about. So
the single most valuable fact — *the app needed X and X was not there* —
produces **no wire traffic whatsoever**, and a ledger built by logging
`wl_registry.bind` is structurally blind to exactly the case it was built
for. The app's own reaction ranges from a warning on stderr, through silent
degradation, to a SIGSEGV.

`--probe-globals` closes that hole by making demand bindable: a curated
catalogue of interfaces the shim does *not* implement is advertised anyway,
so an app asking for one becomes an ordinary, observable `bind` and a
`globals-demand` line at `ERROR` level. The probes use the **real** generated
marshalling tables with a generic dispatcher, so they are inert but
well-formed — every request demarshals, every `new_id` gets a real child
resource, destructors really destroy. The cheap alternative (a synthetic
`wl_interface` with no methods) kills the client on its first request to it,
which yields one datum per run and a corpse; this way one run surveys the
whole catalogue. `--probe-globals=IFACE,...` arms a subset, which is what
turns *"what did the app ask for"* into *"which of those did it actually
need"* without recompiling.

Probe mode **lies to the client** — an app that waits on an inert global can
hang — so it is off by default, announced at `ERROR` on startup, stamped into
every report as `probe_mode=1`, and never used to run a realm. It also
refuses to shadow a real global: a probe for an interface already in the v0
set is skipped, because advertising one interface twice lets the app bind the
inert copy (observed: 3 forwarded frames instead of 55).

**A global was added, empirically — and this time by the machinery.**
`wl_subcompositor` joined the v0 set here. Firefox 140.12.0esr segfaults
without it after two `wl_surface`s and before any `xdg_surface`: no window at
all (exit 139, one commit). The probe run named it — two `globals-demand`
lines, one per connection, checked in as
[`docs/globals-demand-wl_subcompositor-140.12.0esr.log`](docs/globals-demand-wl_subcompositor-140.12.0esr.log)
— and it is now implemented for real with `wlr_subcompositor_create`, which
the bisection shows was necessary: armed as an inert *probe* the window maps
but only 3 frames are forwarded, because the content subsurface never
composites. That evidence file is a **pre-addition** run and has to be,
since the shipping shim arms no probe for an interface already in the
contract. It grants nothing
across the realm boundary — the protocol requires a subsurface and its parent
to belong to the same client, and a shim serves exactly one. Fifteen other
interfaces Firefox asks for are refused, each with a written reason
([`docs/firefox.md`](docs/firefox.md)) and an entry in
[`docs/firefox-refused-globals.txt`](docs/firefox-refused-globals.txt), which
the acceptance script enforces: a future ESR that wants something new turns
the check red instead of going unnoticed.

**A bug Firefox found.** The shim *aborted* — taking the realm with it — on
`wlr_xdg_surface_schedule_configure: Assertion 'surface->initialized' failed`.
xdg-shell lets a client set its initial state before the first commit makes
the surface configurable, and Firefox does exactly that during window
construction. The fix (`src/xdg.c`) defers the answer to the initial-commit
path, which configures every toplevel to the view anyway. No test client in
this tree reaches that path, which is why the ladder ends at a real browser.

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
That global still grants nothing across the realm boundary — one client per
shim means both ends of any transfer *through it* are the same app.

**The last clause of that sentence used to be "and v0 has no clipboard message
on the wire at all". It does now** (WS-E.2.1, issue #213,
[D-024](../docs/plan/20-decision-log.md)). Version 2 adds three
`vitrin_shim_session` messages and `src/clipboard.c` serves them, so a human
can copy in one realm and paste in another. Read the difference carefully,
because it is the whole trust argument: the channel does **not** run through
`wl_data_device_manager`, and no client can reach it at any verb set. The core
*asks* (`request_selection`), this shim *answers once* (`selection`), and a
second, separate physical human gesture makes the core *offer*
(`offer_selection`) to some other realm. There is deliberately no message by
which a shim announces a copy, so an ordinary in-app Ctrl-C is still nothing
but an in-app Ctrl-C. `text/plain;charset=utf-8` only, 60 KiB, cleared on a
timeout, on the source realm's death and on a dead-man trigger.

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
empirically by the "globals touched" ledger above, and each one cites the
log line it came from):

| Global | Source |
|---|---|
| `wl_compositor` | `wlr_compositor_create` |
| `wl_subcompositor` | `wlr_subcompositor_create` — **P1.6.4**, see above |
| `wl_shm` | `wlr_renderer_init_wl_shm` |
| `xdg_wm_base` | `wlr_xdg_shell_create` |
| `wl_seat` | `wlr_seat_create` (+ the virtual keyboard, `src/seat.c`) |
| `wl_output` | `wlr_output_create_global` |
| `wl_data_device_manager` | `wlr_data_device_manager_create` — **P1.6.3**, see above |
| `zwp_relative_pointer_manager_v1` | `wlr_relative_pointer_manager_v1_create` — **WS-E.4.2**, carries `vitrin_shim_seat.relative_motion` |
| `zwp_pointer_gestures_v1` | `wlr_pointer_gestures_v1_create` — **WS-E.4.2**, carries the four gesture events |
| `zwp_pointer_constraints_v1` | `vitrin_constraint_create` (`src/constraint.c`) — **WS-E.4.2**, relays the app's pointer lock/confinement ask to the core and the core's verdict back; see `include/constraint.h` |
| `zxdg_decoration_manager_v1` | `wlr_xdg_decoration_manager_v1_create` (declines SSD) |
| `zwp_linux_dmabuf_v1` | `wlr_linux_dmabuf_v1_create_with_renderer` — **only with `--dmabuf`** |

This table is also asserted at runtime: the ledger cross-checks the list in
`src/ledger.c` against the `wl_registry.global` events actually sent and
reports `globals-contract-drift` if they disagree — so a dependency that
starts creating a global as a side effect cannot slip in unnoticed.

Server-side decorations are declined (clients always draw their own). Per
**D3**, shm is the mandatory buffer path; `linux-dmabuf-v1` is opt-in.

## Building

**D11:** wlroots is pinned to **0.19.3** and vendored as a Meson subproject
(`subprojects/wlroots.wrap`); `wayland-protocols` likewise. The build is
system-first — an installed `wlroots-0.19` is used when present and the wrap
stays dormant; otherwise (e.g. CI) the pinned checkout is built from source.
The wrap is the source of truth for the version; budget one wlroots upgrade
task per phase.

**How strong that pin is, exactly.** `revision = 0.19.3` is a *tag name* on
`gitlab.freedesktop.org`, not a commit hash, and the wrap carries no
`source_hash`. So the wrap fixes what Meson **asks for**, not what it
receives: a moved tag would be fetched without complaint, and nothing
re-checks `subprojects/wlroots/` after the clone. That is a weaker pin than
the Firefox one, which since issue #298 verifies the download *and* the
installed tree ([`docs/firefox.md`](docs/firefox.md) section 1), and weaker
than [`tests/kernel-matrix/kernels.manifest`](../tests/kernel-matrix/kernels.manifest),
which hashes both the `.deb` and the `vmlinuz` extracted from it. It is
recorded here rather than fixed in passing: closing it means a commit-sha
revision and a decision about the system-first path, which is not pinned at
all.

```bash
meson setup build            # uses system wlroots-0.19 if available
ninja -C build
meson test -C build          # header-compiles, xdg-conformance, focus-succession

# Build the vendored wlroots from source (e.g. CI, or no system wlroots-0.19),
# taking wlroots' own dependencies from the system:
meson setup build --force-fallback-for=wlroots-0.19,wlroots
```

The build needs no Rust toolchain — only the checked-in generated header.

## Running

```bash
./build/vitrin-shim [--socket NAME] [--dmabuf] [--no-upstream] [--width W] [--height H]
                    [--globals-log PATH] [--probe-globals[=IFACE,IFACE,...]]
```

`--globals-log PATH` additionally writes the globals ledger to a bare file,
one record per line with no log prefix — what CI archives and what the
acceptance script greps. `--probe-globals` is the diagnostic mode described
above; read the warning it prints before using it.

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

All are GPU-free, headless, and need no Rust toolchain.

[`tests/acceptance/xdg_conformance.sh`](tests/acceptance/xdg_conformance.sh)
— three xdg-shell facts about [`src/xdg.c`](src/xdg.c) that only the app's
side of the socket can observe: the initial `xdg_surface.configure` is sent
on the client's initial commit and **not** before it;
`xdg_toplevel.wm_capabilities` precedes that configure carrying exactly the
capabilities the shim implements (`maximize`, `fullscreen` — not wlroots'
default four); and a **popup** is configured on *its* initial commit, so an
app that opens a menu maps it instead of being disconnected. The client is
[`tests/xdg_conformance_client.c`](tests/xdg_conformance_client.c); the
reasoning for each assertion is in its header comment, and the wlcs failures
that provoked it are annotated in
[`wlcs/README.md`](wlcs/README.md). It and
[`tests/acceptance/focus_succession.sh`](tests/acceptance/focus_succession.sh)
are the two scripts here wired into `meson test`, because they are the two
that need nothing but this tree's own binaries.

Each of the three was measured failing before the code that makes it pass:
the popup checks go red on the tree as it stood one commit earlier, with
`xdg_surface#9: error 3: xdg_surface has never been configured` on the
client's side and `globals-error: seq=13 code=3` in the shim's own ledger.
What this script does **not** cover is popup *pixels* — that a popup
composites into the frames the shim forwards was checked by hand under
`mock-core` (a view-sized popup painted over a differently-coloured toplevel
flipped the reported `dominant=` from `ff0000` to `0000ff`), not by any
checked-in test.

```bash
bash tests/acceptance/xdg_conformance.sh ./build/vitrin-shim ./build/xdg-conformance-client
```

[`tests/acceptance/focus_succession.sh`](tests/acceptance/focus_succession.sh)
— keyboard-focus succession between **sibling windows of one app in one
realm** (**#268**, found on bare metal with alacritty and nautilus: closing a
second window cleared focus to nobody and left the survivor visible and
untypable). The client is
[`tests/focus_succession_client.c`](tests/focus_succession_client.c). It maps
three toplevels **created in one order and mapped in another**, so that "most
recently mapped" is distinguishable from "most recently created", then closes
them one at a time: the keyboard must go to the survivor, then stay put when a
*background* window closes, then be released when the last one goes — with
`xdg_toplevel`'s `activated` state naming the front window throughout. Two
more toplevels repeat the sequence with an `xdg_popup.grab` open, because
wlroots defers every focus change to an active keyboard grab and a menu is
one, so that is the input that makes the whole mechanism silently do nothing.
Every assertion's reasoning is in the client's header comment.

This is the second script here wired into `meson test`, on the same grounds as
`xdg_conformance.sh`: headless, GPU-free, no seat device and no core
(`--no-upstream`), because `vitrin_seat_init` runs unconditionally at bring-up
so the virtual keyboard exists and real `wl_keyboard.enter`/`leave` events can
be observed. It is a **component** test of the shim, not milestone evidence:
the hardware half of #268's acceptance — open a second window inside a realm
on bare metal, close it, and type into the first — is a DRM run that CI cannot
reach.

```bash
bash tests/acceptance/focus_succession.sh ./build/vitrin-shim ./build/focus-succession-client
```

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

[`tests/acceptance/firefox_bringup.sh`](tests/acceptance/firefox_bringup.sh)
— the P1.6.4 criteria, against the pinned **Firefox ESR 140.12.0**. This
script runs under `mock_core.c`, not the real `vitrind` — a deliberate
shim-in-isolation choice, not a limitation of `spawn_realm` (which has had a
real, non-test caller since P1.5.4/#103: `session::start_realm` in the
shipped binary's `winit`/`headless` backends). So every criterion here is
reduced to something measurable in the pixels the shim actually forwarded:
`mock_core.c` reports a **dominant colour per committed frame**, which is the
M1.2 verification the plan specifies. Each page is a local `file://` URL and the
profile makes remote requests fail at connect, so **network flake cannot
redden this test**.

| Check | Page | Assertion |
|---|---|---|
| renders **and repaints** | `repaint.html` | `#0000ff`, then `#00ff00` — in that order; plus commits forwarded, zero wire violations, and ≥1 *partial* damage rect |
| injected scroll works | `scroll.html` | `#ff0000` → `#ffff00`, which the page paints only once the document really scrolled past a third of a viewport |
| injected text in the URL bar | `urlbar-target.html` | Ctrl+L, then the target's `file://` URL as one `text` payload ending in `\n` → `#00ffff` becomes dominant, i.e. the browser navigated |
| the ledger | — | the advertised v0 set is exactly the contract; the probe mechanism fired; every demand is in `docs/firefox-refused-globals.txt` |

```bash
bash tests/firefox/fetch-esr.sh          # pinned, verified twice, gitignored
BUILD_DIR=./build bash tests/acceptance/firefox_bringup.sh
```

Twice, because a checksum over the tarball says what was *downloaded* and
Firefox's own updater rewrites what is *installed*: the fetch script checks
the tarball's sha256 **and** the unpacked tree's `application.ini`
Version/BuildID, and leaves the tree non-writable with the updater switched
off (issue #298, [`docs/firefox.md`](docs/firefox.md) section 1).

The pin, the Wayland environment, the software-WebRender rationale, the
profile, and the full record of what Firefox touched are in
[`docs/firefox.md`](docs/firefox.md). Without the browser the script reports
`SKIP` loudly and **fails** rather than skipping under `CI` (declare a
deliberate gap with `VITRIN_SKIP_FIREFOX_GATE=1`) — the same rule the
conformance test below applies to itself. The shim CI job currently declares
that gap: fetching a 75 MB browser needs runtime libraries that container has
not been validated to carry, so these criteria are held on a developer
machine only, and CI says so in its job summary rather than implying
otherwise with a green tick.

**This script runs under the mock core, so it is a shim-in-isolation smoke
test, not the milestone proof.** Since P1.6.6 (issue #106) the M1.2 "Shim runs
Firefox" render half is proved against the **real** `vitrind`:
[`tests/integration/test_real_firefox.py`](../tests/integration/test_real_firefox.py)
boots the shipped core → this shim → the pinned Firefox and captures a real
Firefox frame of a solid-colour `file://` page through the real SDK, asserting
its dominant colour and its globals ledger. That gate fetches the browser and
runs in CI (its own step in the `integration` job); this mock-core script stays
because it still exercises the shim standalone and asserts an ordered per-frame
colour sequence the poll-based SDK cannot. See
[`docs/firefox.md`](docs/firefox.md) §7 for the split and §8 for the nested
watch-it-yourself matrix.

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
