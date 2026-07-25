# `vitrin-fuzz` — cargo-fuzz targets for the wire protocol and transport

P1.9.3 (issue [#46](https://github.com/vitrin-os/vitrin-os/issues/46)):
security-critical fuzzing of the two crates that parse untrusted bytes
before any identity or grant check has happened. Two layers of assurance,
per that issue's design decisions:

1. **This directory** — in-process `cargo-fuzz` (libFuzzer) targets over
   `vitrin-protocol` and `vitrin-ipc`. No socket to a live `vitrind`, no
   process spawn: the "zero-I/O crate split" (`docs/PRD.md` Doc 2 §2) is
   exactly what makes these two crates fuzzable this way.
2. **`tests/integration/test_hostile_client.py`** — the "hostile client
   vs. a live `vitrind`" half: real process, real Unix socket, asserting
   the *policy* (connection killed, core survives, other clients
   unaffected), not the decoder's memory safety. See that file's module
   docstring.

This crate is deliberately **not** a member of the root Cargo workspace
(own `[workspace]` table in `fuzz/Cargo.toml`) and is not exercised by
`cargo test --workspace`, `cargo clippy --workspace`, or the `rust` CI
job — fuzzing tooling has its own build profile (sanitizer/coverage
instrumentation) that has no business touching the main workspace's
lockfile or compile times.

## Targets

| Target | Crate under test | What it drives |
|---|---|---|
| `protocol_decode` | `vitrin-protocol` | Every one of the 29 generated `<message>.decode(bytes, fd)` functions, directly, with arbitrary bytes and an arbitrary fd presence. Pure in-memory: no socket at all. |
| `ipc_framing` | `vitrin-ipc` | `Connection::recv_message`'s frame reassembly and `SCM_RIGHTS` fd matching, over a real `socketpair(2)` fed with arbitrary byte streams split across two raw `sendmsg(2)` calls with arbitrary attached-fd counts. |

Each target's own doc comment (top of its `fuzz_targets/*.rs` file) is the
normative description of its input layout and what it asserts — read that
before changing either one.

## Why `--sanitizer none`

Plain `cargo fuzz build`/`run` asks for an ASan-instrumented build, which
needs `-Z sanitizer=address` — nightly-only. This workspace pins an exact
**stable** toolchain (`rust-toolchain.toml`, deliberately, so
`cargo xtask codegen --check` never drifts on an unrelated rustfmt
release) and does not want a second, nightly-pinned toolchain just for
this directory. Every command below therefore passes `--sanitizer none`,
which drops `-Z sanitizer=address` but keeps the part that actually drives
libFuzzer's coverage-guided mutation (`-C passes=sancov-module` and the
SanitizerCoverage counters/tables, all stable-compatible flags) — so
mutation quality is unaffected, only memory-corruption-via-ASan detection
is traded away. That trade is a reasonable one specifically for the code
these two targets actually link: `vitrin-protocol` has no crate-level
`unsafe` at all, and this crate's `vitrin-ipc` dependency is pinned to
its `client`-only feature slice (`default-features = false, features =
["client"]` in `fuzz/Cargo.toml`) — `Connection`/`Listener`/framing/
`SCM_RIGHTS`, none of which contains an `unsafe` block; the raw syscalls
underneath are `rustix`'s, behind its own audited `unsafe`. (The one
`unsafe` block `vitrin-ipc` does have lives in `event_loop.rs`, behind
the `server` feature this crate deliberately does not enable — the
calloop glue is core-side event-loop plumbing, not decode/framing logic,
and is out of scope for these targets.) So the bug class ASan exists to
catch barely applies here; what these targets are for is **panics** (an
unhandled `unwrap`/`assert`/index/arithmetic overflow reachable from
untrusted bytes) and **logic divergence** (a byte sequence that decodes
successfully but violates the round-trip property) — both of which
libFuzzer catches today, on stable, with no sanitizer at all. If a future
change adds real `unsafe` to the `client` slice (or a target grows to
cover `server`), install a nightly toolchain
(`rustup toolchain install nightly`) and drop `--sanitizer none` from
every command below (and from `ci.yml`'s job) to restore ASan coverage;
nothing else here needs to change.

## Running locally

A single `CORPUS` positional argument after the target name is where
libFuzzer writes newly-discovered "interesting" inputs -- give it nothing
and cargo-fuzz defaults to `fuzz/corpus/<target>` itself, which means an
ordinary mutation run silently grows the curated, checked-in corpus by
hundreds of auto-named files. **Always pass an explicit scratch directory
first** when actually fuzzing (as opposed to replaying); the checked-in
corpus is then given as a second, read-only-in-effect argument that only
seeds the run. Replaying a single named file (no directory in play at all)
never writes anywhere, which is why the regression commands below pass
files directly with no scratch dir.

```bash
cargo install cargo-fuzz   # once

# Fast smoke run (what CI's per-PR step does — see ci.yml's `fuzz-smoke` job).
# $(mktemp -d) is the corpus libFuzzer is allowed to grow; fuzz/corpus/<target>
# only ever seeds it.
cargo fuzz run --sanitizer none protocol_decode "$(mktemp -d)" fuzz/corpus/protocol_decode -- -max_total_time=60
cargo fuzz run --sanitizer none ipc_framing     "$(mktemp -d)" fuzz/corpus/ipc_framing     -- -max_total_time=60

# Replay one seed/regression file without fuzzing (exit 0 = did not crash):
cargo fuzz run --sanitizer none protocol_decode fuzz/corpus/protocol_decode/valid_hello

# Replay the whole checked-in corpus once each, as a fast regression pass
# (this is what a crash-fixing PR should run before it lands):
for f in fuzz/corpus/protocol_decode/*; do cargo fuzz run --sanitizer none protocol_decode "$f"; done
for f in fuzz/corpus/ipc_framing/*;     do cargo fuzz run --sanitizer none ipc_framing     "$f"; done
```

## The 24-hour pre-M1.5 campaign

The plan (`docs/plan/01-phase-1-mvp.md` §5) gates M1.5 on "24 h clean run"
of both targets. This is **not** run per-PR (way too slow for CI) and is
**not simulated** by anything in this repo — it is a real, long-running
job a maintainer runs once before cutting the M1.5 milestone, reproducible
by anyone from exactly these commands:

```bash
# One target at a time (each pins one core; run both in parallel on a
# multi-core machine by launching two terminals/tmux panes/background jobs).
# Each gets its own scratch corpus (grows to thousands of files over 24h --
# deliberately NOT fuzz/corpus/<target>, which stays the small curated set;
# see "Running locally" above for why) seeded from the checked-in one.
mkdir -p fuzz/campaign-logs fuzz/campaign-corpus/protocol_decode fuzz/campaign-corpus/ipc_framing
nohup cargo fuzz run --sanitizer none protocol_decode \
  fuzz/campaign-corpus/protocol_decode fuzz/corpus/protocol_decode -- \
  -max_total_time=86400 \
  > fuzz/campaign-logs/protocol_decode.log 2>&1 &
nohup cargo fuzz run --sanitizer none ipc_framing \
  fuzz/campaign-corpus/ipc_framing fuzz/corpus/ipc_framing -- \
  -max_total_time=86400 \
  > fuzz/campaign-logs/ipc_framing.log 2>&1 &
```

**Exit criterion:** both processes reach `DONE` in their log with no
`ERROR`/`SUMMARY: libFuzzer` crash report in between; `fuzz/artifacts/`
stays empty for both targets the whole run. The *result* that matters is
"no new file appeared under `fuzz/artifacts/<target>/`" — the log itself
is one machine's scratch output, not a repo artifact (see the note on
`fuzz/campaign-logs/` below).

If either run DOES produce a crash: `cargo fuzz fmt <target> <artifact>`
to see the input's structure, then copy the artifact file into
`fuzz/corpus/<target>/` under a descriptive name (`crash_<what-it-does>`,
matching the naming style in `seed_corpus.py`) and commit it — that
promotion from `fuzz/artifacts/` (ignored) to `fuzz/corpus/` (tracked) is
literally what "every crash becomes a permanent regression test" means
here: the next run (CI's short smoke job included, since it seeds from
this same checked-in corpus) replays it forever.

`fuzz/campaign-logs/` and `fuzz/campaign-corpus/` are both listed in
`fuzz/.gitignore` — a 24 h run's log and grown corpus are large and
specific to one machine/run, so `git status` cannot flag them by accident;
paste the one line of the log worth keeping (the final `DONE` summary)
into the M1.5 milestone tracking issue as the campaign's evidence instead.

## Seed corpus

`fuzz/corpus/<target>/` is checked in — see `seed_corpus.py`'s module
doc for what "checked in" means here (a small, hand-curated, named-per-
condition set, not the sprawling directory a long fuzzing run grows
locally) and how to regenerate it deterministically:

```bash
python3 fuzz/seed_corpus.py
```

### A seed that claims a path must reach it

A seed is named for one wire condition, and that name is the only thing a
reviewer reads — so a seed whose bytes stop reaching that condition is
invisible in a `git diff` and buys nothing, while still *looking* like
coverage. That happened twice, found on 2026-07-25:
`protocol_decode/attach_with_fd` carried a `fd_count = 0` header byte for
a message whose `HAS_FD` is `true`, so it died at `FdCountMismatch` on
every run instead of reaching the successful-decode path it exists to
seed; and `ipc_framing/unsolicited_fd` put its fd on a zero-length
`sendmsg`, which Linux drops on a `SOCK_STREAM` socket, so it replayed as
an ordinary valid frame and never seeded `PeerViolation::UnsolicitedFd`.

Two checks now make that class of rot fail loudly:

```bash
# Structural, no Rust toolchain: cross-checks every seed's header bytes
# against protocol/vitrin-v0.xml and both targets' input layouts, and
# requires every seed to be byte-distinct. Runs automatically on every
# regeneration; --check verifies what is already on disk.
python3 fuzz/seed_corpus.py --check

# Authoritative: feeds each seed file to the REAL decoder / a REAL
# vitrin_ipc::Connection over a REAL socketpair and asserts the outcome
# the seed's name claims. An ordinary `cargo test` — no nightly, no
# cargo-fuzz, no libFuzzer runtime (both `[[bin]]` targets set
# `test = false`), so anyone editing `seed_corpus.py` can run it.
cargo test --manifest-path fuzz/Cargo.toml
```

Adding a seed means adding its claim to both tables
(`PROTOCOL_DECODE_CLAIMS` / `IPC_FRAMING_CLAIMS`, one copy in each file);
a seed that claims nothing fails the checks, and so does a claim with no
seed. Neither check is wired into `ci.yml` yet — the `fuzz-smoke` job
replays the corpus but cannot tell which path a replay reached — so today
they are a documented local gate. Wiring `python3 fuzz/seed_corpus.py
--check` plus `cargo test --manifest-path fuzz/Cargo.toml` into that job
is the natural follow-up.

## CI

- **Every PR** (`ci.yml`'s `fuzz-smoke` job): both targets, `-sanitizer
  none`, a low `-max_total_time` (see that job's comment for the current
  number) — long enough to catch an obviously broken target or an
  immediately-reachable panic, short enough to never be the slow step in
  a PR. Seeded from the checked-in `fuzz/corpus/`, so every past
  regression replays on every PR.
- **Not scheduled yet.** The 24 h campaign above is presently a manual,
  documented, reproducible procedure (this section) rather than a
  scheduled GitHub Actions job — a 24 h *hosted-runner* job has real cost
  and queuing implications this repo has not made a call on yet. Wiring a
  `schedule:`-triggered job that runs the exact commands in the section
  above is a natural follow-up once that call is made; until then, "24 h
  clean run required before M1.5 exit" (the issue's acceptance criterion)
  is satisfied by a maintainer running this section's commands once,
  by hand, before cutting the M1.5 milestone, and recording the result
  (a `DONE` log line, `fuzz/artifacts/` still empty) in the milestone
  tracking issue.
