// SPDX-License-Identifier: MPL-2.0
//! **The human's screenshot key** (WS-E.2.4, issue #216): a core-owned chord
//! that writes one PNG of the realm view into one operator-configured
//! directory, and touches no grant at any point.
//!
//! The title of the issue is the specification. A human photographing their
//! own screen is not an agent capability and must not become one, so nothing
//! here mints, checks, spends or even *names* authority. Two mechanisms hold
//! that up rather than a rule: [`crate::capture::render_screenshot`] can only
//! be called with a [`HumanGesture`], which nothing outside this module can
//! construct; and `enforcement.rs`'s source scan counts the identifiers in
//! both directions, so neither this module naming the chokepoint nor the
//! chokepoint naming the encoder survives CI.
//!
//! # What a vitrin screenshot contains, and what it deliberately does not
//!
//! It contains **the focused realm's view** — the exact bytes an `observe`
//! grant would serve for that realm, from
//! [`Runtime::view_cache`](crate::session::Runtime), the cache the capture
//! path itself reads.
//!
//! It therefore contains **no trusted band, no consent card, no trusted ring,
//! no lock cover, no status strip and no agent cursor.** Those live on the
//! other side of the output-stage fork ([`crate::backend::human_visible_from_view`]),
//! and reading the human-visible side instead was considered and refused. The
//! reason is one sentence long: the trusted band **is** the session secret
//! ([`crate::consent::TrustedIndicator`]), the confined realm runs as the
//! core's own uid, and a file the core writes is therefore a file the app
//! reads — so the first screenshot would hand a forger the colour every
//! genuine consent prompt is framed in, and the indicator would be worth
//! nothing for the rest of the session. `indicator.rs` states the rule this
//! obeys: the secret *"is never written to any descriptor or file"*.
//!
//! Two softer alternatives were examined and both are worse:
//!
//! - **Crop the band's rows out.** It does not close the hole. The band is not
//!   the only place the secret is painted — a genuine consent card is framed
//!   in the same colour, in the middle of the output — so a crop protects the
//!   secret only while no prompt is up, which is precisely the moment a human
//!   most wants a screenshot.
//! - **Redact by colour: replace every pixel equal to the secret.** This is
//!   actively dangerous, and it is worth writing down because it looks
//!   thorough. It hands the app an **oracle**: an app paints its own surface
//!   with a field of candidate colours, the human takes a screenshot, and the
//!   app reads back which of *its own* pixels were recoloured. Each screenshot
//!   tests as many candidates as there are pixels, so a ~22-bit secret
//!   (`TrustedIndicator::generate` scales each channel into `[64, 255]`) falls
//!   to a handful of screenshots on a 1080p output. A structural exclusion has
//!   no oracle because there is nothing to compare against.
//!
//! The cost is real, it is published (`docs/book/src/limits.md`), and it is
//! not softened here: **a vitrin screenshot cannot show a suspicious prompt.**
//! "Send me a picture of that weird dialog" is the single most useful thing a
//! screenshot does, and this one cannot do it. The correct answer today is a
//! phone camera. That is worse than every other desktop offers, and it buys
//! the one property those desktops do not have.
//!
//! # No grant, and no principal either
//!
//! There is no human principal in the identity model, and inventing one to
//! satisfy a screenshot would be growing the identity layer for the wrong
//! reason (issue #216's own decision). So the chord's effect is not a *use* at
//! all: no facet, no verb, no token bucket, no refusal event. What bounds it
//! instead is that it is only reachable from a physical key on the machine's
//! own keyboard, judged by the same hook stack the dead-man switch lives in.
//! A locked session and a raised consent prompt each suppress it — both
//! consume *all* physical input while they are up — because each is a moment
//! when a convenience must not happen.
//!
//! **A dead-man hold does not suppress it**, and an earlier version of this
//! paragraph claimed it did. `DeadManHook::gate` consumes only its own chord's
//! key and delivers every other, so nothing about a hold reaches this hook.
//! That is the intended behaviour rather than a gap —
//! [`crate::session::Kernel::screenshot_dir`] says so in as many words: the
//! off-switch destroys authority, and a human's own screenshot key is not
//! authority. The claim was wrong; the code was not.
//!
//! # The path is the attack surface, so the path is not a string
//!
//! Everything about where the file goes is fixed at startup and nothing about
//! it is expressible afterwards:
//!
//! - `--screenshot-dir` is validated once, at parse time, and the directory is
//!   **opened and held** as an `O_DIRECTORY|O_NOFOLLOW` descriptor for the
//!   process's life. Every later write is an `openat` relative to that
//!   descriptor, so no rename, no symlink and no mount planted afterwards can
//!   redirect one — the path was resolved exactly once, by us, before any
//!   client existed.
//! - The name is **minted by [`ScreenshotDir::write`] itself** from the
//!   session's start time and a monotonic counter. There is no parameter a
//!   name could arrive in: nothing outside this file can influence a path
//!   component, because there is no argument through which to try.
//! - Each file is created `O_CREAT|O_EXCL|O_NOFOLLOW`, mode `0600`. `O_EXCL`
//!   refuses to follow a symlink planted at the name, and refuses to overwrite
//!   an existing file rather than clobbering it.
//! - Absent flag = the feature is off. A bad flag = the core refuses to start,
//!   naming the reason. There is no "fall back to a default directory"
//!   behaviour, because a screenshot written somewhere the operator did not
//!   ask for is the failure this whole section exists to prevent.
//!
//! `0600` is worth being honest about: it keeps the file from *other* users,
//! and it does nothing at all about the confined app, which runs as the same
//! uid. Screenshots on disk are readable by every realm under the current
//! no-sandbox posture (`docs/book/src/limits.md`, D9). This issue does not
//! create that property; it does create the files.
//!
//! # A failure is loud
//!
//! A screenshot that does not happen is journalled
//! ([`crate::recorder::Event::ScreenshotFailed`]) with a fixed reason label and
//! logged at `warn`. The human has no other instrument — the core has no
//! surface on which to tell them — and a silently dropped screenshot on a full
//! disk is exactly the failure someone discovers a week later.

use std::cell::RefCell;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use rustix::fs::{Mode, OFlags};

use crate::chord::{ChordMatcher, ChordSpecError, ModChord};
use crate::input::{Gate, PreemptionHook, SeatInput};
use crate::recorder::ObservationDigest;

/// The default screenshot chord.
///
/// **Not a bare `Print`, and that is a decision rather than an accident.**
/// Issue #216 proposed the bare key; [`crate::chord::ModChord`] refuses a
/// chord with no modifier by construction, because a bare-key gesture belongs
/// to [`crate::deadman::Chord`] or [`crate::attention::AttentionChord`] and a
/// third spelling of "a bare key the core owns" is how two mechanisms end up
/// disagreeing about which keys are the core's. D-024 makes
/// [`crate::chord`] the one matcher every later chord consumes, so the choice
/// was between a modifier and a parallel path, and the parallel path is the
/// thing that decision exists to forbid.
///
/// Requiring a modifier also **repays one of #216's published costs**: with a
/// bare Print the confined app would never see PrintScreen at all (the matcher
/// consumes what it matches). With `ctrl+print`, bare Print is delivered
/// untouched and an app that binds it keeps it.
///
/// `ctrl` rather than `shift`, `alt` or `super`: it is the one modifier no
/// other core-owned chord pairs with a *editing* key, `super` is the attention
/// chord's own key and pairing with it would make every screenshot also tap
/// the attention key, and `alt+print`/`shift+print` are the two combinations
/// desktops most often claim for their own screenshot variants.
pub(crate) const DEFAULT_SCREENSHOT_CHORD: &str = "ctrl+print";

/// The most screenshot gestures one dispatch turn will queue before dropping
/// them.
///
/// A human cannot chord twice in one compositor turn; the bound exists so a
/// stuck embedder cannot grow this queue without limit, and it is smaller than
/// [`crate::clipboard`]'s four because a screenshot writes a file — a backlog
/// of them is a backlog of disk writes rather than of bookkeeping.
const MAX_PENDING_GESTURES: usize = 2;

/// The screenshot key's policy, as the command line asked for it.
///
/// The **path**, not an open descriptor: [`crate::main`]'s `Action` is
/// `PartialEq` and printed by the parse tests, and an `OwnedFd` is neither. The
/// directory is opened and audited exactly once, in `run_session`, before the
/// listener accepts anyone — so there is still no stat-then-open window, and a
/// bad flag is still a refusal to start. Same shape and same reason as
/// [`crate::lock::LockConfig`]'s passphrase path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenshotConfig {
    /// `--screenshot-dir`, or `None` when the key does nothing.
    pub dir: Option<PathBuf>,
    /// `--screenshot-chord`, defaulted to [`DEFAULT_SCREENSHOT_CHORD`].
    pub chord: ModChord,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            dir: None,
            chord: ModChord::parse(DEFAULT_SCREENSHOT_CHORD)
                .expect("the default screenshot chord is in the vocabulary"),
        }
    }
}

/// Why a `--screenshot-dir` was refused at startup.
///
/// Every variant is a refusal to start, never a warning: a session that came
/// up with a screenshot key writing somewhere unintended is the fail-open
/// configuration trap [`crate::realm`]'s loader and [`crate::deadman`]'s chord
/// parser both refuse, and this one writes pictures of the human's screen.
#[derive(Debug)]
pub(crate) enum ScreenshotDirError {
    /// Not an absolute path. Refused rather than resolved against the process's
    /// working directory, which is not a thing an operator can see in a unit
    /// file.
    NotAbsolute,
    /// Could not be opened as a directory: missing, not a directory, or a
    /// symlink (`O_NOFOLLOW`), with the OS error that said so.
    Unopenable(io::Error),
    /// Opened, and someone other than root or the core can write it — or a
    /// directory on the way to it can be. `String` carries
    /// [`crate::realm::untrusted_writer`]'s own wording so the two refusals
    /// read identically.
    Untrusted { at: PathBuf, fault: String },
}

impl std::fmt::Display for ScreenshotDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenshotDirError::NotAbsolute => write!(
                f,
                "must be an absolute path: a relative one names a different \
                 directory depending on where the core was started from"
            ),
            ScreenshotDirError::Unopenable(err) => write!(
                f,
                "cannot be opened as a directory ({err}); it must exist, be a \
                 directory, and not be a symlink"
            ),
            ScreenshotDirError::Untrusted { at, fault } => write!(
                f,
                "{} is {fault}; whoever can write it, or rename it, chooses \
                 where pictures of the human's screen land",
                at.display()
            ),
        }
    }
}

impl std::error::Error for ScreenshotDirError {}

/// The validated screenshot target: a held directory descriptor, the path it
/// was opened from (for messages and the journal), and the name minter.
///
/// **There is no constructor that takes a name and no method that takes a
/// path.** [`Self::open`] is the only way to obtain one, and [`Self::write`]
/// takes bytes and returns the name it chose. That is the whole of "nothing a
/// client controls reaches a path component": not a check, an absent
/// parameter.
#[derive(Debug)]
pub(crate) struct ScreenshotDir {
    /// The audited directory, opened once at startup and held. Every write is
    /// an `openat` relative to this, so the path is resolved exactly once —
    /// before any realm exists — and no later filesystem change can redirect
    /// one.
    dir: OwnedFd,
    /// What the operator wrote, for the journal and for error messages. Never
    /// used to open anything after [`Self::open`] returns.
    path: PathBuf,
    /// Seconds since the epoch at session start: the first half of every name.
    ///
    /// A raw epoch count rather than a calendar stamp because **the core has
    /// no calendar**. [`crate::status`] converts a `SystemTime` as far as
    /// hours and minutes and no further; growing a civil-date implementation
    /// (months, leap years, the operator's zone) inside the TCB to make a
    /// filename prettier is not a trade this module will make. The number
    /// sorts correctly, which is what a filename has to do.
    session: u64,
    /// Screenshots written this session, so two files in the same second
    /// cannot collide. Monotone, never reset.
    counter: u32,
}

/// How many names one press may try before giving up loudly.
///
/// The names are predictable, so a same-uid app can plant entries at them; a
/// single attempt made that a permanent denial of the key. Bounded because this
/// runs on the compositor thread — an app that has genuinely filled the
/// namespace must not spin it — and 64 because a human pressing a key expects
/// an answer, not a directory scan.
const NAME_ATTEMPTS: u32 = 64;

impl ScreenshotDir {
    /// Audit `path` and hold it open, or refuse.
    ///
    /// The order is deliberate. The directory is **opened first**, with
    /// `O_DIRECTORY|O_NOFOLLOW`, so a symlink where the operator's directory
    /// should be is refused rather than followed; everything after that is a
    /// question about the descriptor we already hold, never about the name
    /// again — no stat-then-open window, exactly
    /// [`crate::realm`]'s `check_config_security` posture.
    ///
    /// The ancestor walk that follows is the one part that reasons about names
    /// rather than the descriptor, and it is honest about what it buys: it
    /// cannot protect *this* session, whose target is already pinned by the
    /// fd. It refuses a target whose parent directory someone untrusted could
    /// re-point **before the next** startup — the same reason
    /// `realm::audit_spawn_target` walks the ancestors of a program it is
    /// about to exec.
    pub fn open(path: &Path) -> Result<Self, ScreenshotDirError> {
        if !path.is_absolute() {
            return Err(ScreenshotDirError::NotAbsolute);
        }
        let dir = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|err| ScreenshotDirError::Unopenable(err.into()))?;
        let euid = rustix::process::geteuid().as_raw();
        let st = rustix::fs::fstat(&dir)
            .map_err(|err| ScreenshotDirError::Unopenable(io::Error::from(err)))?;
        // `sticky_tolerated: false`, unlike a directory on the way to a
        // program: the sticky bit stops another user *deleting* our files, and
        // says nothing about a world-writable directory being the place the
        // operator wants pictures of their screen to land.
        if let Some(fault) = crate::realm::untrusted_writer(st.st_uid, st.st_mode, euid, false) {
            return Err(ScreenshotDirError::Untrusted {
                at: path.to_path_buf(),
                fault,
            });
        }
        // Skip(1): `ancestors` yields the directory itself first, and the fstat
        // above already judged it — on the descriptor rather than the name.
        let resolved = std::fs::canonicalize(path).map_err(ScreenshotDirError::Unopenable)?;
        for parent in resolved.ancestors().skip(1) {
            let st = rustix::fs::stat(parent)
                .map_err(|err| ScreenshotDirError::Unopenable(io::Error::from(err)))?;
            if let Some(fault) = crate::realm::untrusted_writer(st.st_uid, st.st_mode, euid, true) {
                return Err(ScreenshotDirError::Untrusted {
                    at: parent.to_path_buf(),
                    fault,
                });
            }
        }
        let session = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(0);
        Ok(Self {
            dir,
            path: path.to_path_buf(),
            session,
            counter: 0,
        })
    }

    /// The directory the operator named, for a log line and the journal.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write `png` under a freshly minted name and return that name.
    ///
    /// The name is `vitrin-<session-epoch-seconds>-<counter>.png`, built from
    /// two integers this struct owns. It cannot contain a separator, a `.`
    /// component, or any byte an app chose, because there is no input.
    ///
    /// `O_CREAT|O_EXCL|O_NOFOLLOW` and mode `0600`: `O_EXCL` makes the create
    /// fail rather than follow a symlink someone planted at the name, and fail
    /// rather than truncate a file that is already there.
    ///
    /// # Why this retries, and why the retry is bounded
    ///
    /// The name is fully predictable — session epoch plus a counter — so a
    /// same-uid app can compute every future name and plant entries at them.
    /// With one attempt per press, `O_EXCL` turned that into a **permanent
    /// denial of the human's screenshot key**: every press collided, failed,
    /// and advanced the counter by exactly one, so an app that had planted the
    /// next thousand names won for a thousand presses. `O_EXCL` is right and
    /// the single attempt was not: refusing to clobber is a safety property,
    /// while giving up on the first collision is a liveness bug wearing its
    /// clothes.
    ///
    /// So a collision advances and retries, up to [`NAME_ATTEMPTS`]. Bounded
    /// rather than unbounded because this runs on the compositor thread and an
    /// app that really has filled the namespace must not spin it; when the
    /// bound is reached the press fails **loudly**, naming the directory, which
    /// is the honest answer to "somebody has filled your screenshot directory".
    /// Only `EEXIST` retries — any other error is the operator's problem
    /// (permissions, a full disk) and repeating it would only obscure it.
    pub fn write(&mut self, png: &[u8]) -> io::Result<String> {
        for _ in 0..NAME_ATTEMPTS {
            self.counter = self.counter.wrapping_add(1);
            let name = format!("vitrin-{}-{:04}.png", self.session, self.counter);
            match rustix::fs::openat(
                &self.dir,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(file) => {
                    let mut file = std::fs::File::from(file);
                    std::io::Write::write_all(&mut file, png)?;
                    return Ok(name);
                }
                // Planted entry, or a symlink `O_NOFOLLOW` refused: skip the
                // name rather than the screenshot.
                Err(err) if err == rustix::io::Errno::EXIST || err == rustix::io::Errno::LOOP => {
                    continue;
                }
                Err(err) => return Err(err.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "no free screenshot name after {NAME_ATTEMPTS} attempts in {}: something has \
                 filled the directory with entries at the names this session mints",
                self.path.display()
            ),
        ))
    }
}

/// The one gesture this module owns. A unit-shaped enum rather than `()` so
/// the matcher's payload names what happened, exactly as
/// [`crate::clipboard::ClipboardGesture`] does, and so a second screenshot
/// variant (a region, a window) has somewhere to go that is not a `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenshotGesture {
    /// The whole focused realm's view.
    Full,
}

/// **Proof that a human's physical chord fired**, and the only argument
/// through which [`crate::capture::render_screenshot`] can be reached.
///
/// The field is private to this module and there is no `pub` constructor, no
/// `Copy` and no `Clone`, so the sole way to obtain one is [`Self::of`] — a
/// private function whose only caller is [`capture_to_file`], which is handed
/// a [`ScreenshotGesture`] that only [`ScreenshotSignal::take_pending`]
/// produces, which only [`ScreenshotHook::gate`] fills, and only for a
/// **physical-origin** chord ([`crate::chord`]'s rule 1: an emulated key
/// carrying the chord's keysyms matches nothing).
///
/// That chain is the answer to "never touches a grant" stated as a type. A
/// principal holding every verb in `Verb::VALID_MASK` cannot construct this
/// value, so it cannot call the encoder, so it cannot make the core write a
/// picture of the screen — and the reason is a compile error rather than the
/// current absence of such a call. The shape is `spawn::SpawnRecord`'s
/// (#207) and `clipboard::PendingPromotion`'s (#213).
pub(crate) struct HumanGesture(());

impl HumanGesture {
    /// Mint the proof from a gesture the matcher produced. Private on purpose:
    /// widening this to `pub` is the one edit that would undo everything the
    /// type says.
    fn of(_gesture: ScreenshotGesture) -> Self {
        Self(())
    }
}

/// The chord queue the hook writes and the embedder drains.
///
/// The same shape, and the same reason, as
/// [`crate::clipboard::ClipboardSignal`]: the hook runs inside
/// `InputRouter::route_physical`, which is never told a realm, so the gate can
/// only record *that a gesture happened* — which realm's pixels are written,
/// and whether a directory is even configured, are the embedder's to answer.
#[derive(Debug)]
pub(crate) struct ScreenshotSignal {
    matcher: ChordMatcher<ScreenshotGesture>,
    pending: Vec<ScreenshotGesture>,
    /// The configured chord's spelling, for the startup log and the journal.
    spelling: String,
}

impl ScreenshotSignal {
    pub fn new(chord: ModChord) -> Result<Self, ChordSpecError> {
        Ok(Self {
            spelling: chord.spelling(),
            matcher: ChordMatcher::new(vec![(chord, ScreenshotGesture::Full)])?,
            pending: Vec::new(),
        })
    }

    /// A signal for a build whose hook stack carries no [`ScreenshotHook`] — a
    /// plain headless run, which has no physical input device to chord on.
    ///
    /// The kernel holds one unconditionally, exactly as it holds an attention
    /// signal and a clipboard signal unconditionally, so "this backend has no
    /// screenshot chord" is a fact about the hook stack rather than about the
    /// session's state.
    pub fn detached() -> Self {
        Self::new(
            ModChord::parse(DEFAULT_SCREENSHOT_CHORD)
                .expect("the default screenshot chord is in the vocabulary"),
        )
        .expect("a single binding is never a duplicate")
    }

    /// The configured chord's spelling.
    pub fn spelling(&self) -> &str {
        &self.spelling
    }

    /// Drain the gestures this turn produced, oldest first.
    pub fn take_pending(&mut self) -> Vec<ScreenshotGesture> {
        std::mem::take(&mut self.pending)
    }

    /// Forget the modifier bits and the consumed set across a seat pause —
    /// [`ChordMatcher::forget_physical_state`], whose docs carry the reason.
    pub fn forget_physical_state(&mut self) {
        self.matcher.forget_physical_state();
    }
}

/// The screenshot chord attached at the router's preemption hook point.
///
/// Stacked
/// `LockGate<ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<AttentionHook<NoopHook>>>>>>`,
/// and every boundary above and below it is a decision:
///
/// - **inside [`crate::lock::LockGate`]**, so a locked session cannot be
///   screenshotted by whoever is standing at it. This is the position that
///   matters most: the lock exists so somebody who is not the human cannot
///   operate the machine, and a screenshot key that worked through a lock
///   would write the contents of the locked-away session to a file for them;
/// - **inside `ConsentGate`**, so no screenshot fires while the human is
///   answering a security question — while `observe` still runs (the grab
///   forwards it unconditionally), so a modifier release the prompt swallowed
///   still clears its bit and the matcher cannot be left believing Ctrl is
///   held;
/// - **inside `DeadManHook`**, so the human's off-switch is never delayed by a
///   file write;
/// - **inside [`crate::clipboard::ClipboardHook`]** rather than outside it.
///   The two are peers — both are human conveniences on the same matcher — and
///   the order between them is unobservable while startup refuses a shared
///   trigger key. Inside is still the deliberate side: the clipboard moves an
///   application's bytes across a realm boundary and this only reads, so if
///   the collision check ever failed to fire, the more consequential gesture
///   is the one that would win;
/// - **outside [`crate::attention::AttentionHook`]**, which consumes both
///   Supers. Inside it, this matcher would never see a Super press and its
///   `super` modifier bit would be permanently wrong — which the default chord
///   does not use, and `--screenshot-chord super+print` does.
pub(crate) struct ScreenshotHook<H: PreemptionHook> {
    signal: Rc<RefCell<ScreenshotSignal>>,
    inner: H,
}

impl<H: PreemptionHook> ScreenshotHook<H> {
    pub fn new(signal: Rc<RefCell<ScreenshotSignal>>, inner: H) -> Self {
        Self { signal, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for ScreenshotHook<H> {
    /// Modifier tracking only — unconditional, so a release the consent grab
    /// swallowed still clears its bit ([`crate::chord`]'s rule 2). It decides
    /// nothing and can stop nothing.
    fn observe(&mut self, input: &SeatInput) {
        self.signal.borrow_mut().matcher.observe(input);
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        let verdict = {
            let mut signal = self.signal.borrow_mut();
            let (gate, gesture) = signal.matcher.gate(input);
            if let Some(gesture) = gesture {
                if signal.pending.len() < MAX_PENDING_GESTURES {
                    signal.pending.push(gesture);
                } else {
                    // A human cannot chord twice in one compositor turn, so
                    // this means the embedder is not draining. Dropping is the
                    // benign direction: a screenshot that does not happen costs
                    // a repeat, where an unbounded queue costs the session.
                    tracing::warn!(
                        "screenshot gesture dropped: the pending queue is not being drained"
                    );
                }
            }
            gate
        };
        match verdict {
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

    fn screenshot(&self) -> Option<Rc<RefCell<ScreenshotSignal>>> {
        Some(Rc::clone(&self.signal))
    }
}

/// Encode one realm view and write it, or say why not.
///
/// The **whole** of the effect half, in one function, so the embedder's drain
/// stays a question about which realm and this stays a question about bytes.
/// `Err` carries a fixed label from a closed vocabulary — never an OS string —
/// for the same reason [`crate::recorder::Event::ClipboardRefused`]'s does: a
/// journal field an errno message could reach is a journal field an attacker's
/// filename could reach.
pub(crate) fn capture_to_file(
    dir: &mut ScreenshotDir,
    gesture: ScreenshotGesture,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(String, ObservationDigest), &'static str> {
    let view = crate::capture::RealmViewFrame {
        rgba,
        width,
        height,
    };
    let (png, digest) = crate::capture::render_screenshot(&view, HumanGesture::of(gesture))
        .map_err(|err| {
            tracing::warn!(%err, "screenshot could not be encoded");
            "encode_failed"
        })?;
    match dir.write(&png) {
        Ok(name) => Ok((name, digest)),
        Err(err) => {
            tracing::warn!(
                dir = %dir.path().display(),
                %err,
                "screenshot could not be written"
            );
            Err("write_failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputRouter, NoopHook};
    use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

    /// The chord's own scancodes, so a test presses what the operator
    /// configured rather than a number it guessed.
    fn chord() -> ModChord {
        ModChord::parse(DEFAULT_SCREENSHOT_CHORD).expect("the default chord parses")
    }

    fn signal() -> Rc<RefCell<ScreenshotSignal>> {
        Rc::new(RefCell::new(
            ScreenshotSignal::new(chord()).expect("one binding"),
        ))
    }

    type Router = InputRouter<ScreenshotHook<NoopHook>>;

    /// Route one key through the **production intake**: `SeatInput::physical`
    /// is private to `crate::input`, so a test cannot mint a physical tag and
    /// drives the real scancode table instead. Returns how many events reached
    /// a realm.
    fn route(router: &mut Router, evdev: u32, state: KeyState) -> usize {
        let mut delivered = 0;
        for input in crate::input::physical_key(evdev, None, state) {
            if router
                .route_physical(input, (640, 480), Some((640, 480)))
                .is_some()
            {
                delivered += 1;
            }
        }
        delivered
    }

    /// Press and release the whole configured chord, modifiers released in
    /// reverse order as a human does. Returns the delivery count.
    fn press_chord(router: &mut Router, chord: &ModChord) -> usize {
        let (mods, trigger) = chord.scancodes();
        let mut delivered = 0;
        for evdev in &mods {
            delivered += route(router, *evdev, KeyState::Pressed);
        }
        delivered += route(router, trigger, KeyState::Pressed);
        delivered += route(router, trigger, KeyState::Released);
        for evdev in mods.iter().rev() {
            delivered += route(router, *evdev, KeyState::Released);
        }
        delivered
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vitrin-screenshot-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_default_chord_is_ctrl_print_and_it_parses() {
        let chord = chord();
        assert_eq!(chord.spelling(), "ctrl+print");
        assert_eq!(chord.trigger_keysym(), 0xff61);
        let (mods, trigger) = chord.scancodes();
        assert_eq!(mods, vec![29]);
        assert_eq!(trigger, 99);
    }

    /// The gesture is produced by a **physical** chord and consumed, and a
    /// bare trigger is delivered untouched — which is what repays issue #216's
    /// "the confined app never sees PrintScreen" cost.
    #[test]
    fn the_chord_queues_a_gesture_and_a_bare_print_is_delivered() {
        let sig = signal();
        let mut router: Router =
            InputRouter::detached(ScreenshotHook::new(Rc::clone(&sig), NoopHook));
        let bound = crate::grants::RealmId::new("realm-0");
        let _owed = router.bind_to(&bound);

        let delivered = press_chord(&mut router, &chord());
        assert_eq!(
            sig.borrow_mut().take_pending(),
            vec![ScreenshotGesture::Full],
            "the configured chord queues exactly one gesture"
        );
        assert_eq!(
            delivered, 2,
            "both halves of the TRIGGER are consumed -- the app never learns the \
             chord happened -- and both halves of the MODIFIER are still \
             delivered, because the app needs its Ctrl"
        );

        // A bare Print, no modifier: the matcher compares modifier sets for
        // equality, so this is not the chord and the app keeps its key.
        assert_eq!(
            route(&mut router, 99, KeyState::Pressed),
            1,
            "a bare Print reaches the realm"
        );
        assert!(
            sig.borrow_mut().take_pending().is_empty(),
            "and queues nothing"
        );
    }

    /// An **agent's** emulated key carrying the chord's keysyms is an
    /// actuation the chokepoint already admitted against a live grant. It must
    /// not be able to make the core write a file.
    #[test]
    fn an_emulated_chord_can_never_take_a_screenshot() {
        let sig = signal();
        let mut hook = ScreenshotHook::new(Rc::clone(&sig), NoopHook);
        // The chord spelled correctly -- Ctrl held ACROSS the trigger, not
        // pressed and released before it. Getting that wrong is how this test
        // passes for the wrong reason: with the modifier already up, a matcher
        // with both origin razors deleted would still match nothing.
        const CTRL_L: u32 = 0xffe3;
        const PRINT: u32 = 0xff61;
        for (keysym, state) in [
            (CTRL_L, KeyState::Pressed),
            (PRINT, KeyState::Pressed),
            (PRINT, KeyState::Released),
            (CTRL_L, KeyState::Released),
        ] {
            let input = SeatInput::emulated(crate::input::SeatInputKind::Key {
                source: crate::input::KeySource::Keysym,
                keysym,
                state,
            });
            hook.observe(&input);
            assert_eq!(
                hook.gate(&input),
                Gate::Deliver,
                "an emulated chord key is never consumed either: the app must still see it"
            );
        }
        assert!(
            sig.borrow_mut().take_pending().is_empty(),
            "an agent holding actuate_text cannot manufacture the human's chord"
        );

        // The other half, and it is the one the `gate` razor exists for: the
        // HUMAN is holding Ctrl (physically, through the production intake) and
        // the agent sends the trigger. Without the razor in `gate` this is a
        // screenshot the agent took with the human's modifier.
        for input in crate::input::physical_key(29, None, KeyState::Pressed) {
            hook.observe(&input);
            hook.gate(&input);
        }
        for state in [KeyState::Pressed, KeyState::Released] {
            let input = SeatInput::emulated(crate::input::SeatInputKind::Key {
                source: crate::input::KeySource::Keysym,
                keysym: PRINT,
                state,
            });
            hook.observe(&input);
            assert_eq!(hook.gate(&input), Gate::Deliver);
        }
        assert!(
            sig.borrow_mut().take_pending().is_empty(),
            "an agent cannot borrow the human's held modifier to complete the chord"
        );
    }

    #[test]
    fn a_relative_directory_is_refused() {
        let err = ScreenshotDir::open(Path::new("shots")).expect_err("relative is refused");
        assert!(matches!(err, ScreenshotDirError::NotAbsolute), "{err:?}");
    }

    #[test]
    fn a_missing_directory_and_a_plain_file_are_both_refused() {
        let dir = tmpdir("missing");
        let absent = dir.join("nope");
        assert!(matches!(
            ScreenshotDir::open(&absent),
            Err(ScreenshotDirError::Unopenable(_))
        ));
        let file = dir.join("file");
        std::fs::write(&file, b"x").expect("write");
        assert!(
            matches!(
                ScreenshotDir::open(&file),
                Err(ScreenshotDirError::Unopenable(_))
            ),
            "a regular file is not a directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlinked_directory_is_refused_never_followed() {
        let dir = tmpdir("symlink");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).expect("mkdir");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert!(
            matches!(
                ScreenshotDir::open(&link),
                Err(ScreenshotDirError::Unopenable(_))
            ),
            "O_NOFOLLOW refuses a symlink where the directory should be"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_world_writable_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("worldwritable");
        let target = dir.join("open");
        std::fs::create_dir_all(&target).expect("mkdir");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        let err = ScreenshotDir::open(&target).expect_err("world-writable is refused");
        assert!(
            matches!(err, ScreenshotDirError::Untrusted { .. }),
            "{err:?}"
        );
        // Not the sticky exemption a directory on the way to a program gets:
        // 1777 is refused too.
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o1777)).expect("chmod");
        assert!(matches!(
            ScreenshotDir::open(&target),
            Err(ScreenshotDirError::Untrusted { .. })
        ));
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Names are minted, monotone, and contain nothing but the two integers
    /// this struct owns.
    #[test]
    fn names_are_minted_and_carry_no_separator() {
        let dir = tmpdir("names");
        let mut target = ScreenshotDir::open(&dir).expect("clean dir");
        let one = target.write(b"first").expect("write");
        let two = target.write(b"second").expect("write");
        assert_ne!(one, two);
        for name in [&one, &two] {
            assert!(!name.contains('/'), "{name}");
            assert!(
                name.starts_with("vitrin-") && name.ends_with(".png"),
                "{name}"
            );
        }
        assert_eq!(std::fs::read(dir.join(&one)).expect("read"), b"first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlink planted at the name the core is about to mint does not get
    /// followed: `O_EXCL` fails the create instead of writing through it.
    #[test]
    fn a_symlink_planted_at_the_next_name_is_never_followed() {
        let dir = tmpdir("plant");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"precious").expect("write");
        let shots = dir.join("shots");
        std::fs::create_dir_all(&shots).expect("mkdir");
        let mut target = ScreenshotDir::open(&shots).expect("clean dir");
        // Mint the name the next write will use, without writing it: the
        // counter is advanced by `write`, so the first name is index 1.
        let next = format!("vitrin-{}-0001.png", target.session);
        std::os::unix::fs::symlink(&victim, shots.join(&next)).expect("symlink");
        // The planted name is SKIPPED, not fatal: the write succeeds under the
        // next name. Refusing to follow the symlink is the safety property;
        // giving up on the screenshot was a liveness bug wearing its clothes,
        // and one an app could hold open forever by planting the next names.
        let written = target
            .write(b"attacker-visible")
            .expect("a planted name must cost a name, not the screenshot");
        assert_ne!(written, next, "the planted name must not have been used");
        assert_eq!(
            std::fs::read(&victim).expect("read"),
            b"precious",
            "the symlink's target was never written through"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A same-uid app cannot permanently disable the human's screenshot
    /// key** (WS-E.2.4 review).
    ///
    /// The names are predictable by construction, so an app can compute and
    /// plant every one this session will mint. With a single `O_EXCL` attempt
    /// per press, that was a permanent denial: each press collided, failed, and
    /// advanced the counter by exactly one, so planting N names bought N dead
    /// presses. Planting a whole run of them and asserting ONE press still
    /// writes is what distinguishes "refuses to clobber" from "gives up".
    /// How long one press actually costs the compositor thread, measured
    /// rather than argued: the encode runs synchronously inside
    /// `route_physical_turn`, so this is a real stall on the human's frame.
    #[test]
    #[ignore = "measurement, not an assertion; run with --ignored"]
    fn measure_encode_cost_at_a_real_panel_size() {
        let (w, h) = (2560u32, 1600u32);
        let rgb = vec![0x5a; (w * h * 3) as usize];
        let start = std::time::Instant::now();
        let png = vitrin_png::encode_rgb(w, h, &rgb);
        let elapsed = start.elapsed();
        eprintln!(
            "MEASURED encode {w}x{h}: {:?}  -> {} bytes",
            elapsed,
            png.len()
        );
    }

    #[test]
    fn a_run_of_planted_names_costs_names_not_the_screenshot() {
        let dir = tmpdir("plantrun");
        let mut target = ScreenshotDir::open(&dir).expect("clean dir");
        for i in 1..=20u32 {
            std::fs::write(
                dir.join(format!("vitrin-{}-{:04}.png", target.session, i)),
                b"squat",
            )
            .expect("plant");
        }
        let written = target
            .write(b"real")
            .expect("twenty planted names must not cost the human their key");
        assert_eq!(
            std::fs::read(dir.join(&written)).expect("read"),
            b"real",
            "the screenshot must be under the name the write reported"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file on disk is the encoder's output over the view's pixels, byte
    /// for byte — checked by **re-encoding** rather than decoding, which is
    /// what a repository with no PNG decoder can honestly assert.
    #[test]
    fn the_written_file_is_the_encoding_of_the_realm_view() {
        let dir = tmpdir("bytes");
        let mut target = ScreenshotDir::open(&dir).expect("clean dir");
        let (w, h) = (4u32, 3u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 7) as u8, (i * 11) as u8, (i * 13) as u8, 0xff])
            .collect();
        let (name, digest) =
            capture_to_file(&mut target, ScreenshotGesture::Full, &rgba, w, h).expect("written");
        let on_disk = std::fs::read(dir.join(&name)).expect("read back");
        let rgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        assert_eq!(on_disk, vitrin_png::encode_rgb(w, h, &rgb));
        assert_eq!(
            digest.to_hex(),
            ObservationDigest::of(&on_disk).to_hex(),
            "the journal's digest identifies the bytes on disk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A degenerate view is refused with a label, and nothing is created.
    #[test]
    fn a_degenerate_view_writes_no_file() {
        let dir = tmpdir("degenerate");
        let mut target = ScreenshotDir::open(&dir).expect("clean dir");
        assert_eq!(
            capture_to_file(&mut target, ScreenshotGesture::Full, &[], 0, 0),
            Err("encode_failed")
        );
        assert_eq!(
            std::fs::read_dir(&dir).expect("readdir").count(),
            0,
            "a refused encode leaves nothing behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
