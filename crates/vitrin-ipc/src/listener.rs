//! The principal-facing listening socket.
//!
//! [`Listener::bind`] owns the whole named-socket lifecycle: it creates the
//! runtime directory `0700`, takes a `flock`-guarded lock file next to the
//! socket (the libwayland pattern), clears any stale socket left by a
//! crashed predecessor, and binds with `SOCK_CLOEXEC`. Holding the lock is
//! what makes unlinking a preexisting socket path safe: a *live* core still
//! holds its own lock, so a second bind fails with `AddrInUse` instead of
//! stealing the path; a dead one's lock was released by the kernel, so its
//! leftover socket file is provably stale.
//!
//! Only principals connect through a named socket. Shims inherit a
//! socketpair end at fork (conventions section 1.2), so the connection
//! class is fixed by transport construction -- there is no way to reach the
//! shim dispatch table through this listener.

use std::fs;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{flock, fstat, stat, FlockOperation};
use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

use crate::connection::Connection;

/// Pending-connection queue length. Accepts are serviced from the core's
/// event loop, so this only needs to absorb a burst of simultaneous
/// connects; 128 matches libwayland's choice.
const LISTEN_BACKLOG: i32 = 128;

/// How many times `lock_at` re-races a lock file whose inode changed
/// underneath it before giving up. Each retry means one competing bind
/// created (or a dropping listener unlinked) the lock file between our
/// `open` and our post-`flock` verification -- transient by construction,
/// so a small bound only guards against a pathological churn loop.
const LOCK_RETRY_LIMIT: u32 = 16;

/// Open `<socket>.lock`, take the exclusive `flock`, and verify the locked
/// inode is still the one on disk at `lock_path`.
///
/// The verification closes an unlink TOCTOU: a dropping [`Listener`]
/// unlinks its lock file and only then releases the `flock` (when the
/// `File` closes), so a binder that opened the *old* path just before the
/// unlink can win the flock on an orphaned inode -- holding a lock no
/// later binder can see, which would let that later binder unlink a live
/// socket out from under it. Re-checking `fstat(locked fd) == stat(path)`
/// and retrying on mismatch guarantees the winner holds the lock on the
/// inode every future binder will also open.
fn lock_at(lock_path: &Path, socket_path: &Path) -> io::Result<fs::File> {
    for _ in 0..LOCK_RETRY_LIMIT {
        // std opens O_CLOEXEC like every fd in this crate.
        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        flock(&lock_file, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "another process holds the lock for socket {}",
                    socket_path.display()
                ),
            )
        })?;
        let locked = fstat(&lock_file)?;
        match stat(lock_path) {
            Ok(on_disk) if on_disk.st_dev == locked.st_dev && on_disk.st_ino == locked.st_ino => {
                return Ok(lock_file);
            }
            // The file we locked is no longer the file at the path
            // (unlinked or replaced mid-race): drop lock and fd, retry
            // against whatever is there now.
            Ok(_) | Err(rustix::io::Errno::NOENT) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!(
            "lock file for socket {} kept changing underneath us",
            socket_path.display()
        ),
    ))
}

/// A bound, listening Unix socket that yields [`Connection`]s with their
/// peer credentials already captured.
pub struct Listener {
    fd: OwnedFd,
    socket_path: PathBuf,
    lock_path: PathBuf,
    /// Held for the listener's lifetime; the kernel drops the `flock` when
    /// this closes (even if the process dies without running `Drop`).
    _lock_file: fs::File,
}

impl Listener {
    /// Bind and listen at `path` (conventionally
    /// [`paths::core_socket_path`](crate::paths::core_socket_path)).
    ///
    /// Fails with `ErrorKind::AddrInUse` if another live process holds the
    /// lock for this path. The parent directory is created `0700` if
    /// missing, so the socket is reachable only by the owning user.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let socket_path = path.as_ref().to_path_buf();
        if let Some(parent) = socket_path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }

        // Lock file beside the socket: `<socket>.lock`. std opens it
        // O_CLOEXEC like every fd in this crate.
        let lock_path = {
            let mut p = socket_path.clone().into_os_string();
            p.push(".lock");
            PathBuf::from(p)
        };
        let lock_file = lock_at(&lock_path, &socket_path)?;

        // We hold the lock, so anything at the socket path is stale.
        match fs::remove_file(&socket_path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let fd = net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )?;
        let addr = SocketAddrUnix::new(&socket_path)?;
        net::bind(&fd, &addr)?;
        net::listen(&fd, LISTEN_BACKLOG)?;

        Ok(Listener {
            fd,
            socket_path,
            lock_path,
            _lock_file: lock_file,
        })
    }

    /// Accept one connection (`accept4` with `SOCK_CLOEXEC`), capturing its
    /// `SO_PEERCRED` before it is ever readable -- the "recorded at accept"
    /// guarantee the identity layer (P1.4.1) builds on. Blocking; readiness
    /// integration goes through [`AsFd`] (P1.2.2).
    pub fn accept(&self) -> io::Result<Connection> {
        let fd = loop {
            match net::accept_with(&self.fd, SocketFlags::CLOEXEC) {
                Err(rustix::io::Errno::INTR) => continue,
                r => break r.map_err(io::Error::from)?,
            }
        };
        Connection::from_fd(fd)
    }

    /// The path this listener is bound to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl AsFd for Listener {
    /// The listening socket, for readiness integration (calloop, P1.2.2).
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Unlink the socket first, then the lock file; the flock itself is
        // released when `_lock_file` closes after this body, so no other
        // process can be mid-bind on this path while the files disappear.
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.lock_path);
    }
}
