# Headless integration suite — CI entry-point contract

This directory hosts the Phase-1 integration tests: spawn `vitrind
--headless`, drive it with scripted Python SDK tests (plan
`docs/plan/01-phase-1-mvp.md` §2; needs P1.3.x headless + P1.8.x SDK).

The CI `integration` job (`.github/workflows/ci.yml`) is **dormant** until
the suite lands. Its activation contract — the PR that lands the first
integration test must satisfy this, or update this file *and* the workflow
together:

- **Entry point:** `tests/integration/run.sh`, invoked by CI as
  `bash tests/integration/run.sh`. Exit `0` = pass, anything else = fail.
  The job's steps are gated on this exact path via `hashFiles()`; a guard
  step fails the job if other files land in this directory without it, so
  the gate cannot silently drift.
- **Budget:** the `run.sh` step is capped at **10 minutes**
  (`timeout-minutes: 10` — the P1.9.1 acceptance criterion). CI runs
  `cargo build --workspace` beforehand as an untimed warm-up step, so
  `run.sh` should *reuse* the already-built binaries (plain `cargo run` /
  `target/debug/vitrind` is fine — the build cache is warm) rather than
  budgeting for a cold compile.
- **Environment:** GPU-less `ubuntu-latest` runner — pixman rendering +
  shm buffers only (plan §6 D3); nested mode is never a CI dependency
  (§7 R1). Toolchain from `rust-toolchain.toml`; runner `python3` is 3.12
  (satisfies the SDK's `>=3.11` floor, D8).
- **Later occupants:** this job also hosts the M1.5 gates — demo job
  (P1.8.4), golden frames (P1.9.2), hostile-client tests (P1.9.3) — behind
  the same entry point.
