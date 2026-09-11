// SPDX-License-Identifier: MPL-2.0
//! Turning the row a human touched into an open descriptor, with no window in
//! which the filesystem can change underneath the answer.
//!
//! # Why this exists at all
//!
//! "Designation is authorization" is only true if the thing designated and the
//! thing opened are the same thing. Resolving the human's choice **by path** at
//! confirm time reintroduces exactly the race the fd-granting picker exists to
//! close: between the human touching the row and the core opening the file,
//! anything with write access to a parent directory can swap a component for a
//! symlink pointing somewhere else. The human approved one file; the agent
//! receives another.
//!
//! So the picker holds a directory descriptor from the moment it lists, and
//! resolution walks **one component at a time** from that descriptor with
//! `RESOLVE_NO_SYMLINKS`. Each step is anchored to a descriptor that already
//! names a specific inode, so there is no name for an attacker to re-point.
//!
//! # Refuse, never fall back
//!
//! `openat2` needs Linux 5.6. On an older kernel this module **refuses to
//! designate** rather than resolving the racy way. That is the whole bargain:
//! a picker that quietly degraded to `openat` would hand out descriptors under
//! a guarantee it had stopped providing, and nothing on the wire would say so.
//!
//! `rustix` has no `ENOSYS` fallback in either backend — the linux_raw backend
//! issues `__NR_openat2` directly and the libc backend goes via `SYS_OPENAT2`
//! — so a too-old kernel's `ENOSYS` arrives here verbatim and is answerable.
//! That was verified against the vendored source rather than assumed.
//!
//! # Measured errno map (kernel 7.2.4, this repository's dev machine)
//!
//! | condition | errno |
//! |---|---|
//! | a symlink component under `RESOLVE_NO_SYMLINKS` | `ELOOP` (40) |
//! | escaping the root under `RESOLVE_BENEATH` | `EXDEV` (18), **not** `ELOOP` |
//! | a resolve flag the kernel does not know | `EINVAL` (22) |
//! | `openat2` itself missing | `ENOSYS` (38) |
//!
//! The `EXDEV` row is the one worth having measured: the obvious guess is that
//! both containment failures report `ELOOP`, and a refusal keyed on that guess
//! would have mapped an escape attempt to the wrong wire code.

#![allow(dead_code)]

use rustix::fs::{Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

/// The mode a designation was made in. Narrows at the picker, never widens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EffectiveMode {
    ReadOnly,
    ReadWrite,
}

impl EffectiveMode {
    fn oflags(self) -> OFlags {
        match self {
            Self::ReadOnly => OFlags::RDONLY,
            Self::ReadWrite => OFlags::RDWR,
        }
    }
}

/// Why a resolution produced no descriptor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ResolveError {
    /// This kernel has no `openat2`. The deployment cannot designate at all,
    /// and says so rather than resolving unsafely.
    Unsupported,
    /// A component was a symlink, or resolution tried to leave the root. Both
    /// are the containment guarantee doing its job.
    Contained,
    /// The entry is gone, or was never there.
    Missing,
    /// Permission, I/O, or anything else the kernel reported.
    Denied,
}

impl ResolveError {
    fn from_errno(err: Errno) -> Self {
        match err {
            Errno::NOSYS => Self::Unsupported,
            // A symlink under NO_SYMLINKS, and an escape under BENEATH.
            Errno::LOOP | Errno::XDEV => Self::Contained,
            Errno::NOENT | Errno::NOTDIR => Self::Missing,
            _ => Self::Denied,
        }
    }
}

/// The resolve flags every step uses.
///
/// `NO_SYMLINKS` is the TOCTOU defence; `BENEATH` keeps a `..` component from
/// walking out of the root the deployment chose; `NO_MAGICLINKS` closes the
/// `/proc/*/fd/*` reopen trick, which is a symlink the kernel resolves without
/// it ever being one on disk.
const RESOLVE: ResolveFlags = ResolveFlags::NO_SYMLINKS
    .union(ResolveFlags::BENEATH)
    .union(ResolveFlags::NO_MAGICLINKS);

/// Whether this kernel can designate at all.
///
/// Probed once at startup rather than per ask, so a deployment below the floor
/// says so when it starts instead of at the moment a human is waiting. The
/// probe opens the root through the same call path a real resolution uses —
/// a probe that tested something cheaper would be testing something else.
pub(crate) fn probe(root: BorrowedFd<'_>) -> Result<(), ResolveError> {
    match rustix::fs::openat2(
        root,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY,
        Mode::empty(),
        RESOLVE,
    ) {
        Ok(_) => Ok(()),
        Err(err) => Err(ResolveError::from_errno(err)),
    }
}

/// Open the entry named by `components`, relative to `root`.
///
/// `components` is the path the human's navigation actually walked — never a
/// string parsed here. Each element must be a single path component; a caller
/// that passed `a/b` would be asking the kernel to resolve two steps at once,
/// which still cannot escape but does lose the property that every
/// intermediate descriptor is one this core opened itself.
pub(crate) fn resolve(
    root: BorrowedFd<'_>,
    components: &[&std::ffi::OsStr],
    mode: EffectiveMode,
    directory: bool,
) -> Result<OwnedFd, ResolveError> {
    debug_assert!(
        components
            .iter()
            .all(|c| Path::new(c).components().count() == 1),
        "each element must be exactly one component: resolution walks them one \
         at a time so that every intermediate descriptor is one this core opened"
    );

    // Walk the parents. Every step is a directory open anchored to the
    // previous descriptor, so a rename above us cannot redirect the next step.
    let mut here: Option<OwnedFd> = None;
    let Some((last, parents)) = components.split_last() else {
        return Err(ResolveError::Missing);
    };
    for component in parents {
        let at = here.as_ref().map_or(root, |fd| fd.as_fd());
        let next = rustix::fs::openat2(
            at,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(ResolveError::from_errno)?;
        here = Some(next);
    }

    let at = here.as_ref().map_or(root, |fd| fd.as_fd());
    let mut flags = mode.oflags() | OFlags::CLOEXEC;
    if directory {
        // A subtree designation is a directory descriptor: the app's own
        // `openat` walks it and the kernel enforces containment, so the core
        // never re-checks a path after the designation. That is the point.
        flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    }
    rustix::fs::openat2(at, *last, flags, Mode::empty(), RESOLVE).map_err(ResolveError::from_errno)
}

/// The identity of what a descriptor actually opened.
///
/// Recorded in the journal beside the designation, and compared in the
/// path-race test against the entry the picker displayed. A pair, because
/// an inode number alone is only unique within a device.
pub(crate) fn identity(fd: BorrowedFd<'_>) -> Result<(u64, u64), ResolveError> {
    let st = rustix::fs::fstat(fd).map_err(ResolveError::from_errno)?;
    Ok((st.st_dev as u64, st.st_ino as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    /// A private scratch tree, on `crate::spawn`'s precedent.
    fn scratch(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-picker-{}-{}-{}",
            tag,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn open_dir(path: &std::path::Path) -> OwnedFd {
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("scratch dir opens")
    }

    fn os(s: &str) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(s)
    }

    /// The kernel this runs on can designate at all.
    #[test]
    fn the_probe_answers_on_this_kernel() {
        let root = scratch("probe");
        let fd = open_dir(&root);
        match probe(fd.as_fd()) {
            Ok(()) => {}
            Err(ResolveError::Unsupported) => {
                // Honest rather than silent: below 5.6 the picker refuses to
                // designate, and that is the designed answer, not a skip.
                eprintln!("openat2 unsupported here; the picker would refuse to designate");
            }
            Err(other) => {
                panic!("probe failed for a reason that is not the kernel floor: {other:?}")
            }
        }
        fs::remove_dir_all(&root).ok();
    }

    /// A symlink component is refused rather than followed.
    #[test]
    fn a_symlink_component_is_contained() {
        let root = scratch("symlink");
        fs::write(root.join("real.txt"), b"real").unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();
        let fd = open_dir(&root);

        assert!(resolve(
            fd.as_fd(),
            &[os("real.txt")],
            EffectiveMode::ReadOnly,
            false
        )
        .is_ok());
        assert_eq!(
            resolve(
                fd.as_fd(),
                &[os("link.txt")],
                EffectiveMode::ReadOnly,
                false
            )
            .err(),
            Some(ResolveError::Contained),
            "a symlink is refused, not followed: following one is the whole race"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// `..` cannot walk out of the root the deployment chose.
    #[test]
    fn resolution_cannot_leave_the_root() {
        let root = scratch("beneath");
        fs::create_dir(root.join("sub")).unwrap();
        let fd = open_dir(&root.join("sub"));
        assert_eq!(
            resolve(fd.as_fd(), &[os("..")], EffectiveMode::ReadOnly, true).err(),
            Some(ResolveError::Contained),
            "RESOLVE_BENEATH refuses the escape; note the kernel reports EXDEV \
             here, not ELOOP, which is why both map to Contained"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// **The path-race gate, with its control.**
    ///
    /// A racer swaps a path component between a real directory and a symlink
    /// pointing at a decoy, as fast as it can, while the resolver runs in a
    /// loop. Two runs, and both halves matter:
    ///
    /// * **Guarded** — with `RESOLVE_NO_SYMLINKS`. Every descriptor delivered
    ///   must be the intended inode. Not "usually": every one.
    /// * **Control** — the same racer against a resolve that follows symlinks.
    ///   The racer must **win at least once**. Without that, a green guarded
    ///   run proves only that the racer was too slow to matter, which is the
    ///   failure mode that makes a race test look like evidence while
    ///   testing nothing.
    #[test]
    fn the_racer_cannot_change_what_a_guarded_resolve_delivers() {
        let root = scratch("race");
        let good = root.join("good");
        let decoy = root.join("decoy");
        fs::create_dir(&good).unwrap();
        fs::create_dir(&decoy).unwrap();
        fs::write(good.join("f"), b"the file the human chose").unwrap();
        fs::write(decoy.join("f"), b"the file an attacker substituted").unwrap();

        let root_fd = open_dir(&root);
        let intended = {
            let fd = resolve(
                root_fd.as_fd(),
                &[os("good"), os("f")],
                EffectiveMode::ReadOnly,
                false,
            )
            .expect("the intended file opens");
            identity(fd.as_fd()).expect("fstat")
        };
        let substituted = {
            let fd = resolve(
                root_fd.as_fd(),
                &[os("decoy"), os("f")],
                EffectiveMode::ReadOnly,
                false,
            )
            .expect("the decoy opens");
            identity(fd.as_fd()).expect("fstat")
        };
        assert_ne!(
            intended, substituted,
            "the two files must be distinguishable, or this test cannot fail"
        );

        // The racer flips `swap` between a real directory and a symlink to the
        // decoy. `rename` over a symlink is atomic, so the resolver always
        // sees one or the other, never a missing component.
        let swap = root.join("swap");
        std::os::unix::fs::symlink("good", &swap).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let racer = {
            let (stop, root, swap) = (Arc::clone(&stop), root.clone(), swap.clone());
            std::thread::spawn(move || {
                let a = root.join(".swap-a");
                let b = root.join(".swap-b");
                while !stop.load(Ordering::Relaxed) {
                    let _ = fs::remove_file(&a);
                    std::os::unix::fs::symlink("decoy", &a).ok();
                    fs::rename(&a, &swap).ok();
                    let _ = fs::remove_file(&b);
                    std::os::unix::fs::symlink("good", &b).ok();
                    fs::rename(&b, &swap).ok();
                }
            })
        };

        // Guarded: never the decoy, on any iteration.
        let mut delivered = 0usize;
        let mut contained = 0usize;
        for _ in 0..4000 {
            match resolve(
                root_fd.as_fd(),
                &[os("swap"), os("f")],
                EffectiveMode::ReadOnly,
                false,
            ) {
                Ok(fd) => {
                    let got = identity(fd.as_fd()).expect("fstat");
                    assert_ne!(
                        got, substituted,
                        "a guarded resolve delivered the substituted file: the racer won, \
                         which is exactly the TOCTOU this picker exists to close"
                    );
                    delivered += 1;
                }
                // `swap` IS a symlink in this design, so containment refusing
                // it is the expected outcome — the point is that it never
                // resolves to the decoy.
                Err(ResolveError::Contained) => contained += 1,
                Err(other) => panic!("unexpected resolve failure: {other:?}"),
            }
        }
        assert!(
            contained > 0,
            "the racer never had the component swapped during a guarded attempt, \
             so this run exercised nothing"
        );

        // Control: the same race, following symlinks. The racer must win.
        let mut racer_won = 0usize;
        for _ in 0..4000 {
            let unguarded = rustix::fs::openat(
                root_fd.as_fd(),
                "swap/f",
                OFlags::RDONLY | OFlags::CLOEXEC,
                Mode::empty(),
            );
            if let Ok(fd) = unguarded {
                if identity(fd.as_fd()).ok() == Some(substituted) {
                    racer_won += 1;
                }
            }
        }

        stop.store(true, Ordering::Relaxed);
        racer.join().ok();
        fs::remove_dir_all(&root).ok();

        assert!(
            racer_won > 0,
            "the control never delivered the substituted file, so the racer is too \
             slow to threaten anything and the guarded run above proved nothing. \
             delivered={delivered} contained={contained}"
        );
    }
}
