// SPDX-License-Identifier: MPL-2.0
//! The wire shape `vitrind` and `vitrin-realm-init` share (P2.6.2, issue
//! #186, D-036), and nothing else.
//!
//! Both sides import these types. A hand-transcribed second copy of a frame
//! layout is exactly the cost D-022 records for the Python SDK's enum, and
//! this channel is the one the realm's confinement is negotiated over, so a
//! silent divergence between the two spellings would be a divergence between
//! what the core believes it asked for and what the helper built.
//!
//! # Why a helper binary exists at all
//!
//! `unshare(2)` mutates the caller. The core may not confine itself as a side
//! effect of confining a realm, so the namespaces have to be created by
//! *some* other process. The choice of **which** other process is D-036(1),
//! and it is the inverse of the shape the plan row implied:
//!
//! - **`execve` first, `unshare` second.** The core `spawn`s this binary with
//!   `std::process::Command` -- the ordinary path, with the existing
//!   `pre_exec` closure unchanged -- and *this* process unshares itself once
//!   it is running. Cloning into namespaces from the core is not implementable
//!   while the core keeps `Command`, and the reason is a deadlock rather than
//!   a preference: D-020(1) requires the parent to write the id maps while the
//!   child blocks, and std's own fork path has the parent blocked reading its
//!   exec-report pipe until `execve` closes it.
//! - **It is also the stronger posture, which is why it is a decision and not
//!   an implementation detail.** `execve` recomputes capabilities. A process
//!   that unshared a user namespace with no map written has an unmapped uid,
//!   so it is not namespace-root and would leave `execve` with an empty
//!   permitted set -- which forces the map to be written *before* that
//!   `execve`, and forces it to be `0 <euid> 1`, handing the confined app
//!   `CAP_SYS_ADMIN` inside its own user namespace. Unsharing *after* this
//!   binary's `execve` avoids both: no `execve` intervenes between `unshare`
//!   and the mount work, so this process keeps its namespace-local
//!   capabilities, and the map can be an **identity** map. The app therefore
//!   ends up with **zero capabilities** and cannot reshape its own
//!   confinement.
//! - **And it deletes a window.** No shape where a child blocks inside the
//!   fork window leaves a copy-on-write image of the core's address space --
//!   the clipboard slot, the grant table, principal keys -- sitting in a
//!   not-yet-`exec`'d process readable through `/proc/<pid>/mem` by any
//!   same-uid process.
//!
//! # Why there are two processes on this side of the fork
//!
//! `unshare(CLONE_NEWPID)` does not move the caller. It sets
//! `pid_ns_for_children`, so the *next* `fork` produces PID 1 -- measured on
//! this kernel, not read: after `unshare(CLONE_NEWPID)` the caller's own
//! `/proc/self/ns/pid` is still the core's. So this binary forks:
//!
//! - the **supervisor** stays in the host PID namespace and is the process
//!   the core's [`std::process::Child`] names;
//! - the **PID-1 child** builds the mount table and `execve`s the shim.
//!
//! That fork is not merely forced, it pays for itself: with
//! `PR_SET_PDEATHSIG = SIGKILL` on PID 1 and the kernel's rule that a PID
//! namespace dies with its init, the orphan residual `vitrin-core`'s
//! `lifecycle` module used to publish -- "rung 3 kills the shim and its app
//! is reparented to init" -- is closed. A fresh `proc` mount also requires
//! the mounter to be in a new PID namespace, so the mount table could not be
//! built before the fork in any case.
//!
//! ## One correction to the record, because a false reason is worse than none
//!
//! The verification the core performs never reads
//! `/proc/<pid>/ns/pid_for_children`, and an earlier draft of this design
//! justified that by citing `proc(5)`'s statement that the file reads back
//! empty until the first child exists. **That is not what this kernel does**
//! (measured on 7.1.8: the link is populated immediately after the
//! `unshare`). The justification is therefore *not* "it would be empty". It
//! is that `pid_for_children` describes a namespace the reader is not in, so
//! a difference there proves only that a namespace was *requested*; the
//! PID-1 child's own `/proc/<host pid>/ns/pid` is the unambiguous fact, and
//! it is the one the core reads.
//!
//! # The invariant this channel exists to serve
//!
//! > `applied` is computed from what the kernel says about the child, never
//! > from the flag that was requested, the path that was resolved, or the
//! > table that was sent.
//!
//! Nothing in the frames below is evidence of confinement. [`Frame::Mounted`]
//! carries a count and a fingerprint the *child* computed, and the core
//! journals both as `child-asserted` for exactly that reason. What licenses
//! the spawn is the core's own reads of `/proc/<pid>/ns/*`,
//! `/proc/<pid>/root` and the canary set -- see `vitrin_core::spawn`.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the core will hold the compositor waiting for this helper to
/// report a built realm, end to end (D-036(11)).
///
/// **250 ms, and the number is not free.** Since WS-E.1.1 `launch` is
/// reachable from an admitted `realm_launch`, so a wire-reachable verb now
/// blocks the compositor for the length of this handshake. `lifecycle` draws
/// the line explicitly for the *death* path ("nothing in the death-detection
/// path blocks or sleeps, so a dying realm can never park the compositor");
/// the birth path is bounded here instead, at exactly the shortest dead-man
/// hold the off-switch will accept -- so a realm being born can never
/// out-wait the switch that would kill it. `vitrin_core::spawn` holds that
/// equality with a compile-time assertion against `deadman::MIN_HOLD`; this
/// constant may not be raised without moving that floor first.
pub const HELPER_DEADLINE: Duration = Duration::from_millis(250);

/// The realm's private runtime directory, *inside* the realm: a bind of the
/// host directory the core created, purged and `flock`ed before the fork.
///
/// Short and fixed on purpose. It removes the `sun_path` 108-byte pressure a
/// long realm id creates under the host spelling, and it makes the app's
/// `XDG_RUNTIME_DIR` a constant rather than a path that leaks the host tree's
/// shape.
pub const IN_REALM_RUNTIME_DIR: &str = "/run/vitrin";

/// The app-facing Wayland socket, inside the realm. `wl_display_add_socket`
/// takes an absolute name verbatim, so the shim binds exactly this.
pub const IN_REALM_WAYLAND_SOCKET: &str = "/run/vitrin/wayland-0";

/// **Reserved** (D-020(4)): the path P2.1.3's per-realm accessibility bus
/// daemon will bind, and the only thing that may ever be bound there.
///
/// It is a name plus an ordering, not a mount. The directory it sits in is
/// the core-created realm runtime directory bound at
/// [`IN_REALM_RUNTIME_DIR`], so P2.1.3 adds **no bind mount** and re-makes
/// **no** confinement claim -- which is what the reservation was written for.
///
/// The residual is published rather than papered over: within one realm the
/// app can `unlink` and rebind any socket in [`IN_REALM_RUNTIME_DIR`],
/// including `wayland-0` and this path. Mode bits cannot stop it (the shim
/// and the app are one uid inside a single-id map), the blast radius is that
/// one realm, and the closure is P2.6.3's shim-side Landlock stack.
pub const IN_REALM_A11Y_BUS: &str = "/run/vitrin/a11y-bus";

/// The directory the core owns inside the realm's root: the shim binary and
/// the realm's private storage hang off it.
///
/// A single core-chosen prefix rather than two unrelated paths, so "what did
/// the core put in this realm's filesystem" has a one-line answer an operator
/// can check with `ls`.
pub const IN_REALM_PREFIX: &str = "/vitrin";

/// The shim binary, inside the realm -- a read-only, `nosuid`, `nodev` file
/// bind of the host shim the core audited.
///
/// It has to be bound at all because in a development tree the shim lives in
/// `target/debug` under `$HOME`, which is the exact tree this task exists to
/// hide from the realm.
pub const IN_REALM_SHIM: &str = "/vitrin/shim";

/// The realm's private storage, inside the realm, and the value of its
/// `HOME`.
///
/// Disk-backed and realm-writable. Two published costs travel with it: it
/// carries **no quota** (a project quota needs `prjquota` on the filesystem
/// and is out of scope), and it is keyed on realm id and never purged, so a
/// realm whose `command` changes inherits the old app's `HOME`.
pub const IN_REALM_HOME: &str = "/vitrin/home";

/// Fixed `size=` caps for the four tmpfs mounts a realm gets.
///
/// # The basis, written down because an unexplained cap is a cap nobody can
/// revise
///
/// Every tmpfs in the table carries an explicit `size=`. This is not
/// tidiness: an unsized tmpfs defaults to **half of RAM**, and OOM-killing
/// `vitrind` under session mode takes the display with it. The four numbers
/// below are *reasoned*, not measured, and they are flagged here as revisable
/// the moment a measurement exists.
///
/// - **`root` = 4 MiB.** The new root holds mount points (empty directories)
///   and a handful of symlinks, and it is remounted read-only before the app
///   exists, so nothing can grow it after bring-up. The number is deliberately
///   ~1000x what the table needs rather than as small as possible, so adding a
///   row later cannot turn into an `ENOSPC` nobody can read.
/// - **`dev` = 1 MiB.** Bind targets (empty files) and one symlink. Device
///   nodes are file binds, not `mknod` -- a `mknod` of a device node is
///   impossible in a non-initial user namespace -- so nothing here consumes
///   data blocks.
/// - **`shm` = 64 MiB.** The only cap an app legitimately fills, and therefore
///   the one a measurement should move first. Sized from one full-screen
///   double-buffered `wl_shm` pool at 4K (3840x2160x4 = 31.6 MiB, twice), which
///   is an upper bound on the shape rather than an observation of any real
///   client.
/// - **`tmp` = 64 MiB.** App scratch. Chosen equal to `shm` so an operator has
///   two numbers to reason about rather than four.
///
/// **The number that matters is the product.** 133 MiB of pageable memory per
/// realm, and `MAX_REALMS` is 16, so a fully populated session can pin 2.1 GiB
/// in tmpfs. That total, not any single row, is what an operator should read
/// before raising a cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmpfsCaps {
    pub root: u64,
    pub dev: u64,
    pub shm: u64,
    pub tmp: u64,
}

impl TmpfsCaps {
    /// The caps argued in this type's docs.
    pub const DEFAULT: TmpfsCaps = TmpfsCaps {
        root: 4 * 1024 * 1024,
        dev: 1024 * 1024,
        shm: 64 * 1024 * 1024,
        tmp: 64 * 1024 * 1024,
    };
}

/// The largest `CONFIG` blob the helper will accept, and the receive buffer
/// both sides size to.
///
/// Bounded rather than streamed because the helper reads this before it has
/// unshared anything: a peer that could make it allocate without limit would
/// be a denial of service against the core's own spawn path. 64 KiB is far
/// past the largest plausible table (a few dozen paths).
pub const CONFIG_MAX: usize = 64 * 1024;

/// The fixed size of every non-`CONFIG` frame.
pub const FRAME_LEN: usize = 32;

/// The exit status this binary uses for a failure that happened **before**
/// the shim's `execve`.
///
/// Reserved rather than arbitrary: after `execve` succeeds the supervisor
/// mirrors the shim's exit exactly and invents no code of its own, so a `125`
/// can only ever be observed on a spawn the core already refused. `125` is
/// the `env(1)`/`nice(1)` convention for "the wrapper failed, not the
/// program".
pub const PRE_EXEC_EXIT: i32 = 125;

// Tags. Two disjoint spaces, so a frame decoded on the wrong side is a
// protocol error rather than a plausible-looking value.
const TAG_CONFIG: u8 = 0x10;
const TAG_MAP_DONE: u8 = 0x11;
const TAG_PROCEED: u8 = 0x12;
const TAG_UNSHARED: u8 = 0x20;
const TAG_CHILD: u8 = 0x21;
const TAG_MOUNTED: u8 = 0x22;
const TAG_FAIL: u8 = 0x23;

/// Where in the sequence the helper gave up.
///
/// A closed vocabulary, drawn on the `cause_class` convention
/// `vitrin_core::spawn` already holds: the core maps each of these to a fixed
/// label a reader can switch on, never to free-form text the helper composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// The `CONFIG` blob did not decode, or its `schema_version` did not
    /// match this binary's. A stale helper beside a new `vitrind` is refused,
    /// never silently executed.
    Version = 1,
    /// The single six-flag `unshare` failed.
    Unshare = 2,
    /// Reserved, and **unreachable in this build**.
    ///
    /// It was to be `setgroups(0, NULL)`'s refusal. That call turns out to be
    /// impossible for an unprivileged realm in *either* window -- see
    /// `main.rs`'s C3 comment for the two kernel predicates and the
    /// measurement -- so nothing sends this stage. The variant is kept
    /// rather than deleted so the wire numbering is stable and so a future
    /// per-uid tier, where the drop does become reachable, has its label
    /// already agreed.
    Setgroups = 3,
    /// The child's own read-back of `uid_map` / `gid_map` / `setgroups`
    /// disagreed with what the core said it wrote.
    MapVerify = 4,
    /// The supervisor/PID-1 fork failed.
    Fork = 5,
    /// A mount-table entry failed.
    Mount = 6,
    /// `pivot_root` (or its `umount2` tail) failed.
    PivotRoot = 7,
    /// The shim's `execve` failed. This frame is the *only* way the core
    /// learns that, because a successful `execve` is reported by EOF.
    Exec = 8,
    /// Anything else this binary refuses to continue past.
    Internal = 9,
}

impl Stage {
    fn from_u8(v: u8) -> Option<Stage> {
        Some(match v {
            1 => Stage::Version,
            2 => Stage::Unshare,
            3 => Stage::Setgroups,
            4 => Stage::MapVerify,
            5 => Stage::Fork,
            6 => Stage::Mount,
            7 => Stage::PivotRoot,
            8 => Stage::Exec,
            9 => Stage::Internal,
            _ => return None,
        })
    }
}

/// One message on the config channel.
///
/// `SOCK_SEQPACKET`, so a frame is a datagram and there is no framing to get
/// wrong. **There is no `READY` frame**: EOF on this channel is the success
/// signal for the shim's `execve`, because the channel descriptor is
/// `FD_CLOEXEC` and `FD_CLOEXEC` takes effect only on a *successful* `execve`
/// -- the same trick std uses for its own exec-report pipe. A `READY` sent
/// before `execve` would report success for a shim that then failed to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// core -> helper. The whole realm description, once.
    Config(Box<Config>),
    /// core -> helper. `setgroups`, `uid_map` and `gid_map` are written *and
    /// read back*; the child may proceed to verify them itself.
    MapDone,
    /// core -> helper. The core's own reads of `/proc/<pid>/ns/*`,
    /// `/proc/<pid>/root` and the canary set all passed. Nothing else
    /// licenses the second `execve`.
    Proceed,
    /// helper -> core. The six-flag `unshare` and the `setgroups(0, NULL)`
    /// both succeeded; the helper is now blocked waiting for its maps.
    Unshared,
    /// helper -> core. The PID-1 child exists, at this **host** pid -- the
    /// number the core addresses `/proc` by.
    Child { host_pid: i32 },
    /// helper -> core, **child-asserted**. What the PID-1 child's own
    /// post-pivot `/proc/self/mountinfo` said. Journaled as an assertion, not
    /// as evidence: a substituted helper could send anything, which is why the
    /// core's own root-view check exists.
    Mounted { count: u32, fingerprint: u64 },
    /// helper -> core. A refusal, at a named stage, with the kernel's errno.
    Fail { stage: Stage, errno: i32 },
}

/// Why a frame or a config blob did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The datagram was not the length this frame kind requires.
    Length { got: usize },
    /// The tag byte names no frame in this direction.
    Tag(u8),
    /// A length-prefixed field ran past the end of the blob.
    Truncated,
    /// A `Fail` frame named a stage this build does not know.
    Stage(u8),
    /// The blob is larger than [`CONFIG_MAX`].
    TooLarge(usize),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::Length { got } => write!(f, "frame is {got} bytes, not {FRAME_LEN}"),
            CodecError::Tag(t) => write!(f, "unknown frame tag {t:#04x} for this direction"),
            CodecError::Truncated => write!(f, "a length-prefixed field ran past the end"),
            CodecError::Stage(s) => write!(f, "unknown failure stage {s}"),
            CodecError::TooLarge(n) => write!(f, "config blob is {n} bytes, over {CONFIG_MAX}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Everything the helper needs to build one realm, and nothing it could use
/// to decide anything.
///
/// Every path here is one the **core** canonicalized and audited before the
/// fork; the helper resolves no name of its own and reads no environment
/// variable, ever. Environment composition stays at one site in
/// `vitrin_core::spawn::child_env`, and this binary passes its own `environ`
/// through to the shim verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The core's `CARGO_PKG_VERSION`. Compared for **exact** equality
    /// against this binary's own before anything else happens: a `--shim`
    /// from `/usr/lib/vitrin` beside a helper from `target/debug` is a
    /// refusal, not undefined behaviour.
    pub schema_version: String,
    /// The realm's id -- used as the realm's hostname, so `uname -n` inside
    /// the realm names the realm rather than the operator's machine.
    pub realm_id: String,
    /// The identity map's single line: `<inner_uid> <inner_uid> 1`.
    ///
    /// Identity, one fixed shape for every realm (D-036(3)). Namespace-root
    /// (`0 <euid> 1`) was available and is refused: it buys nothing and grants
    /// the app `CAP_SYS_ADMIN` in the realm's own user namespace, from which
    /// it could bind-mount over the read-only view for every process in the
    /// realm.
    pub inner_uid: u32,
    pub inner_gid: u32,
    /// Host source for [`IN_REALM_RUNTIME_DIR`].
    pub runtime_dir: PathBuf,
    /// Host source for [`IN_REALM_HOME`].
    pub storage_dir: PathBuf,
    /// Host source for [`IN_REALM_SHIM`].
    pub shim_source: PathBuf,
    /// The directory holding the app binary, bound at the **same** in-realm
    /// path, when the app is not already covered by `/usr` or `/etc`.
    pub app_dir: Option<PathBuf>,
    /// Render nodes (`/dev/dri/renderD*`) the core enumerated and audited at
    /// startup. Never `card*`, never `controlD*`.
    pub render_nodes: Vec<PathBuf>,
    /// Operator-declared read-only binds from `realm.toml`, canonicalized and
    /// audited at spawn exactly as `command` is.
    pub binds: Vec<PathBuf>,
    /// The full argv of the second `execve`: the in-realm shim path, the
    /// shim's own leading arguments, `--`, then the app command and its args.
    pub argv: Vec<OsString>,
    pub caps: TmpfsCaps,
}

impl Frame {
    /// Encode into a datagram. `CONFIG` is variable-length and bounded;
    /// everything else is exactly [`FRAME_LEN`].
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let fixed = |tag: u8, a: i64, b: u64, stage: u8| {
            let mut buf = vec![0u8; FRAME_LEN];
            buf[0] = tag;
            buf[1] = stage;
            buf[4..8].copy_from_slice(&(a as i32).to_le_bytes());
            buf[8..16].copy_from_slice(&b.to_le_bytes());
            buf
        };
        Ok(match self {
            Frame::Config(cfg) => {
                let mut buf = vec![TAG_CONFIG];
                cfg.encode_into(&mut buf);
                if buf.len() > CONFIG_MAX {
                    return Err(CodecError::TooLarge(buf.len()));
                }
                buf
            }
            Frame::MapDone => fixed(TAG_MAP_DONE, 0, 0, 0),
            Frame::Proceed => fixed(TAG_PROCEED, 0, 0, 0),
            Frame::Unshared => fixed(TAG_UNSHARED, 0, 0, 0),
            Frame::Child { host_pid } => fixed(TAG_CHILD, i64::from(*host_pid), 0, 0),
            Frame::Mounted { count, fingerprint } => {
                fixed(TAG_MOUNTED, i64::from(*count), *fingerprint, 0)
            }
            Frame::Fail { stage, errno } => {
                fixed(TAG_FAIL, i64::from(*errno), 0, *stage as u8)
            }
        })
    }

    /// Decode one datagram.
    pub fn decode(bytes: &[u8]) -> Result<Frame, CodecError> {
        let tag = *bytes.first().ok_or(CodecError::Length { got: 0 })?;
        if tag == TAG_CONFIG {
            if bytes.len() > CONFIG_MAX {
                return Err(CodecError::TooLarge(bytes.len()));
            }
            return Ok(Frame::Config(Box::new(Config::decode(&bytes[1..])?)));
        }
        if bytes.len() != FRAME_LEN {
            return Err(CodecError::Length { got: bytes.len() });
        }
        let a = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let b = u64::from_le_bytes(bytes[8..16].try_into().expect("eight bytes"));
        Ok(match tag {
            TAG_MAP_DONE => Frame::MapDone,
            TAG_PROCEED => Frame::Proceed,
            TAG_UNSHARED => Frame::Unshared,
            TAG_CHILD => Frame::Child { host_pid: a },
            TAG_MOUNTED => Frame::Mounted {
                count: a as u32,
                fingerprint: b,
            },
            TAG_FAIL => Frame::Fail {
                stage: Stage::from_u8(bytes[1]).ok_or(CodecError::Stage(bytes[1]))?,
                errno: a,
            },
            other => return Err(CodecError::Tag(other)),
        })
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    put_u32(out, v.len() as u32);
    out.extend_from_slice(v);
}

fn put_path(out: &mut Vec<u8>, p: &Path) {
    put_bytes(out, p.as_os_str().as_bytes());
}

/// A bounds-checked cursor. Every read either yields a value inside the blob
/// or [`CodecError::Truncated`]; there is no arithmetic on a length the blob
/// did not supply.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, CodecError> {
        let end = self.at.checked_add(4).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        let end = self.at.checked_add(8).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(u64::from_le_bytes(slice.try_into().expect("eight bytes")))
    }

    fn bytes(&mut self) -> Result<&'a [u8], CodecError> {
        let len = self.u32()? as usize;
        let end = self.at.checked_add(len).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn string(&mut self) -> Result<String, CodecError> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    fn os_string(&mut self) -> Result<OsString, CodecError> {
        Ok(OsString::from_vec(self.bytes()?.to_vec()))
    }

    fn path(&mut self) -> Result<PathBuf, CodecError> {
        Ok(PathBuf::from(OsStr::from_bytes(self.bytes()?)))
    }

    fn paths(&mut self) -> Result<Vec<PathBuf>, CodecError> {
        let n = self.u32()? as usize;
        // Bounded by the blob's own remaining length: an attacker-supplied
        // count cannot make this allocate more than the bytes that exist.
        let mut out = Vec::with_capacity(n.min(self.bytes.len()));
        for _ in 0..n {
            out.push(self.path()?);
        }
        Ok(out)
    }
}

impl Config {
    fn encode_into(&self, out: &mut Vec<u8>) {
        put_bytes(out, self.schema_version.as_bytes());
        put_bytes(out, self.realm_id.as_bytes());
        put_u32(out, self.inner_uid);
        put_u32(out, self.inner_gid);
        put_path(out, &self.runtime_dir);
        put_path(out, &self.storage_dir);
        put_path(out, &self.shim_source);
        match &self.app_dir {
            Some(p) => {
                put_u32(out, 1);
                put_path(out, p);
            }
            None => put_u32(out, 0),
        }
        put_u32(out, self.render_nodes.len() as u32);
        for p in &self.render_nodes {
            put_path(out, p);
        }
        put_u32(out, self.binds.len() as u32);
        for p in &self.binds {
            put_path(out, p);
        }
        put_u32(out, self.argv.len() as u32);
        for a in &self.argv {
            put_bytes(out, a.as_bytes());
        }
        put_u64(out, self.caps.root);
        put_u64(out, self.caps.dev);
        put_u64(out, self.caps.shm);
        put_u64(out, self.caps.tmp);
    }

    fn decode(bytes: &[u8]) -> Result<Config, CodecError> {
        let mut c = Cursor { bytes, at: 0 };
        let schema_version = c.string()?;
        let realm_id = c.string()?;
        let inner_uid = c.u32()?;
        let inner_gid = c.u32()?;
        let runtime_dir = c.path()?;
        let storage_dir = c.path()?;
        let shim_source = c.path()?;
        let app_dir = match c.u32()? {
            0 => None,
            _ => Some(c.path()?),
        };
        let render_nodes = c.paths()?;
        let binds = c.paths()?;
        let argv_len = c.u32()? as usize;
        let mut argv = Vec::with_capacity(argv_len.min(bytes.len()));
        for _ in 0..argv_len {
            argv.push(c.os_string()?);
        }
        let caps = TmpfsCaps {
            root: c.u64()?,
            dev: c.u64()?,
            shm: c.u64()?,
            tmp: c.u64()?,
        };
        Ok(Config {
            schema_version,
            realm_id,
            inner_uid,
            inner_gid,
            runtime_dir,
            storage_dir,
            shim_source,
            app_dir,
            render_nodes,
            binds,
            argv,
            caps,
        })
    }
}

/// FNV-1a, 64-bit, over the child's own post-pivot `mountinfo`.
///
/// **Deliberately not a cryptographic digest, and named as one would not be.**
/// The value is child-asserted: a substituted helper can send any number it
/// likes, so collision resistance would buy nothing that the core's own
/// root-view check does not already provide. What it *is* for is noticing that
/// the realm's mount table changed shape between two runs of the same build --
/// a job a checksum does, and calling it a "digest" beside the recorder's
/// blake3 capture digests would overclaim.
pub fn mount_fingerprint(mountinfo: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in mountinfo {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The algorithm name the flight recorder prints beside a fingerprint, so a
/// reader never has to guess which family the number is from.
pub const MOUNT_FINGERPRINT_ALG: &str = "fnv1a-64";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Config {
        Config {
            schema_version: "0.1.0".into(),
            realm_id: "realm-0".into(),
            inner_uid: 1000,
            inner_gid: 1000,
            runtime_dir: PathBuf::from("/run/user/1000/vitrin-0/realm-0"),
            storage_dir: PathBuf::from("/home/op/.local/share/vitrin/realms/realm-0"),
            shim_source: PathBuf::from("/usr/lib/vitrin/vitrin-shim"),
            app_dir: Some(PathBuf::from("/opt/app/bin")),
            render_nodes: vec![PathBuf::from("/dev/dri/renderD128")],
            binds: vec![PathBuf::from("/nix"), PathBuf::from("/opt/data")],
            argv: vec![
                OsString::from(IN_REALM_SHIM),
                OsString::from("--"),
                OsString::from("/opt/app/bin/app"),
                OsString::from("--flag"),
            ],
            caps: TmpfsCaps::DEFAULT,
        }
    }

    #[test]
    fn the_config_blob_round_trips() {
        // Both sides import this type, so a divergent hand-transcribed copy
        // cannot exist -- but the codec itself still has to be an identity.
        let cfg = sample_config();
        let frame = Frame::Config(Box::new(cfg.clone()));
        let bytes = frame.encode().expect("encodes");
        assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
    }

    #[test]
    fn a_config_with_no_app_dir_and_no_binds_round_trips() {
        // The `Option`/empty-vector edges, which is where a hand-rolled codec
        // gets its off-by-one.
        let mut cfg = sample_config();
        cfg.app_dir = None;
        cfg.binds.clear();
        cfg.render_nodes.clear();
        let frame = Frame::Config(Box::new(cfg));
        let bytes = frame.encode().expect("encodes");
        assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
    }

    #[test]
    fn every_fixed_frame_round_trips_at_exactly_one_length() {
        for frame in [
            Frame::MapDone,
            Frame::Proceed,
            Frame::Unshared,
            Frame::Child { host_pid: 424_242 },
            Frame::Mounted {
                count: 21,
                fingerprint: 0xdead_beef_cafe_f00d,
            },
            Frame::Fail {
                stage: Stage::Unshare,
                errno: libc::EPERM,
            },
        ] {
            let bytes = frame.encode().expect("encodes");
            assert_eq!(bytes.len(), FRAME_LEN, "{frame:?} is not a fixed frame");
            assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
        }
    }

    #[test]
    fn every_stage_survives_the_wire() {
        // A `Fail` whose stage did not decode would be reported to the core as
        // a protocol error instead of the refusal it is, which would hide the
        // real cause behind a generic one.
        for stage in [
            Stage::Version,
            Stage::Unshare,
            Stage::Setgroups,
            Stage::MapVerify,
            Stage::Fork,
            Stage::Mount,
            Stage::PivotRoot,
            Stage::Exec,
            Stage::Internal,
        ] {
            let frame = Frame::Fail { stage, errno: 13 };
            let bytes = frame.encode().expect("encodes");
            assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
        }
    }

    #[test]
    fn a_truncated_blob_is_an_error_and_never_a_partial_config() {
        let bytes = Frame::Config(Box::new(sample_config()))
            .encode()
            .expect("encodes");
        for cut in [1usize, 5, 17, bytes.len() - 1] {
            assert!(
                matches!(Frame::decode(&bytes[..cut]), Err(CodecError::Truncated)),
                "a blob cut at {cut} decoded to something"
            );
        }
    }

    #[test]
    fn a_wrong_length_fixed_frame_is_refused() {
        let mut bytes = Frame::Unshared.encode().expect("encodes");
        bytes.push(0);
        assert_eq!(
            Frame::decode(&bytes),
            Err(CodecError::Length { got: FRAME_LEN + 1 })
        );
        assert_eq!(Frame::decode(&[]), Err(CodecError::Length { got: 0 }));
    }

    #[test]
    fn an_unknown_tag_is_refused_rather_than_guessed() {
        let mut bytes = vec![0u8; FRAME_LEN];
        bytes[0] = 0x77;
        assert_eq!(Frame::decode(&bytes), Err(CodecError::Tag(0x77)));
    }

    #[test]
    fn an_oversized_config_is_refused_at_both_ends() {
        let mut cfg = sample_config();
        // One path far past the cap: the encoder must refuse rather than hand
        // the helper a datagram `sendmsg` would truncate.
        cfg.binds = vec![PathBuf::from("/".to_string() + &"a".repeat(CONFIG_MAX))];
        assert!(matches!(
            Frame::Config(Box::new(cfg)).encode(),
            Err(CodecError::TooLarge(_))
        ));
        let oversized = vec![TAG_CONFIG; CONFIG_MAX + 1];
        assert!(matches!(
            Frame::decode(&oversized),
            Err(CodecError::TooLarge(_))
        ));
    }

    #[test]
    fn the_helper_deadline_is_the_shortest_dead_man_hold() {
        // The core holds the *equality* with a compile-time assertion against
        // `deadman::MIN_HOLD`, which is private to that crate. This is the
        // half that can be stated here: the number, so a change to it shows up
        // as a failing test in this crate too rather than only in the core's.
        assert_eq!(HELPER_DEADLINE, Duration::from_millis(250));
    }

    #[test]
    fn the_fingerprint_is_a_function_of_the_bytes_and_changes_with_them() {
        assert_eq!(mount_fingerprint(b"a b c"), mount_fingerprint(b"a b c"));
        assert_ne!(mount_fingerprint(b"a b c"), mount_fingerprint(b"a b d"));
        // Non-vacuity for the loop body: an implementation that ignored its
        // input would pass the equality above and fail this.
        assert_ne!(mount_fingerprint(b""), mount_fingerprint(b"\0"));
    }

    #[test]
    fn the_reserved_bus_path_is_inside_the_realms_runtime_directory() {
        // D-020(4)'s reservation is a name plus an ordering: it is only "no
        // new mount for P2.1.3" if the path lives in a mount that already
        // exists.
        assert!(IN_REALM_A11Y_BUS.starts_with(IN_REALM_RUNTIME_DIR));
        assert!(IN_REALM_WAYLAND_SOCKET.starts_with(IN_REALM_RUNTIME_DIR));
        assert_ne!(IN_REALM_A11Y_BUS, IN_REALM_WAYLAND_SOCKET);
    }

    #[test]
    fn the_core_owned_prefix_holds_the_shim_and_the_storage() {
        assert!(IN_REALM_SHIM.starts_with(IN_REALM_PREFIX));
        assert!(IN_REALM_HOME.starts_with(IN_REALM_PREFIX));
    }
}
