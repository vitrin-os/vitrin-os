//! The identity layer of the capability kernel (P1.4.1, issue #25): the
//! pluggable [`Verifier`] trait and its MVP implementation,
//! [`StaticVerifier`].
//!
//! PRD Doc 2 section 5.1 abstracts identity verification behind a pluggable
//! verifier so the same handshake serves SPIFFE SVIDs, OIDC workload tokens,
//! and SSH-cert principals later (decision D-008). **The trait is the
//! artifact; `StaticVerifier` is scaffolding** (plan decision D5): a
//! principal registry in `principals.toml` -- SPIFFE-shaped identity string
//! plus a pre-shared random token -- checked at `hello` time alongside the
//! `SO_PEERCRED` the transport recorded at accept (P1.2.1). The bound
//! identity this module produces is what every grant-table row (P1.4.2) and
//! every enforcement decision (P1.4.4) refers back to.
//!
//! # The `Verifier` trait shape (why it looks like this)
//!
//! The signature is designed so that SVID/OIDC verifiers -- which perform
//! remote work (JWKS fetch, SPIRE workload API, OIDC introspection) -- fit
//! **without a signature change**, while the MVP pulls no async runtime
//! (plan risk R7):
//!
//! - **`verify` receives the whole presented credential**
//!   ([`PresentedCredential`]): the claimed identity URI (a routing hint,
//!   never trusted), the scheme discriminator, the opaque credential bytes,
//!   and the kernel-attested [`PeerCred`]. Nothing about the static scheme
//!   leaks into the parameter list, and the `SO_PEERCRED` leg of the
//!   sender-constraint triple is the *verifier's* to weigh -- a fleet-tier
//!   SVID verifier may legitimately accept cross-uid peers where the MVP
//!   policy refuses them.
//! - **The outcome is a tri-state** ([`VerifyOutcome`]): `Bound` with the
//!   verifier-canonical identity, `Rejected` with a **log-only** cause, or
//!   `Unavailable` for infrastructure failure (remote verifier unreachable,
//!   trust bundle expired). The third state is the deferred/remote-failure
//!   carrier future verifiers need: a remote outage must be loggable and
//!   alertable as *not the client's fault* without becoming distinguishable
//!   on the wire -- every non-`Bound` outcome collapses to the uniform fatal
//!   `auth_failed` (identity-probing resistance, conventions section 7.1).
//! - **The call is synchronous and MAY block** for the server's own
//!   verifier latency: the handshake state machine's VERIFYING interval is
//!   explicitly "the server's own verifier latency" (conventions 7.1), and
//!   `StaticVerifier` is O(registry). When a genuinely remote verifier
//!   arrives, it is wrapped behind this same trait by a calloop-integrated
//!   worker (submission on the loop, completion as an event source) that
//!   parks the connection in VERIFYING -- reifying a state that already
//!   exists in the protocol -- rather than by changing this signature; the
//!   outcome type already carries everything a deferred completion needs to
//!   deliver.
//! - **`&self`, object-safe**: one verifier instance serves every
//!   connection (`&dyn Verifier`), so implementations manage their own
//!   interior caches (JWKS, trust bundles) and the core holds exactly one
//!   policy object.
//!
//! Implementations MUST NOT write credential bytes to logs or carry them in
//! [`RejectionCause`] -- at most the scheme and the byte length may be
//! recorded (IDL: the credential is secret material).
//!
//! # `principals.toml` schema (decided here, per issue #25)
//!
//! An array of `[[principal]]` tables; see `examples/principals.toml` for a
//! commented template. Keys per table:
//!
//! | key | type | required | meaning |
//! |---|---|---|---|
//! | `identity` | string | yes | SPIFFE-shaped canonical identity URI, e.g. `vitrin://local/agent/demo` -- unique across the file, at most 2048 bytes (the wire bound), shape-validated by [`PrincipalIdentity`] |
//! | `token` | string | yes | the pre-shared random bearer token, compared byte-wise constant-time against `hello`'s `credential`; at least 16 bytes (64 random hex chars recommended: `openssl rand -hex 32`) |
//! | `uid` | integer | no | additionally require the connecting peer's `SO_PEERCRED` uid to equal this value; must equal the uid the core itself runs as, or the registry is refused at load (see the `SO_PEERCRED` policy section) |
//!
//! The file is parsed by a **hand-rolled strict TOML subset** rather than a
//! TOML crate: plan risk R7 names config parsing as the classic TCB
//! dependency-creep vector, and this schema is three scalar keys. The subset
//! accepts exactly: comments, blank lines, `[[principal]]` headers, `key =
//! "basic string"` (escapes limited to `\\` and `\"`), and `key = integer`.
//! Everything else -- other table headers, dotted keys, inline tables,
//! arrays, multi-line or literal strings, other escapes -- is a load error,
//! never a guess. Every file this parser accepts is valid TOML, so external
//! tooling interoperates and a later swap to a full parser changes nothing
//! user-visible.
//!
//! # Registry-file permission checks (decided here)
//!
//! The registry holds bearer tokens, so the file is secret material.
//! [`StaticVerifier::load`] **refuses** (hard startup error, never a
//! warning) a registry that is not a regular file, is not owned by the
//! core's own effective uid, or is readable or writable by group or other
//! (any of the `0o077` mode bits) -- the OpenSSH strict-modes posture for
//! private keys. A world-readable `principals.toml` would let any local
//! process steal every agent identity; failing fast is the only honest
//! response. The check runs on the **opened fd** (`fstat`), so there is no
//! stat-then-open TOCTOU window.
//!
//! # `SO_PEERCRED` policy (decided here: hard refusal)
//!
//! A peercred mismatch is a **hard refusal** (wire-uniform `auth_failed`),
//! not a logged warning:
//!
//! - the IDL already names "`SO_PEERCRED` mismatch" as an `auth_failed`
//!   *rejection* cause -- the protocol reserves refusal semantics for it,
//!   and a warn-only MVP would make one leg of the sender-constraint triple
//!   (connection, verified credential, `SO_PEERCRED`) decorative;
//! - refusal is uniform on the wire, so strictness costs nothing in
//!   identity-probing resistance;
//! - in the MVP's single-user deployment a mismatch is always
//!   misconfiguration or an attack, never a legitimate state.
//!
//! Concretely, [`StaticVerifier`] requires the peer uid to equal the core's
//! own effective uid (the socket directory is `0700`, so this is defense in
//! depth against a mis-permissioned runtime dir), plus the per-row `uid`
//! pin when present. Because the pin is conjoined with that euid rule, a
//! pin naming any other uid could never be satisfied, so
//! [`StaticVerifier::from_rows`] refuses such a row at load time (a hard
//! error, like every registry problem) rather than carrying a row that
//! silently never binds. The pin is thereby a deployment assertion: a
//! registry pinning uid `N` loads only where the core itself runs as uid
//! `N`. The peer **pid is never policy**: it is `None` when
//! not mappable into the core's pid namespace and is racy by nature
//! (documented on [`PeerCred`]); uid is the authority anchor. Future
//! verifiers choose their own peercred policy -- the trait hands them the
//! recorded credentials and no more.
//!
//! # Token comparison
//!
//! [`StaticVerifier`] compares tokens with a branch-free constant-time
//! routine and performs **identical verification work for unknown
//! identities** (a decoy comparison), per conventions 7.1: latency must not
//! become the oracle the uniform `auth_failed` message refuses to be.
//! Honest limits, documented rather than hidden: token *length* equality is
//! observable in principle (content is not) -- mitigated by the recommended
//! fixed-length 64-hex-char token format -- and this is best-effort
//! constant time in portable Rust (`std::hint::black_box`, no data-
//! dependent branches), not a vetted cryptographic primitive; swapping in
//! one is a verifier-internal change.

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use vitrin_ipc::PeerCred;

/// The wire bound on identity strings (`vitrin_handshake.hello.identity`
/// and `vitrin_principal.bound.identity`, conventions 2.3): the SPIFFE-ID
/// maximum. Enforced at registry load so encoding `bound` can never
/// panic on an over-bound canonical identity.
pub(crate) const MAX_IDENTITY_BYTES: usize = 2048;

/// The `credential_type` discriminator [`StaticVerifier`] serves (the D5
/// scheme; the SDK's default). Other schemes name future verifiers
/// (`spiffe-jwt-svid`, `oidc`, `ssh-cert`).
pub(crate) const STATIC_TOKEN_SCHEME: &str = "static-token";

/// Minimum registry token length in bytes, enforced at load. A static
/// bearer token with trivial entropy would quietly rot the security story
/// (the plan's R6 honesty posture); 16 bytes of text is still far below the
/// recommended 64 random hex characters, but rules out "hunter2".
pub(crate) const MIN_TOKEN_BYTES: usize = 16;

/// Decoy compared against when the claimed identity is unknown, so unknown
/// and known identities cost the same credential-comparison work
/// (conventions 7.1). Content is irrelevant (the result is discarded);
/// 64 bytes matches the recommended token length so the common work shape
/// is identical too.
const DECOY_TOKEN: [u8; 64] = [0u8; 64];

// ---------------------------------------------------------------------------
// Principal identity
// ---------------------------------------------------------------------------

/// A validated, verifier-canonical principal identity.
///
/// The type distinguishes the **verifier-owned canonical identity** from
/// free client text: `vitrin_principal.bound`, the grant table (P1.4.2),
/// the consent prompt, and the flight recorder must only ever see this
/// type, never the claimed string out of `hello` (IDL: everything rendered
/// about a principal derives from the verifier-owned value). Constructing
/// one validates shape, so an instance is always wire-encodable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PrincipalIdentity(String);

impl PrincipalIdentity {
    /// Validate and wrap a SPIFFE-shaped identity URI:
    /// `scheme://trust-domain/path`, at most [`MAX_IDENTITY_BYTES`] bytes,
    /// ASCII subset `[A-Za-z0-9._/-]` after a `[a-z][a-z0-9+.-]*` scheme,
    /// non-empty trust domain, non-empty path, no empty segments, no
    /// trailing slash. Strict by design: this string reaches logs, the
    /// consent prompt, and the wire, so control characters and lookalike
    /// games are rejected at the door.
    pub fn parse(s: &str) -> Result<Self, IdentityShapeError> {
        if s.is_empty() {
            return Err(IdentityShapeError("identity is empty"));
        }
        if s.len() > MAX_IDENTITY_BYTES {
            return Err(IdentityShapeError(
                "identity exceeds the 2048-byte SPIFFE-ID maximum",
            ));
        }
        let (scheme, rest) = s
            .split_once("://")
            .ok_or(IdentityShapeError("identity is not a `scheme://` URI"))?;
        let mut scheme_chars = scheme.chars();
        let scheme_ok = match scheme_chars.next() {
            Some(c) if c.is_ascii_lowercase() => scheme_chars
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c)),
            _ => false,
        };
        if !scheme_ok {
            return Err(IdentityShapeError(
                "identity scheme must match [a-z][a-z0-9+.-]*",
            ));
        }
        let (authority, path) = rest.split_once('/').ok_or(IdentityShapeError(
            "identity must carry a path after the trust domain",
        ))?;
        if authority.is_empty() {
            return Err(IdentityShapeError("identity trust domain is empty"));
        }
        if path.is_empty() {
            return Err(IdentityShapeError("identity path is empty"));
        }
        if path.split('/').any(str::is_empty) {
            return Err(IdentityShapeError(
                "identity has an empty path segment or trailing slash",
            ));
        }
        let body_ok = rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c));
        if !body_ok {
            return Err(IdentityShapeError(
                "identity contains a character outside [A-Za-z0-9._/-]",
            ));
        }
        Ok(Self(s.to_owned()))
    }

    /// The canonical identity string (always wire-encodable).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a candidate identity string failed [`PrincipalIdentity::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdentityShapeError(&'static str);

impl fmt::Display for IdentityShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

// ---------------------------------------------------------------------------
// The Verifier trait
// ---------------------------------------------------------------------------

/// Everything a `hello` presents, plus the kernel-attested peer
/// credentials: the full input to one verification.
///
/// `credential` is **secret material** -- deliberately a byte slice with no
/// `Debug` derive on this struct's containing path ever printing it.
/// Implementations MUST NOT copy it into logs, errors, or
/// [`RejectionCause`]; at most `credential_type` and `credential.len()` may
/// be recorded.
pub(crate) struct PresentedCredential<'a> {
    /// The claimed identity URI out of `hello` -- a routing hint. The
    /// authoritative identity is whatever the verifier returns in
    /// [`VerifyOutcome::Bound`], never this string.
    pub claimed_identity: &'a str,
    /// The credential scheme discriminator (e.g. [`STATIC_TOKEN_SCHEME`]).
    pub credential_type: &'a str,
    /// Opaque scheme-defined credential bytes.
    pub credential: &'a [u8],
    /// `SO_PEERCRED` recorded by the transport at accept (P1.2.1) -- one
    /// leg of the sender-constraint triple, handed to the verifier to
    /// weigh under its own policy.
    pub peer: PeerCred,
}

/// The identity a successful verification binds.
///
/// A struct rather than a bare identity so future verifiers can extend it
/// additively (an X.509-SVID's not-after, an OIDC token's audience) without
/// touching every match site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundPrincipal {
    /// The verifier-canonical identity: what `vitrin_principal.bound`
    /// carries and what the grant table keys rows by (P1.4.2).
    pub identity: PrincipalIdentity,
}

/// Why a credential was rejected. **Log-only**: on the wire every variant
/// collapses to the uniform fatal `auth_failed` with a fixed phrase
/// (conventions 7.1, identity-probing resistance); this type exists so the
/// server log can carry the real cause. Carries no credential bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RejectionCause {
    /// `credential_type` names a scheme this verifier does not serve.
    UnsupportedScheme { presented: String },
    /// The claimed identity is not in the registry (or failed shape
    /// validation -- deliberately not distinguished further even in the
    /// log's cause taxonomy; the raw claimed string is logged alongside by
    /// the handshake layer).
    UnknownIdentity,
    /// The credential did not match the registered token.
    BadToken,
    /// The kernel-attested peer uid failed this verifier's policy (the
    /// same-euid rule or a per-row `uid` pin).
    PeerCredMismatch { required_uid: u32, peer_uid: u32 },
}

impl fmt::Display for RejectionCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RejectionCause::UnsupportedScheme { presented } => {
                write!(f, "unsupported credential scheme {presented:?}")
            }
            RejectionCause::UnknownIdentity => write!(f, "unknown or malformed claimed identity"),
            RejectionCause::BadToken => write!(f, "credential does not match the registered token"),
            RejectionCause::PeerCredMismatch {
                required_uid,
                peer_uid,
            } => write!(
                f,
                "SO_PEERCRED mismatch: peer uid {peer_uid}, required {required_uid}"
            ),
        }
    }
}

/// The tri-state outcome of one verification. Both non-`Bound` states are
/// wire-uniform `auth_failed`; they differ only in what the server log and
/// operator alerting should say (a rejected client vs. broken verifier
/// infrastructure). See the module docs for why this shape already fits
/// remote SVID/OIDC verifiers.
pub(crate) enum VerifyOutcome {
    /// Credential accepted: bind this principal.
    Bound(BoundPrincipal),
    /// Credential rejected; the cause goes to the server log only.
    Rejected(RejectionCause),
    /// Verification could not complete -- infrastructure failure, not a
    /// judgement on the credential (remote verifier unreachable, trust
    /// bundle expired). The message describes the failure for the log;
    /// it MUST NOT contain credential bytes.
    Unavailable(String),
}

/// The pluggable identity verifier (PRD Doc 2 section 5.1, D-008): the one
/// seam through which every principal handshake's credential is judged.
///
/// Contract:
///
/// - Object-safe; one instance serves every connection (`&dyn Verifier`),
///   for the connection's whole lifetime.
/// - MAY block for its own verification latency (the VERIFYING interval is
///   server latency, conventions 7.1); a future remote verifier integrates
///   through a worker behind this same trait (module docs).
/// - SHOULD take uniform time across rejection causes so latency does not
///   become an identity oracle.
/// - MUST NOT log or otherwise emit credential bytes.
pub(crate) trait Verifier {
    /// Judge one presented credential.
    fn verify(&self, presented: &PresentedCredential<'_>) -> VerifyOutcome;
}

// ---------------------------------------------------------------------------
// StaticVerifier
// ---------------------------------------------------------------------------

/// One registry row: the D5 static principal.
#[derive(Clone)]
pub(crate) struct StaticPrincipal {
    /// The canonical identity this row binds.
    pub identity: PrincipalIdentity,
    /// The pre-shared bearer token, byte-for-byte as registered.
    pub token: Vec<u8>,
    /// Optional per-row `SO_PEERCRED` uid pin.
    pub uid: Option<u32>,
}

impl fmt::Debug for StaticPrincipal {
    /// Hand-written so the token can never reach a log through `{:?}` --
    /// the same secrecy rule the trait imposes on credentials, applied to
    /// the registry's own copy. Only the token *length* is shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticPrincipal")
            .field("identity", &self.identity)
            .field(
                "token",
                &format_args!("<{} bytes redacted>", self.token.len()),
            )
            .field("uid", &self.uid)
            .finish()
    }
}

/// The MVP verifier: a static registry loaded from `principals.toml`
/// (module docs carry the schema and every policy decision).
pub(crate) struct StaticVerifier {
    rows: Vec<StaticPrincipal>,
    /// The uid every peer must present (the core's own euid under
    /// [`StaticVerifier::load`]); per-row pins narrow further.
    required_uid: u32,
}

impl StaticVerifier {
    /// Load and validate `principals.toml` at `path`, refusing insecure
    /// file permissions (module docs). The conventional location is
    /// `$XDG_CONFIG_HOME/vitrin/principals.toml`; the path is the caller's
    /// because the runtime wiring (and its `--principals` flag) lands with
    /// M1.1 integration.
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let mut file = fs::File::open(path).map_err(RegistryError::Io)?;
        check_registry_security(&file)?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(RegistryError::Io)?;
        let rows = parse_registry(&text)?;
        Self::from_rows(rows, effective_uid())
    }

    /// Build a verifier from already-validated-shape rows and an explicit
    /// required peer uid. Shared by [`load`](Self::load), tests, and any
    /// embedder that sources rows elsewhere; row-set invariants (non-empty,
    /// unique identities, minimum token length) are enforced here so no
    /// constructor can bypass them.
    pub fn from_rows(rows: Vec<StaticPrincipal>, required_uid: u32) -> Result<Self, RegistryError> {
        if rows.is_empty() {
            return Err(RegistryError::Invalid {
                detail: "registry defines no principals".into(),
            });
        }
        for (i, row) in rows.iter().enumerate() {
            if row.token.len() < MIN_TOKEN_BYTES {
                return Err(RegistryError::Invalid {
                    detail: format!(
                        "principal {} token is shorter than {MIN_TOKEN_BYTES} bytes",
                        row.identity
                    ),
                });
            }
            // The verify-time uid policy is a conjunction of the global
            // anchor and the pin, so a pin differing from the anchor is
            // unsatisfiable -- a row that could never bind. Refuse it here
            // (module docs: the pin is a deployment assertion) instead of
            // letting it rot into mysterious per-connection refusals.
            if let Some(pin) = row.uid {
                if pin != required_uid {
                    return Err(RegistryError::Invalid {
                        detail: format!(
                            "principal {} pins peer uid {pin}, but every peer must \
                             present uid {required_uid} (the core's own); the pin is \
                             unsatisfiable -- run the core as uid {pin} or drop the pin",
                            row.identity
                        ),
                    });
                }
            }
            if rows[..i].iter().any(|r| r.identity == row.identity) {
                return Err(RegistryError::Invalid {
                    detail: format!("duplicate identity {}", row.identity),
                });
            }
        }
        Ok(Self { rows, required_uid })
    }
}

impl Verifier for StaticVerifier {
    /// Uniform-work verification (conventions 7.1): every call runs the
    /// full row scan, exactly one constant-time token comparison (against
    /// the registered token or the decoy), and the uid policy checks,
    /// *then* picks the outcome -- so rejection causes are not
    /// latency-distinguishable. Identities are compared with ordinary
    /// equality: the identity is not secret; the token comparison is what
    /// must not leak.
    fn verify(&self, presented: &PresentedCredential<'_>) -> VerifyOutcome {
        let scheme_ok = presented.credential_type == STATIC_TOKEN_SCHEME;
        let mut matched: Option<&StaticPrincipal> = None;
        for row in &self.rows {
            if row.identity.as_str() == presented.claimed_identity {
                matched = Some(row);
            }
        }
        let expected: &[u8] = matched.map_or(&DECOY_TOKEN[..], |r| &r.token);
        let token_ok = constant_time_eq(presented.credential, expected) && matched.is_some();
        let row_uid = matched.and_then(|r| r.uid);
        // The uid to cite on refusal: the leg of the conjunction below that
        // actually failed -- the global anchor whenever the peer misses it,
        // the per-row pin only when the anchor passed. (Citing the pin
        // unconditionally once rendered a self-contradictory "peer uid N,
        // required N" when the pin matched but the anchor did not.)
        // `from_rows` refuses pins differing from the anchor, so today the
        // two branches agree; the selection stays locally correct either way.
        let uid_required = if presented.peer.uid == self.required_uid {
            row_uid.unwrap_or(self.required_uid)
        } else {
            self.required_uid
        };
        let uid_ok = presented.peer.uid == self.required_uid
            && row_uid.is_none_or(|u| presented.peer.uid == u);

        // Cause precedence (log taxonomy only; the wire is uniform):
        // scheme, identity, token, peercred.
        if !scheme_ok {
            return VerifyOutcome::Rejected(RejectionCause::UnsupportedScheme {
                presented: presented.credential_type.to_owned(),
            });
        }
        let Some(row) = matched else {
            return VerifyOutcome::Rejected(RejectionCause::UnknownIdentity);
        };
        if !token_ok {
            return VerifyOutcome::Rejected(RejectionCause::BadToken);
        }
        if !uid_ok {
            return VerifyOutcome::Rejected(RejectionCause::PeerCredMismatch {
                required_uid: uid_required,
                peer_uid: presented.peer.uid,
            });
        }
        VerifyOutcome::Bound(BoundPrincipal {
            identity: row.identity.clone(),
        })
    }
}

/// The core's own effective uid: the registry-ownership requirement and the
/// default peer-uid policy anchor.
fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Refuse a registry file that is not a regular file, not owned by the
/// core's euid, or accessible to group/other (any `0o077` bit) -- checked
/// on the opened fd, so no stat-then-open TOCTOU. See the module docs for
/// the policy rationale.
fn check_registry_security(file: &fs::File) -> Result<(), RegistryError> {
    let st = rustix::fs::fstat(file).map_err(|e| RegistryError::Io(e.into()))?;
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(RegistryError::Insecure {
            detail: "registry is not a regular file".into(),
        });
    }
    let euid = effective_uid();
    if st.st_uid != euid {
        return Err(RegistryError::Insecure {
            detail: format!(
                "registry is owned by uid {}, not the core's uid {euid}",
                st.st_uid
            ),
        });
    }
    let mode = st.st_mode & 0o777;
    if mode & 0o077 != 0 {
        return Err(RegistryError::Insecure {
            detail: format!(
                "registry mode {mode:03o} is readable or writable by group/other; \
                 it holds bearer tokens -- chmod 600 it"
            ),
        });
    }
    Ok(())
}

/// Branch-free byte equality for secret comparison. Equal lengths fold the
/// XOR of every byte pair into one accumulator with no data-dependent
/// branch; unequal lengths burn a same-shaped pass over the presented bytes
/// before rejecting (length is treated as non-secret -- module docs).
/// `black_box` keeps the folds from being optimized into an early exit.
fn constant_time_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        let burn = presented.iter().fold(0u8, |acc, &b| acc | (b ^ 0xa5));
        std::hint::black_box(burn);
        return false;
    }
    let diff = presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    std::hint::black_box(diff) == 0
}

// ---------------------------------------------------------------------------
// Registry loading: errors and the strict TOML subset
// ---------------------------------------------------------------------------

/// Why loading `principals.toml` failed. Always a hard startup error --
/// the core must not come up half-authenticated.
#[derive(Debug)]
pub(crate) enum RegistryError {
    /// Filesystem failure opening or reading the registry.
    Io(io::Error),
    /// The file's type, ownership, or mode fails the secret-material policy.
    Insecure { detail: String },
    /// The file is outside the documented TOML subset or malformed.
    Parse { line: usize, detail: String },
    /// The rows violate a registry invariant (duplicate identity, short
    /// token, empty registry, bad identity shape).
    Invalid { detail: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Io(e) => write!(f, "registry i/o error: {e}"),
            RegistryError::Insecure { detail } => write!(f, "registry refused: {detail}"),
            RegistryError::Parse { line, detail } => {
                write!(f, "registry parse error at line {line}: {detail}")
            }
            RegistryError::Invalid { detail } => write!(f, "registry invalid: {detail}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// One `[[principal]]` table under construction.
#[derive(Default)]
struct RawRow {
    identity: Option<(String, usize)>,
    token: Option<String>,
    uid: Option<u32>,
}

/// Parse the documented strict TOML subset (module docs) into validated
/// rows. Anything outside the subset is an error, never a guess.
fn parse_registry(text: &str) -> Result<Vec<StaticPrincipal>, RegistryError> {
    let parse_err = |line: usize, detail: &str| RegistryError::Parse {
        line,
        detail: detail.to_owned(),
    };
    let mut raw_rows: Vec<RawRow> = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let (header, rest) =
                split_header(line).ok_or_else(|| parse_err(line_no, "malformed table header"))?;
            if !matches!(rest.trim_start().chars().next(), None | Some('#')) {
                return Err(parse_err(line_no, "trailing content after table header"));
            }
            if header != "[[principal]]" {
                return Err(parse_err(
                    line_no,
                    "only [[principal]] tables are allowed in this file",
                ));
            }
            raw_rows.push(RawRow::default());
            continue;
        }
        let Some(row) = raw_rows.last_mut() else {
            return Err(parse_err(
                line_no,
                "key outside any [[principal]] table (top-level keys are not allowed)",
            ));
        };
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| parse_err(line_no, "expected `key = value`"))?;
        let key = key.trim();
        let value = value.trim_start();
        match key {
            "identity" => {
                if row.identity.is_some() {
                    return Err(parse_err(line_no, "duplicate `identity` key"));
                }
                row.identity = Some((parse_basic_string(value, line_no)?, line_no));
            }
            "token" => {
                if row.token.is_some() {
                    return Err(parse_err(line_no, "duplicate `token` key"));
                }
                row.token = Some(parse_basic_string(value, line_no)?);
            }
            "uid" => {
                if row.uid.is_some() {
                    return Err(parse_err(line_no, "duplicate `uid` key"));
                }
                row.uid = Some(parse_integer(value, line_no)?);
            }
            other => {
                return Err(RegistryError::Parse {
                    line: line_no,
                    detail: format!(
                        "unknown key {other:?} (this schema defines identity, token, uid)"
                    ),
                });
            }
        }
    }

    let mut rows = Vec::with_capacity(raw_rows.len());
    for row in raw_rows {
        let (identity_str, line) = row.identity.ok_or_else(|| RegistryError::Invalid {
            detail: "a [[principal]] table is missing `identity`".into(),
        })?;
        let identity =
            PrincipalIdentity::parse(&identity_str).map_err(|e| RegistryError::Parse {
                line,
                detail: format!("identity {identity_str:?}: {e}"),
            })?;
        let token = row.token.ok_or_else(|| RegistryError::Invalid {
            detail: format!("principal {identity} is missing `token`"),
        })?;
        rows.push(StaticPrincipal {
            identity,
            token: token.into_bytes(),
            uid: row.uid,
        });
    }
    Ok(rows)
}

/// Split a `[[principal]]`-shaped header off the front of `line`. Returns
/// the header text (brackets included) and the remainder.
fn split_header(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("[[")?;
    let end = rest.find("]]")?;
    let name = &rest[..end];
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
        Some((&line[..end + 4], &rest[end + 2..]))
    } else {
        None
    }
}

/// Parse a TOML basic string (`"..."`) at the start of `value`, allowing
/// only the `\\` and `\"` escapes and rejecting control characters; the
/// remainder may hold only whitespace or a comment. Multi-line strings,
/// literal strings, and other escapes are outside the subset.
fn parse_basic_string(value: &str, line: usize) -> Result<String, RegistryError> {
    let err = |detail: &str| RegistryError::Parse {
        line,
        detail: detail.to_owned(),
    };
    let mut chars = value.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return Err(err("expected a double-quoted string value")),
    }
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in chars {
        if escaped {
            match c {
                '\\' | '"' => out.push(c),
                _ => {
                    return Err(err(
                        "escape outside the supported subset (only \\\\ and \\\" are allowed)",
                    ))
                }
            }
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => {
                let rest = value[i + 1..].trim_start();
                if !matches!(rest.chars().next(), None | Some('#')) {
                    return Err(err("trailing content after string value"));
                }
                return Ok(out);
            }
            c if c.is_control() => return Err(err("control character inside string value")),
            c => out.push(c),
        }
    }
    Err(err("unterminated string value"))
}

/// Parse a bare non-negative decimal integer fitting `u32` (the uid
/// domain); the remainder may hold only whitespace or a comment. Leading
/// zeros are rejected: TOML 1.0 forbids them, and the subset's invariant
/// is that every accepted file is valid TOML (module docs) -- `007` must
/// not load today only to break under external tooling or a future parser
/// swap. A bare `0` stays legal.
fn parse_integer(value: &str, line: usize) -> Result<u32, RegistryError> {
    let err = |detail: &str| RegistryError::Parse {
        line,
        detail: detail.to_owned(),
    };
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, rest) = value.split_at(end);
    if digits.is_empty() {
        return Err(err("expected a non-negative integer value"));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(err("leading zeros are not valid TOML integers"));
    }
    if !matches!(rest.trim_start().chars().next(), None | Some('#')) {
        return Err(err("trailing content after integer value"));
    }
    digits
        .parse::<u32>()
        .map_err(|_| err("integer does not fit u32"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A registered demo token: 64 chars, the recommended shape.
    const TOKEN: &str = "9b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a";

    fn demo_identity() -> PrincipalIdentity {
        PrincipalIdentity::parse("vitrin://local/agent/demo").unwrap()
    }

    fn my_uid() -> u32 {
        effective_uid()
    }

    fn demo_verifier() -> StaticVerifier {
        StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            }],
            my_uid(),
        )
        .unwrap()
    }

    fn peer() -> PeerCred {
        PeerCred {
            pid: Some(std::process::id() as i32),
            uid: my_uid(),
            gid: 0,
        }
    }

    fn present<'a>(identity: &'a str, credential: &'a str) -> PresentedCredential<'a> {
        PresentedCredential {
            claimed_identity: identity,
            credential_type: STATIC_TOKEN_SCHEME,
            credential: credential.as_bytes(),
            peer: peer(),
        }
    }

    fn expect_rejected(outcome: VerifyOutcome) -> RejectionCause {
        match outcome {
            VerifyOutcome::Rejected(cause) => cause,
            VerifyOutcome::Bound(b) => panic!("expected rejection, got bound {}", b.identity),
            VerifyOutcome::Unavailable(m) => panic!("expected rejection, got unavailable: {m}"),
        }
    }

    // -- identity shape ----------------------------------------------------

    #[test]
    fn identity_shape_is_validated() {
        for good in [
            "vitrin://local/agent/demo",
            "spiffe://example.org/workload/billing-7",
            "ssh://fleet.example/ops/alice",
        ] {
            assert_eq!(PrincipalIdentity::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "no-scheme",
            "vitrin://",                     // no authority, no path
            "vitrin://local",                // no path
            "vitrin://local/",               // empty path
            "vitrin://local//demo",          // empty segment
            "vitrin://local/agent/demo/",    // trailing slash
            "Vitrin://local/agent/demo",     // uppercase scheme
            "vitrin://local/agent demo",     // space
            "vitrin://local/agent/d\u{7}mo", // control char
        ] {
            assert!(
                PrincipalIdentity::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        let oversized = format!("vitrin://local/{}", "a".repeat(MAX_IDENTITY_BYTES));
        assert!(PrincipalIdentity::parse(&oversized).is_err());
    }

    // -- token comparison --------------------------------------------------

    #[test]
    fn token_comparison_semantics() {
        assert!(constant_time_eq(b"same-bytes", b"same-bytes"));
        assert!(!constant_time_eq(b"same-bytes", b"same-bytez"));
        assert!(!constant_time_eq(b"short", b"longer-than-short"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    // -- registry parsing --------------------------------------------------

    #[test]
    fn registry_parses_the_documented_schema() {
        let text = format!(
            r##"# Vitrin OS principal registry
[[principal]]
identity = "vitrin://local/agent/demo"   # the demo agent
token = "{TOKEN}"

[[principal]]
identity = "vitrin://local/agent/ci-probe"
token = "abcdefabcdefabcdefabcdef"
uid = 4242  # deployment assertion: only loads where the core runs as 4242
"##
        );
        let rows = parse_registry(&text).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].identity.as_str(), "vitrin://local/agent/demo");
        assert_eq!(rows[0].token, TOKEN.as_bytes());
        assert_eq!(rows[0].uid, None);
        assert_eq!(rows[1].identity.as_str(), "vitrin://local/agent/ci-probe");
        assert_eq!(rows[1].uid, Some(4242));
    }

    #[test]
    fn registry_supports_the_two_escapes_and_hash_inside_strings() {
        let text = r##"[[principal]]
identity = "vitrin://local/agent/demo"
token = "with#hash-and-\\-and-\"-inside"
"##;
        let rows = parse_registry(text).unwrap();
        assert_eq!(rows[0].token, br#"with#hash-and-\-and-"-inside"#);
    }

    #[test]
    fn registry_rejects_non_subset_toml() {
        for (text, why) in [
            ("[principal]\n", "single-bracket table"),
            ("[[principal]] extra\n", "trailing content after header"),
            ("identity = \"vitrin://l/a\"\n", "top-level key"),
            ("[[principal]]\nidentity.claim = \"x\"\n", "dotted key"),
            ("[[principal]]\nidentity = { x = 1 }\n", "inline table"),
            ("[[principal]]\ntoken = ['a']\n", "array value"),
            (
                "[[principal]]\ntoken = \"\"\"m\"\"\"\n",
                "multi-line string",
            ),
            ("[[principal]]\ntoken = 'literal'\n", "literal string"),
            (
                "[[principal]]\ntoken = \"a\\nb\"\n",
                "escape outside subset",
            ),
            ("[[principal]]\ntoken = \"unterminated\n", "unterminated"),
            ("[[principal]]\nuid = -1\n", "negative integer"),
            ("[[principal]]\nuid = 99999999999\n", "integer overflow"),
            ("[[principal]]\nuid = 007\n", "leading zero"),
            ("[[principal]]\nuid = 00\n", "leading zero on zero"),
            ("[[principal]]\ntoken = \"a\" junk\n", "trailing junk"),
        ] {
            assert!(parse_registry(text).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn bare_zero_is_a_legal_integer() {
        // The leading-zero rejection must not take the one-digit `0` with
        // it: TOML allows a bare zero, and uid 0 (root) is expressible.
        let text =
            format!("[[principal]]\nidentity = \"vitrin://l/a\"\ntoken = \"{TOKEN}\"\nuid = 0\n");
        assert_eq!(parse_registry(&text).unwrap()[0].uid, Some(0));
    }

    #[test]
    fn registry_rejects_unknown_and_duplicate_keys() {
        let unknown = "[[principal]]\ntokn = \"typo\"\n";
        assert!(parse_registry(unknown).is_err());
        let duplicate = "[[principal]]\nidentity = \"vitrin://l/a\"\nidentity = \"vitrin://l/b\"\n";
        assert!(parse_registry(duplicate).is_err());
    }

    #[test]
    fn registry_rejects_missing_fields_short_tokens_and_duplicates() {
        let missing_token = "[[principal]]\nidentity = \"vitrin://local/agent/demo\"\n";
        assert!(parse_registry(missing_token).is_err());
        let missing_identity = format!("[[principal]]\ntoken = \"{TOKEN}\"\n");
        assert!(parse_registry(&missing_identity).is_err());

        let short = vec![StaticPrincipal {
            identity: demo_identity(),
            token: b"tooshort".to_vec(),
            uid: None,
        }];
        assert!(StaticVerifier::from_rows(short, my_uid()).is_err());

        let dup = vec![
            StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            },
            StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            },
        ];
        assert!(StaticVerifier::from_rows(dup, my_uid()).is_err());

        assert!(StaticVerifier::from_rows(Vec::new(), my_uid()).is_err());
    }

    // -- file permission policy -------------------------------------------

    /// A scratch registry file with the given mode, in a private temp dir.
    fn registry_file(mode: u32) -> (std::path::PathBuf, std::path::PathBuf) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-identity-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("principals.toml");
        fs::write(
            &path,
            format!(
                "[[principal]]\nidentity = \"vitrin://local/agent/demo\"\ntoken = \"{TOKEN}\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        (dir, path)
    }

    #[test]
    fn registry_with_owner_only_mode_loads() {
        let (dir, path) = registry_file(0o600);
        let verifier = StaticVerifier::load(&path).unwrap();
        assert_eq!(verifier.rows.len(), 1);
        assert_eq!(verifier.required_uid, my_uid());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn group_or_world_accessible_registry_is_refused() {
        for mode in [0o644, 0o640, 0o604, 0o660, 0o602, 0o620] {
            let (dir, path) = registry_file(mode);
            match StaticVerifier::load(&path) {
                Err(RegistryError::Insecure { detail }) => {
                    assert!(detail.contains("chmod 600"), "unexpected detail: {detail}")
                }
                other => panic!("mode {mode:03o} must be refused, got {other:?}"),
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn non_regular_registry_is_refused() {
        let (dir, path) = registry_file(0o600);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            StaticVerifier::load(&path),
            // Opening a directory for read succeeds on Linux only until the
            // first read; the fstat probe refuses it first.
            Err(RegistryError::Insecure { .. }) | Err(RegistryError::Io(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    impl fmt::Debug for StaticVerifier {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // Token bytes are secret; Debug shows row count only. Defined in
            // tests because only test assertions ever format the verifier.
            write!(f, "StaticVerifier({} rows)", self.rows.len())
        }
    }

    // -- verification ------------------------------------------------------

    #[test]
    fn registered_principal_with_the_right_token_binds() {
        let v = demo_verifier();
        match v.verify(&present("vitrin://local/agent/demo", TOKEN)) {
            VerifyOutcome::Bound(b) => {
                assert_eq!(b.identity, demo_identity());
            }
            _ => panic!("expected Bound"),
        }
    }

    #[test]
    fn wrong_missing_or_unknown_credentials_are_rejected_with_the_cause() {
        let v = demo_verifier();
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/demo", "wrong-token"))),
            RejectionCause::BadToken
        );
        // Missing token: the SDK cannot omit the argument (the signature has
        // no null string), so "missing" arrives as empty bytes.
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/demo", ""))),
            RejectionCause::BadToken
        );
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/ghost", TOKEN))),
            RejectionCause::UnknownIdentity
        );
        // Same-length-as-decoy credential for an unknown identity must not
        // accidentally match the decoy.
        let decoy_len = String::from_utf8(DECOY_TOKEN.to_vec()).unwrap();
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/ghost", &decoy_len))),
            RejectionCause::UnknownIdentity
        );
        let mut wrong_scheme = present("vitrin://local/agent/demo", TOKEN);
        wrong_scheme.credential_type = "spiffe-jwt-svid";
        assert_eq!(
            expect_rejected(v.verify(&wrong_scheme)),
            RejectionCause::UnsupportedScheme {
                presented: "spiffe-jwt-svid".into()
            }
        );
    }

    #[test]
    fn peer_uid_policy_is_a_hard_refusal() {
        // Global policy: the verifier requires a uid the peer does not have.
        let other_uid = my_uid().wrapping_add(1);
        let v = StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            }],
            other_uid,
        )
        .unwrap();
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/demo", TOKEN))),
            RejectionCause::PeerCredMismatch {
                required_uid: other_uid,
                peer_uid: my_uid(),
            }
        );

        // With a (satisfiable) pin present, a refusal cites the anchor the
        // peer actually failed -- the log can never render the
        // self-contradictory "peer uid N, required N".
        let v = StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: Some(other_uid),
            }],
            other_uid,
        )
        .unwrap();
        assert_eq!(
            expect_rejected(v.verify(&present("vitrin://local/agent/demo", TOKEN))),
            RejectionCause::PeerCredMismatch {
                required_uid: other_uid,
                peer_uid: my_uid(),
            }
        );
    }

    #[test]
    fn a_pin_matching_the_anchor_loads_and_binds() {
        let v = StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: demo_identity(),
                token: TOKEN.as_bytes().to_vec(),
                uid: Some(my_uid()),
            }],
            my_uid(),
        )
        .unwrap();
        assert!(matches!(
            v.verify(&present("vitrin://local/agent/demo", TOKEN)),
            VerifyOutcome::Bound(_)
        ));
    }

    #[test]
    fn an_unsatisfiable_uid_pin_is_refused_at_load() {
        // A pin differing from the required uid can never be satisfied
        // (the verify-time policy is a conjunction): loading must fail
        // loudly instead of carrying a row that silently never binds.
        let rows = vec![StaticPrincipal {
            identity: demo_identity(),
            token: TOKEN.as_bytes().to_vec(),
            uid: Some(my_uid().wrapping_add(1)),
        }];
        match StaticVerifier::from_rows(rows, my_uid()) {
            Err(RegistryError::Invalid { detail }) => {
                assert!(
                    detail.contains("unsatisfiable"),
                    "unexpected detail: {detail}"
                )
            }
            other => panic!("an unsatisfiable pin must be refused, got {other:?}"),
        }
    }

    #[test]
    fn rejection_causes_never_carry_credential_bytes() {
        // The Display of every cause reachable with a live credential must
        // not echo the credential -- the log-hygiene rule made testable.
        let v = demo_verifier();
        let secret = "super-secret-credential-bytes";
        for claimed in ["vitrin://local/agent/demo", "vitrin://local/agent/ghost"] {
            let cause = expect_rejected(v.verify(&present(claimed, secret)));
            assert!(!cause.to_string().contains(secret));
        }
    }
}
