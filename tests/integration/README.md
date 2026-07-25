# Headless integration suite — CI entry-point contract

Phase-1 integration tests: spawn `vitrind --headless`, drive it with the
Python SDK, assert on the wire and on the flight recorder (plan
`docs/plan/01-phase-1-mvp.md` §2).

**Status: active.** The CI `integration` job (`.github/workflows/ci.yml`)
runs this suite on every PR.

## Why this exists next to `cargo test --workspace`

It is not redundant with the unit tests, and the distinction is the point.
The in-process tests in `crates/vitrin-core/src/session.rs` build a runtime
by calling `start_realm_in` directly — so they never execute `run_session`,
the function that orders startup in the shipped binary.

That leaves one class of regression invisible to them: **startup ordering**.
Issue #77's trap T1 is the example. If a change ever registers the shim
socketpair's event source *after* the fork, the shim blocks on `configure`
forever, every real session wedges permanently and silently, and
`cargo test --workspace` stays entirely green. Only a test that runs
`target/debug/vitrind` catches it.

Everything here therefore drives the **shipped binary** over a real Unix
socket with a real forked realm. Nothing constructs a runtime in-process.

## Layout

**Definition-of-done note (issue #111, plan §5 D12):** only the rows marked
**mock-free milestone gate** below may be cited as evidence that M1.2–M1.5 is
done. Every other test here is a **component test**: it drives the shipped
`vitrind` binary (so it still catches the startup-ordering and wiring bugs
`cargo test --workspace` structurally cannot — see above), but it does so
against `vitrin-mock-shim` on the far seam, which the mock-free gates must
never use as their evidence source. Component tests stay green and keep
their value; they are just never a substitute for the named gate.

| File | Role | Mock-free milestone gate? |
|---|---|---|
| `run.sh` | CI entry point. Bash, exit 0 = pass. | — (harness) |
| `harness.py` | `Core` — boots the binary in a throwaway `XDG_RUNTIME_DIR` (defaults its shim to `vitrin-mock-shim`, `MOCK_SHIM`, unless a real shim path is passed); `IntegrationTest` — per-test deadline and core reaping. | — (harness) |
| `test_runtime_wiring.py` | **Component test.** Issue #77's acceptance criteria (startup-ordering trap T1) against the real core + `vitrin-mock-shim`. Never a milestone gate — it predates and is orthogonal to M1.2–M1.5. | No |
| `test_actuation.py` | **Component test.** P1.8.3 (#42): the SDK's actuation API and typed grant-error exceptions against the real core + `vitrin-mock-shim`. Superseded, for milestone purposes, by `test_real_actuation.py` below. | No |
| `test_demo.py` | The **M1.5 named acceptance gate** (P1.8.7, #110): `examples/agent-demo/run_demo.py`'s headless venue, imported and run against the real chain — `vitrind` → real `vitrin-shim` → real `weston-terminal`, never `vitrin-mock-shim` — with the process spine asserted by ancestry, the click/type recorded verbatim at the chokepoint, and "the page changed" proven against a **settled control capture** — one the app is then watched *idle* through, for at least as long as the gate later polls, so an app that repaints steadily on its own fails the run instead of forging its evidence — by a change carrying the shape only the typed line makes: enough pixels *and* a **densely inked run** of them along one scanline (pixels chained at ≤ 24 px, ≥ 25 % of the run inked), not a bare pixel count, which weston-terminal's own startup paint clears unaided, and not the changed pixels' *bounding span*, which three unrelated one-cell repaints at opposite ends of a scanline clear while drawing nothing anyone typed. Three more binary-free test classes ride along: `DemoUsesNoMockShim` grep-proves neither `crates/xtask` nor `run_demo.py` constructs `vitrin-mock-shim`, `HeadlessGateThresholdsStayDiscriminating` pins the thresholds' ordering so the gate cannot be relaxed back into vacuity, and `ChangeProfileShapeMetrics` pins the shape predicate itself against in-process frames — including that scattered-one-cell pair, asserted to be rejected. Same C-shim env contract; the nested venue (real Firefox) is workstation-only (`shim/docs/firefox.md`). | **Yes — M1.5** |
| `test_real_app.py` | The **M1.2 exit gate** (P1.9.6, #105): the whole real chain — real `vitrind` → real C shim → real `weston-terminal` — with no mock on any seam. Skips without a built C shim; see the env contract below. | **Yes — M1.2** |
| `test_real_gtk.py` | The GTK rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real `gtk-entry-probe`, reusing `test_real_app.py`'s real-app mode. Supporting evidence for M1.2's render half, alongside `test_real_firefox.py`. | Supporting — M1.2 |
| `test_real_firefox.py` | The Firefox rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real pinned Firefox ESR, asserting a real rendered colour and the globals contract, with no mock on any seam. Supporting evidence for M1.2's render half. | Supporting — M1.2 |
| `test_real_capture_fidelity.py` | The **M1.3 exit gate** (P1.8.5, #107): an agent captures a real `solid-client` frame through the real chokepoint; its dominant colour is the served colour, it agrees with the core-internal capture (`vitrind --capture-dump`) by SSIM + per-pixel tolerance via `vitrin-golden-cmp`, and capture-path rate-limit + expiry refuse as `rate_limited`/`expired`. Same C-shim env contract. | **Yes — M1.3** |
| `test_real_actuation.py` | The **M1.4 actuation gate** (P1.8.6, #108): an agent's `grant.pointer` click lands on a real `click-target`'s observed feature (dominant colour flips, D10) and `grant.text` types `héllo→世界` intact into a real `gtk-entry-probe` (D7), each confirmed by the agent's own `observe()` and recorded at the chokepoint. Same C-shim env contract; the GTK rung skips without GTK. M1.4 additionally needs #109, whose **hold-Esc half** is the `test_real_deadman.py` row below and whose **consent half is not covered by any gate in this directory** — see "M1.4's open consent gap" below. | **Yes — M1.4 (actuation half)** |
| `test_real_deadman.py` | The **M1.4 dead-man gate** (P1.7.4, #109): a completed hold-Esc chord, applied over a real `click-target` through the real core, revokes a live grant — `observe()` and `grant.pointer.click()` both refuse `Revoked` on the very next check, the real app's target stays unflipped (read from `--capture-dump`, bypassing the now-revoked grant entirely), and the flight recorder journals `dead_man_triggered` then `grant_revoked`. Headless has no physical key to hold, so a `SIGUSR1` to the core (only meaningful on a `dead-man-injector`-feature `vitrind` — see `run.sh`) stands in for the hold; the nested recipe for a *real* held Escape is `shim/docs/firefox.md` §9. Same C-shim env contract as the rest of the real-app ladder. | **Yes — M1.4 (dead-man half)** |

### M1.4's open consent gap

Issue #109 has two halves — "consent occlusion over a real app" and
"hold-Esc revocation over a real app". This directory covers the second
(`test_real_deadman.py`) and **not the first**. `test_real_deadman.py`
never raises a consent prompt: it takes a grant, fires the dead-man chord,
and asserts the refusals. No `test_real_*.py` module in this directory
puts a consent prompt on screen over a real app at all.

The consent-occlusion evidence that *does* exist is
`crates/vitrin-core/src/backend/headless.rs`'s
`c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture`.
It is genuinely mock-free on the app seam — real C shim, real
`click-target`, real pixels — and it is a real proof of the property. But
it is an **in-process Rust test**: it builds a `HeadlessView` and a
`ShimServer` directly instead of driving the shipped `vitrind` binary,
which is exactly what the top of this file says no gate here does, and
what plan §5 D12 disqualifies as milestone evidence. So:

> **M1.4's consent half has component-level evidence, not gate-level
> evidence.** Do not cite `test_real_deadman.py` for it, and do not cite
> the Rust test as milestone acceptance. Closing the gap means a
> `test_real_consent.py` here that drives the shipped binary, raises a
> prompt over a real app (the build-gated `scripted-consent` feature is
> the headless stand-in, as `dead-man-injector` is for the hold), and
> asserts the same split: the prompt in the human-visible output, absent
> from the agent's capture.

Stated plainly because the previous version of this row claimed the
consent half was "covered by the `test_real_deadman.py` row below", which
it never was.

Grep-proving the split (run from repo root): every named-gate module boots
its `Core` with an **explicit real shim path** (`shim=str(self.shim_bin)`,
resolved from `VITRIN_C_SHIM_BIN`), never a bare `self.core()` — a bare call
defaults to `harness.MOCK_SHIM`. The `no`/mock mentions those files do
contain are disclaiming prose and assertion strings ("no `vitrin-mock-shim`
in the path"), not the shim they actually run:

```bash
# Every named-gate module passes an explicit shim= path to Core(); none
# relies on Core()'s harness.py default (vitrin-mock-shim). Expect: no output
# (a file listed here would be one that never overrides the mock default).
# test_demo.py is named explicitly: it is the M1.5 gate, and the
# `test_real_*.py` glob does not match it -- which is how this check used to
# skip the one gate file whose mock-freeness it most needed to prove.
rg --files-without-match 'shim=str\(self\.shim_bin\)' \
  tests/integration/test_real_*.py \
  tests/integration/test_demo.py
```

**Mock-freeness is not discriminating power.** Both checks above answer
"what is this test wired to", never "can this test fail". Two gates in this
repo were mock-free and still could not fail on the property they named — the
M1.5 demo gate asked for 24 changed pixels, which the real app's own startup
paint clears; the real-app consent-occlusion proof waited only for the view
to stop being the empty test pattern, which the shim's first commit satisfies
before any client attaches. Before citing a gate as evidence, break the
behaviour it claims to prove and watch it go red.

## Running it locally

```bash
bash tests/integration/run.sh
```

Builds the workspace if `target/debug/vitrind` or
`target/debug/vitrin-mock-shim` are missing, then runs the suite. No
virtualenv, no `pip install`.

## Entry-point contract

The job's steps are gated on this exact path via `hashFiles()`; a guard step
fails the job if other files land in this directory without it, so the gate
cannot silently drift.

- **Entry point:** `tests/integration/run.sh`, invoked by CI as
  `bash tests/integration/run.sh`. Exit `0` = pass, anything else = fail.
- **Budget:** the `run.sh` step is capped at **10 minutes**
  (`timeout-minutes: 10` — the P1.9.1 acceptance criterion). CI runs
  `cargo build --workspace` beforehand as an untimed warm-up step, so
  `run.sh` reuses the already-built binaries rather than budgeting for a
  cold compile.
- **Environment:** GPU-less `ubuntu-latest` runner — pixman rendering + shm
  buffers only (plan §6 D3); nested mode is never a CI dependency (§7 R1).
  Toolchain from `rust-toolchain.toml`; runner `python3` is 3.12 (satisfies
  the SDK's `>=3.11` floor, D8).
- **Python dependencies: none.** Stdlib only, `unittest` rather than pytest,
  SDK imported off `PYTHONPATH`. The job installs no Python packages and the
  SDK is zero-runtime-dependency by design (D8), so this suite needs no
  Python setup step and cannot rot when one drifts. Keep it that way — a
  `pip install` here means editing the workflow too.
- **Native dependencies:** the job installs `libxkbcommon-dev
  libpixman-1-dev`, which `vitrind` links (winit and headless backends).
  For the real-app gate it also runs `shim/ci/install-deps.sh` (Meson +
  wlroots build deps + weston), builds the C shim into `${RUNNER_TEMP}/shim-build`,
  and passes its path as `VITRIN_C_SHIM_BIN`. The M1.3 fidelity gate
  (`test_real_capture_fidelity.py`) needs no *new* CI wiring: its `solid-client`
  app is co-built with the shim by the same `meson compile` (resolved as a
  sibling of `VITRIN_C_SHIM_BIN`, like `gtk-entry-probe`), and its
  `vitrin-golden-cmp` SSIM tool is built by the `cargo build --workspace`
  warm-up that already builds `vitrind`. The M1.4 actuation gate
  (`test_real_actuation.py`) adds no CI wiring either: its `click-target` app is
  co-built with the shim, and it reuses the `gtk-entry-probe` the GTK rung
  already builds. The M1.4 dead-man gate (`test_real_deadman.py`) reuses
  `click-target` too, and needs one extra flag on the `vitrind` this job
  already builds: `cargo build --workspace --features
  vitrin-core/dead-man-injector` (both the "Warm build" step and `run.sh`'s
  own fallback build pass it) — the SIGUSR1 handler that stands in for a
  completed hold-Esc chord on a physical-input-free runner.
- **The real-app gate's opt-in knob:** `test_real_app.py` runs only when
  `VITRIN_C_SHIM_BIN` names a built C shim (`shim/build/vitrin-shim`). Unset,
  it **skips** — the local-dev path for anyone without the C toolchain. Set,
  a missing shim or missing `weston-terminal` is a **failure**, not a skip:
  CI sets the variable, so CI can never reach the skip, and a requested gate
  that skipped silently would prove nothing. `VITRIN_SKIP_REAL_APP=1` is the
  explicit local opt-out. Same variable name as the `conformance` job and
  `crates/vitrin-core/src/shim.rs`'s cross-track test. Run it locally with:

  ```bash
  meson setup shim/build shim && meson compile -C shim/build
  VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" bash tests/integration/run.sh
  ```
- **Later occupants:** this job also hosts the rest of the M1.5 gates —
  golden frames (P1.9.2), hostile-client tests (P1.9.3) — behind the same
  entry point. The demo gate (P1.8.4/P1.8.7, `test_demo.py`) has landed; it
  adds no new CI wiring, reusing the real-app ladder's `VITRIN_C_SHIM_BIN` /
  `weston-terminal` install exactly as `test_real_app.py` does.

## Two invariants worth keeping

Both were learned rather than anticipated, and both live in
`harness.IntegrationTest`:

- **Every test has a hard deadline** (`TEST_TIMEOUT_S`). A wedged shim makes
  `observe()` block forever; without the deadline the suite would hang until
  CI's 10-minute cap and report a nameless timeout — the worst possible
  reporting for the exact bug this suite exists to catch. With it, a wedge
  fails as a named test whose message points at trap T1.
- **Every core is reaped, pass or fail.** A test that failed between
  spawning a core and cleaning it up used to orphan a `vitrind` and its
  shim, which kept composing while later tests ran.

Related: `ANIMATE_FRAMES` in `harness.py` is a **CPU budget, not a
duration** — headless has no output clock, so a paced shim composes as fast
as the runtime loop dispatches it, and the frame count is the only thing
bounding how long it spins.
