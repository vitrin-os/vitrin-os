// SPDX-License-Identifier: MPL-2.0
//! The realm object and the realm registry (P1.5.1, issue #30): the core's
//! model of *what a realm is*, loaded from `realm.toml` at startup.
//!
//! A realm is an **addressing scope** (IDL, `vitrin_realm`: "grants attach
//! to realms, and apps launch into realms ... deliberately authority-free")
//! plus the ownership of one confined app. Concretely, one [`Realm`] here:
//!
//! - carries the stable [`RealmId`] the wire names (`get_realm("realm-0")`)
//!   and every grant row keys on ([`crate::grants`]'s `realm_id` column);
//!   the MVP's whole-realm grant scope, `realm:realm-0`, is this id;
//! - **owns** the spawn configuration of its app ([`SpawnConfig`]) without
//!   executing it -- launching is P1.5.2 (issue #31), lifecycle and crash
//!   detection are P1.5.3 (issue #32);
//! - is the subject of the composited realm view ([`crate::scene`], P1.3.3)
//!   that capture reads, and of the chokepoint's `no_surface` judgement
//!   when that view is not live ([`crate::enforcement`]).
//!
//! [`RealmRegistry`] is the **single source of truth for realm existence**.
//! Before this task, [`crate::petitions`] answered "does this realm exist?"
//! with a hardcoded comparison against the well-known name; it now asks the
//! registry, so the answer comes from configuration and from realm state
//! rather than from a constant. Version 0 holds exactly one realm, but the
//! registry is a keyed collection, so Phase 3's fleet adds rows and
//! lifecycle events rather than replacing the addressing model -- exactly
//! the IDL's reason for putting realm ids on the wire from day one.
//!
//! # `realm.toml` schema (decided here, per issue #30)
//!
//! An array of `[[realm]]` tables in the core's strict TOML subset
//! ([`crate::toml_subset`] -- the same dialect as `principals.toml`; see
//! `examples/realm.toml` for a commented template). Version 0 requires
//! **exactly one** table. Keys:
//!
//! | key | type | required | meaning |
//! |---|---|---|---|
//! | `id` | string | no (default [`WELL_KNOWN_REALM_ID`]) | the realm's stable, wire-visible name: what `vitrin_principal.get_realm` addresses, what grant rows state, and the directory name of the realm's private runtime tree. Version 0 accepts **only** `"realm-0"` -- see below |
//! | `command` | string | **yes** | absolute path of the program the realm launches (P1.5.2 execs it); audited at load, see below |
//! | `args` | array of strings | no (default `[]`) | arguments **after** `argv[0]`; the core supplies `argv[0]` itself from `command` |
//! | `env_allow` | array of strings | no (default `[]`) | names of environment variables passed through from the core's own environment; see below |
//!
//! ## The realm's name is the IDL's, not the operator's (version 0)
//!
//! `id` is in the schema, but version 0 refuses every value but
//! [`WELL_KNOWN_REALM_ID`]. The IDL settles this: `get_realm`'s description
//! declares `"realm-0"` **the single well-known realm of version 1** -- the
//! one realm name a conformant client of this protocol version can know
//! without being told. A session that renamed its only realm would still be
//! *structurally* legal (`get_realm` always mints a handle, and an unknown
//! name resolves `unavailable` at petition time), but every conformant
//! client would petition `realm-0` and be told, forever, that the realm is
//! absent. The IDL specifies that absence as a **race** against realm
//! lifecycle, not as a permanent property of a correctly configured
//! session, so a core that manufactured it from a config key would be lying
//! in the protocol's own vocabulary.
//!
//! This is the same shape as the [`MAX_REALMS`] rule, and it lives in the
//! same place for the same reason: a *validator* limit on what version-0
//! configuration may declare, never a narrowing of the model underneath.
//! [`RealmRegistry`] stays a keyed lookup over whatever it holds
//! ([`RealmRegistry::resolve_for_petition`] does a real map lookup, not a
//! constant comparison), so the phase that puts operator-chosen realm names
//! on the wire deletes this one check and the schema, the registry, and the
//! addressing path do not move. Widening it sooner is a **protocol** change
//! first: the IDL description has to stop naming `realm-0` as *the*
//! version-1 realm before a loader may.
//!
//! ## The environment allowlist: names, not pairs, and default-deny
//!
//! `env_allow` lists variable **names** whose values are copied from the
//! core's environment into the app's. It is deliberately *not* a
//! `name = "value"` map, and version 0 has no key that is:
//!
//! - **Default-deny is the only safe default for a TCB-spawned child.** The
//!   core's environment is a session environment: it holds the host
//!   compositor's socket, the session bus address, agent-forwarding sockets,
//!   and whatever the operator's shell exported. Inheriting it wholesale
//!   would hand a confined app the ambient authority the confinement exists
//!   to remove. An **absent or empty `env_allow` therefore means an empty
//!   inherited environment** -- the app starts with nothing but what the
//!   core injects for it (P1.5.2 sets the realm's own `WAYLAND_DISPLAY`,
//!   pointing at the shim's private socket, plus its runtime directory).
//!   That is a real, working configuration, not a degenerate one.
//! - **Literal `name = "value"` pairs would invite exactly the
//!   re-litigation this design forbids.** P1.5.2 scrubs the host's display
//!   variables unconditionally, because a confined app that can see the
//!   host display server is not confined; a config file able to set
//!   arbitrary values could set them back and quietly void the confinement.
//!   Passing *names* narrows that hole but does **not** dissolve it, and
//!   the difference matters: a name whose value in the core's environment
//!   *is itself* a host connection re-opens it exactly. `WAYLAND_SOCKET`
//!   holds the number of an already-connected file descriptor, and
//!   `XAUTHORITY` the credential that authenticates one -- neither is a
//!   display *name*, so no amount of name-vs-pair discipline helps. The
//!   guard is therefore an explicit list, [`RESERVED_ENV`], refused at
//!   load, loudly, naming the variable and why; its membership rule is "the
//!   core decides this variable, **or** this variable is a way to reach the
//!   host display server", not "this variable looks like a display name".
//!   (A pass-through name whose value is merely attacker-*influenced* is
//!   the operator's own environment -- the same trust as the config file
//!   itself. Authority is the line, not influence.)
//! - Explicit pairs are a plausible future convenience and stay purely
//!   additive: a later `env_set` key can arrive without changing the
//!   meaning of `env_allow`.
//!
//! A name in `env_allow` that is unset in the core's environment is simply
//! not passed -- not an error. Config validity must not depend on ambient
//! environment, or the same file would load on one machine and fail on
//! another; `HOME` being unset is a property of the run, not of the file.
//! Name *shape* is validated ([`is_env_name`]): a POSIX portable name, so a
//! typo like `"LANG=en_US"` is refused rather than becoming an
//! unsettable variable.
//!
//! # Validation strictness (decided here): refuse, never default
//!
//! Beyond the subset's own grammar, loading refuses:
//!
//! - **an unknown key** -- a security-relevant config whose typo'd key
//!   silently does nothing (`env_alow`) is a fail-open trap that reads like
//!   success;
//! - **a missing or empty `command`** -- a realm with no app is not a realm
//!   with a default app, and there is no sane program to guess;
//! - **a relative `command`** -- resolution through `PATH` would make what
//!   the TCB executes depend on ambient environment, and the child's
//!   environment is default-deny, so a relative name would silently resolve
//!   against the *core's* `PATH` instead of anything the operator can see in
//!   this file. An absolute path is the only spelling that means one thing;
//! - **a `command` that does not resolve, or that a wider set of people
//!   than root and the core can replace** -- the transitive half of the
//!   not-writable policy below;
//! - **zero or more than one `[[realm]]` table** -- version 0 serves exactly
//!   one realm. This is a *cardinality* rule in the validator, not a
//!   grammar limit: the file format is already an array of tables, so Phase
//!   3 raises [`MAX_REALMS`] and the schema does not move;
//! - **an `id` that is not the IDL's well-known realm name** -- version 0's
//!   counterpart of the cardinality rule, argued above;
//! - **a duplicate key, a duplicate realm id, a duplicate or reserved
//!   `env_allow` name, or an ill-shaped id** -- each names the offending
//!   text and why.
//!
//! Every refusal is a hard startup error carrying the file path, the line
//! where a line is meaningful, and the specific problem. A core that comes
//! up with a realm the operator did not describe is worse than a core that
//! does not come up.
//!
//! # File permissions (decided here): not-writable, unlike `principals.toml`
//!
//! `realm.toml` holds no secrets, so it is *not* held to the
//! secret-material posture [`crate::identity`] applies to `principals.toml`
//! (which refuses any group/other access because it holds bearer tokens).
//! It is held to a different one for a different reason: this file names
//! the program the TCB will execute, so **whoever can write it chooses what
//! the trusted core spawns**. Loading therefore refuses a file that is not
//! a regular file, is not owned by the core's own effective uid, or is
//! writable by group or other (any `0o022` bit). World-*readable* is fine
//! -- a command line is not secret. The check runs on the opened fd
//! (`fstat`), so there is no stat-then-open TOCTOU window.
//!
//! ## ... and the same rule for the program it names
//!
//! That sentence stays true one link further out: of the program `command`
//! names, and of every directory on the way to it. A `realm.toml` hardened
//! to `0600` while `command` points at a group-writable binary hands the
//! very authority the file check was protecting to a wider set of people
//! -- and *reads* like a guarantee while doing it, which is the worse half.
//! The check must be transitive or it is decorative, so loading applies it
//! transitively ([`audit_spawn_target`]): the program, resolved through
//! symlinks, must be a regular file, and it and every directory on its
//! canonical path must be owned by root or by the core's own euid and not
//! writable by group or other. A world-writable *directory* passes only
//! with the sticky bit set, which is precisely the property that makes a
//! `/tmp`-style path unswappable by a non-owner.
//!
//! Root counts as a trusted writer here where it does not for `realm.toml`
//! itself, because the normal case *is* a distribution binary: `/usr/bin`
//! is root-owned, and a rule that demanded the core's own uid would refuse
//! every ordinary configuration and teach operators to work around it.
//!
//! This is a **startup** audit and deliberately not the last word. It
//! proves the operator's configuration is coherent and names the exact path
//! and fault when it is not; it cannot prove anything about the instant of
//! `execve`. That is P1.5.2's (issue #31) to hold, and it needs its own
//! check on the descriptor it will actually run -- not a re-resolution of
//! this name, which is the TOCTOU this one cannot close.
//!
//! # Vacancy: what version 0 answers, and where P1.5.2/P1.5.3 flip it
//!
//! The IDL is precise: `get_realm` always succeeds structurally, and "a name
//! that is unknown **or vacant** yields a handle whose petitions resolve
//! `unavailable` -- realm absence is a race, not a protocol error". So the
//! core must answer, at petition time, one question: does this name denote a
//! realm that can be petitioned for?
//!
//! **Version 0's answer: a configured realm is petitionable, whether or not
//! its app is running.** Unknown and vacant coincide today -- a name resolves
//! to a realm or it does not. The reasoning:
//!
//! - A realm *is* its addressing scope plus its spawn ownership, and both
//!   exist the moment the config loads. Grants attach to realms, not to
//!   processes; `realm:realm-0` is meaningful before the app starts and
//!   stays meaningful across a restart, which is precisely why realm ids and
//!   process lifetimes are separate concepts.
//! - Petition time is an **addressing** question. Whether anything is on
//!   screen is a **use-time** question, and the enforcement chokepoint
//!   already answers it, refusing `no_surface` when the realm view is not
//!   live (P1.4.4). Answering "unavailable" at petition time for a realm
//!   that exists would move a liveness fact into an addressing answer and
//!   tell the agent the realm is *absent* when it is merely *not yet
//!   painted*. [`crate::petitions`] already recorded this split.
//! - Fail-closed is preserved by the layer that owns it: an agent may hold a
//!   grant over a realm whose app never starts, and every use of it refuses
//!   `no_surface`. Authority without a target is inert, not dangerous.
//!
//! **The flip point is one predicate**, [`Realm::admits_petitions`], and it
//! stayed the only thing P1.5.2/P1.5.3 had to touch. P1.5.2 added
//! [`RealmState::Running`] (still `true`: a launched app that has not
//! painted is unpainted, not absent) and P1.5.3 added
//! [`RealmState::Exited`], which is where the answer finally flips to
//! `false` -- a one-arm change in one function, no signature, no caller and
//! no wire behavior moving. [`crate::petitions`] asks
//! [`RealmRegistry::resolve_for_petition`], the registry's only
//! petition-time entry point, which already routed through the predicate.
//!
//! The `Exited` arm is argued in full at the predicate itself. In one line:
//! `no_surface` means *not right now* and `unavailable` means *not ever*,
//! and a realm whose shim is gone in a build with no restart policy is the
//! second one.
//!
//! # Where the config path comes from
//!
//! `vitrind --realm PATH` (the `--consent` / `--recorder` spelling: both
//! `--realm PATH` and `--realm=PATH`), defaulting to
//! `$XDG_CONFIG_HOME/vitrin/realm.toml` -- falling back, per the XDG base
//! directory specification, to `$HOME/.config/vitrin/realm.toml`. That is
//! the same directory `principals.toml` is conventionally read from.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::grants::RealmId;
use crate::toml_subset::{self, SubsetError};

/// The realm name the IDL fixes for version 1: `get_realm`'s description
/// declares `"realm-0"` "the single well-known realm of version 1". Both
/// the default when `realm.toml` omits `id` and -- in version 0 -- the only
/// value it may carry (module docs: a session serving some other name
/// answers every conformant client's `get_realm("realm-0")` petition
/// `unavailable`, forever).
///
/// A *validator* constraint, not a model one: [`RealmRegistry`] serves
/// whatever it holds and looks names up in a map, which is what keeps the
/// petition-time existence check a real lookup rather than a constant
/// comparison wearing a lookup's clothes -- and what makes the later
/// multi-realm phase a deletion here rather than a re-plumbing.
pub(crate) const WELL_KNOWN_REALM_ID: &str = "realm-0";

/// Conventional file name under the core's configuration directory.
pub(crate) const CONFIG_FILE_NAME: &str = "realm.toml";

/// How many realms version 0 serves. The cardinality rule the loader
/// enforces; Phase 3's fleet raises it without touching the schema.
pub(crate) const MAX_REALMS: usize = 1;

/// Environment variable names `env_allow` may not carry, each paired with
/// the reason its refusal states. Two kinds, one rule: **configuration does
/// not get a vote on whether the realm's app can reach the host display
/// server.**
///
/// - Variables the core *injects* for the realm (`WAYLAND_DISPLAY`,
///   `XDG_RUNTIME_DIR`): a pass-through is either silently overwritten --
///   config that reads as if it did something -- or, depending on which
///   side of the merge wins, overwrites the injection and points the app at
///   the host session.
/// - Variables that *are* a host connection (`DISPLAY`, `WAYLAND_SOCKET`,
///   `XAUTHORITY`): an address, an already-open socket, or the credential
///   that authenticates one.
///
/// `WAYLAND_SOCKET` is the entry that motivates spelling the rule out.
/// `wl_display_connect()` reads it *before* `WAYLAND_DISPLAY` and before
/// its own `name` argument, and its value is not a display name but the
/// number of an already-connected file descriptor. In nested mode the core
/// is itself a Wayland client of the host compositor, so that variable is
/// exactly what the core's own environment holds -- passing it through
/// would hand the confined app a live connection to the host compositor,
/// and the realm's private socket would simply never be consulted. A list
/// that stopped at display *names* would let one config key undo the
/// confinement the rest of this file exists to guarantee.
pub(crate) const RESERVED_ENV: [(&str, &str); 5] = [
    (
        "DISPLAY",
        "it addresses the host X server, which the core scrubs at spawn",
    ),
    (
        "WAYLAND_DISPLAY",
        "it addresses a Wayland server: the core scrubs the host value and injects the \
         realm's own private socket at spawn",
    ),
    (
        "WAYLAND_SOCKET",
        "libwayland reads it before WAYLAND_DISPLAY and treats its value as an \
         already-connected file descriptor, so passing it through would hand the app a \
         live connection to the host compositor",
    ),
    (
        "XAUTHORITY",
        "it is the credential file authenticating a connection to the host X server",
    ),
    (
        "XDG_RUNTIME_DIR",
        "it names the host session's private runtime directory (host compositor socket, \
         session bus, agent sockets); the core injects the realm's own runtime directory \
         at spawn",
    ),
];

/// Why `env_allow` may not carry `name`, or `None` if it may. Matching is
/// exact and case-sensitive, as environment lookup itself is: `getenv`
/// finds `DISPLAY`, never `display`.
fn reserved_env_reason(name: &str) -> Option<&'static str> {
    RESERVED_ENV
        .iter()
        .find(|(reserved, _)| *reserved == name)
        .map(|(_, why)| *why)
}

/// What one realm will launch: owned by the realm from load time,
/// **executed by nobody in this build** (P1.5.2, issue #31, owns fork/exec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnConfig {
    /// Absolute path of the program to execute.
    command: PathBuf,
    /// Arguments after `argv[0]`; the core supplies `argv[0]` from
    /// [`command`](Self::command).
    args: Vec<String>,
    /// Names passed through from the core's environment (module docs:
    /// names, not pairs; empty means an empty inherited environment).
    env_allow: Vec<String>,
}

impl SpawnConfig {
    pub fn command(&self) -> &Path {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn env_allow(&self) -> &[String] {
        &self.env_allow
    }

    /// The inherited half of the app's environment: each allow-listed name
    /// that `lookup` resolves, in allowlist order. Names the lookup does
    /// not resolve are skipped (module docs: an unset variable is a
    /// property of the run, not a config error).
    ///
    /// `lookup` is a parameter rather than a direct `std::env` read so the
    /// semantics are testable without mutating process state; P1.5.2 calls
    /// it with `std::env::var`, then adds the variables the core injects
    /// and scrubs [`RESERVED_ENV`] -- which cannot appear here, because
    /// loading refused them.
    pub fn inherited_env<F>(&self, lookup: F) -> Vec<(String, String)>
    where
        F: Fn(&str) -> Option<String>,
    {
        self.env_allow
            .iter()
            .filter_map(|name| lookup(name).map(|value| (name.clone(), value)))
            .collect()
    }
}

/// A realm's lifecycle state. [`Realm::admits_petitions`] is where every
/// variant decides vacancy (module docs), and it is the only place that has
/// to change when a variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RealmState {
    /// Described by `realm.toml` and addressable; no app process yet.
    Configured,
    /// The realm's shim has been forked and `exec`ed ([`crate::spawn`]):
    /// the core holds its end of the identity socketpair and this pid is
    /// the shim. Says nothing about whether the app has painted -- that
    /// stays the chokepoint's `no_surface` judgement, not a realm state.
    Running { pid: u32 },
    /// **Terminal** (P1.5.3): the realm's shim is gone -- it crashed, its
    /// connection died, or shutdown tore it down -- and nothing will bring
    /// it back. `pid` is the process that was serving the realm, retained
    /// so the log and a reader can tie this state to the `realm_died`
    /// entry; it names a process that no longer exists.
    ///
    /// Terminal is a statement about *this build*, not about realms: the
    /// MVP has no restart policy by decision (supervision is a later
    /// concern), so "the shim died" and "the realm is over" coincide. When
    /// supervision arrives it adds a `Restarting` variant beside this one
    /// and answers the predicate below for itself; nothing else moves.
    Exited { pid: u32 },
}

/// One realm: its wire-visible identity, the app it owns, and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Realm {
    id: RealmId,
    spawn: SpawnConfig,
    state: RealmState,
}

impl Realm {
    /// The stable, wire-visible id: what `get_realm` addresses and what
    /// grant rows state.
    pub fn id(&self) -> &RealmId {
        &self.id
    }

    /// The app this realm owns (P1.5.2 executes it).
    pub fn spawn(&self) -> &SpawnConfig {
        &self.spawn
    }

    pub fn state(&self) -> RealmState {
        self.state
    }

    /// **The vacancy predicate** (module docs): may a petition naming this
    /// realm be admitted, or is the realm vacant and its petitions bound to
    /// resolve `unavailable`? The single place realm state becomes a
    /// petition-time answer -- P1.5.2/P1.5.3 add their state arms here and
    /// nothing else moves.
    pub fn admits_petitions(&self) -> bool {
        match self.state {
            // Configured but not yet running is addressable, not vacant: a
            // grant over a realm whose app has not painted is inert at the
            // enforcement chokepoint (`no_surface`), never a lie about the
            // realm's existence.
            RealmState::Configured => true,
            // Running is the same answer for the same reason, one step
            // further along: an app that has started but not yet committed
            // a surface is still merely unpainted. The petition-time answer
            // only changes when a realm can no longer be served at all,
            // which is the terminal state below.
            RealmState::Running { .. } => true,
            // **Vacant** (P1.5.3, the flip this predicate was written for).
            // This is the one state where "the realm cannot be served"
            // stops being a liveness fact and becomes an addressing one:
            // with no restart policy in the MVP, a realm whose shim is gone
            // is gone for the rest of the session, which is precisely the
            // IDL's `vacant` -- "a name that is unknown *or vacant* yields a
            // handle whose petitions resolve `unavailable`".
            //
            // It is not a contradiction of the "unpainted is not absent"
            // rule above; it is the same rule's other side. `no_surface`
            // says *not right now*, and an agent that hears it may sensibly
            // retry. `unavailable` says *not ever*, and an agent that hears
            // it should stop asking. Answering a petition for a dead realm
            // with a live handle would park a consent prompt in front of a
            // human about authority over nothing, and then hold the
            // petitioner's own actuations under `consent_held` while it
            // waited (the chokepoint's step 5b) -- a prompt that cannot
            // matter, blocking a client that cannot be served.
            //
            // Fail-closed is unaffected in the other direction: grants
            // already issued over this realm keep refusing `no_surface` at
            // the chokepoint. Nothing here widens authority, and use-time
            // liveness stays exactly where it was.
            RealmState::Exited { .. } => false,
        }
    }
}

/// The core's realm registry: **the** answer to realm existence, and the
/// owner of every realm's spawn configuration. One instance per core
/// process, beside the one grant table and the one petition registry.
///
/// Version 0 holds exactly one realm ([`MAX_REALMS`]); the map keying is
/// what makes Phase 3's multiplicity additive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealmRegistry {
    /// Keyed by id, so lookup is by the name the wire carries and
    /// enumeration is deterministic.
    realms: BTreeMap<RealmId, Realm>,
}

impl RealmRegistry {
    /// Read, validate, and load `realm.toml` at `path`. Every error names
    /// the path (module docs); startup aborts on any of them.
    pub fn load(path: &Path) -> Result<Self, RealmConfigError> {
        let at = |kind: ErrorKind| RealmConfigError {
            path: path.to_path_buf(),
            kind,
        };
        let mut file = fs::File::open(path).map_err(|e| at(ErrorKind::Io(e)))?;
        check_config_security(&file).map_err(at)?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| at(ErrorKind::Io(e)))?;
        let specs = parse_config(&text).map_err(at)?;
        // The transitive half of the not-writable policy (module docs), and
        // the reason it lives *here* rather than in `parse_config`: what a
        // path means is a fact about this filesystem at this moment, not
        // about the file's text. Keeping it beside `check_config_security`
        // -- the other question only a real filesystem can answer -- leaves
        // parsing pure and total.
        for spec in &specs {
            audit_spawn_target(spec.spawn.command()).map_err(at)?;
        }
        Self::from_specs(specs).map_err(at)
    }

    /// Build a registry from parsed tables, enforcing the cross-table
    /// invariants (cardinality, unique ids). Shared by [`load`](Self::load)
    /// and tests, so no constructor can bypass them.
    ///
    /// Deliberately *not* the filesystem audit of each realm's `command`:
    /// that one needs a real filesystem, so it belongs to loading (module
    /// docs), and a caller synthesizing specs in a test is not describing a
    /// program this core will exec.
    pub fn from_specs(specs: Vec<RealmSpec>) -> Result<Self, ErrorKind> {
        if specs.is_empty() {
            return Err(ErrorKind::Invalid(
                "no [[realm]] table: the core has nothing to serve".into(),
            ));
        }
        if specs.len() > MAX_REALMS {
            return Err(too_many_realms(specs.len()));
        }
        let mut realms = BTreeMap::new();
        for spec in specs {
            let realm = Realm {
                id: spec.id.clone(),
                spawn: spec.spawn,
                state: RealmState::Configured,
            };
            if realms.insert(spec.id.clone(), realm).is_some() {
                return Err(ErrorKind::Invalid(format!(
                    "duplicate realm id {:?}",
                    spec.id.as_str()
                )));
            }
        }
        Ok(Self { realms })
    }

    /// **The petition-time existence query** ([`crate::petitions`]'s
    /// `unavailable` judgement): the realm id a wire name denotes, or
    /// `None` when the name is unknown *or* the realm is vacant -- the two
    /// cases the IDL deliberately makes indistinguishable to the client.
    /// Routed through [`Realm::admits_petitions`], so realm state joins the
    /// answer in P1.5.2/P1.5.3 without a new call site.
    pub fn resolve_for_petition(&self, name: &str) -> Option<&RealmId> {
        self.realms
            .get(&RealmId::new(name))
            .filter(|realm| realm.admits_petitions())
            .map(Realm::id)
    }

    /// The realm with this id, whatever its state (the spawn manager's
    /// lookup; petitions use [`resolve_for_petition`](Self::resolve_for_petition)).
    pub fn get(&self, name: &str) -> Option<&Realm> {
        self.realms.get(&RealmId::new(name))
    }

    /// Record that this realm's app is running under `pid`
    /// ([`crate::spawn`] has forked and `exec`ed its shim). Returns whether
    /// the realm existed.
    ///
    /// State transitions live on the registry rather than on [`Realm`] so
    /// there is one owner of realm state, the way there is one grant table
    /// -- [`mark_exited`](Self::mark_exited) is its sibling.
    pub fn mark_running(&mut self, id: &RealmId, pid: u32) -> bool {
        match self.realms.get_mut(id) {
            Some(realm) => {
                realm.state = RealmState::Running { pid };
                true
            }
            None => false,
        }
    }

    /// Record that this realm's shim is gone for good (P1.5.3): the realm
    /// stops admitting petitions from here on ([`Realm::admits_petitions`]).
    /// Returns whether the realm existed.
    ///
    /// Deliberately **not** idempotence-enforcing: this writes a terminal
    /// state, so writing it twice writes the same thing, and a registry
    /// that refused the second write would be a second place death is
    /// counted. Idempotence -- "one death produces one log entry, one
    /// scene clear, one transition" -- is [`crate::lifecycle`]'s single
    /// latch, and it is the only caller.
    pub fn mark_exited(&mut self, id: &RealmId, pid: u32) -> bool {
        match self.realms.get_mut(id) {
            Some(realm) => {
                realm.state = RealmState::Exited { pid };
                true
            }
            None => false,
        }
    }

    /// Every realm, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Realm> {
        self.realms.values()
    }

    pub fn len(&self) -> usize {
        self.realms.len()
    }
}

/// One validated `[[realm]]` table, before it becomes a [`Realm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealmSpec {
    pub id: RealmId,
    pub spawn: SpawnConfig,
}

/// `$XDG_CONFIG_HOME/vitrin/realm.toml`, falling back to
/// `$HOME/.config/vitrin/realm.toml` (XDG base directory specification).
/// A core that cannot name its configuration directory cannot find its
/// realm either, so this is an error rather than a silent relative path.
pub(crate) fn default_config_path() -> Result<PathBuf, ConfigPathError> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

/// `$XDG_CONFIG_HOME/vitrin/principals.toml` -- the default the
/// `--principals` flag falls back to.
///
/// Lives here rather than in [`crate::identity`] so that **one** function
/// resolves the core's configuration directory: two resolvers would be two
/// chances to disagree about where an operator's files are, and a session
/// that read its realm from one directory and its principal registry from
/// another would be a security surprise, not a convenience.
pub(crate) fn default_principals_path() -> Result<PathBuf, ConfigPathError> {
    Ok(config_dir()?.join(crate::identity::REGISTRY_FILE_NAME))
}

/// `$XDG_CONFIG_HOME/vitrin` (or `$HOME/.config/vitrin`) -- also where
/// `principals.toml` lives ([`default_principals_path`]).
fn config_dir() -> Result<PathBuf, ConfigPathError> {
    // Per the XDG spec, a relative $XDG_CONFIG_HOME must be ignored:
    // honoring one would make the config location depend on the cwd
    // vitrind happened to be started from.
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir.join("vitrin"));
        }
    }
    let home = std::env::var_os("HOME").ok_or(ConfigPathError::NoConfigHome)?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(ConfigPathError::NoConfigHome);
    }
    Ok(home.join(".config").join("vitrin"))
}

/// Why the default configuration path could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigPathError {
    /// Neither an absolute `$XDG_CONFIG_HOME` nor an absolute `$HOME`.
    NoConfigHome,
}

impl fmt::Display for ConfigPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigPathError::NoConfigHome => write!(
                f,
                "neither $XDG_CONFIG_HOME nor $HOME names an absolute directory"
            ),
        }
    }
}

impl std::error::Error for ConfigPathError {}

/// Why loading `realm.toml` failed -- always a hard startup error, always
/// naming the file. The core must not come up with a realm the operator
/// did not describe.
#[derive(Debug)]
pub(crate) struct RealmConfigError {
    pub path: PathBuf,
    pub kind: ErrorKind,
}

/// The problem itself, path-free so the pure constructors can produce it.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    /// Filesystem failure opening or reading the file (including "no such
    /// file": a core with no realm config has nothing to serve).
    Io(io::Error),
    /// The file's type, ownership, or mode fails the not-writable policy.
    Insecure(String),
    /// The file is outside the strict TOML subset, or a value at a known
    /// line is malformed.
    Parse { line: usize, detail: String },
    /// The tables parse but violate a schema or cross-table rule.
    Invalid(String),
}

impl From<SubsetError> for ErrorKind {
    fn from(e: SubsetError) -> Self {
        ErrorKind::Parse {
            line: e.line,
            detail: e.detail,
        }
    }
}

impl fmt::Display for RealmConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "realm config {}: {}", self.path.display(), self.kind)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Io(e) => write!(f, "i/o error: {e}"),
            ErrorKind::Insecure(detail) => write!(f, "refused: {detail}"),
            ErrorKind::Parse { line, detail } => write!(f, "parse error at line {line}: {detail}"),
            ErrorKind::Invalid(detail) => write!(f, "invalid: {detail}"),
        }
    }
}

impl std::error::Error for RealmConfigError {}

/// Refuse a config file that is not a regular file, is not owned by the
/// core's euid, or is group/other **writable** -- whoever can write this
/// file chooses what the trusted core executes (module docs). Checked on
/// the opened fd, so no stat-then-open TOCTOU.
fn check_config_security(file: &fs::File) -> Result<(), ErrorKind> {
    let st = rustix::fs::fstat(file).map_err(|e| ErrorKind::Io(e.into()))?;
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(ErrorKind::Insecure("not a regular file".into()));
    }
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(ErrorKind::Insecure(format!(
            "owned by uid {}, not the core's uid {euid}; it names the program the \
             trusted core will execute",
            st.st_uid
        )));
    }
    let mode = st.st_mode & 0o777;
    if mode & 0o022 != 0 {
        return Err(ErrorKind::Insecure(format!(
            "mode {mode:03o} is writable by group/other; whoever can write it chooses \
             what the trusted core spawns -- chmod go-w it"
        )));
    }
    Ok(())
}

/// Refuse a `command` that a wider set of people than root and the core can
/// replace -- the not-writable policy applied transitively, one link past
/// `realm.toml` itself (module docs). Checks the program *and* every
/// directory on its canonical path, because a writable directory anywhere
/// on the way is a swap of the program by another name.
///
/// Resolving through symlinks first is what makes walking the ancestors
/// meaningful: a lexical walk of `/opt/app` would never look at the
/// directory a symlinked `/opt` actually points into. The resolved path is
/// used for the audit only -- the realm still execs the path the operator
/// wrote, because `argv[0]` is observable to the program (busybox-style
/// multi-call binaries dispatch on it) and rewriting it would change what
/// runs to something the file does not say.
fn audit_spawn_target(command: &Path) -> Result<(), ErrorKind> {
    let resolved = fs::canonicalize(command).map_err(|e| {
        ErrorKind::Invalid(format!(
            "`command` {} does not resolve to a program ({e}); the core will not start \
             with a realm whose app it cannot audit",
            command.display()
        ))
    })?;
    let euid = rustix::process::geteuid().as_raw();

    let st = rustix::fs::stat(&resolved).map_err(|e| ErrorKind::Io(e.into()))?;
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(ErrorKind::Insecure(format!(
            "`command` {} is not a regular file",
            resolved.display()
        )));
    }
    if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, false) {
        return Err(ErrorKind::Insecure(format!(
            "`command` {} is {fault}; whoever can write the program the trusted core \
             execs chooses what it runs -- the rule this config file is held to, one \
             link further out",
            resolved.display()
        )));
    }

    // Skip(1): `ancestors` yields the program itself first, then each
    // enclosing directory up to `/`.
    for dir in resolved.ancestors().skip(1) {
        let st = rustix::fs::stat(dir).map_err(|e| ErrorKind::Io(e.into()))?;
        if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, true) {
            return Err(ErrorKind::Insecure(format!(
                "`command` {}: directory {} is {fault}; whoever can write a directory on \
                 the path can swap the program the trusted core execs",
                resolved.display(),
                dir.display()
            )));
        }
    }
    Ok(())
}

/// Can anyone but root or the core replace this filesystem object? Returns
/// the fault to report, or `None` when only a trusted writer can.
///
/// `sticky_tolerated` is for directories: a world-writable directory with
/// the sticky bit (`/tmp`, mode `1777`) only lets a writer create and
/// remove entries *it owns*, so it cannot be used to swap someone else's
/// program -- and the ownership half of this same check still binds on
/// every component. Files get no such exemption; the bit means something
/// else entirely on a regular file.
///
/// Shared with [`crate::spawn`], which re-applies exactly this rule to the
/// descriptor it is about to `exec` (module docs: the startup audit cannot
/// speak for the instant of `execve`). One definition, two call sites --
/// two copies of "who may replace this" would eventually disagree, and the
/// quieter copy would be the one that matters.
pub(crate) fn untrusted_writer(
    uid: u32,
    raw_mode: u32,
    euid: u32,
    sticky_tolerated: bool,
) -> Option<String> {
    if uid != 0 && uid != euid {
        return Some(format!(
            "owned by uid {uid}, neither root nor the core's uid {euid}"
        ));
    }
    let mode = raw_mode & 0o7777;
    let sticky = mode & 0o1000 != 0;
    if mode & 0o022 != 0 && !(sticky_tolerated && sticky) {
        return Some(format!("mode {mode:04o}, writable by group/other"));
    }
    None
}

/// One `[[realm]]` table under construction, with the line each value came
/// from so a validation refusal can point at it.
#[derive(Default)]
struct RawRealm {
    id: Option<(String, usize)>,
    command: Option<(String, usize)>,
    args: Option<Vec<String>>,
    env_allow: Option<(Vec<String>, usize)>,
}

/// Parse the strict TOML subset into validated specs. Anything outside the
/// documented schema is an error, never a guess (module docs).
fn parse_config(text: &str) -> Result<Vec<RealmSpec>, ErrorKind> {
    let parse_err = |line: usize, detail: String| ErrorKind::Parse { line, detail };
    let mut raw: Vec<RawRealm> = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let (header, rest) = toml_subset::table_header(line)
                .ok_or_else(|| parse_err(line_no, "malformed table header".into()))?;
            if !matches!(rest.trim_start().chars().next(), None | Some('#')) {
                return Err(parse_err(
                    line_no,
                    "trailing content after table header".into(),
                ));
            }
            if header != "[[realm]]" {
                return Err(parse_err(
                    line_no,
                    "only [[realm]] tables are allowed in this file".into(),
                ));
            }
            raw.push(RawRealm::default());
            continue;
        }
        let Some(realm) = raw.last_mut() else {
            return Err(parse_err(
                line_no,
                "key outside any [[realm]] table (top-level keys are not allowed)".into(),
            ));
        };
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| parse_err(line_no, "expected `key = value`".into()))?;
        let key = key.trim();
        let value = value.trim_start();
        match key {
            "id" => {
                if realm.id.is_some() {
                    return Err(parse_err(line_no, "duplicate `id` key".into()));
                }
                realm.id = Some((toml_subset::basic_string(value, line_no)?, line_no));
            }
            "command" => {
                if realm.command.is_some() {
                    return Err(parse_err(line_no, "duplicate `command` key".into()));
                }
                realm.command = Some((toml_subset::basic_string(value, line_no)?, line_no));
            }
            "args" => {
                if realm.args.is_some() {
                    return Err(parse_err(line_no, "duplicate `args` key".into()));
                }
                realm.args = Some(toml_subset::string_array(value, line_no)?);
            }
            "env_allow" => {
                if realm.env_allow.is_some() {
                    return Err(parse_err(line_no, "duplicate `env_allow` key".into()));
                }
                realm.env_allow = Some((toml_subset::string_array(value, line_no)?, line_no));
            }
            other => {
                return Err(parse_err(
                    line_no,
                    format!(
                        "unknown key {other:?} (this schema defines id, command, args, env_allow)"
                    ),
                ));
            }
        }
    }

    // Cardinality before per-table semantics: how many realms this file
    // declares is a property of its shape, and complaining about what is
    // *inside* the second table buries the answer the operator needs (that
    // there should not be a second table). [`RealmRegistry::from_specs`]
    // re-checks it as the invariant no constructor can bypass; this one is
    // here for the message an operator reads.
    if raw.len() > MAX_REALMS {
        return Err(too_many_realms(raw.len()));
    }

    raw.into_iter().map(validate_realm).collect()
}

/// The one wording for "this version serves exactly one realm", shared by
/// the parser and the registry so the two cannot drift.
fn too_many_realms(found: usize) -> ErrorKind {
    ErrorKind::Invalid(format!(
        "{found} [[realm]] tables, but this version serves exactly {MAX_REALMS} \
         (multi-realm is Phase 3)"
    ))
}

/// Turn one parsed table into a validated spec, applying the documented
/// defaults and refusing everything the module docs say is refused.
fn validate_realm(raw: RawRealm) -> Result<RealmSpec, ErrorKind> {
    let parse_err = |line: usize, detail: String| ErrorKind::Parse { line, detail };

    let id = match raw.id {
        Some((text, line)) => {
            validate_realm_id(&text).map_err(|detail| parse_err(line, detail))?;
            // The IDL, not this file, names the version-1 realm (module
            // docs). Refused *after* the shape check so a malformed id is
            // reported as malformed rather than as the wrong name.
            if text != WELL_KNOWN_REALM_ID {
                return Err(parse_err(
                    line,
                    format!(
                        "`id` {text:?} is not {WELL_KNOWN_REALM_ID:?}, the single well-known \
                         realm this protocol version defines: a session serving any other \
                         name answers every conformant client's \
                         get_realm({WELL_KNOWN_REALM_ID:?}) petition `unavailable`, forever. \
                         Omit `id` or write id = {WELL_KNOWN_REALM_ID:?}; operator-chosen \
                         realm names arrive with the wire's multi-realm phase"
                    ),
                ));
            }
            RealmId::new(text)
        }
        None => RealmId::new(WELL_KNOWN_REALM_ID),
    };

    let (command, command_line) = raw.command.ok_or_else(|| {
        ErrorKind::Invalid(format!(
            "realm {id} is missing `command` (the program the realm launches)"
        ))
    })?;
    if command.is_empty() {
        return Err(parse_err(
            command_line,
            "`command` is empty; a realm with no app is not a realm with a default app".into(),
        ));
    }
    if !Path::new(&command).is_absolute() {
        return Err(parse_err(
            command_line,
            format!(
                "`command` {command:?} is not an absolute path; a relative program would \
                 resolve through the core's own $PATH, which this file cannot show"
            ),
        ));
    }
    let command = PathBuf::from(command);

    let mut env_allow = Vec::new();
    if let Some((names, line)) = raw.env_allow {
        for name in names {
            if !is_env_name(&name) {
                return Err(parse_err(
                    line,
                    format!(
                        "`env_allow` entry {name:?} is not an environment variable name \
                         (expected [A-Za-z_][A-Za-z0-9_]*; env_allow carries NAMES passed \
                         through from the core's environment, not name=value pairs)"
                    ),
                ));
            }
            if let Some(why) = reserved_env_reason(&name) {
                return Err(parse_err(
                    line,
                    format!(
                        "`env_allow` entry {name:?} is decided by the core, not by config: \
                         {why}. The realm's app is confined to its own display server, and \
                         that is not a configurable property"
                    ),
                ));
            }
            if env_allow.contains(&name) {
                return Err(parse_err(
                    line,
                    format!("duplicate `env_allow` entry {name:?}"),
                ));
            }
            env_allow.push(name);
        }
    }

    Ok(RealmSpec {
        id,
        spawn: SpawnConfig {
            command,
            args: raw.args.unwrap_or_default(),
            env_allow,
        },
    })
}

/// A realm id must be usable both on the wire (`get_realm`'s `name`, max
/// 64 bytes) and as the single path component of the realm's private
/// runtime directory. Rather than restate those rules, ask the transport's
/// own validator -- the one definition of a legal realm id -- by having it
/// build that directory path under an arbitrary base: the function is pure
/// and validates the id before joining, so id rules can never drift
/// between the two crates.
fn validate_realm_id(id: &str) -> Result<(), String> {
    vitrin_ipc::paths::shim_runtime_dir_in(Path::new("/"), id)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "`id` {id:?} is not a legal realm id ({e}); it must be at most 64 bytes over \
                 [A-Za-z0-9._-] and never `.` or `..`, because it names both the wire realm \
                 and the realm's private runtime directory"
            )
        })
}

/// A POSIX portable environment variable name: `[A-Za-z_][A-Za-z0-9_]*`.
/// Deliberately stricter than what `setenv` tolerates, so a `"LANG=en_US"`
/// entry is refused as the misunderstanding it is rather than becoming an
/// unsettable variable.
fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// The minimal valid file: one realm, one absolute command.
    const MINIMAL: &str = "[[realm]]\ncommand = \"/usr/bin/true\"\n";

    /// A registry holding exactly the named realms, all `Configured` --
    /// what a `realm.toml` naming them would produce. The fixture other
    /// modules' tests build their realm environment from, so no test
    /// invents a realm the loader could not have produced.
    ///
    /// Bypasses [`MAX_REALMS`] deliberately: callers that need two realms
    /// are testing addressing, not the version-0 cardinality rule (which
    /// [`RealmRegistry::from_specs`] enforces and this module's own tests
    /// cover).
    /// A registry holding exactly these realms, however they were built.
    /// [`registry_with`] is the id-only shorthand over it.
    pub(crate) fn registry_of(realms: Vec<Realm>) -> RealmRegistry {
        RealmRegistry {
            realms: realms.into_iter().map(|r| (r.id.clone(), r)).collect(),
        }
    }

    pub(crate) fn registry_with(ids: &[&str]) -> RealmRegistry {
        RealmRegistry {
            realms: ids
                .iter()
                .map(|id| {
                    let id = RealmId::new(*id);
                    let realm = Realm {
                        id: id.clone(),
                        spawn: SpawnConfig {
                            command: PathBuf::from("/usr/bin/true"),
                            args: Vec::new(),
                            env_allow: Vec::new(),
                        },
                        state: RealmState::Configured,
                    };
                    (id, realm)
                })
                .collect(),
        }
    }

    /// A [`SpawnConfig`] with exactly these fields -- the constructor
    /// [`crate::spawn`]'s tests need, because the struct's fields are
    /// private and the loader (deliberately) cannot produce some of the
    /// states the spawn path must still refuse.
    pub(crate) fn spawn_config_with(
        command: &Path,
        args: &[String],
        env_allow: &[String],
    ) -> SpawnConfig {
        SpawnConfig {
            command: command.to_path_buf(),
            args: args.to_vec(),
            env_allow: env_allow.to_vec(),
        }
    }

    /// A `Configured` realm launching `command` -- the fixture
    /// [`crate::spawn`]'s tests execute. Bypasses the loader's filesystem
    /// audits on purpose: those are `RealmRegistry::load`'s, and the spawn
    /// path is required to re-check what it will actually run.
    pub(crate) fn realm_with_spawn(
        id: &str,
        command: &Path,
        args: &[String],
        env_allow: &[String],
    ) -> Realm {
        Realm {
            id: RealmId::new(id),
            spawn: spawn_config_with(command, args, env_allow),
            state: RealmState::Configured,
        }
    }

    fn registry_from(text: &str) -> Result<RealmRegistry, ErrorKind> {
        RealmRegistry::from_specs(parse_config(text)?)
    }

    /// A private (0700) scratch directory owned by this process. Its own
    /// ancestors are `/tmp` (sticky) and `/`, both of which the spawn-target
    /// audit accepts, so a program placed inside it is auditable.
    fn scratch_dir() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-realm-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// A file standing in for the realm's program, with the given mode.
    /// Real, because `command` is audited against a real filesystem.
    fn program_in(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// A `realm.toml` with the given text and mode inside `dir`.
    fn config_in(dir: &Path, text: &str, mode: u32) -> PathBuf {
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// A scratch `realm.toml` with the given mode, in a private temp dir.
    /// Returns `(dir, path)`; the caller removes the dir. For the tests
    /// whose file never reaches the spawn-target audit.
    fn config_file(text: &str, mode: u32) -> (PathBuf, PathBuf) {
        let dir = scratch_dir();
        let path = config_in(&dir, text, mode);
        (dir, path)
    }

    /// A loadable session: a scratch dir holding a trusted-writer-clean
    /// program and a `realm.toml` naming it. `extra` is appended to the
    /// table. Returns `(dir, config path, program path)`.
    fn loadable(extra: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = scratch_dir();
        let program = program_in(&dir, "app", 0o755);
        let path = config_in(
            &dir,
            &format!("[[realm]]\ncommand = \"{}\"\n{extra}", program.display()),
            0o600,
        );
        (dir, path, program)
    }

    // -- the realm object and its addressability ---------------------------

    #[test]
    fn the_realm_is_addressable_by_a_stable_id_defaulting_to_realm_0() {
        // Acceptance criterion 1: the realm has a stable id visible to
        // clients -- `get_realm("realm-0")` names *this* object.
        let registry = registry_from(MINIMAL).unwrap();
        assert_eq!(registry.len(), 1);
        let realm = registry.get(WELL_KNOWN_REALM_ID).expect("realm-0 exists");
        assert_eq!(realm.id().as_str(), "realm-0");
        assert_eq!(realm.state(), RealmState::Configured);
        assert_eq!(realm.spawn().command(), Path::new("/usr/bin/true"));
        assert!(realm.spawn().args().is_empty());
        assert!(realm.spawn().env_allow().is_empty());
        // The id is what grant rows key on: same value, same type.
        assert_eq!(
            registry.resolve_for_petition("realm-0"),
            Some(&RealmId::new("realm-0"))
        );
    }

    #[test]
    fn realm_existence_is_a_registry_lookup_not_a_constant() {
        // Existence is whatever the registry holds -- a real keyed lookup,
        // which is what makes the later multi-realm phase additive. Asserted
        // on a registry serving a name that is *not* the well-known one: a
        // hardcoded "realm-0" comparison would pass this backwards.
        //
        // Version-0 *config* cannot produce this registry (the id is pinned
        // to the IDL's well-known name, asserted just below); the model
        // underneath is deliberately not narrowed to match, so the check
        // that opens up is one line in the validator.
        let registry = registry_with(&["kiosk"]);
        assert_eq!(
            registry.resolve_for_petition("kiosk"),
            Some(&RealmId::new("kiosk"))
        );
        assert_eq!(registry.resolve_for_petition(WELL_KNOWN_REALM_ID), None);
    }

    #[test]
    fn the_realm_id_is_the_one_the_idl_fixes_for_this_version() {
        // The IDL declares "realm-0" the single well-known realm of version
        // 1. A session serving some other name is structurally legal and
        // practically useless: every conformant client petitions "realm-0"
        // and is told `unavailable` forever, which the IDL specifies as a
        // *race*, not as a permanent fact about a configured session.
        let err = registry_from("[[realm]]\nid = \"kiosk\"\ncommand = \"/usr/bin/true\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("kiosk"), "must name the value: {err}");
        assert!(
            err.contains("realm-0"),
            "must name what it should be: {err}"
        );
        assert!(
            err.contains("get_realm"),
            "must say what breaks, in the client's terms: {err}"
        );

        // Written explicitly and omitted mean the same thing, and both load.
        for text in [
            "[[realm]]\nid = \"realm-0\"\ncommand = \"/usr/bin/true\"\n",
            "[[realm]]\ncommand = \"/usr/bin/true\"\n",
        ] {
            let registry = registry_from(text).unwrap();
            assert_eq!(
                registry.resolve_for_petition(WELL_KNOWN_REALM_ID),
                Some(&RealmId::new(WELL_KNOWN_REALM_ID))
            );
        }
    }

    #[test]
    fn unknown_names_resolve_to_nothing_and_a_configured_realm_admits_petitions() {
        // The IDL's "unknown or vacant" pair: version 0 answers vacancy by
        // presence alone, and a configured-but-not-running realm is NOT
        // vacant (module docs). P1.5.2/P1.5.3 flip this in
        // `Realm::admits_petitions` and nowhere else.
        let registry = registry_from(MINIMAL).unwrap();
        for unknown in ["realm-1", "", "realm-0 ", "REALM-0"] {
            assert_eq!(
                registry.resolve_for_petition(unknown),
                None,
                "{unknown:?} must not resolve"
            );
        }
        let realm = registry.get(WELL_KNOWN_REALM_ID).unwrap();
        assert!(
            realm.admits_petitions(),
            "a configured realm whose app has not started is addressable, not vacant: \
             liveness is the chokepoint's no_surface judgement, not a petition-time lie"
        );
    }

    #[test]
    fn a_running_realm_is_addressable_and_records_its_pid() {
        // P1.5.2's state arm: spawning changes what the realm *is doing*,
        // never whether its name resolves. The wire answer is identical
        // before and after the fork -- which is what "no signature, no
        // caller, no wire behavior moves" means in practice.
        let mut registry = registry_from(MINIMAL).unwrap();
        let id = RealmId::new(WELL_KNOWN_REALM_ID);
        assert!(registry.mark_running(&id, 4242));
        assert!(!registry.mark_running(&RealmId::new("realm-1"), 1));

        let realm = registry.get(WELL_KNOWN_REALM_ID).unwrap();
        assert_eq!(realm.state(), RealmState::Running { pid: 4242 });
        assert!(
            realm.admits_petitions(),
            "a running realm whose app has not painted is still merely unpainted"
        );
        assert_eq!(
            registry.resolve_for_petition(WELL_KNOWN_REALM_ID),
            Some(&id)
        );
    }

    #[test]
    fn an_exited_realm_is_vacant_and_stops_resolving() {
        // P1.5.3's state arm, and the ONE place the petition-time answer
        // finally flips (module docs): `no_surface` means "not right now",
        // `unavailable` means "not ever", and with no restart policy a
        // realm whose shim is gone is the second.
        let mut registry = registry_from(MINIMAL).unwrap();
        let id = RealmId::new(WELL_KNOWN_REALM_ID);
        assert!(registry.mark_running(&id, 4242));
        assert!(registry.mark_exited(&id, 4242));
        assert!(!registry.mark_exited(&RealmId::new("realm-1"), 1));

        let realm = registry.get(WELL_KNOWN_REALM_ID).unwrap();
        assert_eq!(realm.state(), RealmState::Exited { pid: 4242 });
        assert!(
            !realm.admits_petitions(),
            "a realm whose shim is gone for good is vacant, not merely unpainted"
        );
        assert_eq!(
            registry.resolve_for_petition(WELL_KNOWN_REALM_ID),
            None,
            "the IDL's `unavailable`: the same answer an unknown name gets, deliberately \
             indistinguishable to the client"
        );

        // Still *in* the registry, and still owning its spawn config: the
        // realm object outlives its process, which is what keeps realm ids
        // and process lifetimes separate concepts.
        assert_eq!(realm.id(), &id);
        assert!(realm.spawn().command().is_absolute());

        // Terminal is terminal, and writing it twice writes the same thing
        // -- idempotence lives in `crate::lifecycle`'s latch, not here, so
        // the registry must not fight it.
        assert!(registry.mark_exited(&id, 4242));
        assert_eq!(
            registry.get(WELL_KNOWN_REALM_ID).unwrap().state(),
            RealmState::Exited { pid: 4242 }
        );
    }

    // -- schema: required, optional, and defaulted keys --------------------

    #[test]
    fn args_and_env_allow_parse_and_default_to_empty() {
        let registry = registry_from(
            "[[realm]]\n\
             id = \"realm-0\"\n\
             command = \"/usr/bin/foot\"\n\
             args = [\"-e\", \"bash\", \"-lc\", \"echo hi\"]\n\
             env_allow = [\"HOME\", \"LANG\"]\n",
        )
        .unwrap();
        let spawn = registry.get("realm-0").unwrap().spawn();
        assert_eq!(spawn.command(), Path::new("/usr/bin/foot"));
        assert_eq!(spawn.args(), ["-e", "bash", "-lc", "echo hi"]);
        assert_eq!(spawn.env_allow(), ["HOME", "LANG"]);

        // Omitted entirely, and explicitly empty, mean the same thing.
        for text in [
            MINIMAL,
            "[[realm]]\ncommand = \"/usr/bin/true\"\nargs = []\nenv_allow = []\n",
        ] {
            let spawn = registry_from(text).unwrap();
            let spawn = spawn.get(WELL_KNOWN_REALM_ID).unwrap().spawn();
            assert!(spawn.args().is_empty());
            assert!(spawn.env_allow().is_empty());
        }
    }

    #[test]
    fn the_env_allowlist_passes_named_variables_and_nothing_else() {
        // Default-deny (module docs): an empty allowlist inherits NOTHING,
        // and a name the core's environment does not define is skipped
        // rather than failing the run.
        let registry = registry_from(
            "[[realm]]\ncommand = \"/usr/bin/true\"\nenv_allow = [\"HOME\", \"LANG\", \"ABSENT\"]\n",
        )
        .unwrap();
        let spawn = registry.get(WELL_KNOWN_REALM_ID).unwrap().spawn();
        let env = spawn.inherited_env(|name| match name {
            "HOME" => Some("/home/agent".into()),
            "LANG" => Some("en_US.UTF-8".into()),
            // Deliberately resolvable but never asked for: the allowlist,
            // not the lookup, decides what crosses.
            "SSH_AUTH_SOCK" => Some("/run/user/1000/ssh-agent".into()),
            _ => None,
        });
        assert_eq!(
            env,
            vec![
                ("HOME".to_string(), "/home/agent".to_string()),
                ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ]
        );

        let empty = registry_from(MINIMAL).unwrap();
        assert!(empty
            .get(WELL_KNOWN_REALM_ID)
            .unwrap()
            .spawn()
            .inherited_env(|_| Some("leaked".into()))
            .is_empty());
    }

    #[test]
    fn config_cannot_hand_the_app_the_host_display_server() {
        // P1.5.2 scrubs the host's display variables unconditionally; a
        // config able to allow-list them back would silently void the
        // confinement (module docs). Each refusal names the variable and
        // the specific mechanism, because "it is reserved" teaches the
        // operator nothing about why their app cannot have it.
        for (name, _) in RESERVED_ENV {
            let text =
                format!("[[realm]]\ncommand = \"/usr/bin/true\"\nenv_allow = [\"{name}\"]\n");
            let err = registry_from(&text).unwrap_err().to_string();
            assert!(err.contains(name), "message must name the variable: {err}");
            assert!(
                err.contains("confined"),
                "message must say why it is refused: {err}"
            );
        }

        // Named regression anchors, because the reasoning that produced the
        // original two-name list does not reach them: WAYLAND_SOCKET is not
        // a display *name* but the number of an already-connected fd, which
        // libwayland honors before WAYLAND_DISPLAY and before its own `name`
        // argument -- so allow-listing it in nested mode (where the core's
        // own environment carries the host compositor's socket) would hand
        // the confined app a live host connection and the realm's private
        // socket would never be consulted. XDG_RUNTIME_DIR is the host
        // session's socket directory *and* a variable the core injects.
        for name in ["WAYLAND_SOCKET", "XDG_RUNTIME_DIR", "XAUTHORITY"] {
            assert!(
                reserved_env_reason(name).is_some(),
                "{name} must be unreachable from config"
            );
        }
        // Exact, case-sensitive matching: `getenv` finds DISPLAY, never
        // display, so refusing the latter would be theater.
        assert!(reserved_env_reason("display").is_none());
        assert!(reserved_env_reason("HOME").is_none());
    }

    // -- validation strictness --------------------------------------------

    #[test]
    fn a_missing_or_empty_or_relative_command_is_refused_with_a_reason() {
        let missing = registry_from("[[realm]]\nid = \"realm-0\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            missing.contains("missing `command`"),
            "unexpected: {missing}"
        );

        let empty = registry_from("[[realm]]\ncommand = \"\"\n")
            .unwrap_err()
            .to_string();
        assert!(empty.contains("`command` is empty"), "unexpected: {empty}");

        for relative in ["foot", "./foot", "bin/foot"] {
            let err = registry_from(&format!("[[realm]]\ncommand = \"{relative}\"\n"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("absolute") && err.contains("$PATH"),
                "a relative command must be refused with its reason: {err}"
            );
        }
    }

    #[test]
    fn unknown_keys_and_duplicate_keys_are_refused_at_their_line() {
        // A typo'd security-relevant key that silently did nothing would
        // read exactly like one that works (module docs).
        let err = registry_from("[[realm]]\ncommand = \"/usr/bin/true\"\nenv_alow = [\"HOME\"]\n")
            .unwrap_err();
        match &err {
            ErrorKind::Parse { line, detail } => {
                assert_eq!(*line, 3);
                assert!(detail.contains("env_alow"), "{detail}");
                assert!(
                    detail.contains("env_allow"),
                    "must list the real keys: {detail}"
                );
            }
            other => panic!("expected a parse error, got {other:?}"),
        }

        for (text, why) in [
            (
                "[[realm]]\ncommand = \"/a\"\ncommand = \"/b\"\n",
                "duplicate command",
            ),
            (
                "[[realm]]\nid = \"a\"\nid = \"b\"\ncommand = \"/a\"\n",
                "duplicate id",
            ),
            (
                "[[realm]]\ncommand = \"/a\"\nargs = []\nargs = []\n",
                "duplicate args",
            ),
            (
                "[[realm]]\ncommand = \"/a\"\nenv_allow = []\nenv_allow = []\n",
                "duplicate env_allow",
            ),
        ] {
            assert!(registry_from(text).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn bad_types_and_non_subset_toml_are_refused() {
        for (text, why) in [
            (
                "[[realm]]\ncommand = 42\n",
                "integer where a string belongs",
            ),
            (
                "[[realm]]\ncommand = [\"/usr/bin/true\"]\n",
                "array where a string belongs",
            ),
            (
                "[[realm]]\ncommand = \"/a\"\nargs = \"-e\"\n",
                "string where an array belongs",
            ),
            (
                "[[realm]]\ncommand = \"/a\"\nenv_allow = \"HOME\"\n",
                "string where an array belongs",
            ),
            ("[realm]\ncommand = \"/a\"\n", "single-bracket table"),
            ("[[realm]] junk\n", "trailing content after header"),
            ("[[principal]]\ncommand = \"/a\"\n", "foreign table"),
            ("command = \"/a\"\n", "key outside any table"),
            ("[[realm]]\ncommand\n", "no `=`"),
            ("[[realm]]\ncommand = \"/a\" junk\n", "trailing junk"),
        ] {
            assert!(registry_from(text).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn env_allow_entries_must_be_environment_variable_names() {
        for (entry, why) in [
            ("LANG=en_US", "a name=value pair, not a name"),
            ("1ST", "leading digit"),
            ("", "empty"),
            ("with space", "space"),
            ("lower-case", "hyphen"),
        ] {
            let text =
                format!("[[realm]]\ncommand = \"/usr/bin/true\"\nenv_allow = [\"{entry}\"]\n");
            let err = registry_from(&text);
            assert!(err.is_err(), "must reject {entry:?}: {why}");
        }
        let dup = "[[realm]]\ncommand = \"/a\"\nenv_allow = [\"HOME\", \"HOME\"]\n";
        assert!(registry_from(dup).is_err(), "duplicate entries are refused");
        // Underscored and digit-bearing names are ordinary and accepted.
        let ok =
            "[[realm]]\ncommand = \"/a\"\nenv_allow = [\"_X\", \"XDG_SESSION_TYPE\", \"A1\"]\n";
        assert_eq!(
            registry_from(ok)
                .unwrap()
                .get("realm-0")
                .unwrap()
                .spawn()
                .env_allow(),
            ["_X", "XDG_SESSION_TYPE", "A1"]
        );
    }

    #[test]
    fn realm_ids_that_could_escape_the_runtime_tree_are_refused() {
        // The id names the realm's private runtime directory, so it is
        // validated by the transport's own realm-id rule -- one definition,
        // no drift.
        for id in ["", ".", "..", "../evil", "a/b", "é", &"x".repeat(65)] {
            let text = format!("[[realm]]\nid = \"{id}\"\ncommand = \"/usr/bin/true\"\n");
            let err = registry_from(&text)
                .expect_err(&format!("must reject id {id:?}"))
                .to_string();
            assert!(
                err.contains("legal realm id"),
                "an ill-shaped id must be reported as ill-shaped, not as the wrong \
                 name -- the shape check runs first: {err}"
            );
        }
        // Shape-legal but not the well-known name: the *other* refusal, so
        // the two checks are visibly distinct and ordered.
        let err = registry_from("[[realm]]\nid = \"realm.0\"\ncommand = \"/a\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("single well-known realm"), "unexpected: {err}");
    }

    #[test]
    fn the_cardinality_rule_is_exactly_one_realm_in_v0() {
        // Zero realms: the core has nothing to serve.
        let none = registry_from("# just a comment\n").unwrap_err().to_string();
        assert!(none.contains("no [[realm]] table"), "unexpected: {none}");

        // Two realms: refused by *cardinality*, with the Phase-3 pointer --
        // the file format already carries an array, so raising MAX_REALMS
        // is the whole change. Counted before the tables are validated, so
        // this is the answer the operator gets whatever is inside the
        // second one: that there should not be a second one.
        let two = registry_from(
            "[[realm]]\nid = \"a\"\ncommand = \"/a\"\n[[realm]]\nid = \"b\"\ncommand = \"/b\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(two.contains("exactly 1"), "unexpected: {two}");
        assert!(two.contains("Phase 3"), "unexpected: {two}");

        // And enforced again by the constructor, so a caller that skips the
        // parser cannot build a registry the wire could not describe.
        assert!(RealmRegistry::from_specs(vec![
            parse_config(MINIMAL).unwrap().remove(0),
            parse_config(MINIMAL).unwrap().remove(0),
        ])
        .is_err());
    }

    // -- loading from disk --------------------------------------------------

    #[test]
    fn loading_names_the_file_and_the_problem() {
        // Acceptance criterion 3: a parse error fails loudly, naming the
        // path, the line, and the specific problem -- asserted on the
        // message an operator reads, not merely on the error type.
        let _fd = crate::capture::tests::fd_lock();
        let (dir, path) = config_file("[[realm]]\ncommand = \"relative/foot\"\n", 0o600);
        let err = RealmRegistry::load(&path).unwrap_err().to_string();
        assert!(
            err.contains(&path.display().to_string()),
            "must name the file: {err}"
        );
        assert!(err.contains("line 2"), "must name the line: {err}");
        assert!(err.contains("absolute"), "must name the problem: {err}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_a_hard_startup_error_naming_the_path() {
        let _fd = crate::capture::tests::fd_lock();
        let path = std::env::temp_dir().join(format!(
            "vitrin-realm-absent-{}/realm.toml",
            std::process::id()
        ));
        let err = RealmRegistry::load(&path).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Io(_)));
        let text = err.to_string();
        assert!(
            text.contains(&path.display().to_string()),
            "must name the path: {text}"
        );
    }

    #[test]
    fn a_well_formed_file_loads_from_disk() {
        let _fd = crate::capture::tests::fd_lock();
        let dir = scratch_dir();
        let program = program_in(&dir, "app", 0o755);
        // World-readable is fine: a command line is not secret material.
        let path = config_in(
            &dir,
            &format!(
                "# a realm\n[[realm]]\ncommand = \"{}\"\nargs = [\"--version\"]\n",
                program.display()
            ),
            0o644,
        );
        let registry = RealmRegistry::load(&path).unwrap();
        assert_eq!(registry.len(), 1);
        let realm = registry.get(WELL_KNOWN_REALM_ID).unwrap();
        assert_eq!(realm.spawn().args(), ["--version"]);
        // The path the operator wrote is the path the realm will exec:
        // auditing resolves symlinks, executing must not (argv[0] is
        // observable to the program).
        assert_eq!(realm.spawn().command(), program);
        assert_eq!(registry.iter().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    // -- the program the core will exec ------------------------------------

    #[test]
    fn a_command_only_a_trusted_writer_can_replace_is_required() {
        // The not-writable policy is transitive or it is decorative: a
        // 0600 realm.toml naming a group-writable program hands exactly the
        // authority the file check protects to everyone who can write that
        // program (module docs).
        let _fd = crate::capture::tests::fd_lock();

        // The clean case, and the same program made world-writable.
        let (dir, path, program) = loadable("");
        assert!(RealmRegistry::load(&path).is_ok());
        for mode in [0o777, 0o757, 0o775] {
            fs::set_permissions(&program, fs::Permissions::from_mode(mode)).unwrap();
            let err = RealmRegistry::load(&path).unwrap_err();
            assert!(
                matches!(err.kind, ErrorKind::Insecure(_)),
                "mode {mode:03o} must be refused, got {err:?}"
            );
            let text = err.to_string();
            assert!(
                text.contains(&program.display().to_string()),
                "must name the program: {text}"
            );
            assert!(
                text.contains("writable by group/other"),
                "must name the fault: {text}"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_writable_directory_on_the_commands_path_is_refused() {
        // Swapping the program is the same attack whether the attacker
        // writes the file or the directory holding it, so the audit walks
        // every component. A sticky world-writable directory (/tmp) is the
        // documented exception -- a non-owner cannot replace an entry it
        // does not own -- and is what the scratch dirs themselves sit in.
        let _fd = crate::capture::tests::fd_lock();
        let dir = scratch_dir();
        let wide = dir.join("wide");
        fs::create_dir(&wide).unwrap();
        fs::set_permissions(&wide, fs::Permissions::from_mode(0o777)).unwrap();
        let program = program_in(&wide, "app", 0o755);
        let path = config_in(
            &dir,
            &format!("[[realm]]\ncommand = \"{}\"\n", program.display()),
            0o600,
        );

        let err = RealmRegistry::load(&path).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Insecure(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains(&wide.display().to_string()),
            "must name the offending directory, not just the program: {text}"
        );
        assert!(text.contains("swap"), "must say what it enables: {text}");

        // The sticky bit is what makes the same mode safe on a directory.
        fs::set_permissions(&wide, fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(RealmRegistry::load(&path).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_command_that_does_not_resolve_is_a_hard_startup_error() {
        // Fail closed and loud: a realm whose app does not exist has
        // nothing to serve, and learning that at startup beats learning it
        // from a spawn failure with no configuration context.
        let _fd = crate::capture::tests::fd_lock();
        let dir = scratch_dir();
        let missing = dir.join("nope");
        let path = config_in(
            &dir,
            &format!("[[realm]]\ncommand = \"{}\"\n", missing.display()),
            0o600,
        );
        let text = RealmRegistry::load(&path).unwrap_err().to_string();
        assert!(
            text.contains(&missing.display().to_string()),
            "must name the command: {text}"
        );
        assert!(text.contains("does not resolve"), "unexpected: {text}");

        // A directory is not a program either.
        let path = config_in(
            &dir,
            &format!("[[realm]]\ncommand = \"{}\"\n", dir.display()),
            0o600,
        );
        let err = RealmRegistry::load(&path).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Insecure(_)), "{err:?}");
        assert!(
            err.to_string().contains("not a regular file"),
            "unexpected: {err}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_audit_is_a_property_of_loading_not_of_parsing() {
        // Parsing stays pure and total: it answers questions about text.
        // Whether a path names a program only this filesystem can say, so
        // the audit lives beside the config-file security check and specs
        // built in-process (tests, and P1.5.2's fixtures) do not pretend to
        // have been audited.
        assert!(registry_from("[[realm]]\ncommand = \"/nonexistent/app\"\n").is_ok());
    }

    #[test]
    fn a_group_or_world_writable_config_is_refused() {
        // Not the secret-material policy principals.toml uses: this file
        // names what the TCB executes, so *writability* is the threat.
        let _fd = crate::capture::tests::fd_lock();
        for mode in [0o666, 0o622, 0o662, 0o620, 0o602] {
            let (dir, path) = config_file(MINIMAL, mode);
            match RealmRegistry::load(&path) {
                Err(RealmConfigError {
                    kind: ErrorKind::Insecure(detail),
                    ..
                }) => assert!(detail.contains("chmod go-w"), "unexpected: {detail}"),
                other => panic!("mode {mode:03o} must be refused, got {other:?}"),
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_non_regular_config_is_refused() {
        let _fd = crate::capture::tests::fd_lock();
        let (dir, path) = config_file(MINIMAL, 0o600);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            RealmRegistry::load(&path).map_err(|e| e.kind),
            // Opening a directory for read succeeds on Linux only until the
            // first read; the fstat probe refuses it first.
            Err(ErrorKind::Insecure(_)) | Err(ErrorKind::Io(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    // -- the default config path --------------------------------------------

    #[test]
    fn the_default_config_path_is_the_xdg_one() {
        // Not env-mutating (tests share a process): the shape is what
        // matters -- `<config home>/vitrin/realm.toml`, the same directory
        // principals.toml is conventionally read from.
        if let Ok(path) = default_config_path() {
            assert!(path.is_absolute(), "{}", path.display());
            assert_eq!(path.file_name().unwrap(), CONFIG_FILE_NAME);
            assert_eq!(path.parent().unwrap().file_name().unwrap(), "vitrin");
            let expected = match std::env::var_os("XDG_CONFIG_HOME") {
                Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
                _ => PathBuf::from(std::env::var_os("HOME").unwrap()).join(".config"),
            };
            assert_eq!(path.parent().unwrap().parent().unwrap(), expected);
        }
    }
}
