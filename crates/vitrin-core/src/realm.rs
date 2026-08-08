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
//! rather than from a constant. The registry is a keyed collection, so
//! raising the realm count added rows and lifecycle events rather than
//! replacing the addressing model -- exactly the IDL's reason for putting
//! realm ids on the wire from day one. What that did and did not buy is
//! audited in full below ("Deletion or re-plumbing").
//!
//! # `realm.toml` schema (decided here, per issue #30)
//!
//! An array of `[[realm]]` tables in the core's strict TOML subset
//! ([`crate::toml_subset`] -- the same dialect as `principals.toml`; see
//! `examples/realm.toml` for a commented template). At least one table, at
//! most [`MAX_REALMS`], and one of them **must** be `realm-0`. Keys:
//!
//! | key | type | required | meaning |
//! |---|---|---|---|
//! | `id` | string | no (default [`WELL_KNOWN_REALM_ID`]) | the realm's stable, wire-visible name: what `vitrin_principal.get_realm` addresses, what grant rows state, and the directory name of the realm's private runtime tree. Free-form within the transport's id rule, but the **set** must contain `"realm-0"` -- see below |
//! | `command` | string | **yes** | absolute path of the program the realm launches (P1.5.2 execs it); audited at load, see below |
//! | `args` | array of strings | no (default `[]`) | arguments **after** `argv[0]`; the core supplies `argv[0]` itself from `command` |
//! | `env_allow` | array of strings | no (default `[]`) | names of environment variables passed through from the core's own environment; see below |
//! | `autostart` | boolean | no (default `true`) | whether startup forks this realm's app. `false` makes the realm a **template**: addressable and petitionable, never itself running -- see below |
//!
//! ## `autostart = false`: a template (WS-E.1.1, issue #207)
//!
//! A realm declared `autostart = false` is loaded in
//! [`RealmState::Template`] and [`crate::session::start_realm_in`] does not
//! fork it. It exists to be the **subject of a `realm_launch` grant**: a
//! principal petitions over the template, a human sees the template's
//! `command` on the consent card, and each admitted
//! `vitrin_launcher.launch` forks one *instance* of that command.
//!
//! Three consequences, all deliberate:
//!
//! - A template **admits petitions** ([`Realm::admits_petitions`]) -- it has
//!   to, or the authority to launch it could never be petitioned for.
//! - A template **never paints**. An `observe` grant over one refuses
//!   `no_surface` forever, which is authority over nothing: inert rather
//!   than dangerous, and the IDL says so at `vitrin_launcher.launch`.
//! - **At least one realm must autostart** ([`RealmRegistry::from_specs`]).
//!   A session of nothing but templates comes up with no app, no output
//!   binding and nothing for a human to look at; refusing it at load is the
//!   same posture as refusing an empty file.
//!
//! `autostart` is *not* what makes a realm launchable. Any realm in the
//! registry names a program, so a `realm_launch` grant over a running realm
//! launches a second instance of that realm's app. The key answers one
//! question only -- does **startup** fork it -- which is why it is a
//! boolean and not a `kind = "template"` enum: there is no second axis.
//!
//! ## `realm-0` is the IDL's name and stays mandatory (WS-E.1.2)
//!
//! `id` was once pinned: version 0 refused every value but
//! [`WELL_KNOWN_REALM_ID`]. That pin is gone, and the reasoning it rested
//! on is not -- it changed shape rather than being retired.
//!
//! The IDL settles the name. `get_realm`'s description declares `"realm-0"`
//! **the single well-known realm of version 1** and, at version 2, a
//! *required* member of every deployment: the one realm name a conformant
//! client can know without being told, because the wire still carries no
//! enumeration. A session that renamed its only realm would still be
//! *structurally* legal (`get_realm` always mints a handle, and an unknown
//! name resolves `unavailable` at petition time), but every conformant
//! client would petition `realm-0` and be told, forever, that the realm is
//! absent. The IDL specifies that absence as a **race** against realm
//! lifecycle, not as a permanent property of a correctly configured
//! session, so a core that manufactured it from a config key would be lying
//! in the protocol's own vocabulary.
//!
//! So the loader now enforces *membership* where it used to enforce
//! *identity*: any shape-legal id may appear, and `realm-0` must be among
//! them ([`RealmRegistry::from_specs`]). That is what makes widening the id
//! rule **additive** rather than merely compatible-looking -- no conformant
//! version-1 client's `get_realm("realm-0")` assumption breaks.
//!
//! Widening it at all was a **protocol** change first, and that half landed
//! before this one: the IDL description had to stop naming `realm-0` as
//! *the* version-1 realm and no more, which it now does (issue #225 put
//! `realm_launch` and the version-2 wire in place; this task's paired edit
//! states version 2's realm cardinality on `get_realm` and in
//! `docs/protocol/03-vitrin_realm.md`).
//!
//! **Two naming authorities, cleanly split.** Configuration names
//! *templates* (operator-chosen); the core names *instances*
//! (`<template>.<n>`, WS-E.1.1, minted by `vitrin_launcher.launch`). This
//! file therefore never names an instance: letting it would make
//! uniqueness across a session a property of a text file, which is one
//! authority too many for something that also names a private runtime
//! directory.
//!
//! The split is enforced rather than described, in two places, and both are
//! in [`validate_realm_id`] -- so a *declared* id that could collide with a
//! *minted* one is refused at load and the collision is unrepresentable
//! afterwards:
//!
//! 1. **A declared id may not look like an instance id.** The minted shape
//!    is `<declared>.<decimal>`, and each realm claims both `<id>` and
//!    `<id>.lock` in the flat runtime tree
//!    ([`reject_runtime_name_collisions`]), so a declared `foo.1` would own
//!    minted `foo.1`'s directory and a declared `foo.1.lock` would own its
//!    lock. Both are refused: a declared id may not end in `.` followed by
//!    digits, with or without a trailing `.lock`.
//! 2. **A declared id must leave room for the suffix.** The wire caps a
//!    realm id at 64 bytes and `launched` has to carry the minted id
//!    *through* that cap, so a declared id is capped at
//!    `64 - `[`MAX_INSTANCE_SUFFIX`] bytes. Without this, minting could
//!    produce an id the wire cannot express and a launch would fail
//!    `internal` for a reason the operator could have been told at load.
//!
//! Uniqueness among minted ids is then a counter, not a search:
//! [`RealmRegistry::mint_instance`] never reissues a number for the life of
//! the session, and the resulting id is a [`MintedRealmId`] -- a type
//! nothing outside this module can construct, so "the id came off the wire"
//! is a compile error rather than a review note.
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
//! - **zero `[[realm]]` tables, or more than [`MAX_REALMS`]** -- a
//!   *cardinality* rule in the validator, not a grammar limit: the file
//!   format is an array of tables and always was, so raising the cap moved
//!   a number and the schema did not move. The cap is argued from memory at
//!   [`MAX_REALMS`], not from taste;
//! - **a set of tables that does not include `realm-0`** -- the membership
//!   rule that replaced the old id pin, argued above;
//! - **a duplicate key, a duplicate realm id, a duplicate or reserved
//!   `env_allow` name, or an ill-shaped id** -- each names the offending
//!   text and why. Two tables that both omit `id` collide on the
//!   `realm-0` default and are refused as duplicates, which is the honest
//!   answer: the second one did not ask for a second realm, it asked for
//!   the same one twice;
//! - **two ids that would fight over one entry in the runtime tree** --
//!   `foo` and `foo.lock`, or any id that claims the listener's `core.sock`
//!   / `core.sock.lock`. Free-form ids made this expressible for the first
//!   time; [`reject_runtime_name_collisions`] refuses it and names both.
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
//! # Deletion or re-plumbing: the audit (WS-E.1.2, issue #208)
//!
//! [`WELL_KNOWN_REALM_ID`]'s doc comment used to end by calling the
//! multi-realm phase *"a deletion here rather than a re-plumbing"*. **That
//! sentence is half true, and this section says which half**, because read
//! from a tracker it sounds like a claim about the session and an estimate
//! built on that reading is wrong by the size of `session.rs`.
//!
//! The true half is the half it is scoped to: `realm.rs`, and everything
//! that was already keyed by [`RealmId`]. The false half is the runtime
//! that owns the *live* realms, which held exactly one and said so in its
//! types.
//!
//! ## Genuinely a deletion: already keyed, nothing moved
//!
//! | Site | Why it was already multi-realm |
//! |---|---|
//! | [`RealmRegistry::realms`] | a `BTreeMap<RealmId, Realm>` from the start; [`RealmRegistry::resolve_for_petition`] and [`RealmRegistry::get`] are real map lookups, not constant comparisons wearing a lookup's clothes |
//! | [`RealmRegistry::mark_running`] / [`RealmRegistry::mark_exited`] / [`RealmRegistry::iter`] / [`RealmRegistry::len`] | already take or yield per-realm values |
//! | [`Realm`], [`RealmState`], [`SpawnConfig`], [`Realm::admits_petitions`] | per-realm objects; a second realm is a second value, not a second code path |
//! | [`crate::grants`]' `realm_id` column and `RealmId` newtype | grant rows keyed on the realm from day one |
//! | [`crate::petitions`]' `unavailable` judgement | asks the registry, so it answered correctly for names it had never seen |
//! | `vitrin_ipc::paths::{shim_runtime_dir_in, realm_lock_path_in, shim_socket_path_in}` | every path is a function of the realm id: N realms are N trees, N locks, N `wayland-0` sockets with no new code |
//! | [`crate::spawn`]'s `spawn_realm` / `SpawnPaths` | takes one `&Realm` and derives everything from its id; called N times it spawns N realms |
//! | [`crate::lifecycle`]'s `RealmLifecycle` | one instance owns one realm's child, runtime dir and `flock`, with its own death latch |
//! | [`crate::recorder`]'s `realm_spawned` / `realm_died` / `realm_exited` | already carry the realm id |
//! | `vitrin_shim_session.configure(realm, …)` | already tells each shim which realm it is |
//!
//! Two things really were deleted here: the `id != realm-0` pin in
//! [`validate_realm`], and the *"exactly one"* reading of [`MAX_REALMS`].
//! The constant itself survives with a new value and a new argument -- a
//! cap is not the rule it replaced.
//!
//! ## Re-plumbing the sentence does not cover
//!
//! | Site | What had to change |
//! |---|---|
//! | `session::Runtime::realm: Option<RealmRuntime>` | became `realms: BTreeMap<RealmId, RealmRuntime>` -- the single field the claim missed |
//! | `session::start_realm_in` | one spawn became a loop, and with it a *partial* startup state that one spawn could not have: a realm failing to attach now tears down the realms already forked, because neither backend reaches its own `shutdown_realm` on that path |
//! | `session::dispatch_shim` / `close_realm` | had no idea *which* realm they served; a `RealmId` is now carried in the shim `ConnectionSource`'s callback data |
//! | `session::with_realm_teardown` | keyed by realm id instead of reaching for "the" realm |
//! | `session::reap_realm` | one `poll_exit` became one per live realm: a reaper that stopped at the first exit would leave zombies and a realm the registry still called `Running` |
//! | `session::shutdown_realm` | one ladder became one ladder per realm |
//! | `session::emit_presented` | one shim server became every shim server |
//! | `session::dispatch_principal`'s liveness derivation | `is_some_and` over "the" realm became a **per-realm** question (`ServerCtx::realm_is_live`), applied by `principal::serve_facet_use` to the realm the grant row names. "Is *any* realm live" was the obvious rewrite and is fail-**open** across realms: a dead realm's grant would clear `no_surface` on a living sibling's account and be served a frame anyway. (Which frame changed under WS-E.1.3 -- per-realm scenes and a per-realm, pruned capture cache mean the fail-open reading would now serve the dead realm's own stale composition rather than the sibling's pixels. Milder, still forbidden: `no_surface` is documented as "never a stale frame") |
//! | `session::route_seat` | reached for "the" realm; WS-E.1.2 pointed it at `session::seat_target`, and WS-E.1.6 replaced that with the realm each admitted actuation's own **grant** names, carried on the event |
//! | `crate::input::InputRouter` | one router serves the session, so its per-shim-generation state had to learn *whose* generation it is (`bind_to` / `reset_for`): an unconditional reset on any realm's death latches a key down in a surviving realm's app. WS-E.1.6 finished the job -- the state is a map keyed by realm, so there is no shared table left to clear by accident |
//! | `crate::shim::ShimServer::connection_closed` | takes the dying realm's id, for that scoped reset |
//! | `backend::winit::deliver_physical` / `route_physical_inputs` | took `&Option<RealmRuntime>`; now resolve the realm through `session::physical_seat_target` (the human's attention), which since WS-E.1.6 the agent path no longer shares |
//! | `vitrin_ipc::paths` | still derives every path from the id, but the runtime directory is *flat*, so free-form ids made two realms able to claim one entry (`foo` vs `foo.lock`); the tree's namespace is now stated there and enforced by `reject_runtime_name_collisions` |
//! | `tests/integration/harness.py`, `examples/realm.toml` | wrote and taught the exactly-one rule at length |
//!
//! ### The comment that promised "nothing else here changes", claim by claim
//!
//! `session::start_realm_in` carried *"when that stops being true this
//! becomes a loop, and nothing else here changes: every piece of state
//! below is already per-realm"*. Checked line by line: the spawn, the
//! `configure` send, `into_parts`, `RealmLifecycle::adopt` and
//! `mark_running` are all per-realm and needed no change -- **but four
//! things did.**
//!
//! 1. **The storage.** The destination the result is stored into was a
//!    single `Option`. This is what the sentence was most wrong about.
//! 2. **The source registration**, inside `adopt`'s `place` closure. A
//!    `ConnectionSource` carries no metadata of its own, so the callback
//!    had to become `move` and capture a `RealmId`; without it
//!    `dispatch_shim` cannot tell which of N attached shims it is
//!    servicing. `adopt` itself did not move -- the closure handed to it
//!    did, which is why an earlier draft of this audit miscounted it as
//!    unchanged.
//! 3. **The geometry**, which the sentence was silent on: the view size
//!    handed to `start_shim_session` is one output's size shared by every
//!    realm, not a per-realm geometry, so it is hoisted out of the loop and
//!    every shim is configured identically. WS-E.1.3 examined that and
//!    **kept it**: there is one output, so one size, and per-realm geometry
//!    would be window-management policy (decision 3). What WS-E.1.3 did
//!    change is which realm the output *shows* and whose pixels a capture
//!    returns, neither of which is the configure geometry.
//! 4. **The failure path**, which did not exist: with one spawn there was
//!    no partial startup to unwind. With a loop there is, and the loop owns
//!    it -- see the table above.
//!
//! ## Not per-realm at all, and deliberately not fixed here
//!
//! **Two rows of this table were closed by WS-E.1.3 (issue #209)** and are
//! kept, struck through, because what they said was true and the shape of
//! the fix is worth reading beside what is still shared:
//!
//! | Site | State | Owner |
//! |---|---|---|
//! | ~~[`crate::scene::Scene`] -- one scene, at most one committed surface~~ | ~~every realm's shim commits into the same scene, so the last committer wins and only one realm is ever visible~~ **Closed by WS-E.1.3**: one [`crate::scene::Scene`] per realm, held in [`crate::scene::RealmScenes`], with exactly one bound to the output. Every realm still holds at most one surface -- that is the MVP's single-maximized model *per app*, and `scene::layout` says it must never grow | -- |
//! | ~~`session::Runtime::view_cache` / `principal::ServerCtx::realm_view`~~ | ~~one frame for the whole session, not one per realm: while two realms are **live**, a capture under a grant over realm A can carry realm B's pixels~~ **Closed by WS-E.1.3**: the cache is keyed by realm and `ServerCtx::realm_view` is a *function of the realm id*, applied to the realm the grant row names, so there is no "the view" left to hand the chokepoint by mistake | -- |
//! | ~~[`crate::input::InputRouter`], `PhysicalPresence`~~ | ~~one router and one presence tracker for the session, so `session::seat_target` picks one realm to deliver to and a human at the keyboard preempts agent actuation in *every* realm at once~~ **Closed by WS-E.1.6**: the router holds one [`crate::input::RealmSeat`] per realm and two named addressing rules (physical follows the bound realm, an agent's actuation follows its grant's realm), and presence is a [`crate::input::PhysicalPresenceMap`] keyed the same way. What is still shared, deliberately, is the **hook stack** -- the consent grab and the dead-man watcher must see every physical event whatever realm is bound, and `PreemptionHook::gate` is not even told the realm so a realm-scoped gate is inexpressible | -- |
//! | The **output**: one view size, one bound realm, one retained framebuffer pair | every realm's shim is configured with the output's geometry and every realm's view composes at it; a hidden realm renders but is never presented. Deliberate and permanent at this layer -- WS-E.1.3 decision 3 (no stacking, no overlap, no resize) and PRD §5.1, which exiles window-management policy from the core | -- (by design) |
//! | [`crate::scene::layout`] | single-maximized placement, the MVP's whole layout policy | D-018 |
//!
//! Naming these is the point of the table. A reader who takes "a deletion
//! rather than a re-plumbing" at face value would budget none of the work
//! in the second table and would not know the third exists.
//!
//! # Where the config path comes from
//!
//! `vitrind --realm PATH` (the `--consent` / `--recorder` spelling: both
//! `--realm PATH` and `--realm=PATH`), defaulting to
//! `$XDG_CONFIG_HOME/vitrin/realm.toml` -- falling back, per the XDG base
//! directory specification, to `$HOME/.config/vitrin/realm.toml`. That is
//! the same directory `principals.toml` is conventionally read from.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::grants::RealmId;
use crate::toml_subset::{self, SubsetError};

/// The realm name the IDL fixes: `get_realm`'s description declares
/// `"realm-0"` "the single well-known realm of version 1", and a required
/// member of every version-2 deployment. Both the default when `realm.toml`
/// omits `id` and a **mandatory member** of whatever set it declares
/// (module docs: a session with no `realm-0` answers every conformant
/// client's `get_realm("realm-0")` petition `unavailable`, forever).
///
/// A *validator* constraint, not a model one: [`RealmRegistry`] serves
/// whatever it holds and looks names up in a map, which is what keeps the
/// petition-time existence check a real lookup rather than a constant
/// comparison wearing a lookup's clothes.
///
/// It used to be the *only* value `id` could carry, and the sentence that
/// justified widening it -- "a deletion here rather than a re-plumbing" --
/// is audited in the module docs, which say exactly which half of it held.
pub(crate) const WELL_KNOWN_REALM_ID: &str = "realm-0";

/// Conventional file name under the core's configuration directory.
pub(crate) const CONFIG_FILE_NAME: &str = "realm.toml";

/// **How many realms one session may hold, and why the number is 16.**
///
/// A cap, not a cardinality rule -- it used to be `1`, which was the
/// version-0 statement "this version serves exactly one realm" wearing a
/// constant's clothes. Raising it is WS-E.1.2 (issue #208); what follows is
/// the argument for the value, because a limit chosen by taste is a limit
/// nobody can revise on evidence.
///
/// **This justification has been wrong twice, and WS-E.1.3 falsified half
/// of the third.** The first argued 16 from a per-realm scene copy that did
/// not exist; the second corrected that and then undercounted the
/// descriptors, saying 2 permanent per realm when there are 3; the third
/// counted the descriptors right and said the core holds "no pixels that
/// scale with the realm count" -- true when it was written, and **false
/// since WS-E.1.3** (issue #209), which made a per-realm scene and a
/// per-realm capture cache real. That third draft said in as many words
/// that when it happened the constant "has to be re-derived, not
/// inherited"; what follows is the re-derivation. All three corrections are
/// kept below rather than deleted, because the pattern is the point: this
/// is a constant whose argument is an *inventory*, and an inventory written
/// from memory is wrong. Every number here is measured against a running
/// core.
///
/// # What a realm actually costs the core
///
/// | Cost | Per realm | At the cap |
/// |---|---|---|
/// | Processes | one `vitrin-shim`, which execs one app | 32 |
/// | Runtime tree | one directory, one `<id>.lock`, one `wayland-0` inode | 16 of each |
/// | Core-side descriptors | **3** permanent + at most [`crate::shim::MAX_LIVE_SURFACES`] = 16 staged attach fds, so **≤ 19** | **≤ 304** |
/// | **Core-side pixels** | **two** view-sized RGBA copies: the realm's [`crate::scene::Scene`] surface and its `session::Runtime::view_cache` entry | **32 of them** |
/// | Core-side bytes (transport) | the transport's bounded queues: `MAX_SEND_QUEUE_BYTES` (256 KiB) + one 64 KiB read scratch + at most one partial frame -- well under a MiB | low tens of MiB |
///
/// The three permanent descriptors, each against the line that opens it:
///
/// | # | Descriptor | Opened by | Held by |
/// |---|---|---|---|
/// | 1 | the core's end of the identity socketpair | `Connection::pair` in [`crate::spawn::spawn_realm`] | the realm's `ConnectionSource` |
/// | 2 | the realm's `<id>.lock` `flock` | `RuntimeDirGuard::create`, same function | `RealmLifecycle::_realm_lock`, released only at teardown |
/// | 3 | the **outbox ping eventfd** | `calloop::ping::make_ping` inside `ConnectionSource::with_outbox` | the same source, for the life of the realm |
///
/// # The pixels, re-derived for WS-E.1.3 and measured
///
/// A realm now costs the core **exactly two view-sized RGBA buffers**, and
/// naming both is the whole correction -- the third draft's own forecast
/// named only one:
///
/// 1. its [`crate::scene::Scene`]'s committed `SurfaceContent`, the copy-in
///    of the client's buffer (buffer path v0 = copy-in, plan D3), which
///    under single-maximized is the view's size;
/// 2. its `session::Runtime::view_cache` entry, the composed frame a capture
///    of *that realm* serves.
///
/// So `2 * width * height * 4` bytes of **core-side pixels** per realm. The
/// two retained framebuffers the headless backend holds are the **output's**
/// and stay session-wide, because there is one output (WS-E.1.3 decision 3:
/// one bound realm, no stacking).
///
/// **One more thing scales with the realm count, and it is not core-side
/// pixels.** The nested backend retains one zero-copy GPU slot per realm
/// ([`crate::dmabuf::RealmGpuContent`]), so a session where every realm
/// commits `kind=dmabuf` holds up to `MAX_REALMS` `GlesTexture`s at once
/// instead of the one it held while the slot was session-wide. It costs no
/// core-side *copy*: each texture is an EGLImage sampling the client's own
/// buffer, which is the whole zero-copy claim. What it does cost is a
/// **reference that keeps that client buffer alive**, now up to sixteen of
/// them rather than one, and the client's buffers are outside every number on
/// this page ("What no number here bounds"). It is unavoidable rather than a
/// regression: while one realm's import evicted another's, a hidden realm's
/// commit was also what the human's window presented — the confidentiality
/// defect WS-E.1.3 closed. The measured table below is a `--headless` run and
/// is unaffected, because headless has no GPU renderer and its map is always
/// empty.
///
/// **Measured**, on a release build with N animating shims, `VmRSS` at rest
/// after the commits settle:
///
/// | View | 1 realm | 16 realms | Slope per realm | `2*w*h*4` |
/// |---|---|---|---|---|
/// | 1920x1080 | 63 MiB | **301 MiB** | ~15.9 MiB | 15.8 MiB |
/// | 2560x1600 | 117 MiB | **586 MiB** | ~31.3 MiB | 31.3 MiB |
///
/// The slope matches the arithmetic to well under a percent, which is what
/// makes this an *inventory that was checked* rather than a third guess: a
/// third, unnoticed per-realm buffer would show up as a slope the formula
/// does not predict. Descriptors were re-measured in the same runs and are
/// unchanged at `11 + 3N` (14, 17, 20, 23 at N = 1..4; **59 at N = 16**).
///
/// # So which constraint binds now, and does 16 still hold
///
/// **The binding constraint has moved from descriptors to memory**, exactly
/// as the third draft predicted it would if a per-realm copy became real.
/// At the cap, descriptors reach ≤ 304 -- ~31% of the 1024 soft
/// `RLIMIT_NOFILE` this repo already sizes against
/// ([`vitrin_ipc::MAX_SEND_QUEUE_FDS`]'s rationale), the same fraction as
/// before -- while pixels reach 500 MiB on a 2560x1600 panel.
///
/// **16 still holds, and this is why**: 586 MiB is ~7% of an 8 GiB laptop
/// and ~4% of 16 GiB, on a machine that is by construction also running 16
/// GUI apps whose own buffers dwarf it (the shims' and apps' memory is not
/// counted here and never was -- see "What no number here bounds"). A cap
/// whose worst case is a single-digit percentage of RAM is not the thing
/// that will fail first.
///
/// **WS-E.1.6 (issue #212) added per-realm state and it does not move this
/// number.** The input router now holds one [`crate::input::RealmSeat`] per
/// realm and one [`crate::input::PhysicalPresence`] per realm the human has
/// been in. Measured on x86-64: 96 bytes and 40 bytes respectively, plus each
/// one's `Vec` heap for the presses actually outstanding. Sixteen realms is
/// therefore about **2 KiB**, against 500 MiB of pixels. It is recorded here
/// because this constant's justification has been wrong three times by
/// *asserting* a cost instead of measuring one -- not because 2 KiB matters.
///
/// **What to re-derive from if the cap is ever raised** -- and it is a
/// different number from last time, so inheriting *this* paragraph would
/// repeat the mistake: the product `cap x width x height x 8 bytes`. At
/// 2560x1600 a cap of 64 would cost 2.0 GiB of core-side pixels alone, which
/// is where "a percentage of RAM" stops being the right frame -- and 64
/// realms would want ≤ 1216 descriptors, *past* the 1024 soft limit. So at
/// that scale **both** constraints bind, and neither may be inherited from
/// here. Sixteen is also a human-scale ceiling for "apps on one
/// desktop", which is what this cap is for: it is a *policy* on how many
/// shims one configuration file may make the trusted core fork, not a
/// memory computation -- but it is now a policy with a measured memory bill
/// attached.
///
/// **The corrections, kept rather than deleted.**
///
/// 1. *The scene that did not exist -- and now does.* This constant used to
///    argue 16 from "a realm's scene holds a full RGBA copy of its client's
///    buffer ... ~262 MB at sixteen realms", when the core held exactly one
///    [`crate::scene::Scene`] shared by every realm. The correction was
///    right at the time. WS-E.1.3 then made the per-realm scene real -- and
///    also a per-realm capture cache, which no draft had counted -- so the
///    original number was accidentally close for the wrong reason and is
///    now replaced by the measured table above. The lesson kept from it:
///    **a forecast is not a measurement**, and the forecast here named one
///    copy where there are two.
/// 2. *The descriptor that was not counted.* The second argument said "2
///    permanently (the shim socketpair end, the realm `flock`)" and derived
///    ≤ 18 per realm, ≤ 288 at the cap, ~28% of `RLIMIT_NOFILE`. It omitted
///    the outbox ping eventfd, which every realm has. The table above is
///    the corrected count, re-measured for this revision.
/// 3. *The pixel claim that expired.* The third argument's table said
///    core-side pixels were "**none that scale with the realm count**".
///    That row is now the largest per-realm cost there is. It expired
///    because a *different* issue changed the data structure it described,
///    which is the failure mode a doc comment cannot prevent on its own --
///    which is why the third draft wrote down the trigger ("if WS-E.1.3
///    makes a per-realm surface copy real, re-derive") rather than only the
///    number.
///
/// **What no number here bounds.** The shim's own memory and its app's --
/// the dominant cost of a realm by a wide margin, since that is where the
/// client's buffers actually live. That is the operator's to manage; the
/// core neither measures nor limits it, and pretending a cap on realm
/// *count* constrains it would be the same class of false claim as the
/// paragraph above. Nor does anything here bound **CPU**: WS-E.1.3 decision
/// 2 has every live realm composing at the output's rate whether or not it
/// is visible, which is published as a limit
/// (`docs/book/src/limits.md`) rather than traded away.
///
/// **Unbounded is not the alternative.** A deployment that served
/// `realm_launch` with no cap would turn one launch grant into an
/// fd-exhaustion, memory-exhaustion and process-exhaustion primitive with no
/// protocol violation anywhere in the trace. The wire already has the
/// vocabulary for the refusal -- `vitrin_grant.refusal.capacity`, "the
/// deployment is at its realm capacity" -- which exists precisely because
/// the answer is a policy, not a fault.
///
/// **It is no longer only a startup bound** (WS-E.1.1, issue #207). It was,
/// while realms came only from `realm.toml`; now [`too_many_realms`] is one
/// of *two* enforcement sites, and the second is the one the paragraph
/// above was written for. `vitrin_launcher.launch` refuses `capacity` when
/// [`RealmRegistry::capacity_used`] has reached this number, so the
/// fd/memory/process bill measured above is what a launch grant is bounded
/// by rather than a bound only an operator's own file could hit.
///
/// The two sites count different things and must: the loader counts
/// `[[realm]]` tables, because it is judging a file; the launch path counts
/// realms that are **not** terminal, because an exited realm holds no shim,
/// no descriptors and no pixels, and charging a session for the sixteen
/// apps it has already closed would make the verb useless after one
/// afternoon.
pub(crate) const MAX_REALMS: usize = 16;

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

    /// **The one program name a consent prompt may render** (WS-E.1.1).
    ///
    /// [`crate::consent::PromptContent`] holds no free-text field, so that
    /// "an agent cannot put a glyph on screen" is a property of a type
    /// rather than a rule someone keeps applying. A `realm_launch` prompt
    /// has to name the program the human is approving the launching of, and
    /// [`AuditedCommand`] is how that is done without reopening the door: it
    /// wraps a `PathBuf` whose only source is a [`SpawnConfig`], and a
    /// `SpawnConfig`'s only source is [`validate_realm`] reading an
    /// operator-owned, not-group/other-writable `realm.toml` whose
    /// `command` passed [`audit_spawn_target`]. There is no constructor
    /// taking a wire string, in this module or any other.
    pub fn audited_command(&self) -> AuditedCommand {
        AuditedCommand(self.command.clone())
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

/// A program path the core itself validated, carried where a bare
/// `PathBuf` would be indistinguishable from client text.
///
/// The private field is the whole point: it is constructed at exactly one
/// site, [`SpawnConfig::audited_command`], whose receiver can only have come
/// out of `realm.toml`. See that method for the argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditedCommand(PathBuf);

impl AuditedCommand {
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// A command for tests that do not load a config file. Deliberately
    /// `#[cfg(test)]`: a release build has exactly one constructor, and a
    /// second one behind a feature flag would be a second way for a string
    /// to reach a consent card.
    #[cfg(test)]
    pub fn for_test(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

/// **A realm id the core minted for a launch instance** (WS-E.1.1, issue
/// #207): the only thing [`crate::enforcement`] will put in a
/// `vitrin_launcher.launched` event.
///
/// A newtype with a private field and exactly one constructor,
/// [`RealmRegistry::mint_instance`], because "instance ids are minted by the
/// core, never supplied" is otherwise a rule a reviewer has to keep
/// re-checking against a `String` that looks like every other `String`. As a
/// type it is a compile error: the launch arm of the chokepoint can only
/// answer with a value that came out of the registry, and no wire decode
/// produces one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MintedRealmId(RealmId);

impl MintedRealmId {
    pub fn as_realm_id(&self) -> &RealmId {
        &self.0
    }
}

impl fmt::Display for MintedRealmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A realm's lifecycle state. [`Realm::admits_petitions`] is where every
/// variant decides vacancy (module docs), and it is the only place that has
/// to change when a variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RealmState {
    /// Described by `realm.toml` and addressable; no app process yet.
    Configured,
    /// **A template** (WS-E.1.1): declared `autostart = false`, so startup
    /// never forks it and nothing ever will -- it is not a realm waiting to
    /// run, it is the *configuration* a `vitrin_launcher.launch` forks
    /// instances from.
    ///
    /// Distinct from [`RealmState::Configured`], which is a realm on its way
    /// to [`RealmState::Running`] and reaches it within one startup. Folding
    /// the two would make "this realm has no app yet" and "this realm will
    /// never have an app" the same state, and the flight recorder could then
    /// not say which of the two a `no_surface` refusal came from.
    Template,
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
    /// The **declared** realm this one was minted from, or `None` for a
    /// realm `realm.toml` declared itself (WS-E.1.1).
    ///
    /// Read by [`RealmRegistry::mint_instance`] so an instance launched from
    /// an instance is still named after the *template*: `foo.1` launching
    /// again yields `foo.9`, never `foo.1.9`. Without it, ids would nest and
    /// the 64-byte wire bound would become a function of how many times an
    /// agent had launched.
    template: Option<RealmId>,
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

    /// The **declared** realm whose configuration this realm runs: itself,
    /// for a realm `realm.toml` declared, and its template for an instance
    /// [`RealmRegistry::mint_instance`] created.
    fn template_root(&self) -> &RealmId {
        self.template.as_ref().unwrap_or(&self.id)
    }

    /// Whether this realm counts against [`MAX_REALMS`] right now: every
    /// state but the terminal one.
    ///
    /// A template counts even though it holds no process: it is a row the
    /// operator declared, and the loader's cap already counted it. An
    /// `Exited` realm does not -- it holds no shim, no descriptors and no
    /// pixels, and keeping it in the cap would mean a session that launched
    /// and closed sixteen apps could never launch again.
    fn occupies_capacity(&self) -> bool {
        !matches!(self.state, RealmState::Exited { .. })
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
            // **A template** (WS-E.1.1): `true`, and it is the one arm
            // where that answer is not about painting at all.
            //
            // A template never runs, so on the `no_surface`/`unavailable`
            // reading above it looks like the `Exited` case -- it will never
            // paint either. It is not: `unavailable` is an *addressing*
            // answer, and a template is exactly the thing a `realm_launch`
            // petition has to be able to address. Answering `unavailable`
            // here would make the launch verb unpetitionable over the only
            // realms it is meant for, and the IDL states the shape this arm
            // produces in as many words: a template "is addressable but
            // never itself paints", and a grant to observe one "refuses
            // no_surface forever, which is authority over nothing rather
            // than authority over something dangerous".
            //
            // The difference from `Exited` is what an agent should do next.
            // `unavailable` says stop asking; a template says ask, and if
            // what you asked for was `observe` you will hold authority that
            // never yields a frame. Fail-closed is untouched either way --
            // the chokepoint still refuses every capture and every
            // actuation over a realm with no live view.
            RealmState::Template => true,
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
/// Holds between one and [`MAX_REALMS`] realms, one of which is always
/// `realm-0`; the map keying is what made that multiplicity additive
/// (module docs, "Deletion or re-plumbing").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealmRegistry {
    /// Keyed by id, so lookup is by the name the wire carries and
    /// enumeration is deterministic.
    realms: BTreeMap<RealmId, Realm>,
    /// The next instance number [`Self::mint_instance`] will issue --
    /// **session-global, monotonic, never reset** (WS-E.1.1).
    ///
    /// Session-global rather than per template because uniqueness is the
    /// only property it owes and one counter gives it across every
    /// template at once; monotonic and never reused because an exited
    /// realm keeps its id for the life of the session (an `unavailable`
    /// that later became a live realm would make the answer a lie), so a
    /// per-template counter that reset on exit would reissue a name.
    ///
    /// A [`Cell`] because minting happens where the registry is borrowed
    /// **shared**: the chokepoint's launch sink is built beside a
    /// `ServerCtx` that already holds `&RealmRegistry` for petition
    /// admission, and the alternative is a second counter living outside
    /// the registry -- which would put the session's naming authority in
    /// two places, exactly the thing the module docs above say must not
    /// happen. Nothing else in this type is interior-mutable, and the cell
    /// is written by one method.
    next_instance: Cell<u64>,
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
    /// invariants (the cap, unique ids, and `realm-0`'s membership). Shared
    /// by [`load`](Self::load) and tests, so no constructor can bypass them.
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
                // `autostart = false` is a template: never forked by
                // startup, still addressable and petitionable (module docs).
                state: if spec.autostart {
                    RealmState::Configured
                } else {
                    RealmState::Template
                },
                // Declared, so it *is* a template root; only
                // [`Self::mint_instance`] sets this.
                template: None,
            };
            if realms.insert(spec.id.clone(), realm).is_some() {
                return Err(ErrorKind::Invalid(format!(
                    "duplicate realm id {:?}",
                    spec.id.as_str()
                )));
            }
        }
        // At least one realm has to actually come up (module docs,
        // `autostart = false`). Checked on the assembled set for the same
        // reason `realm-0`'s membership is: it is a property of the file,
        // and pointing at one table would name an innocent one.
        if !realms
            .values()
            .any(|realm| realm.state != RealmState::Template)
        {
            return Err(ErrorKind::Invalid(
                "every [[realm]] sets `autostart = false`, so this session would come up with \
                 no app running, no realm on the output and nothing for a human to look at. \
                 At least one realm must autostart; a template is something to launch FROM, \
                 not a session"
                    .into(),
            ));
        }
        // The membership rule that replaced the version-0 id pin (module
        // docs). Checked on the assembled set rather than per table,
        // because it is a property of the *file*, not of any one entry:
        // pointing at a table would name an innocent one.
        if !realms.contains_key(&RealmId::new(WELL_KNOWN_REALM_ID)) {
            return Err(ErrorKind::Invalid(format!(
                "no realm is named {WELL_KNOWN_REALM_ID:?}, which every configuration must \
                 include: the IDL declares it the single well-known realm of version 1 and \
                 the wire carries no way to enumerate the others, so it is the one realm \
                 name a conformant client can know without being told. A session without it \
                 answers every such client's get_realm({WELL_KNOWN_REALM_ID:?}) petition \
                 `unavailable`, forever. Declared realms: {}",
                realms
                    .keys()
                    .map(RealmId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        reject_runtime_name_collisions(&realms)?;
        Ok(Self {
            realms,
            next_instance: Cell::new(1),
        })
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

    /// **How many realms this session currently holds against
    /// [`MAX_REALMS`]** (WS-E.1.1): every realm but the terminal ones.
    ///
    /// This, not [`Self::len`], is what a launch is refused `capacity`
    /// against. `len` counts rows, and rows outlive realms on purpose --
    /// an exited realm keeps its id so `unavailable` keeps meaning *not
    /// ever* -- so a session that started and closed sixteen apps would
    /// otherwise be permanently at its cap while holding no processes at
    /// all.
    pub fn capacity_used(&self) -> usize {
        self.realms
            .values()
            .filter(|realm| realm.occupies_capacity())
            .count()
    }

    /// **Mint the id of a new instance of `template`** -- the core's half
    /// of the two naming authorities (module docs), and the only
    /// constructor of [`MintedRealmId`] there is.
    ///
    /// `None` when `template` is not a realm this registry holds, which is
    /// unreachable from the launch path (the id comes from a grant row,
    /// and rows are only ever minted over realms the registry resolved)
    /// and is surfaced as the IDL's `internal` rather than guessed at.
    ///
    /// The id is `<template-root>.<n>`: the *declared* realm's id, so an
    /// instance launched from an instance does not nest
    /// ([`Realm::template_root`]), and a session-global counter that never
    /// reissues a number. It cannot collide with a declared id, and it
    /// cannot exceed the wire's 64-byte realm-id bound -- both because
    /// [`validate_realm_id`] refused, at load, every declared id that
    /// would have made either possible. The `debug_assert` below is the
    /// tripwire for that reasoning, not the guard: the guard is at load.
    pub fn mint_instance(&self, template: &RealmId) -> Option<MintedRealmId> {
        let realm = self.realms.get(template)?;
        let n = self.next_instance.get();
        self.next_instance.set(n.saturating_add(1));
        let id = format!("{}.{n}", realm.template_root());
        debug_assert!(
            validate_transport_realm_id(&id).is_ok()
                && !self.realms.contains_key(&RealmId::new(&id)),
            "minted realm id {id:?} is illegal or already taken; the load-time rules in \
             validate_realm_id are supposed to make both unreachable"
        );
        Some(MintedRealmId(RealmId::new(id)))
    }

    /// Enter a minted instance of `template` into the registry, in
    /// [`RealmState::Configured`] and ready for [`Self::mark_running`].
    ///
    /// Takes a [`MintedRealmId`] rather than a [`RealmId`], which is what
    /// makes "a client cannot name the realm it creates" structural: there
    /// is no other way to obtain the argument, and no wire decode produces
    /// one.
    ///
    /// Returns `false` (and inserts nothing) when the template is gone --
    /// unreachable, and fail-closed rather than fabricating a realm with
    /// no configuration.
    pub fn insert_instance(&mut self, template: &RealmId, id: MintedRealmId) -> bool {
        let Some(realm) = self.instance_of(template, &id) else {
            return false;
        };
        self.realms.insert(realm.id.clone(), realm);
        true
    }

    /// **The [`Realm`] an instance of `template` would be, without entering
    /// it in the registry.**
    ///
    /// The launch path forks *before* it registers, and deliberately: a
    /// registry that gained a row for a fork that then failed would answer
    /// petitions about a realm that never existed, and the client would
    /// have been told `internal` about the same id. So the spawn is handed
    /// this value -- the template's configuration under the instance's id,
    /// which is exactly what the spawn needs (it reads `spawn()` and
    /// derives every path from `id()`).
    ///
    /// `None` when the template is gone; unreachable from the launch path,
    /// where the id came from a grant row.
    pub fn instance_of(&self, template: &RealmId, id: &MintedRealmId) -> Option<Realm> {
        let parent = self.realms.get(template)?;
        Some(Realm {
            id: id.as_realm_id().clone(),
            spawn: parent.spawn.clone(),
            state: RealmState::Configured,
            // The *declared* root, never the realm launched from: see
            // [`Realm::template`].
            template: Some(parent.template_root().clone()),
        })
    }

    /// Drop an instance whose spawn never got as far as serving.
    ///
    /// The narrow inverse of [`Self::insert_instance`], for the one window
    /// the launch path has: the fork succeeded (so the client already holds
    /// its `launched`), and the attach sequence after it did not. Removing
    /// the row rather than marking it `Exited` keeps the flight recorder
    /// honest -- `Exited` carries the pid of a process that served the
    /// realm, and this one never did -- and costs the client nothing: an
    /// unknown name and a vacant realm are one answer at petition time by
    /// design.
    ///
    /// **Instances only.** It takes a [`MintedRealmId`], so no declared
    /// realm can be removed through it and the registry stays what
    /// `realm.toml` said for the life of the session.
    pub fn remove_instance(&mut self, id: &MintedRealmId) {
        self.realms.remove(id.as_realm_id());
    }
}

/// One validated `[[realm]]` table, before it becomes a [`Realm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealmSpec {
    pub id: RealmId,
    pub spawn: SpawnConfig,
    /// `autostart` (default `true`): whether startup forks this realm's
    /// app, or it is a template to launch instances from (module docs).
    pub autostart: bool,
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
    autostart: Option<bool>,
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
            "autostart" => {
                if realm.autostart.is_some() {
                    return Err(parse_err(line_no, "duplicate `autostart` key".into()));
                }
                realm.autostart = Some(toml_subset::boolean(value, line_no)?);
            }
            other => {
                return Err(parse_err(
                    line_no,
                    format!(
                        "unknown key {other:?} (this schema defines id, command, args, \
                         env_allow, autostart)"
                    ),
                ));
            }
        }
    }

    // Cardinality before per-table semantics: how many realms this file
    // declares is a property of its shape, and complaining about what is
    // *inside* the seventeenth table buries the answer the operator needs
    // (that there should not be a seventeenth table).
    // [`RealmRegistry::from_specs`] re-checks it as the invariant no
    // constructor can bypass; this one is here for the message an operator
    // reads.
    if raw.len() > MAX_REALMS {
        return Err(too_many_realms(raw.len()));
    }

    raw.into_iter().map(validate_realm).collect()
}

/// The one wording for "this session may not hold that many realms",
/// shared by the parser and the registry so the two cannot drift.
///
/// Names the count and the cap, because an operator who hit it needs to
/// know both numbers and the reason -- the shape the old `exactly one`
/// refusal had, kept when the rule became a cap ([`MAX_REALMS`]).
fn too_many_realms(found: usize) -> ErrorKind {
    ErrorKind::Invalid(format!(
        "{found} [[realm]] tables, but this session serves at most {MAX_REALMS}: each realm is \
         a shim process, a private runtime tree, a lock and up to 18 descriptors in the core, \
         so the cap bounds how much of the trusted core's fd table one configuration file can \
         claim (see MAX_REALMS in crates/vitrin-core/src/realm.rs for the accounting)"
    ))
}

/// **Refuse a set of realm ids that would fight over an entry in the
/// runtime tree** (WS-E.1.2 review).
///
/// `$XDG_RUNTIME_DIR/vitrin-0/` is a **flat namespace**, and each realm
/// claims two entries in it: its private directory `<id>/` and its
/// ownership lock `<id>.lock`, a *sibling* of that directory rather than a
/// file inside it (`vitrin_ipc::paths` explains why). Realm ids are
/// free-form now that the `realm-0` pin is gone, so two of those claims can
/// name the same byte string:
///
/// - realm `foo.lock`'s **directory** is realm `foo`'s **lock file**. Each
///   path is individually well-formed, so neither `paths` helper can see
///   the clash -- only the set can. Left unchecked, whichever realm spawns
///   second finds the other's entry in the way and fails with a `mkdir` or
///   `flock` error naming a path but not the reason, and if it did somehow
///   succeed the two would be purging each other's state.
/// - a realm named `core.sock` (or `core.sock.lock`) collides with the
///   **listener's** entries, which are not any realm's to take.
///
/// Checked on the assembled set, like `realm-0`'s membership, and for the
/// same reason: it is a property of the *file*. The message names both ids,
/// because "these two entries are the same" is unactionable without them.
fn reject_runtime_name_collisions(realms: &BTreeMap<RealmId, Realm>) -> Result<(), ErrorKind> {
    // name -> the realm that claims it, in id order so the refusal is
    // reproducible.
    let mut claimed: BTreeMap<String, &RealmId> = BTreeMap::new();
    for reserved in vitrin_ipc::paths::reserved_runtime_names() {
        for id in realms.keys() {
            if vitrin_ipc::paths::runtime_names_claimed_by(id.as_str()).contains(&reserved) {
                return Err(ErrorKind::Invalid(format!(
                    "realm {:?} claims {reserved:?} in the session's runtime directory, which \
                     belongs to the core's own listener ($XDG_RUNTIME_DIR/vitrin-0/{reserved}). \
                     Rename the realm",
                    id.as_str()
                )));
            }
        }
    }
    for id in realms.keys() {
        for name in vitrin_ipc::paths::runtime_names_claimed_by(id.as_str()) {
            if let Some(other) = claimed.insert(name.clone(), id) {
                return Err(ErrorKind::Invalid(format!(
                    "realms {:?} and {:?} would both own {name:?} in the session's runtime \
                     directory ($XDG_RUNTIME_DIR/vitrin-0/): every realm gets a private \
                     directory <id>/ and, beside it, a lock file <id>.lock, so an id ending in \
                     .lock names another realm's lock. Rename one of them",
                    other.as_str(),
                    id.as_str()
                )));
            }
        }
    }
    Ok(())
}

/// Turn one parsed table into a validated spec, applying the documented
/// defaults and refusing everything the module docs say is refused.
fn validate_realm(raw: RawRealm) -> Result<RealmSpec, ErrorKind> {
    let parse_err = |line: usize, detail: String| ErrorKind::Parse { line, detail };

    // Shape only. *Which* names a configuration must contain is a
    // cross-table property and lives in [`RealmRegistry::from_specs`]
    // (module docs): the version-0 pin that refused every id but
    // `realm-0` was deleted here, and the membership rule that replaced
    // it cannot be answered from one table.
    let id = match raw.id {
        Some((text, line)) => {
            validate_realm_id(&text).map_err(|detail| parse_err(line, detail))?;
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
        // Default `true`: every configuration written before this key
        // existed means exactly what it always meant, and the surprising
        // reading -- a file whose realms silently do not start -- is the
        // one an operator has to ask for.
        autostart: raw.autostart.unwrap_or(true),
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
    validate_transport_realm_id(id)?;
    // The two rules that keep the operator's names and the core's apart
    // (module docs, "Two naming authorities"). Both are load-time refusals
    // precisely so the collision they prevent is unrepresentable at run
    // time rather than checked for on every launch.
    if looks_like_an_instance_id(id) {
        return Err(format!(
            "`id` {id:?} has the shape the core mints for LAUNCH INSTANCES \
             (<template>.<number>, optionally with this session's .lock suffix beside it), so \
             it could collide with a realm `vitrin_launcher.launch` creates -- either taking \
             that realm's private runtime directory or its lock file. Configuration names \
             templates and the core names instances; rename this one"
        ));
    }
    if id.len() > MAX_DECLARED_ID_BYTES {
        return Err(format!(
            "`id` {id:?} is {} bytes; a realm declared in this file may be at most {} so that \
             the core can append the instance suffix it mints (`.<number>`, up to {} bytes) \
             and still produce a realm id the wire can carry (64 bytes, vitrin_realm)",
            id.len(),
            MAX_DECLARED_ID_BYTES,
            MAX_INSTANCE_SUFFIX
        ));
    }
    Ok(())
}

/// The **transport's** realm-id rule, and only that: at most 64 bytes over
/// `[A-Za-z0-9._-]`, never `.` or `..`.
///
/// Split out from [`validate_realm_id`] because the two callers ask
/// different questions. A *declared* id must additionally not look like,
/// and must leave room for, an id the core mints; a *minted* id is
/// instance-shaped by construction and only has to be legal on the wire and
/// as a directory name. Applying the declared rules to a minted id would
/// reject every id the core produces, which is exactly the assertion
/// failure this split fixed.
fn validate_transport_realm_id(id: &str) -> Result<(), String> {
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

/// The most bytes [`RealmRegistry::mint_instance`] can append: a `.` plus
/// the 20 decimal digits of `u64::MAX`.
///
/// Stated as the counter's worst case rather than as a plausible one. A
/// session cannot realistically reach `u64::MAX` launches, but the bound
/// that keeps a minted id inside the wire's 64 bytes must not depend on
/// anyone's estimate of how many times an agent will call `launch`.
const MAX_INSTANCE_SUFFIX: usize = 1 + 20;

/// The most bytes a realm id **declared in `realm.toml`** may have: the
/// wire's 64-byte realm-id bound less the suffix the core may append.
const MAX_DECLARED_ID_BYTES: usize = 64 - MAX_INSTANCE_SUFFIX;

/// Whether `id` has the shape [`RealmRegistry::mint_instance`] produces, or
/// the shape of such an id's runtime **lock** entry.
///
/// Both matter, and only the second is non-obvious: the runtime tree is
/// flat and every realm claims `<id>` *and* `<id>.lock`
/// ([`reject_runtime_name_collisions`]), so a declared realm named
/// `foo.1.lock` would own minted realm `foo.1`'s lock file even though its
/// own name ends in letters.
fn looks_like_an_instance_id(id: &str) -> bool {
    let stem = id.strip_suffix(".lock").unwrap_or(id);
    let Some((prefix, tail)) = stem.rsplit_once('.') else {
        return false;
    };
    !prefix.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit())
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

    /// A registry holding exactly these realms, all `Configured` -- what a
    /// `realm.toml` naming them would produce. The fixture other modules'
    /// tests build their realm environment from, so no test invents a realm
    /// the loader could not have produced.
    ///
    /// Bypasses the loader's **cross-table** rules deliberately (the
    /// [`MAX_REALMS`] cap and `realm-0`'s mandatory membership): a caller
    /// that needs a registry serving only `"kiosk"` is testing addressing,
    /// not configuration validity, and [`RealmRegistry::from_specs`] plus
    /// this module's own tests are where those rules are enforced and
    /// checked. [`registry_with`] is the id-only shorthand over it.
    pub(crate) fn registry_of(realms: Vec<Realm>) -> RealmRegistry {
        RealmRegistry {
            realms: realms.into_iter().map(|r| (r.id.clone(), r)).collect(),
            next_instance: Cell::new(1),
        }
    }

    pub(crate) fn registry_with(ids: &[&str]) -> RealmRegistry {
        registry_of(
            ids.iter()
                .map(|id| realm_with_spawn(id, Path::new("/usr/bin/true"), &[], &[]))
                .collect(),
        )
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
            template: None,
        }
    }

    /// The same realm declared `autostart = false` -- a [`RealmState::Template`]
    /// (WS-E.1.1). Startup must not fork it; a `realm_launch` grant over it
    /// must still be petitionable.
    pub(crate) fn template_with_spawn(id: &str, command: &Path, args: &[String]) -> Realm {
        Realm {
            state: RealmState::Template,
            ..realm_with_spawn(id, command, args, &[])
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
        // which is what made the multi-realm phase additive on this side of
        // the module (module docs, "Deletion or re-plumbing"). Asserted on a
        // registry serving a name that is *not* the well-known one: a
        // hardcoded "realm-0" comparison would pass this backwards.
        //
        // The loader would refuse this *file* -- `realm-0` is a mandatory
        // member, asserted just below -- and the model underneath is
        // deliberately not narrowed to match, which is what let the pin come
        // out without the registry moving.
        let registry = registry_with(&["kiosk"]);
        assert_eq!(
            registry.resolve_for_petition("kiosk"),
            Some(&RealmId::new("kiosk"))
        );
        assert_eq!(registry.resolve_for_petition(WELL_KNOWN_REALM_ID), None);
    }

    /// **Replaces `the_realm_id_is_the_one_the_idl_fixes_for_this_version`**
    /// (WS-E.1.2, issue #208). The old rule was "`id` may only be
    /// `realm-0`"; the new one is "`realm-0` must be among the ids, and the
    /// rest are free-form". The IDL argument did not go away -- a session
    /// with no `realm-0` still answers every conformant client's
    /// `get_realm("realm-0")` petition `unavailable` forever -- so the test
    /// that carried it is rewritten rather than deleted.
    #[test]
    fn realm_0_is_required_and_every_other_id_is_free_form() {
        // A configuration with no realm-0 is refused, and the refusal makes
        // the same argument the old one did.
        let err = registry_from("[[realm]]\nid = \"kiosk\"\ncommand = \"/usr/bin/true\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("kiosk"), "must list what was declared: {err}");
        assert!(err.contains("realm-0"), "must name what is missing: {err}");
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

        // ...and with realm-0 present, an operator-chosen name beside it is
        // ordinary: the pin is gone, not merely relaxed.
        let registry = registry_from(
            "[[realm]]\ncommand = \"/usr/bin/true\"\n\
             [[realm]]\nid = \"kiosk\"\ncommand = \"/usr/bin/true\"\n",
        )
        .unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry.resolve_for_petition("kiosk"),
            Some(&RealmId::new("kiosk"))
        );
        assert_eq!(
            registry.resolve_for_petition(WELL_KNOWN_REALM_ID),
            Some(&RealmId::new(WELL_KNOWN_REALM_ID))
        );

        // Two tables that both default their id ask for the same realm
        // twice, and are refused as the duplicate they are.
        let dup = registry_from(&format!("{MINIMAL}{MINIMAL}"))
            .unwrap_err()
            .to_string();
        assert!(dup.contains("duplicate realm id"), "unexpected: {dup}");
    }

    /// **A four-table `realm.toml` loads every realm** -- acceptance
    /// criterion 1 of issue #208, and the shape the runtime's per-realm
    /// spawn loop consumes.
    #[test]
    fn a_four_table_config_loads_four_distinct_realms() {
        let registry = registry_from(
            "[[realm]]\ncommand = \"/usr/bin/true\"\n\
             [[realm]]\nid = \"editor\"\ncommand = \"/usr/bin/false\"\nargs = [\"-e\"]\n\
             [[realm]]\nid = \"browser\"\ncommand = \"/usr/bin/true\"\nenv_allow = [\"HOME\"]\n\
             [[realm]]\nid = \"term-1\"\ncommand = \"/usr/bin/true\"\n",
        )
        .unwrap();
        assert_eq!(registry.len(), 4);

        // Enumeration is deterministic (BTreeMap, id order), which is what
        // makes the runtime's spawn order and this assertion stable.
        assert_eq!(
            registry.iter().map(|r| r.id().as_str()).collect::<Vec<_>>(),
            ["browser", "editor", "realm-0", "term-1"]
        );

        // Each realm owns its own spawn configuration -- the per-realm
        // ownership the registry always had, now exercised by more than one.
        assert_eq!(
            registry.get("editor").unwrap().spawn().command(),
            Path::new("/usr/bin/false")
        );
        assert_eq!(registry.get("editor").unwrap().spawn().args(), ["-e"]);
        assert_eq!(
            registry.get("browser").unwrap().spawn().env_allow(),
            ["HOME"]
        );
        assert!(registry.get("term-1").unwrap().spawn().args().is_empty());

        // And every one of them is independently addressable and
        // petitionable -- a real map lookup per name, not one realm wearing
        // four labels.
        for id in ["realm-0", "editor", "browser", "term-1"] {
            assert_eq!(
                registry.resolve_for_petition(id),
                Some(&RealmId::new(id)),
                "{id} must resolve"
            );
        }
        assert_eq!(registry.resolve_for_petition("absent"), None);

        // State is per realm: marking one exited leaves the others
        // petitionable, which is the registry half of "killing one realm
        // does not disturb the others".
        let mut registry = registry;
        assert!(registry.mark_exited(&RealmId::new("editor"), 4242));
        assert_eq!(registry.resolve_for_petition("editor"), None);
        for id in ["realm-0", "browser", "term-1"] {
            assert_eq!(
                registry.resolve_for_petition(id),
                Some(&RealmId::new(id)),
                "{id} must survive a sibling's death"
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
        // Shape-legal and not the well-known name: no longer a refusal at
        // all, provided `realm-0` is also present. The id pin used to fire
        // here; the shape check is what survives, and this is the pair that
        // shows the two were always distinct.
        // (`realm.0` would have served here until WS-E.1.1 (issue #207)
        // gave the core the `<template>.<number>` instance shape and
        // refused declared ids that could collide with it; `realm.zero`
        // makes the same point -- dotted, not well-known, accepted.)
        let registry = registry_from(
            "[[realm]]\ncommand = \"/a\"\n[[realm]]\nid = \"realm.zero\"\ncommand = \"/a\"\n",
        )
        .unwrap();
        assert_eq!(
            registry.resolve_for_petition("realm.zero"),
            Some(&RealmId::new("realm.zero"))
        );
    }

    /// **Ids that would fight over one entry in the runtime tree are
    /// refused, naming both** (WS-E.1.2 review, MEDIUM 5).
    ///
    /// Deleting the `realm-0` pin left only a *shape* check on ids, and the
    /// runtime directory is flat: a realm's lock is `<id>.lock`, a sibling
    /// of its directory `<id>/`, so realm `foo.lock` and realm `foo` name
    /// the same entry. Individually both ids are well-formed and both paths
    /// are well-formed; only the set can see it, which is why the refusal
    /// lives on the assembled registry.
    #[test]
    fn realm_ids_that_collide_in_the_runtime_tree_are_refused_naming_both() {
        // A realm's directory against another realm's lock file.
        let err = registry_from(
            "[[realm]]\ncommand = \"/a\"\n\
             [[realm]]\nid = \"foo\"\ncommand = \"/a\"\n\
             [[realm]]\nid = \"foo.lock\"\ncommand = \"/a\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"foo\""), "must name the first id: {err}");
        assert!(
            err.contains("\"foo.lock\""),
            "must name the second id: {err}"
        );
        assert!(
            err.contains("runtime directory"),
            "must say where they collide: {err}"
        );

        // ...and the same class against the entries the listener owns,
        // which belong to no realm at all.
        for reserved in ["core.sock", "core.sock.lock"] {
            let err = registry_from(&format!(
                "[[realm]]\ncommand = \"/a\"\n\
                 [[realm]]\nid = \"{reserved}\"\ncommand = \"/a\"\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(reserved) && err.contains("listener"),
                "a realm named {reserved:?} must be refused as the listener's: {err}"
            );
        }

        // The near misses stay legal: dots are ordinary id characters, and
        // only the *derived* pair may not clash. `foo.lock` alone is fine --
        // there is no realm `foo` for it to collide with.
        for extra in ["foo", "foo.lock", "core.socket", "lock", "a.b.c"] {
            registry_from(&format!(
                "[[realm]]\ncommand = \"/a\"\n\
                 [[realm]]\nid = \"{extra}\"\ncommand = \"/a\"\n"
            ))
            .unwrap_or_else(|e| panic!("{extra:?} must stay legal: {e}"));
        }
    }

    #[test]
    fn the_cardinality_rule_is_a_cap_and_it_names_both_numbers() {
        // Zero realms: the core has nothing to serve. Unchanged.
        let none = registry_from("# just a comment\n").unwrap_err().to_string();
        assert!(none.contains("no [[realm]] table"), "unexpected: {none}");

        // Exactly the cap loads -- the boundary on the legal side, so a
        // future off-by-one in either direction is visible here.
        let mut at_cap = String::new();
        for n in 0..MAX_REALMS {
            at_cap.push_str(&format!(
                "[[realm]]\nid = \"realm-{n}\"\ncommand = \"/usr/bin/true\"\n"
            ));
        }
        assert_eq!(registry_from(&at_cap).unwrap().len(), MAX_REALMS);

        // One over: refused by *cardinality*, naming the file's count and
        // the cap -- the refusal shape the old `exactly one` message had.
        // Counted before the tables are validated, so this is the answer the
        // operator gets whatever is inside the last one.
        let over = format!("{at_cap}[[realm]]\nid = \"one-too-many\"\ncommand = \"/x\"\n");
        let err = registry_from(&over).unwrap_err().to_string();
        assert!(
            err.contains(&format!("{} [[realm]] tables", MAX_REALMS + 1)),
            "must name the count: {err}"
        );
        assert!(
            err.contains(&format!("at most {MAX_REALMS}")),
            "must name the cap: {err}"
        );
        assert!(
            err.contains("descriptors in the core"),
            "must say why there is a cap at all, from something true: {err}"
        );

        // And enforced again by the constructor, so a caller that skips the
        // parser cannot build a registry over the cap.
        let specs: Vec<RealmSpec> = parse_config(&at_cap)
            .unwrap()
            .into_iter()
            .chain(parse_config("[[realm]]\nid = \"extra\"\ncommand = \"/x\"\n").unwrap())
            .collect();
        assert!(RealmRegistry::from_specs(specs).is_err());

        // The membership rule is likewise re-checked by the constructor, so
        // no path builds a registry a conformant client cannot address.
        assert!(RealmRegistry::from_specs(
            parse_config("[[realm]]\nid = \"kiosk\"\ncommand = \"/x\"\n").unwrap()
        )
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

    #[test]
    fn every_realm_in_a_multi_table_file_is_audited_from_disk() {
        // The spawn-target audit is per realm, not per file: three tables
        // mean three `audit_spawn_target` walks, and any one of them failing
        // aborts startup. That posture is deliberate (module docs) and it is
        // what makes a mistyped path in the *third* table stop the desktop
        // coming up rather than producing a realm that cannot start.
        let _fd = crate::capture::tests::fd_lock();
        let dir = scratch_dir();
        let good = program_in(&dir, "app", 0o755);
        let wide = program_in(&dir, "wide", 0o757);

        let ok = config_in(
            &dir,
            &format!(
                "[[realm]]\ncommand = \"{p}\"\n\
                 [[realm]]\nid = \"editor\"\ncommand = \"{p}\"\n\
                 [[realm]]\nid = \"browser\"\ncommand = \"{p}\"\n",
                p = good.display()
            ),
            0o600,
        );
        let registry = RealmRegistry::load(&ok).unwrap();
        assert_eq!(registry.len(), 3);
        for id in ["realm-0", "editor", "browser"] {
            assert_eq!(registry.get(id).unwrap().spawn().command(), good);
        }

        // The third table's program is group/other-writable: the whole load
        // fails, and the message names *that* program rather than the file's
        // first one.
        let bad = config_in(
            &dir,
            &format!(
                "[[realm]]\ncommand = \"{good}\"\n\
                 [[realm]]\nid = \"editor\"\ncommand = \"{good}\"\n\
                 [[realm]]\nid = \"browser\"\ncommand = \"{wide}\"\n",
                good = good.display(),
                wide = wide.display()
            ),
            0o600,
        );
        let err = RealmRegistry::load(&bad).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Insecure(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains(&wide.display().to_string()),
            "must name the offending realm's program: {text}"
        );
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

    // -- templates and instance ids (WS-E.1.1, issue #207) ------------------

    #[test]
    fn autostart_false_loads_a_template_that_still_admits_petitions() {
        let registry = registry_from(
            "[[realm]]\ncommand = \"/a\"\n\
             [[realm]]\nid = \"kiosk\"\ncommand = \"/b\"\nautostart = false\n",
        )
        .expect("a template beside an autostarting realm loads");
        assert_eq!(registry.get("kiosk").unwrap().state(), RealmState::Template);
        // Addressable: the whole point is that `realm_launch` can be
        // petitioned over it. A template answering `unavailable` would make
        // the verb unpetitionable over exactly the realms it is for.
        assert_eq!(
            registry.resolve_for_petition("kiosk"),
            Some(&RealmId::new("kiosk"))
        );
        // ...and the realm that did not say so is unchanged.
        assert_eq!(
            registry.get(WELL_KNOWN_REALM_ID).unwrap().state(),
            RealmState::Configured,
            "autostart defaults to true: a file written before the key existed means \
             exactly what it always meant"
        );
    }

    #[test]
    fn a_config_of_nothing_but_templates_is_refused() {
        let err = registry_from(
            "[[realm]]\ncommand = \"/a\"\nautostart = false\n\
             [[realm]]\nid = \"kiosk\"\ncommand = \"/b\"\nautostart = false\n",
        )
        .expect_err("a session with no app to run must not come up");
        assert!(
            format!("{err}").contains("autostart"),
            "the refusal has to name the key an operator would change: {err}"
        );
    }

    #[test]
    fn autostart_accepts_only_the_two_toml_spellings() {
        for value in ["True", "yes", "1", "\"false\"", "on"] {
            let err = registry_from(&format!(
                "[[realm]]\ncommand = \"/a\"\nautostart = {value}\n"
            ))
            .expect_err("only `true` and `false` are TOML booleans");
            assert!(
                matches!(err, ErrorKind::Parse { .. }),
                "{value} must be a parse refusal, not a silent reading: {err:?}"
            );
        }
    }

    /// **A declared id that could collide with a core-minted instance id is
    /// refused at load**, which is what makes the collision unrepresentable
    /// afterwards rather than checked for on every launch.
    #[test]
    fn declared_ids_shaped_like_instance_ids_are_refused() {
        // `foo.1` would own minted realm `foo.1`'s private directory;
        // `foo.1.lock` would own its lock file, which sits *beside* that
        // directory in the flat runtime tree.
        for id in ["foo.1", "foo.1.lock", "realm-0.10", "a.b.7"] {
            let err = registry_from(&format!(
                "[[realm]]\ncommand = \"/a\"\n[[realm]]\nid = \"{id}\"\ncommand = \"/b\"\n"
            ))
            .expect_err("an instance-shaped declared id must be refused");
            assert!(
                format!("{err}").contains("LAUNCH INSTANCES"),
                "{id} must be refused as instance-shaped: {err}"
            );
        }
        // ...and ids that merely *contain* digits or dots are untouched.
        for id in ["term-1", "realm.zero", "a.b", "v2.x", "1.a"] {
            registry_from(&format!(
                "[[realm]]\ncommand = \"/a\"\n[[realm]]\nid = \"{id}\"\ncommand = \"/b\"\n"
            ))
            .unwrap_or_else(|e| panic!("{id} is not instance-shaped and must load: {e}"));
        }
    }

    #[test]
    fn a_declared_id_with_no_room_for_an_instance_suffix_is_refused() {
        // Shape-legal on the wire (<= 64 bytes) but too long to carry the
        // suffix the core appends, so minting could produce an id the wire
        // cannot express. Told to the operator at load instead.
        let id = "a".repeat(MAX_DECLARED_ID_BYTES + 1);
        let err = registry_from(&format!(
            "[[realm]]\ncommand = \"/a\"\n[[realm]]\nid = \"{id}\"\ncommand = \"/b\"\n"
        ))
        .expect_err("an id with no room for the instance suffix must be refused");
        assert!(format!("{err}").contains("instance suffix"), "{err}");
        // The longest id that *does* fit still loads, so the bound is the
        // stated one rather than an off-by-one.
        let id = "a".repeat(MAX_DECLARED_ID_BYTES);
        registry_from(&format!(
            "[[realm]]\ncommand = \"/a\"\n[[realm]]\nid = \"{id}\"\ncommand = \"/b\"\n"
        ))
        .expect("the stated maximum must load");
    }

    #[test]
    fn minted_instance_ids_are_unique_and_named_for_the_declared_template() {
        let mut registry = registry_with(&[WELL_KNOWN_REALM_ID, "kiosk"]);
        let first = registry.mint_instance(&RealmId::new("kiosk")).unwrap();
        let second = registry.mint_instance(&RealmId::new("kiosk")).unwrap();
        assert_eq!(
            (first.to_string(), second.to_string()),
            ("kiosk.1".to_string(), "kiosk.2".to_string())
        );
        // The counter is session-global, so a different template continues
        // it rather than restarting -- uniqueness is the only property it
        // owes, and one counter gives it across every template at once.
        let other = registry
            .mint_instance(&RealmId::new(WELL_KNOWN_REALM_ID))
            .unwrap();
        assert_eq!(other.to_string(), "realm-0.3");

        // **Instances do not nest.** Launching from an instance names the
        // declared root, so ids cannot grow past the wire's bound however
        // many times an agent launches.
        assert!(registry.insert_instance(&RealmId::new("kiosk"), first.clone()));
        let grandchild = registry.mint_instance(first.as_realm_id()).unwrap();
        assert_eq!(grandchild.to_string(), "kiosk.4");

        // An unknown template mints nothing rather than guessing one.
        assert!(registry.mint_instance(&RealmId::new("absent")).is_none());
    }

    #[test]
    fn capacity_counts_live_realms_and_forgets_exited_ones() {
        let mut registry = registry_with(&[WELL_KNOWN_REALM_ID, "kiosk"]);
        assert_eq!(registry.capacity_used(), 2);
        // A template costs a row and counts: the loader already counted it.
        let mut with_template = registry_of(vec![
            realm_with_spawn(WELL_KNOWN_REALM_ID, Path::new("/usr/bin/true"), &[], &[]),
            template_with_spawn("kiosk", Path::new("/usr/bin/true"), &[]),
        ]);
        assert_eq!(with_template.capacity_used(), 2);
        assert!(with_template.mark_running(&RealmId::new("kiosk"), 42));
        assert_eq!(with_template.capacity_used(), 2);
        // An exited realm keeps its row -- `unavailable` must keep meaning
        // *not ever* -- and stops costing capacity, or a session that
        // launched and closed sixteen apps could never launch again.
        assert!(registry.mark_exited(&RealmId::new("kiosk"), 7));
        assert_eq!(registry.capacity_used(), 1);
        assert_eq!(
            registry.len(),
            2,
            "the row survives so the name stays taken"
        );
    }

    #[test]
    fn an_instance_runs_the_templates_program_under_its_own_id() {
        let mut registry = registry_of(vec![
            realm_with_spawn(WELL_KNOWN_REALM_ID, Path::new("/usr/bin/true"), &[], &[]),
            template_with_spawn(
                "kiosk",
                Path::new("/usr/bin/kiosk"),
                &["--fullscreen".to_string()],
            ),
        ]);
        let minted = registry.mint_instance(&RealmId::new("kiosk")).unwrap();
        let instance = registry
            .instance_of(&RealmId::new("kiosk"), &minted)
            .expect("the template exists");
        assert_eq!(instance.id().as_str(), "kiosk.1");
        assert_eq!(instance.spawn().command(), Path::new("/usr/bin/kiosk"));
        assert_eq!(instance.spawn().args(), ["--fullscreen"]);
        // An instance is never itself a template: it is `Configured`, on its
        // way to `Running`, so a capture over it is judged by liveness
        // rather than by "this realm never runs".
        assert_eq!(instance.state(), RealmState::Configured);

        // ...and a spawn that never served is removed rather than marked
        // `Exited`, whose `pid` names a process that *did* serve.
        assert!(registry.insert_instance(&RealmId::new("kiosk"), minted.clone()));
        assert!(registry.get("kiosk.1").is_some());
        registry.remove_instance(&minted);
        assert!(registry.get("kiosk.1").is_none());
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
