# Vitrin OS demo agent

`run_demo.py` is Phase 1's integrating demo agent — and, in its headless
venue, the **M1.5 acceptance test**. A single script exercises the whole MVP
slice against a live `vitrind`:

> connect (the static demo identity) → request the one MVP grant (observe +
> `actuate.pointer` + `actuate.text` on `realm-0`, `while-running`) → await
> consent → capture a *before* frame → locate the URL bar by pixels → click it,
> type a URL, press Enter → capture an *after* frame → assert the page changed.

The same agent code drives both venues; only *what stands in for the app* and
*how "the page changed" is proven* differ.

## Running it

The launcher is a Rust `xtask` that writes a throwaway one-principal registry
(`principals.toml`, mode 0600) and realm config, starts the shipped `vitrind`,
waits for its socket, runs this script against it, then tears the core down
with SIGTERM and prints the flight-recorder path.

```console
# Nested venue — needs a display and Firefox ESR:
cargo xtask demo

# Headless venue — no display, no browser, no GPU (this is the CI gate):
cargo xtask demo --headless
```

### Nested (`cargo xtask demo`)

A real Firefox ESR runs in `realm-0`. The script serves a deterministic
solid-colour page from a stdlib `http.server` bound to `127.0.0.1:0` on a
daemon thread, types that local URL into Firefox's URL bar, and proves the
navigation by the **dominant colour** of the after-frame matching the colour it
served (bucketed, so anti-aliasing does not matter). A human answers the
consent prompt by clicking **Allow**.

Firefox's path defaults to `/usr/bin/firefox-esr`; override with
`VITRIN_DEMO_FIREFOX=/path/to/firefox`.

### Headless (`cargo xtask demo --headless`)

The `vitrin-mock-shim`'s animated buffer stands in for the app, so CI never
depends on Firefox or a GPU (plan risk R6/R1). "The page changed" here means
the two captures differ (the animation advanced across the actuation
sequence). What proves the actuation *causally reached the app* is not pixels
but the **flight recorder**: an allowed `move` at the clicked coordinate and an
allowed `type` whose `chars` equals the typed URL's length plus one (the
trailing Enter) — the same evidence
`tests/integration/test_actuation.py` relies on. Consent is
`--consent=auto-approve`, sound only because the launcher's registry holds
nothing but the one demo principal.

## It doubles as the M1.5 acceptance test

`tests/integration/test_demo.py` imports this script's `run()` entry point and
drives the **headless** flow against a real `vitrind` (via `harness.Core`). It
asserts the demo returns success, that the two captures differ, and that the
flight recorder reconstructs the session in order — handshake/bind → petition →
resolution → capture(s) carrying frame digests → the allowed `move` at the
clicked coordinate → the allowed `type` with `chars == len(url) + 1`. Because
the integration suite's entry point is `unittest discover -p 'test_*.py'`
(`tests/integration/run.sh`), this gate rides CI with no workflow edit.

## Notes

- Pure stdlib, Python ≥ 3.11, zero runtime dependencies — the SDK's posture
  (decision D8). Frames are written with `Frame.to_png`; the local page is
  served with `http.server`. No Pillow, no requests.
- On any assertion failure the script saves the before/after frames as PNGs
  under `--out` and prints the flight-recorder path, so a failure is
  diagnosable from the artifacts alone.
