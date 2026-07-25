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
  Phase 1 MVP slice), so there is nothing to bridge. **On wlcs 1.7.0 this
  crashes the runner** the moment a test actually reaches for the touch
  device — see "Known hazard" below. An earlier version of this file claimed
  wlcs treats a `NULL` touch factory as "skip touch-dependent tests"; that
  is not what happens, and it is also what the 32 SKIPs in the pass-list
  were incorrectly attributed to.
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
a real, structural absence (see above), not a bug this run would surface.
Any other wlcs suite (data device, subsurfaces, layer-shell, ...) is
excluded for the same reason: the shim doesn't implement the underlying
global, so the "failure" would just be "no such interface," which nobody
here needs wlcs to tell them.

**That exclusion is incomplete.** Excluding the `*Touch*` suites by name
does *not* exclude the touch-device *parameters* of the parameterised suites
that are in scope — `SurfaceInputRegions/SurfaceInputCombinations` is
instantiated over (surface type × input device), so half its parameters use
a touch device. On the wlcs version CI installs they fail before reaching
the touch device at all; on a newer one they crash the runner. See "Known
hazard".

## Known hazard: `create_touch` returning NULL crashes newer wlcs

With **wlcs 1.6.1**, the version Ubuntu 24.04 (noble) ships and therefore the
one the CI job installs, the scope above runs to completion. With **wlcs
1.7.0** (a later Ubuntu release's package) the runner **segfaults** partway
through, at
`SurfaceInputRegions/SurfaceInputCombinations.input_not_seen_in_region_after_null_buffer_committed/9`
— the `subsurface_at_x0_y0` surface type paired with a **touch** device —
and the remaining ~131 tests in scope never run.

What is established, on 1.7.0:

- The crash is reproducible on demand: `--gtest_filter` narrowed to that one
  parameter segfaults every time.
- It is the *first* test parameter that gets far enough to actually use a
  touch device; earlier touch parameters die on a protocol error before
  reaching `create_touch`.
- `AllSurfaceTypes/TouchTest.*` reproduces the same thing at the same point:
  the first parameter whose surface is created successfully.
- `gdb` puts every frame of the crash inside the `wlcs` runner binary, not
  inside `vitrin-shim-wlcs.so`. (The distribution binary is stripped, so
  there are no symbols — the frames are addresses in the main executable's
  mapping. That places the fault in wlcs, but does not name the function.)

The obvious explanation is that wlcs dereferences the `NULL` this module
returns from `create_touch`. That is consistent with every observation above
and with `WlcsTouch`'s ABI having no "unsupported" value to return instead,
but it is not *proven* here — proving it would need a wlcs build with
symbols. What *is* proven is that the behaviour this repository previously
documented — "wlcs treats a `NULL` touch factory as skip-these-tests" — does
not happen.

Why it matters even though CI's current wlcs survives it: the touch-device
*parameters* of the parameterised suites are in scope (see "Scope" above),
so the day the runner image's `wlcs` moves past 1.6.1 this job starts losing
most of its coverage to a crash. `run-advisory.sh` now prints
`status=aborted` and a warning in exactly that case, because a partial run's
`failed=` is a floor, not a tally — and `failed=0` from a run that died two
tests in reads the same as `failed=0` from a clean sweep.

## The pass-list, annotated

**Provenance.** wlcs 1.6.1-1 — the version in Ubuntu 24.04 (noble) universe,
which is what the `wlcs-advisory` job's `apt install wlcs` gets on
`ubuntu-latest` — driving a `vitrin-shim-wlcs.so` built from this tree
against system wlroots 0.19.3 and wayland 1.25.0,
`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`, full `run-advisory.sh`
scope, 2026-07-25.

```
total=180 passed=3 failed=145 skipped=32 status=complete
```

These are the same counts this file has carried since the harness landed,
reproduced here on a re-run. **The numbers were right. The root-cause
annotation attached to them was not** — see below.

**Passing (3):**

- `XdgSurfaceStableTest.supports_xdg_shell_stable_protocol`
- `WlOutputTest.wl_output_properties_set`
- `WlOutputTest.wl_output_release`

**Skipped (32):** all `SurfaceInputRegions/SurfaceInputCombinations.*`
parameters whose surface type is `wl_shell` or `zxdg_shell_v6`. wlcs prints
its own reason on the line above each one — `Missing extension: wl_shell>= 1`
(16) and `Missing extension: zxdg_shell_v6>= 1` (16) — and gates them on the
extension list this module advertises (`xdg_shell` v6, and nothing else).
This shim implements neither the deprecated `wl_shell` nor the unstable
`zxdg_shell_v6`, by design. Nothing to fix.

> Two earlier explanations of these 32 are retracted: they are not wlcs
> needing "more than one output mode / buffer-format combination", and they
> are not `create_touch` returning `NULL`. Both were wrong; the reason is
> printed in the log.

### Failing (145), by cause

**1. 128 failures — `"Wayland protocol error: 3 on interface xdg_surface v5"`
(wlroots' message: `xdg_surface has never been configured`), spread across
`SurfaceInputRegions/SurfaceInputCombinations` (100),
`XdgPopupStable/XdgPopupTest` (8), `ClientSurfaceEventsTest` (6),
`XdgToplevelStableTest` (5), the two `SurfacePointerMotionTest`
instantiations (4 + 4) and `XdgSurfaceStableTest` (1) — 128 exactly.**

> Correction: an earlier revision of this table said `XdgSurfaceStableTest`
> (2), which made the breakdown sum to 129 for a 128-failure cause. It is 1:
> only `creating_xdg_surface_from_wl_surface_with_existing_role_is_an_error`
> throws the `"Wayland protocol error: 3"` exception. That suite's four
> other failures are itemised under cause 3 below — including
> `creating_xdg_surface_from_wl_surface_with_committed_buffer_is_an_error`,
> which *does* raise a code-3 error but fails on the code-mismatch
> assertion rather than on an unexpected protocol error, and so belongs
> there and not here. Counting it twice is what produced the 129.
>
> To re-derive the split, attribute *tests*: each `[ RUN      ]` … result
> block whose body carries the exception. Two things will otherwise give you
> 129 rather than 128, and both are artefacts of the log, not of the shim.
> `grep -c` over the **finished** `wlcs-run.log` counts one extra hit
> because `run-advisory.sh` appends its own "Dominant failure categories"
> block to that same file, and that block quotes the exception string
> verbatim. Naive attribution of every matching line to the preceding
> `[ RUN      ]` then charges that appended line to whichever test ran last
> (`PointerCrossingSurfaceEdge/SurfacePointerMotionTest.pointer_movement/3`
> here), inflating that instantiation to 5. Stop attribution at each test's
> result line and both go away: the exception occurs exactly 128 times in
> the gtest output, once per failing test, which is also what
> `run-advisory.sh`'s own counter reports.

`xdg_surface` error 3 is `unconfigured_buffer` — "Attaching a buffer to an
unconfigured surface". `WAYLAND_DEBUG=1` traces show wlcs 1.6.1's client
reaching that state two different ways. `ClientSurfaceEventsTest
.surface_enters_output`, representative of the bulk:

```
 -> wl_compositor#5.create_surface(new id wl_surface#11)
 -> xdg_wm_base#7.get_xdg_surface(new id xdg_surface#9, wl_surface#11)
 -> xdg_surface#9.get_toplevel(new id xdg_toplevel#13)
 -> wl_surface#11.commit()                  <- initial commit: present
 -> wl_shm_pool#14.create_buffer(new id wl_buffer#15, ...)
 -> wl_surface#11.attach(wl_buffer#15, 0, 0)
 -> wl_surface#11.commit()                  <- no ack_configure in between
```

Every one of those requests is written before the compositor processes any
of them; there is no `xdg_surface.ack_configure` anywhere in the trace. And
`XdgSurfaceStableTest.gets_configure_event` — one of the five "other"
failures below — goes further: it does `get_toplevel`, `attach`, then a
`wl_display.sync`, with **no `wl_surface.commit` at all**, and asserts a
`configure` arrived.

Both are the same assumption: that the compositor sends
`xdg_surface.configure` without the client having performed the buffer-less
initial commit and acknowledged the result. xdg-shell-stable requires the
opposite, in the `xdg_surface` interface description:

> After creating a role-specific object and setting it up […], the client
> must perform an initial commit without any buffer attached. The compositor
> will reply with initial `wl_surface` state […] followed by an
> `xdg_surface.configure` event. **The client must acknowledge it and is
> then allowed to attach a buffer** to map the surface.

and

> any attempts by a client to attach or manipulate a buffer prior to the
> first `xdg_surface.configure` call must also be treated as errors.

wlroots 0.19.3 implements exactly that: `surface->configured` is set **only**
in `xdg_surface_handle_ack_configure`, and a commit carrying a buffer while
`!configured` is rejected with `unconfigured_buffer`
(`types/xdg_shell/wlr_xdg_surface.c`). The shim is right to reject these.

**Cross-version confirmation, against the same shim sources.** Rebuilt only
against wlcs 1.7.0's headers and run under the 1.7.0 runner — `shim/src`
untouched — the pass count goes from 3 to 8, picking up
`XdgSurfaceStableTest.gets_configure_event`,
`XdgSurfaceStableTest.creating_xdg_surface_from_wl_surface_with_existing_role_is_an_error`,
`ClientSurfaceEventsTest.frame_timestamp_increases` and
`ClientSurfaceEventsTest.surface_enters_output`, because 1.7.0's helpers
perform the initial-commit/configure/ack sequence. Nothing about the shim's
configure timing changed between those two runs.

> **The root cause previously recorded here was the exact inverse of the
> rule, and is retracted.** It claimed `xdg.c` gating the initial configure
> on the first `wl_surface.commit` was a spec violation and that the
> compositor should configure proactively at role assignment. That is wrong
> three times over: the spec mandates the ordering `xdg.c` already
> implements; wlroots makes the alternative unreachable
> (`wlr_xdg_surface_schedule_configure` opens with
> `assert(surface->initialized)`, and `initialized` only becomes true at the
> initial commit); and it would not even fix these tests, because the
> failing clients never send `ack_configure` at all, so `configured` would
> stay false no matter when the configure was sent.
>
> **Issue #128 was filed on that retracted premise. Its premise is invalid —
> do not implement it.** Doing so would move `xdg.c` from conformant to
> non-conformant while changing none of these results. The issue text itself
> is not edited by this change; someone with write access needs to close or
> rewrite it.

What is *not* established: whether wlcs 1.6.1's behaviour is a plain bug or
a deliberate accommodation of compositors that configure eagerly at role
assignment (Mir, wlcs's home compositor, being the obvious candidate), and
what exactly changed in 1.7.0. Settling that needs wlcs's own sources, which
this repository deliberately does not vendor. Either way there is nothing
for `xdg.c` to change.

**2. 12 failures — `"Timeout waiting for condition"`**, all in
`XdgToplevelStableTest.{parent_can_be_set, null_parent_can_be_set,
when_parent_is_set_to_{self,child_descendant}_error_is_raised,
pointer_respects_window_geom_offset, touch_respects_window_geom_offset}` (6)
and `XdgToplevelStableConfigurationTest.{defaults,
activated_state_follows_pointer, window_can_{,un}maximize_itself,
window_can_{,un}fullscreen_itself}` (6).

These need either independently-positioned toplevels (the parenting and
geometry-offset tests — `position_window_absolute` is a no-op here) or
configure-driven state transitions the shim's fixed
single-maximized-activated-toplevel policy never produces. That
correspondence is inferred from the test names against `xdg.c`'s documented
layout policy ("LAYOUT IS ONE RULE"), **not** verified test by test with a
trace the way cause 1 was. It is consistent with Phase 1 having no window
manager.

**3. 5 failures, individually triaged.** Small, concrete, and the only ones
here that point at shim-side work:

- `XdgToplevelStableTest.wm_capabilities_are_sent` — a gmock
  `EXPECT_CALL(toplevel, wm_capabilities)` that is never satisfied. The shim
  advertises `xdg_wm_base` version 6 but never sends
  `xdg_toplevel.wm_capabilities`, which v5+ clients are entitled to. A
  genuine, narrow conformance gap in `xdg.c`.
- `XdgSurfaceStableTest.gets_configure_event` — see cause 1; on 1.6.1 this
  test never commits, so no conformant compositor can pass it. It passes on
  wlcs 1.7.0 against the same shim.
- `XdgSurfaceStableTest.creating_xdg_surface_from_wl_surface_with_attached_buffer_is_an_error`
  — "Expected protocol error not received". Creating an `xdg_surface` from a
  `wl_surface` that has a buffer *attached but not yet committed* is
  supposed to be an error; the shim accepts it.
- `XdgSurfaceStableTest.creating_xdg_surface_from_wl_surface_with_committed_buffer_is_an_error`
  — the error *is* raised, with a different code than wlcs expects:
  `xdg_wm_base` error 3 (`invalid_popup_parent`) carrying the message
  "xdg_surface must not have a buffer at creation", where wlcs expects 4
  (`invalid_surface_state`). That code comes from wlroots 0.19.3, not from
  anything under `shim/src`.
- `XdgSurfaceStableTest.attaching_buffer_to_unconfigured_xdg_surface_is_an_error`
  — "Expected protocol error not received": the one test that *wants*
  `unconfigured_buffer` does not get it, while 128 others get it unasked.
  Not triaged.

Net: of the 148 tests that actually exercise the shim, 128 fail on a test
client that skips the xdg-shell bring-up sequence the shim is required to
enforce, 12 on an accepted Phase-1 policy limitation (inferred, not traced),
and 5 individually — of which one (`wm_capabilities`) is a real, actionable
`xdg.c` gap and one is a wlroots error-code mismatch.

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

Prints a `total=/passed=/failed=/skipped=/status=` summary and a breakdown
of the most common failure messages, and writes a full gtest log plus JUnit
XML into the output directory (default `./wlcs-advisory-out/`). **Always
exits 0** — see below.

`status=` is the part to read first:

| `status` | Meaning |
|---|---|
| `complete` | the runner printed its end-of-run summary; the counts are its own tally |
| `aborted` | the runner died mid-suite (see "Known hazard"); the counts are only what finished before that, and everything after is counted nowhere |
| `no-output` | no test ever started — almost always the module failing to `dlopen`. **Not** a clean run |

Without that word, `failed=0` from a run that crashed on its second test is
textually identical to `failed=0` from a clean sweep. Distinguishing them is
the whole point of the field.

## Testing the harness itself

```sh
bash shim/wlcs/test-summary.sh
```

Self-test for the log parsing (`summary.sh`) and for `run-advisory.sh`
end-to-end. No wlcs package, no built module, no compositor, no GPU — it
replays real wlcs captures checked in under `testdata/` and drives
`run-advisory.sh` against a stub runner. It is a real test: it exits
non-zero on failure, unlike `run-advisory.sh`.

It exists because the counting patterns are matched against a format
nothing here controls — the wlcs runner's own gtest event listener, which
does *not* print stock googletest output (`[     SKIP ]`, not
`[  SKIPPED ]`; `N tests failed:`, not `N tests, listed below:`). A pattern
that stops matching does not fail loudly, it reports zero. Every fixture in
the self-test therefore asserts non-zero failure and skip counts, so a
rotted pattern takes the test red with it. `summary.sh` additionally derives
every count twice — from the per-test lines and from the end-of-run summary
block — and warns on stderr when the two disagree.

Three things about that arrangement are load-bearing in a way the
summarised counts alone do **not** pin, so the self-test asserts each of
them directly:

- **Every pattern, in both dialects.** Each `WLCS_RE_SUM_*` is asserted
  against both a wlcs capture and a stock-googletest log. A summary pattern
  that matches only the wlcs spelling changes no printed number (the
  per-test fallback covers it) while silently collapsing the two
  extractions to one for that dialect — at which point the disagreement
  canary compares one source against itself and can never fire.
- **De-duplication.** wlcs prints every failed and skipped test twice, and
  only the trailing `(N ms)` anchor separates the live line from the
  end-of-run re-listing. The parser's de-duplicated per-test counts are
  asserted directly, because on a *complete* log the summary block wins and
  those counts never reach the output — so nothing else would notice the
  anchor going away until the next aborted run double-counted.
- **The stderr diagnostics.** Both the `ABORTED` warning and the
  disagreement canary are asserted as stderr output, plus the negative: a
  consistent complete run must print nothing at all.

Not wired into `meson test` or CI: `shim/meson.build` and
`.github/workflows/ci.yml` are outside this directory. Run it by hand after
touching `summary.sh` or `run-advisory.sh`.

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
  about yet. And the touch parameters that remain in scope are a standing
  hazard rather than merely uninformative, see "Known hazard".
- **Scope is hand-picked**, not "all of wlcs" — see "Scope" above. A green
  number here is not evidence about any wlcs suite not in that table.
- **A point-in-time snapshot.** The pass-list above reflects one run
  against the shim, dated and attributed in that section. It will drift as
  `xdg.c` and `seat.c` evolve; re-run `run-advisory.sh` rather than trusting
  this file's numbers once meaningful time has passed. The CI job's per-run
  summary (`$GITHUB_STEP_SUMMARY`) and uploaded artifact are the live source
  of truth; this file is the annotated baseline for interpreting them.
- **Version-sensitive in both directions.** The same shim scores 3/180
  against wlcs 1.6.1 and (before crashing) 8/49 against wlcs 1.7.0, with no
  shim change in between — see the pass-list. A number from this harness
  means nothing without the wlcs version beside it, which is why the
  pass-list leads with provenance.
