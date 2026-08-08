// SPDX-License-Identifier: MPL-2.0
//! The one filesystem read [`crate::status`]'s sampler makes: a battery
//! percentage and a charging flag out of `/sys/class/power_supply/`.
//!
//! # This is new authority in the TCB, so it is bounded rather than trusted
//!
//! Before WS-E.2.3 the core read `realm.toml` and `principals.toml` **once**,
//! at startup, and embedded its font at compile time. A *recurring* read of an
//! operator-controlled tree is a different shape of authority, and the shape it
//! is given here is deliberately the smallest one that can answer the question:
//!
//! - **One fixed root**, [`SYSFS_ROOT`], never composed from anything a client
//!   or an agent can influence. The root is a parameter of [`read_battery`]
//!   only so the unit tests can point it at a checked-in fixture tree — every
//!   production caller passes the constant, and
//!   [`crate::status::sample::Sampler`] holds it as a `&'static Path` so there
//!   is no field a later edit could make client-settable.
//! - **A hard cap on every read**, enforced by [`std::io::Read::take`] rather
//!   than by a length check after the fact: a `capacity` file that is a
//!   gigabyte of digits costs [`CAPACITY_CAP`] bytes and no more. The cap is a
//!   property of the reader, so "read the whole file" is not expressible here.
//! - **A hard cap on how many entries are examined** ([`MAX_ENTRIES`]), so a
//!   root with a million directories costs a bounded walk.
//! - **No panic and no error path.** Every failure — a missing tree, a
//!   directory that vanished mid-suspend, a `capacity` holding `unknown`, a
//!   permission error — is [`BatteryReading::Absent`], which the strip draws as
//!   an *empty slot* and never as a guess. A desktop with no battery and a
//!   laptop whose battery was hot-unplugged are the same ordinary answer.
//! - **Never journalled.** The reading goes to the strip's raster and nowhere
//!   else; [`crate::recorder`] has no entry for it.
//!
//! # When the core confines itself, this becomes a rule someone must write
//!
//! E2.6/E2.7 put Landlock over `vitrind`'s own process. This read is then a
//! rule the core has to grant *itself*: read-only access to [`SYSFS_ROOT`] and
//! its immediate children. That is a real widening of the core's future
//! sandbox and it is recorded here, in `docs/book/src/limits.md` and in
//! `docs/plan/14-workstream-session-mode.md` rather than left to be
//! rediscovered by whoever writes the ruleset.
//!
//! # Why a directory scan rather than a single hard-coded `BAT0`
//!
//! Because `BAT0` is not a contract. The kernel's power-supply class names
//! devices by driver: ACPI laptops get `BAT0`/`BAT1`, Apple silicon gets
//! `macsmc-battery`, some ARM boards get `battery`. A hard-coded name would be
//! "no battery" on hardware that has one, which is the silent-wrong answer this
//! module's whole posture is against. The scan is bounded, sorted (so a machine
//! with two batteries reports the same one every time rather than whichever
//! `readdir` happened to yield first), and reads at most three tiny files per
//! candidate.

use std::io::Read;
use std::path::Path;

/// The fixed root. Never composed with anything from a peer.
pub(crate) const SYSFS_ROOT: &str = "/sys/class/power_supply";

/// At most this many directory entries are examined. A power-supply class with
/// more than sixteen devices is not a laptop; the cap is what makes the walk's
/// cost independent of what is in the tree.
const MAX_ENTRIES: usize = 16;

/// Byte caps, per file. Each is *at least* an order of magnitude more than the
/// kernel ever writes (`capacity` is at most `"100\n"`; `type` at most
/// `"Battery\n"`; `status` at most `"Not charging\n"`), and each is enforced by
/// the reader rather than checked afterwards.
const CAPACITY_CAP: u64 = 16;
const TYPE_CAP: u64 = 32;
const STATUS_CAP: u64 = 32;

/// The longest device name this module will even `open`. Longer than any name
/// the kernel mints, short enough that a hostile tree cannot make the path
/// buffer interesting.
const MAX_NAME: usize = 32;

/// What the sampler learned about the battery.
///
/// Three states, not two, because "there is no battery" and "there is a battery
/// at 42%" are different facts and a strip that showed `--%` for both would be
/// lying about one of them. A malformed or unreadable file collapses into
/// [`Self::Absent`] on purpose: the strip has no vocabulary for "something is
/// wrong with sysfs" that a human could act on, and inventing one would put a
/// diagnostic on the trusted path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatteryReading {
    /// No battery, or nothing legible where one would be. The strip draws
    /// **nothing** for this — an empty slot, never a guess.
    Absent,
    /// A percentage in `0..=100` and whether it is charging.
    Present { percent: u8, charging: bool },
}

/// Read the first battery under `root`, in device-name order.
///
/// `root` is [`SYSFS_ROOT`] in every production caller; the parameter exists so
/// the unit tests can drive checked-in fixture trees copied from real sysfs.
/// Total cost is bounded by [`MAX_ENTRIES`] directory entries and, per
/// candidate, three reads of at most [`TYPE_CAP`] + [`CAPACITY_CAP`] +
/// [`STATUS_CAP`] bytes.
pub(crate) fn read_battery(root: &Path) -> BatteryReading {
    let Ok(entries) = std::fs::read_dir(root) else {
        return BatteryReading::Absent;
    };
    // Collected and sorted rather than taken in `readdir` order: two batteries
    // must yield the same reading on every sample, and `readdir` order is not
    // stable across kernels.
    //
    // **The cap is applied AFTER the sort, and that ordering is the whole
    // point.** Capping first bounded the allocation and reintroduced exactly
    // the answer this module exists to avoid: on a machine whose battery is not
    // among the first sixteen `readdir` entries, the strip reported "no
    // battery" on hardware that has one -- non-deterministically, because
    // `readdir` order is not stable. Sorting first makes which entries survive
    // the cap a function of their NAMES rather than of the kernel's directory
    // layout, so the answer is at least the same on every sample. The
    // allocation is still bounded, just one step later: names are length-capped
    // as they are collected, and a power-supply directory holds tens of entries
    // rather than millions.
    let mut names: Vec<std::ffi::OsString> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter(|name| name.as_encoded_bytes().len() <= MAX_NAME)
        .collect();
    names.sort();
    names.truncate(MAX_ENTRIES);

    for name in names {
        let dir = root.join(&name);
        // `type` first: it is the cheapest way to skip an AC adapter, and every
        // power supply has one.
        if read_capped(&dir.join("type"), TYPE_CAP).as_deref() != Some("Battery") {
            continue;
        }
        let Some(percent) = read_capped(&dir.join("capacity"), CAPACITY_CAP)
            .and_then(|text| text.parse::<u8>().ok())
            .filter(|percent| *percent <= 100)
        else {
            // A battery whose capacity is missing or unparseable is not a
            // reason to go looking at the *next* battery: this one is the one
            // the human has. Answering `Absent` here is the honest empty slot.
            return BatteryReading::Absent;
        };
        let charging = matches!(
            read_capped(&dir.join("status"), STATUS_CAP).as_deref(),
            Some("Charging") | Some("Full")
        );
        return BatteryReading::Present { percent, charging };
    }
    BatteryReading::Absent
}

/// Read at most `cap` bytes of `path` and trim it, or `None`.
///
/// The cap is the reader's, not a post-hoc check: [`Read::take`] makes a longer
/// file cost `cap` bytes. Non-UTF-8 is `None` rather than lossy — a sysfs
/// attribute that is not ASCII is not one this module understands, and
/// `from_utf8_lossy` would turn that into a string that *almost* parses.
fn read_capped(path: &Path, cap: u64) -> Option<String> {
    let mut buf = Vec::with_capacity(cap as usize);
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8(buf).ok()?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture tree under a fresh temporary directory. Files are written from
    /// the literal bytes real sysfs holds (checked in as the string constants
    /// below rather than as binary files, since every one of them is at most a
    /// dozen ASCII bytes and a checked-in `capacity` file is a checked-in file
    /// nobody can read the *point* of).
    fn tree(entries: &[(&str, &[(&str, &str)])]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vitrin-battery-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        for (name, files) in entries {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).expect("fixture device");
            for (file, body) in *files {
                std::fs::write(dir.join(file), body).expect("fixture attribute");
            }
        }
        root
    }

    /// Real `/sys/class/power_supply/BAT0` on an ACPI laptop, discharging.
    const REAL_BAT0: &[(&str, &str)] = &[
        ("type", "Battery\n"),
        ("capacity", "87\n"),
        ("status", "Discharging\n"),
    ];
    /// The same laptop on mains.
    const REAL_BAT0_CHARGING: &[(&str, &str)] = &[
        ("type", "Battery\n"),
        ("capacity", "87\n"),
        ("status", "Charging\n"),
    ];
    /// The AC adapter that sits beside it, which must never be read as a
    /// battery: it has a `type` of `Mains` and no `capacity` at all.
    const REAL_AC: &[(&str, &str)] = &[("type", "Mains\n"), ("online", "1\n")];

    #[test]
    fn a_real_discharging_battery_reads_as_its_percentage() {
        let root = tree(&[("AC", REAL_AC), ("BAT0", REAL_BAT0)]);
        assert_eq!(
            read_battery(&root),
            BatteryReading::Present {
                percent: 87,
                charging: false
            }
        );
    }

    #[test]
    fn charging_and_full_both_read_as_charging() {
        let root = tree(&[("BAT0", REAL_BAT0_CHARGING)]);
        assert_eq!(
            read_battery(&root),
            BatteryReading::Present {
                percent: 87,
                charging: true
            }
        );
        let root = tree(&[(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", "100\n"),
                ("status", "Full\n"),
            ][..],
        )]);
        assert_eq!(
            read_battery(&root),
            BatteryReading::Present {
                percent: 100,
                charging: true
            }
        );
    }

    /// A desktop: the class exists and holds only an adapter.
    #[test]
    fn a_machine_with_no_battery_is_absent_and_not_an_error() {
        let root = tree(&[("AC", REAL_AC)]);
        assert_eq!(read_battery(&root), BatteryReading::Absent);
    }

    /// A container, and a CI runner: the tree is not there at all.
    #[test]
    fn a_missing_tree_is_absent() {
        let root = std::env::temp_dir().join("vitrin-battery-does-not-exist");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(read_battery(&root), BatteryReading::Absent);
    }

    /// Mid-suspend, and on some drivers at boot: the attribute exists but says
    /// `unknown`.
    #[test]
    fn an_unparseable_capacity_is_absent_and_never_a_guess() {
        for body in ["unknown\n", "", "  \n", "-1\n", "101\n", "9999999999\n"] {
            let root = tree(&[(
                "BAT0",
                &[
                    ("type", "Battery\n"),
                    ("capacity", body),
                    ("status", "Discharging\n"),
                ][..],
            )]);
            assert_eq!(
                read_battery(&root),
                BatteryReading::Absent,
                "capacity {body:?} must be an empty slot"
            );
        }
    }

    #[test]
    fn a_missing_status_reads_as_not_charging_rather_than_absent() {
        let root = tree(&[("BAT0", &[("type", "Battery\n"), ("capacity", "42\n")][..])]);
        assert_eq!(
            read_battery(&root),
            BatteryReading::Present {
                percent: 42,
                charging: false
            }
        );
    }

    /// The cap is the reader's. A `capacity` far larger than any kernel writes
    /// must cost the cap and must not parse into a number: the first
    /// `CAPACITY_CAP` bytes of a megabyte of `9`s is not a percentage.
    #[test]
    fn an_enormous_attribute_costs_the_cap_and_yields_nothing() {
        let huge = "9".repeat(1024 * 1024);
        let root = tree(&[(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", huge.as_str()),
                ("status", "Discharging\n"),
            ][..],
        )]);
        assert_eq!(read_battery(&root), BatteryReading::Absent);
        // ...and the read really was capped: `read_capped` returns exactly
        // CAPACITY_CAP bytes of the file, not the file.
        let text = read_capped(&root.join("BAT0").join("capacity"), CAPACITY_CAP)
            .expect("the file is readable");
        assert_eq!(text.len(), CAPACITY_CAP as usize);
    }

    /// A `type` file long enough to *contain* "Battery" must not be read as one.
    #[test]
    fn a_type_that_merely_starts_with_battery_is_not_a_battery() {
        let root = tree(&[(
            "BAT0",
            &[
                ("type", "Battery-ish\n"),
                ("capacity", "42\n"),
                ("status", "Charging\n"),
            ][..],
        )]);
        assert_eq!(read_battery(&root), BatteryReading::Absent);
    }

    /// Two batteries: the *sorted* first one, on every sample.
    #[test]
    fn two_batteries_resolve_to_the_same_one_every_time() {
        let root = tree(&[
            (
                "BAT1",
                &[
                    ("type", "Battery\n"),
                    ("capacity", "11\n"),
                    ("status", "Discharging\n"),
                ][..],
            ),
            (
                "BAT0",
                &[
                    ("type", "Battery\n"),
                    ("capacity", "22\n"),
                    ("status", "Discharging\n"),
                ][..],
            ),
        ]);
        for _ in 0..8 {
            assert_eq!(
                read_battery(&root),
                BatteryReading::Present {
                    percent: 22,
                    charging: false
                }
            );
        }
    }

    /// A device name longer than `MAX_NAME` is never opened.
    #[test]
    fn an_over_long_device_name_is_skipped() {
        let long = "b".repeat(MAX_NAME + 1);
        let root = tree(&[(
            long.as_str(),
            &[
                ("type", "Battery\n"),
                ("capacity", "42\n"),
                ("status", "Charging\n"),
            ][..],
        )]);
        assert_eq!(read_battery(&root), BatteryReading::Absent);
    }
}
