// SPDX-License-Identifier: MPL-2.0
//! The lock screen's passphrase file (WS-E.2.2, issue #214): a stored Argon2id
//! digest the core checks a typed attempt against, and nothing else.
//!
//! # What is stored, and what is deliberately not
//!
//! One line, in a format this module both writes (`vitrind --lock-hash`) and
//! reads, and which nothing else in the tree parses:
//!
//! ```text
//! vitrin-lock-v1 argon2id m=19456 t=2 p=1 <32 hex salt> <64 hex digest>
//! ```
//!
//! **The passphrase itself is never stored, never logged, never journaled and
//! never rendered.** It exists in this process only as the `Vec<u8>` a human is
//! currently typing ([`crate::lock::LockScreen`]), which is wiped on every path
//! that ends an attempt. Nothing in this module has a `Debug` impl that can
//! print key material: [`PassphraseFile`] prints its *parameters* and the fact
//! that it holds a digest, and never a byte of either the salt or the digest —
//! a salt is not a secret, but a `Debug` that prints one is a `Debug` somebody
//! extends.
//!
//! # Why a real KDF, and why Argon2id
//!
//! Anyone who can read this file can attack it offline, at their own pace, on
//! hardware the core does not choose. The only defence is making each guess
//! expensive, and the only defence that survives a GPU is making it expensive in
//! **memory**. That rules out the zero-dependency option (BLAKE3 of the
//! passphrase, using a crate already in the tree), which would be a single fast
//! hash and is exactly the design every password-storage postmortem is about.
//! Argon2id is RFC 9106's recommendation and OWASP's default; the crate cost is
//! four crates, measured and justified in `crates/vitrin-core/Cargo.toml`.
//!
//! **The KDF identity is pinned in the file, not in the dependency.** The
//! `argon2id` token is refused as anything else, so a `cargo update` cannot
//! silently change what a stored digest means. The *parameters* travel in the
//! file instead of being compiled in, so an operator can raise them on hardware
//! that can afford it without a rebuild — bounded ([`MAX_MEMORY_KIB`],
//! [`MAX_TIME_COST`], [`MAX_PARALLELISM`]) so a mistyped `m=` cannot turn a
//! session start into an out-of-memory kill.
//!
//! # The file's own permissions are stricter than `realm.toml`'s, on purpose
//!
//! [`crate::realm`]'s config audit refuses a file that is group/other
//! **writable**, because whoever can write it chooses what the core executes.
//! This file is refused if it is group/other *readable or* writable, because
//! whoever can read it gets an offline attack on the human's session
//! passphrase. That is the same posture `ssh` takes on a private key, and it is
//! stated here because it is a **stricter** rule than the one next door and a
//! reader would otherwise assume they match.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use subtle::ConstantTimeEq;

/// The format's first token. Bumped only if the line's shape changes; the KDF
/// moving is a *different* token, so the two cannot be confused.
const MAGIC: &str = "vitrin-lock-v1";

/// The KDF token. Exactly one value is accepted, and it is checked rather than
/// parsed into an enum: this core hashes with Argon2id and refuses to guess
/// what a file naming something else expected.
const KDF: &str = "argon2id";

/// Salt length in bytes (RFC 9106's recommendation, and what
/// [`Params::DEFAULT_SALT_LENGTH`] would be).
const SALT_LEN: usize = 16;

/// Derived-key length in bytes.
const DIGEST_LEN: usize = 32;

/// Default memory cost in KiB — 19 MiB, OWASP's "second recommended"
/// Argon2id configuration (m=19456, t=2, p=1).
///
/// Chosen over the first (m=47104, t=1) because verification runs **inside the
/// compositor's dispatch turn**: one attempt is one blocking derivation, and a
/// smaller working set with one more pass keeps the stall closer to a frame
/// than to a stutter a human would read as a hang. An operator who wants the
/// stronger profile writes it into the file; nothing here has to change.
const DEFAULT_MEMORY_KIB: u32 = 19_456;
const DEFAULT_TIME_COST: u32 = 2;
const DEFAULT_PARALLELISM: u32 = 1;

/// Upper bounds on what a file may ask for. The file is operator-owned and
/// mode-audited, so this is not an attacker bound — it is the bound that keeps
/// a typo (`m=194560000`) from turning "the session starts" into "the session
/// is OOM-killed at the first unlock attempt", which is a fail-*open* outcome
/// dressed as caution.
const MAX_MEMORY_KIB: u32 = 1_048_576;
const MAX_TIME_COST: u32 = 10;
const MAX_PARALLELISM: u32 = 4;

/// Minimums, from Argon2's own definition — below these the KDF is not the
/// thing whose name is in the file.
const MIN_MEMORY_KIB: u32 = 8;
const MIN_TIME_COST: u32 = 1;

/// Longest passphrase a human may type at the lock screen, in bytes.
///
/// Bounded because the buffer lives in the trusted core and grows on physical
/// input the core is consuming anyway; 512 bytes is far past any passphrase and
/// far short of anything worth calling a leak. A press past the cap is dropped
/// silently rather than truncating the attempt at submit time — a passphrase
/// that silently lost its tail would fail with no explanation a human could act
/// on, and this way the attempt they submitted is the attempt they typed up to
/// the cap.
pub(crate) const MAX_ATTEMPT_BYTES: usize = 512;

/// Why a passphrase file was refused.
///
/// Every variant is a **startup** refusal: a session that comes up holding an
/// unusable lock passphrase is the fail-open configuration trap
/// [`crate::realm`] and [`crate::deadman`] both refuse, one gesture further out.
#[derive(Debug)]
pub(crate) enum PassphraseError {
    /// The path is not absolute. Same rule `realm.toml`'s `command` is held to,
    /// for the same reason: a relative path resolves against a working
    /// directory nobody wrote down.
    NotAbsolute(PathBuf),
    Io(PathBuf, std::io::Error),
    /// Not a regular file, not owned by the core's uid, or reachable by
    /// somebody else (module docs).
    Insecure(PathBuf, String),
    /// Well-formed file, malformed contents.
    Malformed(PathBuf, &'static str),
}

impl std::fmt::Display for PassphraseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PassphraseError::NotAbsolute(p) => write!(
                f,
                "lock passphrase file {} is not an absolute path",
                p.display()
            ),
            PassphraseError::Io(p, e) => {
                write!(f, "lock passphrase file {}: i/o error: {e}", p.display())
            }
            PassphraseError::Insecure(p, detail) => {
                write!(f, "lock passphrase file {}: refused: {detail}", p.display())
            }
            PassphraseError::Malformed(p, detail) => {
                write!(f, "lock passphrase file {}: invalid: {detail}", p.display())
            }
        }
    }
}

impl std::error::Error for PassphraseError {}

/// A loaded passphrase file: the KDF parameters and the digest an attempt is
/// checked against.
///
/// No `Debug` derive — see [`Self::fmt`] below for what it prints instead.
pub(crate) struct PassphraseFile {
    memory_kib: u32,
    time_cost: u32,
    parallelism: u32,
    salt: [u8; SALT_LEN],
    digest: [u8; DIGEST_LEN],
}

impl std::fmt::Debug for PassphraseFile {
    /// Parameters only. The salt and the digest are **never** printed, however
    /// a future caller reaches this type — a tracing span, a panic message, a
    /// `dbg!` left in a review. A salt is not secret; a habit of printing key
    /// material one field at a time is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseFile")
            .field("kdf", &KDF)
            .field("memory_kib", &self.memory_kib)
            .field("time_cost", &self.time_cost)
            .field("parallelism", &self.parallelism)
            .finish_non_exhaustive()
    }
}

impl PassphraseFile {
    /// Read and validate the file at `path`.
    ///
    /// Ownership and mode are checked **on the opened descriptor**, not by
    /// stat-then-open, so there is no TOCTOU window — the same discipline
    /// [`crate::realm`]'s `check_config_security` follows and for the same
    /// reason.
    pub fn load(path: &Path) -> Result<Self, PassphraseError> {
        if !path.is_absolute() {
            return Err(PassphraseError::NotAbsolute(path.to_path_buf()));
        }
        let mut file =
            fs::File::open(path).map_err(|e| PassphraseError::Io(path.to_path_buf(), e))?;
        check_security(path, &file)?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| PassphraseError::Io(path.to_path_buf(), e))?;
        Self::parse(path, &text)
    }

    /// [`Self::load`]'s parser, split out so the format has unit tests that do
    /// not need a filesystem — and so the writer below can be checked against
    /// the reader directly.
    fn parse(path: &Path, text: &str) -> Result<Self, PassphraseError> {
        let bad = |detail: &'static str| PassphraseError::Malformed(path.to_path_buf(), detail);
        // Exactly one non-empty, non-comment line. Deliberately strict: this
        // file is written by one command and read by one function, so a
        // tolerant reader would only ever tolerate a mistake.
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        let line = lines
            .next()
            .ok_or_else(|| bad("the file has no hash line"))?;
        if lines.next().is_some() {
            return Err(bad("more than one hash line; the file holds exactly one"));
        }
        let mut fields = line.split_whitespace();
        let mut next = |what: &'static str| fields.next().ok_or_else(|| bad(what));
        if next("missing format tag")? != MAGIC {
            return Err(bad("not a vitrin-lock-v1 file"));
        }
        if next("missing kdf name")? != KDF {
            return Err(bad("this core hashes lock passphrases with argon2id only"));
        }
        let memory_kib = parse_param(next("missing m=")?, "m=", path)?;
        let time_cost = parse_param(next("missing t=")?, "t=", path)?;
        let parallelism = parse_param(next("missing p=")?, "p=", path)?;
        let salt: [u8; SALT_LEN] = parse_hex(next("missing salt")?, path)?;
        let digest: [u8; DIGEST_LEN] = parse_hex(next("missing digest")?, path)?;
        if fields.next().is_some() {
            return Err(bad("trailing data after the digest"));
        }
        if !(MIN_MEMORY_KIB..=MAX_MEMORY_KIB).contains(&memory_kib) {
            return Err(bad("m= is outside the accepted range (8..=1048576 KiB)"));
        }
        if !(MIN_TIME_COST..=MAX_TIME_COST).contains(&time_cost) {
            return Err(bad("t= is outside the accepted range (1..=10)"));
        }
        if !(1..=MAX_PARALLELISM).contains(&parallelism) {
            return Err(bad("p= is outside the accepted range (1..=4)"));
        }
        // Built once here so a file whose parameters Argon2 itself rejects is a
        // startup failure rather than a first-unlock failure.
        params(memory_kib, time_cost, parallelism)
            .map_err(|_| bad("the parameters are not a valid Argon2 configuration"))?;
        Ok(Self {
            memory_kib,
            time_cost,
            parallelism,
            salt,
            digest,
        })
    }

    /// Does `attempt` derive to the stored digest?
    ///
    /// **Constant-time in the digest comparison** ([`subtle`]), so a wrong
    /// passphrase leaks nothing about how many leading bytes it got right. It
    /// is *not* constant-time in the attempt's length, and cannot be: Argon2
    /// hashes the bytes it is given. That is the standard and accepted shape.
    ///
    /// A derivation failure (which the bounded parameters make unreachable)
    /// answers **false**: the fail-closed direction, since the only other
    /// answer would unlock a session because the KDF errored.
    pub fn verify(&self, attempt: &[u8]) -> bool {
        let Ok(params) = params(self.memory_kib, self.time_cost, self.parallelism) else {
            tracing::error!("lock passphrase parameters became invalid; refusing the attempt");
            return false;
        };
        let mut derived = [0u8; DIGEST_LEN];
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        if argon
            .hash_password_into(attempt, &self.salt, &mut derived)
            .is_err()
        {
            tracing::error!("lock passphrase derivation failed; refusing the attempt");
            return false;
        }
        let ok = derived.ct_eq(&self.digest).into();
        wipe(&mut derived);
        ok
    }

    /// **Test seam.** A verifier for `secret` at Argon2's cheapest legal
    /// parameters.
    ///
    /// The default 19 MiB x 2 passes is right for a session and wrong for a
    /// suite that unlocks a few dozen times — a `cargo test` run would spend
    /// seconds deriving keys for tests that are about the lock's state machine.
    /// The *default* encoding is still exercised, by
    /// `a_generated_line_round_trips_through_the_parser` above, so this seam
    /// cannot hide a bug in the parameters an operator actually gets.
    #[cfg(test)]
    pub(crate) fn cheap_for_test(secret: &[u8]) -> Self {
        let salt = [7u8; SALT_LEN];
        let mut digest = [0u8; DIGEST_LEN];
        Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            params(MIN_MEMORY_KIB, MIN_TIME_COST, 1).expect("the minimum parameters are valid"),
        )
        .hash_password_into(secret, &salt, &mut digest)
        .expect("argon2 derivation at the minimum parameters");
        Self {
            memory_kib: MIN_MEMORY_KIB,
            time_cost: MIN_TIME_COST,
            parallelism: 1,
            salt,
            digest,
        }
    }
}

/// Build the Argon2 parameter set, with the output length pinned to
/// [`DIGEST_LEN`] so a file can never ask for a digest of a different size than
/// the one it stores.
fn params(memory_kib: u32, time_cost: u32, parallelism: u32) -> Result<Params, argon2::Error> {
    Params::new(memory_kib, time_cost, parallelism, Some(DIGEST_LEN))
}

fn parse_param(field: &str, prefix: &str, path: &Path) -> Result<u32, PassphraseError> {
    field
        .strip_prefix(prefix)
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| PassphraseError::Malformed(path.to_path_buf(), "malformed KDF parameter"))
}

fn parse_hex<const N: usize>(field: &str, path: &Path) -> Result<[u8; N], PassphraseError> {
    let bad = PassphraseError::Malformed(path.to_path_buf(), "malformed hex field");
    if field.len() != N * 2 {
        return Err(bad);
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&field[i * 2..i * 2 + 2], 16)
            .map_err(|_| PassphraseError::Malformed(path.to_path_buf(), "malformed hex field"))?;
    }
    Ok(out)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Refuse a passphrase file anybody but the core's own uid can read or write.
///
/// Checked on the opened fd (no stat-then-open TOCTOU). Stricter than
/// [`crate::realm`]'s config audit by exactly the read bit, for the reason the
/// module docs give.
fn check_security(path: &Path, file: &fs::File) -> Result<(), PassphraseError> {
    let insecure = |detail: String| PassphraseError::Insecure(path.to_path_buf(), detail);
    let st =
        rustix::fs::fstat(file).map_err(|e| PassphraseError::Io(path.to_path_buf(), e.into()))?;
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(insecure("not a regular file".into()));
    }
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(insecure(format!(
            "owned by uid {}, not the core's uid {euid}",
            st.st_uid
        )));
    }
    let mode = st.st_mode & 0o777;
    if mode & 0o077 != 0 {
        return Err(insecure(format!(
            "mode {mode:03o} is readable or writable by group/other; whoever can read it \
             gets an offline attack on the session passphrase, and whoever can write it \
             chooses what unlocks the session -- chmod 600 it"
        )));
    }
    Ok(())
}

/// Best-effort overwrite of a buffer that held key material.
///
/// `black_box` rather than a `zeroize` dependency: it is the barrier the
/// standard library documents for exactly this ("an opaque function the
/// optimizer must assume can observe its argument"), and one more crate in the
/// TCB to write zeroes is a poor trade. Stated as *best effort* because it is
/// one: Rust makes no guarantee that a dropped `Vec`'s old allocation, a
/// register spill, or a `realloc`'d growth is not still holding a copy. What
/// this buys is that the live buffer does not linger with a passphrase in it
/// for the rest of the session, which is the realistic exposure.
pub(crate) fn wipe(buf: &mut [u8]) {
    buf.fill(0);
    std::hint::black_box(buf);
}

/// Draw `SALT_LEN` bytes of kernel entropy for a fresh hash line.
///
/// The same `getrandom` call [`crate::consent::indicator`] mints the session's
/// trusted colour with, for the same reason it is spelled with `libc` there:
/// libc is already linked and rustix would need a feature bump for one call.
/// A short read is a hard failure — a salt drawn from a partially filled buffer
/// is a salt with predictable bytes.
fn random_salt() -> Result<[u8; SALT_LEN], std::io::Error> {
    let mut buf = [0u8; SALT_LEN];
    // SAFETY: `buf` is a live, correctly sized, exclusively borrowed stack
    // buffer; `getrandom` writes at most `len` bytes into it and returns how
    // many. No pointer outlives the call.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    if n < 0 || n as usize != buf.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(buf)
}

/// Turn a passphrase into the one line a `--lock-passphrase-file` holds
/// (`vitrind --lock-hash`).
///
/// # Why the core writes this rather than pointing at the `argon2(1)` CLI
///
/// The format is this module's, and a file nobody can produce is a feature
/// nobody can use — which in practice means an operator hand-assembling a line
/// and getting the parameter encoding subtly wrong, then discovering it at the
/// moment they are locked out. The generator and the parser being the same
/// module is what makes `a_generated_line_round_trips` a real check.
///
/// The passphrase arrives on **stdin**, never in `argv`, because `argv` is
/// world-readable through `/proc`. See `main`'s help text for the invocation.
pub(crate) fn hash_line(passphrase: &[u8]) -> Result<String, std::io::Error> {
    let salt = random_salt()?;
    let params = params(DEFAULT_MEMORY_KIB, DEFAULT_TIME_COST, DEFAULT_PARALLELISM)
        .map_err(|e| std::io::Error::other(format!("argon2 parameters: {e}")))?;
    let mut digest = [0u8; DIGEST_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase, &salt, &mut digest)
        .map_err(|e| std::io::Error::other(format!("argon2: {e}")))?;
    let line = format!(
        "{MAGIC} {KDF} m={DEFAULT_MEMORY_KIB} t={DEFAULT_TIME_COST} p={DEFAULT_PARALLELISM} {} {}",
        to_hex(&salt),
        to_hex(&digest)
    );
    wipe(&mut digest);
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file this test wrote, removed when the guard drops.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn with(contents: &str, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!(
                "vitrin-lock-pass-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            let path = dir.join("lock.hash");
            std::fs::write(&path, contents).expect("scratch write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("scratch chmod");
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    /// A fixed, cheap parameter set: the tests are about the format and the
    /// comparison, and the default 19 MiB x 2 passes would make every one of
    /// them a visible pause.
    fn cheap_line(passphrase: &[u8]) -> String {
        let salt = [7u8; SALT_LEN];
        let mut digest = [0u8; DIGEST_LEN];
        Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            params(MIN_MEMORY_KIB, MIN_TIME_COST, 1).unwrap(),
        )
        .hash_password_into(passphrase, &salt, &mut digest)
        .unwrap();
        format!(
            "{MAGIC} {KDF} m={MIN_MEMORY_KIB} t={MIN_TIME_COST} p=1 {} {}",
            to_hex(&salt),
            to_hex(&digest)
        )
    }

    #[test]
    fn the_right_passphrase_verifies_and_a_wrong_one_does_not() {
        // The passphrase is a test fixture and is not the maintainer's; it is
        // still never printed, so a failing run leaks nothing.
        let file = PassphraseFile::parse(Path::new("/x"), &cheap_line(b"correct horse")).unwrap();
        assert!(file.verify(b"correct horse"));
        assert!(!file.verify(b"correct hors"));
        assert!(!file.verify(b"correct horsee"));
        assert!(!file.verify(b""));
    }

    #[test]
    fn a_generated_line_round_trips_through_the_parser() {
        // The generator and the parser are one module precisely so this is
        // checkable; a mismatch here is an operator locked out of their own
        // session. Uses the real (default-parameter) writer, so the default
        // encoding is exercised at least once.
        let line = hash_line(b"a real passphrase").expect("entropy and argon2 are available");
        let file = PassphraseFile::parse(Path::new("/x"), &line).expect("its own line parses");
        assert!(file.verify(b"a real passphrase"));
        assert!(!file.verify(b"a real passphras"));
    }

    #[test]
    fn two_generated_lines_for_one_passphrase_differ() {
        // The salt is drawn per line, so a stolen file cannot be compared
        // against another machine's file for the same passphrase.
        let a = hash_line(b"same").unwrap();
        let b = hash_line(b"same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn malformed_files_are_refused_rather_than_guessed_at() {
        let cases: [&str; 9] = [
            "",
            "# only a comment\n",
            "not-vitrin argon2id m=8 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000",
            "vitrin-lock-v1 bcrypt m=8 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000",
            // m= outside the accepted range.
            "vitrin-lock-v1 argon2id m=2 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000",
            // t= outside the accepted range.
            "vitrin-lock-v1 argon2id m=8 t=99 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000",
            // Salt of the wrong length.
            "vitrin-lock-v1 argon2id m=8 t=1 p=1 0707 \
             0000000000000000000000000000000000000000000000000000000000000000",
            // Trailing data.
            "vitrin-lock-v1 argon2id m=8 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000 extra",
            // Two hash lines.
            "vitrin-lock-v1 argon2id m=8 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000\n\
             vitrin-lock-v1 argon2id m=8 t=1 p=1 07070707070707070707070707070707 \
             0000000000000000000000000000000000000000000000000000000000000000",
        ];
        for case in cases {
            assert!(
                PassphraseFile::parse(Path::new("/x"), case).is_err(),
                "must be refused: {case:?}"
            );
        }
    }

    #[test]
    fn a_relative_path_is_refused_before_anything_is_opened() {
        let err = PassphraseFile::load(Path::new("lock.hash")).unwrap_err();
        assert!(matches!(err, PassphraseError::NotAbsolute(_)));
    }

    #[test]
    fn a_group_readable_file_is_refused() {
        // The rule that is STRICTER than realm.toml's: 0640 is fine for a
        // config that names a program and fatal for a file that holds the
        // digest of the human's session passphrase.
        let scratch = Scratch::with(&cheap_line(b"x"), 0o640);
        let err = PassphraseFile::load(&scratch.path).expect_err("0640 must be refused");
        assert!(
            matches!(&err, PassphraseError::Insecure(_, detail) if detail.contains("640")),
            "{err}"
        );
        // ...and the same file at 0600 loads, so the test measures the mode
        // and not something incidental about the fixture.
        let ok = Scratch::with(&cheap_line(b"x"), 0o600);
        PassphraseFile::load(&ok.path).expect("0600 loads");
    }

    #[test]
    fn the_debug_impl_carries_no_key_material() {
        let salt_hex = to_hex(&[7u8; SALT_LEN]);
        let file = PassphraseFile::parse(Path::new("/x"), &cheap_line(b"secretish")).unwrap();
        let rendered = format!("{file:?}");
        assert!(!rendered.contains(&salt_hex), "{rendered}");
        assert!(!rendered.contains(&to_hex(&file.digest)), "{rendered}");
        assert!(rendered.contains("argon2id"), "{rendered}");
    }

    #[test]
    fn wiping_clears_the_buffer() {
        let mut buf = b"passphrase".to_vec();
        wipe(&mut buf);
        assert!(buf.iter().all(|b| *b == 0));
    }
}
