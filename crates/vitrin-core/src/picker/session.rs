// SPDX-License-Identifier: MPL-2.0
//! One raised picker's state: where the human is, what they can see, and
//! which row they are on.
//!
//! # Navigation is by descriptor, never by path string
//!
//! This module holds two descriptors — the root the deployment chose and the
//! directory currently listed — and every move produces the next one by
//! opening *through* one it already holds ([`super::resolve`]). It never
//! builds a path string and re-resolves it, and that is not a stylistic
//! preference: re-resolving a name is exactly the race the whole `picker`
//! module exists to close, so a `String` of the current directory would
//! reintroduce it at the one place where the human has already been shown an
//! answer.
//!
//! **And the confirm hands out a descriptor too, not a path.** [`Chosen`]
//! carries the directory the human was standing in, held open, plus one name
//! inside it — so the settle funnel's `openat2` is one step from an inode
//! this core already had open. It carried a root-relative component list
//! until this was reviewed, and re-walking that list at settle time was a
//! resolution *by name* with the human's answer already given:
//! `RESOLVE_NO_SYMLINKS` does not stop a `rename(2)` swapping a different
//! real directory over a component between the confirm and the open. There is
//! no component list on `Chosen` any more, so that cannot come back by
//! somebody rewriting the funnel.
//!
//! Leaving a directory is the one move that cannot go "through" the
//! descriptor it holds, because `..` is refused by `RESOLVE_BENEATH`. So it
//! pops the component stack and re-walks from the **root descriptor** — every
//! step of that walk is still anchored to a descriptor this core opened, so
//! nothing about the containment guarantee weakens; what changes is only that
//! the walk is O(depth) rather than O(1).
//!
//! # What the human sees is what the digest saw
//!
//! [`super::listing::build`] wants the digest of the pixels a row draws, so
//! that two rows which *render* alike get told apart in the gutter. This
//! module supplies that by actually rasterizing each row — the transcript,
//! elided to the name field's width, drawn through [`crate::paint::text`] and
//! hashed. Digesting the transcript *text* instead would have been far
//! cheaper and would have made the whole repair vacuous: transcripts are
//! globally injective by [`crate::paint::transcript`]'s own proof, so a
//! text-keyed digest finds no collisions ever and the gutter would stay empty
//! by construction while claiming to protect against look-alikes.
//!
//! **One obligation is owed and unmet**: [`NAME_FIELD_PX`] is the width this
//! module elides and digests at, and **nothing checks it against a renderer**,
//! because there is no renderer — no surface in this tree draws a picker (see
//! [`crate::consent::grab::ConsentGrab::raise_picker`]). Whoever draws one
//! owes making its name column that width or making this constant follow it;
//! until then the collision repair is computed for a column width that is
//! declared here and drawn nowhere.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::designation::{AskedFor, DesignationId};
use crate::paint::canvas::Canvas;
use crate::paint::text::{Text, Vetted};
use crate::paint::transcript::Transcript;
use crate::scene::BYTES_PER_PIXEL;

use super::keys::{Motion, PickerStep};
use super::listing::{self, Listing, ListingError, RowDigest};
use super::resolve::{self, EffectiveMode, ResolveError};

/// The width, in pixels, of the name field a row is drawn in.
///
/// See the module docs' last paragraph: this is a **declared** width with no
/// renderer to check it against. It is the elision width and therefore the
/// width at which two rows are judged to render alike.
pub(crate) const NAME_FIELD_PX: u32 = 320;

/// The tallest a single row's raster can be, for the scratch canvas.
///
/// Generous rather than tight: the digest is over a fixed-size buffer, so a
/// glyph that ascends or descends further than expected must land inside it
/// or two rows differing only in that overflow would digest alike.
const ROW_H: u32 = 32;

/// The baseline within [`ROW_H`].
const ROW_BASELINE: i32 = 22;

/// The most entries this module will list in one directory.
///
/// A bound on work, not a policy: every entry is transcribed **and
/// rasterized**, so an unbounded directory is an unbounded amount of the
/// trusted core's time inside one dispatch turn. Directories larger than this
/// are refused rather than truncated — showing a human the first 8192 of
/// 40000 entries, with no way to reach the rest and nothing saying so, is the
/// silent-narrowing failure this repository's docs are written to avoid.
const MAX_ENTRIES: usize = 8192;

/// Where the keyboard's next step lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// The listing itself: motion moves the selection.
    Listing,
    /// The Cancel button. Reachable by Tab, and **not** by Escape — Escape
    /// belongs to the dead-man chord ([`super::keys`]), so cancelling is a
    /// button on the surface rather than a key with a hidden meaning.
    Cancel,
    /// The Confirm button.
    Confirm,
}

/// What applying one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Nothing moved: a motion at the end of the list, a filter that matched
    /// nothing new, a descent that the kernel refused.
    Unchanged,
    /// The human-visible state changed.
    Changed,
    /// The human committed the selected row. The caller runs the settle
    /// funnel; **this method opens nothing**.
    Confirmed,
    /// The human cancelled.
    Cancelled,
}

/// Why a picker could not be built or moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerError {
    /// The directory could not be opened or read.
    Unreadable(ResolveError),
    /// More entries than [`MAX_ENTRIES`].
    TooManyEntries(usize),
    /// The listing could not be made pairwise distinguishable.
    Indistinguishable(ListingError),
}

/// The row the human is on, resolved into everything an `openat2` needs.
///
/// # It carries a **descriptor**, not a path and not a component list
///
/// This is the module's whole guarantee reaching the one caller that acts on
/// it. `dir` is the directory the human was actually standing in, held open,
/// and `name` is one component inside it — so the settle funnel's `openat2`
/// is a single step from an inode this core already had open, exactly like
/// every navigation step that got here.
///
/// It carried a root-relative **component list** until this was reviewed, and
/// that quietly re-opened the race the whole `picker` module exists to close.
/// Re-walking `work/notes.txt` from the root at settle time is a resolution
/// *by name*, and `RESOLVE_NO_SYMLINKS` does not save it: nothing stops
/// anything with write access to the root from `rename`-ing a different real
/// directory over `work` between the human's confirm and the core's open, and
/// the human would then have approved one file and the agent received
/// another. Holding `dir` removes the names from the question. It is not a
/// rule anybody has to keep applying — there is no longer a component list
/// here to re-walk.
#[derive(Debug)]
pub(crate) struct Chosen {
    /// The directory the human was standing in, held open.
    ///
    /// A private duplicate, so it outlives the [`PickerSession`] that made it
    /// — the session is dropped the moment the human confirms, and the funnel
    /// runs afterwards.
    pub(crate) dir: OwnedFd,
    /// The one component to open inside [`Self::dir`]: the row's own name.
    pub(crate) name: Vec<u8>,
    /// The mode the descriptor is opened with.
    pub(crate) mode: EffectiveMode,
    /// Whether this designates a directory subtree.
    pub(crate) directory: bool,
    /// Where in the tree that directory is, **for the log only**. Never
    /// resolved: see this type's own docs for why re-walking it would undo
    /// the guarantee `dir` provides.
    pub(crate) shown_path: Vec<OsString>,
}

/// One directory entry as the picker knows it.
#[derive(Debug, Clone)]
struct Entry {
    name: Vec<u8>,
    /// Whether descending into it is meaningful. Taken from `getdents`'
    /// `d_type` where the filesystem provides one and from an `openat2`
    /// probe where it does not — never from the name.
    dir: bool,
}

/// The deployment's picker root: a directory descriptor held open for the
/// life of the session.
///
/// Held open rather than re-opened per designation for the reason the whole
/// module exists: a root re-resolved from a string at each ask is a root an
/// attacker with write access to a parent can re-point between asks.
#[derive(Debug)]
pub(crate) struct PickerRoot {
    fd: OwnedFd,
    /// The path it was opened from, for the operator's log only. **Never
    /// re-resolved**: resolution walks from `fd`.
    shown: PathBuf,
}

impl PickerRoot {
    /// Open `path` as this session's picker root, and prove this kernel can
    /// designate through it.
    ///
    /// Both halves at startup, deliberately: a deployment below the `openat2`
    /// floor, or one whose configured root does not exist, says so when it
    /// comes up rather than at the moment a human is waiting for a card.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let fd = rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|err| format!("cannot open {} as the picker root: {err}", path.display()))?;
        resolve::probe(fd.as_fd()).map_err(|err| match err {
            ResolveError::Unsupported => format!(
                "this kernel has no openat2, so {} cannot be served as a picker root: the \
                 picker refuses to designate rather than resolving the racy way",
                path.display()
            ),
            other => format!(
                "the picker root {} failed its openat2 probe: {other:?}",
                path.display()
            ),
        })?;
        Ok(Self {
            fd,
            shown: path.to_path_buf(),
        })
    }

    pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub(crate) fn shown(&self) -> &Path {
        &self.shown
    }
}

/// One raised picker.
#[derive(Debug)]
pub(crate) struct PickerSession {
    id: DesignationId,
    ask: AskedFor,
    /// A private duplicate of the session root, so a `PickerSession` owns
    /// everything it walks from and cannot outlive a borrow.
    root: OwnedFd,
    /// The directory currently listed.
    here: OwnedFd,
    /// The components walked from the root to reach `here`, in order.
    path: Vec<OsString>,
    entries: Vec<Entry>,
    listing: Listing,
    /// Indices into `listing.rows` currently shown, after the filter.
    shown: Vec<usize>,
    /// Index into `shown`, never into `listing.rows`.
    cursor: usize,
    query: String,
    focus: Focus,
}

impl PickerSession {
    /// Build a picker rooted at `root`, listing the root itself.
    pub(crate) fn open(
        id: DesignationId,
        ask: AskedFor,
        root: BorrowedFd<'_>,
    ) -> Result<Self, PickerError> {
        let root_fd = root
            .try_clone_to_owned()
            .map_err(|_| PickerError::Unreadable(ResolveError::Denied))?;
        let here = root
            .try_clone_to_owned()
            .map_err(|_| PickerError::Unreadable(ResolveError::Denied))?;
        let (entries, listing) = read_and_build(here.as_fd())?;
        let shown = listing.filter("");
        Ok(Self {
            id,
            ask,
            root: root_fd,
            here,
            path: Vec::new(),
            entries,
            listing,
            shown,
            cursor: 0,
            query: String::new(),
            focus: Focus::Listing,
        })
    }

    pub(crate) fn id(&self) -> DesignationId {
        self.id
    }

    pub(crate) fn ask(&self) -> AskedFor {
        self.ask
    }

    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    /// How many rows the filter is showing.
    pub(crate) fn visible(&self) -> usize {
        self.shown.len()
    }

    /// The selected row's index within the visible set.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// The components walked from the root, for the journal and for tests.
    pub(crate) fn path(&self) -> &[OsString] {
        &self.path
    }

    /// The selected row's raw bytes, if anything is selected.
    ///
    /// The **bytes**, never the drawn text: what the human touched is a row,
    /// and the row remembers which name it stands for.
    pub(crate) fn selected_name(&self) -> Option<&[u8]> {
        self.selected().map(|e| e.name.as_slice())
    }

    /// The selected row, if the filter is showing anything.
    fn selected(&self) -> Option<&Entry> {
        let row = *self.shown.get(self.cursor)?;
        let name = &self.listing.rows.get(row)?.name;
        self.entries.iter().find(|e| &e.name == name)
    }

    /// The row the human is on, resolved into what an `openat2` needs.
    ///
    /// `None` when nothing is selected — an empty directory, or a filter that
    /// matches no row. A confirm in that state designates nothing rather than
    /// designating the directory the human happens to be standing in.
    ///
    /// Also `None` if the held directory descriptor cannot be duplicated,
    /// which is an fd-limit failure and nothing else. Refusing there is the
    /// fail-closed answer: the alternative would be to hand the funnel a
    /// component list to re-walk, which is the race this returns a descriptor
    /// to avoid.
    pub(crate) fn chosen(&self) -> Option<Chosen> {
        let entry = self.selected()?;
        // A `request_file` on a directory row cannot happen: `apply` descends
        // instead of confirming. A `request_dir` on a file row likewise
        // cannot: it refuses the confirm. So this is a straight read of the
        // ask, not a second policy.
        let directory = matches!(self.ask, AskedFor::Dir);
        let mode = match self.ask {
            AskedFor::File { write: true } => EffectiveMode::ReadWrite,
            // A directory descriptor is opened read-only whatever was asked
            // (`resolve` enforces it too); a read ask is read-only.
            AskedFor::File { write: false } | AskedFor::Dir => EffectiveMode::ReadOnly,
        };
        let dir = self
            .here
            .as_fd()
            .try_clone_to_owned()
            .inspect_err(|err| {
                tracing::warn!(%err, "the picker cannot duplicate the directory it is standing in");
            })
            .ok()?;
        Some(Chosen {
            dir,
            name: entry.name.clone(),
            mode,
            directory,
            shown_path: self.path.clone(),
        })
    }

    /// Apply one step the human typed.
    ///
    /// **Opens no file.** The most this can do is open a *directory* the
    /// human navigated into; the designated descriptor is minted by the
    /// caller's settle funnel, after the ledger has re-judged the grant. That
    /// split is the point: a revoked grant must not merely fail to send a
    /// descriptor, it must never cause the core to open one.
    pub(crate) fn apply(&mut self, step: PickerStep) -> Applied {
        match step {
            PickerStep::FocusNext => {
                self.focus = match self.focus {
                    Focus::Listing => Focus::Confirm,
                    Focus::Confirm => Focus::Cancel,
                    Focus::Cancel => Focus::Listing,
                };
                Applied::Changed
            }
            PickerStep::FocusPrev => {
                self.focus = match self.focus {
                    Focus::Listing => Focus::Cancel,
                    Focus::Cancel => Focus::Confirm,
                    Focus::Confirm => Focus::Listing,
                };
                Applied::Changed
            }
            PickerStep::Filter(ch) => {
                self.query.push(ch);
                self.refilter();
                Applied::Changed
            }
            PickerStep::FilterBackspace => {
                if self.query.pop().is_none() {
                    return Applied::Unchanged;
                }
                self.refilter();
                Applied::Changed
            }
            PickerStep::Move(motion) => self.move_by(motion),
            PickerStep::Confirm => match self.focus {
                Focus::Cancel => Applied::Cancelled,
                Focus::Listing | Focus::Confirm => self.confirm(),
            },
        }
    }

    /// A confirm on the listing or the Confirm button.
    fn confirm(&mut self) -> Applied {
        let Some(entry) = self.selected().cloned() else {
            // Nothing selected: an empty directory, or a filter matching
            // nothing. Confirming designates nothing rather than falling back
            // to something the human did not point at.
            return Applied::Unchanged;
        };
        match (self.ask, entry.dir) {
            // A file ask on a directory row is a descent, which is what every
            // file dialog does and what the human means by pressing Return on
            // a folder.
            (AskedFor::File { .. }, true) => self.descend(&entry.name),
            (AskedFor::File { .. }, false) => Applied::Confirmed,
            (AskedFor::Dir, true) => Applied::Confirmed,
            // `request_dir` cannot designate a file. Refused rather than
            // silently designating its parent.
            (AskedFor::Dir, false) => Applied::Unchanged,
        }
    }

    fn move_by(&mut self, motion: Motion) -> Applied {
        // Motion belongs to the listing. On a button it does nothing rather
        // than moving a selection the human cannot see the effect of.
        if self.focus != Focus::Listing && !matches!(motion, Motion::Out | Motion::In) {
            return Applied::Unchanged;
        }
        let last = self.shown.len().saturating_sub(1);
        let page = 8usize;
        let before = self.cursor;
        match motion {
            Motion::Up => self.cursor = self.cursor.saturating_sub(1),
            Motion::Down => self.cursor = (self.cursor + 1).min(last),
            Motion::PageUp => self.cursor = self.cursor.saturating_sub(page),
            Motion::PageDown => self.cursor = (self.cursor + page).min(last),
            Motion::First => self.cursor = 0,
            Motion::Last => self.cursor = last,
            Motion::Out => return self.ascend(),
            Motion::In => {
                let Some(entry) = self.selected().cloned() else {
                    return Applied::Unchanged;
                };
                if !entry.dir {
                    return Applied::Unchanged;
                }
                return self.descend(&entry.name);
            }
        }
        if self.cursor == before {
            Applied::Unchanged
        } else {
            Applied::Changed
        }
    }

    /// Enter `name`, opening it **through the descriptor already held**.
    fn descend(&mut self, name: &[u8]) -> Applied {
        let component = OsString::from_vec(name.to_vec());
        let next = match resolve::resolve(
            self.here.as_fd(),
            &[component.as_os_str()],
            EffectiveMode::ReadOnly,
            true,
        ) {
            Ok(fd) => fd,
            Err(err) => {
                // Containment refusing a symlinked directory is the guarantee
                // working, not a failure: the picker will not walk somewhere
                // the human cannot see it walked to.
                tracing::debug!(?err, "picker cannot enter the selected directory");
                return Applied::Unchanged;
            }
        };
        let (entries, listing) = match read_and_build(next.as_fd()) {
            Ok(built) => built,
            Err(err) => {
                tracing::debug!(?err, "picker cannot list the selected directory");
                return Applied::Unchanged;
            }
        };
        self.here = next;
        self.path.push(component);
        self.entries = entries;
        self.listing = listing;
        self.query.clear();
        self.refilter();
        self.cursor = 0;
        Applied::Changed
    }

    /// Leave the current directory: pop a component and **re-walk from the
    /// root**.
    ///
    /// `..` is not an option: `RESOLVE_BENEATH` refuses it, deliberately, so
    /// that no navigation can leave the root the deployment chose. Re-walking
    /// keeps every intermediate descriptor one this core opened.
    fn ascend(&mut self) -> Applied {
        if self.path.is_empty() {
            // Already at the root; there is nowhere above it by design.
            return Applied::Unchanged;
        }
        let mut walk = self.path.clone();
        let left = walk.pop().expect("non-empty, checked above");
        let next = if walk.is_empty() {
            match self.root.as_fd().try_clone_to_owned() {
                Ok(fd) => fd,
                Err(err) => {
                    tracing::warn!(%err, "picker cannot re-open its own root");
                    return Applied::Unchanged;
                }
            }
        } else {
            let refs: Vec<&OsStr> = walk.iter().map(OsString::as_os_str).collect();
            match resolve::resolve(self.root.as_fd(), &refs, EffectiveMode::ReadOnly, true) {
                Ok(fd) => fd,
                Err(err) => {
                    tracing::debug!(?err, "picker cannot re-walk to the parent directory");
                    return Applied::Unchanged;
                }
            }
        };
        let (entries, listing) = match read_and_build(next.as_fd()) {
            Ok(built) => built,
            Err(err) => {
                tracing::debug!(?err, "picker cannot list the parent directory");
                return Applied::Unchanged;
            }
        };
        self.here = next;
        self.path = walk;
        self.entries = entries;
        self.listing = listing;
        self.query.clear();
        self.refilter();
        // Land on the directory just left, so a human stepping out and back
        // in does not lose their place.
        self.cursor = self
            .shown
            .iter()
            .position(|&row| self.listing.rows[row].name == left.as_bytes())
            .unwrap_or(0);
        Applied::Changed
    }

    fn refilter(&mut self) {
        self.shown = self.listing.filter(&self.query);
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
    }
}

/// Read one directory and build its distinguishable listing.
fn read_and_build(dir: BorrowedFd<'_>) -> Result<(Vec<Entry>, Listing), PickerError> {
    let entries = read_entries(dir)?;
    let names: Vec<Vec<u8>> = entries.iter().map(|e| e.name.clone()).collect();
    let mut ink = RowInk::new();
    let listing =
        listing::build(&names, |t| ink.digest(t)).map_err(PickerError::Indistinguishable)?;
    Ok((entries, listing))
}

/// Read `dir`'s entries, excluding `.` and `..`, in a deterministic order.
///
/// Sorted by the raw bytes: `getdents` order is the filesystem's, which is
/// neither stable across runs nor the same on two machines, and a human
/// comparing two sessions must not see rows move.
fn read_entries(dir: BorrowedFd<'_>) -> Result<Vec<Entry>, PickerError> {
    let mut reader = rustix::fs::Dir::read_from(dir)
        .map_err(|err| PickerError::Unreadable(resolve_error_of(err)))?;
    let mut out: Vec<Entry> = Vec::new();
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|err| PickerError::Unreadable(resolve_error_of(err)))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if out.len() >= MAX_ENTRIES {
            return Err(PickerError::TooManyEntries(MAX_ENTRIES + 1));
        }
        let dir_flag = match entry.file_type() {
            rustix::fs::FileType::Directory => true,
            // `d_type` is unknown on some filesystems. Probing with an
            // `O_DIRECTORY` open through the same guarded path a descent
            // would take is the only honest answer; a name-based guess is
            // not an answer at all.
            rustix::fs::FileType::Unknown => resolve::resolve(
                dir,
                &[OsStr::from_bytes(name)],
                EffectiveMode::ReadOnly,
                true,
            )
            .is_ok(),
            _ => false,
        };
        out.push(Entry {
            name: name.to_vec(),
            dir: dir_flag,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn resolve_error_of(err: rustix::io::Errno) -> ResolveError {
    match err {
        rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR => ResolveError::Missing,
        _ => ResolveError::Denied,
    }
}

/// The row rasterizer [`super::listing::build`] asks for: real pixels, real
/// digest.
///
/// Holds the font cache and one scratch buffer across a whole listing, so a
/// directory of N entries pays one allocation rather than N.
struct RowInk {
    text: Text,
    buf: Vec<u8>,
}

impl RowInk {
    fn new() -> Self {
        Self {
            text: Text::new(),
            buf: vec![0u8; (NAME_FIELD_PX * ROW_H) as usize * BYTES_PER_PIXEL],
        }
    }

    /// Digest exactly the pixels this row's name field would carry.
    fn digest(&mut self, transcript: &Transcript) -> RowDigest {
        for byte in self.buf.iter_mut() {
            *byte = 0;
        }
        let drawn = self.elide(&transcript.text);
        // A transcript is by construction drawable (`route(ch) != Escape` for
        // every character it emits), so `Vetted::new` cannot fail here. If it
        // somehow did, an empty run digests to the blank row -- which is a
        // collision with every other such row and therefore lands in the
        // gutter rather than passing silently.
        if let Some(vetted) = Vetted::new(&drawn) {
            if let Some(mut canvas) = Canvas::new(&mut self.buf, NAME_FIELD_PX, ROW_H) {
                self.text.draw_runs(
                    &mut canvas,
                    &[(vetted, [0xff, 0xff, 0xff])],
                    0,
                    ROW_BASELINE,
                );
            }
        }
        *blake3::hash(&self.buf).as_bytes()
    }

    /// Cut `s` to [`NAME_FIELD_PX`], marking the cut.
    ///
    /// **Cuts from the end**, which is why the collision repair's mark lives
    /// in a reserved gutter rather than in a suffix: the rows that collide are
    /// exactly the ones sharing a long prefix, so a suffix mark would be the
    /// first thing this function removed.
    fn elide(&mut self, s: &str) -> String {
        let whole = Vetted::new(s);
        if let Some(v) = &whole {
            if self.text.width_vetted(v) <= NAME_FIELD_PX {
                return s.to_owned();
            }
        }
        let mut cut = String::new();
        for ch in s.chars() {
            let mut candidate = cut.clone();
            candidate.push(ch);
            candidate.push_str("...");
            let fits = Vetted::new(&candidate)
                .map(|v| self.text.width_vetted(&v) <= NAME_FIELD_PX)
                .unwrap_or(false);
            if !fits {
                break;
            }
            cut.push(ch);
        }
        cut.push_str("...");
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-picker-session-{}-{}-{}",
            tag,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    fn session(root: &Path, ask: AskedFor) -> (PickerRoot, PickerSession) {
        let picker_root = PickerRoot::open(root).expect("the scratch root opens");
        let session = PickerSession::open(
            DesignationId::from_u32_for_test(1),
            ask,
            picker_root.as_fd(),
        )
        .expect("the root lists");
        (picker_root, session)
    }

    fn names_shown(s: &PickerSession) -> Vec<String> {
        s.shown
            .iter()
            .map(|&row| String::from_utf8_lossy(&s.listing.rows[row].name).into_owned())
            .collect()
    }

    /// **The property the whole module is shaped around**: entering and
    /// leaving a directory never builds a path string to re-resolve.
    ///
    /// Asserted against behaviour rather than by reading the source: the
    /// picker descends into a directory, the racer *replaces that directory
    /// with a symlink to a decoy*, and the picker's held descriptor must keep
    /// listing the original contents. A picker that re-resolved by name would
    /// list the decoy's.
    #[test]
    fn a_held_directory_survives_its_name_being_repointed() {
        let root = scratch("repoint");
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/inside.txt"), b"x").unwrap();
        fs::create_dir(root.join("decoy")).unwrap();
        fs::write(root.join("decoy/planted.txt"), b"x").unwrap();

        let (hold, mut s) = session(&root, AskedFor::File { write: false });
        // Rows are sorted: decoy, real.
        assert_eq!(names_shown(&s), vec!["decoy", "real"]);
        s.apply(PickerStep::Move(Motion::Down));
        assert_eq!(s.apply(PickerStep::Move(Motion::In)), Applied::Changed);
        assert_eq!(names_shown(&s), vec!["inside.txt"]);

        // The name `real` now points somewhere else entirely.
        fs::rename(root.join("real"), root.join("real-moved")).unwrap();
        std::os::unix::fs::symlink("decoy", root.join("real")).unwrap();

        // **The control, and without it the assertion below proves nothing.**
        // A re-resolution of the same *name* must now reach somewhere else --
        // if it did not, the racer changed nothing and the guarded half would
        // be green against a filesystem that never moved. Containment refuses
        // the symlink outright, which is itself the answer a path-based picker
        // would have had to cope with; either way it is no longer the
        // directory the human is standing in.
        let by_name = resolve::resolve(hold.as_fd(), &[os("real")], EffectiveMode::ReadOnly, true);
        match by_name {
            Err(ResolveError::Contained) => {}
            Ok(fd) => {
                let (_e, listing) =
                    read_and_build(fd.as_fd()).expect("the re-resolved directory lists");
                assert!(
                    listing
                        .rows
                        .iter()
                        .all(|r| r.name != b"inside.txt".to_vec()),
                    "the control did not move: `real` still names the original directory, so \
                     the guarded assertion below is vacuous"
                );
            }
            Err(other) => panic!("unexpected re-resolution failure: {other:?}"),
        }

        // The picker is still inside the directory it opened, because it
        // holds that directory's descriptor and never re-resolves its name.
        let (_e, listing) = read_and_build(s.here.as_fd()).expect("the held directory still lists");
        assert_eq!(listing.rows.len(), 1);
        assert_eq!(listing.rows[0].name, b"inside.txt".to_vec());
        fs::remove_dir_all(&root).ok();
    }

    /// Leaving the root is not possible: `..` is refused, and the component
    /// stack cannot be popped past empty.
    #[test]
    fn navigation_cannot_leave_the_root() {
        let root = scratch("bounded");
        fs::create_dir(root.join("sub")).unwrap();
        let (_hold, mut s) = session(&root, AskedFor::Dir);
        assert_eq!(
            s.apply(PickerStep::Move(Motion::Out)),
            Applied::Unchanged,
            "there is nothing above the root the deployment chose"
        );
        assert!(s.path().is_empty());
        s.apply(PickerStep::Move(Motion::In));
        assert_eq!(s.path().len(), 1);
        assert_eq!(s.apply(PickerStep::Move(Motion::Out)), Applied::Changed);
        assert!(s.path().is_empty());
        assert_eq!(
            s.apply(PickerStep::Move(Motion::Out)),
            Applied::Unchanged,
            "and still nothing above it after coming back"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// A `request_file` confirm on a directory descends; a `request_dir`
    /// confirm on a file designates nothing.
    #[test]
    fn a_confirm_answers_the_ask_it_was_made_under() {
        let root = scratch("asks");
        fs::create_dir(root.join("folder")).unwrap();
        fs::write(root.join("folder/leaf.txt"), b"x").unwrap();

        let (_h1, mut file_ask) = session(&root, AskedFor::File { write: false });
        assert_eq!(
            file_ask.apply(PickerStep::Confirm),
            Applied::Changed,
            "a file ask on a folder row descends rather than designating the folder"
        );
        assert_eq!(file_ask.path().len(), 1);
        assert_eq!(file_ask.apply(PickerStep::Confirm), Applied::Confirmed);

        let (_h2, mut dir_ask) = session(&root, AskedFor::Dir);
        assert_eq!(dir_ask.apply(PickerStep::Confirm), Applied::Confirmed);
        dir_ask.apply(PickerStep::Move(Motion::In));
        assert_eq!(
            dir_ask.apply(PickerStep::Confirm),
            Applied::Unchanged,
            "request_dir cannot designate a file, and must not silently designate its parent"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// A confirm with nothing selected designates nothing.
    #[test]
    fn a_confirm_with_an_empty_filter_result_designates_nothing() {
        let root = scratch("empty");
        fs::write(root.join("a.txt"), b"x").unwrap();
        let (_hold, mut s) = session(&root, AskedFor::File { write: false });
        for ch in "zzzz".chars() {
            s.apply(PickerStep::Filter(ch));
        }
        assert_eq!(s.visible(), 0);
        assert!(s.chosen().is_none());
        assert_eq!(
            s.apply(PickerStep::Confirm),
            Applied::Unchanged,
            "an empty result must not fall back to designating something the human did not \
             point at"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Cancel is a focusable button, reached by Tab and never by Escape.
    #[test]
    fn cancel_is_reached_by_focus_and_not_by_a_key() {
        let root = scratch("cancel");
        fs::write(root.join("a.txt"), b"x").unwrap();
        let (_hold, mut s) = session(&root, AskedFor::File { write: false });
        assert_eq!(s.focus(), Focus::Listing);
        s.apply(PickerStep::FocusNext);
        assert_eq!(s.focus(), Focus::Confirm);
        s.apply(PickerStep::FocusNext);
        assert_eq!(s.focus(), Focus::Cancel);
        assert_eq!(s.apply(PickerStep::Confirm), Applied::Cancelled);
        // Escape decodes to no step at all, so it cannot reach this state.
        assert_eq!(super::super::keys::decode(0xff1b), None);
        fs::remove_dir_all(&root).ok();
    }

    /// The chosen row carries **the directory descriptor the human was
    /// standing in**, and one name inside it resolves to the file they were
    /// on.
    #[test]
    fn the_chosen_row_resolves_to_what_was_selected() {
        let root = scratch("chosen");
        fs::create_dir(root.join("d")).unwrap();
        fs::write(root.join("d/target.txt"), b"the chosen bytes").unwrap();
        let (_hold, mut s) = session(&root, AskedFor::File { write: false });
        s.apply(PickerStep::Move(Motion::In));
        let chosen = s.chosen().expect("a row is selected");
        assert_eq!(chosen.name, b"target.txt".to_vec());
        assert_eq!(
            chosen.shown_path,
            vec![OsString::from("d")],
            "the walked path is still carried, for the log"
        );
        let name = OsString::from_vec(chosen.name.clone());
        let fd = resolve::resolve(
            chosen.dir.as_fd(),
            &[name.as_os_str()],
            chosen.mode,
            chosen.directory,
        )
        .expect("one component from the held directory resolves");
        let mut buf = [0u8; 32];
        let n = rustix::io::read(&fd, &mut buf).expect("read");
        assert_eq!(&buf[..n], b"the chosen bytes");
        fs::remove_dir_all(&root).ok();
    }

    /// **A directory swapped under the human between confirm and open must
    /// not change what is delivered** — the race the settle funnel used to
    /// have, pinned.
    ///
    /// The funnel once re-walked the chosen row's root-relative components at
    /// settle time. `RESOLVE_NO_SYMLINKS` does not close that: `rename(2)` is
    /// atomic and involves no symlink, so anything with write access to the
    /// root could swap a *real* directory over `d` between the human's confirm
    /// and the core's open, and the descriptor handed over would be a file the
    /// human never saw. `Chosen` now carries the directory descriptor instead,
    /// which names an inode no rename can re-point.
    ///
    /// **The control is the load-bearing half.** It re-walks the same
    /// components by name after the swap and asserts it really does land on
    /// the decoy — without it, a green guarded assertion would prove only that
    /// the swap never took effect.
    #[test]
    fn a_swapped_directory_cannot_change_what_the_confirm_designated() {
        let root = scratch("swap");
        fs::create_dir(root.join("d")).unwrap();
        fs::write(root.join("d/target.txt"), b"the chosen bytes").unwrap();
        fs::create_dir(root.join("decoy")).unwrap();
        fs::write(root.join("decoy/target.txt"), b"the attacker's bytes").unwrap();

        let (hold, mut s) = session(&root, AskedFor::File { write: false });
        s.apply(PickerStep::Move(Motion::In));
        assert_eq!(s.path(), [OsString::from("d")].as_slice());
        let chosen = s.chosen().expect("a row is selected");
        assert_eq!(chosen.name, b"target.txt".to_vec());
        // The picker itself is gone by the time the funnel runs: the card
        // comes down on the confirm. Only what `chosen()` handed over is
        // left, which is the whole point of it holding a descriptor.
        drop(s);

        // The swap, between the confirm and the open.
        fs::rename(root.join("d"), root.join("d.moved")).unwrap();
        fs::rename(root.join("decoy"), root.join("d")).unwrap();

        let name = OsString::from_vec(chosen.name.clone());
        let fd = resolve::resolve(
            chosen.dir.as_fd(),
            &[name.as_os_str()],
            chosen.mode,
            chosen.directory,
        )
        .expect("the held directory still resolves its own entry");
        let mut buf = [0u8; 64];
        let n = rustix::io::read(&fd, &mut buf).expect("read");
        assert_eq!(
            &buf[..n],
            b"the chosen bytes",
            "the swap changed what the confirm designated: the human approved one file and \
             the agent would have received another"
        );

        // Control: the same designation expressed as a component list --
        // which is what the funnel used to carry -- lands on the decoy.
        let walked = resolve::resolve(
            hold.as_fd(),
            &[os("d"), os("target.txt")],
            EffectiveMode::ReadOnly,
            false,
        )
        .expect("the swapped-in directory resolves by name");
        let n = rustix::io::read(&walked, &mut buf).expect("read");
        assert_eq!(
            &buf[..n],
            b"the attacker's bytes",
            "control: the racer did not actually re-point the name, so the guarded assertion \
             above proves nothing"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// **The rasterizer really rasterizes.** Two names that differ only past
    /// the elision point must digest alike, so the gutter marks them — which
    /// is the whole property a text-keyed digest would silently lose.
    #[test]
    fn rows_that_elide_alike_share_a_digest() {
        let mut ink = RowInk::new();
        let long = "a".repeat(200);
        let one = crate::paint::transcript::encode(format!("{long}-one.txt").as_bytes());
        let two = crate::paint::transcript::encode(format!("{long}-two.txt").as_bytes());
        assert_ne!(
            one.text, two.text,
            "the two transcripts differ, so a text-keyed digest would call them distinct"
        );
        assert_eq!(
            ink.digest(&one),
            ink.digest(&two),
            "past the name field's width these draw identical pixels, and the listing must \
             therefore mark them apart in the gutter"
        );
        let short_a = crate::paint::transcript::encode(b"alpha.txt");
        let short_b = crate::paint::transcript::encode(b"beta.txt");
        assert_ne!(
            ink.digest(&short_a),
            ink.digest(&short_b),
            "two short, plainly different names must not share a digest, or this rasterizer \
             is digesting nothing"
        );
    }

    /// The digest is a pure function of the text, as `listing::build`'s
    /// contract requires: the same transcript must digest the same however
    /// many rows came before it.
    #[test]
    fn the_row_digest_does_not_depend_on_row_order() {
        let mut ink = RowInk::new();
        let a = crate::paint::transcript::encode(b"notes.txt");
        let first = ink.digest(&a);
        for name in ["other.txt", "third.txt", "fourth.txt"] {
            ink.digest(&crate::paint::transcript::encode(name.as_bytes()));
        }
        assert_eq!(
            ink.digest(&a),
            first,
            "a rasterizer that varied with position would make every digest meaningless"
        );
    }

    /// A directory listing is byte-sorted, so the same directory shows the
    /// same order on every run and on every machine.
    #[test]
    fn entries_are_ordered_deterministically() {
        let root = scratch("order");
        for name in ["zeta", "alpha", "Mid", "0nine"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        let (_hold, s) = session(&root, AskedFor::File { write: false });
        assert_eq!(names_shown(&s), vec!["0nine", "Mid", "alpha", "zeta"]);
        fs::remove_dir_all(&root).ok();
    }

    /// A root that cannot be opened is refused, so a deployment says so at
    /// startup rather than when a human is waiting.
    #[test]
    fn a_missing_root_is_refused() {
        let root = scratch("missing");
        let gone = root.join("not-here");
        assert!(PickerRoot::open(&gone).is_err());
        fs::remove_dir_all(&root).ok();
    }
}
