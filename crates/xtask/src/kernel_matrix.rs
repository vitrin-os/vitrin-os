// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask kernel-matrix` — render `docs/book/src/isolation-kernels.md`
//! from the boot rows under `tests/kernel-matrix/rows/` (issue #281).
//!
//! # What separates this from `isolation_matrix.rs`, and why both exist
//!
//! [`crate::isolation_matrix`] is a **build** table: it parses this
//! repository's own source and says what `vitrind` *requires*. It probes
//! nothing, deliberately, because a page carrying the ABI of the machine that
//! generated it could not be byte-stable across two machines and so could not
//! be the thing CI holds.
//!
//! This module is the other half, and it solves that problem a different way:
//! the measurement does not happen here. It happened once, under
//! `tests/kernel-matrix/collect.sh`, which boots each pinned kernel under QEMU
//! with the shipped `vitrind` in a minimal initramfs and writes the binary's
//! own output to a checked-in row. This generator only **reads** those rows.
//! So the page is a measurement, and it is still byte-stable on every machine,
//! because rendering it re-measures nothing.
//!
//! The staleness gate that would normally live in `--check` therefore lives in
//! two places rather than one, and the split is the honest one:
//!
//! - `cargo xtask kernel-matrix --check` (cheap, every PR) holds the **page**
//!   to the **rows**. It proves the prose was not hand-edited away from the
//!   measurement.
//! - `tests/kernel-matrix/collect.sh --check` (expensive, manual or scheduled)
//!   holds the **rows** to the **kernels**. It re-boots all five and diffs.
//!   Nothing in a pull request runs it, and the rows carry a collection date so
//!   a reader can see how old the measurement is.
//!
//! Neither one alone would be enough, and saying which is which is the point:
//! a green PR proves the page matches the rows, and proves nothing at all about
//! whether the rows still match the kernels.
//!
//! # What this generator refuses to render
//!
//! Every refusal below exists because its absence would let the page state
//! something no row measured:
//!
//! 1. a row file with no [`KERNELS`] note, or a note with no row file — the
//!    note is what says *why* a kernel is in the set, and a row nothing
//!    explains is a number without a reason;
//! 2. a row whose `build.landlock_min_abi` is not this tree's
//!    `LANDLOCK_MIN_ABI` — that row was collected against a different build,
//!    and mixing it with the others would publish one table describing two
//!    floors;
//! 3. a row whose verdict disagrees with the arithmetic — if `landlock.abi=6`
//!    and the floor is 6, the collected startup line must be an admission. The
//!    row and the constant are two independent witnesses and the page prints
//!    both, so they are held to each other rather than averaged;
//! 4. a row missing any cell the table prints, or missing its provenance —
//!    an empty cell rendered into a published table reads as a fact;
//! 5. a row whose recorded **floor mechanism set** differs from this build's by
//!    anything other than the one acknowledged, named delta — see below.
//!
//! # The staleness this file could not see, and now can
//!
//! Refusal (2) above holds each row's `build.landlock_min_abi` to this tree's.
//! That is one number out of the whole build half of a row, and P2.6.4 (#188)
//! walked straight through the gap it left: the floor grew from
//! `{namespaces, landlock}` to `{namespaces, landlock, seccomp, no-new-privs}`,
//! every checked-in row went on saying `applies.seccomp=not-yet` and carrying a
//! startup line that ends *“the seccomp filter (P2.6.4) is not applied by this
//! build”* — and `cargo xtask kernel-matrix --check` **passed**, because it
//! compares the page against the rows and both were stale together. It never
//! re-boots, so it had no witness to the build the rows were taken against
//! except the one number it already read.
//!
//! The rows always carried the rest of that witness: `--print-floor`'s
//! `floor.mechanism=` lines and `applies.*` rows are in every row file,
//! verbatim. [`BuildMechanisms::from_source`] reads the same two sets out of the
//! crate that declares them, and [`validate`] holds one to the other. The
//! difference is then **acknowledged rather than tolerated**:
//! [`ROWS_PREDATE_FLOOR`] and [`ROWS_PREDATE_APPLIED`] name exactly which
//! mechanisms entered this build after the rows were collected, so
//!
//! * a delta equal to the acknowledgement renders — and the page publishes the
//!   rows as **stale pending re-collection**, naming the moved mechanisms, in a
//!   paragraph generated from the computed difference rather than typed;
//! * **any other delta is red**, in either direction, naming what moved and
//!   asking for a re-collection. A sixth mechanism, or a floor move that undid
//!   one of these two, does not get the benefit of an acknowledgement written
//!   for something else.
//!
//! Both acknowledgements are **empty** as this is written: the rows were
//! re-collected on 2026-08-16 against a build carrying the whole of P2.6.4's
//! floor, so the delta is nothing and the page renders the current banner. That
//! is a fact with a shelf life and this module does not publish it as more:
//! nothing here re-boots a kernel, so what the mechanism *guarantees* is only
//! that the rows cannot go on describing an older binary in silence — the day
//! the floor moves again, this gate is RED until either `collect.sh` runs or
//! the new delta is named in the two constants below and published on the page.
//! The debt case is therefore still live code, exercised by
//! [`tests::an_acknowledged_delta_renders_the_stale_banner_and_a_wrong_one_is_refused`]
//! through [`staleness_with`], because the day it is next needed is a day
//! somebody is already deferring something.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::isolation_matrix::{join_and, normalize, Constants};

/// Where the collector writes, and where this reads.
const ROWS_DIR: &str = "tests/kernel-matrix/rows";
/// The record set the rows must correspond to, one-for-one.
const MANIFEST: &str = "tests/kernel-matrix/kernels.manifest";
/// The page this emits.
const PAGE: &str = "docs/book/src/isolation-kernels.md";
/// Where the floor constant is declared.
const REALM_INIT_LIB_RS: &str = "crates/vitrin-realm-init/src/lib.rs";
/// Where the floor's *mechanism set* is declared, and the source of the
/// `floor.mechanism=` and `applies.*` rows every row file carries.
const CORE_ISOLATION_RS: &str = "crates/vitrin-core/src/spawn/isolation.rs";
/// The plan surfaces that **enumerate** the rungs no measured kernel reports,
/// rather than only counting them. Held to the rows by
/// [`check_unreported_enumeration`]; see [`unreported_rungs_sentence`] for why.
const ENUMERATING_SURFACES: &[&str] = &[
    "docs/plan/20-decision-log.md",
    "docs/plan/02-phase-2-semantic-epochs.md",
];

/// **The build-half staleness the checked-in rows carry, in the FLOOR.**
///
/// Every mechanism named here is one this build **gates startup on** and no row
/// was collected under. It is an acknowledgement, not a permission: the page
/// publishes each of these by name as a reason the rows are stale, and
/// [`validate`] goes red the moment the real delta is anything else.
///
/// **Empty, and that is the paid-off state rather than the default one.** The
/// rows collected on 2026-08-15 predated P2.6.4 ([#188]) putting `seccomp` and
/// `no-new-privs` into the floor, and this constant named those two for as long
/// as that was true. `tests/kernel-matrix/collect.sh` re-booted all five kernels
/// on 2026-08-16 against a build that carries them, so the delta is now nothing.
///
/// **Do not read the emptiness as a guarantee of freshness.** It says the rows'
/// build half matches *this* tree, which is re-checked on every run; it says
/// nothing about the kernels, which only `collect.sh --check` re-takes.
///
/// **To defer a re-collection again**: name the moved mechanisms here, and the
/// page publishes them by name instead of the current banner. That branch is not
/// dead code — [`staleness_with`] takes the acknowledgement as an argument so
/// [`tests::an_acknowledged_delta_renders_the_stale_banner_and_a_wrong_one_is_refused`]
/// exercises both outcomes while these constants are empty, and
/// [`the_stale_banner_and_the_current_banner_are_both_reachable`] renders both
/// paragraphs.
///
/// [#188]: https://github.com/vitrin-os/vitrin-os/issues/188
const ROWS_PREDATE_FLOOR: &[&str] = &[];

/// **The same acknowledgement for what the build APPLIES**, which is a
/// different set from the floor and is why there are two constants.
///
/// Empty for the same reason as [`ROWS_PREDATE_FLOOR`]. While it was not, it
/// named `seccomp` alone and *not* `no-new-privs`: `vitrin-realm-init` set
/// `PR_SET_NO_NEW_PRIVS` on every confined spawn long before #188, so the rows
/// already read `applies.no-new-privs=yes`. It was not a *gate* until #188,
/// which is exactly why `--print-floor` prints two row families and why this
/// gate compares both — a build that started applying a mechanism without
/// gating it would move one set and not the other, and one constant could not
/// tell the difference.
const ROWS_PREDATE_APPLIED: &[&str] = &[];

/// What a kernel is in the set FOR.
///
/// The row says what a kernel *answered*; this says why anybody booted it. They
/// are kept apart on purpose: the answer is a measurement and must never be
/// edited, and the reason is editorial and must never be mistaken for one.
struct KernelNote {
    /// Matches `rows/<id>.row` and the manifest's first field.
    id: &'static str,
    /// The distribution this vmlinuz is shipped by, as a reader would name it.
    /// It labels the *source of the binary*, not the userspace it ran under —
    /// which was a bare initramfs for every row here.
    distribution: &'static str,
    /// Why this kernel and not another.
    why: &'static str,
}

/// The kernels this page covers, and the reason each is here.
///
/// Five, and every one of them earns its place by being a machine somebody
/// might actually be refused on — not by filling in a rung. Four of the nine
/// ABI rungs are reported by none of these kernels, and the page says so rather
/// than implying a sweep.
const KERNELS: &[KernelNote] = &[
    KernelNote {
        id: "ubuntu-22.04-5.15",
        distribution: "Ubuntu 22.04 LTS",
        why: "The oldest kernel with Landlock at all that a reader might still be running. It is \
              the bottom of the range: if this build refused nothing else, it would refuse this.",
    },
    KernelNote {
        id: "debian-12-6.1",
        distribution: "Debian 12 (bookworm)",
        why: "The previous Debian stable, and the row that shows the floor is not satisfied by \
              \"a recent LTS\" — 6.1 is a long-term kernel and it is still four rungs short.",
    },
    KernelNote {
        id: "ubuntu-24.04-6.8",
        distribution: "Ubuntu 24.04 LTS (GA kernel)",
        why: "The kernel PR #290's AppArmor profile was written for, and the reason that profile \
              cannot be sufficient on its own: the profile is aimed at the namespace refusal, \
              and this kernel is refused at the next gate anyway, on the Landlock floor. The \
              page's \"AppArmor profile\" section is about this row.",
    },
    KernelNote {
        id: "debian-13-6.12",
        distribution: "Debian 13 (trixie), current stable",
        why: "The row the floor was lowered for (owner's decision, 2026-08-16). Under the \
              previous floor of 7 this kernel was refused; it reports ABI 6, and the domain this \
              build enforces at rung 6 is identical to the one it enforces at rung 7.",
    },
    KernelNote {
        id: "ubuntu-azure-6.17",
        distribution: "Ubuntu (azure kernel) — what this repository's CI runners boot",
        why: "The cross-validation row. It is the kernel release the CI runner reported ON THE \
              COLLECTION DATE, so booting it here says whether this harness reproduces that \
              machine — and, on the policy cells, whether it does not. **The runner's kernel \
              moves:** it reported `6.17.0-1020-azure` on 2026-08-14 and `6.17.0-1022-azure` by \
              2026-08-16. Both are ABI 7, so the rung this row cross-validates is unaffected, \
              but do not read the row as naming whatever the runner boots today. GitHub bumps \
              that image with no commit here, which is why this table is dated rather than \
              presented as current.",
    },
];

/// This build's two mechanism sets, read out of the crate that declares them.
///
/// **Parsed, never transcribed**, for the reason [`Constants::from_source`]
/// gives about the floor number: a second copy of a set is a second thing to
/// keep true, and the whole point of this check is that moving the declaration
/// makes the checked-in rows stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildMechanisms {
    /// `vitrin_core::spawn::isolation::FLOOR`, in its wire spellings — what
    /// this build **refuses to start without**, and what `--print-floor` emits
    /// as `floor.mechanism=` lines.
    floor: Vec<String>,
    /// `vitrin_core::spawn::isolation::APPLIED` — what this build applies to a
    /// confined realm, gate or not, and what `--print-floor` emits as
    /// `applies.<name>=yes`.
    applied: Vec<String>,
}

impl BuildMechanisms {
    /// Read `FLOOR`, `APPLIED` and the `Display` spellings out of
    /// [`CORE_ISOLATION_RS`].
    ///
    /// The spellings are parsed too, rather than mapped here from variant
    /// names: the row files carry the **wire** spelling (`no-new-privs`, not
    /// `NoNewPrivs`), and a hand-written mapping in this file would be a third
    /// copy that could disagree with the `Display` impl the binary actually
    /// prints through.
    pub fn from_source(isolation_rs: &str) -> Result<BuildMechanisms> {
        let spellings = mechanism_spellings(isolation_rs)?;
        let read = |name: &str| -> Result<Vec<String>> {
            let variants = mechanism_list(isolation_rs, name)?;
            variants
                .iter()
                .map(|variant| {
                    spellings.get(variant).cloned().with_context(|| {
                        format!(
                            "kernel-matrix: `{name}` names `Mechanism::{variant}` and the \
                             `Display` impl in {CORE_ISOLATION_RS} gives it no wire spelling. \
                             The row files record the wire spelling, so a variant this cannot \
                             name is a mechanism this check would silently skip."
                        )
                    })
                })
                .collect()
        };
        Ok(BuildMechanisms {
            floor: read("FLOOR")?,
            applied: read("APPLIED")?,
        })
    }
}

/// `Mechanism::Namespaces => write!(f, "namespaces"),` → `Namespaces` →
/// `namespaces`.
fn mechanism_spellings(src: &str) -> Result<BTreeMap<String, String>> {
    let body = src
        .split_once("impl fmt::Display for Mechanism {")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .with_context(|| {
            format!(
                "kernel-matrix: {CORE_ISOLATION_RS} no longer carries an `impl fmt::Display for \
                 Mechanism` this parser can find. That impl is what turns a variant into the \
                 `floor.mechanism=` spelling every row file records, so without it this check \
                 cannot compare a row to the build and refuses to guess."
            )
        })?;
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let Some((_, rest)) = line.split_once("Mechanism::") else {
            continue;
        };
        let Some((variant, rest)) = rest.split_once(" => write!(f, \"") else {
            continue;
        };
        let Some((spelling, _)) = rest.split_once('"') else {
            continue;
        };
        out.insert(variant.to_string(), spelling.to_string());
    }
    if out.is_empty() {
        bail!(
            "kernel-matrix: the `Display` impl in {CORE_ISOLATION_RS} yielded no \
             `Mechanism::X => write!(f, \"y\")` arms. Refusing to compare a row's recorded floor \
             against an empty vocabulary, which would call every row current."
        );
    }
    Ok(out)
}

/// The `Mechanism::X` variants of `pub const NAME: &[Mechanism] = &[...];`, in
/// declaration order.
fn mechanism_list(src: &str, name: &str) -> Result<Vec<String>> {
    let head = format!("pub const {name}: &[Mechanism] = &[");
    let body = src
        .split_once(head.as_str())
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("];"))
        .map(|(body, _)| body)
        .with_context(|| {
            format!(
                "kernel-matrix: could not read `{head}...];` from {CORE_ISOLATION_RS}. That \
                 declaration is what `vitrind --print-floor` renders, and every checked-in row \
                 holds a copy of what it printed on the collection date; without it this check \
                 cannot tell a current row from a stale one."
            )
        })?;
    let mut out = Vec::new();
    for piece in body.split("Mechanism::").skip(1) {
        let variant: String = piece
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !variant.is_empty() && !out.contains(&variant) {
            out.push(variant);
        }
    }
    if out.is_empty() {
        bail!(
            "kernel-matrix: `{name}` in {CORE_ISOLATION_RS} declares no mechanisms. An empty \
             floor is a build that gates on nothing, and this generator will not publish a \
             per-kernel table describing one."
        );
    }
    Ok(out)
}

/// One parsed row file.
struct Row {
    id: String,
    class: String,
    release: String,
    source_url: String,
    source_sha256: String,
    vmlinuz_member: String,
    vmlinuz_sha256: String,
    boot: String,
    userspace: String,
    collected: String,
    accel: String,
    /// `--print-isolation`'s schema tag, e.g. `vitrin-isolation 3`. It is the
    /// one line of that section that is NOT `key=value`, so it is parsed on its
    /// own -- an earlier draft folded it into the map and published `?`.
    schema: String,
    /// `--print-isolation`'s key=value lines.
    isolation: BTreeMap<String, String>,
    /// `--print-floor`'s key=value lines. `floor.mechanism` repeats, so only
    /// the single-valued keys this page reads are kept -- the repeating one is
    /// [`Row::floor_mechanisms`].
    floor: BTreeMap<String, String>,
    /// **Every** `floor.mechanism=` line of the row, in the order the collected
    /// binary printed them.
    ///
    /// This is the row's own record of the build it was taken against, and the
    /// witness [`validate`] holds to [`BuildMechanisms::floor`]. It is kept as
    /// a list rather than folded into [`Row::floor`] because that map is
    /// last-wins, which for a repeating key means every reading but one was
    /// thrown away -- and the whole set is the fact.
    floor_mechanisms: Vec<String>,
    /// `admitted` or `refused`, as the collector classified the startup phase.
    verdict: String,
    /// The startup line itself, verbatim.
    verdict_line: String,
}

impl Row {
    fn isolation_cell(&self, key: &str) -> Result<&str> {
        self.isolation
            .get(key)
            .map(String::as_str)
            .with_context(|| {
                format!(
                    "kernel-matrix: {}.row has no `{key}=` line. The page prints that cell, and \
                     an absent reading rendered into a published table reads as a fact about \
                     the kernel.",
                    self.id
                )
            })
    }

    /// The Landlock ABI as a number, refusing anything that is not one.
    ///
    /// A row may legitimately carry `landlock.abi=absent(errno=38)` on a kernel
    /// without the LSM — but none of these five do, and a page that silently
    /// rendered such a string in a numeric column would be comparing it to the
    /// floor by string order. If that day comes, this is where it stops.
    fn landlock_abi(&self) -> Result<u32> {
        let raw = self.isolation_cell("landlock.abi")?;
        raw.parse().with_context(|| {
            format!(
                "kernel-matrix: {}.row reports `landlock.abi={raw}`, which is not a number. This \
                 generator compares that cell to the build's floor, and it will not compare a \
                 support string to an integer. Teach the page to render this shape before \
                 adding such a row.",
                self.id
            )
        })
    }
}

/// Split `key: value` out of a `# ` or `#~ ` header line.
fn header<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let body = line
            .strip_prefix("#~ ")
            .or_else(|| line.strip_prefix("# "))?;
        body.strip_prefix(key)?.strip_prefix(": ")
    })
}

/// Everything between two `--- ... ---` fences, as key=value pairs.
fn section(text: &str, fence: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("--- ") {
            inside = line == fence;
            continue;
        }
        if inside && !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn kv(lines: &[String]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once('=') {
            // `floor.mechanism` repeats; last wins, and nothing this page
            // prints reads it.
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

fn parse_row(id: &str, text: &str) -> Result<Row> {
    let need = |key: &str| -> Result<String> {
        header(text, key)
            .map(str::to_string)
            .with_context(|| format!("kernel-matrix: {id}.row carries no `# {key}:` header"))
    };

    let isolation_lines = section(text, "--- vitrind --print-isolation ---");
    let schema = isolation_lines
        .iter()
        .find(|l| l.starts_with("vitrin-isolation "))
        .cloned()
        .with_context(|| {
            format!(
                "kernel-matrix: {id}.row's isolation section carries no `vitrin-isolation N` \
                 schema tag. That tag is how a row collected against an older row set is \
                 recognisable as stale rather than merely different, so the page prints it and \
                 will not print a placeholder in its place."
            )
        })?;
    let isolation = kv(&isolation_lines);
    let floor_lines = section(text, "--- vitrind --print-floor ---");
    let floor_mechanisms: Vec<String> = floor_lines
        .iter()
        .filter_map(|line| line.strip_prefix("floor.mechanism="))
        .map(str::to_string)
        .collect();
    let floor = kv(&floor_lines);
    if isolation.is_empty() {
        bail!("kernel-matrix: {id}.row holds no `--print-isolation` section");
    }
    if floor.is_empty() {
        bail!("kernel-matrix: {id}.row holds no `--print-floor` section");
    }
    if floor_mechanisms.is_empty() {
        bail!(
            "kernel-matrix: {id}.row's `--print-floor` section carries no `floor.mechanism=` \
             line. That set is the row's own record of the build it was collected against, and \
             the only witness this generator has to whether the row is stale in its build half. \
             A row without it would be compared against nothing and published as current."
        );
    }

    let startup = section(
        text,
        "--- vitrind --headless --consent=auto-approve: the isolation preflight's own line ---",
    );
    let verdict = startup
        .first()
        .and_then(|l| l.strip_prefix("verdict: "))
        .map(str::to_string)
        .with_context(|| {
            format!("kernel-matrix: {id}.row carries no `verdict:` line for the startup phase")
        })?;
    if verdict != "admitted" && verdict != "refused" {
        bail!(
            "kernel-matrix: {id}.row says `verdict: {verdict}`, and the only two verdicts a \
             startup can have are `admitted` and `refused`"
        );
    }
    let verdict_line = startup.get(1).cloned().with_context(|| {
        format!("kernel-matrix: {id}.row carries a verdict with no line under it")
    })?;

    Ok(Row {
        id: id.to_string(),
        class: need("class")?,
        release: need("kernel.release")?,
        source_url: need("source.url")?,
        source_sha256: need("source.sha256")?,
        vmlinuz_member: need("vmlinuz.member")?,
        vmlinuz_sha256: need("vmlinuz.sha256")?,
        boot: need("boot")?,
        userspace: need("userspace")?,
        collected: need("collected")?,
        accel: need("accel")?,
        schema,
        isolation,
        floor,
        floor_mechanisms,
        verdict,
        verdict_line,
    })
}

/// Load every row, in [`KERNELS`] order, refusing any mismatch with the set.
fn load_rows(root: &Path) -> Result<Vec<Row>> {
    let dir = root.join(ROWS_DIR);
    let mut on_disk: Vec<String> = fs::read_dir(&dir)
        .with_context(|| {
            format!(
                "kernel-matrix: cannot read {ROWS_DIR}. The page is rendered FROM the boot rows; \
                 without them there is nothing to render and this command will not emit a page \
                 describing kernels nobody booted. Collect them with \
                 tests/kernel-matrix/collect.sh."
            )
        })?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".row").map(str::to_string)
        })
        .collect();
    on_disk.sort();

    for id in &on_disk {
        if !KERNELS.iter().any(|k| k.id == *id) {
            bail!(
                "kernel-matrix: {ROWS_DIR}/{id}.row has no entry in KERNELS \
                 (crates/xtask/src/kernel_matrix.rs). A measured row with no stated reason is a \
                 number on a page nobody can act on -- add the entry, or delete the row."
            );
        }
    }

    let mut rows = Vec::new();
    for note in KERNELS {
        if !on_disk.iter().any(|id| id == note.id) {
            bail!(
                "kernel-matrix: KERNELS names `{}` and {ROWS_DIR}/{}.row does not exist. This \
                 generator will not render a table with a hole in it: run \
                 tests/kernel-matrix/collect.sh, or remove the entry.",
                note.id,
                note.id
            );
        }
        let path = dir.join(format!("{}.row", note.id));
        let text = fs::read_to_string(&path)
            .with_context(|| format!("kernel-matrix: reading {}", path.display()))?;
        rows.push(parse_row(note.id, &text)?);
    }
    Ok(rows)
}

/// What moved in the build since the rows were collected, computed rather than
/// asserted, and rendered onto the page.
#[derive(Debug, Default, PartialEq, Eq)]
struct Staleness {
    /// Mechanisms this build **gates startup on** that no row was collected
    /// under. Empty means the rows' floor is this build's floor.
    floor: Vec<String>,
    /// Mechanisms this build **applies** that no row was collected under.
    applied: Vec<String>,
    /// The distinct collection dates across the row set, sorted. Rendered so a
    /// reader sees how old the stale half is without opening a row.
    collected: Vec<String>,
}

impl Staleness {
    fn is_current(&self) -> bool {
        self.floor.is_empty() && self.applied.is_empty()
    }
}

/// Everything in `build` that `row` does not have, in `build` order.
fn missing(build: &[String], row: &[String]) -> Vec<String> {
    build.iter().filter(|m| !row.contains(m)).cloned().collect()
}

/// The mechanisms a row records as **applied**, from its `applies.<name>=yes`
/// rows.
fn row_applied(row: &Row) -> Vec<String> {
    row.floor
        .iter()
        .filter(|(_, v)| v.as_str() == "yes")
        .filter_map(|(k, _)| k.strip_prefix("applies.").map(str::to_string))
        .collect()
}

/// Hold every row's recorded **build half** to this tree's, and acknowledge the
/// one named difference.
///
/// See this module's docs for why this exists at all: `validate`'s
/// `build.landlock_min_abi` check covers one number, and P2.6.4 moved the
/// mechanism *sets* straight past it with every gate green.
///
/// Returns what moved, so [`render`] can publish it. Fails when the delta is
/// anything other than [`ROWS_PREDATE_FLOOR`] / [`ROWS_PREDATE_APPLIED`] --
/// including the direction nobody expects, a row naming a mechanism this build
/// has stopped gating on.
fn staleness(rows: &[Row], build: &BuildMechanisms) -> Result<Staleness> {
    staleness_with(rows, build, ROWS_PREDATE_FLOOR, ROWS_PREDATE_APPLIED)
}

/// [`staleness`] with the acknowledgement passed in rather than read from the
/// two constants.
///
/// This split exists because both constants are now `&[]`, and with an empty
/// acknowledgement two of this function's three refusals become **unreachable**:
/// nothing can be "acknowledged as stale but did not move" when nothing is
/// acknowledged, and the stale banner has no delta to render. Those are the
/// paths the next deferred re-collection depends on, so they are held by
/// argument rather than left to be discovered broken on the day somebody needs
/// them. `staleness` is the only caller that supplies the real constants.
fn staleness_with(
    rows: &[Row],
    build: &BuildMechanisms,
    ack_floor: &[&str],
    ack_applied: &[&str],
) -> Result<Staleness> {
    let mut out = Staleness::default();
    for row in rows {
        let row_applied = row_applied(row);

        for (what, acknowledged, build_set, row_set) in [
            (
                "floor.mechanism",
                ack_floor,
                &build.floor,
                &row.floor_mechanisms,
            ),
            ("applies", ack_applied, &build.applied, &row_applied),
        ] {
            // The direction an acknowledgement can never cover: the row was
            // taken under a build that did something this one does not.
            let dropped = missing(row_set, build_set);
            if !dropped.is_empty() {
                bail!(
                    "kernel-matrix: {}.row records `{what}` = {row_set:?} and this build's set \
                     is {build_set:?}, so the row names {dropped:?}, which this build no longer \
                     has. That is not staleness in the row -- a row can only be older than the \
                     build, never newer -- so either the mechanism was removed from \
                     {CORE_ISOLATION_RS} and the published page still describes it, or the row \
                     was hand-edited. Neither is publishable: re-collect with \
                     tests/kernel-matrix/collect.sh, or restore the mechanism.",
                    row.id
                );
            }

            let moved = missing(build_set, row_set);
            let unacknowledged: Vec<&String> = moved
                .iter()
                .filter(|m| !acknowledged.contains(&m.as_str()))
                .collect();
            let stale_acknowledgement: Vec<&&str> = acknowledged
                .iter()
                .filter(|m| !moved.iter().any(|got| got == *m))
                .collect();
            if !unacknowledged.is_empty() || !stale_acknowledgement.is_empty() {
                bail!(
                    "kernel-matrix: THE CHECKED-IN ROWS MUST BE RE-COLLECTED. {}.row records \
                     `{what}` = {row_set:?}; this build's set is {build_set:?}. What moved: \
                     {moved:?}. The acknowledged, published staleness is {acknowledged:?} -- \
                     {}{}This generator will not publish a table whose build half describes a \
                     binary nobody ran. Either run tests/kernel-matrix/collect.sh (needs QEMU) \
                     and re-render, or -- if the move is deliberate and re-collection is being \
                     deferred again -- update the acknowledgement in \
                     crates/xtask/src/kernel_matrix.rs so the page publishes the new delta by \
                     name.",
                    row.id,
                    if unacknowledged.is_empty() {
                        String::new()
                    } else {
                        format!("{unacknowledged:?} moved and is NOT acknowledged. ")
                    },
                    if stale_acknowledgement.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{stale_acknowledgement:?} is acknowledged as stale and did NOT \
                             move, so the page would publish a staleness that is not there. "
                        )
                    },
                );
            }
            if what == "floor.mechanism" {
                out.floor.clone_from(&moved);
            } else {
                out.applied.clone_from(&moved);
            }
        }

        if !out.collected.contains(&row.collected) {
            out.collected.push(row.collected.clone());
        }
    }
    out.collected.sort();
    Ok(out)
}

/// Hold every row to the build it was collected against, and to itself.
///
/// This is the check that stops the page being a museum: rows are collected
/// once and the floor is a constant somebody can re-tune in a one-line diff, so
/// without it a floor move would silently republish verdicts taken under the
/// old number.
fn validate(rows: &[Row], constants: &Constants, manifest: &str) -> Result<()> {
    for row in rows {
        let printed = row.floor.get("build.landlock_min_abi").with_context(|| {
            format!(
                "kernel-matrix: {}.row has no `build.landlock_min_abi=`",
                row.id
            )
        })?;
        let printed: u32 = printed.parse().with_context(|| {
            format!(
                "kernel-matrix: {}.row reports `build.landlock_min_abi={printed}`, not a number",
                row.id
            )
        })?;
        if printed != constants.min_abi {
            bail!(
                "kernel-matrix: {}.row was collected against a build whose floor was ABI \
                 {printed}, and this tree declares ABI {}. Every verdict in that row was taken \
                 under the old number, so publishing it beside this tree's would put two floors \
                 in one table. Re-collect with tests/kernel-matrix/collect.sh.",
                row.id,
                constants.min_abi
            );
        }

        // The row and the constant are two independent witnesses to the same
        // rule. Averaging them would hide a bug in either.
        let abi = row.landlock_abi()?;
        let expected = if abi >= constants.min_abi {
            "admitted"
        } else {
            "refused"
        };
        if row.verdict != expected {
            bail!(
                "kernel-matrix: {}.row reports `landlock.abi={abi}` against a floor of {}, so the \
                 startup phase should have been {expected}; the collected line says {}. One of \
                 the binary and the constant is wrong and this generator will not pick a winner.",
                row.id,
                constants.min_abi,
                row.verdict
            );
        }

        // Every cell the table prints, required here rather than defaulted at
        // render time. `render` has `unwrap_or("?")` fallbacks that are
        // unreachable once this passes, and they are unreachable on purpose:
        // a published table that renders a missing reading as a character is
        // the failure mode this whole harness is built against, and it shipped
        // once already (the schema tag).
        for key in [
            "kernel.release",
            "landlock.abi",
            "ns.all",
            "mount.in_userns",
            "tier",
        ] {
            row.isolation_cell(key)?;
        }
        row.floor.get("build.version").with_context(|| {
            format!(
                "kernel-matrix: {}.row has no `build.version=`, and the page prints which \
                 `vitrind` took the measurement",
                row.id
            )
        })?;

        if !manifest.contains(&format!("{} {} {} ", row.id, row.class, row.release)) {
            bail!(
                "kernel-matrix: {MANIFEST} has no record whose id, class and release are `{} {} \
                 {}`. The manifest is what makes a row re-measurable; a row it does not describe \
                 cannot be re-collected and must not be published.",
                row.id,
                row.class,
                row.release
            );
        }
    }
    Ok(())
}

/// The rungs of this build's ladder that **no checked-in row reports**.
///
/// Derived from `landlock.abi` in `tests/kernel-matrix/rows/*.row` and this
/// build's ceiling, so a sixth kernel filling one in moves the set here and
/// everywhere the set is published.
fn unreported_rungs(rows: &[Row], constants: &Constants) -> Vec<u32> {
    let reported: Vec<u32> = rows.iter().filter_map(|r| r.landlock_abi().ok()).collect();
    (1..=constants.max_rung)
        .filter(|rung| !reported.contains(rung))
        .collect()
}

/// The sentence the two plan surfaces that **name** those rungs must carry.
///
/// **This exists because the count was machine-held and the names were not.**
/// [`render`] has computed "N of this build's 9 rungs are reported by none of
/// these kernels" since the page was first emitted, and that number has always
/// been right. But `docs/plan/20-decision-log.md` (D-044, Decision 3) and
/// `docs/plan/02-phase-2-semantic-epochs.md` (Correction 7) went further and
/// *enumerated* them by hand -- and both published **4, 7 and 8**, which is the
/// set the `--landlock=abi:N` cap cannot simulate, not the set no measured
/// kernel reports. The rows say `landlock.abi` 1, 2, 4, 6 and 7, so the real
/// answer is 3, 5, 8 and 9: two of the three names published were rungs a
/// checked-in row reports. A full green build carried that sentence for four
/// days, which is exactly the defect class this repository's generators exist
/// to end -- so the enumeration is now derived here and demanded there.
///
/// It is a **substring** requirement, matched after whitespace normalization
/// ([`crate::isolation_matrix::normalize`]), so the surrounding paragraph can
/// be rewritten and re-wrapped freely; only the claim is pinned.
fn unreported_rungs_sentence(rows: &[Row], constants: &Constants) -> String {
    let names: Vec<String> = unreported_rungs(rows, constants)
        .iter()
        .map(u32::to_string)
        .collect();
    format!(
        "the rungs no measured kernel reports are **{}**",
        join_and(&names)
    )
}

/// Hold both plan surfaces to [`unreported_rungs_sentence`].
///
/// Runs on `--check` **and** on a regeneration, because a wrong enumeration is
/// wrong on the day it is written and not on the day CI next runs.
fn check_unreported_enumeration(root: &Path, rows: &[Row], constants: &Constants) -> Result<()> {
    let required = unreported_rungs_sentence(rows, constants);
    let needle = normalize(&required);
    for surface in ENUMERATING_SURFACES {
        let text = fs::read_to_string(root.join(surface))
            .with_context(|| format!("kernel-matrix: reading {surface}"))?;
        if !normalize(&text).contains(&needle) {
            bail!(
                "kernel-matrix: {surface} does not carry the enumeration the rows in {ROWS_DIR} \
                 produce. It must say, in these words:\n  {required:?}\nThe rungs are derived \
                 from `landlock.abi` across the checked-in rows and this build's ceiling of {}. \
                 Do not retype the list on the page -- if it disagrees with this, the rows \
                 changed and the prose around them has to be re-derived too.",
                constants.max_rung
            );
        }
    }
    Ok(())
}

fn admitted(rows: &[Row]) -> Vec<&Row> {
    rows.iter().filter(|r| r.verdict == "admitted").collect()
}

fn refused(rows: &[Row]) -> Vec<&Row> {
    rows.iter().filter(|r| r.verdict == "refused").collect()
}

fn note_for(id: &str) -> &'static KernelNote {
    KERNELS
        .iter()
        .find(|k| k.id == id)
        // Unreachable: `load_rows` refuses a row with no note before this runs.
        .expect("every row has a KERNELS note")
}

/// The row whose kernel release is the one this repository's CI runners report.
///
/// It is the cross-validation row, and it is found by its [`KernelNote`] id
/// rather than by position so reordering the set cannot silently point the
/// comparison at a different kernel.
const CROSS_VALIDATION_ROW: &str = "ubuntu-azure-6.17";

/// What the CI runner answered on the same kernel release, cell by cell.
///
/// **Transcribed, not measured here.** These come from the `What confinement
/// this runner actually grants` step's log — a diagnostic that archives
/// nothing, on a service that expires job logs. They are in this table so the
/// disagreement is visible; they are labelled on the page as a transcription so
/// nobody reads them as an artefact of this tree. The left-hand column beside
/// them IS an artefact: it is read out of the checked-in row.
const RUNNER_READINGS: &[(&str, &str)] = &[
    ("landlock.abi", "7"),
    ("ns.all", "available"),
    ("policy.apparmor_restrict_unprivileged_userns", "1"),
    ("mount.in_userns", "restricted-by-policy(errno=13)"),
    ("tier", "none"),
];

/// Render the harness-vs-runner table, reading the harness column from the row.
fn cross_validation(rows: &[Row]) -> Result<String> {
    let row = rows
        .iter()
        .find(|r| r.id == CROSS_VALIDATION_ROW)
        .with_context(|| {
            format!(
                "kernel-matrix: there is no `{CROSS_VALIDATION_ROW}` row, and the page's \
                 \"these are kernel rows\" argument rests entirely on comparing that kernel \
                 against the runner that reports the same release. Without it the page would \
                 assert the distinction and show nothing."
            )
        })?;
    let mut table = String::from(
        "| cell | this harness, bare initramfs | the CI runner, Ubuntu userspace |\n\
         |---|---|---|\n",
    );
    let mut disagreements = 0;
    for (key, runner) in RUNNER_READINGS {
        let here = row.isolation_cell(key)?;
        if here != *runner {
            disagreements += 1;
        }
        table.push_str(&format!("| `{key}` | `{here}` | `{runner}` |\n"));
    }
    table.push_str(
        "| `provisioning.subuid` | `absent` — an initramfs has no `/etc/subuid` | whatever the \
         image ships |\n",
    );
    if disagreements == 0 {
        bail!(
            "kernel-matrix: the {CROSS_VALIDATION_ROW} row now agrees with the transcribed \
             runner readings on every cell. The page's whole argument is that they DISAGREE on \
             the policy cells, so with no disagreement left the section would assert a \
             distinction its own table refutes. Re-take the runner reading before publishing."
        );
    }
    Ok(table)
}

/// The staleness paragraph, **generated from the computed delta**.
///
/// Typed prose is what this whole module exists to replace: the page carried a
/// hand-written "these rows predate P2.6.4" paragraph for exactly as long as
/// somebody remembered to write one, and nothing would have removed it when the
/// rows were re-collected or extended it when the next mechanism landed. This
/// renders from [`Staleness`], so the mechanism names on the page are the ones
/// [`staleness`] measured and the paragraph disappears by itself the day
/// `collect.sh` is run.
fn staleness_banner(stale: &Staleness, build: &BuildMechanisms) -> String {
    let list = |items: &[String]| -> String {
        items
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let dates = stale.collected.join(", ");
    let build_floor = list(&build.floor);

    if stale.is_current() {
        return format!(
            "**The rows' build half is held to this build, and that is the durable part.** \
             `cargo xtask kernel-matrix --check` reads every row's own `floor.mechanism=` and \
             `applies.*` lines and holds them to the sets declared in \
             `crates/vitrin-core/src/spawn/isolation.rs`, so **the day this build's floor moves \
             out from under these rows, this page goes RED** and names the mechanism that moved. \
             It cannot quietly go on describing an older binary, which is the failure it was \
             built after: the floor grew by two mechanisms and every gate stayed green because \
             the page and the rows were stale together. As rendered, that delta is empty — each \
             row below was collected ({dates}) against a build whose startup floor was the same \
             set this tree declares, {build_floor}.\n\n\
             Read the scope of that narrowly. This check is cheap and **re-boots nothing**, so it \
             says the rows describe THIS build and says nothing whatever about whether these \
             kernels still answer this way; only `tests/kernel-matrix/collect.sh --check` re-takes \
             that half, and every row carries the date it was last taken on.\n\n"
        );
    }

    let mut p = String::new();
    p.push_str(&format!(
        "**These rows are STALE IN THEIR BUILD HALF, and pending re-collection.** They were \
         collected ({dates}) against a build whose startup floor was {}. This tree's floor is \
         {build_floor}",
        list(&stale_floor_of(stale, build)),
    ));
    p.push_str(&format!(
        ", so every row's `--print-floor` section and startup line describe a binary that did \
         not gate on {} — and the `applies.` rows still read `not-yet` for {}, which this build \
         prints `yes` for.\n\n",
        list(&stale.floor),
        list(&stale.applied),
    ));
    p.push_str(
        "**What is still current is the half this page exists for.** The `landlock.abi`, `ns.*`, \
         `mount.in_userns`, `policy.*` and `tier` readings are **kernel** facts: they are \
         properties of the booted bytes and do not move when this repository changes. So the \
         measured table below, the admitted/refused split and the cross-validation are \
         unaffected. The `tier` column in particular: `intra-user` has always been defined as \
         all four mechanisms, and these kernels' answers to all four are what the rows already \
         carry. What is stale is every cell that describes the *binary* — the `--print-floor` \
         provenance and the verbatim startup line under each provenance block. Note the \
         direction: the old rows **understate** what this build refuses, so a reader following \
         them would expect a session to start where it now might not.\n\n",
    );
    p.push_str(&format!(
        "**This is detected, not merely written down.** `cargo xtask kernel-matrix --check` reads \
         each row's recorded mechanism sets and holds them to the sets declared in \
         `crates/vitrin-core/src/spawn/isolation.rs`. It passes today only because the difference \
         is exactly the one acknowledged in `crates/xtask/src/kernel_matrix.rs` — \
         `ROWS_PREDATE_FLOOR` names {{{}}} and `ROWS_PREDATE_APPLIED` names {{{}}} — and this \
         paragraph is generated from that measured difference rather than typed, so it names \
         what moved and cannot be left behind. Any further move goes RED and asks for a \
         re-collection by name. Re-collecting means re-booting all five kernels under QEMU \
         (`tests/kernel-matrix/collect.sh`), which no pull request runs; it is owed and is not \
         done, and this is the record of that rather than a silence.\n\n",
        list(&stale.floor),
        list(&stale.applied),
    ));
    p
}

/// The floor the rows were collected under: this build's, minus what moved.
fn stale_floor_of(stale: &Staleness, build: &BuildMechanisms) -> Vec<String> {
    build
        .floor
        .iter()
        .filter(|m| !stale.floor.contains(m))
        .cloned()
        .collect()
}

fn render(
    rows: &[Row],
    constants: &Constants,
    crossval: &str,
    stale: &Staleness,
    build: &BuildMechanisms,
) -> String {
    let mut p = String::new();
    let floor = constants.min_abi;

    p.push_str(
        "<!-- GENERATED FILE -- do not edit by hand.\n     \
         Source: crates/xtask/src/kernel_matrix.rs, rendered from the boot rows in\n     \
         tests/kernel-matrix/rows/. Regenerate with `cargo xtask kernel-matrix`.\n     \
         `cargo xtask kernel-matrix --check` fails if this file and those rows disagree. -->\n\n",
    );

    p.push_str("# Which kernels this build starts on, measured\n\n");
    p.push_str(&format!(
        "Each row below is a distribution kernel — {n} of them — booted under QEMU with the\n\
         **shipped `vitrind`** in a minimal initramfs, and this page is their answers. Every cell below is a line that\n\
         binary printed on that kernel — not a lookup table of when a Landlock ABI landed in\n\
         mainline, and not a claim about the distributions those kernels come from.\n\n\
         The rows live in `tests/kernel-matrix/rows/`, one file per kernel, holding\n\
         `vitrind --print-isolation` and `vitrind --print-floor` verbatim plus the startup\n\
         line. This page is rendered from them; it measures nothing itself.\n\n\
         **This build's floor is Landlock ABI {floor}** — `build.landlock_min_abi`, printed by\n\
         `vitrind --print-floor`. A kernel below it is refused at startup rather than confined\n\
         at a weaker rung. What each rung of the ABI buys, and why the floor sits where it\n\
         does, is [the isolation matrix](isolation-matrix.md) — a **build**-static page that\n\
         probes no machine. Do not read the two as one document: that page says what this\n\
         build requires, this one says what these {n} kernels answered.\n\n",
        n = rows.len(),
    ));
    p.push_str(&staleness_banner(stale, build));

    // ---------------------------------------------------------------- table
    p.push_str("## The measured table\n\n");
    p.push_str(
        "| kernel `release` | shipped by | `landlock.abi` | `ns.all` | `mount.in_userns` | \
         `tier` | `--isolation=default` |\n|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let note = note_for(&row.id);
        let verdict = if row.verdict == "admitted" {
            "**starts**".to_string()
        } else {
            format!(
                "**refused** — `below-floor(abi={},required={floor})`",
                row.isolation
                    .get("landlock.abi")
                    .map(String::as_str)
                    .unwrap_or("?")
            )
        };
        p.push_str(&format!(
            "| `{}` | {} | {} | `{}` | `{}` | `{}` | {} |\n",
            row.release,
            note.distribution,
            row.isolation
                .get("landlock.abi")
                .map(String::as_str)
                .unwrap_or("?"),
            row.isolation
                .get("ns.all")
                .map(String::as_str)
                .unwrap_or("?"),
            row.isolation
                .get("mount.in_userns")
                .map(String::as_str)
                .unwrap_or("?"),
            row.isolation.get("tier").map(String::as_str).unwrap_or("?"),
            verdict,
        ));
    }
    p.push('\n');

    let yes = admitted(rows);
    let no = refused(rows);
    p.push_str(&format!(
        "So of the five: **{} start and {} are refused**, and the boundary is exactly the floor.\n\n",
        yes.len(),
        no.len()
    ));
    p.push_str("Admitted:\n\n");
    for row in &yes {
        p.push_str(&format!(
            "- `{}` ({}) — `landlock.abi={}`, at or above the floor of {floor}.\n",
            row.release,
            note_for(&row.id).distribution,
            row.isolation
                .get("landlock.abi")
                .map(String::as_str)
                .unwrap_or("?"),
        ));
    }
    p.push_str("\nRefused:\n\n");
    for row in &no {
        p.push_str(&format!(
            "- `{}` ({}) — `landlock.abi={}`, below the floor of {floor}. The remedy is a newer\n  \
             kernel, and specifically **not** a sysctl, an `lsm=` edit or a `CONFIG_` change:\n  \
             this kernel's Landlock is present, enabled and answering.\n",
            row.release,
            note_for(&row.id).distribution,
            row.isolation
                .get("landlock.abi")
                .map(String::as_str)
                .unwrap_or("?"),
        ));
    }
    p.push('\n');

    // The unreported rungs are COUNTED, never typed: a sixth kernel that filled
    // one in would otherwise leave a published number describing the old set.
    // The same derivation names them for the two plan surfaces that enumerate
    // them -- see [`unreported_rungs_sentence`], which exists because the count
    // was held here and the names were not.
    let unreported = unreported_rungs(rows, constants).len();
    p.push_str(&format!(
        "All {n} rows report `ns.all=available` and `mount.in_userns=available`, so the\n\
         Landlock floor is the only thing separating the two groups. **{unreported} of this\n\
         build's {rungs} rungs are reported by none of these kernels** — these are {n} machines\n\
         somebody might be refused on, not a sweep of the ABI ladder, and no row here says\n\
         anything about a kernel that is not in the table.\n\n",
        n = rows.len(),
        rungs = constants.max_rung,
    ));

    // ---------------------------------------------------- why kernel rows
    p.push_str("## These are KERNEL rows. They are not distribution rows\n\n");
    p.push_str(&format!(
        "Each boot loads a distribution's unmodified `vmlinuz` and then runs a **minimal\n\
         initramfs of this repository's own making**: a static PID 1 (`tests/kernel-matrix/init.c`),\n\
         `/vitrind` and its library closure, `/proc` `/sys` `/dev` `/run` mounted, uid 0 with a\n\
         full capability set — and **no distribution userspace at all**. No AppArmor or SELinux\n\
         policy is loaded, no `/etc/subuid` exists, no sysctl file from `/etc/sysctl.d` has been\n\
         applied, no container runtime has adjusted anything.\n\n\
         That is a deliberate choice and it decides what the rows mean. A kernel is the same\n\
         kernel wherever it boots, so `landlock.abi` and the namespace rows are properties of\n\
         these bytes. The **policy** rows are not: they are properties of a running system, and\n\
         this one is bare.\n\n\
         **The cross-validation that settles it.** The last row is the same kernel release the\n\
         runners this repository's CI uses report. Booted here it agrees with that runner on\n\
         the kernel facts and disagrees with it on the policy facts. The left column is read\n\
         out of the checked-in row; the right column is transcribed from a CI job log:\n\n\
         {crossval}\n\
         Same kernel, same binary, two different answers — and the ones that moved are exactly\n\
         the policy cells. So this method can produce a kernel row and can never produce a\n\
         distribution row. **A distribution row has to come from that distribution**, which for\n\
         this repository means the runner's own `--print-isolation` output, printed by the\n\
         `What confinement this runner actually grants` step in\n\
         [`.github/workflows/ci.yml`](https://github.com/vitrin-os/vitrin-os/blob/main/.github/workflows/ci.yml).\n\
         The runner column above is that step's reading, transcribed from a job log GitHub\n\
         expires; it is not an artefact in this tree, and [the limits page](limits.md)\n\
         publishes that bound.\n\n\
         This is also why `tests/kernel-matrix/kernels.manifest` has no policy-variant record.\n\
         Booting one of these kernels with an AppArmor sysctl flipped on the command line would\n\
         produce a row that *looks* like a distribution row while still being a kernel row with\n\
         one knob moved, which is the confusion this whole section exists to prevent.\n\n"
    ));

    // ------------------------------------------- the AppArmor interaction
    p.push_str("## Ubuntu 24.04 needs the AppArmor profile *and* is refused anyway\n\n");
    p.push_str(&format!(
        "This one is not obvious and it undercuts something this repository shipped, so it is\n\
         stated plainly rather than left to be inferred from the table.\n\n\
         PR #290 added an AppArmor profile for `vitrind` (`packaging/apparmor/vitrind`), because\n\
         Ubuntu 24.04 denies the capabilities `vitrind` needs *inside a user namespace it has\n\
         already granted* — issue #286. **That profile is measured working on one kernel on one\n\
         CI image** — [the limits page](limits.md) carries what the `apparmor-profile` job\n\
         reported, and the bound it did not clear. Nothing below depends on which way that\n\
         went.\n\n\
         **What this page adds is that the profile cannot be sufficient on Ubuntu 24.04's own\n\
         GA kernel, whatever the job reports.** That kernel is `6.8.0-139-generic`, measured\n\
         above at `landlock.abi=4` — below this build's floor of {floor}. So on a stock 24.04,\n\
         even granting the profile everything it is meant to grant:\n\n\
         1. the profile is aimed at the **namespace** refusal, and then\n\
         2. the isolation preflight refuses the session at the next gate anyway, on the\n\
            **Landlock floor**, with `below-floor(abi=4,required={floor})`.\n\n\
         The two refusals are independent and their remedies are disjoint — no AppArmor policy\n\
         changes the number a kernel reports for its Landlock ABI. So on a stock 24.04 a working\n\
         profile would change *which* refusal you get, not *whether* you get one; the remedy for\n\
         the second is a newer kernel. Only a 24.04 running a **newer HWE or cloud kernel** —\n\
         the `6.17.0-1020-azure` row above is one, at ABI 7 — is a machine where the profile is\n\
         the only thing standing between it and a session.\n\n\
         That is a real qualification on what PR #290 bought, and it belongs beside every\n\
         description of the profile rather than only here. [The limits page](limits.md) carries\n\
         it in the `host-must-have-landlock` entry.\n\n"
    ));

    // ------------------------------------------------------ what each row is for
    p.push_str("## Why each kernel is in the set\n\n");
    for row in rows {
        let note = note_for(&row.id);
        p.push_str(&format!(
            "- **`{}`** — {} ({}). {}\n",
            row.release, note.distribution, row.class, note.why
        ));
    }
    p.push('\n');

    // --------------------------------------------------------- provenance
    p.push_str("## Provenance, per row\n\n");
    p.push_str(
        "A row is only worth as much as its ability to be re-taken. Each block below carries\n\
         the bytes that were booted, where they came from, and the command that booted them.\n\
         The **sha256 is the identity and the URL is only where those bytes were found on the\n\
         collection date**: distribution pools prune superseded kernels, so a dead URL is\n\
         expected eventually and the remedy is another mirror serving the same checksum — never\n\
         a re-measurement against whatever the pool holds now.\n\n",
    );
    for row in rows {
        let note = note_for(&row.id);
        p.push_str(&format!(
            "### `{}` — {}\n\n",
            row.release, note.distribution
        ));
        p.push_str(&format!(
            "| | |\n|---|---|\n\
             | row | [`{ROWS_DIR}/{id}.row`](https://github.com/vitrin-os/vitrin-os/blob/main/{ROWS_DIR}/{id}.row) |\n\
             | collected | {collected} (UTC) |\n\
             | `vitrind` version | {version} |\n\
             | `--print-isolation` schema | `{schema}` |\n\
             | package | `{url}` |\n\
             | package sha256 | `{deb_sha}` |\n\
             | vmlinuz | `{member}` |\n\
             | vmlinuz sha256 | `{vmlinuz_sha}` |\n\
             | boot | `{boot}` (`<accel>` was `{accel}`) |\n\
             | userspace | {userspace} |\n\n",
            id = row.id,
            collected = row.collected,
            version = row
                .floor
                .get("build.version")
                .map(String::as_str)
                .unwrap_or("unrecorded"),
            schema = row.schema,
            url = row.source_url,
            deb_sha = row.source_sha256,
            member = row.vmlinuz_member,
            vmlinuz_sha = row.vmlinuz_sha256,
            boot = row.boot,
            accel = row.accel,
            userspace = row.userspace,
        ));
        p.push_str("The startup line this kernel produced, verbatim:\n\n```text\n");
        p.push_str(&row.verdict_line);
        p.push_str("\n```\n\n");
    }

    // -------------------------------------------------------- one caveat
    p.push_str("## One cell is normalized, and it is named\n\n");
    p.push_str(
        "`policy.max_user_namespaces` is derived from guest memory and tracks the size of the\n\
         compressed initramfs, so it moves whenever `vitrind`'s binary changes size — which is\n\
         most commits, including ones that touch nothing here. Publishing it raw would make\n\
         every unrelated change look like a kernel behaving differently. The rows therefore\n\
         replace that one value with a placeholder in the compared body and keep the raw\n\
         reading in the row's own header, where `--check` ignores it. It is the only cell that\n\
         gets this treatment, and every other line of every row is compared byte-for-byte.\n\n",
    );

    // ---------------------------------------------------------- runbook
    p.push_str("## Runbook\n\n");
    p.push_str(&format!(
        "**No pull request boots a kernel.** Collecting the rows needs QEMU, roughly 220 MiB of\n\
         downloaded kernel packages and about fifteen seconds of emulation, and wiring that\n\
         into per-PR CI would buy a check on something that changes when a *distribution* ships\n\
         a kernel — not when this repository changes. It is a command a person runs, and the\n\
         rows carry a collection date so a reader can see how stale the answer is.\n\n\
         ```console\n\
         # Re-measure every kernel and rewrite tests/kernel-matrix/rows/. Needs qemu.\n\
         $ cargo build --release --bin vitrind\n\
         $ tests/kernel-matrix/collect.sh\n\n\
         # Re-measure and DIFF against what is checked in; writes nothing. Red if a kernel\n\
         # now answers differently, or if a row is older than {max_age} days.\n\
         $ tests/kernel-matrix/collect.sh --check\n\n\
         # One kernel only. Says loudly that it is a partial run.\n\
         $ tests/kernel-matrix/collect.sh --only debian-13-6.12\n\n\
         # Re-render THIS PAGE from the checked-in rows. No qemu, no network.\n\
         $ cargo xtask kernel-matrix\n\
         $ cargo xtask kernel-matrix --check   # what CI runs on every pull request\n\
         ```\n\n\
         The two `--check`s prove different things and neither substitutes for the other:\n\
         `cargo xtask kernel-matrix --check` holds this **page** to the **rows**, and\n\
         `collect.sh --check` holds the **rows** to the **kernels**. A green pull request means\n\
         the first one passed. It says nothing about whether these kernels still answer this\n\
         way.\n\n\
         A failed boot never produces a row. The collector requires its init's sentinels, a\n\
         zero exit from each probe, a schema-tagged and complete reading, exactly one startup\n\
         verdict, and a `kernel.release` equal to the one the manifest pins — and it prints\n\
         `FAIL:` and exits nonzero on any of them. There is no branch in it that emits an empty\n\
         cell, a default, or an \"unmeasured\" that reads like an answer.\n\n",
        max_age = 180,
    ));

    // ------------------------------------------------------- what is not here
    p.push_str("## What is NOT on this page\n\n");
    p.push_str(&format!(
        "- **A distribution matrix.** See the section above: every row here ran with no\n  \
         distribution policy loaded. \"Does vitrind run on Ubuntu 24.04\" is not answered by\n  \
         the `6.8.0-139-generic` row alone — that row answers \"does this build's floor admit\n  \
         Ubuntu 24.04's GA kernel\", and the answer is no.\n\
         - **One row per ABI rung.** Five kernels reported five distinct ABIs; four of this\n  \
         build's nine rungs are reported by none of them. The per-rung behaviour table is\n  \
         [the isolation matrix](isolation-matrix.md), which is generated from source and\n  \
         measures no machine.\n\
         - **A claim that these rows are current.** Each carries a collection date. Nothing in\n  \
         a pull request re-boots them, which is a deliberate scope decision and not an\n  \
         oversight; `collect.sh --check` is how the claim gets re-taken, and it goes red on a\n  \
         row older than {max_age} days when somebody runs it.\n\
         - **Anything about non-x86-64, or about kernels not in the table.** No row here says\n  \
         where between ABI {below} and ABI {floor} a given mainline release sits. That mapping\n  \
         is a fact about mainline, and this page publishes measurements rather than lookups.\n\n",
        max_age = 180,
        below = floor.saturating_sub(1),
    ));

    p
}

/// `cargo xtask kernel-matrix [--check]`.
pub fn kernel_matrix(root: &Path, check: bool) -> Result<()> {
    let lib_rs = fs::read_to_string(root.join(REALM_INIT_LIB_RS))
        .with_context(|| format!("kernel-matrix: reading {REALM_INIT_LIB_RS}"))?;
    let constants = Constants::from_source(&lib_rs)?;
    let isolation_rs = fs::read_to_string(root.join(CORE_ISOLATION_RS))
        .with_context(|| format!("kernel-matrix: reading {CORE_ISOLATION_RS}"))?;
    let build = BuildMechanisms::from_source(&isolation_rs)?;
    let manifest = fs::read_to_string(root.join(MANIFEST))
        .with_context(|| format!("kernel-matrix: reading {MANIFEST}"))?;

    let rows = load_rows(root)?;
    if rows.is_empty() {
        bail!(
            "kernel-matrix: no rows were loaded. A generator that rendered a page from nothing \
             would publish an empty table as an answer."
        );
    }
    validate(&rows, &constants, &manifest)?;
    check_unreported_enumeration(root, &rows, &constants)?;
    let stale = staleness(&rows, &build)?;

    let crossval = cross_validation(&rows)?;
    let page = render(&rows, &constants, &crossval, &stale, &build);
    let path = root.join(PAGE);

    if check {
        let current = fs::read_to_string(&path).with_context(|| {
            format!("kernel-matrix: reading {PAGE}. Generate it with `cargo xtask kernel-matrix`.")
        })?;
        if current != page {
            bail!(
                "kernel-matrix: {PAGE} is not what the generator emits from the checked-in rows \
                 in {ROWS_DIR}. Either the page was hand-edited -- it is generated, so edit \
                 crates/xtask/src/kernel_matrix.rs instead -- or the rows were re-collected \
                 without re-rendering. Run `cargo xtask kernel-matrix` and commit the result."
            );
        }
        eprintln!(
            "xtask: kernel-matrix --check: {PAGE} matches {} checked-in rows",
            rows.len()
        );
        // Green, and NOT current. Printed at every `--check` so the debt is in
        // front of whoever runs the command and not only in front of whoever
        // reads the page -- a pass that says nothing is how this went unseen
        // in the first place.
        if !stale.is_current() {
            eprintln!(
                "xtask: kernel-matrix --check: the rows are STALE IN THEIR BUILD HALF and \
                 pending re-collection. This build gates on {:?} and applies {:?} that no row \
                 was collected under; the difference is the acknowledged one \
                 (ROWS_PREDATE_FLOOR / ROWS_PREDATE_APPLIED in \
                 crates/xtask/src/kernel_matrix.rs) and is published on {PAGE}. Clear it with \
                 tests/kernel-matrix/collect.sh (needs QEMU).",
                stale.floor, stale.applied,
            );
        }
    } else {
        fs::write(&path, &page).with_context(|| format!("kernel-matrix: writing {PAGE}"))?;
        eprintln!(
            "xtask: kernel-matrix: wrote {PAGE} ({} kernels, floor ABI {})",
            rows.len(),
            constants.min_abi
        );
        eprintln!("xtask: kernel-matrix complete -- review `git diff` and commit.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_path_buf()
    }

    fn constants() -> Constants {
        let lib_rs = fs::read_to_string(root().join(REALM_INIT_LIB_RS)).expect("lib.rs");
        Constants::from_source(&lib_rs).expect("constants")
    }

    fn build() -> BuildMechanisms {
        let src = fs::read_to_string(root().join(CORE_ISOLATION_RS)).expect("isolation.rs");
        BuildMechanisms::from_source(&src).expect("the build's mechanism sets")
    }

    fn page_of(rows: &[Row]) -> String {
        let build = build();
        let stale = staleness(rows, &build).expect("the checked-in rows must validate");
        let crossval = cross_validation(rows).expect("the cross-validation row must render");
        render(rows, &constants(), &crossval, &stale, &build)
    }

    /// The checked-in page is what the checked-in rows render to.
    ///
    /// This is `--check` as a test, so a hand edit to the page is red in the
    /// ordinary suite as well as in the job that runs the command.
    #[test]
    fn the_checked_in_page_matches_the_checked_in_rows() {
        let rows = load_rows(&root()).expect("rows must load");
        assert_eq!(
            rows.len(),
            KERNELS.len(),
            "every KERNELS entry must have a row"
        );
        let page = page_of(&rows);
        let current = fs::read_to_string(root().join(PAGE)).expect("the page must exist");
        assert_eq!(
            current, page,
            "docs/book/src/isolation-kernels.md is generated; run `cargo xtask kernel-matrix`"
        );
    }

    /// The rungs no measured kernel reports are what the rows say they are.
    ///
    /// Pinned rather than only derived, so a re-collection that changes the set
    /// has to change this assertion too and cannot slide past as "the
    /// generator agrees with itself". The values are the `landlock.abi` cells
    /// of the five checked-in rows -- 1, 2, 4, 6, 7 -- against a ceiling of 9.
    #[test]
    fn the_unreported_rungs_are_derived_from_the_rows() {
        let rows = load_rows(&root()).expect("rows must load");
        assert_eq!(
            unreported_rungs(&rows, &constants()),
            vec![3, 5, 8, 9],
            "the checked-in rows report landlock.abi 1, 2, 4, 6 and 7"
        );
    }

    /// Both plan surfaces carry that enumeration in the generator's words.
    ///
    /// This is [`check_unreported_enumeration`] as a test, so a page that
    /// enumerates a different set is red in the ordinary suite and not only in
    /// the job that runs `cargo xtask kernel-matrix --check`.
    #[test]
    fn both_plan_surfaces_carry_the_enumeration() {
        let rows = load_rows(&root()).expect("rows must load");
        check_unreported_enumeration(&root(), &rows, &constants())
            .expect("the plan surfaces must name the rungs the rows produce");
    }

    /// And a surface naming the wrong set is refused.
    ///
    /// The wrong set here is the one that was actually published -- 4, 7 and 8
    /// -- so this test fails against the exact prose the gate was written to
    /// catch, rather than against a synthetic mutation.
    #[test]
    fn a_surface_naming_the_wrong_rungs_is_refused() {
        let rows = load_rows(&root()).expect("rows must load");
        let required = unreported_rungs_sentence(&rows, &constants());
        assert_eq!(
            required, "the rungs no measured kernel reports are **3, 5, 8 and 9**",
            "the demanded sentence is the one the plan pages carry"
        );

        let tmp = std::env::temp_dir().join(format!(
            "vitrin-kernel-matrix-enumeration-{}",
            std::process::id()
        ));
        for surface in ENUMERATING_SURFACES {
            let dst = tmp.join(surface);
            fs::create_dir_all(dst.parent().expect("a parent")).expect("mkdir");
            let text = fs::read_to_string(root().join(surface)).expect("the real surface");
            fs::write(
                &dst,
                text.replace(
                    &required,
                    "the rungs no measured kernel reports are **4, 7 and 8**",
                ),
            )
            .expect("write");
        }
        let err = check_unreported_enumeration(&tmp, &rows, &constants())
            .expect_err("a surface naming 4, 7 and 8 must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains(ENUMERATING_SURFACES[0]) && msg.contains("3, 5, 8 and 9"),
            "the refusal must name the surface and the set the rows produce: {msg}"
        );
        fs::remove_dir_all(&tmp).expect("cleanup");
    }

    /// Every provenance cell the page prints is a real reading, not a
    /// placeholder.
    ///
    /// This exists because one of them **was** a placeholder. The schema tag is
    /// the single line of the isolation section that is not `key=value`, an
    /// earlier draft read it out of the key=value map, and the published table
    /// said `vitrin-isolation ?` on all five rows. A missing reading rendered as
    /// a character is exactly the failure this whole harness is written against,
    /// so the shape is asserted rather than eyeballed.
    #[test]
    fn no_provenance_cell_renders_as_a_placeholder() {
        let rows = load_rows(&root()).expect("rows");
        for row in &rows {
            assert!(
                row.schema.starts_with("vitrin-isolation ")
                    && row
                        .schema
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|n| n.parse::<u32>().is_ok()),
                "{}: the schema tag must be `vitrin-isolation <number>`, got {:?}",
                row.id,
                row.schema
            );
            for (what, value) in [
                ("collected", &row.collected),
                ("accel", &row.accel),
                ("source.url", &row.source_url),
                ("source.sha256", &row.source_sha256),
                ("vmlinuz.member", &row.vmlinuz_member),
                ("vmlinuz.sha256", &row.vmlinuz_sha256),
                ("boot", &row.boot),
                ("userspace", &row.userspace),
                ("verdict line", &row.verdict_line),
            ] {
                assert!(
                    !value.is_empty() && !value.contains('?'),
                    "{}: the {what} cell is empty or carries a placeholder: {value:?}",
                    row.id
                );
            }
            for sha in [&row.source_sha256, &row.vmlinuz_sha256] {
                assert_eq!(
                    sha.len(),
                    64,
                    "{}: a sha256 that is not 64 hex characters is not a checksum",
                    row.id
                );
            }
            assert!(
                row.floor.contains_key("build.version"),
                "{}: the page prints the vitrind version and the row must carry it",
                row.id
            );
        }
        // And the rendered page really carries them, so a cell that stopped
        // being emitted would not pass on the strength of the struct alone.
        let page = page_of(&rows);
        assert!(
            !page.contains('?'),
            "the page carries a placeholder: look for `?`"
        );
        for row in &rows {
            assert!(
                page.contains(&row.vmlinuz_sha256),
                "{}: sha missing",
                row.id
            );
            assert!(page.contains(&row.schema), "{}: schema missing", row.id);
        }
    }

    /// Every row was collected against the floor this tree declares, and its
    /// verdict is the one the arithmetic requires.
    ///
    /// The non-vacuity is in the second half: a corrupted verdict must be
    /// rejected, or `validate` would be a function that returns `Ok`.
    #[test]
    fn a_row_collected_against_another_floor_or_carrying_a_wrong_verdict_is_refused() {
        let root = root();
        let manifest = fs::read_to_string(root.join(MANIFEST)).expect("manifest");
        let c = constants();
        let mut rows = load_rows(&root).expect("rows");
        validate(&rows, &c, &manifest).expect("the checked-in rows must validate");

        // A row from a build with a different floor.
        let saved = rows[0]
            .floor
            .insert("build.landlock_min_abi".into(), (c.min_abi + 1).to_string());
        let err = validate(&rows, &c, &manifest)
            .expect_err("a row from another build's floor must be refused");
        assert!(
            format!("{err}").contains("was collected against a build whose floor"),
            "got: {err}"
        );
        rows[0]
            .floor
            .insert("build.landlock_min_abi".into(), saved.expect("was present"));
        validate(&rows, &c, &manifest).expect("restoring must restore");

        // A verdict that contradicts the ABI the same row reports.
        let flipped = if rows[0].verdict == "admitted" {
            "refused"
        } else {
            "admitted"
        };
        rows[0].verdict = flipped.to_string();
        let err = validate(&rows, &c, &manifest)
            .expect_err("a verdict contradicting the row's own ABI must be refused");
        assert!(
            format!("{err}").contains("this generator will not pick a winner"),
            "got: {err}"
        );
    }

    /// Both directions of the row set / KERNELS correspondence.
    #[test]
    fn the_kernel_notes_and_the_row_files_are_the_same_set() {
        let dir = root().join(ROWS_DIR);
        let on_disk: Vec<String> = fs::read_dir(&dir)
            .expect("rows directory")
            .filter_map(|e| {
                let name = e.ok()?.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".row").map(str::to_string)
            })
            .collect();
        for note in KERNELS {
            assert!(
                on_disk.iter().any(|id| id == note.id),
                "KERNELS names {} and there is no row for it",
                note.id
            );
        }
        for id in &on_disk {
            assert!(
                KERNELS.iter().any(|k| k.id == *id),
                "{id}.row has no KERNELS entry saying why that kernel is in the set"
            );
        }
        assert_eq!(on_disk.len(), KERNELS.len());
    }

    /// The page's headline numbers are counted from the rows, not typed.
    ///
    /// The assertion that matters is the last one: **both groups are
    /// non-empty**. A page that admitted everything or refused everything would
    /// render happily and would be evidence of nothing.
    #[test]
    fn the_admitted_and_refused_groups_are_both_non_empty_and_split_at_the_floor() {
        let rows = load_rows(&root()).expect("rows");
        let c = constants();
        for row in &rows {
            let abi = row.landlock_abi().expect("a numeric ABI");
            let expected = abi >= c.min_abi;
            assert_eq!(
                row.verdict == "admitted",
                expected,
                "{} reports ABI {abi} against floor {}, and its verdict is {}",
                row.id,
                c.min_abi,
                row.verdict
            );
        }
        assert!(
            !admitted(&rows).is_empty(),
            "no kernel in the set starts, so the table proves only that this build refuses \
             things"
        );
        assert!(
            !refused(&rows).is_empty(),
            "every kernel in the set starts, so the floor is not exercised by any row and the \
             page's refusal column is decoration"
        );
    }

    /// The cross-validation section only renders while the two columns really
    /// disagree.
    ///
    /// The page's central argument -- that this harness produces kernel rows and
    /// never distribution rows -- rests entirely on the same kernel answering
    /// differently under a distribution's policy. If the checked-in row ever
    /// came to agree with the transcribed runner readings on every cell, the
    /// section would assert a distinction its own table refutes. So the
    /// generator refuses, and this proves the refusal is reachable rather than
    /// decorative.
    #[test]
    fn the_cross_validation_refuses_to_render_when_the_two_columns_agree() {
        let mut rows = load_rows(&root()).expect("rows");
        let table = cross_validation(&rows).expect("the real rows must disagree somewhere");
        assert!(
            table.contains("restricted-by-policy(errno=13)"),
            "the runner column must be in the rendered table: {table}"
        );

        // Force agreement on every transcribed cell.
        let idx = rows
            .iter()
            .position(|r| r.id == CROSS_VALIDATION_ROW)
            .expect("the cross-validation row");
        for (key, runner) in RUNNER_READINGS {
            rows[idx]
                .isolation
                .insert((*key).to_string(), (*runner).to_string());
        }
        let err = cross_validation(&rows)
            .expect_err("a table with nothing to compare must not be published");
        assert!(
            format!("{err}").contains("agrees with the transcribed runner readings"),
            "got: {err}"
        );
    }

    /// A row missing a cell the page prints must stop the render, not render a
    /// hole.
    #[test]
    fn a_row_missing_a_printed_cell_refuses_to_render() {
        let mut rows = load_rows(&root()).expect("rows");
        rows[0].isolation.remove("landlock.abi");
        let err = rows[0]
            .landlock_abi()
            .expect_err("a row with no landlock.abi must not render");
        assert!(format!("{err:#}").contains("landlock.abi"), "got: {err:#}");
    }

    /// This build's two mechanism sets really parse out of the crate that
    /// declares them, and are not empty or identical by accident.
    #[test]
    fn the_builds_mechanism_sets_are_read_from_the_crate_that_declares_them() {
        let b = build();
        assert!(
            b.floor.contains(&"namespaces".to_string())
                && b.floor.contains(&"landlock".to_string()),
            "the floor parsed as {:?}, which is not this build's",
            b.floor
        );
        assert!(!b.applied.is_empty(), "APPLIED parsed empty");
        // The wire spellings, not the variant names -- the row files carry the
        // former, and a parser that returned `NoNewPrivs` would compare two
        // vocabularies and call every row stale.
        for m in b.floor.iter().chain(b.applied.iter()) {
            assert!(
                m.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !m.is_empty(),
                "{m:?} is not a wire spelling; the Display impl is what row files record"
            );
        }
        // FLOOR must be a subset of APPLIED -- a build that gates on something
        // it does not apply refuses machines in exchange for nothing -- and the
        // parse would be worthless if it silently returned the same list twice.
        for m in &b.floor {
            assert!(b.applied.contains(m), "{m} is gated and not applied");
        }
        // Non-vacuity for the two parsers: a source that declares neither is
        // refused rather than read as an empty set.
        assert!(BuildMechanisms::from_source("").is_err());
        assert!(mechanism_list("pub const FLOOR: &[Mechanism] = &[];", "FLOOR").is_err());
    }

    /// **The staleness detector, levered in both directions.**
    ///
    /// The gap this closes was live and measured: `cargo xtask kernel-matrix
    /// --check` passed over a floor that had grown by two mechanisms, because
    /// it compared the PAGE to the ROWS and both were stale together. So the
    /// assertions that matter here are the failures.
    #[test]
    fn a_build_half_that_moved_by_anything_but_the_acknowledgement_is_refused() {
        let rows = load_rows(&root()).expect("rows");
        let real = build();
        let stale = staleness(&rows, &real).expect("the checked-in rows must be acknowledged");

        // The acknowledgement describes the tree, exactly.
        assert_eq!(
            stale.floor,
            ROWS_PREDATE_FLOOR
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>(),
            "the measured floor delta and the published acknowledgement disagree"
        );
        assert_eq!(
            stale.applied,
            ROWS_PREDATE_APPLIED
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
        );

        // (1) A SIXTH mechanism enters the floor. Nobody acknowledged it, so
        //     the rows must be re-collected and the check says so by name.
        let mut grown = real.clone();
        grown.floor.push("landlock-net".to_string());
        grown.applied.push("landlock-net".to_string());
        let err = staleness(&rows, &grown).expect_err("an unacknowledged floor move must be red");
        let text = format!("{err}");
        assert!(
            text.contains("MUST BE RE-COLLECTED") && text.contains("landlock-net"),
            "the failure must name what moved: {text}"
        );

        // (2) A row naming a mechanism this build no longer has. A row can only
        //     be older than the build, never newer, so no acknowledgement
        //     covers this. Note this is now also what a REVERTED floor move
        //     looks like -- with the rows re-collected, dropping `seccomp` from
        //     the build makes every row newer than the build rather than
        //     stale-and-acknowledged.
        let mut shrunk = real.clone();
        shrunk.floor.retain(|m| m != "landlock");
        shrunk.applied.retain(|m| m != "landlock");
        let err = staleness(&rows, &shrunk).expect_err("a row newer than the build must be red");
        assert!(
            format!("{err}").contains("which this build no longer has"),
            "got: {err}"
        );
    }

    /// **The deferred-re-collection path, kept alive while nothing is
    /// deferred.**
    ///
    /// [`ROWS_PREDATE_FLOOR`] and [`ROWS_PREDATE_APPLIED`] are both `&[]` since
    /// the rows were re-collected, which makes two of [`staleness_with`]'s
    /// refusals unreachable through [`staleness`] -- an empty acknowledgement
    /// can never be "acknowledged but did not move", and the stale banner has no
    /// delta to render. Without this test those paths would rot silently and be
    /// found broken on the day somebody has to defer a re-collection again,
    /// which is exactly when nobody wants to debug the gate.
    ///
    /// So the acknowledgement is passed in, against the REAL rows and the REAL
    /// build, and both outcomes are asserted.
    #[test]
    fn an_acknowledged_delta_renders_the_stale_banner_and_a_wrong_one_is_refused() {
        let rows = load_rows(&root()).expect("rows");
        let real = build();

        // A build one mechanism ahead of the rows, with that mechanism
        // acknowledged: the shape the page carried between P2.6.4 and the
        // re-collection. It renders, and it renders the delta.
        let mut ahead = real.clone();
        ahead.floor.push("landlock-net".to_string());
        ahead.applied.push("landlock-net".to_string());
        let stale = staleness_with(&rows, &ahead, &["landlock-net"], &["landlock-net"])
            .expect("an acknowledged delta must render");
        assert_eq!(stale.floor, vec!["landlock-net".to_string()]);
        let banner = staleness_banner(&stale, &ahead);
        assert!(
            banner.contains("STALE IN THEIR BUILD HALF") && banner.contains("`landlock-net`"),
            "the stale banner must name the acknowledged mechanism: {banner}"
        );

        // The same acknowledgement against a build that did NOT move. The page
        // would publish a staleness that is not there, so it is red -- the
        // direction that catches an acknowledgement nobody cleared after
        // running the collector.
        let err = staleness_with(&rows, &real, &["landlock-net"], &[])
            .expect_err("an acknowledgement for a move that did not happen must be red");
        assert!(format!("{err}").contains("did NOT move"), "got: {err}");
    }

    /// The page publishes what the gate GUARANTEES, not a freshness assertion.
    ///
    /// The rows are current as this is written, and a page that simply said so
    /// would start decaying the moment it was rendered -- the exact shape this
    /// repository has been bitten by. What the page must carry instead is the
    /// durable half: that a floor move reddens this gate, and that the cheap
    /// check says nothing about the kernels.
    #[test]
    fn the_page_publishes_the_guarantee_rather_than_a_freshness_claim() {
        let rows = load_rows(&root()).expect("rows");
        let page = page_of(&rows);
        assert!(
            !page.contains("STALE IN THEIR BUILD HALF"),
            "the rows are not stale, so the page must not say they are"
        );
        assert!(
            page.contains("goes RED"),
            "the page must say what the gate does when the floor moves, not merely that the \
             rows happen to match today"
        );
        // The scope limit is as load-bearing as the guarantee: this check never
        // boots anything, and a reader must not take a green page as evidence
        // that these kernels were re-measured.
        assert!(
            page.contains("re-boots nothing"),
            "the page must bound the cheap check's scope"
        );
        assert!(
            page.contains("tests/kernel-matrix/collect.sh --check"),
            "the page must name the only thing that re-takes the kernel half"
        );
        // Provenance survives: every row's collection date is still published.
        for row in &rows {
            assert!(
                page.contains(&row.collected),
                "the page must publish {}'s collection date",
                row.id
            );
        }
    }

    /// **Both banners render**, so the one that is not currently on the page is
    /// a branch rather than dead text.
    ///
    /// The rows are current, so the STALE banner is now the unexercised half --
    /// the mirror image of why this test was written, and the same hazard. A
    /// generator that can only emit the paragraph it happens to emit today is
    /// one whose other paragraph is discovered broken while somebody is already
    /// dealing with a moved floor.
    #[test]
    fn the_stale_banner_and_the_current_banner_are_both_reachable() {
        let b = build();
        let stale = Staleness {
            floor: vec!["seccomp".into()],
            applied: vec!["seccomp".into()],
            collected: vec!["2026-08-15".into()],
        };
        let current = Staleness {
            floor: Vec::new(),
            applied: Vec::new(),
            collected: vec!["2026-08-16".into()],
        };
        assert!(!stale.is_current() && current.is_current());
        let stale_text = staleness_banner(&stale, &b);
        let current_text = staleness_banner(&current, &b);
        assert!(stale_text.contains("STALE IN THEIR BUILD HALF"));
        // The current banner leads with the GUARANTEE, and the guarantee is
        // that a floor move reddens this gate. A banner that merely announced
        // the rows were current would be a claim with a shelf life.
        assert!(
            current_text.contains("goes RED") && current_text.contains("re-boots nothing"),
            "the current banner must publish the guarantee and its scope: {current_text}"
        );
        assert!(
            !current_text.contains("STALE"),
            "the current banner must not carry the stale one's words: {current_text}"
        );
        assert_ne!(stale_text, current_text);
    }

    /// The parser reads the fenced sections, and reads them as sections.
    #[test]
    fn the_row_parser_separates_the_two_probe_sections() {
        let rows = load_rows(&root()).expect("rows");
        for row in &rows {
            assert!(
                row.isolation.contains_key("kernel.release"),
                "{}: the isolation section must carry kernel.release",
                row.id
            );
            assert!(
                !row.isolation.contains_key("build.landlock_min_abi"),
                "{}: a build constant leaked into the kernel-facts section",
                row.id
            );
            assert!(
                row.floor.contains_key("build.landlock_min_abi"),
                "{}: the floor section must carry the build's floor",
                row.id
            );
            assert_eq!(
                row.isolation.get("kernel.release").map(String::as_str),
                Some(row.release.as_str()),
                "{}: the header's release and the probe's release must agree",
                row.id
            );
        }
    }
}
