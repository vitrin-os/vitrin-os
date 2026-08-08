// SPDX-License-Identifier: MPL-2.0
//! The **impure half** of the status strip: the one place in [`crate::status`]
//! that reads a clock or a file.
//!
//! # Why the split exists at all
//!
//! [`crate::consent::render`] refuses to draw a countdown on the consent card
//! because "a clock on the card would make its pixels a function of
//! `Instant::now()` — untestable by construction". A status strip is that trap
//! by definition, and it is escaped the same way the consent card escapes it:
//! everything that reads the world lands in a [`StatusSnapshot`] value here,
//! and [`super::render::rasterize`] is a pure function of that value. The
//! golden (`tests/golden/status_strip.txt`) pins the raster; this module is
//! what the golden does not have to cover.
//!
//! # Which clock, and the timezone the core deliberately does not have
//!
//! The strip shows **UTC by default**, and an operator who wants local time
//! says so once with `--status-utc-offset`. That is a real limitation and it is
//! chosen rather than stumbled into: local time needs the IANA timezone
//! database, which means a binary file-format parser and a per-process global
//! (`tzset`) inside the TCB, plus a recurring read of `/etc/localtime` and
//! `/usr/share/zoneinfo` — a third filesystem authority for the core to grant
//! itself once E2.6/E2.7 confine it, bought for a cosmetic field. A fixed
//! offset needs none of that: it is an integer the operator states, applied
//! with one addition.
//!
//! What it costs, stated rather than implied: **no DST**. A session that ran
//! across a DST boundary shows an hour that is off by one until the operator
//! changes the flag. The strip therefore always renders the zone it is showing
//! (`UTC`, or `+09:00`), so a human reading it is never left guessing which
//! clock they are looking at — a wrong hour labelled `+09:00` is a wrong flag,
//! whereas a wrong hour labelled nothing is a wrong clock.
//!
//! # Cadence
//!
//! [`Sampler::sample`] is called once per dispatch round and is cheap by
//! construction: [`SystemTime::now`] is a vDSO read, and the battery — the only
//! syscall on this path — is behind [`BATTERY_INTERVAL`]. See that constant for
//! the measurement.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use crate::grants::RealmId;

use super::battery::{read_battery, BatteryReading};

/// How often the battery tree is actually read.
///
/// **Measured, not guessed.** [`read_battery`] against the real
/// `/sys/class/power_supply` on the target laptop (two devices: `ADP1` mains,
/// `BAT1` battery) costs one `getdents64` plus, per candidate, up to three
/// `openat`/`read`/`close` triples. Timed over 10 000 iterations on
/// 2026-08-08: **8.90 µs/call** in `--release`, 10.97 µs/call in the debug
/// profile (the measurement is recorded in
/// `docs/plan/14-workstream-session-mode.md` §6). At a 30 s interval that is
/// 0.30 µs of work per second of session — 0.00003% of one core — so the
/// interval is chosen for what it *reports* rather than for what it costs.
///
/// What it reports: the strip shows whole percent, and a laptop battery moves
/// one percent no faster than roughly once a minute under load. Thirty seconds
/// is inside that by a factor of two, so no percent step is ever missed by more
/// than half its own lifetime.
pub(crate) const BATTERY_INTERVAL: Duration = Duration::from_secs(30);

/// The clock's fixed offset from UTC, in minutes.
///
/// Bounded to the range real civil offsets occupy (`UTC-12:00` to `UTC+14:00`,
/// the widest the IANA database uses), so a parse cannot produce an hour field
/// that is not an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UtcOffset(i16);

/// The refusal `--status-utc-offset` prints. A `const` so the operator's
/// message and the test's expectation are one string.
pub(crate) const OFFSET_REFUSAL: &str =
    "`--status-utc-offset` takes `UTC` or a signed offset like `+09:00` or `-03:30`, between \
     -12:00 and +14:00. The core holds no timezone database -- the strip shows the offset it \
     is given and labels it, so a wrong hour is a wrong flag rather than a wrong clock.";

impl UtcOffset {
    pub const UTC: Self = Self(0);

    const MIN_MINUTES: i16 = -12 * 60;
    const MAX_MINUTES: i16 = 14 * 60;

    /// Parse `UTC`, `Z`, or `[+-]HH:MM`.
    ///
    /// Deliberately strict: no bare `+9`, no `+0900`, no seconds. One accepted
    /// spelling per value means the flag a session ran with and the flag a
    /// runbook records cannot be two different strings that look the same.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text == "UTC" || text == "Z" {
            return Ok(Self::UTC);
        }
        let bytes = text.as_bytes();
        if bytes.len() != 6 || bytes[3] != b':' {
            return Err(OFFSET_REFUSAL);
        }
        let sign: i16 = match bytes[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return Err(OFFSET_REFUSAL),
        };
        let hours: i16 = text[1..3].parse().map_err(|_| OFFSET_REFUSAL)?;
        let minutes: i16 = text[4..6].parse().map_err(|_| OFFSET_REFUSAL)?;
        if minutes > 59 {
            return Err(OFFSET_REFUSAL);
        }
        let total = sign * (hours * 60 + minutes);
        if !(Self::MIN_MINUTES..=Self::MAX_MINUTES).contains(&total) {
            return Err(OFFSET_REFUSAL);
        }
        Ok(Self(total))
    }

    pub fn minutes(self) -> i16 {
        self.0
    }
}

/// A wall-clock reading, already reduced to what the strip draws.
///
/// Hours and minutes only — **no seconds**, and that is a design choice with a
/// number behind it. A seconds field changes the strip's raster 60 times a
/// minute; on the zero-copy path that is 60 texture uploads a minute and 60
/// forced repaints of an otherwise idle display, and on the trusted-band
/// witness it is 60 strip-changed ticks a minute of pure noise. `HH:MM` changes
/// once a minute, which is what a status bar shows anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockReading {
    pub hour: u8,
    pub minute: u8,
    pub offset: UtcOffset,
}

impl ClockReading {
    /// Reduce a [`SystemTime`] to wall-clock hours and minutes at `offset`.
    ///
    /// Pure integer arithmetic on the seconds since the epoch, and no date: the
    /// strip shows a time of day, so the civil-calendar conversion (leap years,
    /// month lengths) is work this module never has to do and therefore never
    /// has to get right. A pre-epoch clock — which a machine with a dead RTC
    /// really can present — is handled by `rem_euclid` rather than by a branch,
    /// so it yields a time of day rather than a panic or a negative hour.
    pub fn from_system_time(now: SystemTime, offset: UtcOffset) -> Option<Self> {
        let epoch_secs = match now.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(delta) => i64::try_from(delta.as_secs()).ok()?,
            Err(err) => -i64::try_from(err.duration().as_secs()).ok()?,
        };
        let local = epoch_secs.checked_add(i64::from(offset.minutes()) * 60)?;
        let day = local.rem_euclid(86_400);
        Some(Self {
            hour: (day / 3600) as u8,
            minute: ((day % 3600) / 60) as u8,
            offset,
        })
    }
}

/// Everything the strip is allowed to say, as a value.
///
/// **No free-form string field**, exactly as [`crate::consent::PromptContent`]
/// and [`crate::lock::LockContent`] have none, and for the same consequence: an
/// app cannot put a glyph on this surface, because there is no field through
/// which one could travel. The realm is a [`RealmId`] out of the operator's
/// `realm.toml`; the clock and the battery are integers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusSnapshot {
    /// `None` only when the system clock could not be reduced to a time of day
    /// at all (a `SystemTime` beyond `i64` seconds). The strip then draws an
    /// empty slot, never `00:00`.
    pub clock: Option<ClockReading>,
    pub battery: BatteryReading,
    /// The realm the output is bound to, or `None` when nothing is bound.
    pub realm: Option<RealmId>,
}

/// The sampler: the clock offset, the battery root, and the battery cache.
///
/// Holds **no** view of the session beyond what it is handed per call, and its
/// battery root is a `&'static Path` rather than a `PathBuf` field, so there is
/// nothing here a later edit could make settable from the wire.
pub(crate) struct Sampler {
    offset: UtcOffset,
    root: &'static Path,
    battery: BatteryReading,
    battery_read_at: Option<Instant>,
}

impl Sampler {
    pub fn new(offset: UtcOffset) -> Self {
        Self::with_root(offset, Path::new(super::battery::SYSFS_ROOT))
    }

    /// The test constructor. Production has exactly one root and it is a
    /// constant; this exists so the strip's own tests can drive a fixture tree.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_root(offset: UtcOffset, root: &'static Path) -> Self {
        Self {
            offset,
            root,
            battery: BatteryReading::Absent,
            battery_read_at: None,
        }
    }

    /// One reading.
    ///
    /// `now` is the wall clock and `mono` the monotonic clock; both are
    /// parameters rather than read here so a test can advance either
    /// independently, which is what makes "the band's rows are constant while
    /// the strip's are not" a testable claim over a thousand composites.
    pub fn sample(
        &mut self,
        now: SystemTime,
        mono: Instant,
        realm: Option<&RealmId>,
    ) -> StatusSnapshot {
        let due = match self.battery_read_at {
            None => true,
            Some(last) => mono.saturating_duration_since(last) >= BATTERY_INTERVAL,
        };
        if due {
            self.battery = read_battery(self.root);
            self.battery_read_at = Some(mono);
        }
        StatusSnapshot {
            clock: ClockReading::from_system_time(now, self.offset),
            battery: self.battery,
            realm: realm.cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(epoch_secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_secs)
    }

    #[test]
    fn utc_is_the_default_and_both_spellings_parse() {
        assert_eq!(UtcOffset::parse("UTC"), Ok(UtcOffset::UTC));
        assert_eq!(UtcOffset::parse("Z"), Ok(UtcOffset::UTC));
        assert_eq!(UtcOffset::UTC.minutes(), 0);
    }

    #[test]
    fn signed_offsets_parse_to_minutes() {
        assert_eq!(UtcOffset::parse("+09:00").map(UtcOffset::minutes), Ok(540));
        assert_eq!(UtcOffset::parse("-03:30").map(UtcOffset::minutes), Ok(-210));
        assert_eq!(UtcOffset::parse("+14:00").map(UtcOffset::minutes), Ok(840));
        assert_eq!(UtcOffset::parse("-12:00").map(UtcOffset::minutes), Ok(-720));
    }

    #[test]
    fn out_of_range_and_loose_spellings_are_refused() {
        for bad in [
            "+15:00",
            "-13:00",
            "+9",
            "+0900",
            "9:00",
            "+09:60",
            "+09:0",
            "",
            "+aa:bb",
            "+09:00:00",
        ] {
            assert_eq!(
                UtcOffset::parse(bad),
                Err(OFFSET_REFUSAL),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn the_clock_reduces_to_a_time_of_day_at_the_offset() {
        // 1786244643 = 20674 whole days + 11043 s, i.e. 03:04:03 UTC.
        let t = at(1_786_244_643);
        let utc = ClockReading::from_system_time(t, UtcOffset::UTC).expect("a time of day");
        assert_eq!((utc.hour, utc.minute), (3, 4));
        let jst = ClockReading::from_system_time(t, UtcOffset::parse("+09:00").unwrap())
            .expect("a time of day");
        assert_eq!((jst.hour, jst.minute), (12, 4));
        // Crossing midnight backwards must wrap into the previous day rather
        // than producing a negative hour.
        let honolulu = ClockReading::from_system_time(t, UtcOffset::parse("-10:00").unwrap())
            .expect("a time of day");
        assert_eq!((honolulu.hour, honolulu.minute), (17, 4));
    }

    #[test]
    fn a_pre_epoch_clock_still_yields_a_time_of_day() {
        let t = SystemTime::UNIX_EPOCH - Duration::from_secs(3661);
        let r = ClockReading::from_system_time(t, UtcOffset::UTC).expect("a time of day");
        assert_eq!((r.hour, r.minute), (22, 58));
    }

    #[test]
    fn the_battery_is_read_once_per_interval_and_not_once_per_sample() {
        // The fixture root is missing, so every read answers `Absent` -- what
        // is under test is how many reads happen, which the cache timestamp
        // reports.
        let root: &'static Path = Path::new("/nonexistent/vitrin/power_supply");
        let mut sampler = Sampler::with_root(UtcOffset::UTC, root);
        let start = Instant::now();
        sampler.sample(at(0), start, None);
        let first = sampler.battery_read_at.expect("the first sample reads");
        // A sample one second later must NOT re-read.
        sampler.sample(at(1), start + Duration::from_secs(1), None);
        assert_eq!(
            sampler.battery_read_at,
            Some(first),
            "a sample inside the interval must not touch the filesystem"
        );
        // One past the interval must.
        sampler.sample(at(31), start + BATTERY_INTERVAL, None);
        assert_ne!(
            sampler.battery_read_at,
            Some(first),
            "a sample at the interval must re-read"
        );
    }

    #[test]
    fn the_realm_field_follows_the_binding() {
        let mut sampler = Sampler::with_root(UtcOffset::UTC, Path::new("/nonexistent"));
        let realm = RealmId::new("realm-0");
        let now = Instant::now();
        assert_eq!(sampler.sample(at(0), now, None).realm, None);
        assert_eq!(
            sampler.sample(at(0), now, Some(&realm)).realm,
            Some(realm.clone())
        );
    }
}
