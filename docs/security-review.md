# Reviewing Vitrin OS for security

This page is for anyone reviewing this tree for vulnerabilities: a person, or
an automated tool, including one driven by a language model. It says where
untrusted input enters, how to build and exercise the tree offline, how a
finding might be rated, and what a useful report looks like.

It is a guide, not a policy. [`SECURITY.md`](../SECURITY.md) is the policy,
and where the two disagree, `SECURITY.md` wins. Nothing here widens or
narrows its scope.

## Read SECURITY.md first

Two sections of it are normative for any review, and this page does not
restate them:

- **"The trust boundary is the scope boundary"** says what is in scope (the
  trusted core, the transport, the confinement helper, the decoders, the
  protocol's design) and what is out of scope (the shim on its own, test
  scaffolds, the Python SDK as a TCB target, third-party dependencies). It
  also lists the findings that would be especially valuable.
- **"Known gaps that are not findings"** lists what the project already says
  out loud. Two things in that section are themselves findings: a gap that
  turns out materially worse than documented, and documentation that
  overclaims.

Two more reading rules come from the same file. The README's
[Security notes](../README.md#security-notes--what-the-mvp-does-and-does-not-confine)
describe what the shipped code does; [`docs/PRD.md`](PRD.md) describes what
the design intends. Where they differ, the README is the one telling the
truth. And this is a pre-1.0 project that makes no security guarantees yet.

## Where untrusted input enters

Each entry point below is taken from `SECURITY.md`, the threat model in
[`docs/PRD.md`](PRD.md) §15, and
[`docs/protocol/00-conventions.md`](protocol/00-conventions.md). None is
added here.

- **A principal's connection.** Any process of this uid can connect to
  `$XDG_RUNTIME_DIR/vitrin-0/core.sock` and send bytes. Those bytes reach
  `vitrin-ipc`'s framing and `SCM_RIGHTS` fd matching, then the generated
  decoders in `vitrin-protocol`, before any identity or grant check has
  happened. The handshake's credential goes to a pluggable verifier, and
  `SO_PEERCRED` is recorded at accept. Authority is sender-constrained to the
  triple of connection, verified credential and peer credentials (conventions
  §1.3).
- **An agent that holds grants.** PRD §15's hijacked or prompt-injected agent
  acts within exactly the grants it holds. Every route by which it does more
  -- a capture or actuation that skips `enforcement.rs`, a grant outliving its
  expiry, revocation or sender constraint, a handle used on another
  connection -- is in scope.
- **A shim's connection.** The shim speaks over a socketpair the core hands
  it at fork; holding that socketpair is the credential (conventions §1.2). A
  hostile shim is expected input. Only the crossing is in scope: affecting
  another realm, reaching into the core, or forging a realm identity the core
  assigned.
- **Realm spawn.** What a realm inherits across the fork (descriptors,
  environment), and what `vitrin-realm-init` confines it to. `SECURITY.md`
  describes the outside verification the core performs; a path past it is a
  core finding.
- **The consent surface.** The core-rendered prompt, its exclusive input grab,
  and the origin tag that separates physical input from injected input. A
  client that can draw over, occlude or spoof the prompt, escape the grab, or
  reach an app tagged `physical` is in scope.

## Building and exercising the tree offline

[`.github/security-review/Dockerfile`](../.github/security-review/Dockerfile)
builds an image with everything fetched up front. Build it from the
repository root:

```sh
docker build -f .github/security-review/Dockerfile -t vitrin-review .
```

Inside it the checkout is at `/src`, the pinned Rust toolchain and
`cargo-fuzz` are on `PATH`, the shim is built at `shim/build/`, and a Python
venv with the SDK installed is first on `PATH`. Nothing below needs the
network. Set `CARGO_NET_OFFLINE=true` if you want cargo to say so rather
than try.

**Rust.** `cargo test --workspace` runs the unit tests and the in-process
integration tests. It is not the whole suite.
[`CONTRIBUTING.md`](../CONTRIBUTING.md) explains why: the tests that drive the
real C shim skip unless `VITRIN_C_SHIM_BIN` is set, so run them as well:

```sh
VITRIN_C_SHIM_BIN="$PWD/shim/build/vitrin-shim" cargo test -p vitrin-core c_shim
```

**Skips.** The confinement and Landlock tests need unprivileged user
namespaces and a Landlock ABI the host may not grant. In a VM or container
that withholds them, they skip and print a marker. The image does not set the
`VITRIN_REQUIRE_*` variables that turn such a skip into a failure, because
those are claims about a CI runner. **A skip is not a pass.** The invocation
the `rust` job in `ci.yml` runs itemises every skip a run took:

```sh
cargo xtask skip-census --min-tests 1200 --expect-self-marker \
  -- cargo test --workspace -- --show-output
```

`cargo xtask skip-scan` lists every sanctioned skip site in the tree.

**Fuzzing.** `fuzz/` holds two cargo-fuzz targets over the two crates that
parse untrusted bytes before any check: `protocol_decode` and `ipc_framing`.
Always give libFuzzer a scratch corpus first, so the checked-in corpus is not
grown by accident:

```sh
cargo fuzz run --sanitizer none protocol_decode "$(mktemp -d)" fuzz/corpus/protocol_decode -- -max_total_time=600
cargo fuzz run --sanitizer none ipc_framing     "$(mktemp -d)" fuzz/corpus/ipc_framing     -- -max_total_time=600
```

`--sanitizer none` is deliberate: ASan needs a nightly toolchain, and these
targets look for panics and round-trip divergence, which libFuzzer catches on
stable. [`fuzz/README.md`](../fuzz/README.md) explains the trade and when it
would stop being a good one.

**The shim.** `meson test -C shim/build --suite vitrin-shim` runs its own
tests. Remember that the shim is outside the TCB; what matters is what it can
do to the core.

One of those suites, `designation-relay`, fails where CPU is scarce, for
example in a container capped at two CPUs. Its bulk arm sends a thousand
designations a millisecond apart, and a reader that falls a few hundred
behind fills the socket's send buffer. The shim then drops the connection on
`EAGAIN`. That drop is a deliberate rule, which the same script's arm (G)
proves. The failure is a test that depends on host speed, tracked in
[#354](https://github.com/vitrin-os/vitrin-os/issues/354). It is not a finding.

**The Python SDK.** `python -m pytest sdk/python/tests` runs its unit tests
against a scripted mock server. The SDK trusts the core it connects to, so
its bugs are ordinary issues, not TCB findings.

**The shipped binary.** [`tests/integration/`](../tests/integration/) drives
`target/debug/vitrind` over a real socket with real forked realms, which is
the evidence this project trusts most. Run it with `bash
tests/integration/run.sh`; it finds the shim at `shim/build/` by itself. Its
real-app gates need real Wayland clients, and at the default isolation they
need user namespaces. Where the host refuses those, `vitrind` refuses to
start rather than start weaker. That refusal is designed behaviour, not a
finding, and a finding reproduced only at `--isolation=off` is triaged as
`SECURITY.md` says: as a finding about an explicitly unconfined session.

## Rating severity

> **Proposal, not yet signed off by the maintainer.** Until it is, the rating
> in the published advisory is the one that counts, and this scale is only a
> suggestion of what a report should claim.

The scale follows `SECURITY.md`'s list of especially valuable findings.
Anything out of scope stays out of scope, however it would rate.

- **Critical.** A capture or an actuation that does not pass through
  `enforcement.rs`. A capture that returns another realm's pixels. A grant
  that survives its expiry, its revocation or its sender constraint. A client
  that draws over, occludes or spoofs the core-rendered consent prompt, or
  escapes its input grab. Input that reaches an app tagged `physical` when it
  did not originate physically. A confinement that is reported as applied and
  is not. A hostile shim crossing into the core or into another realm.
- **High.** An unprivileged client that crashes or stalls the compositor loop
  for everyone. A descriptor or environment variable that survives the spawn
  fork it should not.
- **Medium.** An in-scope defect that weakens a documented property without
  reaching the outcomes above.
- **Low.** Hardening with no demonstrated path to any of the above.

Two adjustments. A documented gap that is materially worse than its
documentation is rated by what it actually allows, not by the fact that a gap
was documented. A documentation overclaim is rated by the property a reader
would wrongly rely on, not as low by default.

## What a useful report looks like

`SECURITY.md` lists what a report should name: the commit, the binary and
backend, the realm and principal configuration, and what the attacker
controls at the start. Beyond that:

- **A reproducer.** A failing test under `tests/integration/` is the gold
  standard. A Rust test, or a fuzz input with its target name, is also good.
  `SECURITY.md` accepts a prose walkthrough from a person; an automated
  report must carry something runnable.
- **A patch, when you have one.** Keep it minimal, and add a regression test
  that fails before the patch and passes after it. A fuzz crash becomes a
  permanent regression input under `fuzz/corpus/<target>/`, as
  `fuzz/README.md` describes.
- **The rule it breaks.** Cite the IDL `<description>` text in
  [`protocol/vitrin-v0.xml`](../protocol/vitrin-v0.xml), which is the source
  of truth for every interface; the decision or open question in
  [`docs/plan/20-decision-log.md`](plan/20-decision-log.md); or the sentence
  in `SECURITY.md`, the README or the
  [limits page](book/src/limits.md) that the finding shows to be false.

Automated and model-generated reports are welcome through the same private
channel, on the terms `SECURITY.md` sets out. A tool's severity rating is
read as a suggestion. Model-generated reports have been known to inflate
severity and to misread a project's threat model, which is part of why this
page exists.
