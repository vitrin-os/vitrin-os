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
| `test_demo.py` | The **M1.5 named acceptance gate** (P1.8.7, #110): `examples/agent-demo/run_demo.py`'s headless venue, imported and run against the real chain — `vitrind` → real `vitrin-shim` → real `form-target`, never `vitrin-mock-shim` — with the process spine asserted by ancestry and the clicks/types recorded verbatim at the chokepoint. **The demo is goal-directed and the criterion is a positive content check, not a diff.** The agent is handed a task record it did not author, locates each field by its marker colour in its own capture, clicks, types, confirms ink landed *inside* that field, clicks the located submit button, and then the gate demands that the confirmation frame carry three full-width bands whose colours are exactly what **this task's** 36-bit checksum produces, in order (`examples/agent-demo/README.md` is the normative encoding) — plus the complement, that a *wrong* task's bands do **not** match that same frame — plus the app's own byte-exact `SUBMIT … canon=<hex>` line, the out-of-band ground truth beside the pixels. Every earlier headless threshold ("≥ 400 changed px and a ≥ 64 px dense run", and the 24 px before it) was **deleted, not reworded**: those numbers were derived against `weston-terminal` glyph cells and swapping the app invalidates the derivation. **Two disclosures.** (1) `form-target` is **repo-authored** (`shim/tests/form_target.c`) where `weston-terminal` was third-party; it is a real Wayland client and is neither `vitrin-mock-shim` nor `mock_core.c`, so D12 holds literally, but "the app is written by the same repo that asserts on it" is fair criticism — mitigated by the `click-target` precedent in the M1.4 gate and by the third-party rungs below staying green. (2) The receipt is a **checksum, not glyph recognition**: the agent reads back a function of the record, never the characters. Binary-free classes ride along: `DemoUsesNoMockShim` (grep-proof), `ReceiptEncodingIsPinned` (the Python reference vs. the C and JS restatements), `ReceiptDecodingIsDiscriminating` (wrong/reordered/thin bands rejected), `TaskInputIsValidated`, `DefaultTaskAgreesAcrossLaunchers`, and `ChangeProfileShapeMetrics` — which now pins the **focus-ring trap**: a ring drawn *inside* the field is rejected by the inset rectangle, and the same ring measured *without* the inset is accepted, proving which mitigation does the work. Same C-shim env contract; the nested venue (real Firefox) is workstation-only (`shim/docs/firefox.md`). | **Yes — M1.5** |
| `test_real_app.py` | The **M1.2 exit gate** (P1.9.6, #105): the whole real chain — real `vitrind` → real C shim → real `weston-terminal` — with no mock on any seam. Skips without a built C shim; see the env contract below. | **Yes — M1.2** |
| `test_real_gtk.py` | The GTK rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real `gtk-entry-probe`, reusing `test_real_app.py`'s real-app mode. Supporting evidence for M1.2's render half, alongside `test_real_firefox.py`. | Supporting — M1.2 |
| `test_real_firefox.py` | The Firefox rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real pinned Firefox ESR, asserting a real rendered colour and the globals contract, with no mock on any seam. Supporting evidence for M1.2's render half. | Supporting — M1.2 |
| `test_real_capture_fidelity.py` | The **M1.3 exit gate** (P1.8.5, #107): an agent captures a real `solid-client` frame through the real chokepoint; its dominant colour is the served colour, it agrees with the core-internal capture (`vitrind --capture-dump`) by SSIM + per-pixel tolerance via `vitrin-golden-cmp`, and capture-path rate-limit + expiry refuse as `rate_limited`/`expired`. Same C-shim env contract. | **Yes — M1.3** |
| `test_real_actuation.py` | The **M1.4 actuation gate** (P1.8.6, #108): an agent's `grant.pointer` click lands on a real `click-target`'s observed feature (dominant colour flips, D10) and `grant.text` types `héllo→世界` intact into a real `gtk-entry-probe` (D7), each confirmed by the agent's own `observe()` and recorded at the chokepoint. Same C-shim env contract; the GTK rung skips without GTK. M1.4 additionally needs #109, whose **hold-Esc half** is the `test_real_deadman.py` row below and whose **consent half** is the `test_real_consent.py` row below it (#138) — with the scope that gate deliberately does *not* cover recorded under "What the consent gate still does not prove". | **Yes — M1.4 (actuation half)** |
| `test_real_deadman.py` | The **M1.4 dead-man gate** (P1.7.4, #109): a completed hold-Esc chord, applied over a real `click-target` through the real core, revokes a live grant — `observe()` and `grant.pointer.click()` both refuse `Revoked` on the very next check, the real app's target stays unflipped (read from `--capture-dump`, bypassing the now-revoked grant entirely), and the flight recorder journals `dead_man_triggered` then `grant_revoked`. Headless has no physical key to hold, so a `SIGUSR1` to the core (only meaningful on a `dead-man-injector`-feature `vitrind` — see `run.sh`) stands in for the hold; the nested recipe for a *real* held Escape is `shim/docs/firefox.md` §9. Same C-shim env contract as the rest of the real-app ladder. | **Yes — M1.4 (dead-man half)** |
| `test_real_consent.py` | The **M1.4 consent gate** (P1.7.5, #138): a real petition raises a real core-rendered prompt over a real `click-target`; the prompt occludes the human-visible output (the exported footprint is first shown to *be* a card raster at exactly the rectangle the core named — accent ring on all four edges, exact perimeter count, opaque body, buttons, antialiased text — and then to carry zero of the app's target pixels) and never the capture path (the realm-view dump is **byte-identical** to a settled control taken while the app was watched idle, and the agent's own mid-prompt `observe()` still shows the full target and agrees with that dump through `vitrin-golden-cmp`); a mid-prompt actuation on an *already-granted* grant, on a **second connection of the same principal**, refuses `ConsentHeld` **specifically**, the app's own surface stays green in the core-internal dump, and the chokepoint's `consent_held` record falls strictly between the prompt's `shown` and its resolution in the journal; then the identical click lands after the denial (the positive control, last, because `click-target`'s flip is one-way). The decision is resolved by `PetitionRegistry::resolve_human` — asserted as `issuer: "human_consent"` — and the run brands itself `consent_policy: "interactive+consent-injector"`. Headless has no pointer for a human to click with, so an inherited socketpair named by `--consent-injector-fd N` on a `consent-injector`-feature `vitrind` stands in for the click; see "What the consent gate still does not prove" below. Same C-shim env contract as the rest of the real-app ladder. | **Yes — M1.4 (consent half)** |
| `test_real_trust_band.py` | The **trusted-band property gate** (issue #139, refs #85) — mock-free, real-app, and deliberately **not** a milestone gate: plan §5 adjudicated unspoofability out of M1.4's criteria, so this closes a tracked gap on its own terms rather than adding to a closed milestone. A real `click-target`, driven by the agent through the real chokepoint, repaints its **whole surface** — the trusted band's rows included — from black to red, which is the strongest counterfeit available to something that cannot observe the colour it would have to match. Two claims: the band's rows are exactly the app's own colour in **both** capture artifacts (the agent's `observe()` frame and the core-internal `--capture-dump`), before and after, and both of the app's colours carry a channel below the indicator's `[64, 255]` floor so that is proof rather than a 1-in-7-million coincidence; and the core-side witness reports `band_changes == 0` over every composite **it evaluated** (that this is every composite of the session is a fact about `BandWitness::observe` being called from `HeadlessOutput::present`, the backend's single composition path — code, not this gate). The zero is held up by counterweights in the same reply — `probe_changes` increases across the repaint, `composites` rises by **at least two** across a span containing exactly one `band` request (the bound is the point: `answer_band` recomposites before reporting, so a read pays for one composite itself and a bare "it went up" would be satisfied by a witness wired only into the reply path), `tracks_view` refuses a frozen or erased human-visible framebuffer, `band_uniform` refuses a partly-overdrawn or blended band, `refusals == 0`, and `probe_fnv` (a digest of *realm-view* rows just below the band) must equal the digest the harness computes over its own dump of the same instant. **The harness never learns the indicator colour**, and the rule the witness holds to is stricter than "export no pixels": every field must be a constant function of the run, independent of the secret's value — `band_witness.rs`'s `a_report_does_not_depend_on_the_bands_colour`, and the same check again over prompt-up composites in `…_with_a_card_up`, pin that mechanically. Same C-shim env contract as the rest of the ladder; rides #138's `consent-injector` channel and feature, so it needs no new CI wiring. **It proves a negative, not that the band is unforgeable to a human's eye** — see below. | No — property gate (#139) |
| `test_consent_injector.py` | **Component test.** The `consent-injector` channel's fail-closed matrix (no card up, a button the card does not draw, an unknown or spent token, an unparseable line, an over-long line, the peer disappearing) against the real core + `vitrin-mock-shim`. Explicitly **not** a milestone gate: it exists so the gate above stays about the milestone property rather than about the channel's error handling. | No |

### What the consent gate still does not prove

`test_real_consent.py` closes the gap this section used to describe. Two
things it deliberately does **not** close, stated here rather than left to
be discovered:

- **Unspoofability (issue #85) is not gate-level evidence *here*, and will
  not be.** This gate never learns this session's trusted-indicator colour,
  at all. That secret is never written to any descriptor or file in any
  build, and the pixels the instrumented core exports are exactly the
  consent card's own footprint — which `consent/mod.rs`'s `card_rect` and
  its `the_card_footprint_carries_no_indicator_pixel` test prove is
  indicator-free, because the trusted ring is stroked strictly *outside*
  it and the opaque card is blitted last. **So the gate proves occlusion;
  it does not prove the card is framed in a colour a confined app cannot
  forge.** **Adjudicated 2026-07-25: unspoofability is not an M1.4
  criterion** — neither the milestone table nor its verification list in
  `docs/plan/01-phase-1-mvp.md` §5 names it — **so M1.4 is closed, and the
  gap is tracked separately as #139** rather than folded into a closed
  milestone. Do not cite M1.4 as evidence that the trusted indicator is
  unforgeable.
  (A whole-frame mirror would have let this gate check the band — and would
  have written the session secret to a file a same-uid app can read, which
  is precisely what `consent/indicator.rs` forbids. The smaller claim is
  the honest one.)

  **#139 has since split that gap in two, and closed the half that can be
  closed.** `test_real_trust_band.py` (row above) is a mock-free real-app
  gate for the **negative** half: a confined app's own rendering never
  reaches the band's rows on the human-visible output and never reaches the
  capture path at all. It is *not* a milestone gate and does not reopen
  M1.4. The **positive** half — that a human who learned the colour off the
  band can tell a genuine prompt's frame from a forgery — is
  **permanently component-and-human evidence**: `consent/mod.rs`'s band and
  frame tests, `backend/headless.rs`'s
  `a_prompt_reaches_human_visible_output_but_never_a_capture` and the
  real-app `c_shim_consent_prompt_occludes_…`, `backend/winit.rs`'s
  `no_presentation_path_can_drop_the_trusted_band`, and, for a human,
  `shim/docs/firefox.md` §9's nested recipe. It needs an eye at a real
  display and there is no automatable form of it, so it is recorded as
  closed-to-CI rather than left as an open question.
- **The physical click is not proven here.** The hit test, the 500 ms
  `GUARD_INTERVAL`, the press-arms/release-commits ladder, and the origin
  check that stops an agent answering its own prompt are proven only by
  `crates/vitrin-core/src/consent/grab.rs`'s own tests — which drive the
  private `judge_parts` with real events, including
  `an_agent_cannot_answer_the_prompt_it_petitioned_for` — and by
  `shim/docs/firefox.md` §9 with a human at a mouse. The injector bypasses
  `judge` completely, and the headless router still stacks `NoopHook`, so
  no input of any origin can reach that grab. This is the same shape as
  `test_real_deadman.py`'s SIGUSR1 standing in for a held Escape.

The gate also says nothing about the human-visible frame *outside* the
card footprint (not the scrim, not the ring), and nothing about whether
the card is legible or names the right principal — `consent/render.rs`'s
golden and sourcing tests hold that.

With `click-target` the first of those is **structural, not an omission
an extra assertion could close**, and it is worth knowing which: the
export is clamped to the card's footprint and the card is blitted opaque
last, so those bytes do not depend on the frame beneath them; and the
realm view *outside* that footprint is, measured on the gate's own
artifacts, 93 840 px of one colour (black — the app paints black except
for one centred 160×160 square the card wholly covers). A regression that
erased the realm view from the human-visible output altogether therefore
moves no pixel a human sees in this scenario; the exported window comes
back byte-identical, sha256 and all, which was confirmed by running it.
That defect is caught, at component level, by `backend/headless.rs`'s
`a_prompt_reaches_human_visible_output_but_never_a_capture` (full-bleed
test pattern, bottom-left asserted scrimmed-not-erased) and
`backend/winit.rs`'s `the_nested_window_uploads_the_consent_overlay`.

What the gate *does* now establish about those exported bytes, and did
not before the P1.7.5 repair pass, is their **provenance**: they are
checked to be a raster of vitrind's card at exactly the rectangle the
core named — accent ring on all four edges, its exact perimeter count,
`CARD_BG` over most of the body, both button colours present, and enough
distinct colours to carry antialiased text — *before* the absence of the
app's green is read out of them. An absence over bytes of unproven origin
is satisfied just as well by an empty buffer: an export that never read
the framebuffer at all used to pass this gate, printing its success line
verbatim, and now fails it on the first edge pixel.

`crates/vitrin-core/src/backend/headless.rs`'s
`c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture`
remains valuable and remains a **component** test: it is mock-free on the
app seam but builds a `HeadlessView` and a `ShimServer` in-process instead
of driving the shipped binary, which plan §5 D12 disqualifies as milestone
evidence. Cite `test_real_consent.py` for the milestone, that test for the
property in isolation.

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
  `click-target` too, and needs one extra cargo feature on the `vitrind` this
  job already builds. The M1.4 consent gate (`test_real_consent.py`) reuses
  `click-target` as well and needs a second one, so the list is
  `cargo build --workspace --features
  vitrin-core/dead-man-injector,vitrin-core/consent-injector` (both the "Warm
  build" step and `run.sh`'s own fallback build pass exactly that string, and
  the milestone-gate-drift guard asserts they agree): the SIGUSR1 handler that
  stands in for a completed hold-Esc chord, and the socketpair channel that
  stands in for a human clicking Allow, on a physical-input-free runner.
  Neither feature does anything by itself at runtime — the consent one is
  additionally inert unless the invocation carries `--consent-injector-fd N`.
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
- **The named gates must exist.** `run.sh` carries two lists —
  `MILESTONE_GATES` and `PROPERTY_GATES` — and fails before any test runs if
  one of those modules is absent; CI's "Guard against milestone-gate drift"
  step asserts the same two lists plus the `INJECTORS=` feature line in
  seconds. `unittest discover` cannot tell a gate that was never written from a
  green suite — nothing collected, nothing failed, exit 0 — which is the exact
  shape issue #138 was filed on. Editing the gate table above means editing the
  matching list in the same commit. The split into two lists is not
  bookkeeping: a milestone gate's absence is a claim about that milestone's
  definition of done, and `test_real_trust_band.py` is deliberately not one —
  plan §5 adjudicated its property out of M1.4's criteria, so it is named,
  guarded, and never cited as milestone evidence.
- **Later occupants:** this job also hosts the rest of the M1.5 gates —
  golden frames (P1.9.2), hostile-client tests (P1.9.3) — behind the same
  entry point. The demo gate (P1.8.4/P1.8.7, `test_demo.py`) has landed; it
  adds no new CI wiring, reusing the real-app ladder's `VITRIN_C_SHIM_BIN`
  contract exactly as `test_real_app.py` does. It needs **no distro package
  at all** since the goal-directed rewrite: its app, `form-target`, is
  co-built with the C shim (`shim/meson.build`) rather than installed, so its
  absence beside a built shim is a build misconfiguration the gate **fails**
  on rather than a machine state it skips for. `VITRIN_FORM_TARGET_APP`
  overrides the path, the way `VITRIN_CLICK_TARGET_APP` does for the M1.4
  gate's app.

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
