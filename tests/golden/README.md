# Golden frames and the `cargo xtask bless` flow

This repository pins several renders as **golden files** — checked-in
reference bytes a test recomputes and compares against, so a change that moves
a pixel it should not shows up as a failing test and a reviewable `git diff`.
This page is the single home for how those goldens are compared and, when a
change is deliberate, regenerated.

## The comparison harness — `crates/vitrin-golden`

`vitrin-golden` is the shared pixel-assertion library (P1.9.2). It has no
dependencies: its SSIM score and its PNG artifact encoder are hand-rolled, so
no image codec enters the tree (plan risk R7).

- **`Frame`** — a borrowed view over a tightly packed 4-byte-per-pixel image
  in one of the two layouts the codebase already speaks: `Frame::rgba(...)`
  (the `test_pattern::render` / headless-readback layout) or `Frame::xrgb(...)`
  (the `vitrin_view.frame_ready` wire layout).
- **`compare(actual, expected, policy) -> Report`** under one of three
  policies:
  - `Policy::Exact` — byte-for-byte. What deterministic integer renders use;
    a single differing channel fails.
  - `Policy::Tolerance { max_channel_diff, max_bad_fraction }` — allow a
    bounded per-channel difference on a bounded fraction of pixels.
  - `Policy::Ssim { min }` — structural similarity, the fallback where
    exactness is unreasonable (GPU output vs. a pixman software render: the
    same scene, different anti-aliasing).
  The `Report` carries the verdict plus every statistic (max channel diff,
  bad-pixel fraction, SSIM score) so a failing test can log a precise reason.
- **`write_artifacts(dir, actual, expected)`** — drops `actual.png`,
  `expected.png` and an amplified `diff.png` heatmap (black where the frames
  agree, bright toward the disagreeing channels) so a break is legible at a
  glance. Call it from the failing branch of a golden test.

## Regeneration — always through `cargo xtask bless`

Goldens change **only on purpose**, and only through one command:

```sh
cargo xtask bless                       # regenerate every golden
cargo xtask bless --filter consent      # only goldens whose test name contains "consent"
```

`bless` runs the golden tests with `VITRIN_REGEN_GOLDEN=1` set; each golden
test, seeing that variable, rewrites its committed file before asserting. This
supersedes running `VITRIN_REGEN_GOLDEN=1 cargo test ...` by hand — one
documented entrypoint covers every golden, old and new, as long as the test's
name matches the filter (all golden test names contain `golden`, so the
default filter catches them). After running it, **review `git diff`** and
commit the regenerated files together with the change that motivated them.

## The goldens

| Golden file | Regenerating test | Policy / representation |
|---|---|---|
| `tests/golden/headless_test_pattern_96x60.xrgb` | `capture::tests::headless_test_pattern_image_golden` | **Exact**, raw xrgb8888 bytes — the flagship harness consumer |
| `crates/vitrin-core/tests/golden/consent_prompt.txt` | `consent::tests::consent_prompt_golden` | Deterministic ink map + blake3 (bundled font, SIMD disabled) |
| `sdk/python/tests/golden/test_pattern_64x40.xrgb` | `capture::tests::sdk_capture_golden_file_pins_the_wire_bytes` | Exact, raw xrgb8888 — cross-language pin (Rust writes, Python consumes) |
| `crates/vitrin-core/tests/golden/lock_screen.txt` | `lock::tests::lock_screen_golden` | Deterministic ink map (one character per 8×8 block) + blake3, same bundled font and disabled SIMD |
| `crates/vitrin-core/tests/golden/status_strip.txt` | `status::tests::status_strip_golden` | Deterministic ink map (one character per 4×4 block) + blake3 |

The consent-UI golden is deterministic across CI runs because the font is
vendored and embedded and fontdue's architecture-dependent SIMD is disabled
(see `crates/vitrin-core/Cargo.toml`), so the same anti-aliased bytes are
produced on every machine.

## Acceptance (P1.9.2)

- A deliberate one-pixel scene change fails the affected golden —
  `capture::tests::a_one_pixel_change_fails_the_image_golden_and_writes_artifacts`
  proves the harness reports the failure and writes the three artifacts.
- The headless test-pattern golden is **Exact**; the consent-UI golden stays
  deterministic across CI runs; regeneration goes through `cargo xtask bless`.
