# Firefox in a realm (P1.6.4)

Firefox is the MVP's real app and the top of the R4 bring-up ladder
(`weston-terminal` → GTK app → Firefox). This page is what a person needs to
run it, plus the record of what running it actually taught us.

Companion files in this directory:

| File | What it is |
|---|---|
| [`globals-touched-firefox-140.12.0esr.log`](globals-touched-firefox-140.12.0esr.log) | Raw, unedited ledger output from a probe run — the evidence |
| [`firefox-refused-globals.txt`](firefox-refused-globals.txt) | The machine-readable allowlist of interfaces we knowingly refuse |

---

## 1. The pin

**Firefox ESR 140.12.0**, as a Mozilla tarball, sha256
`3323ee13ac6fe4877fa2e1f4a3aa6b8009f65a620c7bbca96fe86f1a6f433d92`.

The single source of truth is
[`../tests/firefox/firefox-esr.pin`](../tests/firefox/firefox-esr.pin), which
is shell-sourceable so the fetch script, CI and a human all read the same
three values.

```bash
bash shim/tests/firefox/fetch-esr.sh          # fetch if absent, verify always
bash shim/tests/firefox/fetch-esr.sh --print  # just print the binary path
```

It lands in `shim/.firefox/` — **gitignored**. Neither the tarball nor the
unpacked browser is ever committed.

**Why a tarball and not a distro package.** Arch ships no `firefox-esr` at
all, so on the development machine there is nothing to pin to. Where a
package does exist (Debian, Ubuntu) the version is the distro's to move, and
"pin an ESR so the target stops moving" (R4) is not satisfied by a name that
resolves to a different build next month — Firefox's Wayland path changes
between releases, which is the entire reason R4 exists. The tarball is
version-exact, and Mozilla publishes `SHA256SUMS` for it, so the pin is
*verifiable* rather than merely stated. A pin that is not checksummed is not
a pin, so `fetch-esr.sh` hashes into a temporary name and only moves the file
into place after the comparison passes — there is no path through it that
leaves an unverified browser on disk.

**Upgrading is a deliberate act.** Change the three values in the pin file,
re-run the fetch, re-run the acceptance script, and **regenerate the checked-in
ledger** — a new Firefox may want different globals, and that log is what the
global set is argued from.

---

## 2. The Wayland environment

```
MOZ_ENABLE_WAYLAND=1     use the Wayland backend, not XWayland
GDK_BACKEND=wayland      and make GTK agree, so a stray DISPLAY cannot
                         silently drop the whole browser onto X11
MOZ_ACCELERATED=0        no GPU compositing; see software WebRender below
LIBGL_ALWAYS_SOFTWARE=1  and no GL driver probing behind its back
MOZ_CRASHREPORTER_DISABLE=1   a crash must fail the run, not open a dialog
GTK_A11Y=none            no accessibility bus…
NO_AT_BRIDGE=1           …and no AT-SPI bridge: neither exists here, and
                         waiting for them adds ~20 s of nothing to startup
```

The harness passes these with `env -i`, so the browser inherits *exactly* this
and nothing from the operator's session — no host `WAYLAND_DISPLAY`, no
`DISPLAY`, no theme, no locale. A test that passes only on a machine with the
right desktop session is not a test.

### Software WebRender is a documented supported configuration

Not a workaround — a supported configuration, and the plan says so (issue #36:
"software WebRender fallback is a documented supported configuration
(CI/GPU-less parity with D3's shm-first posture)").

The reason is structural. The shim runs on wlroots' **headless backend with
the pixman renderer** and never touches a DRM device — the trusted core owns
the screen. There is no GPU in that picture by construction, and CI will never
have one. D3 makes the same choice on the buffer path: shm is the mandatory v0
path and dmabuf is an opt-in optimization. A browser configuration that needed
hardware would contradict the architecture, not merely be inconvenient.

So the profile pins WebRender's CPU backend (SWGL) explicitly:

```js
user_pref("gfx.webrender.software", true);
user_pref("gfx.webrender.all", true);
user_pref("layers.acceleration.disabled", true);
```

`webrender.all` alongside `webrender.software` is load-bearing: without it,
Firefox on a machine where it cannot probe a GPU can fall back *further*, to
the legacy Basic compositor. That still paints, so nothing looks broken — it
is simply a different code path from the one the demo and CI are meant to
exercise, and the drift would be silent.

Measured on this configuration: a 1024×768 realm view, 55–1200 forwarded
frames over a 20-second run depending on how hard the page works the
compositor, `max_inflight=1` throughout (the P1.6.2 backpressure rule holds
under a real browser), and genuine partial damage — a typical run has commits
carrying 46 080 px of damage against a 786 432 px surface, so Firefox's
incremental repaints survive the shim's damage path rather than being
flattened into full-surface blits.

---

## 3. The profile

[`../tests/firefox/profile.user.js`](../tests/firefox/profile.user.js), copied
to `<profile>/user.js`. A **fresh profile per scenario** — a profile that
persists across runs accumulates session state, and "deterministic" then means
"deterministic until someone runs it twice".

Four guarantees, in order of how badly each would corrupt a result:

1. **No network.** Not "less network" — none. Every remote request is pointed
   at `127.0.0.1:1`, where nothing listens, so it fails at connect
   *immediately* rather than hanging for a timeout. The individual feature
   switches (telemetry, Safe Browsing, Normandy, captive-portal detection,
   blocklists, GMP, region lookup, DoH) are still set, because a request never
   issued costs nothing and the list doubles as documentation of what Firefox
   would otherwise phone home about. The pages under test are `file://` URLs,
   which proxying does not touch. **Network flake cannot redden this test.**
2. **No first-run UI.** The welcome tour, the "what's new" page and the
   default-browser prompt all paint over the content area — the exact thing
   being measured. `browser.startup.homepage_override.mstone = "ignore"` is
   the specific switch that matters: without it, a *fresh* profile on a pinned
   build still shows a "what's new" tab, because "fresh profile" reads to
   Firefox as "upgraded from nothing".
3. **No update machinery.** An update check that succeeds is a network
   dependency; one that half-succeeds can restart the browser mid-run. The pin
   is meaningless if the binary can update itself out from under it.
4. **URL-bar determinism.** `keyword.enabled = false` stops a typed URL
   becoming a *search* (which, behind the dead proxy, would be an error page
   and would fail the assertion for the wrong reason); the suggestion prefs
   stop an autocomplete dropdown painting over the content area between the
   keystrokes and the Return.

---

## 4. What Firefox actually touched

Read [`globals-touched-firefox-140.12.0esr.log`](globals-touched-firefox-140.12.0esr.log)
for the raw record. The mechanism that produced it — and why an interface we
do *not* advertise is otherwise completely invisible — is documented in
[`../include/ledger.h`](../include/ledger.h).

### Bound from the v0 set

| Interface | Advertised | Firefox asked for | Note |
|---|---|---|---|
| `wl_compositor` | 6 | 3 and 4 | two registries: GDK's and Firefox's own `nsWaylandDisplay` |
| `wl_subcompositor` | 1 | 1 | **added here** — see below |
| `wl_shm` | 2 | 1 | three binds, one per connection |
| `wl_output` | 4 | 2 | |
| `wl_seat` | 9 | 5 and 8 | |
| `xdg_wm_base` | 6 | 6 | |
| `wl_data_device_manager` | 3 | 3 | added in P1.6.3 for GDK |
| `zxdg_decoration_manager_v1` | 1 | — | **never bound.** Firefox draws its own decorations and does not negotiate; the global costs nothing and is kept for apps that do. |

### The addition: `wl_subcompositor`

This is the one global P1.6.4 added, and it is the whole point of the
exercise, so the trail is worth stating end to end:

1. Firefox segfaulted after creating exactly two `wl_surface`s, before ever
   creating an `xdg_surface`. No window, no frames, and nothing in the shim's
   log that explained it. (Exit 139, core dumped; 1 commit forwarded.)
2. A probe run produced `globals-demand: interface=wl_subcompositor` — twice,
   from GDK and from Firefox's own display code. Those two lines, with the
   whole run around them, are checked in as
   [`globals-demand-wl_subcompositor-140.12.0esr.log`](globals-demand-wl_subcompositor-140.12.0esr.log)
   at `seq=15` and `seq=22`. Read each next to its neighbours: `seq=15` sits
   with `wl_compositor` v3 (GDK's bind), `seq=22` with `wl_compositor` v4 and
   `wl_seat` v8 (`nsWaylandDisplay`'s).
3. Bisecting with `--probe-globals=wl_subcompositor` (nothing else in the
   catalogue armed) took the same build from **segfault, 2 surfaces, no
   window** to **window mapped, ran to the end of the timebox**. One
   interface, decisive.
4. It was then implemented for real — `wlr_subcompositor_create`, not a stub —
   and the probe entry retired. A stub would not have done: with the interface
   present but *inert*, the window maps and only **3** frames are forwarded
   against the ~57 the shipping build manages, because Firefox's content
   subsurface never composites.

That evidence file is necessarily a **pre-addition** run: once an interface is
in the v0 set the shim refuses to arm a probe for it (`in_v0_contract`, so an
inert copy can never shadow the real one), and it can then only appear in a
ledger as a successful `class=v0` bind — the weaker signal. The file's header
records the two-line source edit that reproduces it.

It grants nothing across the realm boundary, by a stronger version of the
argument that admitted `wl_data_device_manager`: the protocol *requires* a
subsurface and its parent to belong to the same client, and a shim serves
exactly one client. So this composes one app's own surfaces into one window,
which is the shim's job description. It also needed no change to the frame
path: `wlr_scene_xdg_surface_create` already covers a toplevel's subsurfaces,
so they composite into the same buffer and travel upstream as ordinary damage.

### What Firefox asked for and did not get

Fifteen interfaces, all refused deliberately. The list is mirrored in
[`firefox-refused-globals.txt`](firefox-refused-globals.txt), which the
acceptance script enforces. Firefox degrades gracefully on every one — it
renders, repaints, scrolls and navigates without them, which is the empirical
part of "no more than is genuinely needed".

| Interface | Why not |
|---|---|
| `wp_viewporter` | Surface scaling/cropping. The realm view is 1:1 and the shim does no scaling; adding it would advertise a capability with nothing behind it. Revisit only if HiDPI enters the realm model. |
| `wp_fractional_scale_manager_v1` | Same reason, fractional. The realm view has one integer scale. |
| `wp_presentation` | Presentation timestamps. v0 paces the app with the core's `frame_done` relay (PRD Doc 2 §4.4) — that *is* the presentation clock, and a second, unsynchronised one would be a lie about when pixels were shown. |
| `wp_cursor_shape_manager_v1` | Lets a client name a cursor instead of supplying a buffer. The shim has no cursor at all — the core owns the pointer, and cursor rendering is the core's business. |
| `zwp_pointer_constraints_v1` | Pointer lock/confinement. Would fight the actuation model head-on: D10 says the agent addresses realm-view pixel coordinates, and a client that can warp or confine the pointer can invalidate what the agent observed between observation and actuation. Needs a deliberate design pass, not a stub. |
| `zwp_relative_pointer_manager_v1` | The other half of pointer lock. Same argument; also, v0's seat vocabulary has no relative-motion event to feed it. |
| `zwp_pointer_gestures_v1` | Pinch/swipe/hold. v0 has no gesture event on the wire; advertising this invites a client to wait for gestures that can never arrive. |
| `zwp_tablet_manager_v2` | Same shape, for a device class the realm has no vocabulary for. |
| `zwp_text_input_manager_v3` | IME. This is explicitly the **Phase-2 E2.8 workstream** (D7): the dynamic-keymap technique is the Phase-1 answer and `text-input-v3` is what retires it. Adding an inert one now would make apps *stop* using the keymap path that works. |
| `zwp_primary_selection_device_manager_v1` | Middle-click paste. Unlike `wl_data_device_manager` — which GDK treats as a prerequisite for having a seat at all — nothing depends on this to function; it is convenience, and the same one-client argument that makes it harmless also makes it useless. |
| `zwp_keyboard_shortcuts_inhibit_manager_v1` | Lets a client ask the compositor to stop intercepting shortcuts. The shim intercepts none: every key it delivers came from the core. Nothing to inhibit. |
| `xdg_activation_v1` | Cross-app focus transfer and startup notification. Explicitly *between* applications, which is exactly what a realm boundary exists to mediate. If it ever lands it belongs to the core, not to a per-app shim. |
| `zxdg_exporter_v2` | xdg-foreign: hands another process a handle to your surface so it can parent a window to it. A capability-native display server does not get to hand out ambient cross-app surface handles. |
| `zxdg_output_manager_v1` | Logical output geometry. Superseded by `wl_output` v4, which the shim advertises and Firefox binds; the information is already available. |
| `zwp_idle_inhibit_manager_v1` | "Do not blank the screen while this video plays." A realm has no screen and no idle timer; the host's screen is the core's, and inhibiting it from inside a confined app is a decision for the core. |

Five catalogue entries went **untouched** by Firefox — no demand at all:
`wp_single_pixel_buffer_manager_v1`, `wp_content_type_manager_v1`,
`xdg_dialog_v1`, `xdg_system_bell_v1`, `xdg_toplevel_icon_manager_v1`. They
stay in the probe catalogue because GTK 4 references them and the catalogue is
for the whole bring-up ladder, not for Firefox alone.

---

## 5. A bug Firefox found

The shim **aborted** — not misbehaved, aborted, taking the realm with it — the
first time a real browser ran against it:

```
vitrin-shim: types/xdg_shell/wlr_xdg_surface.c:168:
  wlr_xdg_surface_schedule_configure: Assertion `surface->initialized' failed.
```

xdg-shell lets a client set its initial state (`set_maximized`,
`set_fullscreen`, `set_title`) on a brand-new toplevel **before** the first
commit that makes the surface configurable. Firefox does exactly that during
window construction. The shim's `request_maximize` handler answered
immediately, as the protocol requires it to answer *eventually*, and wlroots
answered a configure scheduled against an uninitialised surface with an
assertion.

The fix (`../src/xdg.c`) is to defer: a state request that arrives before the
initial commit is honoured by the initial-commit path, which configures every
toplevel to the view anyway and is the first moment a configure is legal. Same
geometry, one round trip later.

Neither `weston-terminal` nor any test client in this tree reaches that path,
which is precisely why the ladder ends at a real browser. The acceptance
script now checks the shim log for `Assertion` on every scenario.

---

## 6. Running it

```bash
bash shim/tests/firefox/fetch-esr.sh
meson compile -C shim/build
BUILD_DIR=./build bash shim/tests/acceptance/firefox_bringup.sh
```

Without the browser the script **skips loudly and exits 0** locally, and
**fails** under `CI` unless the gap is declared with
`VITRIN_SKIP_FIREFOX_GATE=1` — the same rule the P1.6.2 conformance test and
the P1.6.3 GTK gate apply to themselves. A named acceptance criterion that
only ever reports SKIP on the machine that gates merges is a criterion nobody
is holding.

To survey the globals yourself:

```bash
# everything an app wants, including what we do not provide
./build/vitrin-shim --no-upstream --probe-globals --globals-log /tmp/g.log

# bisect: arm one candidate and see whether the app stops failing
./build/vitrin-shim --no-upstream --probe-globals=wp_viewporter --globals-log /tmp/g.log
```

The bisect example names an interface that is **not** in the v0 set, because
those are the only ones a probe can be armed for. Naming a contract interface
(`--probe-globals=wl_subcompositor`) arms *nothing* — advertising an interface
twice, once real and once inert, would let the app bind the inert copy and
measure a browser strictly worse than the real one. The shim says so at
`ERROR` rather than leaving you with a run that reports `demanded=0`, which
would read as "the app did not want it" when the truth is "it was never
offered, so the question was never asked":

```
--probe-globals=wl_subcompositor: 'wl_subcompositor' IS ALREADY IN THE V0 SET
and was not armed -- ... To re-test whether it is still needed, remove it from
globals.c and from vitrin_v0_contract[], then probe it.
```

A typo (`--probe-globals=wp_viewporer`) gets the same treatment, naming the
catalogue size so you can tell a misspelling from a build that dropped the
row. Re-probing a contract interface is deliberately a source edit, not a
flag: see the header of
[`globals-demand-wl_subcompositor-140.12.0esr.log`](globals-demand-wl_subcompositor-140.12.0esr.log),
which is exactly that procedure, run.

`--probe-globals` **lies to the client** — it advertises interfaces backed by
nothing, so an app that waits on one can hang. It is a diagnostic mode, it is
off by default, it announces itself at `ERROR` level on startup, and every
report it produces carries `probe_mode=1`. Never run a realm with it.

---

## 7. The real-core gate (P1.6.6) — the milestone proof

`firefox_bringup.sh` (section 6) runs the real shim and the real Firefox, but
under the **mock core** (`shim/tests/mock_core.c`). That makes it a
shim-in-isolation smoke test — valuable, and kept — but **not** the M1.2 "Shim
runs Firefox" milestone proof, because one half of the system (the trusted
Rust core) is a stand-in.

The milestone proof is
[`tests/integration/test_real_firefox.py`](../../tests/integration/test_real_firefox.py):
the shipped `vitrind` execs the built `vitrin-shim`, which fork/execs this same
pinned Firefox, which renders a local `file://` page
([`../tests/firefox/pages/solid.html`](../tests/firefox/pages/solid.html),
a solid `#0000ff`), and the **real Python SDK** captures a real Firefox frame
through the real enforcement/capture path and asserts its dominant colour is
the served colour — no mock on any seam. It also captures the globals ledger
from that real-core run and asserts Firefox demanded nothing outside
[`firefox-refused-globals.txt`](firefox-refused-globals.txt), the same
allowlist check (D) makes.

```bash
# Real chain: real vitrind → real vitrin-shim → Firefox, headless, no network.
cargo build --workspace
meson compile -C shim/build
bash shim/tests/firefox/fetch-esr.sh
VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" \
VITRIN_REPO="$PWD" \
PYTHONPATH="$PWD/sdk/python/src" \
  python3 -m unittest discover -s tests/integration -p 'test_real_firefox.py' -v
```

It obeys the same skip-or-fail discipline as `firefox_bringup.sh`:
`VITRIN_SKIP_FIREFOX_GATE=1` opts out; with `VITRIN_C_SHIM_BIN` set it
**fails** (never skips) on a missing shim or a missing browser. In CI it is its
own step in the `integration` job (it fetches the pinned ESR first), separate
from the ordinary suite, which sets `VITRIN_SKIP_FIREFOX_GATE=1` because it does
not fetch the browser.

**How the shim gets `--globals-log`/`--probe-globals` under the real core.**
The production core conveys only the app command in the shim's argv (`<shim> --
<app> <app-args>`; it does *not* pass shim flags), so the real-core gate asks
for the ledger and the probe catalogue through the **environment** instead:
`VITRIN_SHIM_GLOBALS_LOG=<path>` and `VITRIN_SHIM_PROBE_GLOBALS=1`, forwarded to
the shim the one legal way a realm's environment grows — the realm's
`env_allow` (`shim/src/main.c` reads them only as a fallback when the matching
flag is absent, exactly as it already falls back to `$WAYLAND_DISPLAY` for its
socket). Both are diagnostic, both default off, and neither changes what the
shim advertises to a normal app, so the isolation-invisible wire contract
(PRD Doc 2 §4.5) is untouched.

The agent-driven counterpart — *injecting* pointer scroll and typing into
Firefox's URL bar through the SDK, under the real core — is issue #108
(`track:sdk`), not this render gate.

## 8. Watching it nested (dev / manual QA matrix)

The CI gates above are headless and GPU-free by construction (R1): they prove
the pixels are correct, but nobody *sees* the window. To actually watch an
agent operate Firefox in a nested realm on a workstation with a GPU, run the
real core in nested mode (`vitrind --nested`, the core as a Wayland client of
your host compositor, rendering into one host window — PRD Doc 2 §4). This is a
**manual** procedure, not a CI test.

Both venues need the pinned Firefox fetched (`bash
shim/tests/firefox/fetch-esr.sh`), the workspace built (`cargo build
--workspace`), and the shim built (`meson compile -C shim/build`). Point the
core at the shim with `--shim "$PWD/shim/build/vitrin-shim"` and a `--realm`
file whose `command` is the Firefox binary with the section 2 environment (a
fresh `--profile`, `MOZ_ENABLE_WAYLAND=1`, software WebRender, `file://` pages
only) forwarded via `env_allow` — the same realm the render gate builds, just
against `--nested` instead of `--headless`.

| Host | How to get a nesting host, and watch |
|---|---|
| **GNOME** | A nested GNOME gives the core a host Wayland socket to be a client of, without disturbing your session: `dbus-run-session -- gnome-shell --devkit` (or `--nested` on older GNOME) opens GNOME in a window; run `vitrind --nested …` inside it and Firefox's realm appears as a window within that window. |
| **Hyprland** | From a spare TTY (Ctrl+Alt+F3), `Hyprland` starts a bare session on that VT; launch `vitrind --nested …` from a terminal inside it. Hyprland is the second host the M1.1/M1.2 nested matrix (docs/plan §5, R1) is validated on, so a regression that only shows under one compositor is caught. |

Keep the nested rendering trivial and expect the winit backend's known rough
edges (input-model gaps, HiDPI oddities — R1); headless is the fallback venue
and CI never depends on nested. What you are checking by eye here is the thing
the headless gates cannot show: that a human and an agent can watch the *same*
live Firefox in a confined nested surface at once.

### The P1.8.5 agent-capture nested variant (issue #107, criterion 6)

Section 7's gate proves the **agent** captures a real Firefox frame through the
real chokepoint — headless. Its nested counterpart is the M1.3 exit gate's
sixth criterion: run that same agent capture against a Firefox realm the human
is *also* watching, in one host window, on a workstation. It is the visible
proof of the whole thesis — a human and an AI agent observing one confined GUI
concurrently — and it is **manual by construction**: nested mode is a Wayland
client of your host compositor and needs a real display and (for a browser) a
GPU, none of which a CI runner has (R1, and docs/plan §6 D3). So this is **never
a CI test** — not skipped in CI, not present in CI. The headless gates
(`test_real_firefox.py`, `test_real_capture_fidelity.py`) are what merges are
held on; this is what a person runs to *see* it.

Run it with the same core the headless gate uses, in `--nested` instead of
`--headless`, and drive it with the agent-capture snippet the fidelity gate
uses. Get a nesting host up (the table above — nested GNOME or Hyprland from a
spare TTY), then, inside it:

```bash
# Prerequisites, same as every venue on this page.
cargo build --workspace
meson compile -C shim/build
bash shim/tests/firefox/fetch-esr.sh

# A realm whose command is the pinned Firefox on a solid #0000ff file:// page,
# with the section-2 environment forwarded via env_allow. `--consent=interactive`
# is available here (unlike headless): the nested backend draws the consent
# surface, so you approve the agent's grant on screen, in the trusted indicator.
cat > /tmp/ff-nested-realm.toml <<TOML
[[realm]]
id = "realm-0"
command = "$(bash shim/tests/firefox/fetch-esr.sh --print)"
args = ["--profile", "/tmp/ff-nested-profile", "--no-remote",
        "file://$PWD/shim/tests/firefox/pages/solid.html"]
env_allow = ["MOZ_ENABLE_WAYLAND", "GDK_BACKEND", "MOZ_ACCELERATED",
             "LIBGL_ALWAYS_SOFTWARE", "MOZ_CRASHREPORTER_DISABLE",
             "GTK_A11Y", "NO_AT_BRIDGE", "HOME"]
TOML

# The core, nested: it appears as one window in your host compositor, and
# Firefox's realm composites inside it. You watch; the agent captures.
MOZ_ENABLE_WAYLAND=1 GDK_BACKEND=wayland MOZ_ACCELERATED=0 \
LIBGL_ALWAYS_SOFTWARE=1 MOZ_CRASHREPORTER_DISABLE=1 GTK_A11Y=none \
NO_AT_BRIDGE=1 HOME=/tmp/ff-nested-profile \
  target/debug/vitrind --nested --consent=interactive \
    --shim "$PWD/shim/build/vitrin-shim" \
    --realm /tmp/ff-nested-realm.toml \
    --principals examples/principals.toml \
    --recorder /tmp/ff-nested.jsonl
```

Then, from a second terminal, connect the demo agent (or a REPL over the SDK)
to `$XDG_RUNTIME_DIR/vitrin-0/core.sock`, request the one whole-realm grant,
approve it on the nested consent surface, and `observe()` — the dominant colour
is `#0000ff`, exactly as the headless gate asserts, but now over a window you
can see. Add `--capture-dump /tmp/ff-nested.rgba` to the core and compare an
agent frame against it with `vitrin-golden-cmp` for the no-distortion proof on
real GPU output — where a `tol:` policy earns its keep over `exact`, because a
GPU composite and the agent's readback can differ by a hair of blending that a
software pixman render never shows.

Because it is manual, its "never a silent skip" guarantee is structural: there
is no CI job to skip. The commands above either run and show you the window or
fail loudly on a missing host/browser in your own terminal.

### The P1.8.6 agent-actuation nested variant (issue #108, criterion 6)

The headless actuation gate (`test_real_actuation.py`) proves an agent's click
lands on a `click-target` and a typed Unicode string lands in a `gtk-entry-probe`
— toolkit-free and GPU-free, on apps built for a clean observe()-able response.
Its nested counterpart, workstation-only for the same R1 reasons as the capture
variant above, drives the actuation A/B/C proofs `firefox_bringup.sh` runs under
the *mock* core (§6) but through the *real* core and the *real agent SDK*, and
confirms each by the agent's own `observe()`:

- **Pointer scroll (B).** Point the realm's `command` at Firefox on
  `pages/scroll.html` (three viewports tall, `#ff0000` at the top, repainting
  `#ffff00` only once the document has really scrolled past a viewport). The
  agent `grant.pointer.scroll(...)`s, then `observe()`s the dominant colour go
  red → yellow — the page scrolled, driven from the agent through the real
  chokepoint.
- **URL bar text (C).** On `about:blank`, the agent `grant.text.type("\x0c")`
  (Ctrl+L focuses the URL bar) or clicks it, types the `file://` URL of
  `pages/urlbar-target.html` ending in `\n`, and `observe()`s the navigation
  land as that page's dominant colour. The URL — a full Unicode-capable string
  — arriving intact in a real browser's URL bar is D7 at the top of the ladder.

Same realm and environment as the capture variant above (a fresh `--profile`,
software WebRender, `file://` pages, `env_allow`), just with the agent
actuating rather than only observing, and `--consent=interactive` so you
approve the actuate grant on the nested consent surface. It is manual by
construction — nested needs a real host compositor and GPU (R1) — so, like the
capture variant, its "never a silent skip" is structural: there is no CI job to
skip, and the commands fail loudly in your own terminal on a missing
host/browser. The agent-driven injection *into* Firefox is issue #108's nested
rung; the headless rungs are what merges are held on.

### Nested HUMAN keyboard — the #118 fix and workstation check

Distinct from the agent-injection rungs above: this is the *human* typing on
the host keyboard while a nested realm is focused (P1.3.7 — "nested host
keyboard/pointer events are the human principal"). Pointer works fully, and
since #118, so does the keyboard, text keys included:

- **Layout-invariant keys** — Escape (the hold-Esc dead-man chord), Enter,
  Tab, arrows, Home/End/PageUp/Down, F-keys, modifiers — resolve from the
  scancode alone (`crates/vitrin-core/src/input/mod.rs` `invariant_keysym`),
  regardless of which path reached them, so dead-man is unaffected by
  anything below.
- **Text keys (letters, digits, punctuation)** now reach the app too. Their
  keysym is layout-*dependent* and only the host knows it; winit computes it
  (`KeyEvent::logical_key`), but the pinned Smithay 0.7.0 `backend::winit`
  wrapper discards it before the core ever sees it — its own
  `ApplicationHandler` and event loop are both private, with no hook to
  observe the raw `WindowEvent`. Issue #118's fix is for
  `crates/vitrin-core/src/backend/winit.rs` to own that winit glue outright: a
  from-scratch `WinitGraphicsBackend`/`WinitEventLoop` pair
  (`NestedWinitBackend`/`NestedWinitEvents`), built from the same public
  `smithay::backend::egl` primitives Smithay's own module uses, whose
  `ApplicationHandler` (`NestedWinitEventsApp`) resolves `logical_key` via
  `input::host_keysym` and routes it straight to `input::physical_key`
  (`NestedState::handle_key`), bypassing `intake_physical`'s scancode-only
  `Keyboard` arm (which stays, for the generic-`InputBackend` unit tests).
  Resolution and delivery were already in place and unit-tested before the
  wiring landed (`input`'s `resolve_prefers_the_host_keysym_…` and
  `a_text_key_given_a_host_keysym_reaches_the_app_as_physical_input`); #118
  closed the one remaining gap, winit's own event pump.

**Workstation check** (nested needs a display + GPU, so this is manual, never
CI): boot the nested realm as in the capture variant above, focus the Firefox
window inside it, and type a `file://` URL into the address bar followed by
Enter. The typed characters should appear in the URL bar and the page should
navigate — the flight recorder (JSON lines) carries
`"kind":"seat_delivered","event":"key","origin":"physical"` for every key, text
and layout-invariant alike. Click on another window to move host focus away
and back: `WindowEvent::Focused(false)`/`(true)` should log at `debug` (host
window lost/gained keyboard focus) with no crash and no stuck dead-man hold —
losing focus mid-hold forgets it (`NestedState::handle_focus`), so alt-tabbing
away with Escape held never revokes the session with no gesture behind it.

## 9. Holding Escape for real: the nested dead-man recipe (issue #109)

Section 8's rungs are all agent-side (a grant petitioning, capturing,
actuating) or human-typing. This one is the *human's own off-switch*:
P1.7.3's hold-Esc dead-man chord, held on a real keyboard against a real
Firefox realm, watched revoke a real grant on screen.

### Why this is nested-only, structurally, and what the headless CI gate proves instead

The chord's *detection* half — a human pressing and holding a real key long
enough — needs a physical input device, which headless has none of by
construction (`crates/vitrin-core/src/deadman.rs`'s module docs; PRD Doc 2
§9). So, like every other rung on this page, it is **never a CI test** — not
skipped, not present — and this section is manual by the same R1 posture as
sections 8's other venues.

What headless CI *does* prove, in
`tests/integration/test_real_deadman.py` (P1.7.4, the M1.4 dead-man exit
gate), is everything **downstream** of a completed chord, against a real app:
a `SIGUSR1` sent to a `dead-man-injector`-feature `vitrind` synthesizes the
same [`Trigger`] a completed physical hold produces and applies it through
the identical `Runtime::apply_dead_man` entry point the nested backend's
`DeadManHost::on_trigger` calls — so what CI proves about *revocation* (every
grant gone, the table sealed, the real app's `wl_seat` receiving nothing more)
is exactly as strong as this manual recipe's. Only the keypress-to-`Trigger`
wiring below is unproven by CI, and that is what this section is for.

### The recipe

Same prerequisites and nesting-host table as section 8 (a nested GNOME or
Hyprland session, the pinned Firefox fetched, the workspace and shim built).
Boot the same nested Firefox realm as the P1.8.5 capture variant, adding
`--dead-man-chord`/`--dead-man-hold` only if you want to override their
defaults (`esc`, 1000 ms — `crates/vitrin-core/src/deadman.rs`'s
`DEFAULT_CHORD`/`DEFAULT_HOLD`):

```bash
# Prerequisites, same as every venue on this page.
cargo build --workspace
meson compile -C shim/build
bash shim/tests/firefox/fetch-esr.sh

# The same nested Firefox realm section 8's capture variant builds
# (/tmp/ff-nested-realm.toml) — reused here unchanged.

MOZ_ENABLE_WAYLAND=1 GDK_BACKEND=wayland MOZ_ACCELERATED=0 \
LIBGL_ALWAYS_SOFTWARE=1 MOZ_CRASHREPORTER_DISABLE=1 GTK_A11Y=none \
NO_AT_BRIDGE=1 HOME=/tmp/ff-nested-profile \
  target/debug/vitrind --nested --consent=interactive \
    --shim "$PWD/shim/build/vitrin-shim" \
    --realm /tmp/ff-nested-realm.toml \
    --principals examples/principals.toml \
    --recorder /tmp/ff-nested.jsonl
    # --dead-man-chord esc --dead-man-hold 1000   (the defaults; spelled out
    # here only so this is the one place to change them)
```

From a second terminal, connect the demo agent, request the one whole-realm
grant, and approve it on the nested consent surface (visible in the trusted
indicator band, issue #85) exactly as in section 8's capture variant. Confirm
it is live with one `observe()`.

Then, **with the nested window focused**, press and hold Escape for the
configured duration (1000 ms by default — long enough that a single tap never
fires it; P1.7.3's tap-through-hold-swallow design, `deadman.rs`'s module
docs). You should see:

- Escape does **not** reach Firefox while held (no dialog dismissed, nothing
  typed) — the chord gate withholds it for the hold/tap decision, exactly the
  "held-Esc never reaches the confined app" guarantee `deadman.rs` documents.
- On release *before* the hold completes, Escape is replayed to Firefox as an
  ordinary tap (press+release) — confirming the gate is a *tap-through* one,
  not a swallow-everything one.
- On a *completed* hold, the flight recorder (`tail -f /tmp/ff-nested.jsonl`)
  gains a `dead_man_triggered` entry with `"chord":"esc"` and a `held_ms` at
  or above 1000, followed by a `grant_revoked` entry naming the grant row you
  just approved — the exact write order `deadman::apply` documents.
- The agent's very next `observe()` (or any `actuate.*`) now raises
  `vitrin_os.errors.Revoked` — the same typed exception
  `test_real_deadman.py` asserts headless, now watched fire live over a real
  Firefox realm from a real held key.

Because it is manual, its "never a silent skip" guarantee is structural, the
same as every other rung on this page: there is no CI job to skip, and the
recipe above either shows you the revocation on screen or fails loudly (a
missing host/browser, a chord that never fires) in your own terminal.

[`Trigger`]: ../../crates/vitrin-core/src/deadman.rs
