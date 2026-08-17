// SPDX-License-Identifier: MPL-2.0
//! The human's **brightness keys** (D-041, issue #303): the one filesystem
//! *write* the trusted core makes on its own behalf, `/sys/class/backlight/*/
//! brightness`, so `XF86MonBrightnessUp`/`Down` do what their labels say.
//!
//! # What was decided, and what was NOT
//!
//! [D-033] examined writing this file and **refused** it — as the *blanking*
//! mechanism, in place of `DrmSurface::clear()`. That refusal stands untouched:
//! nothing here blanks, nothing here is reached by a timer, and the idle path
//! does not know this module exists. What D-041 decided is *key actuation*: a
//! different trigger (a human's finger, not a timeout), a different purpose (a
//! brightness the human asked for, not a dark screen), and a different failure
//! mode (nothing happens, rather than a modeset that leaves a dark panel with
//! the session running).
//!
//! Two of D-033's four objections still bite and were **accepted rather than
//! answered**, so they are stated here beside the code rather than left in the
//! log: this is a *second* display-power interface inside the TCB, which the
//! blank state machine knows nothing about; and it does nothing at all for an
//! external display. Neither is mitigated here.
//!
//! # Bounded exactly the way [`crate::status::battery`] is
//!
//! That is the owner's accepted implementation constraint, and the pattern is a
//! *file* rather than a description, because the battery read was got wrong
//! once and the fixed version is what must be copied:
//!
//! - **One fixed root**, [`SYSFS_ROOT`], never composed from anything a client
//!   or an agent can influence. The root is a field only so the unit tests can
//!   point it at a fixture tree, and it is a `&'static Path` — not a `PathBuf`
//!   — so the only values it can ever hold are compile-time constants and
//!   there is no field a later edit could make settable from the wire. The
//!   *leaf* may be named by the operator (`--backlight-device`), and that is
//!   why [`DeviceName`] exists: a name with a separator, a dot, or anything
//!   outside `[A-Za-z0-9_.:-]` in it cannot be constructed at all, so the join
//!   below can only ever reach an immediate child of the fixed root.
//! - **A hard cap on how many directory entries are examined**
//!   ([`MAX_ENTRIES`]), and the names are **collected, sorted, then
//!   truncated — in that order**. Capping first was a *shipped defect* in the
//!   battery module (`battery.rs:104-125`): on a machine whose device is not
//!   among the first sixteen `readdir` entries the answer became "no device"
//!   on hardware that has one, non-deterministically, because `readdir` order
//!   is not stable across kernels. This is the same walk over the same kind of
//!   class directory, so it would reproduce that defect exactly if it were
//!   written from memory instead of from that file.
//! - **A hard cap on every read**, enforced by [`std::io::Read::take`] rather
//!   than by a length check afterwards: a `max_brightness` that is a gigabyte
//!   of digits costs [`VALUE_CAP`] bytes and no more. The cap is a property of
//!   the reader, so "read the whole file" is not expressible here. The *write*
//!   needs no cap because its length is a property of its type: a `u32`
//!   rendered in decimal is at most ten bytes.
//! - **No panic and no error path.** No device, an unreadable root, an
//!   unparseable value, a non-UTF-8 attribute, a `max_brightness` of zero, a
//!   `brightness` file this uid cannot open for writing — every one of them is
//!   **the key doing nothing**, which is precisely the behaviour this repo
//!   published before D-041. A bounded, failing backlight write therefore
//!   degrades to the status quo rather than to a new failure mode, and that is
//!   the strongest property this change has; it is an acceptance criterion
//!   ([`Outcome`] has no error variant a caller could be tempted to surface)
//!   rather than a hope.
//!
//! # The floor, and why zero is not reachable from here
//!
//! [D-030] published a hazard it did not close: `/sys/class/backlight` can
//! leave a human staring at nothing while `vitrind` believes it is presenting.
//! A core that can write that file can now *manufacture* that state — a
//! brightness of zero is a dark panel the blank state machine does not know is
//! dark, produced by the core's own hand. So this module has **no path that
//! reaches zero**: every value it writes is in `[floor, max]`, where the floor
//! is [`FLOOR_PERCENT`] of `max_brightness` and never less than 1 raw unit, and
//! a step that would go below the floor from *at or below* it is refused
//! outright ([`Outcome::AtLimit`]) rather than clamped upward. Refusing rather
//! than clamping is deliberate: clamping would let a `Down` press *raise* a
//! panel somebody else had dimmed, which is a surprise in the one direction a
//! human reads as a malfunction.
//!
//! # The key is CONSUMED, and no agent can reach any of this
//!
//! [`BacklightHook`] consumes both halves of both keysyms when the session
//! opted in, which reverses part of what WS-E.4.3 shipped: a nested compositor
//! or a remote-desktop client inside a realm loses those keys with no
//! pass-through and no way to ask for one. That cost was taken because the
//! alternative — actuate *and* deliver — hands a confined application a free
//! timing oracle over the human's brightness presses (D-023's stated reason for
//! consuming Super) and puts two actors on one interface with no arbitration.
//!
//! **There is no verb, no wire surface and no request.** The gate answers
//! [`Gate::Deliver`] for anything that is not [`Origin::Physical`], so an
//! emulated key carrying `XF86MonBrightnessUp` is delivered untouched and
//! actuates nothing. An agent that wants a brighter screen has no message to
//! send and no refusal to receive, which is D-033(2)'s *"there is no verb in
//! the IDL for 'power the human's display'"* applied unchanged.
//!
//! # When the core confines itself, this becomes a rule someone must write
//!
//! E2.6/E2.7 put Landlock over `vitrind`'s own process. `battery.rs:32-39`
//! already records that its *read* becomes a rule the core must grant itself.
//! This is **strictly larger**: the first rule in that future ruleset with a
//! **write** bit, over [`SYSFS_ROOT`] and the `brightness` file of one
//! immediate child. It is recorded here, in `docs/book/src/limits.md` and in
//! `docs/plan/14-workstream-session-mode.md` rather than left to be
//! rediscovered by whoever writes the ruleset
//! ([#187](https://github.com/vitrin-os/vitrin-os/issues/187)).
//!
//! It also depends on a permission this repository does not own: on a stock
//! install the write is reachable through a `video`-group membership or a
//! logind/udev tag, so **"the brightness key works" is a property of the
//! machine's udev rules rather than of a checkout**. That is published rather
//! than owned, exactly as D-033(5) accepted for logind's lid policy.
//!
//! [D-030]: https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md
//! [D-033]: https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::input::{Gate, PreemptionHook, SeatInput, SeatInputKind};

/// The fixed root. Never composed with anything from a peer, and a `&'static
/// str` so the only thing [`Backlight::root`] can hold in production is this.
pub(crate) const SYSFS_ROOT: &str = "/sys/class/backlight";

/// At most this many directory entries are examined. A backlight class with
/// more than sixteen devices is not a laptop; the cap is what makes the walk's
/// cost independent of what is in the tree. Applied **after** the sort — see
/// [`Backlight::resolve`].
const MAX_ENTRIES: usize = 16;

/// The longest device name this module will even `open`. Longer than any name
/// the kernel mints (`intel_backlight`, `amdgpu_bl0`, `acpi_video0`), short
/// enough that a hostile tree cannot make the path buffer interesting.
const MAX_NAME: usize = 32;

/// The byte cap on every attribute read, enforced by the reader. `brightness`
/// and `max_brightness` are decimal integers the kernel writes with a trailing
/// newline, so the largest legitimate body is about eight bytes; this is an
/// order of magnitude more, and a file longer than it yields a string that does
/// not parse rather than a truncated number that does — because the cap lands
/// mid-digits and the parse of `"9999999999999999"` overflows `u32` and is
/// `None`.
const VALUE_CAP: u64 = 24;

/// One press moves the brightness by this percentage of `max_brightness`.
///
/// **Five percent, so twenty presses traverse the range.** The alternative
/// shapes were a fixed number of raw units (meaningless across panels whose
/// `max_brightness` is anywhere from 10 to 96000) and a perceptual/logarithmic
/// ladder (which needs a model of the panel this core does not have and cannot
/// check). A linear percentage is the only one of the three whose behaviour a
/// reader can predict from this constant alone.
pub(crate) const STEP_PERCENT: u32 = 5;

/// The lowest brightness this module will ever write, as a percentage of
/// `max_brightness`.
///
/// **Equal to [`STEP_PERCENT`] on purpose**: the ladder of reachable values is
/// then exactly 5%, 10%, … 100%, the floor is one step rather than a separate
/// number, and "the lowest reachable value is one step above off" is true by
/// construction rather than by arithmetic. It is a *floor above zero* because
/// D-041 makes that a constraint on the implementation rather than an option
/// within it: a black panel is indistinguishable from a blanked one, and the
/// core already owns the blank.
pub(crate) const FLOOR_PERCENT: u32 = 5;

/// `XF86MonBrightnessUp`, the keysym `crate::input::invariant_keysym` maps
/// `KEY_BRIGHTNESSUP` (evdev 225) to. Held here as well as there because this
/// module is the only thing that acts on it, and
/// `the_gate_matches_the_scancode_table` pins the two against each other so a
/// row that moved cannot silently disarm the key.
pub(crate) const KEYSYM_UP: u32 = 0x1008_ff02;
/// `XF86MonBrightnessDown` (evdev 224), on the same terms.
pub(crate) const KEYSYM_DOWN: u32 = 0x1008_ff03;

/// How many un-drained presses the gate will hold.
///
/// A human cannot press a brightness key eight times inside one compositor
/// turn, so reaching this bound means the embedder is not draining. Dropping is
/// the benign direction — a step that does not happen costs a repeat, where an
/// unbounded queue costs the session — and it is [`crate::screenshot`]'s bound
/// for the same reason.
const MAX_PENDING_STEPS: usize = 8;

/// Which way the human asked the brightness to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Up,
    Down,
}

impl Step {
    /// The keysym this step is spelled by, or `None` for every other key.
    fn from_keysym(keysym: u32) -> Option<Self> {
        match keysym {
            KEYSYM_UP => Some(Self::Up),
            KEYSYM_DOWN => Some(Self::Down),
            _ => None,
        }
    }

    /// The flight recorder's stable label. A fixed vocabulary, never a
    /// `Debug` rendering, on [`crate::recorder`]'s rule for every reason field.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// What one press did, from a **closed vocabulary**.
///
/// Deliberately not a `Result`: there is no error here for anybody to handle.
/// Five of the six variants mean "the key did nothing", which is exactly the
/// behaviour every checkout had before D-041, and the sixth carries the
/// percentage that was written so the journal can say what happened rather than
/// that something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A value was written. The percentage is of `max_brightness`, rounded
    /// down, and is never below [`FLOOR_PERCENT`].
    Stepped { percent: u8 },
    /// The step would have left the `[floor, max]` band, so nothing was
    /// written. Pressing `Down` at the floor, or `Up` at full, lands here.
    AtLimit,
    /// This session was never given a backlight to write to (`--backlight` was
    /// not passed). Unreachable while the gate is armed only when a device is
    /// configured, and journalled rather than asserted because a panic on the
    /// input path is the one thing this module must never do.
    NotConfigured,
    /// The root is not there, holds no device, or holds none whose name and
    /// `max_brightness` are legible. A desktop, a VM, a CI runner.
    NoDevice,
    /// The device is there but its `brightness` could not be read as a number
    /// in `0..=max`.
    Unreadable,
    /// The device is there and legible, and the `brightness` file could not be
    /// opened for writing — the ordinary shape of "this uid is not in `video`
    /// and no udev rule tagged the seat".
    NotWritable,
}

impl Outcome {
    /// The flight recorder's stable label, from the closed set above.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stepped { .. } => "stepped",
            Self::AtLimit => "at_limit",
            Self::NotConfigured => "not_configured",
            Self::NoDevice => "no_device",
            Self::Unreadable => "unreadable",
            Self::NotWritable => "not_writable",
        }
    }

    /// The percentage written, or `None` when nothing was.
    pub(crate) fn percent(self) -> Option<u8> {
        match self {
            Self::Stepped { percent } => Some(percent),
            _ => None,
        }
    }
}

/// The startup refusal for `--backlight` on a backend that does not drive the
/// human's panel.
///
/// A `const` in one place, on [`crate::backend::blank::BLANK_NEEDS_THE_OUTPUT`]'s
/// precedent, so the message the operator reads and the message the test
/// asserts are one string.
pub(crate) const BACKLIGHT_NEEDS_THE_PANEL: &str =
    "`--backlight` requires `--drm`: on a nested session the write would succeed and dim a      panel this core does not own, while the host compositor acts on the same key -- two      actors on one backlight with no arbitration. A headless session has no panel at all.      Drop the flag, or run the bare-metal backend";

/// What `--backlight` and `--backlight-device` were parsed into.
///
/// The *presence* of this config is the opt-in — `None` means the operator did
/// not pass `--backlight`, the gate is detached, and the keys keep reaching the
/// app. There is deliberately no `enabled: bool` inside it, because a config
/// that could say "off" would be a second way to spell the same thing and one
/// of the two would eventually be read and not the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BacklightConfig {
    /// `--backlight-device NAME`, or `None` for the auto-pick.
    pub device: Option<DeviceName>,
}

/// What [`Backlight::probe`] found, for the startup log and for nothing else.
///
/// Separate from [`Outcome`] because a probe has a success shape a press does
/// not: the operator wants the device's *name* and where it currently sits, so
/// that a key which turns out to drive the wrong panel is diagnosable from the
/// first line of the log rather than from pressing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// A legible, writable device. `max` is its ceiling and `current` where it
    /// sits right now, both in raw units.
    Ready {
        device: String,
        max: u32,
        current: u32,
    },
    /// Nothing this session can write, and which of the three reasons it was.
    Absent(Outcome),
}

/// An immediate child of [`SYSFS_ROOT`], and nothing else.
///
/// The newtype is the whole of the "one fixed root" claim on the half an
/// operator can name. `--backlight-device ../../etc` and
/// `--backlight-device a/b` are not values of this type, so the join in
/// [`Backlight::resolve`] cannot leave the root however the flag is spelled,
/// and the check happens once at argument parsing rather than at every press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceName(String);

impl DeviceName {
    /// Parse an operator-supplied device name, or say why not.
    ///
    /// The accepted alphabet is what the kernel's backlight class actually
    /// mints — `intel_backlight`, `amdgpu_bl0`, `acpi_video0`, `nvidia_wmi_ec_
    /// backlight`, and the `card0-eDP-1-backlight` shape DRM's own devices use.
    /// A leading dot is refused separately from the alphabet so that `.` and
    /// `..` are refused *by name* rather than as an accident of which
    /// characters were allowed.
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err("`--backlight-device` requires a device name (e.g. \
                        `--backlight-device intel_backlight`)"
                .into());
        }
        if value.len() > MAX_NAME {
            return Err(format!(
                "`--backlight-device {value}`: a backlight device name is at most {MAX_NAME} \
                 characters; nothing the kernel mints is longer"
            ));
        }
        if value.starts_with('.') {
            return Err(format!(
                "`--backlight-device {value}`: a device name may not begin with `.`. This flag \
                 names an immediate child of `{SYSFS_ROOT}`, never a path"
            ));
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
        {
            return Err(format!(
                "`--backlight-device {value}`: a device name is letters, digits, `_`, `-`, `.` \
                 and `:` only. This flag names an immediate child of `{SYSFS_ROOT}`, never a path"
            ));
        }
        Ok(Self(value.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// The session's one backlight: a fixed root, and optionally the operator's
/// choice of device under it.
///
/// Holds **no** view of the session and nothing a peer supplied. Its root is a
/// `&'static Path` rather than a `PathBuf` field, so there is nothing here a
/// later edit could make settable from the wire — the property
/// `crate::status::sample::Sampler` holds for the battery read, held here for
/// the same reason and pinned by `the_root_is_not_a_runtime_path`.
pub(crate) struct Backlight {
    root: &'static Path,
    /// `None` means "pick the sorted-first legible device". See
    /// [`Self::resolve`] for why the auto-pick is deterministic and why it can
    /// still be the wrong device.
    device: Option<DeviceName>,
}

impl Backlight {
    /// The production constructor: the constant root, and the operator's
    /// device if they named one.
    pub(crate) fn new(device: Option<DeviceName>) -> Self {
        Self::with_root(Path::new(SYSFS_ROOT), device)
    }

    /// The test constructor. Production has exactly one root and it is a
    /// constant; this exists so this module's tests can drive a fixture tree,
    /// which is the only way any of the bounds here can be exercised at all —
    /// **CI has no `/sys/class/backlight`, and a temp directory proves the
    /// bounds and the failure collapse and proves exactly nothing about a
    /// panel** (D-041's own statement of what cannot be known).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_root(root: &'static Path, device: Option<DeviceName>) -> Self {
        Self { root, device }
    }

    /// The bounded, deterministic candidate list: length-capped names,
    /// **collected, sorted, then truncated — in that order**.
    ///
    /// This ordering is the whole point and it is not a style choice.
    /// `battery.rs` shipped the other order once and reported *"no battery"* on
    /// hardware that has one, because `readdir` order is not stable across
    /// kernels, so which entries survived a cap-then-sort walk was a function
    /// of the kernel's directory layout rather than of any name. Sorting first
    /// makes the answer the same on every call. The allocation is still
    /// bounded, one step later: names are length-capped as they are collected,
    /// and a backlight class holds single-digit numbers of entries rather than
    /// millions.
    ///
    /// A function of its own rather than four lines inside [`Self::resolve`],
    /// so `the_entry_cap_is_applied_after_the_sort` can assert the exact list
    /// instead of inferring the order from which device happened to win.
    fn candidate_names(&self) -> Vec<std::ffi::OsString> {
        let Ok(entries) = std::fs::read_dir(self.root) else {
            return Vec::new();
        };
        let mut names: Vec<std::ffi::OsString> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .filter(|name| name.as_encoded_bytes().len() <= MAX_NAME)
            .collect();
        names.sort();
        names.truncate(MAX_ENTRIES);
        names
    }

    /// Which device this session would write to, and what its ceiling is.
    ///
    /// `None` when the root is unreadable, holds nothing, or holds nothing
    /// whose name is short enough to open and whose `max_brightness` reads as a
    /// number above zero.
    ///
    /// **Auto-pick is the sorted-first legible device, and that is
    /// deterministic rather than correct.** A laptop with both `acpi_video0`
    /// and `intel_backlight` gets `acpi_video0`, which on a lot of modern
    /// hardware is the one that does nothing. That is exactly why
    /// `--backlight-device` exists, why the startup probe logs the name it
    /// chose, and why this is written down here instead of being discovered
    /// from a key that appears not to work.
    fn resolve(&self) -> Option<(PathBuf, u32)> {
        for name in self.candidate_names() {
            if let Some(chosen) = self.device.as_ref() {
                // The operator named one. A different device is not a fallback:
                // silently driving a panel they did not ask for is worse than
                // the key doing nothing, which is what this collapses to.
                if name.as_encoded_bytes() != chosen.as_str().as_bytes() {
                    continue;
                }
            }
            let dir = self.root.join(&name);
            // `max_brightness` first, and a device without a legible one is
            // SKIPPED rather than fatal — the difference from `battery.rs`,
            // which returns `Absent` on an unparseable `capacity`. There, the
            // first battery is "the battery the human has" and looking at the
            // next one would answer about different hardware. Here, an entry
            // under this class with no legible ceiling is not a backlight at
            // all, so skipping it is this module's version of that file's
            // `type != "Battery"` skip rather than of its `capacity` refusal.
            let Some(max) = read_capped(&dir.join("max_brightness"), VALUE_CAP)
                .and_then(|text| text.parse::<u32>().ok())
                .filter(|max| *max > 0)
            else {
                continue;
            };
            return Some((dir, max));
        }
        None
    }

    /// Probe this machine once, for the startup log.
    ///
    /// **Not a refusal.** A missing or unwritable device is a warning and never
    /// a startup error, and the two halves of that are decided separately: the
    /// *flag* is validated at parse time (a name that is not a name is a
    /// startup error, on this repo's rule that configuration is checked where
    /// it is read), while the *device* is only ever probed. Sysfs at startup is
    /// not sysfs at press time — a udev rule can land, a dock can arrive, a
    /// panel can be gone — and refusing to start a display server because a
    /// convenience key will not work would be a far worse failure than the key
    /// not working. The operator is told, loudly, once.
    pub(crate) fn probe(&self) -> Probe {
        let Some((dir, max)) = self.resolve() else {
            return Probe::Absent(Outcome::NoDevice);
        };
        let device = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(current) = read_capped(&dir.join("brightness"), VALUE_CAP)
            .and_then(|text| text.parse::<u32>().ok())
            .filter(|value| *value <= max)
        else {
            return Probe::Absent(Outcome::Unreadable);
        };
        // Opened and dropped immediately: the descriptor is not retained
        // anywhere, because the only thing this answers is whether the *key*
        // will be able to do anything, and a session that held a writable
        // descriptor onto the human's panel for its whole life would be a
        // strictly larger authority than the one D-041 granted.
        if open_for_write(&dir.join("brightness"), false).is_none() {
            return Probe::Absent(Outcome::NotWritable);
        }
        Probe::Ready {
            device,
            max,
            current,
        }
    }

    /// **Actuate one press.** The whole of the effect, in one function.
    ///
    /// Every branch that is not a write returns a label the journal can carry,
    /// and no branch can panic: there is no `unwrap`, no indexing and no
    /// arithmetic here that is not saturating or checked.
    pub(crate) fn step(&self, step: Step) -> Outcome {
        let Some((dir, max)) = self.resolve() else {
            return Outcome::NoDevice;
        };
        let path = dir.join("brightness");
        let Some(current) = read_capped(&path, VALUE_CAP)
            .and_then(|text| text.parse::<u32>().ok())
            .filter(|value| *value <= max)
        else {
            return Outcome::Unreadable;
        };
        // Both derived from `max` and both **at least 1**, so neither a panel
        // whose `max_brightness` is 10 nor one whose ceiling is 96000 can
        // produce a step of nothing or a floor of zero. Widened to `u64` before
        // the multiply and divided last: `max / 100 * 5` throws away the
        // remainder first and turns a `max_brightness` of 255 into a 3.9% step,
        // and `max * 5` in `u32` would overflow at ceilings this type can hold
        // even though no panel has one.
        let percent_of_max =
            |percent: u32| -> u32 { ((u64::from(max) * u64::from(percent)) / 100).max(1) as u32 };
        let delta = percent_of_max(STEP_PERCENT);
        let floor = percent_of_max(FLOOR_PERCENT);
        let target = match step {
            Step::Up => current.saturating_add(delta).min(max),
            Step::Down => {
                // At or below the floor, `Down` does NOTHING. Clamping upward
                // here would make a "dim" press brighten a panel somebody else
                // dimmed, and clamping downward would reach a value D-041
                // forbids this core from writing at all.
                if current <= floor {
                    return Outcome::AtLimit;
                }
                current.saturating_sub(delta).max(floor)
            }
        };
        // Up at full, and any step whose arithmetic lands where it started.
        // Skipping the write is not an optimisation: a write that changes
        // nothing would still be a journal line claiming the panel moved.
        if target == current {
            return Outcome::AtLimit;
        }
        // The floor is a property of every value this module writes, not of the
        // `Down` arm alone, and this is where that is true. Reaching it from
        // the `Up` arm needs `current < floor`, which happens when something
        // outside this core dimmed the panel first.
        let target = target.max(floor).min(max);
        if !write_value(&path, target) {
            return Outcome::NotWritable;
        }
        Outcome::Stepped {
            // `max` is non-zero (`resolve` filtered it) and `target <= max`, so
            // this is in `0..=100` and the cast is exact. Rounded down, so the
            // journal never says 100% of a panel that is one unit short.
            percent: ((u64::from(target) * 100) / u64::from(max)) as u8,
        }
    }
}

/// Read at most `cap` bytes of `path` and trim it, or `None`.
///
/// Lifted from `battery.rs:158` deliberately unchanged, including the reason:
/// the cap is the reader's, not a post-hoc check, so [`Read::take`] makes a
/// longer file cost `cap` bytes; and non-UTF-8 is `None` rather than lossy,
/// because a sysfs attribute that is not ASCII is not one this module
/// understands and `from_utf8_lossy` would turn that into a string that
/// *almost* parses.
fn read_capped(path: &Path, cap: u64) -> Option<String> {
    let mut buf = Vec::with_capacity(cap as usize);
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8(buf).ok()?.trim().to_string())
}

/// Open one `brightness` file for writing, or `None`.
///
/// **`create` is off and `truncate` is on**, and both halves of that are
/// deliberate. `std::fs::write` would be `O_WRONLY|O_CREAT|O_TRUNC`, and
/// `O_CREAT` is the bit that turns a typo into a *new file*: this process is
/// pointing at a kernel-owned attribute that already exists, and if it does not
/// exist the honest answer is [`Outcome::NoDevice`] rather than a file called
/// `brightness` in a directory nobody meant.
///
/// `truncate` is a **parameter**, because the two callers want opposite things.
/// The writer truncates: the value shrinks in digit count as the panel dims, so
/// writing `5200` over `10000` without truncating leaves the old trailing digit
/// and the file reads `52000`. Real sysfs does not care — the kernel parses the
/// write buffer and never the file's length, which is why
/// `echo 500 > .../brightness` has always worked — but a writer whose
/// correctness depends on its target being a kernel file is a writer no test
/// can hold, and this one is held by `no_sequence_of_presses_reaches_zero`
/// against an ordinary file. [`Backlight::probe`] does **not** truncate, for
/// the reason that sentence implies: a probe that modified the thing it was
/// asking about would be a probe with a side effect, and it is only asking
/// whether the *key* will be able to do anything.
///
/// `O_NOFOLLOW` covers the final component only, and it is worth exactly what
/// that is worth and no more: the device directory beneath `/sys/class/
/// backlight` **is** a symlink the kernel minted and this deliberately follows
/// it, and anyone who can plant a symlink inside `/sys` already owns this
/// machine. It is one flag, it costs nothing, and it means the leaf this
/// process writes to is the leaf the kernel put there.
fn open_for_write(path: &Path, truncate: bool) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(truncate)
        .create(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()
}

/// **The one write.** Every actuation in this core goes through this function
/// and there is no second one; `the_backlight_write_has_exactly_one_call_site`
/// asserts that structurally rather than by review.
///
/// Returns whether the value landed. Bounded by construction: a `u32` in
/// decimal is at most ten bytes, so there is no length to check and none is
/// checked.
fn write_value(path: &Path, value: u32) -> bool {
    let Some(mut file) = open_for_write(path, true) else {
        return false;
    };
    file.write_all(value.to_string().as_bytes()).is_ok()
}

/// The gate's state: whether this session opted in, and the presses it has
/// gated but the embedder has not yet acted on.
///
/// **It holds no device and touches no filesystem**, which is the point of the
/// split: this is consulted for *every* key event of the session, and a gate
/// that stat'd a file per keystroke would put sysfs on the input path. What it
/// records is *that the human pressed the key*; `session::drain_backlight_steps`
/// — which owns the recorder and the configured device — decides what that is
/// worth.
pub(crate) struct BacklightSignal {
    armed: bool,
    pending: Vec<Step>,
}

impl BacklightSignal {
    /// The session opted in: the keys are consumed and actuate.
    pub(crate) fn armed() -> Self {
        Self {
            armed: true,
            pending: Vec::new(),
        }
    }

    /// The session did not opt in.
    ///
    /// **The keys keep reaching the focused realm's shim seat**, exactly as
    /// they have since WS-E.4.3. Consuming them here would charge D-041's cost
    /// — an app inside a realm losing two keys with no pass-through — to a
    /// session that gets nothing for it, which is strictly worse than either
    /// end of the decision.
    pub(crate) fn detached() -> Self {
        Self {
            armed: false,
            pending: Vec::new(),
        }
    }

    /// **The gate.** Decide whether one event reaches the confined app, and
    /// record a press when one is a brightness key.
    ///
    /// The shape is `crate::attention::AttentionSignal::gate_event`'s, property
    /// for property:
    ///
    /// 1. **The origin check is the whole of the unforgeability claim.** Only
    ///    [`Origin::Physical`] — the tag intake binds, which
    ///    `SeatInput::physical`'s privacy makes unminttable outside
    ///    `crate::input` — is consumed or acted on. An emulated key carrying
    ///    this keysym is an actuation the enforcement chokepoint already
    ///    admitted against a live grant, and consuming it would silently
    ///    overturn an authority decision the core just made. It is one `if`, so
    ///    it is pinned adversarially by
    ///    `an_emulated_brightness_key_actuates_nothing_and_is_delivered`.
    /// 2. **Anything that is not this key is [`Gate::Deliver`], untouched.**
    /// 3. **Both halves are consumed**, so the router's per-keysym pairing
    ///    finds nothing to reconcile and the app is left holding nothing —
    ///    which is the one sound exception `PreemptionHook`'s pairing contract
    ///    names, taken here for a third core-owned key.
    /// 4. **The gate records; the embedder acts.** Nothing here touches sysfs.
    pub(crate) fn gate_event(&mut self, input: &SeatInput) -> Gate {
        if !self.armed {
            return Gate::Deliver;
        }
        if input.origin() != Origin::Physical {
            return Gate::Deliver;
        }
        let SeatInputKind::Key { keysym, state, .. } = input.kind() else {
            return Gate::Deliver;
        };
        let Some(step) = Step::from_keysym(*keysym) else {
            return Gate::Deliver;
        };
        if matches!(state, KeyState::Pressed) {
            if self.pending.len() < MAX_PENDING_STEPS {
                self.pending.push(step);
            } else {
                tracing::warn!("brightness step dropped: the pending queue is not being drained");
            }
        }
        Gate::Consume
    }

    /// Drain the presses this turn produced, oldest first.
    pub(crate) fn take_pending(&mut self) -> Vec<Step> {
        std::mem::take(&mut self.pending)
    }
}

/// The brightness keys attached at the router's preemption hook point.
///
/// Stacked `LockGate<ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<
/// BacklightHook<AttentionHook<NoopHook>>>>>>>`, and its position is a
/// decision at both boundaries:
///
/// - **inside [`crate::lock::LockGate`]**, so a locked session's brightness
///   keys do nothing for whoever is standing at it. This is the weakest of the
///   arguments for a lock position — a stranger dimming a locked laptop learns
///   nothing and takes nothing — and it is still the right side to be on,
///   because the lock's rule is that somebody who is not the human cannot
///   operate the machine, and "operate" should not acquire an exception list;
/// - **inside `ConsentGate` and `DeadManHook`**, so no sysfs write can delay
///   the human's off-switch or fire while they are answering a security
///   question;
/// - **outside [`crate::attention::AttentionHook`]** and inside
///   [`crate::screenshot::ScreenshotHook`], where it is unobservable: this hook
///   consumes only two keysyms, and neither is a modifier, a chord trigger or
///   part of any chord's vocabulary, so no matcher above or below it can have
///   its state changed by what this consumes. It sits here because *something*
///   had to be innermost-but-one, and beside the other two effect-having human
///   gestures is where a reader will look for it.
///
/// It implements `gate` **only**, which is `AttentionHook`'s posture and means
/// the same thing: everything above can suppress it, and it can blind nothing.
pub(crate) struct BacklightHook<H: PreemptionHook> {
    signal: Rc<RefCell<BacklightSignal>>,
    inner: H,
}

impl<H: PreemptionHook> BacklightHook<H> {
    pub(crate) fn new(signal: Rc<RefCell<BacklightSignal>>, inner: H) -> Self {
        Self { signal, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for BacklightHook<H> {
    /// Nothing. The brightness keys have no detection half — no hold, no
    /// modifier state, nothing to track between events — and a hook that
    /// observed would be a hook that could be tempted to stop observing.
    fn observe(&mut self, input: &SeatInput) {
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.signal.borrow_mut().gate_event(input) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }

    fn attention(&self) -> Option<Rc<RefCell<crate::attention::AttentionSignal>>> {
        self.inner.attention()
    }

    fn clipboard(&self) -> Option<Rc<RefCell<crate::clipboard::ClipboardSignal>>> {
        self.inner.clipboard()
    }

    fn screenshot(&self) -> Option<Rc<RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.inner.screenshot()
    }

    fn backlight(&self) -> Option<Rc<RefCell<BacklightSignal>>> {
        Some(Rc::clone(&self.signal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture tree under a fresh temporary directory, in `battery.rs`'s
    /// shape and for its stated reason: every file here is a handful of ASCII
    /// bytes, so the "checked-in fixture" is the string constant beside the
    /// test rather than a file nobody can read the *point* of.
    ///
    /// **Nothing in this module's tests ever touches `/sys`.** The production
    /// root is a constant and every test constructs its `Backlight` through
    /// [`Backlight::with_root`], which is why that constructor exists.
    fn tree(name: &str, entries: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vitrin-backlight-{}-{}-{:?}",
            std::process::id(),
            name,
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        for (device, files) in entries {
            let dir = root.join(device);
            std::fs::create_dir_all(&dir).expect("fixture device");
            for (file, body) in *files {
                std::fs::write(dir.join(file), body).expect("fixture attribute");
            }
        }
        root
    }

    /// `Backlight::with_root` takes a `&'static Path` — the production
    /// property — so a test tree has to be leaked to be pointed at. One
    /// directory's worth of `PathBuf` per test, which is the price of the
    /// field being unable to hold anything else.
    fn at(root: PathBuf, device: Option<&str>) -> Backlight {
        Backlight::with_root(
            Box::leak(root.into_boxed_path()),
            device.map(|name| DeviceName::parse(name).expect("a fixture name is a name")),
        )
    }

    /// Real `/sys/class/backlight/intel_backlight` on a Framework 13: a
    /// ceiling of 96000 and a value a little under half of it.
    const REAL_INTEL: &[(&str, &str)] = &[
        ("max_brightness", "96000\n"),
        ("brightness", "48000\n"),
        ("actual_brightness", "48000\n"),
        ("type", "raw\n"),
    ];

    fn value(root: &Path, device: &str) -> u32 {
        read_capped(&root.join(device).join("brightness"), VALUE_CAP)
            .expect("the fixture is readable")
            .parse()
            .expect("the fixture holds a number")
    }

    #[test]
    fn a_press_moves_the_panel_by_one_step() {
        let root = tree("step", &[("intel_backlight", REAL_INTEL)]);
        let light = at(root.clone(), None);
        // The probe answers and changes nothing: it opens the file for writing
        // to learn whether the key will work, and an opening that truncated
        // would be a startup that dimmed the panel to nothing.
        assert_eq!(
            light.probe(),
            Probe::Ready {
                device: "intel_backlight".to_string(),
                max: 96000,
                current: 48000,
            }
        );
        assert_eq!(value(&root, "intel_backlight"), 48000);
        assert_eq!(light.step(Step::Up), Outcome::Stepped { percent: 55 });
        assert_eq!(value(&root, "intel_backlight"), 52800);
        assert_eq!(light.step(Step::Down), Outcome::Stepped { percent: 50 });
        assert_eq!(value(&root, "intel_backlight"), 48000);
    }

    /// A root that is not there at all — a desktop, a VM, and every CI runner
    /// this project has.
    #[test]
    fn an_unreadable_root_is_a_silent_no_op() {
        let root = std::env::temp_dir().join("vitrin-backlight-does-not-exist");
        let _ = std::fs::remove_dir_all(&root);
        let light = at(root, None);
        assert_eq!(light.step(Step::Up), Outcome::NoDevice);
        assert_eq!(light.step(Step::Down), Outcome::NoDevice);
        assert_eq!(light.probe(), Probe::Absent(Outcome::NoDevice));
    }

    /// The class exists and holds nothing legible.
    #[test]
    fn a_device_without_a_legible_ceiling_is_skipped() {
        for body in ["0\n", "unknown\n", "", "  \n", "-1\n", "99999999999999\n"] {
            let root = tree(
                "ceiling",
                &[(
                    "acpi_video0",
                    &[("max_brightness", body), ("brightness", "3\n")][..],
                )],
            );
            assert_eq!(
                at(root, None).step(Step::Up),
                Outcome::NoDevice,
                "max_brightness {body:?} must be no device at all"
            );
        }
    }

    /// A device name longer than `MAX_NAME` is never opened. The fixture's
    /// `brightness` must be untouched afterwards, which is what makes this a
    /// test about *opening* rather than about the answer.
    #[test]
    fn an_over_long_device_name_is_never_opened() {
        let long = "b".repeat(MAX_NAME + 1);
        let root = tree("longname", &[(long.as_str(), REAL_INTEL)]);
        assert_eq!(at(root.clone(), None).step(Step::Up), Outcome::NoDevice);
        assert_eq!(value(&root, &long), 48000);
    }

    /// The cap is the reader's. A `max_brightness` far larger than any kernel
    /// writes costs the cap and yields nothing parseable.
    #[test]
    fn an_enormous_attribute_costs_the_cap_and_yields_nothing() {
        let huge = "9".repeat(1024 * 1024);
        let root = tree(
            "huge",
            &[(
                "intel_backlight",
                &[("max_brightness", huge.as_str()), ("brightness", "5\n")][..],
            )],
        );
        assert_eq!(at(root.clone(), None).step(Step::Up), Outcome::NoDevice);
        let text = read_capped(
            &root.join("intel_backlight").join("max_brightness"),
            VALUE_CAP,
        )
        .expect("the file is readable");
        assert_eq!(text.len(), VALUE_CAP as usize);
    }

    /// Non-UTF-8 is `None`, never a lossy parse: `b"48\xff000"` must not become
    /// a number, and must not become one after a replacement character either.
    #[test]
    fn a_non_utf8_value_is_a_no_op_rather_than_a_lossy_parse() {
        let root = tree("utf8", &[("intel_backlight", REAL_INTEL)]);
        std::fs::write(
            root.join("intel_backlight").join("brightness"),
            [b'4', b'8', 0xff, b'0', b'0', b'0'],
        )
        .expect("fixture attribute");
        assert_eq!(at(root.clone(), None).step(Step::Up), Outcome::Unreadable);
        assert_eq!(at(root, None).probe(), Probe::Absent(Outcome::Unreadable));
    }

    /// **The entry cap is applied after the sort**, asserted on the exact
    /// candidate list rather than inferred from which device won.
    ///
    /// Thirty-two devices, of which the surviving sixteen must be the sixteen
    /// *smallest names* — not the sixteen `readdir` yielded first. A
    /// cap-before-sort walk answers the sorted first sixteen only if the
    /// kernel's directory order happens to begin with the globally smallest
    /// sixteen entries, which is one arrangement out of six hundred million,
    /// and the guard below says so out loud if this filesystem is the one that
    /// hands them back in order.
    #[test]
    fn the_entry_cap_is_applied_after_the_sort() {
        let names: Vec<String> = (0..32).map(|i| format!("bl{i:02}_backlight")).collect();
        let files: Vec<(&str, &str)> = vec![("max_brightness", "100\n"), ("brightness", "50\n")];
        let refs: Vec<(&str, &[(&str, &str)])> = names
            .iter()
            .map(|name| (name.as_str(), files.as_slice()))
            .collect();
        let root = tree("sorted", &refs);
        let light = at(root.clone(), None);

        let raw: Vec<std::ffi::OsString> = std::fs::read_dir(&root)
            .expect("the fixture root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        let mut expected = raw.clone();
        expected.sort();
        expected.truncate(MAX_ENTRIES);
        assert_eq!(
            light.candidate_names(),
            expected,
            "the surviving entries must be the smallest names, not the first ones readdir gave"
        );
        let mut readdir_first: Vec<std::ffi::OsString> =
            raw.iter().take(MAX_ENTRIES).cloned().collect();
        readdir_first.sort();
        if readdir_first == expected {
            eprintln!(
                "note: this filesystem returned the fixture in sorted order, so the assertion \
                 above cannot tell the two implementations apart on this run"
            );
        }

        // ...and the behavioural half: the sorted-first device is the one that
        // moves, on every call, and no other device is ever written to.
        for _ in 0..8 {
            assert_eq!(light.step(Step::Up), Outcome::Stepped { percent: 55 });
            assert_eq!(light.step(Step::Down), Outcome::Stepped { percent: 50 });
        }
        for name in names.iter().skip(1) {
            assert_eq!(
                value(&root, name),
                50,
                "{name} must never have been written"
            );
        }
    }

    /// **No input drives the panel to zero.** A thousand `Down` presses from
    /// full on five panel geometries, including the ones whose
    /// `max_brightness` is smaller than the percentage step, where the naive
    /// arithmetic gives a step of zero and a floor of zero.
    #[test]
    fn no_sequence_of_presses_reaches_zero() {
        for max in [96000u32, 100, 10, 7, 1] {
            let ceiling = format!("{max}\n");
            let start = format!("{max}\n");
            let root = tree(
                "floor",
                &[(
                    "intel_backlight",
                    &[
                        ("max_brightness", ceiling.as_str()),
                        ("brightness", start.as_str()),
                    ][..],
                )],
            );
            let light = at(root.clone(), None);
            for _ in 0..1_000 {
                light.step(Step::Down);
                let now = value(&root, "intel_backlight");
                assert!(
                    now > 0,
                    "max_brightness {max} reached {now}: the core must have no path to a black \
                     panel"
                );
            }
            // ...and it settles exactly on the floor rather than somewhere
            // above it, so the test is about the bound and not about the loop
            // running out. A further press must be `AtLimit` rather than a
            // write that changes nothing.
            assert_eq!(light.step(Step::Down), Outcome::AtLimit);
            // The same arithmetic `step` does, restated once here on purpose:
            // a change to either side is then a red test rather than a silent
            // drift in what "the floor" means.
            let floor = ((u64::from(max) * u64::from(FLOOR_PERCENT)) / 100).max(1) as u32;
            assert_eq!(
                value(&root, "intel_backlight"),
                floor,
                "max_brightness {max} must settle on its floor"
            );
        }
    }

    /// A panel somebody else dimmed below the floor is left alone by `Down`
    /// rather than raised to the floor.
    #[test]
    fn a_down_press_below_the_floor_does_nothing_at_all() {
        let root = tree(
            "belowfloor",
            &[(
                "intel_backlight",
                &[("max_brightness", "100\n"), ("brightness", "2\n")][..],
            )],
        );
        assert_eq!(at(root.clone(), None).step(Step::Down), Outcome::AtLimit);
        assert_eq!(value(&root, "intel_backlight"), 2);
    }

    /// ...and `Up` from there lands on the floor rather than on 2 + step,
    /// because the floor is a property of every value written and not of one
    /// arm.
    #[test]
    fn an_up_press_below_the_floor_lands_on_the_floor() {
        let root = tree(
            "upfromfloor",
            &[(
                "intel_backlight",
                &[("max_brightness", "100\n"), ("brightness", "2\n")][..],
            )],
        );
        assert_eq!(
            at(root.clone(), None).step(Step::Up),
            Outcome::Stepped { percent: 7 }
        );
        assert_eq!(value(&root, "intel_backlight"), 7);
    }

    #[test]
    fn a_press_at_full_writes_nothing() {
        let root = tree(
            "full",
            &[(
                "intel_backlight",
                &[("max_brightness", "100\n"), ("brightness", "100\n")][..],
            )],
        );
        assert_eq!(at(root.clone(), None).step(Step::Up), Outcome::AtLimit);
        assert_eq!(value(&root, "intel_backlight"), 100);
    }

    /// **A `brightness` that is not a readable file is a no-op with a name**,
    /// and the published behaviour: the session starts, the key does nothing,
    /// and the journal says which of the six outcomes it was.
    ///
    /// The fixture makes `brightness` a *directory* — the shape a uid-agnostic
    /// failure has, because a mode-444 file is writable by root and this suite
    /// has to pass in a container that runs as one. It lands on `unreadable`
    /// rather than `not_writable`, and that is the point: a directory is not a
    /// number either, and **neither is a panic**. The permission case proper is
    /// covered underneath, where it can be skipped honestly.
    #[test]
    fn a_brightness_that_is_not_a_file_is_a_named_no_op() {
        let root = tree(
            "unwritable",
            &[("intel_backlight", &[("max_brightness", "100\n")][..])],
        );
        std::fs::create_dir_all(root.join("intel_backlight").join("brightness"))
            .expect("fixture directory");
        // Unreadable-as-a-number comes first: a directory is not a number
        // either, and the point of the test is that neither is a panic.
        assert_eq!(at(root.clone(), None).step(Step::Up), Outcome::Unreadable);
        assert_eq!(at(root, None).probe(), Probe::Absent(Outcome::Unreadable));
    }

    /// The permission case proper, skipped rather than faked where the test
    /// process can write anything.
    #[test]
    fn a_read_only_brightness_file_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tree(
            "readonly",
            &[(
                "intel_backlight",
                &[("max_brightness", "100\n"), ("brightness", "50\n")][..],
            )],
        );
        let path = root.join("intel_backlight").join("brightness");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))
            .expect("fixture permissions");
        // A uid that can write a mode-444 file is root, or holds
        // CAP_DAC_OVERRIDE. The property under test is then not observable at
        // all, and the honest answer is a **declared** skip rather than an
        // assertion about something else -- through `vitrin_skip`, so the skip
        // prints a marker line CI can see instead of being an invisible
        // `return` (#288). `HOST_TOOLING` is the right class: this is a
        // property of the machine the suite is running on, exactly like a
        // missing `/usr/bin/env`, and CI's `rust` job requires the class, so a
        // future container that ran the suite as root goes RED here rather
        // than green-having-measured-nothing.
        vitrin_skip::skip_unless!(
            vitrin_skip::HOST_TOOLING,
            vitrin_skip::Verdict::capable_if(
                open_for_write(&path, false).is_none(),
                "this uid can open a mode-444 file for writing, so it is root or holds \
                 CAP_DAC_OVERRIDE and `not_writable` is unreachable on this machine. Run the \
                 suite as an ordinary user.",
            )
        );
        assert_eq!(
            at(root.clone(), None).step(Step::Down),
            Outcome::NotWritable
        );
        assert_eq!(
            at(root.clone(), None).probe(),
            Probe::Absent(Outcome::NotWritable)
        );
        assert_eq!(value(&root, "intel_backlight"), 50);
    }

    /// A named device that is not there is the key doing nothing — never a
    /// silent fallback onto whichever panel happens to be present.
    #[test]
    fn a_named_device_that_is_absent_never_falls_back() {
        let root = tree(
            "named",
            &[("acpi_video0", REAL_INTEL), ("intel_backlight", REAL_INTEL)],
        );
        let light = at(root.clone(), Some("nvidia_wmi_ec_backlight"));
        assert_eq!(light.step(Step::Up), Outcome::NoDevice);
        assert_eq!(value(&root, "acpi_video0"), 48000);
        assert_eq!(value(&root, "intel_backlight"), 48000);
        // ...and naming one of the two really does select it over the
        // sorted-first one, or the test above would prove nothing.
        let light = at(root.clone(), Some("intel_backlight"));
        assert_eq!(light.step(Step::Up), Outcome::Stepped { percent: 55 });
        assert_eq!(value(&root, "acpi_video0"), 48000);
        assert_eq!(value(&root, "intel_backlight"), 52800);
    }

    #[test]
    fn a_device_name_is_a_name_and_never_a_path() {
        for bad in [
            "",
            "..",
            ".",
            "../../etc",
            "a/b",
            "intel backlight",
            "intel\nbacklight",
            "intel\0backlight",
            &"b".repeat(MAX_NAME + 1),
        ] {
            assert!(
                DeviceName::parse(bad).is_err(),
                "{bad:?} must not be a device name"
            );
        }
        for good in [
            "intel_backlight",
            "acpi_video0",
            "amdgpu_bl0",
            "card0-eDP-1-backlight",
            "nvidia_wmi_ec_backlight",
        ] {
            assert!(
                DeviceName::parse(good).is_ok(),
                "{good:?} is a name the kernel mints"
            );
        }
    }

    // ---- The gate ----------------------------------------------------------

    /// A physical key, minted through the **real intake path** rather than
    /// hand-assembled: `SeatInput::physical` is private to `crate::input`
    /// precisely so a physical origin cannot be forged, and a test that dodged
    /// that would be testing a shape production cannot produce.
    fn key(evdev: u32, state: KeyState) -> SeatInput {
        let mut inputs = crate::input::physical_key(evdev, None, state);
        assert_eq!(
            inputs.len(),
            1,
            "evdev {evdev} must survive the layout-invariant table"
        );
        inputs.remove(0)
    }

    fn emulated(keysym: u32, state: KeyState) -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            source: crate::input::KeySource::Keysym,
            keysym,
            state,
        })
    }

    /// `KEY_BRIGHTNESSUP` and `KEY_BRIGHTNESSDOWN`, the two scancodes the
    /// kernel emits and `invariant_keysym` maps.
    const EVDEV_UP: u32 = 225;
    const EVDEV_DOWN: u32 = 224;

    #[test]
    fn an_armed_gate_consumes_both_halves_of_both_keys() {
        let mut signal = BacklightSignal::armed();
        for evdev in [EVDEV_UP, EVDEV_DOWN] {
            assert_eq!(
                signal.gate_event(&key(evdev, KeyState::Pressed)),
                Gate::Consume
            );
            assert_eq!(
                signal.gate_event(&key(evdev, KeyState::Released)),
                Gate::Consume
            );
        }
        assert_eq!(signal.take_pending(), vec![Step::Up, Step::Down]);
        assert!(signal.take_pending().is_empty());
    }

    /// The same claim through the hook the backends really stack, so a refactor
    /// that moved the decision out of `gate_event` is still caught.
    #[test]
    fn the_hook_consumes_what_the_signal_consumes() {
        let sig = Rc::new(RefCell::new(BacklightSignal::armed()));
        let mut hook = BacklightHook::new(Rc::clone(&sig), crate::input::NoopHook);
        assert_eq!(hook.gate(&key(EVDEV_UP, KeyState::Pressed)), Gate::Consume);
        assert_eq!(
            hook.gate(&emulated(KEYSYM_UP, KeyState::Pressed)),
            Gate::Deliver
        );
        assert_eq!(sig.borrow_mut().take_pending(), vec![Step::Up]);
        assert!(
            hook.backlight().is_some(),
            "the hook must expose its signal"
        );
    }

    /// Without `--backlight` the keys reach the app exactly as they have since
    /// WS-E.4.3, and nothing is queued for a drain that would find no device.
    #[test]
    fn a_detached_gate_delivers_the_keys_untouched() {
        let mut signal = BacklightSignal::detached();
        for evdev in [EVDEV_UP, EVDEV_DOWN] {
            assert_eq!(
                signal.gate_event(&key(evdev, KeyState::Pressed)),
                Gate::Deliver
            );
            assert_eq!(
                signal.gate_event(&key(evdev, KeyState::Released)),
                Gate::Deliver
            );
        }
        assert!(signal.take_pending().is_empty());
    }

    /// **The origin check is the whole of the unforgeability claim.** An
    /// agent's actuation carrying this keysym is delivered untouched and queues
    /// nothing, so no message an agent can send changes the human's brightness.
    #[test]
    fn an_emulated_brightness_key_actuates_nothing_and_is_delivered() {
        let mut signal = BacklightSignal::armed();
        for keysym in [KEYSYM_UP, KEYSYM_DOWN] {
            assert_eq!(
                signal.gate_event(&emulated(keysym, KeyState::Pressed)),
                Gate::Deliver
            );
            assert_eq!(
                signal.gate_event(&emulated(keysym, KeyState::Released)),
                Gate::Deliver
            );
        }
        assert!(signal.take_pending().is_empty());
    }

    /// Escape, Super and the volume keys — every other key in the stream is
    /// delivered untouched, including the other XF86 rows WS-E.4.3 added, which
    /// this decision explicitly does **not** cover.
    #[test]
    fn every_other_key_is_untouched() {
        let mut signal = BacklightSignal::armed();
        for evdev in [1, 125, 113, 114, 115, 57] {
            assert_eq!(
                signal.gate_event(&key(evdev, KeyState::Pressed)),
                Gate::Deliver
            );
        }
        assert!(signal.take_pending().is_empty());
    }

    /// The queue is bounded and the overflow is dropped rather than grown.
    #[test]
    fn the_pending_queue_is_bounded() {
        let mut signal = BacklightSignal::armed();
        for _ in 0..(MAX_PENDING_STEPS * 4) {
            signal.gate_event(&key(EVDEV_UP, KeyState::Pressed));
        }
        assert_eq!(signal.take_pending().len(), MAX_PENDING_STEPS);
    }

    /// The gate and the scancode table must agree, or the key is dead in a
    /// build where every test is green. Asked of the real table rather than
    /// restated.
    #[test]
    fn the_gate_matches_the_scancode_table() {
        assert_eq!(crate::input::invariant_keysym(225), Some(KEYSYM_UP));
        assert_eq!(crate::input::invariant_keysym(224), Some(KEYSYM_DOWN));
        assert_eq!(Step::from_keysym(KEYSYM_UP), Some(Step::Up));
        assert_eq!(Step::from_keysym(KEYSYM_DOWN), Some(Step::Down));
    }

    // ---- Structural pins ---------------------------------------------------

    /// Every production `.rs` under this crate, truncated at its test module.
    ///
    /// `crate::backend`'s `the_blank_cover_has_exactly_one_chokepoint` is the
    /// precedent, including the truncation: otherwise the tests that exercise
    /// the write would count as production call sites.
    fn rust_sources(dir: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let production = text
                    .split_once("\n#[cfg(test)]\nmod tests {")
                    .map(|(before, _)| before.to_string())
                    .unwrap_or(text);
                out.push((
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    production,
                ));
            }
        }
    }

    fn crate_sources() -> Vec<(String, String)> {
        let mut sources = Vec::new();
        rust_sources(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources,
        );
        assert!(
            sources.iter().any(|(name, _)| name == "backlight.rs"),
            "the scan must cover this file, or it proves nothing"
        );
        assert!(
            sources.iter().any(|(name, _)| name == "session.rs"),
            "the scan must cover the embedder, or a second call site there would be invisible"
        );
        sources
    }

    /// **The write has exactly one call site.**
    ///
    /// Scanned for the FUNCTION, never for a receiver's name — the lesson
    /// `backend/mod.rs`'s cover test paid for, where counting a literal
    /// `blank.composite_over(` was walked straight through by binding the same
    /// surface to a differently-named local. Two named failure modes: a second
    /// place in the core that opens a backlight attribute for writing, and this
    /// module growing a second caller of its own writer so the floor is applied
    /// in one of them and not the other.
    #[test]
    fn the_backlight_write_has_exactly_one_call_site() {
        let mut writers: Vec<(String, usize)> = Vec::new();
        let mut openers: Vec<(String, usize)> = Vec::new();
        for (name, text) in crate_sources() {
            let w = text.match_indices("write_value(").count();
            let o = text.match_indices("open_for_write(").count();
            if w > 0 {
                writers.push((name.clone(), w));
            }
            if o > 0 {
                openers.push((name, o));
            }
        }
        // One definition and one call, both in this file: `fn write_value(` and
        // the call inside `Backlight::step`.
        assert_eq!(
            writers,
            vec![("backlight.rs".to_string(), 2)],
            "the brightness write must have exactly one caller, and it must be in this module"
        );
        // The definition, the call from `write_value`, and the call from
        // `Backlight::probe`. A fourth would be a second place that can hold a
        // writable descriptor onto the human's panel.
        assert_eq!(
            openers,
            vec![("backlight.rs".to_string(), 3)],
            "nothing outside this module may open a backlight attribute for writing"
        );
    }

    /// **The root is not reachable from any client-settable field.**
    ///
    /// The production root is a `&'static Path`, so the only values it can hold
    /// are compile-time constants; a `PathBuf` field would accept any runtime
    /// string, and the difference is what makes "no wire message can move this"
    /// a property of the type rather than of a review. Pinned in both
    /// directions: the field must be declared that way, and `SYSFS_ROOT` must
    /// be named nowhere in the crate but here.
    #[test]
    fn the_root_is_not_a_runtime_path() {
        let sources = crate_sources();
        let this = sources
            .iter()
            .find(|(name, _)| name == "backlight.rs")
            .map(|(_, text)| text.as_str())
            .expect("this file is in the scan");
        assert!(
            this.contains("root: &'static Path,"),
            "the backlight root must be a `&'static Path` field, never a `PathBuf`"
        );
        assert!(
            !this.contains("root: PathBuf") && !this.contains("root: String"),
            "a runtime-typed root field is a root a later edit can make settable"
        );
        let named: Vec<&String> = sources
            .iter()
            .filter(|(_, text)| text.contains("\"/sys/class/backlight\""))
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            named,
            vec!["backlight.rs"],
            "the literal root must be written once, in the module that owns the bound"
        );
    }
}
