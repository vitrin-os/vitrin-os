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
//!    an empty cell rendered into a published table reads as a fact.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::isolation_matrix::Constants;

/// Where the collector writes, and where this reads.
const ROWS_DIR: &str = "tests/kernel-matrix/rows";
/// The record set the rows must correspond to, one-for-one.
const MANIFEST: &str = "tests/kernel-matrix/kernels.manifest";
/// The page this emits.
const PAGE: &str = "docs/book/src/isolation-kernels.md";
/// Where the floor constant is declared.
const REALM_INIT_LIB_RS: &str = "crates/vitrin-realm-init/src/lib.rs";

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
        why: "The cross-validation row. It is the same kernel release the CI runner reports, so \
              booting it here says whether this harness reproduces that machine — and, on the \
              policy cells, whether it does not.",
    },
];

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
    /// the single-valued keys this page reads are kept.
    floor: BTreeMap<String, String>,
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
    let floor = kv(&section(text, "--- vitrind --print-floor ---"));
    if isolation.is_empty() {
        bail!("kernel-matrix: {id}.row holds no `--print-isolation` section");
    }
    if floor.is_empty() {
        bail!("kernel-matrix: {id}.row holds no `--print-floor` section");
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

fn render(rows: &[Row], constants: &Constants, crossval: &str) -> String {
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
    let reported: Vec<u32> = rows.iter().filter_map(|r| r.landlock_abi().ok()).collect();
    let unreported = (1..=constants.max_rung)
        .filter(|rung| !reported.contains(rung))
        .count();
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

    let crossval = cross_validation(&rows)?;
    let page = render(&rows, &constants, &crossval);
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
        let crossval = cross_validation(&rows).expect("the cross-validation row must render");
        let page = render(&rows, &constants(), &crossval);
        let current = fs::read_to_string(root().join(PAGE)).expect("the page must exist");
        assert_eq!(
            current, page,
            "docs/book/src/isolation-kernels.md is generated; run `cargo xtask kernel-matrix`"
        );
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
        let page = render(
            &rows,
            &constants(),
            &cross_validation(&rows).expect("cross-validation"),
        );
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
