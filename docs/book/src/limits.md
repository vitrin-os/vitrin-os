# Where this is honest about its limits

Phase 1 is complete. That is a statement about a defined slice closing on
named, mock-free gates — not a statement that this is ready for anything
real. This page is the whole list, in one place, so you never have to
discover an item on it yourself.

## Do not deploy this yet

**There is no sandbox.** Decision D9. No namespaces, no seccomp, no
Landlock. A realm's app runs as the core's own uid with the core's full view
of the filesystem and the network. An app that ignores `WAYLAND_DISPLAY` and
connects to a path it already knows is not stopped by anything here. The
session D-Bus remains reachable in practice.

Environment hygiene confines the well-behaved; it does not contain the
hostile. Do not run untrusted applications, or untrusted agents, against
this. Real sandboxing is Phase 2 (E2.6/E2.7, P13).

## Testing gaps

**The 24-hour fuzz soak has never been run.** `fuzz/` ships two cargo-fuzz
targets with a checked-in corpus that CI replays on every PR, plus a short
per-PR burst. The 24-hour clean run the plan asks for is a documented manual
procedure, not a scheduled job, and nobody has executed it end to end.

**wlcs conformance is advisory and mostly red.** The 2026-07-25 run:
`total=180 passed=3 failed=145 skipped=32`.

That number needs its context, and the context is not an excuse. wlcs tests
a general-purpose desktop compositor. The shim deliberately serves a narrow
surface — no touch, no full `xdg-shell` policy, no decoration protocols — so
most failures are "no such global" rather than misbehaviour, and the
excluded touch tests are excluded for a structural absence rather than an
expected failure. `shim/wlcs/README.md` separates the two categories
honestly. But it is still the real number, it has **not** been re-measured
since that date, and a partial run's `failed=` count is a floor rather than
a tally. It never gates a PR and is never built by default.

**dmabuf zero-copy is proven by an env-gated test, not by CI.** The path is
implemented and wired on the nested backend. The zero-memcpy assertion needs
a real GPU (EGL + a DRM render node) and runs only under
`VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests --
--ignored dmabuf`. CI is GPU-free and exercises the shm path exclusively.

## Model gaps

**The trusted indicator is unforgeable, not necessarily noticed.** There is
a rigorous gate proving a client cannot counterfeit the band. There is no
evidence that a human *notices* when it is wrong — that needs user research
nobody has done. The plan explicitly adjudicated unspoofability out of
M1.4's criteria for exactly this reason. Do not cite the milestone as
evidence for the human half.

**Several realms run; only one is visible, and captures cannot tell them
apart.** A `realm.toml` may now declare up to 16 realms, and each gets its
own shim process, its own private runtime tree and its own Wayland socket.
What it does *not* get is its own view: the core composites **one output
from one scene holding at most one client surface**, so with two realms
running only the one that committed last is on screen — and an agent's
capture is of that output rather than of the realm its grant names. While
two realms are **live**, a capture taken under a grant over one can
therefore carry the other's pixels. Binding an output to a realm is the next
task in this workstream; until it lands, treat a multi-realm configuration
as "several apps running", not as "several apps observable". A single-realm
configuration — the default, and what every shipped example uses — is
unaffected.

The exposure stops at *live* realms, and that boundary is enforced rather
than incidental: liveness is judged against the realm a grant names, so a
grant over a realm whose app has died refuses `no_surface` however busy its
siblings are. A dead realm can never be photographed through a sibling.

**Input goes to one realm, and an agent that names another is refused.**
There is one input router and one physical-presence tracker for the whole
session, so physical input reaches the first realm in id order and a human
touching the keyboard preempts agent actuation in every realm at once.

An agent's actuation is different, because it is *authorized against a named
realm*: delivering it to whichever realm the router happens to serve would
drive an app the grant confers no authority over. So the core refuses it
instead. With more than one realm configured, **only grants over the realm
the session's seat currently serves can actuate**; every other actuation is
refused `internal` (the IDL's "server-side failure during this use …
delivery"), recoverably, with nothing delivered. Observation is unaffected —
a capture addresses no seat. This is a stopgap that fails closed, not a
routing policy: per-realm routing is deferred with the view binding above,
and replaces the refusal. A single-realm configuration never meets it.

**No realm enumeration on the wire, and no runtime launch.** A client
cannot ask what realms exist; `realm-0` is the one name it can know without
being told. `realm_launch` is defined on the wire and served by no
deployment, so realms come only from the configuration file the core reads
at startup. Multi-realm *fleet* mode — a 50-realm headless box — is
Phase 3 and is a different thing again.

**Identities are static tokens.** Listed in `principals.toml`. The IDL is
shaped for SPIFFE/OIDC credentials; the machinery is not here yet.

**No semantic layer.** Agents work on pixels. The AccessKit/AT-SPI2 bridge,
versioned and diffable semantic trees, and epoch/CAS action semantics are
all Phase 2 — which is to say the token-hungry screenshot loop this project
criticises is still what an agent does against it today. The difference so
far is authorization, not efficiency.

**No X11 shim.** Wayland only. Per-app X11 with an embedded WM is Phase 3.

**The protocol will break.** v0 is frozen for Phase 1, not forever.

## Project gaps

**One maintainer.** Governance is a documented BDFL. Bus factor is tracked
as a first-class project risk rather than waved away; the standing
mitigations are spec-first artifacts, a design-doc-per-subsystem rule, and a
review norm against cleverness in the TCB.

**No OIN membership yet.** The project files no patents and relies on
defensive publication plus the Apache-2.0 §3 and MPL-2.0 §2.1(b) grants,
which are in force today. Joining the Open Invention Network is decided and
not yet done. None of this is a freedom-to-operate opinion.

**SPDX header coverage is not machine-checked.** There is no `reuse
lint`-style CI gate, so a new file without a header will not be caught
automatically.

## Why this page exists

From the project's own security notes: *a half-believed confinement claim is
worse than an honest gap.* Every item above is a recorded decision with a
scheduled closure, not an oversight — see
[the decision log](https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md).

If you find something true that belongs on this page and is not here, that
is a bug worth reporting, and it will be treated as one.
