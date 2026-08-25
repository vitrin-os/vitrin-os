# Contributing to Vitrin OS

Contributions are welcome, including the ones that tell the project it is
wrong. This page is what you need before your first pull request.

If you are here to report a **security** issue, stop and read
[`SECURITY.md`](SECURITY.md) instead — vulnerabilities go through a private
channel, not a public issue.

## Sign your commits (DCO, not a CLA)

This project takes contributions under the
[Developer Certificate of Origin](https://developercertificate.org/) —
decision **D-012**. There is no Contributor License Agreement, no copyright
assignment, and none will be asked for. You keep your copyright.

That is a deliberate trade. A CLA would hand a single-maintainer project
unilateral power to relicense *your* code later, which is exactly the power
that makes the license split in [`NOTICE`](NOTICE) worth trusting — a
project that can relicense at will has made a promise it can revoke. The
DCO asks for something much smaller: that you have the right to submit what
you are submitting.

Adding the sign-off is one flag:

```sh
git commit -s -m "fix(c-shim): ..."
```

which appends

```
Signed-off-by: Your Name <your.email@example.com>
```

Forgot it? `git commit --amend -s` for the last commit, or
`git rebase --signoff main` for a branch. CI checks every commit in a PR
and will tell you which one is missing it.

## Before you open a pull request

- **Open or find an issue first** for anything beyond a typo. Work is
  tracked as issues, and the branch/commit conventions below reference them.
- **Branch name**: `p<phase>.<epic>.<task>-slug` — e.g.
  `p1.6.7-xdg-popup-configure`. The epic map is in
  [`docs/plan/01-phase-1-mvp.md`](docs/plan/01-phase-1-mvp.md).
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/),
  `type(scope): summary`. Types: `feat`, `fix`, `docs`, `refactor`, `perf`,
  `test`, `build`, `ci`, `chore`, `revert`. Scope is the track
  (`protocol`, `rust-core`, `c-shim`, `sdk`, `ci-docs`) or `root`.
  Reference the issue in the footer: `Closes #10` / `Refs #10`.
- **English only** — code, comments, docs, commits, issues, PRs.

## The two rules that are easy to break by accident

**1. A protocol change is a paired edit, never one alone.** Changing an
interface means editing *both*:

1. [`protocol/vitrin-v0.xml`](protocol/vitrin-v0.xml) (and
   `protocol/vitrin-v0.rng` only if the *dialect itself* changes), and
2. the matching [`docs/protocol/NN-vitrin_name.md`](docs/protocol/) prose
   page.

Then regenerate and validate, committing the generated output in the same
change:

```sh
xmllint --noout --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml
cargo xtask codegen
```

Read [`docs/protocol/00-conventions.md`](docs/protocol/00-conventions.md)
first — it defines the wire format, object-id rules, the
fatal-vs-recoverable error razor, and the Wayland-style growth rules. Where
prose and IDL disagree, **the IDL's `<description>` wins.**

**2. Never hand-edit a generated file, and never add MPL Exhibit B.**
Generated code comes from the templates in
[`crates/vitrin-scanner/`](crates/vitrin-scanner); hand-editing its SPDX
line turns `cargo xtask codegen --check` red. And Exhibit B anywhere in the
tree would make [`shim/wlcs/`](shim/wlcs) undistributable — the reasoning is
in [`NOTICE`](NOTICE).

## Licensing your contribution

The repository is split by derivation, not by directory, and
[`NOTICE`](NOTICE) is the normative path→license map. Read it before adding
or moving a file. In short: the protocol and everything derived from it plus
the SDKs are Apache-2.0; the trusted core, the transport and the shim are
MPL-2.0; spec prose is CC-BY-4.0; `shim/wlcs/` alone is GPL-3.0-only.

A contribution is taken under the license of the path it lands in. **A new
crate must declare its own `license` field** — the workspace-wide default
was deliberately deleted so nothing inherits the wrong half silently. New
first-party `.rs`, `.c`, `.h`, `.py`, `.sh` and `.js` files carry an inline
`SPDX-License-Identifier`. That coverage is not machine-checked yet, so
please do not rely on review to catch a missing header.

## Running the tests

```sh
cargo test --workspace                    # unit + in-process integration
cargo xtask codegen --check               # generated-code drift
xmllint --noout --relaxng protocol/vitrin-v0.rng protocol/vitrin-v0.xml

# The real-binary suite: needs the C shim built and a real Wayland client
meson setup shim/build shim && meson compile -C shim/build
VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" bash tests/integration/run.sh

# The Rust tests that drive the REAL C shim. `cargo test --workspace` above
# does NOT run these — it skips them (see below).
VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" cargo test -p vitrin-core c_shim
```

### `cargo test --workspace` is not the whole Rust suite

Read this before reporting a branch green. A plain `cargo test --workspace`
leaves `VITRIN_C_SHIM_BIN` unset, and the two cross-track tests that drive the
real C shim against the real Rust core — `c_shim_conforms_to_the_real_core`
and `c_shim_consent_prompt_occludes_..._the_real_apps_capture` — **skip
outside CI and fail inside it**. Locally they skip. So the workspace run
reports `ok` having never started a shim, and the first machine that actually
runs them is GitHub.

That is not hypothetical: three review rounds on
[#304](https://github.com/vitrin-os/vitrin-os/issues/304) each ran
`cargo test --workspace`, each reported green, and each missed a broken
geometric expectation in `c_shim_consent_prompt_occludes_...` that CI caught
on the first push. It is the same class as
[#288](https://github.com/vitrin-os/vitrin-os/issues/288) and
[#229](https://github.com/vitrin-os/vitrin-os/issues/229) — a green count
standing in for absent evidence — reaching the seam nobody re-ran.

The `c_shim` line in the block above is the fix, and
`cargo xtask skip-census --min-tests 2 -- cargo test -p vitrin-core c_shim --
--show-output` is what the `conformance` job itself runs, which additionally
fails if either test skipped. Two further sets run on no default local
invocation at all and are worth knowing about before touching anything
geometric: `dmabuf::gpu_tests::*` (`#[ignore]`d; needs a real GPU —
`VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests --
--ignored dmabuf`), and the confinement and Landlock tests, which skip on a
host whose kernel or userns policy cannot support them. `cargo xtask
skip-scan` enumerates every sanctioned skip site in the tree.

The two `meson` lines create `shim/build/`, and `cargo test --workspace`
passes in a tree that has it. That was not true until
[#295](https://github.com/vitrin-os/vitrin-os/issues/295): `xtask`'s
repository-scanning gates walked the directory and reported meson's copy of
wlroots as if it were this project's source, so the crate failed for anyone
who had followed the lines above. Those gates — `cargo xtask limits-check`,
`cargo xtask skip-scan` and the tests behind them — now ask `git` which paths
are build output instead of guessing from directory names, which means they
read the tree CI's checkout reads, and it means they need a git work tree to
run at all.

CI builds with `RUSTFLAGS="-D warnings"` and the toolchain is pinned exactly
in [`rust-toolchain.toml`](rust-toolchain.toml) — the pin is exact because
`codegen --check` compares output byte-for-byte and that depends on
rustfmt's exact decisions.

## What gets a change rejected

Only a few things, and none of them are about style:

- **Cleverness in the trusted core.** `crates/vitrin-core` and
  `crates/vitrin-ipc` are the TCB. Boring, obvious code is a review norm
  there, not a preference — cleverness in a capability kernel is a liability
  twice over.
- **A test that proves less than it claims.** This matters more here than
  in most projects: a milestone closes only on a named, mock-free gate
  (decision **D12**), and the repo has already caught two of its own gates
  passing vacuously. If a test's name and its assertion disagree, the
  assertion is the bug.
- **Citing a mock as evidence.** [`vitrin-mock-shim`](crates/vitrin-mock-shim)
  and `shim/tests/mock_core.c` are component-test fixtures. They are useful
  and they are kept — but they are never acceptance evidence for a
  milestone, and a PR that says otherwise will be asked to relabel.

## Governance

A documented single maintainer (BDFL), stated plainly rather than dressed up
— see [`docs/plan/12-workstream-community.md`](docs/plan/12-workstream-community.md)
§5. A `GOVERNANCE.md` with decision process and maintainer-addition rules is
triggered by three regular contributors or the first funded contributor,
whichever comes first.

Response times are best-effort and this is an unfunded project. If a PR goes
quiet, a ping on the issue is welcome, not rude.
