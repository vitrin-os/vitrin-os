# Workstream A — Spec & standards

The protocol outlives any implementation ([PRD](../PRD.md) §9, bus-factor mitigation). This workstream turns the PRD's Doc 2 into a published, versioned, reviewable protocol specification — on a schedule that follows running code rather than preceding it (D-014 in [20-decision-log.md](20-decision-log.md)).

## 1. Artifact split

- Spec prose: **CC-BY-4.0**; wire/IDL definitions and schemas: **Apache-2.0 with explicit patent grant** (D-005).
- Eventually a separate repo, `vitrin-os/protocol` — the licensing boundary and the "protocol outlives the implementation" argument both want a clean cut. Until the split is worth its overhead (first external reviewer or implementer, whichever comes first), the spec lives in-tree at `docs/protocol/` (created in Phase 1, task P1.9.5) with the licensing split marked per-directory.

## 2. Module-freeze ladder (the key sequencing call)

The spec is published early — PRD §8 Phase 0 requires an object-model and wire-protocol draft — but versioned `0.x` and explicitly tracking the implementation. Rationale: the anti-Arcan posture (§9: running code before prose authority) and the PRD's own caveat that epoch/CAS is "a design claim, not a proven result" — freezing it before E2.3 measures it would enshrine guesswork.

| Spec module | Version event | At milestone | Precondition |
|---|---|---|---|
| Object model, handshake, grant/consent, observe/actuate | **spec 0.1** (as designed) | M0 | this plan tree committed — **see "The ladder, restated" below, which is the standing sequencing record and restates this row rather than replacing it** |
| Same, corrected against running code | **spec 0.2** | M1 | Phase-1 demo end-to-end — **see "The ladder, restated" below, which is the standing sequencing record and restates this row rather than replacing it** |
| Semantic tree, diff format, epoch/CAS semantics | **core 1.0-candidate** | M2 — **this rung now sits at M2b, not M2: D-047 decision 3 splits M2 into M2a and M2b and puts "the core spec 1.0-candidate" in M2b. See "The ladder, restated" below** | Q1 and Q4 closed after E2.3's empirical tuning — **plus the two obligations added below: the per-connection version matrix (D-047 decision 4) and D-015's patent re-check** |
| Network profile (QUIC session, transport-invariant semantics) | **network 1.0-candidate** | M3 | Q6 closed at E3.1 |
| Wallet/provenance profile | stays **0.x** until Phase 4 | M4+ | EUDI/OID4VC churn settles (PRD Caveats) |

### The ladder, restated

**REALIGNED 2026-08-25 BY D-047** (decision 4, and the audit that produced it). The
ladder above is left standing because it is a published sequencing call and this
project does not silently rewrite the claims it reverses. What follows says what is
spent and what replaces it.

**Spent: the two `0.x` events. M0 and M1 are both closed and neither spec version
exists.** Verified against `main` at `7855fdc`:

- The strings `spec 0.1` and `spec 0.2` occur in exactly two files, **both under
  `docs/plan/`** — the table above, and [`00-roadmap.md`](00-roadmap.md) (its M0 row,
  its §2 ASCII ladder, and its metrics row "Published, versioned protocol spec |
  spec 0.1 tagged | M0"). Nothing under `docs/protocol/`, `protocol/`, the book, the
  site, or the tag namespace carries either string. A spec version that only the plan
  tree mentions was never published.
- **There is no `CHANGES` log in the tree, and no numbered change proposal has ever
  been opened.** §3's review process has therefore never run once, on any module. §3
  is *not* restated here: it describes a process for a spec release that has not
  happened, and the first release it must actually govern is the core 1.0-candidate.
- **The one tagged release is `v0.1.0`**, and it tags the merge of
  `p1.9.16-launch-logo-site-docs` — launch prep, not a spec release. It ships
  `<protocol name="vitrin" version="1">`, verified in **D-032**(6) with
  `git show v0.1.0:protocol/vitrin-v0.xml`.

**What happened instead is the axis the project actually versions on.**
`protocol/vitrin-v0.xml` now carries root `version="2"` and **29 `since="2"`
messages** — 14 requests and 15 events, across `vitrin_principal` (`attention`),
`vitrin_grant` (all five `get_*` facet mints), `vitrin_shim_session` (7),
`vitrin_shim_seat` (5), `vitrin_launcher`, `vitrin_layout_focus`,
`vitrin_layout_arrange`, `vitrin_powerbox` and `vitrin_egress`. (A further six
`since="2"` strings, on five lines, are prose inside `<description>` bodies — line 150
carries two of them — so the file holds 35 occurrences on 34 lines, and `grep -c`
counts lines, which is why it returns 34.) **The 1→2 bump crossed no rung of the
ladder above**: no version event, no tag, no `CHANGES` entry, no announcement, and no
liaison review — and what it added came from WS-E and the confinement track, neither
of which the ladder contemplated.

**Replacement: the version events are the IDL's version integer, not a `0.x` string.**
The project has exactly one version axis that anything outside `docs/plan/` can read,
and inventing a second one that nothing stamps is how the first two rungs came to be
claimed and unbuilt.

| Spec module | Version event | At | Precondition |
|---|---|---|---|
| Object model, handshake, grant/consent, observe/actuate | **wire version 1** — shipped in `v0.1.0` (2026-07-26) | closed, retroactively | none; recorded rather than scheduled |
| The above plus realm spawn, launcher, layout, powerbox, egress, clipboard, gestures, `attention` | **wire version 2** — the tree's current root `version`, released nowhere outside this repo | now | none; recorded rather than scheduled |
| Semantic tree, diff format, epoch/CAS semantics | **core 1.0-candidate** (unchanged) | **M2b** (per D-047 decision 3) | Q1 and Q4 closed after E2.3's empirical tuning; P2.3.7's false-reject number (D-014's own rule: the freeze follows the measurement); **plus the two obligations below** |

**What "corrected against running code" would have meant, said plainly, since the
ladder promised it and never defined it.** Three checks hold prose to the IDL today
and all three are machine-run: `xmllint --relaxng protocol/vitrin-v0.rng
protocol/vitrin-v0.xml`; `cargo xtask verb-sets --check`, which holds every surface
that *enumerates* a verb set to the set the IDL derives; and `cargo xtask
protocol-tables --check`, which holds every `docs/protocol/NN-<interface>.md` header's
stated interface version and message counts, plus `00-conventions.md` §2.3's
string-bound registry, to the IDL. Both xtask checks exist because repeated reviews
of issue #196 kept finding the same defect — a set written out in prose, corrected in
one place and left stale in another. **What none of them checks is the IDL against the core**,
which is the direction the phrase "corrected against running code" actually pointed;
that gap is what D-047's routed conformance finding
(`vitrin_grant.get_powerbox` and `get_egress` are defined at version 2 and answered
fatal `invalid_opcode`) is an instance of. **So: `docs/protocol/` is held to the IDL
by CI, and the IDL is held to the core by nothing.** Declaring the prose to *be* a
corrected spec 0.2 would have asserted the second half, which is not true.

**The ladder's own decision is still `proposed`.** [D-014](20-decision-log.md) —
"Spec versions track the implementation" — has carried **Status: proposed** since it
was written, and it is cited as normative from four files: this document's opening
paragraph, `00-roadmap.md`, `02-phase-2-semantic-epochs.md` (C3, ★P2.3.7, §4's freeze
note, P2.9.2 and risk R2.3), and the decision log itself. P2.9.2 makes the spec freeze
turn on it. **It needs acceptance or supersession before the core 1.0-candidate rung
is worked**, and this document cannot supply either — the decision log is the
instrument and D-047 deliberately amends no earlier entry.

### Obligation added by D-047 decision 4: the per-connection version matrix

**Protocol version 1 is dead as of 2026-08-25.** The shipped core accepts exactly its
maximum and refuses every other integer, version 1 included —
`crates/vitrin-core/src/principal.rs` refuses any `hello.version != PROTOCOL_VERSION`
with `VersionUnsupported`, and its own comment concedes that this contradicts
`docs/protocol/00-conventions.md` §7.3 ("a server whose maximum is N implements every
version from 1 to N") and names the fix: *"Serving 1 and 2 concurrently needs a
per-connection version matrix (which messages each connection may send, which events
it may receive)"*.

**Why declaring it dead costs nothing today.** D-032(6)'s precondition holds
unchanged: version 2 "has never been released, frozen or negotiated by any
implementation outside this repository", the one tagged release ships version 1, shims
are version-pinned at spawn because core and shims are release-paired
(`00-conventions.md` §7.2), and the only other speaker is this repo's own Python SDK.
Nothing outside the tree has ever spoken version 1 *or* 2, so there is no client for
backward compatibility to be compatible with.

**Why it becomes real at the first external spec release, and is owed then.** The
moment a third party implements the published protocol, "an older client keeps
working" stops being a hypothetical and a core that speaks only its newest integer
stops being a disclosed gap and becomes a defect against its own conventions. **The
per-connection version matrix is therefore an obligation of the core 1.0-candidate
rung**, alongside Q1/Q4 and the epoch measurement:

- the matrix itself — for each negotiated version, which requests a connection may
  send and which events it may receive, enforced rather than documented, so a
  version-1 connection cannot reach a `since="2"` opcode;
- the conformance answer for defined-but-undispatched requests, since a published spec
  may not have version-2 requests that the reference core answers fatally;
- a `CHANGES` entry and a numbered change proposal for the release itself — §3's
  process, run for the first time.

**Not scheduled here.** D-047 refused building the matrix now, inside P2.1.2 and on
the critical path of Phase 2, for a compatibility with no consumer. This rung records
the debt; it does not move it earlier.

## 3. Review process

- RFC-style: numbered change proposals as PRs against the spec, a `CHANGES` log, a stated request-for-comment window per proposal.
- Named external reviewers recruited from adjacent projects — the natural reviewers already appear in the PRD's references: AccessKit, Newton, wlroots, Smithay circles.
- Two PRD §7 spec metrics become tracked counters here: substantive external reviews logged, and independent-implementer statements of intent (targets in the [00-roadmap.md](00-roadmap.md) metrics table).

## 4. Standards-liaison table

Operationalizes Q7 and the PRD Caveats: every moving external dependency is pinned **or its unpinned surface named**, with a named re-check cadence. Reviewed at each milestone.

| Dependency | What we pin | Why it moves | Re-check |
|---|---|---|---|
| AccessKit schema | schema version adopted at E2.1 | Newton/COSMIC evolution | each milestone; before core 1.0-candidate |
| GNOME Newton protocols | prototype status noted; no hard dependency | "not yet finalized" (PRD §1.3) | each milestone |
| IETF AIMS (`draft-klrc-aiagent-auth`) | none — behind the pluggable verifier (D-008) | Security Considerations still "TODO" | each milestone |
| MCP authorization spec | revision referenced in consent design (PRD Doc 2 §5.3) | active revision cycle | each milestone |
| OID4VC / OID4VP / eIDAS 2.0 EUDI | revision pinned at E3.7 | member-state wallet timelines, revision churn | Phase-2 onward, quarterly |
| Wayland staging protocols (`wp_security_context_v1`, `ext-transient-seat-v1`, libei/EIS) | versions consumed by shim/core | staging-protocol churn | with each wlroots/Smithay upgrade task (D11) |
| DRM/KMS, libseat, libudev, libinput, GBM — the bare-metal backend | the Smithay feature set behind `drm-backend` (`backend_drm`, `backend_udev`, `backend_libinput`, `backend_session_libseat`, `backend_gbm`), with `smithay` pinned exactly at `=0.7.0` and `gbm` at `0.18`; the C libraries themselves are the distribution's and are **not** pinned | kernel DRM property churn, libinput API and gesture changes, seatd-vs-logind session backends, and build scripts that hard-fail on a missing header — `libseat-sys` and `libudev-sys` both do, and issue #218 named only the first | with each Smithay upgrade task (D11), **and before any bare-metal bring-up rung is re-run** — the backend is exercised only on the maintainer's hardware, so CI cannot detect churn here |
| xkbcommon keymaps (`xkbcommon 0.8`, behind `session-keymap`, a hard dependency of `drm-backend`) | the pre-compiled-keymap path rather than an RMLVO name lookup (D-028, WS-E.3.1); `CONTEXT_NO_ENVIRONMENT_NAMES`, so the library has no include path and no environment input | XKB data and `libxkbcommon` ship with the distribution and move with it | with each Smithay upgrade task (D11); on any change to `crate::input::keymap`. A wrong or absent keymap is not a degraded session but "a session whose keyboard types nothing but Escape and the arrows" (`crates/vitrin-core/Cargo.toml`), which is why the feature is a build-level dependency rather than a documented pairing |
| Patent landscape (**D-015**) | the 2026-07-26 public-source scan: its nearest-neighbour applications (`US20250299023A1`, `US 12,430,150`) and its prior-art anchors | pending claims get amended and continuations get filed, which is why D-015 names the re-check itself | **each milestone from M2a onward, and as a blocking precondition of the core 1.0-candidate rung** in §2 |

**Three rows added 2026-08-25, and *not* by D-047's obligation on this document** —
that entry obliges one thing here, *"WS-A's ladder restated and given the
version-matrix obligation (4)"*, discharged in §2 above; these rows stand on the
measurements cited in them rather than on a decision entry, and the next entry to
touch this workstream should adopt or reject them. The
paragraph above said "every moving external dependency is pinned"; that was simply
false about the tree — the first two rows added pin a Rust feature set and leave the C
libraries behind it to the distribution — so the words **"or its unpinned surface
named"** are added to that paragraph in place rather than annotated: it was a
description, not a decision, and there was nothing here to preserve. The first two are
the DRM/xkb stack WS-E took on and closed on 2026-08-13 without this table ever
gaining a row for it. The third is
**D-015's own re-check**, which that entry states as an "explicit re-check item
before 1.0" and which nothing owned: of D-015's three legs, publication and the
license patent grants are in force, the OIN leg is tracked as
[#159](https://github.com/vitrin-os/vitrin-os/issues/159), and **the landscape
re-check had no owner, no cadence and no issue.** This row is the cadence; it is not
an issue, and D-047 files rather than schedules.

Two limits on that third row, stated because D-015 states them: the 2026-07-26 scan
was a **public-source landscape scan, not a freedom-to-operate opinion**, no FTO has
been obtained and none is budgeted; and the realistic exposure D-015 names
materialises with *revenue*, not with publication, so the cadence above is a
disclosure discipline rather than a risk control.
