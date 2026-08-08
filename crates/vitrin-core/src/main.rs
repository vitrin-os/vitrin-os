// SPDX-License-Identifier: MPL-2.0
//! `vitrind` — the Vitrin OS trusted core.
//!
//! This binary is the entire Trusted Computing Base (TCB) of a Vitrin
//! session: compositor, capability kernel, grant store, realm/spawn manager,
//! and consent surface (PRD Doc 2 §2). Everything else — per-app shims,
//! agents, shells — is untrusted and talks to this process over the
//! capability protocol (`protocol/vitrin-v0.xml`).
//!
//! TCB discipline, enforced from this first commit:
//!
//! - **No policy in the core.** Window management, decoration, and theming
//!   run in unprivileged components outside this binary (PRD Doc 2 §2 — the
//!   Nitpicker/Qubes lesson). Client-surface layout (P1.3.3) is trivial —
//!   single maximized, no decorations — and quarantined in `scene::layout`,
//!   which is doc-marked as not the core's job long-term.
//! - **One enforcement chokepoint.** When capture (P1.3.6) and actuation
//!   (P1.3.7) land, every request is checked at a single site against the
//!   grant table. No second authority-checking code path.
//! - **Budgeted dependencies** (plan risk R7): the core links Smithay (its
//!   winit and pixman-software-renderer backends), calloop, a tracing
//!   subscriber, one pure-Rust hash crate for the flight recorder's
//!   observation digests (P1.4.5), and one pure-Rust glyph rasterizer for the
//!   consent prompt (P1.7.1 — fontdue, `default-features = false`, whose only
//!   transitive dependency is ttf-parser). Nothing else, at runtime or in
//!   tests, and each of those five carries its justification against R7 in
//!   `Cargo.toml`. Note what is *not* there: no GUI toolkit and no vector
//!   rasterizer — the consent surface composites its own shapes with integer
//!   byte assembly (`consent::canvas`), exactly as `scene` does. The headless
//!   capture golden asserts against the deterministic test pattern
//!   in-process, so it needs no image codec; PNG serialization is the SDK's
//!   job (P1.8.2), never the core. There is deliberately no serialization
//!   framework: the recorder's JSON-lines emitter is hand-rolled over a
//!   closed set of entry shapes.
//!
//! Scope so far: two presentation backends presenting the same composed
//! realm view (`scene`, P1.3.3 — single maximized surface, layout policy
//! quarantined in `scene::layout`; with no client surface committed the view
//! is the deterministic test pattern). Nested mode (`vitrind --nested`) runs
//! the core as a client of the host compositor, presenting one host window at
//! the host's frame cadence (P1.3.1). Headless mode (`vitrind --headless
//! --size WxH`) drives a fixed-size virtual output composited entirely in
//! software (pixman) and retained in memory for capture, with no display or
//! GPU (P1.3.2) — the path CI runs on. The shim-facing protocol server that
//! feeds the scene real client buffers exists (`shim`, P1.3.4, exercised
//! end-to-end by the `vitrin-mock-shim` fixture) and goes live when the
//! realm spawn manager provides the inherited socketpair (P1.5.2). The
//! consent prompt the core draws for a pending petition exists too
//! (`consent`, P1.7.1) and composites above the realm view at the backend
//! output stage — human-visible output only, never a capture — together with
//! its input grab and decision routing (`consent::grab`, P1.7.2): a shown
//! prompt owns physical input exclusively, and a click on one of its buttons
//! resolves the petition through the same state machine every other consent
//! path uses.
//!
//! The runtime wiring (`session`, P1.M1.1) is what makes all of that
//! reachable: a running `vitrind` binds the core socket, accepts principal
//! connections, drives a `PrincipalServer` per connection against the shared
//! capability kernel, forks the configured realm's shim and services its
//! socketpair, and sweeps expired petitions and grants on an armed timer. A
//! realm's whole life — spawn, crash, orderly shutdown — runs through
//! `RealmLifecycle`.
//!
//! Interactive consent is wired end to end (issue #90): under
//! `--consent=interactive` the nested backend's `service_consent` raises the
//! front pending petition's prompt each dispatch round, a physical click
//! becomes a petition resolution, and the sweep timeout is now only the
//! fallback for a prompt a human never answers. `--headless` cannot host a
//! prompt (no display, no physical input device) and so is refused with
//! `--consent=interactive` at startup; auto-approve is the headless policy.

/// The human's attention key (WS-E.1.7, issue #232): the core's **second**
/// physical chord — a tapped, consumed Super that opens a short, single-use
/// window in which the two layout verbs are not refused `preempted`, and that
/// delivers one argument-free `vitrin_principal.attention` event to every
/// principal holding layout authority. It **delegates nothing**: it is the
/// human stating that their own hand is off the app, which withdraws a
/// transient courtesy, and every authority exercised afterwards came from a
/// grant approved on a consent card. Kept structurally apart from `deadman`
/// at every level — see that module's "neighbour, never sibling" table.
mod attention;
mod backend;
/// The general **modifier-aware chord matcher** (WS-E.2.1, issue #213):
/// modifier state tracked from the keysyms `input` already delivers, exact
/// set-equality matching, and the physical-origin razor at both halves. Shared
/// infrastructure by decision, not by accident — `deadman::Chord` excludes
/// every modifier for its own good reason and `attention::AttentionChord` is
/// nothing but modifiers, so neither can express `ctrl+shift+insert`, and
/// D-024 makes this the matcher WS-E.2.2 and WS-E.2.4 consume rather than
/// each adding a parallel one to the stack the human's off-switch lives in.
mod chord;
/// The **cross-realm clipboard** (WS-E.2.1, issue #213, D-024): one core-held
/// slot, `text/plain;charset=utf-8` only, capped and time-limited, filled only
/// by the human's promote chord and read only by their offer chord. The core
/// PULLS — a shim can never push, because filling the slot needs a
/// `PendingPromotion` only a human's gesture mints — and it is the first place
/// the trusted core retains bytes an application authored, which is a cost the
/// maintainer accepted knowingly rather than one this module mitigates away.
mod clipboard;
/// Capture-frame mechanics (P1.3.6): the sealed-memfd pixel path behind
/// `vitrin_view.frame_ready`. Pure mechanics — the authority decision on
/// every capture lives in `enforcement` (P1.4.4), whose single-path test
/// pins that this module's entry has no other caller. Runtime-reachable
/// since the M1.1 wiring: `session` feeds the chokepoint the backend's
/// cached realm-view readback, refreshed at redraw time so capture stays the
/// pure read of "what the compositor last finished".
mod capture;
/// The consent surface's renderer (P1.7.1): the prompt the trusted core draws
/// itself — requesting principal, realm, verbs, expiry, and the MVP consent
/// ladder's only three choices — composited above the realm view at the
/// **backend output stage**, so it reaches the human's display and, by
/// construction, never a `vitrin_view.frame_ready` capture. Read its module
/// docs before believing that last claim: it rests on where the code runs
/// (`backend::compose_human_visible`, and the headless backend's two retained
/// images), not on a check. Also the input grab and decision routing
/// (`consent::grab`, P1.7.2): while a prompt is shown all physical input
/// routes exclusively to it, and a click on a button becomes a petition
/// resolution. Wired at runtime now (issue #90): the nested backend's
/// `session::RuntimeHost::service_consent` runs `session::service_consent_round`
/// once per dispatch round, which raises the front pending petition's prompt
/// through `ConsentGrab::raise`, drains the human's decisions into the petition
/// state machine, and lowers stale cards. Hold-Esc revocation (P1.7.3) is also
/// wired; a trusted indicator (issue #85) is what remains.
#[cfg_attr(not(test), allow(dead_code))]
mod consent;
/// The enforcement chokepoint (P1.4.4): THE single function every capture
/// and actuation passes through — `connection → principal → grant → verbs
/// → constraints` — with the per-grant token bucket, the one
/// `vitrin_grant.refused` emission site, and the admitted-operation
/// dispatch (frame delivery / origin-tagged actuation intake). Runtime-
/// reachable since the M1.1 wiring: every message a real agent sends over the
/// core socket passes through it, and its admitted actuations leave through
/// `session`'s seat routing toward the realm's shim.
mod enforcement;
/// The agent cursor sprite (D-019): the crosshair the core paints at an
/// agent's own pointer position, into human-visible output only. One geometry
/// function, read by both human-visible presentation paths (the CPU composite
/// and the nested zero-copy dmabuf draw list) so they cannot drift; clipped
/// below the trusted band, because an agent chooses its own position. Drawing
/// only -- seat delivery to the shim stays one shared position per realm view
/// (D-017's deferral stands).
mod cursor;
/// The dmabuf import path (P1.3.5): the zero-copy mechanics behind the shim
/// server's `kind=dmabuf` commits — importer seam, hostile-fd probe, GLES
/// import + probe render, copy instrumentation. Dead-code-allowed outside
/// tests for the same reason as `shim`: exercised by its tests (CI-side
/// mocks plus the env-gated real-GPU tests) today, handed a live
/// `GlesRenderer` when the realm/backend wiring lands (P1.5.2).
#[cfg_attr(not(test), allow(dead_code))]
mod dmabuf;
/// The dead-man switch (P1.7.3): holding the configured chord revokes every
/// grant in the session, effective on the very next enforcement check. Whole
/// and live in nested mode since the M1.1 wiring: the watcher rides the
/// backend's router, the timer fires, and a completed chord now reaches
/// `deadman::apply` against the session's real grant table and petition
/// registry (`session::Runtime::apply_dead_man`) instead of being logged and
/// dropped. Headless has no physical input device, structurally, so it has no
/// chord to *hold* -- see `Action::RunHeadless::dead_man`. Since issue #109,
/// a `dead-man-injector`-feature build of the headless backend can still
/// *apply* a synthesized trigger through that same entry point on SIGUSR1 --
/// the CI stand-in for the hold, never compiled into a deployment binary
/// (`deadman`'s module docs, "the test injector proves the consequence half").
mod deadman;
/// Input intake & routing (P1.3.7): origin tagging at intake (backward
/// requirement B2), view→surface coordinate mapping, and the preemption
/// hook point. The nested backend feeds it host input at runtime, and the
/// runtime wiring feeds it chokepoint-admitted agent actuations through the
/// **same** router, so a human's implicit grab and an agent's share one
/// state. Seat delivery to a live shim connection is wired
/// (`session::route_seat`) and waits only on something spawning a realm.
mod input;
/// Realm lifecycle (P1.5.3): the realm's death paths — crash detection over
/// two independent signals (socketpair EOF and `SIGCHLD`, either of which
/// may arrive first), `waitpid`-authoritative reaping through calloop's
/// `signalfd` source so no zombie accumulates, the terminal realm state
/// that makes the chokepoint's existing `no_surface` refusal true, and the
/// orderly shutdown ladder (hang up → `SIGTERM` → `SIGKILL`) that removes
/// the realm's runtime tree on the way out. Dead-code-allowed outside tests
/// Wired at all three of its plug-in sites: `child_signal_source` sits
/// beside each backend's `SIGINT`/`SIGTERM` source (both installed while the
/// process is still single-threaded — see `block_loop_signals`),
/// `note_connection_closed` is the *only* shim-death path
/// (`session::close_realm` routes into it rather than doing its own
/// teardown), and `shutdown` runs the ladder at the end of each backend's
/// `run_inner`, after the loop has stopped and before the recorder is handed
/// back.
#[cfg_attr(not(test), allow(dead_code))]
mod lifecycle;
/// The grant table v0 (P1.4.2): the in-memory PRD Doc 2 §5.2 grant store of
/// the capability kernel — rows keyed by `identity`'s verifier-canonical
/// principal, answering the enforcement chokepoint's grant-scoped use query
/// and admission commit (the P1.4.4 chain: connection → principal → grant →
/// verbs → constraints), plus the embedder-polled proactive expiry sweep --
/// whose embedder is now real: `session` arms a calloop timer that polls it,
/// advisory as designed, since the chokepoint re-checks expiry at use time.///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod grants;
/// The identity layer of the capability kernel (P1.4.1): the pluggable
/// `Verifier` trait, the `principals.toml`-backed `StaticVerifier`, and the
/// principal-identity model every grant and enforcement decision keys on.
/// Loaded once at startup by `run_session` and handed to every connection's
/// `ServerCtx` as the session's one verifier -- once, because two loads of
/// the same registry are two documents, and the R6 guard must audit the one
/// the runtime verifies against.///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod identity;
/// The grant request flow (P1.4.3): the petition lifecycle state machine of
/// the capability kernel -- pending-petition registry, admission policy
/// (caps, `busy`, `unsupported`, `unavailable`), the consent-policy seam
/// (`--consent`), the consent timeout, and the build-gated scripted-consent
/// injector. `run_session` constructs the registry from the parsed consent
/// policy and `session` arms the timer that polls its timeout, so an
/// unanswered petition really does resolve `timed_out` on the wire.///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod petitions;
/// The principal-connection protocol server (P1.4.1) and the wire half of
/// the petition flow (P1.4.3): the server side of the
/// P1.1.3 handshake state machine (`vitrin_handshake` + `vitrin_principal`),
/// where `identity` binds and where the per-connection object table enforces
/// sender-constrained handles. Wired to the live listener since the M1.1
/// integration: `session` accepts on the core socket, mints one server per
/// connection, dispatches every frame against the shared kernel, and calls
/// `teardown` on all three close paths -- EOF, transport fault, and the
/// core-detected protocol violation that structurally cannot tear itself
/// down.///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod principal;
/// The realm object and realm registry (P1.5.1, widened by WS-E.1.2): one
/// to `MAX_REALMS` realms, always including `realm-0`, built at startup from
/// `realm.toml` -- the stable wire-visible
/// id `get_realm` addresses, the whole-realm grant scope grants attach to,
/// and the owner of the app's spawn configuration (which P1.5.2 executes;
/// nothing here forks). Also the single source of realm existence, which
/// petition admission consults for its `unavailable` judgement. Loaded at
/// startup below and carried into the session as a `ServerCtx` field, so a
/// petition naming a realm this file never described is refused
/// `unavailable` by the one registry the operator configured.///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod realm;
/// The flight-recorder log v0 (P1.4.5): the journal seed — a JSON-lines
/// event log of handshakes, grant lifecycle transitions, consent decisions,
/// and every enforcement decision, carrying an observation digest on every
/// delivered capture and null-versioned epoch-reference fields (backward
/// requirement B1). Explicitly NOT the signed P6 journal: no signatures, no
/// tamper evidence, never consulted by an authority decision. The run's
/// single handle is created below in `run_session`, travels through the
/// backend inside the session state (calloop fixes one state type per loop,
/// so the kernel -- recorder included -- must live in it), and comes back
/// here to be closed. Every per-connection emission site is runtime-reachable
/// since the M1.1 wiring.
///
/// Still dead-code-allowed outside tests, but for a *different and much
/// smaller* reason than before: not "no runtime caller exists" -- one does
/// now -- but that a handful of accessors and variants in it are exercised
/// only by tests. Narrow the attribute to those items, or give them callers,
/// rather than reading this as the module still being unreachable.
#[cfg_attr(not(test), allow(dead_code))]
mod recorder;
mod scene;
/// The runtime wiring (P1.M1.1, issue #77): the session state the backends'
/// event loops carry, and the sources that make the capability kernel
/// reachable from the wire -- the core socket's listener, one
/// `PrincipalServer` per accepted connection, the realm's shim socketpair
/// with its coalesced redraw, and the advisory expiry sweeps. This is the
/// module that turned a pile of tested-but-uncalled subsystems into a
/// running server.
mod session;
/// The shim-facing protocol server (P1.3.4): `vitrin_shim_session` +
/// `vitrin_shim_surface`, feeding `Scene::commit`. Fully wired:
/// `session::start_realm` forks the realm and registers its connection, and
/// `session::dispatch_shim` drives this server against the backend's scene,
/// coalescing composites so a repaint flood cannot buy one per 12-byte
/// message.
mod shim;
/// The realm spawn model (P1.5.2): the core's only process-creating code —
/// `fork`/`exec` of the realm's shim with its end of the identity socketpair
/// placed at a fixed descriptor (identity assigned at fork, never claimed),
/// a private `0700` runtime directory, and an environment built from nothing
/// that names only that realm's own socket. Read its module docs before
/// believing any confinement claim: the D9 sandboxing deferral (no
/// namespaces, no seccomp, no Landlock) and the session-D-Bus hole are
/// stated there in full. Called at runtime by `session::start_realm`, which
/// runs it only after `session::install` has put the loop's sources in
/// place: a shim spawned into a loop that is not yet servicing its
/// socketpair blocks on `configure` forever, with no timeout on its side.
mod spawn;
mod test_pattern;
/// The strict TOML subset every core configuration file is written in
/// (P1.4.1's `principals.toml`, P1.5.1's `realm.toml`): one hand-rolled
/// lexer for hostile config bytes, not one per schema. Plan risk R7 keeps a
/// TOML crate out of the TCB; having a single lexer is what keeps that
/// choice from multiplying parsers.
mod toml_subset;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::deadman::DeadManConfig;
use crate::identity::{StaticVerifier, DEMO_PRINCIPAL};
use crate::petitions::ConsentPolicy;
use crate::realm::RealmRegistry;
use crate::recorder::{Event, Recorder};

const USAGE: &str = "\
vitrind — Vitrin OS trusted core

USAGE:
    vitrind --nested            Run nested inside the host compositor (one
                                window, test pattern).
    vitrind --headless [--size WxH]
                                Run headless: a fixed-size virtual output
                                (default 1280x800) composited in software
                                (pixman) and retained in memory for capture.
    vitrind [--consent MODE]    Consent policy for grant petitions:
                                `interactive` (default; petitions await the
                                consent surface, which only `--nested` can
                                draw and answer) or `auto-approve` (every
                                petition granted as requested — headless CI
                                and demos ONLY; loudly and repeatedly logged,
                                and REFUSED unless principals.toml holds
                                nothing but the demo principal).
                                `--headless` REQUIRES `auto-approve`: it has
                                no display for the prompt and no input device
                                to answer it, so interactive is refused at
                                startup.
    vitrind [--principals PATH] Principal registry for this session (bearer
                                tokens; mode 0600, owned by the core's uid).
                                Default: $XDG_CONFIG_HOME/vitrin/principals.toml
                                Read at startup in both consent modes: the
                                listener verifies every principal against it,
                                so a registry that cannot be read is a startup
                                failure. Under `--consent=auto-approve` the
                                same load is also audited by that flag's
                                safety guard.
    vitrind [--recorder PATH]   Flight-recorder log for this run (JSON
                                lines, appended). Default:
                                $XDG_RUNTIME_DIR/vitrin-0/flight-recorder-<pid>.jsonl
                                Startup FAILS if the log cannot be opened.
    vitrind [--realm PATH]      Realm configuration for this session (one
                                [[realm]] table: command, args, env_allow).
                                Default: $XDG_CONFIG_HOME/vitrin/realm.toml
                                Startup FAILS if it is missing or malformed.
    vitrind [--shim PATH]       Shim binary the core execs to hold the realm's
                                fd-3 core connection; it in turn execs the
                                realm's `command` app (the realm never names
                                the shim -- that is the core's job). Default: a
                                sibling `vitrin-shim` beside this `vitrind`.
                                Audited transitively at spawn exactly like
                                `command` (regular file; it and every directory
                                on its path owned by root or the core's uid and
                                not writable by group or other).
    vitrind [--capture-dump PATH]
                                DIAGNOSTIC (P1.8.5): mirror every live realm's
                                composited realm-view readback to
                                PATH.<realm-id> as raw RGBA8888
                                (width*height*4, rows top-down) — the
                                core-internal capture, taken before the wire and
                                the SDK ever run. NOTHING is written to PATH
                                itself: every dump names the realm it is of, so
                                a comparison against an agent's frame cannot be
                                about a realm nobody chose (WS-E.1.3). Used by
                                the real-app fidelity test to prove the capture
                                path adds no distortion; off by default, and not
                                a wire feature. Written atomically each redraw.
    vitrind [--clipboard-key KEY]
                                Trigger key for the cross-realm clipboard
                                (WS-E.2.1): ctrl+shift+KEY promotes the focused
                                realm's selection into the core's single slot,
                                shift+KEY offers that slot to the focused realm.
                                Default: insert. Must be a layout-invariant,
                                non-modifier key this build's input intake can
                                deliver and must not collide with the dead-man
                                or attention chords; startup FAILS otherwise.
                                Both chords are CONSUMED in every realm, so an
                                app that binds shift+KEY loses it.
    vitrind [--dead-man-chord KEY]
                                Key for the dead-man switch: holding it
                                revokes every grant in the session at once.
                                Default: esc. Must be a layout-invariant,
                                non-modifier key this build's input intake
                                can deliver; startup FAILS otherwise, rather
                                than running with an off-switch that cannot
                                fire.
    vitrind [--dead-man-hold MS]
                                How long that key must be held, in
                                milliseconds. Default: 1000 (accepted range
                                250..=10000). Nested mode only: headless has
                                no physical input device, structurally.
    vitrind [--attention-chord KEY]
                                Key for the human's ATTENTION signal: tapping
                                it opens a short, single-use window in which
                                a layout_focus/layout_arrange holder is not
                                refused `preempted`, and delivers one
                                argument-free `attention` event to every such
                                holder. Default: super (also accepted:
                                rsuper). The key is CONSUMED -- no confined
                                app ever sees it. It DELEGATES NOTHING: it is
                                the human saying their own hand is off the
                                app, and every authority exercised afterwards
                                came from a grant approved on a consent card.
                                Startup FAILS on an unknown key, a key this
                                build's intake cannot deliver, or a key equal
                                to --dead-man-chord. The window's length is
                                fixed and deliberately not configurable: it
                                is a security parameter, not an ergonomics
                                one.
    vitrind --agent-cursor      `--headless` ONLY: also composite the agent
                                cursor sprite into this run's human-visible
                                output. Nested mode needs no flag -- a human
                                is watching that window -- which is why this
                                one is refused with `--nested` rather than
                                accepted as a no-op. Off by default here
                                because the headless human-visible framebuffer
                                is measured byte-for-byte against the realm
                                view by the trusted-band witness (issue #139).
                                Neither mode draws a sprite for a realm the
                                output is not bound to: with several realms
                                configured, an agent acting in a hidden realm
                                draws nothing (a published limit, WS-E.1.3).
                                The sprite NEVER reaches a captured frame in
                                either mode: it is drawn at the output stage,
                                downstream of the composite a capture reads.
    vitrind --help              Show this help.
    vitrind --version           Show the version.
    vitrind --print-isolation   Probe this kernel's confinement facilities --
                                namespaces, the Landlock ABI, seccomp filter
                                mode, no_new_privs, the distro sysctls that
                                restrict user namespaces, and whether a
                                per-uid tier is provisioned -- then exit.
                                Every row is what the kernel answered to the
                                exact request the realm spawn will make; none
                                is inferred from a version string. The output
                                is deterministic and is the source of the
                                checked-in per-kernel matrix, so it carries no
                                timestamp, hostname or pid. This flag reports;
                                it changes nothing and confines nothing.
";

/// Base name of the default flight-recorder log inside the core's runtime
/// directory; the pid keeps concurrent runs in separate files ("one log
/// file per run") without a randomness dependency.
const RECORDER_FILE_PREFIX: &str = "flight-recorder";

/// The default virtual-output size for `--headless` when `--size` is omitted;
/// matches the nested backend's initial window size so the two backends agree
/// on the same content by default.
const DEFAULT_HEADLESS_SIZE: (u32, u32) = (1280, 800);

/// How often the auto-approve banner repeats for as long as the policy is
/// active (plan risk R6: the warning must be *persistent*, not a line that
/// scrolls off before the first agent connects).
///
/// One minute is chosen against the failure it guards: an operator who left
/// auto-approve on by accident — a CI flag pasted into a desktop session, a
/// shell alias — must meet the warning again on any glance at the log, not
/// only in its first screenful. Shorter would be noise an operator learns to
/// filter, which is how a warning stops working.
const AUTO_APPROVE_BANNER_INTERVAL: Duration = Duration::from_secs(60);

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    RunNested {
        consent: ConsentPolicy,
        principals: Option<PathBuf>,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
        shim: Option<PathBuf>,
        capture_dump: Option<PathBuf>,
        dead_man: DeadManConfig,
        /// The `--attention-chord` key (WS-E.1.7): the core's second physical
        /// chord. Resolved and validated at parse time exactly as
        /// `dead_man.chord` is, including the cross-flag refusal that the two
        /// may not name the same key.
        attention: attention::AttentionChord,
        /// The `--clipboard-key` trigger (WS-E.2.1): the non-modifier half of
        /// the two cross-realm clipboard chords. Resolved and validated at
        /// parse time exactly as the two chords above are, including the
        /// cross-flag refusal against both of them.
        clipboard: chord::Trigger,
    },
    RunHeadless {
        size: (u32, u32),
        consent: ConsentPolicy,
        principals: Option<PathBuf>,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
        shim: Option<PathBuf>,
        /// The `--capture-dump PATH` diagnostic target (P1.8.5, issue #107):
        /// mirror each live realm's composited realm-view readback to
        /// `PATH.<realm-id>`, the core-internal capture the fidelity gate
        /// compares an agent's `observe()` frame against. The realm suffix is
        /// load-bearing since WS-E.1.3 (issue #209) -- with N realms an
        /// unqualified dump names *a* view and the gate's ground truth
        /// becomes a guess -- and the bare `PATH` is never written; see
        /// `session::capture_dump_path`. Valid in both modes; exercised
        /// headless.
        capture_dump: Option<PathBuf>,
        /// Parsed and validated even here, so the same command line is
        /// accepted or refused identically in both modes -- the `--consent`
        /// precedent, which headless also accepts although it can prompt
        /// nobody. Headless has no physical input device, and that absence
        /// is structural rather than a runtime check ([`crate::input`]), so
        /// there is no chord for it to *hold* here. A `dead-man-injector`
        /// build still reads this to name the synthesized trigger's
        /// chord/hold (issue #109) -- see `backend::headless::run`.
        dead_man: DeadManConfig,
        /// The `--attention-chord` key (WS-E.1.7), validated here for the same
        /// reason `dead_man` is: both modes must accept or refuse the same
        /// command line. A plain headless build has no physical input device
        /// to tap it on, so the signal never opens; a
        /// `physical-input-injector` build stacks the hook and the injector's
        /// `attention` line presses **this** chord through the production
        /// intake, which is what gives the mechanism a mock-free gate.
        attention: attention::AttentionChord,
        /// The `--clipboard-key` trigger (WS-E.2.1), validated here for the
        /// same reason `attention` is: both modes must accept or refuse the
        /// same command line. A plain headless build has no physical input
        /// device to chord on, so it never reads this past
        /// `backend::headless::run`'s signature; a `physical-input-injector`
        /// build reads it so the channel's `clipboard` line chords the key the
        /// operator actually chose.
        clipboard: chord::Trigger,
        /// `--agent-cursor` (D-019): composite the agent cursor sprite into
        /// this run's human-visible output.
        ///
        /// Headless-only, and `false` by default. Nested needs no flag --
        /// it draws the sprite for whatever position the runtime offers it --
        /// so there is no field for it on [`Action::RunNested`], and the flag
        /// is refused with `--nested` at parse time rather than accepted as a
        /// silent no-op, the same posture `--size` and
        /// `--consent-injector-fd` take. Since WS-E.1.3 the *runtime* is what
        /// withholds the position when the router's realm is not the realm
        /// the output is bound to (`session::post_dispatch`), so neither
        /// backend draws a hidden realm's sprite whatever this flag says. The default is
        /// off here because this backend's human-visible framebuffer is
        /// measured against the realm view by the trusted-band witness
        /// (issue #139) and by `tests/integration/test_real_trust_band.py`;
        /// see `backend::headless::run`'s `agent_cursor` argument.
        agent_cursor: bool,
        /// The `--consent-injector-fd N` channel (issue #138): an inherited
        /// `AF_UNIX`/`SOCK_STREAM` socketpair end on which a harness answers
        /// the consent prompts this headless session raises.
        ///
        /// `#[cfg(feature = "consent-injector")]`, so a deployment build
        /// cannot even **name** the flag: `parse_args` has no arm for it and
        /// exits with ``unknown argument `--consent-injector-fd` ``. Pinned by
        /// `a_plain_build_cannot_name_the_consent_injector_flag`.
        #[cfg(feature = "consent-injector")]
        consent_injector_fd: Option<std::os::fd::RawFd>,
        /// The `--physical-input-fd N` channel (issue #212): an inherited
        /// `AF_UNIX`/`SOCK_STREAM` socketpair end on which a harness makes
        /// physical-origin seat input happen in this headless session.
        ///
        /// `#[cfg(feature = "physical-input-injector")]`, so a deployment
        /// build cannot even **name** the flag: `parse_args` has no arm for it
        /// and exits with ``unknown argument `--physical-input-fd` ``. Pinned
        /// by `a_plain_build_cannot_name_the_physical_input_flag`.
        #[cfg(feature = "physical-input-injector")]
        physical_input_fd: Option<std::os::fd::RawFd>,
    },
    Help,
    Version,
    /// Probe this kernel's confinement facilities and exit (P2.6.1, #185).
    ///
    /// Print-and-exit like [`Action::Help`] and [`Action::Version`], and
    /// deliberately **mode-independent**: it must answer identically whether
    /// or not this machine can host a session, because its whole purpose is to
    /// report what a machine *would* grant. That is also why it carries no
    /// fields and takes no companion refusal -- there is no configuration for
    /// it to disagree with.
    PrintIsolation,
}

/// The presentation mode selected on the command line, before it is paired
/// with any `--size` into an [`Action`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Mode {
    Nested,
    Headless,
}

/// Hand-rolled argument parsing: the surface is a handful of flags, which does
/// not justify pulling an argument-parsing crate into the TCB (plan risk R7).
/// `--size` consumes the following argument (`--size 1280x800`) and is only
/// meaningful with `--headless`. `--consent` takes `MODE` as either a
/// following argument or `--consent=MODE` (the issue-#27 spelling), valid
/// with both run modes, defaulting to the fail-closed `interactive`.
/// `--recorder` follows the same two spellings (the `--consent` precedent)
/// and takes the run's flight-recorder log path (P1.4.5); omitted, the
/// default under the core's runtime directory is used.
/// `--dead-man-chord` / `--dead-man-hold` configure the P1.7.3 off-switch,
/// same two spellings again, and are **validated here rather than at first
/// use**: a chord this build's intake cannot deliver, or a hold outside the
/// defensible range, is a startup error. A session that came up with an
/// off-switch which silently never fires is the fail-open trap this
/// codebase refuses everywhere configuration is read.
fn parse_args<'a, I: IntoIterator<Item = &'a str>>(args: I) -> Result<Action, String> {
    let mut mode: Option<Mode> = None;
    let mut size: Option<(u32, u32)> = None;
    let mut consent: Option<ConsentPolicy> = None;
    let mut principals: Option<PathBuf> = None;
    let mut recorder: Option<PathBuf> = None;
    let mut realm: Option<PathBuf> = None;
    let mut shim: Option<PathBuf> = None;
    let mut capture_dump: Option<PathBuf> = None;
    let mut chord: Option<deadman::Chord> = None;
    let mut hold_ms: Option<u64> = None;
    let mut attention_chord: Option<attention::AttentionChord> = None;
    let mut clipboard_key: Option<chord::Trigger> = None;
    let mut agent_cursor = false;
    #[cfg(feature = "consent-injector")]
    let mut consent_injector_fd: Option<std::os::fd::RawFd> = None;
    #[cfg(feature = "physical-input-injector")]
    let mut physical_input_fd: Option<std::os::fd::RawFd> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg {
            "--nested" => set_mode(&mut mode, Mode::Nested)?,
            "--headless" => set_mode(&mut mode, Mode::Headless)?,
            "--size" => {
                if size.is_some() {
                    return Err("`--size` given more than once".into());
                }
                let value = args
                    .next()
                    .ok_or("`--size` requires a `WxH` value (e.g. `--size 1280x800`)")?;
                size = Some(parse_size(value)?);
            }
            "--consent" => {
                let value = args
                    .next()
                    .ok_or("`--consent` requires a mode (`interactive` or `auto-approve`)")?;
                set_consent(&mut consent, parse_consent(value)?)?;
            }
            "--principals" => {
                let value = args.next().ok_or(
                    "`--principals` requires a registry path \
                     (e.g. `--principals ~/.config/vitrin/principals.toml`)",
                )?;
                set_path(&mut principals, "--principals", "registry path", value)?;
            }
            "--recorder" => {
                let value = args
                    .next()
                    .ok_or("`--recorder` requires a log path (e.g. `--recorder /tmp/run.jsonl`)")?;
                set_path(&mut recorder, "--recorder", "log path", value)?;
            }
            "--realm" => {
                let value = args.next().ok_or(
                    "`--realm` requires a config path (e.g. `--realm ~/.config/vitrin/realm.toml`)",
                )?;
                set_path(&mut realm, "--realm", "config path", value)?;
            }
            "--shim" => {
                let value = args.next().ok_or(
                    "`--shim` requires a shim binary path (e.g. `--shim /usr/lib/vitrin/vitrin-shim`)",
                )?;
                set_path(&mut shim, "--shim", "shim path", value)?;
            }
            "--capture-dump" => {
                let value = args.next().ok_or(
                    "`--capture-dump` requires a file path (e.g. `--capture-dump /tmp/internal.rgba`)",
                )?;
                set_path(&mut capture_dump, "--capture-dump", "dump path", value)?;
            }
            "--dead-man-chord" => {
                let value = args
                    .next()
                    .ok_or("`--dead-man-chord` requires a key (e.g. `--dead-man-chord esc`)")?;
                set_chord(&mut chord, value)?;
            }
            "--dead-man-hold" => {
                let value = args.next().ok_or(
                    "`--dead-man-hold` requires a duration in ms (e.g. `--dead-man-hold 1000`)",
                )?;
                set_hold(&mut hold_ms, value)?;
            }
            "--clipboard-key" => {
                let value = args
                    .next()
                    .ok_or("`--clipboard-key` requires a key (e.g. `--clipboard-key insert`)")?;
                set_clipboard_key(&mut clipboard_key, value)?;
            }
            "--attention-chord" => {
                let value = args
                    .next()
                    .ok_or("`--attention-chord` requires a key (e.g. `--attention-chord super`)")?;
                set_attention_chord(&mut attention_chord, value)?;
            }
            // Idempotent rather than "given more than once" -- a boolean
            // switch repeated says the same thing twice, unlike a valued flag
            // where the second value would have to win or lose silently.
            "--agent-cursor" => agent_cursor = true,
            #[cfg(feature = "consent-injector")]
            "--consent-injector-fd" => {
                let value = args.next().ok_or(
                    "`--consent-injector-fd` requires an inherited descriptor number \
                     (e.g. `--consent-injector-fd 7`)",
                )?;
                set_injector_fd(&mut consent_injector_fd, "--consent-injector-fd", value)?;
            }
            #[cfg(feature = "physical-input-injector")]
            "--physical-input-fd" => {
                let value = args.next().ok_or(
                    "`--physical-input-fd` requires an inherited descriptor number \
                     (e.g. `--physical-input-fd 7`)",
                )?;
                set_injector_fd(&mut physical_input_fd, "--physical-input-fd", value)?;
            }
            "--help" | "-h" => return Ok(Action::Help),
            "--version" | "-V" => return Ok(Action::Version),
            // Returns immediately, on the `--help`/`--version` precedent, so
            // it wins over mode selection and over any later parse error. A
            // probe of what this kernel offers must not be answerable only on
            // a command line that would otherwise have run.
            "--print-isolation" => return Ok(Action::PrintIsolation),
            other => {
                #[cfg(feature = "consent-injector")]
                if let Some(value) = other.strip_prefix("--consent-injector-fd=") {
                    set_injector_fd(&mut consent_injector_fd, "--consent-injector-fd", value)?;
                    continue;
                }
                #[cfg(feature = "physical-input-injector")]
                if let Some(value) = other.strip_prefix("--physical-input-fd=") {
                    set_injector_fd(&mut physical_input_fd, "--physical-input-fd", value)?;
                    continue;
                }
                if let Some(value) = other.strip_prefix("--consent=") {
                    set_consent(&mut consent, parse_consent(value)?)?;
                } else if let Some(value) = other.strip_prefix("--principals=") {
                    set_path(&mut principals, "--principals", "registry path", value)?;
                } else if let Some(value) = other.strip_prefix("--recorder=") {
                    set_path(&mut recorder, "--recorder", "log path", value)?;
                } else if let Some(value) = other.strip_prefix("--realm=") {
                    set_path(&mut realm, "--realm", "config path", value)?;
                } else if let Some(value) = other.strip_prefix("--shim=") {
                    set_path(&mut shim, "--shim", "shim path", value)?;
                } else if let Some(value) = other.strip_prefix("--capture-dump=") {
                    set_path(&mut capture_dump, "--capture-dump", "dump path", value)?;
                } else if let Some(value) = other.strip_prefix("--dead-man-chord=") {
                    set_chord(&mut chord, value)?;
                } else if let Some(value) = other.strip_prefix("--dead-man-hold=") {
                    set_hold(&mut hold_ms, value)?;
                } else if let Some(value) = other.strip_prefix("--attention-chord=") {
                    set_attention_chord(&mut attention_chord, value)?;
                } else if let Some(value) = other.strip_prefix("--clipboard-key=") {
                    set_clipboard_key(&mut clipboard_key, value)?;
                } else {
                    return Err(format!("unknown argument `{other}`"));
                }
            }
        }
    }

    // The fail-closed default: nothing is granted without a consent
    // surface unless auto-approve was explicitly flagged.
    let consent = consent.unwrap_or(ConsentPolicy::Interactive);

    // Both halves default independently, so `--dead-man-hold 400` keeps the
    // default chord and vice versa.
    let mut dead_man = DeadManConfig::default();
    if let Some(chord) = chord {
        dead_man.chord = chord;
    }
    if let Some(ms) = hold_ms {
        dead_man = dead_man
            .with_hold_ms(ms)
            .map_err(|err| format!("`--dead-man-hold`: {err}"))?;
    }
    let attention = attention_chord.unwrap_or_else(|| {
        attention::AttentionChord::parse(attention::DEFAULT_CHORD)
            .expect("the default attention chord is in the vocabulary")
    });
    // **The two core-owned chords may not be the same key.** Unreachable
    // today, and checked anyway: the dead-man vocabulary excludes every
    // modifier and the attention vocabulary is nothing but modifiers, so they
    // are disjoint by construction (WS-E.1.7 decision 1) -- but the day an
    // edit to either list breaks that disjointness is the day one of the two
    // keys silently stops working, and one of them is the human's off-switch.
    // Refusing at startup is the same fail-closed posture `Chord::parse` takes
    // against a key intake cannot deliver.
    if attention.keysym() == dead_man.chord.keysym() {
        return Err(format!(
            "`--attention-chord` `{}` names the same key as `--dead-man-chord` `{}`: the \
             core's two physical chords must be distinct, or the one that is gated first \
             silently swallows the other (the dead-man switch is the human's off-switch \
             and must never be the loser of that race)",
            attention.name(),
            dead_man.chord.name()
        ));
    }

    let clipboard = clipboard_key.unwrap_or_else(|| {
        chord::Trigger::parse(clipboard::DEFAULT_TRIGGER)
            .expect("the default clipboard trigger is in the vocabulary")
    });
    // **The clipboard's trigger may not be either core-owned chord's key, and
    // unlike the pair above this one is genuinely reachable** (WS-E.2.1):
    // `insert` is in BOTH `deadman::Chord::VOCABULARY` and
    // `chord::Trigger::VOCABULARY`, so `--dead-man-chord insert` on a default
    // command line is a real collision an operator can type today.
    //
    // What it would do is the reason it is refused rather than ordered. The
    // dead-man watcher detects in `observe`, which is unconditional and runs
    // for events any gate consumes -- so holding ctrl+shift+insert for a second
    // would arm and fire the human's off-switch while they were copying text,
    // and no ordering of the hook stack can prevent that, because the whole
    // point of `observe` is that nothing can blind it. Fail closed at startup,
    // exactly as `Chord::parse` does for a key intake cannot deliver.
    for owned in [
        (
            dead_man.chord.keysym(),
            "--dead-man-chord",
            dead_man.chord.name(),
        ),
        (attention.keysym(), "--attention-chord", attention.name()),
    ] {
        let (keysym, flag, name) = owned;
        if clipboard.keysym() == keysym {
            return Err(format!(
                "`--clipboard-key` `{}` names the same key as `{flag}` `{name}`: the core's \
                 physical chords must not share a key. The dead-man watcher detects in the \
                 router's UNCONDITIONAL observe tap, so holding the clipboard chord would \
                 arm the human's off-switch -- and no hook ordering can prevent that, \
                 because nothing is allowed to blind that tap",
                clipboard.name()
            ));
        }
    }

    // The injector channel's two companion refusals (issue #138), taken at
    // PARSE time rather than at first use, which is this parser's rule
    // everywhere else too:
    //
    // - `--nested` has a real human at a real mouse, and the
    //   `service_consent` override the channel feeds lives on the headless
    //   backend only. A nested run with the flag would silently ignore it.
    // - `--consent=auto-approve` raises no prompt at all, so the channel
    //   would be inert and a gate driving it would go green for the wrong
    //   reason — the exact vacuity class this whole change exists to remove.
    #[cfg(feature = "consent-injector")]
    if consent_injector_fd.is_some() {
        if !matches!(mode, Some(Mode::Headless)) {
            return Err(
                "`--consent-injector-fd` requires `--headless`: the consent channel feeds the \
                 headless backend's consent round, and a nested session has a real human at a \
                 real pointer to answer its prompts."
                    .into(),
            );
        }
        if !matches!(consent, ConsentPolicy::Interactive) {
            return Err(
                "`--consent-injector-fd` requires `--consent=interactive`: under auto-approve \
                 no prompt is ever raised, so the channel would never have anything to answer."
                    .into(),
            );
        }
    }

    // `--agent-cursor`'s companion refusal (D-019), at parse time like every
    // other one here. Nested needs no opt-in -- it is the mode a human watches
    // -- so the flag would be a no-op there, and a flag that silently does
    // nothing is how an operator comes to believe a run is configured
    // differently than it is. `--size` sets the precedent.
    //
    // "No opt-in" is not "always drawn": `session::post_dispatch` offers a
    // position only for the realm the output is bound to (WS-E.1.3), so an
    // agent acting in a hidden realm draws no sprite in either mode. That is
    // a published limit and it is not what this flag is about.
    if agent_cursor && matches!(mode, Some(Mode::Nested)) {
        return Err(
            "`--agent-cursor` is only valid with `--headless`: nested mode composites the \
             agent cursor into the host window with no opt-in, so the flag would do nothing \
             there."
                .into(),
        );
    }

    // `--physical-input-fd` names a channel that exists on the headless
    // backend only (issue #212): a nested session has a real human at a real
    // keyboard, and the adoption, the calloop source and the routing turn all
    // live in `backend::headless`. A nested run with the flag would silently
    // ignore it, which is the "instrumented from the outside, plain in
    // behaviour" state the consent channel's own guard exists to prevent.
    #[cfg(feature = "physical-input-injector")]
    if physical_input_fd.is_some() && !matches!(mode, Some(Mode::Headless)) {
        return Err(
            "`--physical-input-fd` requires `--headless`: the channel feeds the headless \
             backend's physical intake, and a nested session already has a real human at a \
             real keyboard."
                .into(),
        );
    }

    // `--help`/`--version` already returned above, so only the run modes
    // remain to resolve; `--size` is meaningless without `--headless`.
    match (mode, size) {
        (Some(Mode::Nested), None) => Ok(Action::RunNested {
            consent,
            principals,
            recorder,
            realm,
            shim,
            capture_dump,
            dead_man,
            attention,
            clipboard,
        }),
        (Some(Mode::Nested), Some(_)) => Err("`--size` is only valid with `--headless`".into()),
        // A headless core has no display to draw the consent prompt on and no
        // physical input device to answer it, so an interactive petition could
        // only pend until it timed out — no human could ever say yes. Refuse
        // at startup rather than run a session whose every petition silently
        // fails closed. `matches!` rather than `==` so this does not lean on
        // `ConsentPolicy: PartialEq`.
        //
        // # The refusal relaxes on a CONJUNCTION, never on the build alone
        //
        // A `consent-injector` build supplies both halves the sentence above
        // says are missing — but only when `--consent-injector-fd N` is also
        // passed (issue #138). Without the flag the guard arm below is the
        // original one, byte for byte, so an instrumented build with no flag
        // behaves *exactly* like a deployment build. That is the point: a
        // running process must be identifiable as instrumented by inspecting
        // how it was **invoked** (`/proc/<pid>/cmdline`), not only by knowing
        // how it was built.
        #[cfg(not(feature = "consent-injector"))]
        (Some(Mode::Headless), _) if matches!(consent, ConsentPolicy::Interactive) => {
            Err(HEADLESS_INTERACTIVE_REFUSAL.into())
        }
        #[cfg(feature = "consent-injector")]
        (Some(Mode::Headless), _)
            if matches!(consent, ConsentPolicy::Interactive) && consent_injector_fd.is_none() =>
        {
            Err(format!(
                "{HEADLESS_INTERACTIVE_REFUSAL} This build carries the `consent-injector` test \
                 hook and can supply one: pass `--consent-injector-fd N` naming an inherited \
                 socketpair end."
            ))
        }
        (Some(Mode::Headless), size) => Ok(Action::RunHeadless {
            size: size.unwrap_or(DEFAULT_HEADLESS_SIZE),
            consent,
            principals,
            recorder,
            realm,
            shim,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            agent_cursor,
            #[cfg(feature = "consent-injector")]
            consent_injector_fd,
            #[cfg(feature = "physical-input-injector")]
            physical_input_fd,
        }),
        (None, Some(_)) => Err("`--size` requires `--headless`".into()),
        (None, None) => Err("no mode given (expected `--nested` or `--headless`)".into()),
    }
}

/// The `--headless --consent=interactive` refusal, in one place so the
/// deployment build's message and the instrumented build's extended one
/// cannot drift in their shared sentence (issue #138). A plain build emits
/// exactly this and nothing more.
const HEADLESS_INTERACTIVE_REFUSAL: &str =
    "`--headless` cannot serve `--consent=interactive`: a headless core has no display to draw \
     the consent prompt and no physical input device to answer it, so every petition would pend \
     until it timed out. Use `--consent=auto-approve` for a headless run, or `--nested` to \
     answer prompts on screen.";

/// Record `--consent-injector-fd N` (issue #138), rejecting a repeat flag, a
/// non-numeric value, and any descriptor number this process's own stdio
/// occupies.
///
/// The number is only *shape*-checked here; whether it is an open
/// `AF_UNIX`/`SOCK_STREAM` socket is checked at adoption
/// (`consent::injector::validate_injector_fd`), which is also a startup
/// error. Both halves fail closed, and neither is a warning.
///
/// `flag` is a parameter rather than a constant because a second injector
/// channel now takes the same shape (`--physical-input-fd`, issue #212) and
/// two copies of these four rules would be two things to keep right.
#[cfg(any(feature = "consent-injector", feature = "physical-input-injector"))]
fn set_injector_fd(
    slot: &mut Option<std::os::fd::RawFd>,
    flag: &str,
    value: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("`{flag}` given more than once"));
    }
    let number: std::os::fd::RawFd = value
        .parse()
        .map_err(|_| format!("`{flag}` `{value}`: not a descriptor number (e.g. `7`)"))?;
    if number < 3 {
        return Err(format!(
            "`{flag} {number}`: 0, 1 and 2 are this process's own standard \
             descriptors; the channel must be inherited on 3 or above"
        ));
    }
    *slot = Some(number);
    Ok(())
}

/// Record a path-valued flag (`--recorder`, `--realm`), rejecting a repeat
/// flag and an empty value (an empty path can never open, and failing here
/// names the flag rather than surfacing a bare ENOENT at startup).
fn set_path(slot: &mut Option<PathBuf>, flag: &str, what: &str, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("`{flag}` given more than once"));
    }
    if value.is_empty() {
        return Err(format!("`{flag}` requires a non-empty {what}"));
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

/// Record the `--dead-man-chord` key, rejecting a repeat flag and any key
/// this build's input intake could not deliver.
///
/// The error names every accepted key rather than saying "unknown": the
/// vocabulary is short, deliberately excludes modifiers, and an operator who
/// guessed `escape` or `ESC` should be told what to write instead of being
/// left to read the source.
fn set_chord(slot: &mut Option<deadman::Chord>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--dead-man-chord` given more than once".into());
    }
    let chord = deadman::Chord::parse(value).map_err(|err| {
        let accepted: Vec<&str> = deadman::Chord::vocabulary().collect();
        format!(
            "`--dead-man-chord` `{value}`: {err} (accepted: {})",
            accepted.join(", ")
        )
    })?;
    *slot = Some(chord);
    Ok(())
}

/// Record the `--clipboard-key` trigger (WS-E.2.1), rejecting a repeat flag and
/// any key this build's input intake could not deliver.
///
/// The `--dead-man-chord` precedent exactly, including naming every accepted key
/// rather than saying "unknown". The *collision* checks against the two
/// core-owned chords are not here: they need every half resolved, so they live
/// in `parse_args` beside the other cross-flag refusals — and one of them is
/// reachable from a plausible command line, unlike the pair above it.
fn set_clipboard_key(slot: &mut Option<chord::Trigger>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--clipboard-key` given more than once".into());
    }
    let trigger = chord::Trigger::parse(value).map_err(|err| {
        let accepted: Vec<&str> = chord::Trigger::vocabulary().collect();
        format!(
            "`--clipboard-key` `{value}`: {err} (accepted: {})",
            accepted.join(", ")
        )
    })?;
    *slot = Some(trigger);
    Ok(())
}

/// Record the `--attention-chord` key (WS-E.1.7), rejecting a repeat flag and
/// any key this build's input intake could not deliver.
fn set_attention_chord(
    slot: &mut Option<attention::AttentionChord>,
    value: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--attention-chord` given more than once".into());
    }
    let chord = attention::AttentionChord::parse(value).map_err(|err| {
        let accepted: Vec<&str> = attention::AttentionChord::vocabulary().collect();
        format!(
            "`--attention-chord` `{value}`: {err} (accepted: {})",
            accepted.join(", ")
        )
    })?;
    *slot = Some(chord);
    Ok(())
}

/// Record the `--dead-man-hold` duration in milliseconds, rejecting a repeat
/// flag and a value that is not a plain non-negative integer.
fn set_hold(slot: &mut Option<u64>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--dead-man-hold` given more than once".into());
    }
    let ms: u64 = value.parse().map_err(|_| {
        format!("`--dead-man-hold` `{value}` is not a whole number of milliseconds")
    })?;
    *slot = Some(ms);
    Ok(())
}

/// Parse a `--consent` mode value.
fn parse_consent(value: &str) -> Result<ConsentPolicy, String> {
    match value {
        "interactive" => Ok(ConsentPolicy::Interactive),
        "auto-approve" => Ok(ConsentPolicy::AutoApprove),
        other => Err(format!(
            "unknown `--consent` mode `{other}` (expected `interactive` or `auto-approve`)"
        )),
    }
}

/// Record the selected consent policy, rejecting a repeat flag.
fn set_consent(slot: &mut Option<ConsentPolicy>, policy: ConsentPolicy) -> Result<(), String> {
    match slot {
        None => {
            *slot = Some(policy);
            Ok(())
        }
        Some(_) => Err("`--consent` given more than once".into()),
    }
}

/// Record the selected [`Mode`], rejecting a second (or conflicting) mode flag.
fn set_mode(slot: &mut Option<Mode>, mode: Mode) -> Result<(), String> {
    match slot {
        None => {
            *slot = Some(mode);
            Ok(())
        }
        Some(_) => {
            Err("more than one mode given (expected one of `--nested`, `--headless`)".into())
        }
    }
}

/// Parse a `WxH` size such as `1280x800` into a `(width, height)` that fits
/// the `i32` physical-pixel domain the backend uses. A missing `x`, a
/// non-numeric field, a zero dimension, or a dimension past `i32::MAX` is an
/// error: a virtual output with no area — or one large enough to wrap negative
/// on the backend's `i32` cast — is never what the caller meant.
fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let (w, h) = value
        .split_once('x')
        .ok_or_else(|| format!("malformed `--size` `{value}` (expected `WxH`, e.g. `1280x800`)"))?;
    let width: u32 = w
        .parse()
        .map_err(|_| format!("malformed `--size` width in `{value}`"))?;
    let height: u32 = h
        .parse()
        .map_err(|_| format!("malformed `--size` height in `{value}`"))?;
    if width == 0 || height == 0 {
        return Err(format!(
            "`--size` `{value}` must have positive width and height"
        ));
    }
    // The backend carries the size as `i32` physical pixels; a dimension past
    // `i32::MAX` would wrap negative on that cast and silently degenerate the
    // output. Reject it with a clear message instead of accepting a size that
    // can never be honored.
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(format!(
            "`--size` `{value}` exceeds the maximum virtual-output dimension ({})",
            i32::MAX
        ));
    }
    Ok((width, height))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let action = match parse_args(args.iter().map(String::as_str)) {
        Ok(action) => action,
        Err(msg) => {
            eprintln!("vitrind: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match action {
        Action::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("vitrind {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Action::PrintIsolation => {
            // Deliberately upstream of `init_tracing`, exactly as `Help` and
            // `Version` are: this output is byte-compared against a checked-in
            // matrix, and tracing interleaved into stdout would make the
            // comparison depend on the log level.
            print!("{}", spawn::isolation::Report::probe().render());
            ExitCode::SUCCESS
        }
        Action::RunNested {
            consent,
            principals,
            recorder,
            realm,
            shim,
            capture_dump,
            dead_man,
            attention,
            clipboard,
        } => {
            init_tracing();
            run_session(
                consent,
                principals,
                recorder,
                realm,
                shim,
                capture_dump,
                // `--consent-injector-fd` is refused with `--nested` at parse
                // time (issue #138), so a nested run is never instrumented.
                false,
                move |seed| backend::winit::run(dead_man, attention, clipboard, seed),
            )
        }
        Action::RunHeadless {
            size,
            consent,
            principals,
            recorder,
            realm,
            shim,
            capture_dump,
            // Validated at parse time so both modes accept the same command
            // line. Headless has no physical input device to hold a chord
            // on (`Action::RunHeadless::dead_man`), so a plain build never
            // reads this past `backend::headless::run`'s signature; a
            // `dead-man-injector` build reads it to name the chord/hold a
            // SIGUSR1-synthesized trigger reports (issue #109).
            dead_man,
            attention,
            clipboard,
            agent_cursor,
            #[cfg(feature = "consent-injector")]
            consent_injector_fd,
            #[cfg(feature = "physical-input-injector")]
            physical_input_fd,
        } => {
            init_tracing();
            #[cfg(feature = "consent-injector")]
            let instrumented = consent_injector_fd.is_some();
            #[cfg(not(feature = "consent-injector"))]
            let instrumented = false;
            run_session(
                consent,
                principals,
                recorder,
                realm,
                shim,
                capture_dump,
                instrumented,
                move |seed| {
                    backend::headless::run(
                        size,
                        dead_man,
                        attention,
                        clipboard,
                        agent_cursor,
                        #[cfg(feature = "consent-injector")]
                        consent_injector_fd,
                        #[cfg(feature = "physical-input-injector")]
                        physical_input_fd,
                        seed,
                    )
                },
            )
        }
    }
}

/// Block, **process-wide and before any thread exists**, every signal this
/// session will read through a `signalfd`.
///
/// # Why this cannot live in the backend
///
/// A `signalfd` receives a signal only if that signal is blocked in *every*
/// thread of the process; a thread that has not blocked it can be chosen by
/// the kernel for delivery instead, and then the signal takes its default
/// disposition. `calloop::signals::Signals::new` blocks only in the thread
/// that calls it, and the backends call it from the main thread — which is
/// correct only if no other thread exists yet.
///
/// One does. [`announce_consent_policy`] spawns the R6 auto-approve banner
/// thread before any backend starts, and Smithay's winit/EGL stack spawns
/// more. The observable result, measured against the shipped binary before
/// this existed: under `--consent=auto-approve`, `SIGTERM` was delivered to
/// the banner thread, took its default action, and killed the core outright
/// — no shutdown ladder, an orphaned shim, a realm runtime tree left on
/// disk, and no `run_ended` footer. Under `--consent=interactive`, with no
/// banner thread, the identical build shut down cleanly. A bug that depends
/// on an unrelated flag is exactly the kind this ordering rule exists to
/// prevent.
///
/// `SIGCHLD` is the quieter half and the reason this is load-bearing rather
/// than tidy: its default disposition is *ignore*, so a `SIGCHLD` delivered
/// to any thread that has not blocked it is simply lost — and with it the
/// core's only notification that its realm's shim exited. Nothing fails,
/// nothing is logged, and the realm stays `Running` in the registry forever.
///
/// Blocking here rather than at each `Signals::new` also means every thread
/// spawned later inherits the mask, since a new thread starts with a copy of
/// its creator's. The backends' `Signals::new` calls then re-block what is
/// already blocked (a no-op) and create the descriptor that actually reads
/// them.
///
/// A `dead-man-injector` build additionally blocks `SIGUSR1` here, for the
/// same reason as the other three: the headless backend's injector
/// (`backend::headless`, issue #109) reads it through its own `Signals`
/// source, and that source is only reachable if no earlier thread has
/// already taken the signal's default disposition (terminate the process —
/// `signal(7)`'s table for `SIGUSR1`).
fn block_loop_signals() -> std::io::Result<()> {
    // SAFETY: `sigset_t` is a plain bitset that `sigemptyset` initializes
    // before any read; the zeroed value is never observed. This runs on the
    // main thread before the process has spawned any other, so there is no
    // concurrent signal-mask mutation to race. Every argument is a valid
    // pointer to a live local, and the null third argument means "do not
    // report the previous mask", which we do not need.
    let rc = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGCHLD);
        #[cfg(feature = "dead-man-injector")]
        libc::sigaddset(&mut set, libc::SIGUSR1);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut())
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc));
    }
    Ok(())
}

/// Load the session's realm, bind the core socket, open the run's flight
/// recorder, build the capability kernel, run the backend around it, and
/// close the log.
///
/// # Startup order, and the two steps that bite if moved
///
/// 1. **The R6 auto-approve guard**, before anything at all — including the
///    realm, the socket, and the log. A session that must not start should
///    abort at the earliest point where that is knowable, leaving nothing
///    behind and doing nothing first.
/// 2. **The realm** (P1.5.1): the only startup input whose absence means the
///    session has nothing to serve.
/// 3. **The principal registry**, loaded exactly once, here, and handed to
///    every connection's `ServerCtx` as the session's one [`Verifier`]. Under
///    auto-approve this is the *same* load the R6 guard already performed,
///    reused rather than repeated: two loads would be a TOCTOU window against
///    the very file the guard audited, which is the fault
///    [`announce_consent_policy`] exists to prevent.
/// 4. **The core socket's [`Listener`]**, before the recorder. Binding takes
///    an exclusive `flock` on `core.sock.lock` and fails `AddrInUse` if
///    another live core holds it, so binding early means a second core
///    refuses *before* it has created a stray flight-recorder log in the
///    runtime directory the first core is using.
/// 5. **The recorder**, bracketing the session: creation failure is fatal
///    before the backend starts (P1.4.5 — an operator who asked for a flight
///    recorder and cannot have one must learn it before the session, not
///    after it is unreconstructable), and the closing entry reports how many
///    entries a mid-run write failure cost, since that is the one thing a
///    truncated log cannot say about itself.
///
/// # Why the backend takes the kernel rather than the other way round
///
/// calloop fixes one state type per event loop, and every runtime source —
/// the listener, each principal connection, the realm's shim socketpair, the
/// expiry sweep — must be inserted into the same loop the backend already
/// drives. So the capability kernel has to live in the backend's state, which
/// is why the backend is no longer a `FnOnce() -> Result<()>` that owns its
/// loop and tells nobody: it is handed a [`session::RuntimeSeed`] and hands
/// the [`Recorder`] back, so the run's footer is still written here.
///
/// [`Verifier`]: identity::Verifier
/// [`Listener`]: vitrin_ipc::Listener
///
/// # Where the realm's life happens, and why not here
///
/// The spawn is deliberately **not** in this function, even though the
/// realm's `realm_spawned` entry wants to be early in the log. A shim
/// blocks on `configure` and then on every reply, with no timeout anywhere
/// on its side, so it must not exist until something is servicing its
/// socketpair — and that something is the backend's event loop, which does
/// not exist until `backend()` below has built it. So `session::start_realm`
/// runs inside each backend's `run_inner`, immediately after
/// `session::install` and immediately before `event_loop.run`, which is the
/// earliest point at which the spawn is safe rather than a permanent wedge.
///
/// The shutdown ladder is in the backend for the mirror-image reason: it
/// blocks by design, so it must run after the loop has stopped, and it must
/// run before the recorder is handed back here so that the realm's
/// `realm_died` / `realm_exited` entries land in this run rather than after
/// its footer.
///
/// # Interactive consent is wired (issue #90)
///
/// `ConsentGrab::raise` now has a production caller:
/// `session::service_consent_round`, driven once per dispatch round by the
/// nested backend's `session::RuntimeHost::service_consent`. Under
/// `--consent=interactive` the front pending petition's prompt is raised, a
/// physical decision resolves it, and the armed sweep's `timed_out` is only
/// the fallback for a prompt a human never answers. `--headless` has no display
/// or input device for a prompt and is refused with `--consent=interactive` at
/// startup (above), so no petition pends unanswerably.
///
/// # The instrumented-session marker (issue #138)
///
/// `consent_injector` says the invocation carried `--consent-injector-fd`, so
/// this session's consent prompts can be answered over a socket. It is always
/// `false` in a build without the `consent-injector` feature, because the
/// flag has no parse arm there. Two things depend on it, and both are honesty
/// artifacts rather than behaviour:
///
/// - the run's `RunStarted.consent_policy` reads `interactive+consent-injector`
///   instead of `interactive`. This is load-bearing: an injected decision
///   *correctly* journals `Issuer::HumanConsent` (it really did traverse
///   `PetitionRegistry::resolve_human`), so without this marker an
///   instrumented run's journal would be indistinguishable from a
///   human-answered one.
/// - a standing, repeating warning ([`InjectorBanner`]), because a one-line
///   startup notice scrolls off and a session whose "interactive" consent can
///   be answered over a socket is not what the word normally promises.
// Eight parameters, one over clippy's default: the run's five configured
// paths, the consent policy, the instrumented-session marker (issue #138) and
// the backend closure. Bundling them into a struct would hide the one thing
// this signature is good for -- every startup input the session has is named
// here, in the order `run_session`'s doc comment explains them.
#[allow(clippy::too_many_arguments)]
fn run_session<R>(
    consent: ConsentPolicy,
    principals_path: Option<PathBuf>,
    recorder_path: Option<PathBuf>,
    realm_path: Option<PathBuf>,
    shim_path: Option<PathBuf>,
    capture_dump: Option<PathBuf>,
    consent_injector: bool,
    backend: R,
) -> ExitCode
where
    R: FnOnce(session::RuntimeSeed) -> (Recorder, Result<(), Box<dyn std::error::Error>>),
{
    // The R6 guard runs before anything at all, including the realm: a
    // session that must not start should abort at the earliest point where
    // that is knowable, leaving nothing behind and doing nothing first.
    //
    // `banner` is held across the entire session and retired by the
    // explicit `banner.stop()` after the backend returns. Both halves are
    // load-bearing and neither is a style preference: dropping it early
    // would stop R6's repeating auto-approve warning while auto-approve was
    // still granting petitions, and the `stop()` downstream is what makes
    // "tidy the unused binding to `_`" a compile error rather than a silent
    // downgrade of a security warning. The early-return paths below drop it
    // instead, which is correct: no session runs on those.
    //
    // It is deliberately **not** moved into the session state struct the
    // runtime wiring introduced, tempting though that is: a struct field
    // always reads as "used", so the compile-time tripwire above would
    // evaporate, and the state is dropped when the loop ends — possibly
    // before `recorder.finish()` — which would silently shorten the warning's
    // lifetime to less than the policy's.
    // Before the R6 guard, because the guard spawns the banner thread and
    // this must run while the process is still single-threaded. See
    // `block_loop_signals` for what silently breaks otherwise.
    if let Err(err) = block_loop_signals() {
        tracing::error!(
            "fatal: cannot block the session's signals: {err}; without this the shutdown \
             ladder and realm-exit detection are both unreliable"
        );
        return ExitCode::FAILURE;
    }

    let Ok((banner, guard_verifier)) = announce_consent_policy(consent, principals_path.as_deref())
    else {
        return ExitCode::FAILURE;
    };

    // The realm is resolved before anything else this run creates: a
    // session whose realm configuration is missing or malformed has
    // nothing to serve, and aborting here means no log file, no socket,
    // and no window are left behind by a run that could never work.
    let realms = match load_realms(realm_path.as_deref()) {
        Ok(realms) => realms,
        Err(()) => return ExitCode::FAILURE,
    };
    announce_realms(&realms);

    // One verifier for the whole session. Under auto-approve this is the
    // registry the R6 guard already loaded and audited, moved here rather
    // than re-read: see this function's startup-order note.
    let verifier = match guard_verifier {
        Some(verifier) => verifier,
        None => match load_verifier(principals_path.as_deref()) {
            Ok(verifier) => verifier,
            Err(()) => return ExitCode::FAILURE,
        },
    };

    // The single-core gate. `Listener::bind` takes a non-blocking exclusive
    // `flock` on `<socket>.lock` and only then removes whatever is at the
    // socket path, so a second core against the same `$XDG_RUNTIME_DIR`
    // refuses here instead of unlinking a live core's socket out from under
    // it. Before the recorder, so that refusal leaves no stray log behind.
    let listener = match bind_core_socket() {
        Ok(listener) => listener,
        Err(()) => return ExitCode::FAILURE,
    };

    let path = match recorder_path {
        Some(path) => path,
        None => match default_recorder_path() {
            Ok(path) => path,
            Err(err) => {
                tracing::error!(
                    "fatal: cannot place the flight-recorder log: {err}; \
                     pass an explicit `--recorder PATH`"
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let mut recorder = match Recorder::create(&path) {
        Ok(recorder) => recorder,
        Err(err) => {
            tracing::error!("fatal: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(path = %path.display(), run_id = recorder.run_id(), "flight recorder open");
    // A build that can answer its own prompts must say so, standing, for as
    // long as it can — R6's auto-approve lesson applied to the other
    // direction. Started here rather than earlier so it is beside the policy
    // it qualifies in the log, and after `block_loop_signals` (which must run
    // while the process is still single-threaded).
    #[cfg(feature = "consent-injector")]
    let _injector_banner =
        consent_injector.then(|| InjectorBanner::start(INJECTOR_BANNER_INTERVAL));
    recorder.record(Event::RunStarted {
        pid: std::process::id(),
        core_version: env!("CARGO_PKG_VERSION"),
        // The instrumented-run marker (issue #138, this function's docs).
        // Not a new recorder variant: a cfg-gated enum variant would make the
        // journal's schema build-configuration-dependent, which is a worse
        // audit surface for a TCB than a distinct string in a field that
        // already exists.
        consent_policy: match (consent, consent_injector) {
            (ConsentPolicy::Interactive, false) => "interactive",
            (ConsentPolicy::Interactive, true) => "interactive+consent-injector",
            (ConsentPolicy::AutoApprove, _) => "auto-approve",
        },
    });

    // Mint this session's trusted consent indicator (issue #85) before the
    // backend below begins accepting connections: no client is running when
    // the secret is chosen, so none can observe or derive it. Fail closed — a
    // guessable trust colour would train a human to trust a forgeable frame,
    // which is worse than no frame at all.
    let indicator = match crate::consent::TrustedIndicator::generate() {
        Ok(indicator) => indicator,
        Err(err) => {
            tracing::error!("fatal: cannot establish the consent trust indicator: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Deliberately logged WITHOUT its value. The confined realm runs as this
    // core's uid (the SO_PEERCRED same-user policy), so anything written to
    // stderr — or any file — is reachable by the app, directly or via
    // `/proc/<pid>/fd`, and a trust colour the forger can read is no trust
    // colour at all. The human learns it off the display vitrind owns: the
    // reserved band and every genuine prompt's frame
    // (`consent::ConsentSurface`), the one channel the app cannot follow.
    tracing::info!(
        "consent trust indicator established for this session; it is shown to \
         the human only on vitrind's own display, never written to a log"
    );

    // `PetitionConfig::default()` rather than new flags: issue #77 asks for
    // the registry to be *constructed from the parsed consent policy*, and
    // the defaults are the settled values in `petitions`' module docs. A
    // deployment-tunable surface for them (notably `consent_timeout`) is a
    // separate, deliberate CLI change.
    // The shim binary the spawn manager execs to hold the realm's fd-3 core
    // connection (issue #103). Resolved here, before the backend starts, so a
    // core with no usable shim learns it up front rather than at the fork; the
    // binary itself is audited transitively at spawn time, exactly like the
    // realm's `command`. The default is a sibling `vitrin-shim` beside this
    // `vitrind` -- the realm never names the shim, that being the core's job.
    let shim = match shim_path {
        Some(path) => path,
        None => match default_shim_path() {
            Ok(path) => path,
            Err(err) => {
                tracing::error!(
                    "fatal: cannot locate the default shim binary: {err}; \
                     pass an explicit `--shim PATH`"
                );
                return ExitCode::FAILURE;
            }
        },
    };
    tracing::info!(shim = %shim.display(), "realm shim binary (execs the realm's app; audited at spawn)");

    let seed = session::RuntimeSeed {
        listener,
        verifier,
        petitions: petitions::PetitionRegistry::new(consent, petitions::PetitionConfig::default()),
        grants: grants::GrantTable::new(),
        realms,
        recorder,
        shim,
        indicator,
        capture_dump,
    };

    let (mut recorder, result) = backend(seed);

    // The session is over: the consent policy is no longer in force, so its
    // standing warning stops here rather than at an arbitrary scope end.
    banner.stop();

    // `finish` forces one last recovery attempt before writing the footer,
    // so a run that degraded transiently still closes with a `run_ended`
    // naming the loss (crates/vitrin-core/src/recorder.rs, degradation
    // policy).
    recorder.finish();
    let dropped = recorder.dropped_entries();
    if dropped > 0 {
        if recorder.is_degraded() {
            tracing::error!(
                path = %path.display(),
                dropped_entries = dropped,
                "flight recorder was DEGRADED during this run and never recovered; the log \
                 ends mid-run with no footer -- from the file alone that is indistinguishable \
                 from a SIGKILL, so THIS message is the only record that entries were lost"
            );
        } else {
            tracing::error!(
                path = %path.display(),
                dropped_entries = dropped,
                "flight recorder was DEGRADED during this run; the log is incomplete, and the \
                 gap is marked in the file by skipped `seq` values plus a `recording_resumed` \
                 entry naming the loss"
            );
        }
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("fatal: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Bind the core socket, refusing to start if another core already holds the
/// runtime tree.
///
/// This is the whole of the single-core invariant, and the mechanism is
/// `Listener::bind`'s, not a second one invented here: it takes a
/// non-blocking exclusive `flock` on `core.sock.lock`, re-verifies the locked
/// inode against the path to close the unlink race, and only then unlinks a
/// stale socket. The kernel releases that lock on process death including
/// `SIGKILL`, so there is no stale state to heuristically clean up — which is
/// exactly why it is an `flock` and not an `O_EXCL` pidfile.
///
/// What it prevents is worth naming, because "two cores is unsupported" reads
/// as a nicety until you follow it: a second core against the same
/// `$XDG_RUNTIME_DIR` would purge realm runtime directories the first core's
/// live shim is bound to, then bind `core.sock` itself so new agents
/// transparently reach *it* while the first core still holds the grant table
/// those agents' authority lives in — and on its own exit unlink `core.sock`,
/// leaving the first core running, healthy-looking, and unreachable.
fn bind_core_socket() -> Result<vitrin_ipc::Listener, ()> {
    let path = match vitrin_ipc::paths::core_socket_path() {
        Ok(path) => path,
        Err(err) => {
            tracing::error!("fatal: cannot locate the core socket: {err}");
            return Err(());
        }
    };
    vitrin_ipc::Listener::bind(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AddrInUse {
            tracing::error!(
                socket = %path.display(),
                "fatal: another vitrind already holds this runtime tree (its lock on \
                 `{}.lock` is live). Refusing to start rather than unlinking a running \
                 core's socket and purging realm directories it is still serving; point \
                 `XDG_RUNTIME_DIR` somewhere else to run a second core.",
                path.display()
            );
        } else {
            tracing::error!(socket = %path.display(), "fatal: cannot bind the core socket: {err}");
        }
    })
}

/// Load the session's principal registry (P1.4.1) for the interactive path,
/// failing fatally if it cannot be read.
///
/// Fail-fast rather than fail-at-first-connect, matching the realm and
/// recorder posture: a core running with an unreadable registry can verify
/// nobody, so every agent that connects would be refused with an error naming
/// a file the operator never learned was broken. Note that this *changes*
/// interactive's old behaviour, where the registry was never read at startup
/// because nothing at runtime verified against it.
fn load_verifier(principals_path: Option<&Path>) -> Result<StaticVerifier, ()> {
    let path = match principals_path {
        Some(path) => path.to_path_buf(),
        None => match realm::default_principals_path() {
            Ok(path) => path,
            Err(err) => {
                tracing::error!(
                    "fatal: cannot locate the principal registry: {err}; \
                     pass an explicit `--principals PATH`"
                );
                return Err(());
            }
        },
    };
    StaticVerifier::load(&path).map_err(|err| {
        tracing::error!(
            path = %path.display(),
            "fatal: the principal registry could not be read, so this core could verify \
             nobody: {err}"
        );
    })
}

/// `$XDG_RUNTIME_DIR/vitrin-0/flight-recorder-<pid>.jsonl` — the run's log
/// beside the core's socket, in the directory that already holds this
/// session's artifacts. A core that cannot name its runtime directory
/// cannot serve a session either, so failing here is honest rather than
/// silently logging somewhere else.
fn default_recorder_path() -> Result<PathBuf, vitrin_ipc::PathError> {
    Ok(vitrin_ipc::paths::runtime_dir()?.join(format!(
        "{RECORDER_FILE_PREFIX}-{}.jsonl",
        std::process::id()
    )))
}

/// A `vitrin-shim` beside this running `vitrind` — the default shim binary
/// when `--shim` is omitted, resolved through `current_exe` so it honors
/// whatever directory the core was installed or built into rather than
/// guessing one. Only its *location* is decided here; whether it exists and
/// is safe to exec is the spawn-time audit's question (`crate::spawn`), which
/// refuses a missing or untrusted-writable shim exactly as it does an app.
fn default_shim_path() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate the running vitrind: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("vitrind {} has no parent directory", exe.display()))?;
    Ok(dir.join("vitrin-shim"))
}

/// Resolve and load the session's realm configuration (P1.5.1), failing
/// **loudly and fatally** on any problem: the message names the file and
/// the specific fault, because a core that silently defaulted to some
/// realm the operator did not describe would be spawning an app nobody
/// asked for. `Err(())` means the caller returns a failure exit code --
/// the diagnostics have already been emitted.
fn load_realms(realm_path: Option<&Path>) -> Result<RealmRegistry, ()> {
    let path = match realm_path {
        Some(path) => path.to_path_buf(),
        None => match realm::default_config_path() {
            Ok(path) => path,
            Err(err) => {
                tracing::error!(
                    "fatal: cannot locate the realm configuration: {err}; \
                     pass an explicit `--realm PATH`"
                );
                return Err(());
            }
        },
    };
    RealmRegistry::load(&path).map_err(|err| {
        tracing::error!("fatal: {err}");
    })
}

/// Announce what this session will run. The command a trusted core is
/// configured to execute is a security-relevant fact an operator should
/// see in the log, not discover from `ps`; the environment allowlist is
/// named for the same reason (its default is empty -- the app inherits
/// nothing the file does not name).
fn announce_realms(realms: &RealmRegistry) {
    for realm in realms.iter() {
        tracing::info!(
            realm = %realm.id(),
            command = %realm.spawn().command().display(),
            args = realm.spawn().args().len(),
            env_allow = ?realm.spawn().env_allow(),
            "realm configured (spawned once the event loop can service its shim; see \
             session::start_realm)"
        );
    }
}

/// Announce the consent policy and, under auto-approve, **refuse to start**
/// unless the principal registry is safe to grant blindly (plan risk R6).
///
/// Returns the live banner guard: dropping it stops the repeating warning,
/// so the warning's lifetime is the session's lifetime by construction. An
/// `Err` means the session must not start; the diagnostics have already
/// been emitted.
///
/// It also returns the [`StaticVerifier`] it loaded, when it loaded one, so
/// [`run_session`] can use *that* registry rather than reading the file a
/// second time. This is not an optimization. Two reads are two documents:
/// the file can change between them, and a guard that audited the first
/// while the runtime verified against the second would be precisely the
/// "guard auditing a document nobody reads" fault the third bullet below
/// exists to close. Under interactive the guard loads nothing and this is
/// `None`; `run_session` loads the registry itself, once.
///
/// The guard is a plain struct rather than an `Option`, and it is retired by
/// an explicit [`PolicyBanner::stop`] at the end of [`run_session`] rather
/// than by falling out of scope. That is deliberate and is the only defence
/// that survives a refactor: with a `let Ok(_banner) = ...` binding, an
/// editor tidying an "unused" variable to `_` drops the guard immediately
/// and silently converts R6's persistent warning back into the single
/// startup line R6 says is not enough — no test can see the difference
/// without waiting out a banner interval. With a `stop()` call downstream,
/// the same edit does not compile.
///
/// # Why auto-approve needs a startup guard at all
///
/// `--consent=auto-approve` grants every petition as requested with no
/// human in the loop. That is defensible for the walking skeleton and
/// headless CI, where the only principal is a demo agent whose token is in
/// the same repository. It is indefensible for any registry an operator
/// actually deployed: the flag would silently convert every principal in
/// that file into an unattended holder of observe-and-actuate authority
/// over the realm. The flag is a CI convenience that composes into a
/// backdoor, which is exactly the shape plan risk R6 names.
///
/// # What "more than the demo principal" means, precisely
///
/// The registry must contain **exactly one row, and its canonical identity
/// must be exactly [`DEMO_PRINCIPAL`]**. Both halves are load-bearing, and
/// the reason the obvious cheaper check is wrong is worth stating:
///
/// - A pure count (`rows > 1`) is satisfied by a registry holding one
///   principal named `vitrin://prod/agent/fleet-controller` — a real
///   deployed identity with a real token, now auto-granted whatever it
///   asks for, on a configuration the guard called safe. The dangerous
///   registry is not "demo plus others"; it is "any row that is not the
///   demo". A single production principal is exactly as dangerous as ten.
/// - Requiring the exact demo identity makes the guard a whitelist of one
///   known-throwaway name rather than a headcount. An operator who renamed
///   that row has, definitionally, stopped running the demo.
///
/// Three checks it deliberately does **not** make, each of which looks
/// stricter and is worse:
///
/// - **The token is not inspected.** `examples/principals.toml` ships a
///   placeholder and tells operators to replace it; demanding the
///   placeholder would make this guard require a weaker configuration to
///   pass. A demo with a real random token is still a demo.
/// - **A missing registry is refused, not waved through.** "No file, so no
///   principals, so nothing to grant" is true today and is a trapdoor
///   tomorrow: it would let any operator satisfy the guard by pointing
///   `--principals` at a path that does not exist, and then have the M1.1
///   listener load a registry this guard never saw. The guard's job is to
///   *prove* the registry is the demo registry; absence proves nothing.
/// - **The registry is loaded through [`StaticVerifier::load`]**, not a
///   bespoke parse — so the permission checks, the token-length minimum,
///   the duplicate-identity rule, and the uid-pin rule all apply exactly as
///   they will at verification time. A file this guard accepts and the
///   runtime rejects (or vice versa) would be a guard auditing a document
///   nobody reads.
///
/// Under `interactive` the registry is **not** read *here*: interactive is
/// the fail-closed default and needs no permission to run, so this guard has
/// nothing to decide. It is still read at startup — [`run_session`] loads it
/// through [`load_verifier`] on that path, because the listener now really
/// does verify against it and a core that cannot read its registry can
/// verify nobody. The division is deliberate: this function is a *policy
/// guard*, and making it also the interactive path's loader would put a
/// refusal to start into a function whose refusals are all about
/// auto-approve.
fn announce_consent_policy(
    policy: ConsentPolicy,
    principals_path: Option<&Path>,
) -> Result<(PolicyBanner, Option<StaticVerifier>), ()> {
    match policy {
        ConsentPolicy::Interactive => {
            tracing::info!(
                "consent policy: interactive (petitions pend for a human decision on the \
                 core-rendered consent prompt; unanswered petitions resolve timed_out)"
            );
            Ok((PolicyBanner(None), None))
        }
        ConsentPolicy::AutoApprove => {
            let path = match principals_path {
                Some(path) => path.to_path_buf(),
                None => match realm::default_principals_path() {
                    Ok(path) => path,
                    Err(err) => {
                        tracing::error!(
                            "fatal: `--consent=auto-approve` must audit the principal registry, \
                             and this core cannot locate it: {err}; pass an explicit \
                             `--principals PATH`"
                        );
                        return Err(());
                    }
                },
            };
            let verifier = match StaticVerifier::load(&path) {
                Ok(verifier) => verifier,
                Err(err) => {
                    tracing::error!(
                        path = %path.display(),
                        "fatal: `--consent=auto-approve` REFUSED: the principal registry could \
                         not be read, so it cannot be audited: {err}. Auto-approve grants every \
                         petition with no human decision and is only permitted when the registry \
                         holds nothing but the demo principal `{DEMO_PRINCIPAL}`; \
                         run without `--consent=auto-approve` to start."
                    );
                    return Err(());
                }
            };
            let identities: Vec<&str> = verifier.identities().map(|id| id.as_str()).collect();
            if identities != [DEMO_PRINCIPAL] {
                tracing::error!(
                    path = %path.display(),
                    principals = ?identities,
                    "fatal: `--consent=auto-approve` REFUSED: this registry holds more than the \
                     demo principal. Auto-approve would grant EVERY petition from EVERY listed \
                     principal, as requested, with no human decision -- so it is permitted only \
                     for a registry of exactly one row whose identity is `{DEMO_PRINCIPAL}`. \
                     Remove `--consent=auto-approve` (interactive consent is the default), or \
                     point `--principals` at a demo-only registry."
                );
                return Err(());
            }
            Ok((
                PolicyBanner(Some(AutoApproveBanner::start(
                    &path,
                    AUTO_APPROVE_BANNER_INTERVAL,
                ))),
                Some(verifier),
            ))
        }
    }
}

/// The consent policy's live warning state for one session: an
/// [`AutoApproveBanner`] under auto-approve, nothing under interactive
/// (which needs no standing warning — it is the fail-closed default).
///
/// A struct rather than a bare `Option` so [`run_session`] has something it
/// must consume: see [`announce_consent_policy`]'s doc for why the guard's
/// lifetime is pinned by a call rather than by a binding.
struct PolicyBanner(Option<AutoApproveBanner>);

impl PolicyBanner {
    /// End the session's warning. Consuming `self`, so the compiler — not a
    /// reviewer — is what notices if the guard stops being held for the
    /// session's whole duration.
    fn stop(self) {
        drop(self.0);
    }
}

/// The repeating auto-approve warning, alive for exactly as long as the
/// session is (plan risk R6: the warning must be persistent, not a startup
/// line that scrolls away).
///
/// # Why this is a thread in an otherwise single-threaded core
///
/// The core's decision paths are single-threaded on purpose, and this does
/// not change that: the thread **shares no core state** — it holds one
/// stop-flag and calls `tracing`, which is `Sync` — and it takes no part in
/// any authority decision, composition, or protocol step. The alternative
/// would be a timer in each backend's event loop, which means the warning's
/// correctness depends on two loops (and every future backend) remembering
/// to arm it; a policy this loud should not be a thing three call sites can
/// forget.
///
/// Shutdown is prompt rather than "within one interval": [`Drop`] sets the
/// flag and notifies the condvar, so the sleeping thread wakes immediately
/// and joins. A one-minute hang on exit would be its own bug report.
struct AutoApproveBanner {
    /// `(stop requested, waker)`.
    stop: Arc<(Mutex<bool>, Condvar)>,
    /// `None` only after [`Drop`] has taken it, or if the thread could not
    /// be spawned (in which case the startup warning was still emitted --
    /// a missing repeat must never be a reason not to start).
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AutoApproveBanner {
    /// Emit the warning now and every `interval` until dropped.
    ///
    /// The interval is a parameter, not the constant read directly, purely
    /// so tests can observe the *repeat* — the property R6 actually asks
    /// for — without a 60-second sleep in CI. The one production caller
    /// passes [`AUTO_APPROVE_BANNER_INTERVAL`].
    fn start(registry: &Path, interval: Duration) -> Self {
        warn_auto_approve(registry);
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = Arc::clone(&stop);
        let registry = registry.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("auto-approve-banner".into())
            .spawn(move || {
                let (lock, cvar) = &*worker;
                let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*stopped {
                    let (guard, _timeout) = cvar
                        .wait_timeout(stopped, interval)
                        .unwrap_or_else(|e| e.into_inner());
                    stopped = guard;
                    if !*stopped {
                        warn_auto_approve(&registry);
                    }
                }
            })
            .inspect_err(|err| {
                tracing::error!(
                    "auto-approve is ACTIVE but its repeating warning could not be started \
                     ({err}); this line and the startup banner are the only warnings this run \
                     will emit outside the per-approval log"
                );
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for AutoApproveBanner {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.stop;
        {
            let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
            *stopped = true;
        }
        cvar.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How often the `consent-injector` standing warning repeats (issue #138).
///
/// The same interval and the same reasoning as
/// [`AUTO_APPROVE_BANNER_INTERVAL`]: an operator glancing at the log of a
/// session whose "interactive" consent can be answered over a socket must
/// meet that fact on any screenful, not only in the first.
#[cfg(feature = "consent-injector")]
const INJECTOR_BANNER_INTERVAL: Duration = AUTO_APPROVE_BANNER_INTERVAL;

/// The `consent-injector` session's standing warning (issue #138).
///
/// A **second instance** of the repeating-warning shape
/// [`AutoApproveBanner`] already established, deliberately not a
/// generalisation of that type: R6's consuming-`self` `stop()` tripwire and
/// the auto-approve message have to survive byte for byte, and rewriting the
/// one mechanism in the core that prints a standing security warning is not
/// something to buy inside a test-hook change. The duplication is ~30 lines
/// of thread plumbing that never ships in a deployment build at all.
///
/// Retired by `Drop` at the end of [`run_session`], which is the whole
/// session — there is no earlier scope it could be dropped in.
#[cfg(feature = "consent-injector")]
struct InjectorBanner {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "consent-injector")]
impl InjectorBanner {
    fn start(interval: Duration) -> Self {
        warn_consent_injector();
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("consent-injector-banner".into())
            .spawn(move || {
                let (lock, cvar) = &*worker;
                let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*stopped {
                    let (guard, _timeout) = cvar
                        .wait_timeout(stopped, interval)
                        .unwrap_or_else(|e| e.into_inner());
                    stopped = guard;
                    if !*stopped {
                        warn_consent_injector();
                    }
                }
            })
            .inspect_err(|err| {
                tracing::error!(
                    "the consent injector is WIRED but its repeating warning could not be \
                     started ({err}); the startup banner is the only warning this run will emit"
                );
            })
            .ok();
        Self { stop, thread }
    }
}

#[cfg(feature = "consent-injector")]
impl Drop for InjectorBanner {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.stop;
        {
            let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
            *stopped = true;
        }
        cvar.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The `consent-injector` warning text, in one place so the startup emission
/// and every repeat are literally the same message.
#[cfg(feature = "consent-injector")]
fn warn_consent_injector() {
    tracing::warn!(
        "CONSENT INJECTOR IS WIRED (--consent-injector-fd, issue #138): the consent prompts \
         this session raises can be answered by whoever holds the inherited socketpair end, \
         and every decision so taken is journalled as `human_consent` because it really does \
         traverse the human decision path. The run's `run_started.consent_policy` reads \
         `interactive+consent-injector` so this session's journal cannot be read as a \
         human-answered one. Integration tests ONLY; never a deployed session."
    );
}

/// The auto-approve warning text, in one place so the startup emission and
/// every repeat are literally the same message.
fn warn_auto_approve(registry: &Path) {
    tracing::warn!(
        principals = %registry.display(),
        "CONSENT POLICY: AUTO-APPROVE IS ACTIVE -- every grant petition is granted as \
         requested, with NO human decision and no consent prompt. Permitted only because \
         the principal registry holds nothing but the demo principal. Headless CI and \
         demos ONLY; never a deployed session."
    );
}

/// Route Smithay's (and our own) `tracing` diagnostics to stderr.
/// `RUST_LOG` selects the filter; the default is `info`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn nested_flag_selects_nested_mode() {
        assert_eq!(
            parse_args(["--nested"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn headless_flag_defaults_to_1280x800() {
        // Paired with `--consent=auto-approve`: bare `--headless` defaults to
        // interactive consent, which headless cannot serve (issue #90), so it
        // is refused at startup. Auto-approve is the headless policy.
        assert_eq!(
            parse_args(["--headless", "--consent=auto-approve"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                agent_cursor: false,
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                #[cfg(feature = "consent-injector")]
                consent_injector_fd: None,
                #[cfg(feature = "physical-input-injector")]
                physical_input_fd: None,
            })
        );
    }

    #[test]
    fn headless_size_is_parsed() {
        // `--consent=auto-approve` throughout: headless refuses interactive
        // consent (issue #90), so these size cases pair it with the policy a
        // headless run can actually serve.
        assert_eq!(
            parse_args(["--headless", "--consent=auto-approve", "--size", "1280x800"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                agent_cursor: false,
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                #[cfg(feature = "consent-injector")]
                consent_injector_fd: None,
                #[cfg(feature = "physical-input-injector")]
                physical_input_fd: None,
            })
        );
        assert_eq!(
            parse_args(["--headless", "--consent=auto-approve", "--size", "640x480"]),
            Ok(Action::RunHeadless {
                size: (640, 480),
                agent_cursor: false,
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                #[cfg(feature = "consent-injector")]
                consent_injector_fd: None,
                #[cfg(feature = "physical-input-injector")]
                physical_input_fd: None,
            })
        );
    }

    #[test]
    fn consent_defaults_to_interactive_and_parses_both_spellings() {
        // Fail-closed default: no flag means interactive (nothing granted
        // without a consent surface).
        for args in [
            vec!["--headless", "--consent", "auto-approve"],
            vec!["--headless", "--consent=auto-approve"],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: None,
                    recorder: None,
                    realm: None,
                    shim: None,
                    capture_dump: None,
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        assert_eq!(
            parse_args(["--nested", "--consent=interactive"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn headless_refuses_interactive_consent() {
        // Issue #90 scope 4: a headless core has no display to draw the
        // consent prompt on and no physical input device to answer it, so an
        // interactive petition could only pend until it timed out. Both the
        // explicit flag and the interactive *default* (bare `--headless`) must
        // be refused at startup rather than run a session whose every petition
        // silently fails closed.
        for args in [
            vec!["--headless"],
            vec!["--headless", "--consent", "interactive"],
            vec!["--headless", "--consent=interactive"],
            vec!["--headless", "--size", "640x480", "--consent=interactive"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must refuse interactive under headless"));
            assert!(
                err.contains("--headless") && err.contains("--consent=auto-approve"),
                "the refusal must name the conflict and the way out: {err}"
            );
        }
        // The two ways to make a headless run legal, and a bare nested run,
        // still parse.
        assert!(parse_args(["--headless", "--consent=auto-approve"]).is_ok());
        assert!(parse_args(["--headless", "--consent", "auto-approve"]).is_ok());
        assert!(parse_args(["--nested"]).is_ok());
    }

    /// **A deployment build cannot even name the hook** (issue #138, the
    /// compile-time half of C2).
    ///
    /// `--consent-injector-fd` has no parse arm without the feature, so it
    /// falls through to the unknown-argument error like any typo. A build
    /// that cannot name a test hook cannot be tricked into responding to one,
    /// and this is the assertion that keeps that true.
    #[cfg(not(feature = "consent-injector"))]
    #[test]
    fn a_plain_build_cannot_name_the_consent_injector_flag() {
        for args in [
            vec!["--headless", "--consent-injector-fd", "7"],
            vec!["--headless", "--consent-injector-fd=7"],
            vec!["--nested", "--consent-injector-fd=7"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must not parse in a deployment build"));
            assert!(
                err.contains("unknown argument") && err.contains("--consent-injector-fd"),
                "the flag must be an unknown argument, not a recognised-and-ignored one: {err}"
            );
        }
    }

    /// **A deployment build cannot even name the physical-input hook** (issue
    /// #212, the same compile-time half as the consent channel's).
    ///
    /// `--physical-input-fd` has no parse arm without the feature, so it falls
    /// through to the unknown-argument error like any typo. It matters more
    /// here than for either existing injector, because this is the one hook
    /// that can mint **physical-origin** input: a build that cannot name it
    /// cannot be talked into producing one.
    #[cfg(not(feature = "physical-input-injector"))]
    #[test]
    fn a_plain_build_cannot_name_the_physical_input_flag() {
        for args in [
            vec!["--headless", "--physical-input-fd", "7"],
            vec!["--headless", "--physical-input-fd=7"],
            vec!["--nested", "--physical-input-fd=7"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must not parse in a deployment build"));
            assert!(
                err.contains("unknown argument") && err.contains("--physical-input-fd"),
                "the flag must be an unknown argument, not a recognised-and-ignored one: {err}"
            );
        }
    }

    /// **An instrumented build parses the flag in both spellings, once**
    /// (issue #212).
    ///
    /// The shape checks are the consent channel's, shared with it since both
    /// go through `set_injector_fd`: a repeat is refused, a non-number is
    /// refused, and a descriptor this process's own stdio occupies is refused
    /// — the last because adopting 0/1/2 would take the core's log away from
    /// it and turn every `tracing::warn!` into channel garbage.
    #[cfg(feature = "physical-input-injector")]
    #[test]
    fn the_physical_input_flag_parses_once_and_fails_closed() {
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--physical-input-fd",
                "7",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--physical-input-fd=7",
            ],
        ] {
            match parse_args(args.clone()) {
                Ok(Action::RunHeadless {
                    physical_input_fd, ..
                }) => assert_eq!(physical_input_fd, Some(7)),
                other => panic!("{args:?} must parse as an instrumented headless run: {other:?}"),
            }
        }
        for (args, needle) in [
            (
                vec![
                    "--headless",
                    "--consent=auto-approve",
                    "--physical-input-fd=7",
                    "--physical-input-fd=8",
                ],
                "given more than once",
            ),
            (
                vec![
                    "--headless",
                    "--consent=auto-approve",
                    "--physical-input-fd=not-a-number",
                ],
                "not a descriptor number",
            ),
            (
                vec![
                    "--headless",
                    "--consent=auto-approve",
                    "--physical-input-fd=1",
                ],
                "standard descriptors",
            ),
            // Headless only: a nested run has a real human, and the channel's
            // whole machinery lives on the other backend.
            (
                vec!["--nested", "--physical-input-fd=7"],
                "requires `--headless`",
            ),
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must be refused at parse time"));
            assert!(
                err.contains(needle) && err.contains("--physical-input-fd"),
                "the refusal must name the flag and the reason: {err}"
            );
        }
    }

    /// **The startup refusal relaxes on the CONJUNCTION of the feature and
    /// the flag, never on the build alone** (issue #138, the runtime half of
    /// C2).
    ///
    /// The table below is the whole contract, and both directions matter. Row
    /// 1 is what the first attempt at this change got wrong: it relaxed on
    /// the cargo feature alone, so a *running* instrumented core was
    /// indistinguishable from a deployment one by anything short of knowing
    /// how it had been built.
    ///
    /// Row 2 additionally asserts the surviving policy is `Interactive`, so a
    /// future "make headless quietly auto-approve instead" fails here rather
    /// than passing as an equivalent relaxation.
    #[cfg(feature = "consent-injector")]
    #[test]
    fn the_injector_flag_and_the_feature_are_both_required() {
        // 1. Feature build, NO flag -> still refused, and the message still
        //    carries the deployment build's sentence verbatim.
        for args in [
            vec!["--headless"],
            vec!["--headless", "--consent", "interactive"],
            vec!["--headless", "--consent=interactive"],
            vec!["--headless", "--size", "640x480", "--consent=interactive"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must still refuse without the flag"));
            assert!(
                err.starts_with(HEADLESS_INTERACTIVE_REFUSAL),
                "the deployment refusal must survive byte-for-byte as this message's prefix: \
                 {err}"
            );
            assert!(
                err.contains("--consent-injector-fd"),
                "an instrumented build must say how to supply what it is refusing for: {err}"
            );
        }

        // 2. Feature build WITH the flag -> accepted, and the fail-closed
        //    interactive policy survives into the action.
        for args in [
            vec![
                "--headless",
                "--consent=interactive",
                "--consent-injector-fd",
                "7",
            ],
            vec![
                "--headless",
                "--consent=interactive",
                "--consent-injector-fd=7",
            ],
            vec!["--headless", "--consent-injector-fd=7"],
        ] {
            match parse_args(args.clone()) {
                Ok(Action::RunHeadless {
                    consent,
                    consent_injector_fd,
                    ..
                }) => {
                    assert!(
                        matches!(consent, ConsentPolicy::Interactive),
                        "{args:?} must keep the fail-closed interactive policy, not silently \
                         downgrade to auto-approve"
                    );
                    assert_eq!(consent_injector_fd, Some(7));
                }
                other => panic!("{args:?} must parse as an instrumented headless run: {other:?}"),
            }
        }

        // 3. The two companion refusals, taken at parse time rather than at
        //    first use -- this parser's rule everywhere else too.
        let nested = parse_args(["--nested", "--consent-injector-fd=7"])
            .expect_err("nested has a real human at a real pointer");
        assert!(nested.contains("--headless"), "{nested}");
        let auto = parse_args([
            "--headless",
            "--consent=auto-approve",
            "--consent-injector-fd=7",
        ])
        .expect_err("auto-approve raises no prompt, so the channel would be inert");
        assert!(auto.contains("--consent=interactive"), "{auto}");

        // 4. The flag's own shape: a repeat, a non-number, and any descriptor
        //    number this process's own stdio occupies.
        for bad in [
            vec![
                "--headless",
                "--consent-injector-fd=7",
                "--consent-injector-fd=8",
            ],
            vec!["--headless", "--consent-injector-fd=seven"],
            vec!["--headless", "--consent-injector-fd="],
            vec!["--headless", "--consent-injector-fd=-1"],
            vec!["--headless", "--consent-injector-fd=0"],
            vec!["--headless", "--consent-injector-fd=1"],
            vec!["--headless", "--consent-injector-fd=2"],
        ] {
            assert!(
                parse_args(bad.clone()).is_err(),
                "{bad:?} must be refused at parse time"
            );
        }
        // ...and the missing-value spelling.
        assert!(parse_args(["--headless", "--consent-injector-fd"]).is_err());

        // 5. Auto-approve without the flag is still legal, and still not the
        //    default -- the feature changes nothing about the other policies.
        assert!(parse_args(["--headless", "--consent=auto-approve"]).is_ok());
        assert!(parse_args(["--nested"]).is_ok());
    }

    #[test]
    fn recorder_path_parses_both_spellings_and_defaults_to_none() {
        // Omitted, the run uses the default path under the core's runtime
        // directory (resolved at startup, not here) -- `None` is "not
        // given", never "no recorder".
        assert_eq!(
            parse_args(["--headless", "--consent=auto-approve"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                agent_cursor: false,
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                #[cfg(feature = "consent-injector")]
                consent_injector_fd: None,
                #[cfg(feature = "physical-input-injector")]
                physical_input_fd: None,
            })
        );
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--recorder",
                "/tmp/run.jsonl",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--recorder=/tmp/run.jsonl",
            ],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: None,
                    recorder: Some(PathBuf::from("/tmp/run.jsonl")),
                    realm: None,
                    shim: None,
                    capture_dump: None,
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        // Valid with the nested mode too, and alongside --consent.
        assert_eq!(
            parse_args([
                "--nested",
                "--consent=auto-approve",
                "--recorder=/tmp/n.jsonl"
            ]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn realm_config_path_parses_both_spellings_and_defaults_to_none() {
        // `None` is "not given", never "no realm": omitted, startup
        // resolves the documented default path and still fails if nothing
        // is there.
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--realm",
                "/etc/vitrin/realm.toml",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--realm=/etc/vitrin/realm.toml",
            ],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: None,
                    recorder: None,
                    realm: Some(PathBuf::from("/etc/vitrin/realm.toml")),
                    shim: None,
                    capture_dump: None,
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        // Valid with the nested mode and alongside the sibling flags.
        assert_eq!(
            parse_args([
                "--nested",
                "--consent=auto-approve",
                "--recorder=/tmp/n.jsonl",
                "--realm=/tmp/realm.toml"
            ]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: Some(PathBuf::from("/tmp/realm.toml")),
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn malformed_or_repeated_realm_is_an_error() {
        // The `--recorder` precedent, flag for flag: no value to consume,
        // an empty path, and a repeat.
        assert!(parse_args(["--headless", "--realm"]).is_err());
        assert!(parse_args(["--headless", "--realm="]).is_err());
        assert!(parse_args(["--headless", "--realm", ""]).is_err());
        assert!(parse_args(["--headless", "--realm=/a.toml", "--realm=/b.toml"]).is_err());
        // The error names the flag the operator typed.
        assert!(
            parse_args(["--headless", "--realm=/a.toml", "--realm=/b.toml"])
                .unwrap_err()
                .contains("--realm")
        );
    }

    #[test]
    fn shim_path_parses_both_spellings_and_defaults_to_none() {
        // The shim binary is a core input (issue #103): omitted, `None` means
        // "resolve the sibling `vitrin-shim` at startup", never "no shim".
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--shim",
                "/usr/lib/vitrin/vitrin-shim",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--shim=/usr/lib/vitrin/vitrin-shim",
            ],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: None,
                    recorder: None,
                    realm: None,
                    shim: Some(PathBuf::from("/usr/lib/vitrin/vitrin-shim")),
                    capture_dump: None,
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        // Valid with the nested mode too, and the default is `None`.
        assert_eq!(
            parse_args(["--nested", "--shim=/opt/vitrin/vitrin-shim"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: Some(PathBuf::from("/opt/vitrin/vitrin-shim")),
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn malformed_or_repeated_shim_is_an_error() {
        // The `--realm` precedent, flag for flag: no value to consume, an
        // empty path, and a repeat that names the flag.
        assert!(parse_args(["--headless", "--shim"]).is_err());
        assert!(parse_args(["--headless", "--shim="]).is_err());
        assert!(parse_args(["--headless", "--shim", ""]).is_err());
        assert!(parse_args(["--headless", "--shim=/a", "--shim=/b"])
            .unwrap_err()
            .contains("--shim"));
    }

    #[test]
    fn capture_dump_parses_both_spellings_and_defaults_to_none() {
        // The P1.8.5 diagnostic knob (issue #107): omitted, `None`; given, the
        // path both spellings resolve to, in both run modes. `--headless`
        // carries `--consent=auto-approve` because interactive is refused
        // there (a headless core cannot draw a prompt).
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--capture-dump",
                "/tmp/internal.rgba",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--capture-dump=/tmp/internal.rgba",
            ],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: None,
                    recorder: None,
                    realm: None,
                    shim: None,
                    capture_dump: Some(PathBuf::from("/tmp/internal.rgba")),
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        // Valid nested too, and the default really is `None`.
        assert_eq!(
            parse_args(["--nested", "--capture-dump=/tmp/x.rgba"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: Some(PathBuf::from("/tmp/x.rgba")),
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard()
            })
        );
    }

    #[test]
    fn malformed_or_repeated_capture_dump_is_an_error() {
        // The `--shim` precedent: no value, an empty path, and a repeat that
        // names the flag.
        assert!(parse_args(["--headless", "--capture-dump"]).is_err());
        assert!(parse_args(["--headless", "--capture-dump="]).is_err());
        assert!(parse_args(["--headless", "--capture-dump", ""]).is_err());
        assert!(
            parse_args(["--headless", "--capture-dump=/a", "--capture-dump=/b"])
                .unwrap_err()
                .contains("--capture-dump")
        );
    }

    #[test]
    fn a_bad_realm_config_aborts_startup_with_an_actionable_message() {
        // Acceptance criterion 3 at the startup seam: `load_realms` is the
        // one path a session's realm comes from, and every failure is
        // fatal, not defaulted. (The message content itself is asserted in
        // `realm`'s tests, on `RealmConfigError`'s Display -- which is
        // verbatim what the `tracing::error!` above emits.)
        let _fd = crate::capture::tests::fd_lock();
        let absent = std::env::temp_dir().join(format!(
            "vitrin-main-absent-{}/realm.toml",
            std::process::id()
        ));
        assert!(load_realms(Some(&absent)).is_err());
    }

    #[test]
    fn malformed_or_repeated_recorder_is_an_error() {
        // No value to consume, an empty path, and a repeat flag.
        assert!(parse_args(["--headless", "--recorder"]).is_err());
        assert!(parse_args(["--headless", "--recorder="]).is_err());
        assert!(parse_args(["--headless", "--recorder", ""]).is_err());
        assert!(parse_args([
            "--headless",
            "--recorder=/tmp/a.jsonl",
            "--recorder=/tmp/b.jsonl"
        ])
        .is_err());
    }

    #[test]
    fn the_default_recorder_path_lives_beside_the_core_socket() {
        // Not env-dependent in this assertion: the shape is what matters --
        // the run's log lands in the core's own runtime directory, named
        // per-pid so concurrent runs never share a file.
        if let Ok(path) = default_recorder_path() {
            let dir = vitrin_ipc::paths::runtime_dir().expect("runtime dir resolved above");
            assert_eq!(path.parent(), Some(dir.as_path()));
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with(RECORDER_FILE_PREFIX), "{name}");
            assert!(
                name.ends_with(&format!("-{}.jsonl", std::process::id())),
                "{name}"
            );
        }
    }

    // -- the R6 auto-approve guard (P1.7.2) --------------------------------

    /// A scratch `principals.toml` holding exactly `identities`, mode 0600
    /// in a 0700 directory (what [`StaticVerifier::load`] demands). Returns
    /// the directory so the caller can remove it.
    fn scratch_registry(identities: &[&str]) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-r6-guard-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("principals.toml");
        let body: String = identities
            .iter()
            .map(|id| {
                format!(
                    "[[principal]]\nidentity = \"{id}\"\n\
                     token = \"0123456789abcdef0123456789abcdef\"\n\n"
                )
            })
            .collect();
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        (dir, path)
    }

    #[test]
    fn auto_approve_starts_only_for_a_demo_only_registry() {
        // The risk-R6 acceptance criterion. The guard is a whitelist of one
        // exact identity, not a headcount -- see `announce_consent_policy`
        // for why a count alone is satisfied by a registry that is
        // dangerous in practice.
        let (dir, path) = scratch_registry(&[DEMO_PRINCIPAL]);
        let (banner, verifier) = announce_consent_policy(ConsentPolicy::AutoApprove, Some(&path))
            .expect("a demo-only registry is exactly what auto-approve is for");
        assert!(banner.0.is_some(), "auto-approve must start its warning");
        // The audited registry is handed on, so `run_session` verifies
        // against the same document this guard just approved rather than
        // re-reading a file that could have changed in between.
        assert!(
            verifier.is_some(),
            "the guard must hand its loaded registry to the session"
        );
        drop(banner);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn auto_approve_refuses_a_registry_holding_more_than_the_demo_principal() {
        // Every shape of "more than the demo principal", including the one
        // a pure `rows > 1` check would wave through.
        let cases: [(&str, Vec<&str>); 4] = [
            (
                "the demo principal plus a second agent",
                vec![DEMO_PRINCIPAL, "vitrin://local/agent/second"],
            ),
            (
                // The case that motivates checking the name, not the count:
                // one row, and it is a production identity.
                "a single non-demo principal",
                vec!["vitrin://prod/agent/fleet-controller"],
            ),
            (
                // A near-miss name is not the demo principal.
                "a look-alike identity",
                vec!["vitrin://local/agent/demo2"],
            ),
            (
                "several non-demo principals",
                vec!["vitrin://local/agent/a", "vitrin://local/agent/b"],
            ),
        ];
        for (label, identities) in cases {
            let (dir, path) = scratch_registry(&identities);
            assert!(
                announce_consent_policy(ConsentPolicy::AutoApprove, Some(&path)).is_err(),
                "{label}: auto-approve must refuse to start"
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn auto_approve_refuses_a_registry_it_cannot_audit() {
        // Absence proves nothing (a missing file would otherwise be a
        // trapdoor: point `--principals` at nothing and the guard passes),
        // and neither does a file the runtime verifier itself would reject
        // -- the guard loads through `StaticVerifier::load` precisely so
        // the two can never disagree about the same file.
        use std::os::unix::fs::PermissionsExt;

        let absent = std::env::temp_dir().join(format!(
            "vitrin-r6-absent-{}/principals.toml",
            std::process::id()
        ));
        assert!(announce_consent_policy(ConsentPolicy::AutoApprove, Some(&absent)).is_err());

        // World-readable: a registry of bearer tokens anyone can steal.
        let (dir, path) = scratch_registry(&[DEMO_PRINCIPAL]);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(announce_consent_policy(ConsentPolicy::AutoApprove, Some(&path)).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_r6_guard_never_reads_the_registry_under_interactive() {
        // The fail-closed default needs no permission to run, so the R6
        // guard has nothing to decide and reads nothing. This is about the
        // guard alone: the listener does verify against the registry, so
        // `run_session` loads it on this path too -- see `load_verifier`,
        // whose refusal is a separate, differently-worded startup error.
        let absent = std::env::temp_dir().join("vitrin-r6-never-read/principals.toml");
        let (banner, verifier) = announce_consent_policy(ConsentPolicy::Interactive, Some(&absent))
            .expect("interactive starts regardless of the registry");
        assert!(
            banner.0.is_none(),
            "no auto-approve banner under interactive"
        );
        // ...and this *guard* still loads nothing. `run_session` loads the
        // registry for the interactive path itself (`load_verifier`), which
        // is a different refusal with a different message.
        assert!(verifier.is_none());
    }

    #[test]
    fn the_demo_principal_constant_is_the_one_the_example_registry_ships() {
        // The guard's whitelist and the shipped example must name the same
        // identity: an example an operator copies verbatim has to be the
        // configuration the guard accepts, or the demo cannot run.
        let example = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/principals.toml"
        ))
        .expect("the example registry is committed");
        assert!(
            example.contains(&format!("identity = \"{DEMO_PRINCIPAL}\"")),
            "examples/principals.toml no longer names {DEMO_PRINCIPAL}"
        );
        // Table headers only -- the file's prose mentions `[[principal]]`
        // several times while defining exactly one table.
        assert_eq!(
            example
                .lines()
                .filter(|line| line.trim() == "[[principal]]")
                .count(),
            1,
            "the example registry must stay demo-only, or copying it fails the R6 guard"
        );
    }

    /// A scratch directory holding a minimal `realm.toml` and a path for
    /// this run's flight-recorder log, so a test can drive [`run_session`]
    /// end to end without touching the operator's real configuration.
    fn scratch_session(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-run-session-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let realm = dir.join("realm.toml");
        std::fs::write(&realm, "[[realm]]\ncommand = \"/usr/bin/true\"\n").unwrap();
        (dir.clone(), realm, dir.join("run.jsonl"))
    }

    /// [`ExitCode`] implements neither `PartialEq` nor any accessor, so its
    /// `Debug` rendering is the only thing a test can compare. Wrapped once
    /// rather than repeated, so the awkwardness is named in one place.
    fn is_failure(code: ExitCode) -> bool {
        format!("{code:?}") == format!("{:?}", ExitCode::FAILURE)
    }

    #[test]
    fn run_session_refuses_auto_approve_against_a_registry_it_may_not_grant_for() {
        // `announce_consent_policy` is thoroughly tested on its own; what
        // was not tested is that anything CALLS it. Deleting the guard from
        // `run_session` -- the only path a session starts by -- left the
        // whole suite green, which means R6 was pinned on a helper rather
        // than on the behaviour. This asserts the startup path itself:
        // the session refuses, and nothing of it runs.
        let _fd = crate::capture::tests::fd_lock();
        let (dir, realm, log) = scratch_session("refuse");
        let (registry_dir, registry) = scratch_registry(&["vitrin://prod/agent/fleet-controller"]);
        let ran = std::cell::Cell::new(false);

        let code = run_session(
            ConsentPolicy::AutoApprove,
            Some(registry.clone()),
            Some(log.clone()),
            Some(realm.clone()),
            None,
            None,
            // Not an instrumented run: `--consent-injector-fd` is a headless
            // flag and this fixture drives `run_session` directly.
            false,
            |seed| {
                ran.set(true);
                (seed.recorder, Ok(()))
            },
        );

        assert!(
            is_failure(code),
            "a production registry under auto-approve must not start a session"
        );
        assert!(!ran.get(), "the backend must never run");
        assert!(
            !log.exists(),
            "a refused session must leave no flight-recorder log behind"
        );

        // And the same session with the demo-only registry does start --
        // so the assertion above is about the registry, not about
        // `run_session` being broken in this fixture.
        let (demo_dir, demo_registry) = scratch_registry(&[DEMO_PRINCIPAL]);
        // The happy path binds the core socket for real, so it runs against
        // a scratch `XDG_RUNTIME_DIR`: the single-core lock is a live
        // `flock`, and a test that took it in the operator's real runtime
        // directory would refuse to run beside a real `vitrind`.
        let runtime_dir = dir.join("xdg");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let previous = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
        let ran = std::cell::Cell::new(false);
        let code = run_session(
            ConsentPolicy::AutoApprove,
            Some(demo_registry),
            Some(log.clone()),
            Some(realm),
            None,
            None,
            false,
            |seed| {
                ran.set(true);
                (seed.recorder, Ok(()))
            },
        );
        match previous {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        assert!(!is_failure(code), "the demo registry starts cleanly");
        assert!(ran.get(), "the demo registry is what auto-approve is for");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&registry_dir).ok();
        std::fs::remove_dir_all(&demo_dir).ok();
    }

    #[test]
    fn the_auto_approve_warning_repeats_for_the_whole_session() {
        // R6 asks for a *persistent* warning, and "persistent" is the half
        // that a startup line also satisfies. What distinguishes them is
        // whether the warning is still being emitted while the policy is
        // still granting -- so that is what is asserted, by counting
        // emissions across a session that outlives one banner interval.
        //
        // The banner's interval is a parameter for exactly this: the
        // production caller passes 60 s, which no test may sit through.
        //
        // The subscriber has to be the GLOBAL one, not a thread-local
        // `set_default`: the repeats are emitted from the banner's own
        // thread, and a thread-local subscriber would see only the startup
        // line -- which is precisely the regression this test exists to
        // catch, so getting that wrong would make it vacuous. Counts are
        // keyed by the registry path each banner names, so this stays
        // exact even though other tests emit the same warning in parallel.
        let (dir, registry) = scratch_registry(&[DEMO_PRINCIPAL]);
        let interval = Duration::from_millis(20);
        install_warning_counter();

        let banner = AutoApproveBanner::start(&registry, interval);
        // A "session" several intervals long.
        std::thread::sleep(interval * 8);
        drop(banner);

        let seen = warnings_for(&registry);
        assert!(
            seen >= 3,
            "the warning must keep repeating while auto-approve is active, saw {seen}"
        );

        // ...and it stops when the policy does, rather than outliving it.
        std::thread::sleep(interval * 4);
        assert_eq!(
            warnings_for(&registry),
            seen,
            "a dropped banner must emit nothing further"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Auto-approve warnings seen so far, keyed by the registry path the
    /// warning named. A global map because the emitting thread is the
    /// banner's own (see the test above).
    static BANNER_WARNINGS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

    fn warnings_for(registry: &Path) -> usize {
        let map = BANNER_WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
        map.as_ref()
            .and_then(|m| m.get(&registry.display().to_string()).copied())
            .unwrap_or(0)
    }

    /// Install the global counting subscriber, once per test process.
    ///
    /// This is the only global subscriber the test binary installs; if a
    /// second test ever wants one, they must share this layer rather than
    /// race to `set_global_default`.
    fn install_warning_counter() {
        use std::sync::Once;
        use tracing_subscriber::layer::SubscriberExt;

        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            BANNER_WARNINGS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replace(HashMap::new());
            let subscriber = tracing_subscriber::registry().with(BannerCounter);
            tracing::subscriber::set_global_default(subscriber)
                .expect("no other global subscriber in the test binary");
        });
    }

    struct BannerCounter;

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BannerCounter {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            let mut visitor = PrincipalsField(None);
            event.record(&mut visitor);
            let Some(path) = visitor.0 else {
                return;
            };
            let mut map = BANNER_WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(map) = map.as_mut() {
                *map.entry(path).or_insert(0) += 1;
            }
        }
    }

    /// Pulls the `principals` field out of one warning event: the registry
    /// path identifies which banner emitted it.
    struct PrincipalsField(Option<String>);

    impl tracing::field::Visit for PrincipalsField {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "principals" {
                self.0 = Some(format!("{value:?}"));
            }
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "principals" {
                self.0 = Some(value.to_owned());
            }
        }
    }

    #[test]
    fn the_auto_approve_banner_stops_promptly_when_dropped() {
        // The repeat is a background thread; dropping the guard must wake
        // it immediately rather than waiting out the interval. A minute-long
        // hang on shutdown would be its own bug.
        let (dir, path) = scratch_registry(&[DEMO_PRINCIPAL]);
        let start = std::time::Instant::now();
        {
            let _banner = AutoApproveBanner::start(&path, AUTO_APPROVE_BANNER_INTERVAL);
        }
        assert!(
            start.elapsed() < AUTO_APPROVE_BANNER_INTERVAL,
            "dropping the banner must not wait out the repeat interval"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn principals_path_parses_both_spellings_and_defaults_to_none() {
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--principals",
                "/etc/vitrin/principals.toml",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--principals=/etc/vitrin/principals.toml",
            ],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    agent_cursor: false,
                    consent: ConsentPolicy::AutoApprove,
                    principals: Some(PathBuf::from("/etc/vitrin/principals.toml")),
                    recorder: None,
                    realm: None,
                    shim: None,
                    capture_dump: None,
                    dead_man: DeadManConfig::default(),
                    attention: default_attention(),
                    clipboard: default_clipboard(),
                    #[cfg(feature = "consent-injector")]
                    consent_injector_fd: None,
                    #[cfg(feature = "physical-input-injector")]
                    physical_input_fd: None,
                })
            );
        }
        // The `--recorder` precedent, flag for flag.
        assert!(parse_args(["--headless", "--principals"]).is_err());
        assert!(parse_args(["--headless", "--principals="]).is_err());
        assert!(parse_args(["--headless", "--principals", ""]).is_err());
        assert!(parse_args(["--headless", "--principals=/a", "--principals=/b"]).is_err());
    }

    #[test]
    fn malformed_or_repeated_consent_is_an_error() {
        assert!(parse_args(["--headless", "--consent", "yolo"]).is_err());
        assert!(parse_args(["--headless", "--consent=--nested"]).is_err());
        // `--consent` as the final argument has no value to consume.
        assert!(parse_args(["--headless", "--consent"]).is_err());
        assert!(parse_args([
            "--headless",
            "--consent=auto-approve",
            "--consent=interactive"
        ])
        .is_err());
    }

    #[test]
    fn malformed_size_is_an_error() {
        // Missing separator, empty field, extra field, non-numeric, and zero
        // dimensions are all rejected.
        assert!(parse_args(["--headless", "--size", "1280"]).is_err());
        assert!(parse_args(["--headless", "--size", "1280x"]).is_err());
        assert!(parse_args(["--headless", "--size", "1280x800x1"]).is_err());
        assert!(parse_args(["--headless", "--size", "widexhigh"]).is_err());
        assert!(parse_args(["--headless", "--size", "0x800"]).is_err());
        assert!(parse_args(["--headless", "--size", "1280x0"]).is_err());
        // `--size` as the final argument has no value to consume.
        assert!(parse_args(["--headless", "--size"]).is_err());
    }

    #[test]
    fn oversized_size_is_an_error() {
        // A dimension past i32::MAX would wrap negative on the backend's i32
        // cast; reject it rather than silently degenerating the output.
        assert!(parse_args(["--headless", "--size", "3000000000x800"]).is_err());
        assert!(parse_args(["--headless", "--size", "1280x4294967295"]).is_err());
        // The boundary value i32::MAX itself is in-domain and accepted.
        assert_eq!(
            parse_args([
                "--headless",
                "--consent=auto-approve",
                "--size",
                "2147483647x1"
            ]),
            Ok(Action::RunHeadless {
                size: (2147483647, 1),
                agent_cursor: false,
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                #[cfg(feature = "consent-injector")]
                consent_injector_fd: None,
                #[cfg(feature = "physical-input-injector")]
                physical_input_fd: None,
            })
        );
    }

    #[test]
    fn size_without_headless_is_an_error() {
        assert!(parse_args(["--size", "1280x800"]).is_err());
        assert!(parse_args(["--nested", "--size", "1280x800"]).is_err());
    }

    /// `--agent-cursor` (D-019): off by default on headless, on when asked
    /// for, and refused with `--nested` rather than accepted as a no-op.
    ///
    /// The default matters more than the flag does: this backend's
    /// human-visible framebuffer is measured against the realm view by the
    /// trusted-band witness and by `tests/integration/test_real_trust_band.py`,
    /// so a run that composited the sprite without being asked would turn a
    /// mock-free milestone gate red.
    #[test]
    fn the_agent_cursor_flag_is_headless_only_and_off_by_default() {
        let cursor_of = |args: Vec<&str>| match parse_args(args.clone()) {
            Ok(Action::RunHeadless { agent_cursor, .. }) => agent_cursor,
            other => panic!("{args:?} must parse as a headless run: {other:?}"),
        };
        assert!(!cursor_of(vec!["--headless", "--consent=auto-approve"]));
        assert!(cursor_of(vec![
            "--headless",
            "--consent=auto-approve",
            "--agent-cursor"
        ]));
        // A boolean switch repeated says the same thing twice.
        assert!(cursor_of(vec![
            "--headless",
            "--consent=auto-approve",
            "--agent-cursor",
            "--agent-cursor"
        ]));

        // Nested needs no opt-in, so the flag would do nothing there:
        // refused at parse time, the `--size` precedent.
        let nested = parse_args(["--nested", "--agent-cursor"])
            .expect_err("nested composites the agent cursor with no flag");
        assert!(nested.contains("--headless"), "{nested}");
        assert!(nested.contains("--agent-cursor"), "{nested}");
        // With no mode at all it is still the missing mode that is reported.
        assert!(parse_args(["--agent-cursor"]).is_err());
        // The flag is named in the help text, so an operator can find it.
        assert!(USAGE.contains("--agent-cursor"));
    }

    #[test]
    fn help_and_version_win_over_mode() {
        assert_eq!(parse_args(["--nested", "--help"]), Ok(Action::Help));
        assert_eq!(parse_args(["--headless", "--help"]), Ok(Action::Help));
        assert_eq!(parse_args(["--version"]), Ok(Action::Version));
    }

    #[test]
    fn print_isolation_is_answerable_from_any_command_line() {
        // The point of the flag is to report what a machine *would* grant, so
        // it must not require a command line that would otherwise have run.
        // Three cases, each a different way a run-configuring parse could
        // otherwise have swallowed it.
        assert_eq!(
            parse_args(["--print-isolation"]),
            Ok(Action::PrintIsolation)
        );
        assert_eq!(
            parse_args(["--nested", "--print-isolation"]),
            Ok(Action::PrintIsolation)
        );
        // It wins over a later parse error, exactly as `--help` does: a
        // machine with a typo in its flags can still be probed.
        assert_eq!(
            parse_args(["--print-isolation", "--size"]),
            Ok(Action::PrintIsolation)
        );
        // The flag is named in the help text, so an operator can find it.
        assert!(USAGE.contains("--print-isolation"));
    }

    #[test]
    fn no_mode_is_an_error() {
        assert!(parse_args([]).is_err());
    }

    #[test]
    fn unknown_argument_is_an_error() {
        assert!(parse_args(["--frobnicate"]).is_err());
        assert!(parse_args(["--nested", "extra"]).is_err());
    }

    /// The default attention chord, for the `Action` literals these tests
    /// compare against. Resolved through the real parser, so a change to the
    /// default is a change these tests follow rather than one they pin twice.
    fn default_attention() -> attention::AttentionChord {
        attention::AttentionChord::parse(attention::DEFAULT_CHORD).expect("the default parses")
    }

    fn default_clipboard() -> chord::Trigger {
        chord::Trigger::parse(clipboard::DEFAULT_TRIGGER).expect("the default parses")
    }

    /// The dead-man configuration this run resolved, whichever mode it named.
    fn dead_man_of(action: &Action) -> DeadManConfig {
        match action {
            Action::RunNested { dead_man, .. } | Action::RunHeadless { dead_man, .. } => *dead_man,
            other => panic!("not a run action: {other:?}"),
        }
    }

    #[test]
    fn the_dead_man_switch_defaults_to_hold_esc_for_one_second() {
        // The plan default, pinned: a session that came up with a different
        // off-switch than the one documented would be a surprise exactly
        // when a human most needs there not to be one.
        let action = parse_args(["--nested"]).expect("defaults parse");
        let config = dead_man_of(&action);
        assert_eq!(config.chord.name(), "esc");
        assert_eq!(config.hold, std::time::Duration::from_millis(1000));
        // Headless resolves the same policy from the same command line, so a
        // shared alias behaves identically in both modes (it then ignores
        // it: no physical input device exists there). Paired with
        // `--consent=auto-approve` because headless refuses interactive
        // consent (issue #90).
        assert_eq!(
            dead_man_of(
                &parse_args(["--headless", "--consent=auto-approve"]).expect("defaults parse")
            ),
            config
        );
    }

    /// The attention chord this run resolved, whichever mode it named.
    fn attention_of(action: &Action) -> attention::AttentionChord {
        match action {
            Action::RunNested { attention, .. } | Action::RunHeadless { attention, .. } => {
                *attention
            }
            other => panic!("not a run action: {other:?}"),
        }
    }

    #[test]
    fn the_attention_key_defaults_to_super_and_parses_both_spellings() {
        // WS-E.1.7's default, pinned for the same reason the dead-man's is: a
        // session that came up with a different attention key than the one
        // documented is a surprise at exactly the moment the human is trying
        // to find out why their switch does nothing.
        assert_eq!(
            attention_of(&parse_args(["--nested"]).expect("defaults parse")).name(),
            "super"
        );
        for args in [
            vec!["--nested", "--attention-chord", "rsuper"],
            vec!["--nested", "--attention-chord=rsuper"],
        ] {
            let chord =
                attention_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(chord.name(), "rsuper");
            // The other half keeps its default rather than being reset.
            assert_eq!(dead_man_of(&parse_args(args).unwrap()).chord.name(), "esc");
        }
        // Both modes accept the same command line, exactly as they do for the
        // dead-man switch: a shared alias must not behave differently.
        assert_eq!(
            attention_of(
                &parse_args([
                    "--headless",
                    "--consent=auto-approve",
                    "--attention-chord=rsuper"
                ])
                .expect("headless parses it too")
            )
            .name(),
            "rsuper"
        );
    }

    #[test]
    fn a_session_never_comes_up_with_an_attention_key_that_cannot_fire() {
        // The three startup refusals WS-E.1.7 requires, each with a message
        // naming what was wrong. An attention key that silently never fires is
        // the same fail-open configuration trap `Chord::parse` exists to
        // prevent -- one gesture milder, and just as invisible.

        // (1) Not in the vocabulary. The message must list what is.
        let err = parse_args(["--nested", "--attention-chord", "f13"])
            .expect_err("`f13` is not an attention key");
        assert!(err.contains("f13"), "{err}");
        assert!(err.contains("unknown attention key"), "{err}");
        assert!(err.contains("super") && err.contains("rsuper"), "{err}");

        // (2) A key `keysym_is_intakeable` rejects. Unreachable through the
        // CLI while the vocabulary and the intake table agree -- which is the
        // point -- so it is asserted at the parser that owns the check, on the
        // same `AttentionChordError` the CLI renders.
        assert_eq!(
            attention::AttentionChord::parse("f13"),
            Err(attention::AttentionChordError::Unknown)
        );
        assert!(attention::AttentionChord::vocabulary()
            .all(|name| attention::AttentionChord::parse(name).is_ok()));

        // (3) The two core-owned chords may not name the same key. Unreachable
        // today (the vocabularies are disjoint by construction) and refused
        // anyway: the day an edit makes it reachable is the day one of the two
        // keys silently stops working, and one of them is the off-switch. The
        // spelling the issue names is exercised, and it fails at (1) first --
        // `esc` is not an attention key -- which is itself the disjointness.
        let err = parse_args([
            "--nested",
            "--attention-chord",
            "esc",
            "--dead-man-chord",
            "esc",
        ])
        .expect_err("`esc` is not an attention key");
        assert!(err.contains("attention-chord"), "{err}");

        // ...and the collision check itself, reached the only way it can be:
        // by asking the parser's own comparison. If a later edit puts a shared
        // key in both vocabularies, this is what turns red.
        let dead: Vec<u32> = deadman::Chord::vocabulary()
            .map(|n| deadman::Chord::parse(n).unwrap().keysym())
            .collect();
        for name in attention::AttentionChord::vocabulary() {
            let keysym = attention::AttentionChord::parse(name).unwrap().keysym();
            assert!(
                !dead.contains(&keysym),
                "`{name}` is in both chord vocabularies -- the startup collision refusal is \
                 now reachable, and the two chords can no longer both work"
            );
        }
    }

    #[test]
    fn the_attention_chord_flag_is_refused_twice_and_bare() {
        let err = parse_args([
            "--nested",
            "--attention-chord",
            "super",
            "--attention-chord",
            "rsuper",
        ])
        .expect_err("a valued flag repeated must not silently pick a winner");
        assert!(err.contains("given more than once"), "{err}");
        assert!(
            parse_args(["--nested", "--attention-chord"]).is_err(),
            "a bare flag takes the next argument; there is none"
        );
    }

    #[test]
    fn dead_man_flags_parse_both_spellings_and_default_independently() {
        for args in [
            vec!["--nested", "--dead-man-chord", "f12"],
            vec!["--nested", "--dead-man-chord=f12"],
        ] {
            let config =
                dead_man_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(config.chord.name(), "f12");
            // The other half keeps its default rather than being reset.
            assert_eq!(config.hold, std::time::Duration::from_millis(1000));
        }
        for args in [
            vec!["--nested", "--dead-man-hold", "400"],
            vec!["--nested", "--dead-man-hold=400"],
        ] {
            let config =
                dead_man_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(config.hold, std::time::Duration::from_millis(400));
            assert_eq!(config.chord.name(), "esc");
        }
        // Both together.
        let config = dead_man_of(
            &parse_args([
                "--nested",
                "--dead-man-chord=delete",
                "--dead-man-hold=2500",
            ])
            .expect("both parse"),
        );
        assert_eq!(config.chord.name(), "delete");
        assert_eq!(config.hold, std::time::Duration::from_millis(2500));
    }

    #[test]
    fn an_unusable_dead_man_configuration_is_a_startup_error() {
        // The fail-open trap this closes: a session that starts with an
        // off-switch which silently never fires. Every refusal here is a
        // startup error, never a default quietly substituted.
        //
        // A key this build's intake cannot deliver...
        let err = parse_args(["--nested", "--dead-man-chord", "f13"]).unwrap_err();
        assert!(err.contains("--dead-man-chord"), "{err}");
        assert!(
            err.contains("accepted:"),
            "the error must list the vocabulary: {err}"
        );
        assert!(err.contains("esc"), "{err}");
        // ...a modifier, which would fire during ordinary text selection...
        assert!(parse_args(["--nested", "--dead-man-chord", "shift"]).is_err());
        // ...a hold short enough to trip on an ordinary keypress...
        assert!(parse_args(["--nested", "--dead-man-hold", "10"]).is_err());
        // ...one nobody could complete...
        assert!(parse_args(["--nested", "--dead-man-hold", "600000"]).is_err());
        // ...and values that are not durations at all.
        assert!(parse_args(["--nested", "--dead-man-hold", "1s"]).is_err());
        assert!(parse_args(["--nested", "--dead-man-hold", "-1"]).is_err());
        assert!(parse_args(["--nested", "--dead-man-hold"]).is_err());
        assert!(parse_args(["--nested", "--dead-man-chord"]).is_err());
        // Repeats are refused like every other flag here.
        assert!(parse_args(["--nested", "--dead-man-chord=esc", "--dead-man-chord=f12"]).is_err());
        assert!(parse_args(["--nested", "--dead-man-hold=300", "--dead-man-hold=400"]).is_err());
    }

    #[test]
    fn duplicate_or_conflicting_mode_is_an_error() {
        assert!(parse_args(["--nested", "--nested"]).is_err());
        assert!(parse_args(["--headless", "--headless"]).is_err());
        assert!(parse_args(["--nested", "--headless"]).is_err());
    }
}
