//! Socket path convention (settled in P1.2.1, per the plan's E2 table):
//!
//! ```text
//! $XDG_RUNTIME_DIR/vitrin-0/                 <- runtime_dir(), mode 0700
//! ├── core.sock                              <- core_socket_path(): the
//! │                                             principal-facing listener
//! ├── core.sock.lock                         <- Listener's flock guard
//! └── <realm_id>/                            <- shim_runtime_dir(): one
//!     │                                         private dir per spawned shim
//!     └── wayland-0                          <- shim_socket_path(): the
//!                                               shim's private Wayland
//!                                               socket; the app's
//!                                               WAYLAND_DISPLAY names only
//!                                               this (PRD Doc 2 section 4.1)
//! ```
//!
//! `vitrin-0` mirrors the protocol major version the way `wayland-0` does:
//! a future incompatible protocol can listen at `vitrin-1` beside a running
//! v0 core. The shim-to-core connection itself never appears in this tree
//! -- it is an inherited socketpair, not a named socket (conventions
//! section 1.2); what a shim's private directory holds is its *app-facing*
//! Wayland socket, plus whatever scratch the shim needs.
//!
//! Every helper comes in two forms: an environment-reading one (reads
//! `$XDG_RUNTIME_DIR`, fallible) and a pure `*_in` one taking the runtime
//! base explicitly (deterministic, what tests and the spawn manager use).
//! Note `sun_path` is 108 bytes on Linux; with a sane `$XDG_RUNTIME_DIR`
//! (`/run/user/<uid>`) every path built here fits with room to spare, and
//! an oversized one surfaces as an error at bind/connect time, not here.

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

/// Directory under `$XDG_RUNTIME_DIR` holding everything Vitrin: the core
/// socket, its lock file, and one private subdirectory per shim.
pub const RUNTIME_SUBDIR: &str = "vitrin-0";

/// File name of the principal-facing listening socket inside
/// [`RUNTIME_SUBDIR`].
pub const CORE_SOCKET_NAME: &str = "core.sock";

/// File name of a shim's private, app-facing Wayland socket inside its
/// [`shim_runtime_dir_in`] directory -- the value the core puts in the
/// app's `WAYLAND_DISPLAY` (as an absolute path) at spawn time.
pub const SHIM_SOCKET_NAME: &str = "wayland-0";

/// Why a convention path could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// `$XDG_RUNTIME_DIR` is unset (or empty).
    RuntimeDirUnset,
    /// `$XDG_RUNTIME_DIR` is set but not an absolute path, which the XDG
    /// base-directory spec requires; a relative runtime dir would make the
    /// socket location depend on the caller's cwd.
    RuntimeDirNotAbsolute(PathBuf),
    /// The realm id is empty, longer than 64 bytes, `.`/`..`, or contains
    /// a character outside `[A-Za-z0-9._-]` -- rejected so a realm id can
    /// never traverse out of the runtime tree.
    InvalidRealmId(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::RuntimeDirUnset => write!(f, "$XDG_RUNTIME_DIR is not set"),
            PathError::RuntimeDirNotAbsolute(p) => {
                write!(
                    f,
                    "$XDG_RUNTIME_DIR is not an absolute path: {}",
                    p.display()
                )
            }
            PathError::InvalidRealmId(id) => write!(f, "invalid realm id: {id:?}"),
        }
    }
}

impl std::error::Error for PathError {}

/// `$XDG_RUNTIME_DIR/vitrin-0`.
pub fn runtime_dir() -> Result<PathBuf, PathError> {
    Ok(runtime_dir_in(&xdg_runtime_dir()?))
}

/// [`runtime_dir`] against an explicit XDG runtime base.
pub fn runtime_dir_in(xdg_runtime_dir: &Path) -> PathBuf {
    xdg_runtime_dir.join(RUNTIME_SUBDIR)
}

/// `$XDG_RUNTIME_DIR/vitrin-0/core.sock` -- where
/// [`Listener::bind`](crate::Listener::bind) puts the principal-facing
/// listener and agent SDKs connect.
pub fn core_socket_path() -> Result<PathBuf, PathError> {
    Ok(core_socket_path_in(&xdg_runtime_dir()?))
}

/// [`core_socket_path`] against an explicit XDG runtime base.
pub fn core_socket_path_in(xdg_runtime_dir: &Path) -> PathBuf {
    runtime_dir_in(xdg_runtime_dir).join(CORE_SOCKET_NAME)
}

/// `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>` -- the private runtime directory
/// the core creates for one spawned shim (P1.5.2 passes it at fork).
pub fn shim_runtime_dir(realm_id: &str) -> Result<PathBuf, PathError> {
    shim_runtime_dir_in(&xdg_runtime_dir()?, realm_id)
}

/// [`shim_runtime_dir`] against an explicit XDG runtime base.
pub fn shim_runtime_dir_in(xdg_runtime_dir: &Path, realm_id: &str) -> Result<PathBuf, PathError> {
    validate_realm_id(realm_id)?;
    Ok(runtime_dir_in(xdg_runtime_dir).join(realm_id))
}

/// `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>/wayland-0` -- the shim's private,
/// app-facing Wayland socket.
pub fn shim_socket_path(realm_id: &str) -> Result<PathBuf, PathError> {
    shim_socket_path_in(&xdg_runtime_dir()?, realm_id)
}

/// [`shim_socket_path`] against an explicit XDG runtime base.
pub fn shim_socket_path_in(xdg_runtime_dir: &Path, realm_id: &str) -> Result<PathBuf, PathError> {
    Ok(shim_runtime_dir_in(xdg_runtime_dir, realm_id)?.join(SHIM_SOCKET_NAME))
}

fn xdg_runtime_dir() -> Result<PathBuf, PathError> {
    let dir = env::var_os("XDG_RUNTIME_DIR").ok_or(PathError::RuntimeDirUnset)?;
    if dir.is_empty() {
        return Err(PathError::RuntimeDirUnset);
    }
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(PathError::RuntimeDirNotAbsolute(dir));
    }
    Ok(dir)
}

/// Realm ids come from the core's own realm manager (`realm-0` in v0), but
/// they are validated anyway so no code path can ever turn an id into a
/// path escape: nonempty, at most 64 bytes, only `[A-Za-z0-9._-]`, and not
/// `.` or `..`.
fn validate_realm_id(realm_id: &str) -> Result<(), PathError> {
    let valid = !realm_id.is_empty()
        && realm_id.len() <= 64
        && realm_id != "."
        && realm_id != ".."
        && realm_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if valid {
        Ok(())
    } else {
        Err(PathError::InvalidRealmId(realm_id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convention_paths() {
        let base = Path::new("/run/user/1000");
        assert_eq!(runtime_dir_in(base), Path::new("/run/user/1000/vitrin-0"));
        assert_eq!(
            core_socket_path_in(base),
            Path::new("/run/user/1000/vitrin-0/core.sock")
        );
        assert_eq!(
            shim_runtime_dir_in(base, "realm-0").unwrap(),
            Path::new("/run/user/1000/vitrin-0/realm-0")
        );
        assert_eq!(
            shim_socket_path_in(base, "realm-0").unwrap(),
            Path::new("/run/user/1000/vitrin-0/realm-0/wayland-0")
        );
    }

    #[test]
    fn hostile_realm_ids_rejected() {
        let base = Path::new("/run/user/1000");
        for id in [
            "",
            ".",
            "..",
            "../evil",
            "a/b",
            "a\0b",
            "é",
            &"x".repeat(65),
        ] {
            assert_eq!(
                shim_runtime_dir_in(base, id),
                Err(PathError::InvalidRealmId(id.to_string())),
                "realm id {id:?} must be rejected"
            );
        }
        // Dots inside an id are fine; only the traversal names are not.
        shim_runtime_dir_in(base, "realm.0").unwrap();
    }

    #[test]
    fn env_reading_helpers_follow_xdg_runtime_dir() {
        // The only test in the crate that touches process-global env state;
        // no other test (or library code path) reads XDG_RUNTIME_DIR, so
        // there is nothing to race with under the parallel test harness.
        env::set_var("XDG_RUNTIME_DIR", "/run/user/4242");
        assert_eq!(
            core_socket_path().unwrap(),
            Path::new("/run/user/4242/vitrin-0/core.sock")
        );
        assert_eq!(
            shim_socket_path("realm-0").unwrap(),
            Path::new("/run/user/4242/vitrin-0/realm-0/wayland-0")
        );

        env::set_var("XDG_RUNTIME_DIR", "relative/dir");
        assert_eq!(
            core_socket_path(),
            Err(PathError::RuntimeDirNotAbsolute(PathBuf::from(
                "relative/dir"
            )))
        );

        env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(core_socket_path(), Err(PathError::RuntimeDirUnset));
    }
}
