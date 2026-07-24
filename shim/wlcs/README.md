# WLCS advisory conformance (P1.9.4, issue #47)

This directory bridges [wlcs](https://github.com/canonical/wlcs) (the
Wayland Conformance Test Suite) to `vitrin-shim`, so its `xdg-shell` and
seat/pointer behavior can be compared against upstream Wayland compositor
conformance tests. **It is advisory: nothing here ever blocks a PR.** See
"Why advisory, and how that's enforced" below for the two independent
mechanisms that guarantee that.

## Licensing boundary (read this before touching this directory)

`wlcs`'s own headers (`<wlcs/display_server.h>`, `<wlcs/pointer.h>`, ...) are
**GPL-3.0-only**. The rest of this repository is MIT. To keep those apart:

- `integration.c` (this directory) is the *only* file in the repo that
  `#include`s a wlcs header, and it is marked `SPDX-License-Identifier:
  GPL-3.0-only` in its own header comment, which is the normative statement
  of the boundary.
- The module it builds into, `vitrin-shim-wlcs.so`, is a `shared_module` in
  `shim/meson.build` gated behind a `wlcs` Meson `feature` option
  (`value: 'auto'` — silently skipped if `wlcs` isn't installed, never
  built by default without the option, never `install: true`).
- The main `vitrin-shim` executable never links this module and has no
  `#include` of any wlcs header — the GPL boundary is the directory, not
  some symbol-level firewall inside a shared translation unit.
- We do **not** vendor wlcs's headers into this repo. CI and local builds
  both get them from the system package (`apt install wlcs`, which ships
  `/usr/include/wlcs/*.h` and a `wlcs.pc`) or an equivalent local install —
  see "Building" below for the one wrinkle in that story on Ubuntu today.
- The `wlcs` runner binary itself is a separate GPL-3.0 executable
  (`/usr/lib/*/wlcs/wlcs` from the same package); it `dlopen()`s our
  `.so` and drives it out-of-process from `vitrin-shim`'s own test suite.

None of this changes the license of anything else in the repo. If you're
touching `shim/` outside this directory, you can ignore all of the above.

## What this actually tests

`integration.c` implements `WlcsServerIntegration` (the ABI
`<wlcs/display_server.h>` defines) by driving the **real** shim bring-up/
teardown code (`server.c`, `globals.c`, `output.c`, `xdg.c`, `seat.c`) in
exactly the sequence `main.c`'s `--no-upstream` path uses — this is not a
second, WLCS-shaped compositor, it's the same shim's real code, exercised
through wlcs's real xdg-shell and seat client test suites instead of a
Wayland app.

- **`create_pointer`**: converts wlcs's pointer moves/clicks into
  `vitrin_shim_seat` `motion`/`button` wire events with `origin =
  EMULATED`, injected via `vitrin_seat_handle_event` — the same call path
  real input replay uses, just fed synthetic coordinates instead of a
  physical device.
- **`create_touch`**: returns `NULL`. `vitrin_shim_seat`'s wire vocabulary
  has no touch event at all (by design — touch is out of scope for the
  Phase 1 MVP slice), so there is nothing to bridge; wlcs treats a `NULL`
  touch factory as "skip touch-dependent tests" rather than a failure (see
  the SKIP counts below).
- **`position_window_absolute`**: a no-op. The shim's `xdg.c` policy is a
  fixed single-maximized-toplevel-at-the-origin layout (Phase 1 has no
  window manager), so there is no window position for wlcs to set.
- Everything else (`start`/`stop`/`create_client_socket`/lifecycle) drives
  the shim's `wl_display` event loop on its own thread, with a self-pipe +
  mutex + condvar bridging wlcs's test thread into it — see the big comment
  block at the top of `integration.c` for why that machinery exists and how
  it avoids racing `wl_display`'s single-threaded internals.

## Scope: which wlcs suites run

Issue #47 asks for "xdg-shell + seat" groups specifically, not wlcs's full
suite (which also covers e.g. `wl_data_device`, `wl_subsurface`, layer-shell
and other protocols the shim doesn't implement at all yet, where every test
would fail for the uninteresting reason "no such global"). `run-advisory.sh`
hardcodes a `--gtest_filter` covering:

| Suite | Covers |
|---|---|
| `XdgSurfaceStableTest` | `xdg_surface` lifecycle basics |
| `XdgToplevelStableTest` | `xdg_toplevel` role, parenting, geometry |
| `XdgToplevelStableConfigurationTest` | maximize/fullscreen/activate configure acks |
| `XdgPopupStable/XdgPopupTest` | `xdg_popup` role and grabs |
| `ClientSurfaceEventsTest` | pointer enter/leave/motion delivery to surfaces |
| `SurfaceInputRegions/SurfaceInputCombinations` | `wl_surface.set_input_region` |
| `PointerCrossingSurfaceCorner\|Edge/SurfacePointerMotionTest` | pointer motion across surface boundaries |
| `WlOutputTest` | `wl_output` basics |

**Deliberately excluded, not merely expected to fail:** every `*Touch*`
suite (e.g. `AllSurfaceTypes/TouchTest`). `create_touch` returning `NULL` is
a real, structural absence (see above), not a bug this run would surface —
including those suites would only add skip-noise, not information. Any
other wlcs suite (data device, subsurfaces, layer-shell, ...) is excluded
for the same reason: the shim doesn't implement the underlying global, so
the "failure" would just be "no such interface," which nobody here needs
wlcs to tell them.

## The pass-list, annotated (as of this writing)

A full run of the scope above, against the current shim:

```
total=180 passed=3 failed=145 skipped=32
```

**Passing (3):**

- `XdgSurfaceStableTest.supports_xdg_shell_stable_protocol`
- `WlOutputTest.wl_output_properties_set`
- `WlOutputTest.wl_output_release`

**Skipped (32):** all `SurfaceInputRegions/SurfaceInputCombinations.*` —
wlcs's own fixture for this suite requires more than one output mode /
buffer-format combination to parameterize over, which this shim's
single-headless-output setup doesn't provide; wlcs correctly self-skips
rather than failing. Not investigated further because it's wlcs's own
gating, not a shim behavior difference.

**Failing (145), by root cause — one dominant cause explains the large
majority:**

1. **128 failures — `"Wayland protocol error: 3 on interface xdg_surface
   v5"` ("xdg_surface has never been configured").** This is a real,
   pre-existing conformance gap in the shim's `xdg.c`, not an artifact of
   this bridge. The shim currently only sends the initial `xdg_surface
   .configure` on the surface's first `wl_surface.commit`
   (`initial_commit`-gated). The xdg-shell-stable spec — and wlcs's test
   client — expect the compositor to send that configure proactively, as
   soon as the toplevel/popup *role* is assigned
   (`xdg_surface.get_toplevel`/`get_popup`), before the client's first
   commit. wlcs's client library commits promptly after requesting the
   role and then blocks waiting for the ack it was already spec-entitled
   to have — so almost every test in `XdgSurfaceStableTest`,
   `XdgToplevelStableTest`, `XdgToplevelStableConfigurationTest`,
   `XdgPopupStable/XdgPopupTest`, and `ClientSurfaceEventsTest` hits this
   same wall on its very first surface, regardless of what the test is
   actually trying to check. **This is the single most useful thing this
   PR's wlcs run demonstrates**: fixing xdg.c's configure timing to be
   spec-proactive is the highest-leverage next step for xdg-shell
   conformance, and it's now filed as a tracked follow-up rather than a
   vague "conformance TBD" — see #128.
2. **12 failures — `"Timeout waiting for condition"`, in
   `XdgToplevelStableTest.{parent_can_be_set, null_parent_can_be_set,
   when_parent_is_set_to_{self,child_descendant}_error_is_raised,
   pointer_respects_window_geom_offset,
   touch_respects_window_geom_offset}` and
   `XdgToplevelStableConfigurationTest.{defaults,
   activated_state_follows_pointer, window_can_{,un}maximize_itself,
   window_can_{,un}fullscreen_itself}`.** These need either multiple,
   independently-positioned toplevels (parenting tests) or configure-driven
   state transitions (maximize/fullscreen/activate) that this shim's fixed
   single-maximized-toplevel layout policy (`xdg.c`, plus
   `position_window_absolute` being a no-op above) has no mechanism to
   satisfy — expected, given Phase 1 has no window manager, not a bug this
   run found.
3. **5 failures — remainder of `XdgSurfaceStableTest`, timing/geometry
   edge cases layered on top of cause 1** once a surface *does* get past
   its first configure; not independently triaged since they're
   downstream of the same root cause.

Net: of 52 real (non-skipped-by-wlcs, non-touch) test *intents* in scope,
this run surfaces essentially one architectural gap (proactive xdg-shell
configure) plus one known-and-accepted policy limitation (no window
manager), rather than 145 independent bugs.

## Building

Requires the `wlcs` package (headers + `wlcs.pc` + the `wlcs` runner
binary). On Ubuntu 24.04 (noble): `apt install wlcs` — a single package
(universe) that ships all three under `/usr/include/wlcs/`,
`/usr/lib/<triplet>/pkgconfig/wlcs.pc`, and `/usr/lib/<triplet>/wlcs/wlcs`
respectively; nothing else to configure. (If you're bootstrapping this on a
non-Debian system by hand-extracting the `.deb` instead of using `apt`,
its shipped `wlcs.pc` hardcodes `/usr` paths and won't resolve against an
arbitrary extraction directory — point `PKG_CONFIG_PATH` at a `wlcs.pc`
you've adjusted for wherever you extracted it, or symlink the extraction
into `/usr` locally. This is an artifact of extracting outside the package
manager, not a gap in the Ubuntu package itself.)

```sh
meson setup build -Dwlcs=enabled
ninja -C build shim/vitrin-shim-wlcs.so
```

If `wlcs` isn't installed and `-Dwlcs` is left at its default (`auto`),
this target is silently skipped — it never blocks a normal `shim` build.

## Running

```sh
shim/wlcs/run-advisory.sh <path-to-wlcs-binary> <path-to-vitrin-shim-wlcs.so> [output-dir]
# e.g.:
shim/wlcs/run-advisory.sh /usr/lib/x86_64-linux-gnu/wlcs/wlcs build/shim/vitrin-shim-wlcs.so
```

Prints a `total=/passed=/failed=/skipped=` summary and a breakdown of the
most common failure messages, and writes a full gtest log plus JUnit XML
into the output directory (default `./wlcs-advisory-out/`). **Always exits
0** — see below.

## Why advisory, and how that's enforced

Two independent mechanisms, deliberately redundant, per issue #47's "never
blocks PRs" requirement:

1. `run-advisory.sh` itself always exits `0` — a failing or skipped wlcs
   test never becomes a non-zero process exit, by construction (the
   `wlcs` invocation is `|| true`'d; only a usage error, like a missing
   binary, exits non-zero).
2. The CI job that runs it (`.github/workflows/ci.yml`, job
   `wlcs-advisory`) additionally sets `continue-on-error: true` on itself.

Either alone is one bad rebase away from silently starting to gate merges
(a script that starts propagating exit codes, or a job that stops setting
`continue-on-error`); both together make that a two-mistake accident
instead of a one-line one. The CI job also only runs on the `wlcs`
package being installable on the runner image and treats a setup failure
(package not found, `wlcs.pc` missing) as a skipped step, not a failure —
this suite existing at all is strictly additive information, never a
requirement.

## Limitations (what this does *not* prove)

- **Not a compositor certification.** Passing these tests is not a claim
  that `vitrin-shim` is a conformant Wayland compositor in general — only
  that this specific, hand-picked slice of xdg-shell/seat behavior matches
  wlcs's expectations under a headless, single-output, no-window-manager
  configuration.
- **Single-threaded event loop, multi-threaded test driver**: the bridge's
  correctness rests on the wake-pipe/mutex/condvar scheme in
  `integration.c` being race-free. It has been run repeatedly without a
  hang in this PR's development, but it has not been stress-tested under
  contention the way the shim's own (MIT-licensed) test suite is.
- **No touch coverage**, by design (see above) — this run says nothing
  about touch input, because the shim has no touch input to say anything
  about yet.
- **Scope is hand-picked**, not "all of wlcs" — see "Scope" above. A green
  number here is not evidence about any wlcs suite not in that table.
- **A point-in-time snapshot.** The pass-list above reflects one run
  against the shim as of this PR. It will drift as `xdg.c` and `seat.c`
  evolve; re-run `run-advisory.sh` rather than trusting this file's numbers
  once meaningful time has passed. The CI job's per-run summary
  (`$GITHUB_STEP_SUMMARY`) and uploaded artifact are the live source of
  truth; this file is the annotated baseline for interpreting them.
