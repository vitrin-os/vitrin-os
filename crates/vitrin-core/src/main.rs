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
//!   byte assembly (`paint::canvas`), exactly as `scene` does. The headless
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
/// The human's **brightness keys** (D-041, issue #303): the one filesystem
/// *write* the trusted core makes on its own behalf. `--backlight` opts a
/// bare-metal session in; the two `XF86MonBrightness*` keys are then consumed
/// by the core and turned into a bounded write to `/sys/class/backlight`,
/// physical origin only, with no verb, no wire surface and no path that can
/// reach a black panel. Bounded exactly the way `status::battery`'s read is,
/// which is the owner's accepted implementation constraint rather than a
/// resemblance.
mod backlight;
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
/// The lock screen (WS-E.2.2, issue #214): the core-drawn surface that takes
/// the session's physical input away from every realm until a human proves
/// they are there. A lock screen makes an authority claim — "nothing behind me
/// can see your input" — and only the component that owns input routing can
/// make that claim true, which is why this is core code and not a client
/// (PRD §5.1 exiles window-management *policy*, never the trusted path).
/// `lock::gate` is the outermost preemption hook and consumes ALL physical
/// input while raised; `lock::LockSurface` composites at the same output-stage
/// fork the consent card does, so it can no more reach a capture. Read
/// `docs/book/src/limits.md` before believing this is a security boundary: it
/// does not suspend an agent's grants, and in nested mode it locks a window
/// rather than a session.
#[cfg_attr(not(test), allow(dead_code))]
mod lock;
/// The rasterizer every core-drawn trusted surface shares (WS-E.2.2): the
/// borrowed RGBA canvas, the embedded-font glyph engine, and the one placement
/// function a core-drawn card is centered by. Moved out of `consent` when the
/// lock screen arrived, because `deadman`, `cursor` and `attention` were
/// already importing `consent::canvas` — a path that said the off-switch draws
/// with the consent surface's private tools. It confers nothing and holds no
/// session state; the authority claims stay in `consent` and `lock`.
mod paint;
/// **A notice the human can read on the panel they are trying to leave**
/// (WS-E.3.5): a bounded, expiring, human-visible-only band the core raises
/// when something it cannot fix has happened to the session itself. Its only
/// producer today is the bare-metal VT escape ([`vt`]) — a `change_vt` that is
/// refused or never acknowledged leaves a human staring at a key that did
/// nothing, which is exactly the defect first light found, and a `stderr` line
/// is worth nothing to somebody who cannot leave the screen to read it.
mod notice;
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
/// **The human's screenshot key** (WS-E.2.4, issue #216): a core-owned chord
/// that writes one PNG of the focused realm's view into one
/// `--screenshot-dir`, and touches no grant at any point — there is no facet,
/// no verb and no principal in it, because a human photographing their own
/// screen is not an agent capability. It captures the realm view rather than
/// human-visible output, so the session's trusted-indicator secret can never
/// reach a file; the cost, that a vitrin screenshot cannot show a consent
/// prompt, is published in `docs/book/src/limits.md`.
mod screenshot;
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
/// believing any confinement claim: what `--isolation=off` still leaves off
/// (no namespaces, no Landlock, no seccomp), what `--isolation=default`
/// applies (namespaces since #186, a Landlock ruleset since #187, and a
/// seccomp DENY-LIST since #188 -- a named-class claim and not a completeness
/// one), and the session-D-Bus hole are stated there in full. Called at runtime by `session::start_realm`, which
/// runs it only after `session::install` has put the loop's sources in
/// place: a shim spawned into a loop that is not yet servicing its
/// socketpair blocks on `configure` forever, with no timeout on its side.
mod spawn;
/// The status strip (WS-E.2.3, issue #215): clock, battery and the focused
/// realm's name, drawn beside the trusted band and never in it.
mod status;
mod test_pattern;
/// The strict TOML subset every core configuration file is written in
/// (P1.4.1's `principals.toml`, P1.5.1's `realm.toml`): one hand-rolled
/// lexer for hostile config bytes, not one per schema. Plan risk R7 keeps a
/// TOML crate out of the TCB; having a single lexer is what keeps that
/// choice from multiplying parsers.
mod toml_subset;
/// **The usable realm view** (issue #304): `ViewGeometry`, the one value that
/// carries an output's size together with the rows the core reserves above the
/// client — the trusted band always, the status strip when `--status` is on.
/// Every path that places client pixels and the `configure` the shim is told
/// take the placement from it, which is what closes the unmet half of issue
/// #215: the app is configured smaller instead of having its top rows
/// overdrawn.
mod view;
/// **The VT escape** (WS-E.3.5, D-031): `Ctrl-Alt-F1`..`Ctrl-Alt-F12`, matched
/// on physical input only, consumed in every realm, and turned into the one
/// `Session::change_vt` call this workspace contains. Bare metal only, because
/// only a process holding DRM master can implement the chord the kernel stops
/// handling once it does.
#[cfg(feature = "drm-backend")]
mod vt;

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
                                a wire feature. Written atomically, and only
                                when that realm's view is recomposed: a realm
                                whose scene has not changed rewrites nothing,
                                so the file's mtime is not a signal that a
                                frame landed.
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
    vitrind [--lock-chord CHORD]
                                NOT with `--headless`: the chord that raises the lock
                                screen (WS-E.2.2). MOD[+MOD...]+KEY; modifiers
                                ctrl, shift, alt, super; the key must be
                                layout-invariant and non-modifier -- a chord
                                key whose keysym moved with the layout would
                                be a gesture that stops working on somebody
                                else's keyboard, so letters and digits are not
                                in the vocabulary on any backend. Default:
                                ctrl+alt+delete. The chord is CONSUMED -- no
                                confined app sees it. Startup FAILS if its key
                                is also --dead-man-chord's, --attention-chord's
                                or --clipboard-key's.
    vitrind [--lock-idle SECS]  NOT with `--headless`: raise the lock screen after
                                SECS with no PHYSICAL input. An agent's
                                actuations never postpone it -- a session an
                                agent is working in is still a session the human
                                left. Off by default; 0 is REFUSED rather than
                                read as off (it would relock every round).
    vitrind [--lock-on-seat-change POLICY]
                                `--drm` ONLY: what a VT switch AWAY does to the
                                lock. Three policies, and the default is the
                                behaviour this core has always had:
                                  immediate -- losing the seat raises the lock
                                    at once, so coming back always costs a
                                    passphrase (or an Enter, with no
                                    --lock-passphrase-file).
                                  idle      -- the idle countdown keeps running
                                    across the absence, so a LONG switch-away
                                    returns to a locked screen and a short one
                                    does not. Needs --lock-idle to mean
                                    anything; with no idle timeout there is no
                                    countdown to keep running.
                                  never     -- (default) the countdown freezes
                                    for the absence and restarts on return.
                                    The cost, published in
                                    docs/book/src/limits.md: a session you
                                    switched away from eight hours ago is
                                    unlocked when you switch back to it.
                                NO policy lowers a lock that is already up: a
                                VT switch is never a way past a lock screen.
                                REFUSED with --nested and --headless, which
                                have no seat to lose.
    vitrind [--lock-passphrase-file PATH]
                                NOT with `--headless`: unlock requires the passphrase
                                whose Argon2id digest PATH holds. Absolute path,
                                regular file, owned by this uid, mode 600 --
                                stricter than realm.toml's rule, because whoever
                                can READ it gets an offline attack. REJECTED
                                with --headless, naming the reason: a headless
                                backend has no keyboard and no keymap, so it
                                delivers no letters and the passphrase could
                                never be typed. WITHOUT this flag the lock is an
                                unauthenticated privacy screen dismissed by
                                Enter, and the card says so.

                                *** A LOCKED SCREEN DOES NOT SUSPEND AGENTS. ***
                                An agent holding `observe` keeps capturing the
                                realm across a lock, and one holding `actuate_*`
                                keeps acting: observation is concurrent by
                                design (protocol/vitrin-v0.xml, vitrin_view) and
                                a lock takes away the HUMAN's input, not an
                                agent's authority. The instrument for `stop
                                everything` is still the dead-man chord, which
                                revokes every grant and fires while locked. See
                                docs/book/src/limits.md.
    vitrind --lock-hash         Read a passphrase from STDIN (one line, trailing
                                newline stripped) and print the single line a
                                --lock-passphrase-file holds, then exit. The
                                passphrase never appears in argv, which is
                                world-readable through /proc. Use:
                                  read -rs PASS
                                  printf %s $PASS | vitrind --lock-hash \\
                                    > ~/.config/vitrin/lock.hash
                                  chmod 600 ~/.config/vitrin/lock.hash
    vitrind [--blank-idle SECS] `--drm` ONLY (WS-E.4.3): after SECS with no
                                PHYSICAL input, power the panel down. Any
                                physical event brings it back, and that event
                                is CONSUMED -- a key aimed at a dark screen
                                neither commits a consent card nor reaches an
                                app. An agent's actuations neither postpone the
                                blank nor wake the screen: there is no verb in
                                the protocol for `power the human's display`,
                                and one that could be triggered remotely under
                                no grant is not one this core is adding. Off by
                                default; 0 is REFUSED rather than read as off.

                                *** IT DOES NOT LOCK. ***
                                The session stays UNLOCKED behind the dark
                                screen. Locking is --lock-idle and the lock
                                chord, and the two are coupled by nothing but a
                                shared activity clock. Anyone who walks up and
                                presses a key is inside the session. Two more
                                consequences, stated rather than softened: a
                                dark screen is not evidence that nothing is
                                being OBSERVED (a lock does not suspend agents
                                either -- D-025), and it is not evidence that
                                anything is being observed LIVE, because a
                                disabled CRTC produces no vblank, so no realm's
                                frame_done is discharged and every paced app
                                stops painting until the human comes back. See
                                docs/book/src/limits.md.

                                REFUSED with --nested (a client of another
                                compositor cannot power a panel it does not own)
                                and with --headless (no output, and no lock gate
                                in its hook stack, so nothing would ever write
                                the activity clock a wake reads -- the session
                                would go dark and stay dark).
    vitrind [--backlight]       `--drm` ONLY (D-041): let the core WRITE
                                /sys/class/backlight so XF86MonBrightnessUp and
                                Down actuate. Off by default -- a write into a
                                kernel-owned tree from the trusted core is not
                                something a session should pay for without
                                asking, which is --status's argument applied to
                                a stronger authority. One press moves the panel
                                by 5% of that device's max_brightness, rounded
                                up and never by less than 1 raw unit (so a
                                ceiling of 10 moves by 1, which is 10%).

                                *** THE TWO KEYS ARE CONSUMED. ***
                                They stop reaching the focused realm's app
                                entirely, which is a REVERSAL of what WS-E.4.3
                                shipped. A nested compositor, a VM viewer or a
                                remote-desktop client inside a realm loses both
                                keys, with no pass-through and no way to ask for
                                one -- the price D-023 paid for Super and D-024
                                for Shift-Insert, paid again for two more keys.
                                Without this flag they are delivered exactly as
                                before.

                                *** IT DOES NOT BLANK, AND IT NEVER REACHES 0. ***
                                The backlight is not this core's blanking
                                mechanism and D-033's refusal of it as one
                                stands: --blank-idle powers the panel down, this
                                only dims it, and the two paths do not know
                                about each other. The lowest value this core
                                will ever write is 5% of max_brightness --
                                rounded UP, so it is a floor and not a number the
                                arithmetic rounds through -- because a black
                                panel is indistinguishable from a blanked one and
                                the core already owns the blank.

                                *** IT DOES NOTHING FOR AN EXTERNAL DISPLAY, ***
                                and nothing for volume. No agent can trigger it:
                                physical key presses only, no verb, no wire
                                message, nothing to grant and nothing to refuse.
                                Every failure -- no device, an unreadable value,
                                a file this uid cannot write -- is the key doing
                                nothing, journalled and never fatal. Whether it
                                works at all is a property of this machine's udev
                                rules (a `video` group membership or a seat tag),
                                not of this build. See docs/book/src/limits.md.
    vitrind --backlight-device NAME
                                Which device under /sys/class/backlight to
                                write, e.g. `intel_backlight`. A NAME, never a
                                path: it must be an immediate child of that
                                directory. Default: the sorted-first device with
                                a readable max_brightness -- which is
                                deterministic and can be the WRONG one, because
                                a laptop with both `acpi_video0` and
                                `intel_backlight` sorts the first of those first
                                and on a lot of hardware that one does nothing.
                                The startup log names the device it chose. Needs
                                `--backlight`.
    vitrind --status            Draw the status strip: the focused realm's
                                name, the battery and a clock, in a reserved
                                band of rows immediately BELOW the trusted
                                indicator band and never inside it. Off by
                                default: it puts a ticking clock on the
                                human-visible output, which every byte-for-byte
                                comparison of that output (the trusted-band
                                witness, the goldens) would otherwise become a
                                function of. There is no client status bar and
                                there cannot be one -- zwlr_layer_shell_v1 is
                                not in the shim's global contract -- so this is
                                the whole of the status UI: no tray, no
                                notifications, no workspace switcher, no click
                                targets. See docs/book/src/limits.md.
    vitrind --status-height N   Rows the strip occupies, 16..=64 (default 20 --
                                the 14-row line box the bundled face reports at
                                12 px, plus 3 rows above and below). Needs
                                `--status`.
    vitrind --status-utc-offset O
                                The clock's fixed offset from UTC: `UTC`, or a
                                signed `+HH:MM` / `-HH:MM` between -12:00 and
                                +14:00. Default UTC. The core carries NO
                                timezone database -- a tz parser and a
                                recurring read of /usr/share/zoneinfo is
                                authority the TCB is not taking for a cosmetic
                                field -- so there is no DST and the strip always
                                labels the zone it is showing. Needs `--status`.
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
    vitrind [--screenshot-dir PATH]
                                Enable the human's SCREENSHOT KEY (WS-E.2.4)
                                and write its PNGs into PATH. Absent = the key
                                does nothing. PATH must be ABSOLUTE, exist, be
                                a directory (not a symlink to one), be owned by
                                root or this uid, and not be writable by group
                                or other -- whoever can write or rename it
                                chooses where pictures of the screen land.
                                Startup FAILS naming the reason otherwise; the
                                directory is opened ONCE and held, so no later
                                rename or symlink can redirect a write. File
                                names are minted by the core
                                (vitrin-<epoch>-NNNN.png, mode 600) and nothing
                                a client controls reaches a path component.

                                *** A VITRIN SCREENSHOT SHOWS THE REALM ONLY.***
                                It is the realm's view, NOT what you see: no
                                trusted band, no consent prompt, no lock screen,
                                no status strip, no agent cursor. The band's
                                colour IS this session's secret, the confined
                                app runs as this uid and can read any file the
                                core writes, so a screenshot of it would hand a
                                forger the one thing that tells a real prompt
                                from a fake. You cannot screenshot a suspicious
                                dialog; use a phone. See docs/book/src/limits.md.
    vitrind [--screenshot-chord CHORD]
                                The chord that takes one (WS-E.2.4).
                                MOD[+MOD...]+KEY, same vocabulary as
                                --lock-chord. Default: ctrl+print. NOT a bare
                                Print -- the core's chord matcher requires a
                                modifier, and the modifier is what leaves bare
                                PrintScreen to the confined app. The chord is
                                CONSUMED. Startup FAILS if its key is also
                                --dead-man-chord's, --attention-chord's,
                                --clipboard-key's or --lock-chord's, or if
                                --screenshot-dir was not given (a configured
                                gesture with nowhere to write is a key that
                                silently does nothing).
    vitrind [--isolation MODE]  How much confinement every realm this session
                                spawns gets. `default` (the default) puts each
                                realm in its own user, mount, PID, IPC, UTS and
                                network namespaces with an identity uid/gid map
                                and zero capabilities, and REFUSES TO START if
                                this kernel will not grant them -- run
                                `--print-isolation` to see why, and
                                `--print-floor` to see what this build requires.
                                `off` runs the pre-2.6.2 spawn path: no
                                namespaces at all, the realm sees the whole
                                filesystem and every socket on the machine, and
                                the session warns about it every 60 seconds.
                                `off` is NOT a rung of the default/hardened/
                                paranoid dial -- it is the dial being switched
                                out. `none` is refused by name: it was renamed
                                to `off` before it shipped.
    vitrind [--landlock MODE]   How far up the Landlock ABI ladder the realm's
                                ruleset is built (P2.6.3). `highest` (the
                                default) asks for the highest rung this build
                                knows that this kernel accepts. THERE IS A
                                FLOOR, not a degradation ladder: a kernel below
                                the ABI this build declares (--print-floor,
                                `build.landlock_min_abi`) REFUSES TO START
                                rather than confining realms at a weaker rung
                                than the session's own journal names. The
                                one-rung descent still exists and bottoms out
                                at that floor.
                                `abi:N` CAPS the request at rung N: it never
                                raises anything (a rung above the kernel's own
                                is refused at ruleset creation) and exists so
                                each rung's absence can be MEASURED rather
                                than described, because capping to N
                                reproduces exactly what this build asks for on
                                an ABI-N kernel. THREE RUNGS ARE THE
                                EXCEPTION, and they are the reason that
                                sentence is not `reproduces an ABI-N kernel`:
                                a cap moves `handled_access_fs`, so it can
                                only simulate the absence of a rung that MOVES
                                that mask. ABI 4 buys network scoping and ABI
                                7 and 8 buy `landlock_restrict_self` FLAGS
                                (audit-log control; TSYNC) -- this build
                                requests none of the three, passing
                                `handled_access_net` and the flags word as
                                zero. So the enforced domain is BYTE-IDENTICAL
                                at rungs 3 and 4, and byte-identical again at
                                rungs 6, 7 and 8: nine rung numbers, six
                                distinct domains. `abi:4` and `abi:7` are
                                still accepted and still journal their own
                                `applied_profile` string, so read that string
                                as WHICH RUNG WAS OBTAINED and never as how
                                much confinement. It is a
                                DIAL, not a one-way weakening: rung 1 cannot
                                handle REFER, and a domain that does not
                                handle REFER forbids rename/link ACROSS
                                DIRECTORIES outright -- so `abi:1` is stricter
                                than `highest` about reparenting (and weaker
                                about everything else) and breaks apps that
                                write by rename-into-place. `off` builds no
                                ruleset at all; it is the positive control the
                                per-rung gates need, and a session running that
                                way journals `namespaces-only` and warns. The
                                rung actually OBTAINED is what
                                `applied_profile` names, with the request and
                                the kernel's own ABI beside it, and a realm
                                whose ladder fell below either is warned about
                                at spawn -- a pinned-low session cannot read as
                                a full-strength one.
    vitrind [--realm-init PATH] The helper binary the core execs to build a
                                realm's namespaces at --isolation=default.
                                Default: a sibling `vitrin-realm-init` beside
                                this `vitrind`. Audited transitively at spawn
                                exactly like --shim and `command`, and its
                                version must match this core's exactly -- a
                                helper from one install beside a vitrind from
                                another is refused, never run.
    vitrind --help              Show this help.
    vitrind --version           Show the version.
    vitrind --print-floor       Print the confinement mechanisms THIS BUILD
                                applies, and therefore refuses to start
                                without, then exit. A separate verb from
                                --print-isolation on purpose: that one reports
                                kernel facts and feeds a checked-in per-kernel
                                matrix, and a build constant among them would
                                invalidate the matrix over a row that is not a
                                kernel fact.
    vitrind --print-seccomp     Print the seccomp deny-list this build
                                compiles -- one row per denied syscall or
                                syscall+argument predicate, each carrying the
                                PRD Doc 2 section 15 escape class it closes and
                                the errno it returns -- then exit. A build
                                constant like --print-floor: it says what this
                                binary would install, never what a kernel did
                                with it. The filter is a DENY-LIST, so the
                                table is a named-class claim and never a
                                completeness one; the residual syscall surface
                                is unenumerated and docs/book/src/limits.md
                                says so in as many words.
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

/// The `--drm` half of the help, appended by [`Action::Help`] only in a build
/// that has the backend.
///
/// A second const rather than lines inside [`USAGE`] because a build without
/// `drm-backend` has no `--drm` arm in `parse_args` at all: help that named a
/// flag the parser answers ``unknown argument`` to would be worse than help
/// that omits it.
#[cfg(feature = "drm-backend")]
const DRM_USAGE: &str = "
BARE METAL (WS-E.3.2):
    vitrind --drm               Run on the display controller itself: DRM/KMS
                                mode setting, a GBM swapchain, libinput for
                                the human's real devices, libseat for the
                                seat. This process takes DRM MASTER -- run it
                                from a free VT, never from inside a desktop
                                session.
                                It REQUIRES `--consent=interactive` (the
                                default) and refuses `auto-approve`: this
                                backend is the human's display and there is a
                                human at the keyboard, so a petition can
                                always be shown and answered.
                                It refuses `--size` (the mode's size is the
                                panel's), `--agent-cursor` (drawn with no
                                opt-in where a human is watching), and both
                                injector descriptors (headless-only).
                                The `--lock-*` flags ARE valid here, unlike
                                with `--headless`.
                                No green check in this repository proves this
                                backend lights a panel: CI has no DRM device
                                and no seat. See WS-E.3.4.
    vitrind [--keymap PATH]     The compiled xkb keymap this session resolves
                                libinput scancodes through (`--drm` only;
                                WS-E.3.1, D-028). Produce one with
                                `xkbcli compile-keymap --layout LAYOUT > PATH`.
                                PATH and every directory above it must be
                                owned by root or this uid and not writable by
                                group or other -- whoever can write the keymap
                                chooses what the trusted core believes the
                                human typed. Startup FAILS naming the reason
                                otherwise, and also if the keymap does not put
                                every core-owned chord trigger on its own key
                                (a layout that moved Escape would leave the
                                dead-man switch unfirable).
                                WITHOUT it the session still runs and still
                                fires every core chord, but resolves only the
                                layout-invariant scancode table: NO letter, NO
                                digit and NO punctuation reaches any app, and
                                `--lock-passphrase-file` is refused because
                                the passphrase could not be typed.
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
        /// The session's confinement selection (P2.6.2, #186). Accepted on
        /// every run mode, because the namespaces a realm gets are a
        /// property of the spawn and not of how pixels reach a screen.
        isolation: IsolationOptions,
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
        /// The lock screen's policy (WS-E.2.2): chord, idle timeout, and the
        /// optional passphrase file's PATH.
        ///
        /// **Nested only, and there is no field for it on
        /// [`Action::RunHeadless`]** -- the three flags are refused with
        /// `--headless` at parse time, each for its own named reason
        /// ([`crate::lock::PASSPHRASE_NEEDS_A_KEYMAP`] and
        /// [`LOCK_NEEDS_PHYSICAL_INPUT`]), rather than accepted as a silent
        /// no-op. That is the `--agent-cursor` and `--size` posture, taken here
        /// for a sharper reason: a headless idle raise really would fire, on a
        /// backend with no device that could dismiss it.
        ///
        /// The passphrase is a path rather than a loaded digest because this
        /// enum is `PartialEq` and printed in parse tests; the file is read at
        /// the top of `backend::winit::run_inner`, before the listener accepts
        /// anyone, so a bad one is still a startup failure.
        lock: lock::LockConfig,
        /// The screenshot key's policy (WS-E.2.4, issue #216): the audited
        /// `--screenshot-dir` (or `None`, meaning the key does nothing) and the
        /// `--screenshot-chord` that takes one.
        ///
        /// Valid in **both** modes and carried on both variants, for `status`'s
        /// reason below: a plain headless build has no device to press the
        /// chord on, but the same command line must be accepted or refused
        /// identically in both modes, and a `physical-input-injector` build
        /// really can press it -- which is what gives this mechanism a
        /// mock-free gate at all.
        screenshot: screenshot::ScreenshotConfig,
        /// The status strip's policy (WS-E.2.3, issue #215): whether to draw
        /// one, how tall, and which clock.
        ///
        /// Valid in **both** modes and carried on both variants, unlike
        /// `lock` and `agent_cursor`: the strip needs no physical input device
        /// to be useful and no display to be composited, so refusing it under
        /// one backend would be a refusal with no reason behind it. Off by
        /// default in both (`crate::status`'s module docs).
        status: status::StatusConfig,
    },
    RunHeadless {
        size: (u32, u32),
        consent: ConsentPolicy,
        principals: Option<PathBuf>,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
        shim: Option<PathBuf>,
        /// The session's confinement selection (P2.6.2, #186). Accepted on
        /// every run mode, because the namespaces a realm gets are a
        /// property of the spawn and not of how pixels reach a screen.
        isolation: IsolationOptions,
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
        /// The screenshot key's policy (WS-E.2.4, issue #216). Accepted here
        /// as well as on [`Action::RunNested`] -- see that variant's field.
        screenshot: screenshot::ScreenshotConfig,
        /// The status strip's policy (WS-E.2.3, issue #215). Accepted here as
        /// well as on [`Action::RunNested`] -- see that variant's field for why
        /// this one is not backend-gated.
        status: status::StatusConfig,
    },
    /// Run on bare metal: DRM/KMS mode setting, a GBM swapchain, libinput and
    /// libseat (WS-E.3.2, issue #218). `#[cfg(feature = "drm-backend")]`, like
    /// [`Mode::Drm`] itself.
    ///
    /// It carries the nested variant's fields and one more. There is no
    /// `size` (the mode's size is the panel's and no flag may contradict it),
    /// no `agent_cursor` (the sprite is unconditional where a human is
    /// watching), and no injector descriptor (both channels are headless-only
    /// and both are refused here) — each of those is a *refusal* at parse
    /// time rather than a field this variant accepts and ignores.
    #[cfg(feature = "drm-backend")]
    RunDrm {
        consent: ConsentPolicy,
        principals: Option<PathBuf>,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
        shim: Option<PathBuf>,
        /// The session's confinement selection (P2.6.2, #186). Accepted on
        /// every run mode, because the namespaces a realm gets are a
        /// property of the spawn and not of how pixels reach a screen.
        isolation: IsolationOptions,
        capture_dump: Option<PathBuf>,
        dead_man: DeadManConfig,
        attention: attention::AttentionChord,
        clipboard: chord::Trigger,
        /// The lock screen's policy, accepted here on the same terms as
        /// `--nested` and for the same reason: a human is at a real keyboard,
        /// so a lock this session raises can be answered. The passphrase half
        /// additionally requires `--keymap` — see [`Action::RunDrm::keymap`].
        lock: lock::LockConfig,
        screenshot: screenshot::ScreenshotConfig,
        status: status::StatusConfig,
        /// `--blank-idle SECS` (WS-E.4.3, issue #223): power the panel down
        /// after SECS with no physical input, `None` for never.
        ///
        /// **Bare metal only, and there is no field for it on the other two
        /// variants** -- the flag is refused with `--nested` and `--headless`
        /// at parse time, naming the reason
        /// ([`crate::backend::blank::BLANK_NEEDS_THE_OUTPUT`]), rather than
        /// accepted as a silent no-op. That is `--agent-cursor`'s posture with
        /// a sharper edge: on headless the flag would not be inert, it would
        /// wedge the session dark, because that backend's hook stack has no
        /// lock gate and so nothing writes the activity clock a wake reads.
        ///
        /// **It does not lock.** On idle the screen goes dark and the session
        /// stays unlocked (Taha, 2026-08-10); locking is `--lock-idle` and the
        /// lock chord, and the two are coupled by nothing but a shared clock.
        blank_idle: Option<Duration>,
        /// `--backlight` (D-041, issue #303): the human's brightness keys are
        /// consumed by the core and turned into a bounded write to
        /// `/sys/class/backlight`, with `--backlight-device NAME` naming which
        /// device when the sorted-first one is the wrong one.
        ///
        /// **Bare metal only, and there is no field for it on the other two
        /// variants** -- the flag is refused with `--nested` and `--headless`
        /// at parse time, naming the reason
        /// ([`crate::backlight::BACKLIGHT_NEEDS_THE_PANEL`]), on `blank_idle`'s
        /// posture and for a reason that is sharper: a nested run's write would
        /// *succeed*, on a panel this core does not own.
        ///
        /// `None` is the ordinary session: both keys keep reaching the focused
        /// realm's shim seat, exactly as they have since WS-E.4.3.
        backlight: Option<backlight::BacklightConfig>,
        /// `--keymap PATH`: the compiled xkb keymap this session resolves
        /// libinput scancodes through (WS-E.3.1, D-028).
        ///
        /// A path rather than a compiled keymap because this enum is
        /// `PartialEq` and printed in parse tests; the file is read, audited
        /// and compiled at the top of `backend::drm::run_inner`, before the
        /// listener accepts anyone, so a bad one is still a startup failure.
        ///
        /// `None` is a legal, degraded session: only the layout-invariant
        /// scancode table resolves, so every core chord fires and no letter
        /// reaches any app. It is refused in combination with
        /// `--lock-passphrase-file`, because a passphrase nobody can type is a
        /// lock nobody can answer — the same reason `--headless` refuses that
        /// flag outright.
        keymap: Option<PathBuf>,
    },
    Help,
    Version,
    /// Read a passphrase from stdin and print one `--lock-passphrase-file`
    /// line, then exit (WS-E.2.2, issue #214).
    ///
    /// Print-and-exit like [`Action::Help`], and mode-independent for the same
    /// reason [`Action::PrintIsolation`] is: it answers a question about a file
    /// format, not about a session, so it must answer identically whether or
    /// not this machine can host one.
    ///
    /// It exists because a file format nobody can produce is a feature nobody
    /// can use — an operator would hand-assemble the line, get the parameter
    /// encoding subtly wrong, and discover it at the moment they are locked
    /// out. The generator and the parser being one module is what makes that
    /// checkable (`crate::lock::passphrase`).
    ///
    /// **Stdin, never argv.** `/proc/<pid>/cmdline` is world-readable.
    LockHash,
    /// Probe this kernel's confinement facilities and exit (P2.6.1, #185).
    ///
    /// Print-and-exit like [`Action::Help`] and [`Action::Version`], and
    /// deliberately **mode-independent**: it must answer identically whether
    /// or not this machine can host a session, because its whole purpose is to
    /// report what a machine *would* grant. That is also why it carries no
    /// fields and takes no companion refusal -- there is no configuration for
    /// it to disagree with.
    PrintIsolation,
    /// Print the confinement mechanisms *this build* applies and exit
    /// (P2.6.2, #186, D-036(6)).
    ///
    /// A **separate verb from [`Action::PrintIsolation`]**, and the
    /// separation is load-bearing rather than tidy. A tier is a measurement
    /// of the machine; a floor is a property of the binary. Printing the
    /// floor among the kernel rows would contradict the isolation module's
    /// own first rule and would invalidate #185's four-kernel matrix over a
    /// row that is not a kernel fact.
    PrintFloor,
    /// `--print-seccomp`: print the seccomp deny-list table this build
    /// compiles, then exit (P2.6.4, #188).
    ///
    /// A **third** print verb, and the split from [`Action::PrintFloor`] is
    /// the same one again at a finer grain. `--print-floor` says *that* this
    /// build applies a seccomp filter and refuses to start without one; this
    /// says *what the filter denies* -- one line per row, each carrying the
    /// PRD Doc 2 §15 escape class it closes and the errno it returns.
    ///
    /// It exists because a deny-list's contents are the claim. "This build
    /// applies seccomp" is a sentence a reader will complete as
    /// "syscall-confined"; the table is what makes the real, narrower
    /// statement checkable, and `tests/integration/test_real_seccomp.py`
    /// reads this output to learn the row set -- which is what makes "a row
    /// added without a case fails the test" true without a second
    /// hand-maintained list.
    PrintSeccomp,
}

/// The session's confinement selection and the helper that implements it
/// (P2.6.2, #186).
///
/// Bundled rather than two loose fields on three variants, matching how
/// `lock`, `screenshot` and `status` are already carried: the two values are
/// only meaningful together, since `--realm-init` names the binary
/// `--isolation=default` execs and is inert at `off`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IsolationOptions {
    isolation: spawn::isolation::Isolation,
    /// `--landlock`: the cap on the ABI rung the helper may build (P2.6.3,
    /// #187). Inert at `--isolation=off`, where no helper runs at all.
    landlock: vitrin_realm_init::LandlockRequest,
    /// `None` means "a sibling `vitrin-realm-init` beside this `vitrind`",
    /// resolved at startup exactly like `--shim`'s default.
    realm_init: Option<PathBuf>,
}

impl Default for IsolationOptions {
    /// **Omitting `--isolation` confines.**
    ///
    /// This is a hand-written `Default` on the *options struct* and
    /// deliberately not a `Default` on
    /// [`spawn::isolation::Isolation`] itself, which has none: the point of
    /// that absence is that an unconfined session must be unreachable
    /// without an operator typing the word, and a `Default` on the enum would
    /// make `Isolation::default()` a spelling anybody could reach for. Here
    /// the value is named in full, once, in the safe direction.
    ///
    /// The cost is real and is the decided one (D-020(6)): on a machine whose
    /// kernel will not grant namespaces, a `vitrind` that used to start now
    /// refuses. Between "refuses loudly" and "silently confines nothing",
    /// this project picks the former, and `--isolation=off` is the one-word
    /// answer for an operator who means it.
    fn default() -> Self {
        IsolationOptions {
            isolation: spawn::isolation::Isolation::Default,
            landlock: vitrin_realm_init::LandlockRequest::Highest,
            realm_init: None,
        }
    }
}

/// The presentation mode selected on the command line, before it is paired
/// with any `--size` into an [`Action`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Mode {
    Nested,
    Headless,
    /// Bare metal: this process owns the display controller (WS-E.3.2, issue
    /// #218).
    ///
    /// `#[cfg(feature = "drm-backend")]`, so a build without that backend
    /// cannot even **name** `--drm`: `parse_args` has no arm for it and exits
    /// with ``unknown argument `--drm` ``. That is the
    /// `--consent-injector-fd` precedent, applied to a deployment feature
    /// rather than a test one, and it is why the refusal table below is
    /// cfg'd alongside it.
    #[cfg(feature = "drm-backend")]
    Drm,
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
    // `Option` rather than the selector itself, so "not given" and "given
    // `default`" stay distinguishable -- which is what lets a repeated flag be
    // refused, on `set_consent`'s precedent.
    let mut isolation_mode: Option<spawn::isolation::Isolation> = None;
    let mut landlock_mode: Option<vitrin_realm_init::LandlockRequest> = None;
    let mut realm_init: Option<PathBuf> = None;
    let mut capture_dump: Option<PathBuf> = None;
    let mut chord: Option<deadman::Chord> = None;
    let mut hold_ms: Option<u64> = None;
    let mut attention_chord: Option<attention::AttentionChord> = None;
    let mut clipboard_key: Option<chord::Trigger> = None;
    let mut lock_chord: Option<chord::ModChord> = None;
    let mut screenshot_dir: Option<PathBuf> = None;
    let mut screenshot_chord: Option<chord::ModChord> = None;
    let mut lock_idle: Option<Duration> = None;
    let mut blank_idle: Option<Duration> = None;
    let mut lock_passphrase: Option<PathBuf> = None;
    // `Option` rather than the policy itself, so "not given" and "given
    // `never`" stay distinguishable: the refusal below is about a flag being
    // *stored and never read*, and the default value is not a use of the flag.
    let mut lock_on_seat_change: Option<lock::SeatChangePolicy> = None;
    let mut agent_cursor = false;
    #[cfg(feature = "drm-backend")]
    let mut keymap: Option<PathBuf> = None;
    let mut status_enabled = false;
    // The opt-in half is a bare `bool` and the device half an `Option`, on
    // `--status` / `--status-height`'s precedent: a switch repeated says the
    // same thing twice, where a valued flag's second value would have to win or
    // lose silently.
    let mut backlight_enabled = false;
    let mut backlight_device: Option<backlight::DeviceName> = None;
    let mut status_height: Option<u32> = None;
    let mut status_offset: Option<status::UtcOffset> = None;
    #[cfg(feature = "consent-injector")]
    let mut consent_injector_fd: Option<std::os::fd::RawFd> = None;
    #[cfg(feature = "physical-input-injector")]
    let mut physical_input_fd: Option<std::os::fd::RawFd> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg {
            "--nested" => set_mode(&mut mode, Mode::Nested)?,
            "--headless" => set_mode(&mut mode, Mode::Headless)?,
            #[cfg(feature = "drm-backend")]
            "--drm" => set_mode(&mut mode, Mode::Drm)?,
            #[cfg(feature = "drm-backend")]
            "--keymap" => {
                let value = args.next().ok_or(
                    "`--keymap` requires a path to a compiled xkb keymap \
                     (e.g. `--keymap /etc/vitrin/keymap.xkb`)",
                )?;
                set_path(&mut keymap, "--keymap", "keymap path", value)?;
            }
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
            "--lock-chord" => {
                let value = args.next().ok_or(
                    "`--lock-chord` requires a chord (e.g. `--lock-chord ctrl+alt+delete`)",
                )?;
                set_lock_chord(&mut lock_chord, value)?;
            }
            "--screenshot-dir" => {
                let value = args.next().ok_or(
                    "`--screenshot-dir` requires a directory path (e.g. \
                     `--screenshot-dir /home/you/Pictures/vitrin`)",
                )?;
                set_path(
                    &mut screenshot_dir,
                    "--screenshot-dir",
                    "directory path",
                    value,
                )?;
            }
            "--screenshot-chord" => {
                let value = args.next().ok_or(
                    "`--screenshot-chord` requires a chord (e.g. `--screenshot-chord ctrl+print`)",
                )?;
                set_mod_chord(
                    &mut screenshot_chord,
                    "--screenshot-chord",
                    screenshot::DEFAULT_SCREENSHOT_CHORD,
                    value,
                )?;
            }
            "--lock-idle" => {
                let value = args.next().ok_or(
                    "`--lock-idle` requires a whole number of seconds (e.g. `--lock-idle 300`)",
                )?;
                set_lock_idle(&mut lock_idle, value)?;
            }
            "--blank-idle" => {
                let value = args.next().ok_or(
                    "`--blank-idle` requires a whole number of seconds (e.g. `--blank-idle 300`)",
                )?;
                set_blank_idle(&mut blank_idle, value)?;
            }
            "--lock-on-seat-change" => {
                let value = args.next().ok_or(
                    "`--lock-on-seat-change` requires a policy (immediate, idle or never, e.g. \
                     `--lock-on-seat-change immediate`)",
                )?;
                set_seat_change_policy(&mut lock_on_seat_change, value)?;
            }
            "--lock-passphrase-file" => {
                let value = args.next().ok_or(
                    "`--lock-passphrase-file` requires a path (e.g. \
                     `--lock-passphrase-file ~/.config/vitrin/lock.hash`)",
                )?;
                set_path(
                    &mut lock_passphrase,
                    "--lock-passphrase-file",
                    "passphrase file path",
                    value,
                )?;
            }
            // Idempotent rather than "given more than once" -- a boolean
            // switch repeated says the same thing twice, unlike a valued flag
            // where the second value would have to win or lose silently.
            "--agent-cursor" => agent_cursor = true,
            // Idempotent for the same reason `--agent-cursor` is.
            "--status" => status_enabled = true,
            // Idempotent for the same reason again (D-041, issue #303).
            "--backlight" => backlight_enabled = true,
            "--backlight-device" => {
                let value = args.next().ok_or(
                    "`--backlight-device` requires a device name                      (e.g. `--backlight-device intel_backlight`)",
                )?;
                set_backlight_device(&mut backlight_device, value)?;
            }
            "--status-height" => {
                let value = args.next().ok_or(
                    "`--status-height` requires a number of rows (e.g. `--status-height 20`)",
                )?;
                set_status_height(&mut status_height, value)?;
            }
            "--status-utc-offset" => {
                let value = args.next().ok_or(
                    "`--status-utc-offset` requires `UTC` or a signed offset \
                     (e.g. `--status-utc-offset +09:00`)",
                )?;
                if status_offset.is_some() {
                    return Err("`--status-utc-offset` given more than once".into());
                }
                status_offset = Some(status::UtcOffset::parse(value).map_err(str::to_string)?);
            }
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
            "--isolation" => {
                let value = args
                    .next()
                    .ok_or("`--isolation` requires a mode (`default` or `off`)")?;
                set_isolation(
                    &mut isolation_mode,
                    spawn::isolation::Isolation::parse(value)?,
                )?;
            }
            "--landlock" => {
                let value = args
                    .next()
                    .ok_or("`--landlock` requires a mode (`highest`, `off`, or `abi:N`)")?;
                set_landlock(
                    &mut landlock_mode,
                    vitrin_realm_init::LandlockRequest::parse(value)?,
                )?;
            }
            "--realm-init" => {
                let value = args.next().ok_or(
                    "`--realm-init` requires a helper binary path \
                     (e.g. `--realm-init /usr/lib/vitrin/vitrin-realm-init`)",
                )?;
                set_path(&mut realm_init, "--realm-init", "realm-init path", value)?;
            }
            "--help" | "-h" => return Ok(Action::Help),
            "--version" | "-V" => return Ok(Action::Version),
            // Returns immediately, on the `--help`/`--version` precedent, so
            // it wins over mode selection and over any later parse error. A
            // probe of what this kernel offers must not be answerable only on
            // a command line that would otherwise have run.
            "--print-isolation" => return Ok(Action::PrintIsolation),
            // Same precedent again: it answers a question about this binary,
            // so it must not depend on a command line that would otherwise
            // have run.
            "--print-floor" => return Ok(Action::PrintFloor),
            // Same posture as the two verbs above: a question about this
            // binary, answered before anything else on the command line can
            // fail to parse.
            "--print-seccomp" => return Ok(Action::PrintSeccomp),
            // Returns immediately, on the `--print-isolation` precedent above:
            // it answers a question about a file format rather than about a
            // session, so it must not depend on a command line that would
            // otherwise have run.
            "--lock-hash" => return Ok(Action::LockHash),
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
                } else if let Some(value) = other.strip_prefix("--isolation=") {
                    set_isolation(
                        &mut isolation_mode,
                        spawn::isolation::Isolation::parse(value)?,
                    )?;
                } else if let Some(value) = other.strip_prefix("--landlock=") {
                    set_landlock(
                        &mut landlock_mode,
                        vitrin_realm_init::LandlockRequest::parse(value)?,
                    )?;
                } else if let Some(value) = other.strip_prefix("--realm-init=") {
                    set_path(&mut realm_init, "--realm-init", "realm-init path", value)?;
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
                } else if let Some(value) = other.strip_prefix("--screenshot-dir=") {
                    set_path(
                        &mut screenshot_dir,
                        "--screenshot-dir",
                        "directory path",
                        value,
                    )?;
                } else if let Some(value) = other.strip_prefix("--screenshot-chord=") {
                    set_mod_chord(
                        &mut screenshot_chord,
                        "--screenshot-chord",
                        screenshot::DEFAULT_SCREENSHOT_CHORD,
                        value,
                    )?;
                } else if let Some(value) = other.strip_prefix("--backlight-device=") {
                    set_backlight_device(&mut backlight_device, value)?;
                } else if let Some(value) = other.strip_prefix("--lock-chord=") {
                    set_lock_chord(&mut lock_chord, value)?;
                } else if let Some(value) = other.strip_prefix("--lock-idle=") {
                    set_lock_idle(&mut lock_idle, value)?;
                } else if let Some(value) = other.strip_prefix("--blank-idle=") {
                    set_blank_idle(&mut blank_idle, value)?;
                } else if let Some(value) = other.strip_prefix("--lock-on-seat-change=") {
                    set_seat_change_policy(&mut lock_on_seat_change, value)?;
                } else if let Some(value) = other.strip_prefix("--lock-passphrase-file=") {
                    set_path(
                        &mut lock_passphrase,
                        "--lock-passphrase-file",
                        "passphrase file path",
                        value,
                    )?;
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

    // Recorded before defaulting, because the headless refusal below asks what
    // the OPERATOR passed and the default would answer for them. The screenshot
    // chord is recorded here for the same reason one paragraph further down.
    let lock_chord_given = lock_chord.is_some();
    let screenshot_chord_given = screenshot_chord.is_some();
    let lock_chord = match lock_chord {
        Some(chord) => chord,
        None => chord::ModChord::parse(lock::DEFAULT_LOCK_CHORD)
            .expect("the default lock chord is in the vocabulary"),
    };
    // **The lock chord may not share its trigger with any other core-owned
    // chord**, and this is the fourth chord in the stack, so the check is
    // written once over a list rather than three times over pairs.
    //
    // Reachable, unlike the dead-man/attention pair: `chord::Trigger`'s
    // vocabulary and `deadman::Chord`'s overlap on every editing and function
    // key, so `--dead-man-chord delete` on an otherwise default command line is
    // a collision an operator can type today. The consequence is the one
    // WS-E.2.1's own check names: the dead-man watcher detects in the router's
    // UNCONDITIONAL observe tap, so a chord sharing its key would arm the
    // human's off-switch every time they locked their screen, and no hook
    // ordering can prevent it because nothing may blind that tap.
    //
    // The clipboard comparison is trigger-to-trigger and therefore *stricter*
    // than it strictly needs to be: `ctrl+alt+delete` and `ctrl+shift+delete`
    // are different gestures `ChordMatcher` would tell apart perfectly well.
    // It is deliberate anyway — an operator reading `--lock-chord` and
    // `--clipboard-key` should not have to reason about modifier-set equality
    // to know whether their two keys collide, and the cost of the stricter rule
    // is one more spelling they cannot use.
    //
    // **Skipped entirely under `--headless`**, where the lock cannot exist: the
    // chord above was DEFAULTED, not asked for, so comparing it would make
    // `--headless --dead-man-chord delete` exit non-zero citing a
    // `--lock-chord` the operator never passed, for a lock that backend cannot
    // raise. The headless block below refuses the flags that *were* passed,
    // which is the honest half of this.
    for (keysym, flag, name) in if matches!(mode, Some(Mode::Headless)) {
        Vec::new()
    } else {
        vec![
            (
                dead_man.chord.keysym(),
                "--dead-man-chord",
                dead_man.chord.name(),
            ),
            (attention.keysym(), "--attention-chord", attention.name()),
            (clipboard.keysym(), "--clipboard-key", clipboard.name()),
        ]
    } {
        if lock_chord.trigger_keysym() == keysym {
            return Err(format!(
                "`--lock-chord` `{}` uses the same key as `{flag}` `{name}`: the core's \
                 physical chords must not share a key. The dead-man watcher detects in the \
                 router's UNCONDITIONAL observe tap, so a chord sharing its key would arm the \
                 human's off-switch every time they used it -- and no hook ordering can \
                 prevent that, because nothing is allowed to blind that tap",
                lock_chord.spelling()
            ));
        }
    }

    // **The fifth core-owned chord** (WS-E.2.4, issue #216). Same rule, same
    // reason, one list longer: the dead-man watcher detects in the router's
    // UNCONDITIONAL observe tap, so a chord sharing its key would arm the
    // human's off-switch every time they took a screenshot, and no hook
    // ordering can prevent that because nothing may blind that tap.
    //
    // Compared trigger-to-trigger, like `--lock-chord`'s check above and
    // deliberately stricter than `ChordMatcher` needs, for that check's reason:
    // an operator reading two flags should not have to reason about
    // modifier-set equality to know whether their keys collide.
    //
    // **Not skipped under `--headless`**, unlike the lock's -- and the
    // difference is real rather than an oversight. The lock cannot exist on
    // that backend at all, so its DEFAULTED chord would name a flag the
    // operator never passed; the screenshot key exists in both modes (a
    // `physical-input-injector` build really presses it), so its chord is a
    // thing this session owns and a collision is a thing this session has.
    let screenshot_chord = screenshot_chord.unwrap_or_else(|| {
        chord::ModChord::parse(screenshot::DEFAULT_SCREENSHOT_CHORD)
            .expect("the default screenshot chord is in the vocabulary")
    });
    {
        let mut owned = vec![
            (
                dead_man.chord.keysym(),
                "--dead-man-chord",
                dead_man.chord.name().to_string(),
            ),
            (
                attention.keysym(),
                "--attention-chord",
                attention.name().to_string(),
            ),
            (
                clipboard.keysym(),
                "--clipboard-key",
                clipboard.name().to_string(),
            ),
        ];
        if !matches!(mode, Some(Mode::Headless)) {
            owned.push((
                lock_chord.trigger_keysym(),
                "--lock-chord",
                lock_chord.spelling(),
            ));
        }
        for (keysym, flag, name) in owned {
            if screenshot_chord.trigger_keysym() == keysym {
                return Err(format!(
                    "`--screenshot-chord` `{}` uses the same key as `{flag}` `{name}`: the \
                     core's physical chords must not share a key. The dead-man watcher \
                     detects in the router's UNCONDITIONAL observe tap, so a chord sharing \
                     its key would arm the human's off-switch every time they used it -- \
                     and no hook ordering can prevent that, because nothing is allowed to \
                     blind that tap",
                    screenshot_chord.spelling()
                ));
            }
        }
    }
    // **The VT escape reserves `f1`..`f12` from every other core chord, on
    // bare metal only** (WS-E.3.5, D-031).
    //
    // Same rule and same reason as the four refusals above, one list longer:
    // the dead-man watcher detects in the router's UNCONDITIONAL observe tap,
    // so a chord sharing a key with `Ctrl-Alt-F<n>` would arm the human's
    // off-switch every single time they left the VT, and no hook ordering can
    // prevent it because nothing may blind that tap.
    //
    // **Scoped to `--drm`**, on the lock chord's `--headless` precedent: the
    // VT escape does not exist on the other two backends -- only a process
    // holding DRM master can implement the chord -- so `--dead-man-chord f5
    // --headless` must keep working. `the_reservation_does_not_reach_the_other_backends`
    // is the control that stops this passing as a blanket refusal.
    //
    // The cost is published: `deadman::Chord`'s usable vocabulary drops from
    // 19 keys to 7 on bare metal. Nothing collides on a default command line
    // -- the shipped defaults are `esc`, `super`, `insert`, `ctrl+print` and
    // `ctrl+alt+delete`, with no F-key among them.
    #[cfg(feature = "drm-backend")]
    if matches!(mode, Some(Mode::Drm)) {
        let reserved = vt::reserved_triggers();
        for (keysym, flag, name) in [
            (
                dead_man.chord.keysym(),
                "--dead-man-chord",
                dead_man.chord.name().to_string(),
            ),
            // Unreachable today -- `AttentionChord`'s vocabulary is `super`
            // and `rsuper` only -- and listed anyway, for the defence-in-depth
            // reason the dead-man/attention pair above is: a vocabulary that
            // grows must not silently disarm the escape.
            (
                attention.keysym(),
                "--attention-chord",
                attention.name().to_string(),
            ),
            (
                clipboard.keysym(),
                "--clipboard-key",
                clipboard.name().to_string(),
            ),
            (
                lock_chord.trigger_keysym(),
                "--lock-chord",
                lock_chord.spelling(),
            ),
            (
                screenshot_chord.trigger_keysym(),
                "--screenshot-chord",
                screenshot_chord.spelling(),
            ),
        ] {
            if let Some(taken) = reserved.iter().find(|t| t.keysym() == keysym) {
                return Err(format!(
                    "`{flag}` `{name}` uses `{}`, which `--drm` reserves for the VT escape \
                     `Ctrl-Alt-{}`: on bare metal this core implements Ctrl-Alt-F1..F12 \
                     itself, because once it holds the display the kernel stops handling \
                     them and a session that did not implement them would be one you cannot \
                     leave. The dead-man watcher detects in the router's UNCONDITIONAL \
                     observe tap, so a chord sharing that key would arm the human's \
                     off-switch every time they left the VT, and no hook ordering can \
                     prevent it. Pick another key, or run this session `--nested` / \
                     `--headless`, where there is no VT to escape from",
                    taken.name(),
                    taken.name().to_uppercase()
                ));
            }
        }
    }

    // **A configured gesture with nowhere to write is a key that silently does
    // nothing**, which is the fail-open configuration trap `Chord::parse`
    // refuses for an undeliverable key and `realm.rs`'s loader refuses for an
    // unauditable program. `--screenshot-dir` alone is fine (the chord
    // defaults); `--screenshot-chord` alone is an operator who believes they
    // configured a screenshot key.
    if screenshot_dir.is_none() && screenshot_chord_given {
        return Err(
            "`--screenshot-chord` without `--screenshot-dir`: the chord would be consumed \
             and write nothing, which is a key that silently does not work. Pass \
             `--screenshot-dir PATH` as well, or drop the chord"
                .into(),
        );
    }
    let screenshot = screenshot::ScreenshotConfig {
        dir: screenshot_dir,
        chord: screenshot_chord,
    };

    // **The lock is nested-only, and both halves of that are refused
    // separately because they have different reasons** (WS-E.2.2, issue #214).
    //
    // The passphrase first, so its message wins when both flags are given: it
    // names the *keymap*, which is the fact a reader most needs (a headless
    // backend has neither a host to interpret a layout nor a keymap of its
    // own, so its alphabet has no letters in it -- see `crate::lock`). The other two flags are refused for a different and
    // blunter reason: a headless session has no physical input device at all,
    // so a lock it raised could never be dismissed. Both are startup errors
    // with a non-zero exit, on `HEADLESS_INTERACTIVE_REFUSAL`'s precedent,
    // because a session that comes up unanswerably locked -- or holding a
    // passphrase nobody can type -- is the fail-open configuration trap
    // `realm.rs` and `deadman.rs` both refuse.
    //
    // **All three `--lock-*` flags, not two.** `--lock-chord` was parsed and
    // silently discarded under `--headless` while nine surfaces -- this help
    // text, the refusal constants' own wording, four doc comments,
    // `tests/integration/README.md` and `shim/docs/nested-lock-screen.md` --
    // all said every `--lock-*` flag is refused there. Silently accepting a
    // flag that cannot do anything is the same class of trap as the two below,
    // just quieter. Ordering is deliberate: the passphrase message wins when
    // several are given, because the keymap is the sharper reason.
    if matches!(mode, Some(Mode::Headless)) {
        if lock_passphrase.is_some() {
            return Err(lock::PASSPHRASE_NEEDS_A_KEYMAP.into());
        }
        if lock_idle.is_some() || lock_chord_given {
            return Err(LOCK_NEEDS_PHYSICAL_INPUT.into());
        }
    }

    // **`--blank-idle` is `--drm` only** (WS-E.4.3, issue #223), refused on
    // BOTH other backends at parse time rather than accepted as a silent no-op
    // -- the `--agent-cursor` and `--size` posture, taken here for a reason
    // that is sharper on headless than on either of those: a headless session
    // would not merely ignore the flag, it would go dark after the timeout and
    // never come back, because that backend's hook stack carries no lock gate
    // and so nothing at all writes the activity clock a blank is woken by.
    #[cfg(feature = "drm-backend")]
    let blank_has_an_output = matches!(mode, Some(Mode::Drm));
    // A build without the backend has no `--drm` arm at all, so there is no
    // mode this flag could be valid in.
    #[cfg(not(feature = "drm-backend"))]
    let blank_has_an_output = false;
    if !blank_has_an_output && blank_idle.is_some() {
        return Err(crate::backend::blank::BLANK_NEEDS_THE_OUTPUT.into());
    }

    // **`--lock-on-seat-change` is `--drm` only** (issue #246), and the same
    // refusal for the same reason one step milder: a nested or headless session
    // never receives a seat event, so `LockScreen::set_seat_absent` is never
    // called and the policy would be stored and never read. The predicate is
    // the one above rather than a second `matches!`, because "this run drives
    // the panel itself" and "this run holds the seat" are the same fact on
    // this stack -- the bare-metal backend takes both together or neither --
    // and two spellings of one fact are two things that can disagree.
    let has_a_seat = blank_has_an_output;
    if !has_a_seat && lock_on_seat_change.is_some() {
        return Err(lock::SEAT_POLICY_NEEDS_A_SEAT.into());
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

    // **The bare-metal refusal table** (WS-E.3.2, issue #218), at parse time
    // like every other one here, and each entry for its own named reason
    // rather than by analogy.
    #[cfg(feature = "drm-backend")]
    if matches!(mode, Some(Mode::Drm)) {
        // `--agent-cursor`, on `--nested`'s reason exactly: this backend is a
        // human's actual display, which is the situation D-019 exists for, so
        // the sprite has no opt-in and the flag would silently do nothing.
        if agent_cursor {
            return Err(
                "`--agent-cursor` is only valid with `--headless`: a bare-metal session is a \
                 human's display and composites the agent cursor with no opt-in, so the flag \
                 would do nothing there."
                    .into(),
            );
        }
        // The lock's passphrase, refused for `--headless`'s reason applied to
        // a different cause. Headless is refused because it has no keymap at
        // all; a `--drm` session has one only if `--keymap` gave it one, and
        // without it the alphabet is `invariant_keysym`'s -- Escape, Enter,
        // the arrows, the modifiers -- which contains no letter and no digit.
        // A passphrase file nobody can type is a lock nobody can answer.
        if lock_passphrase.is_some() && keymap.is_none() {
            return Err(PASSPHRASE_NEEDS_A_DRM_KEYMAP.into());
        }
        // **Auto-approve is refused outright.** `--headless` refuses
        // *interactive* because it can raise no prompt; this is the inverse
        // and it is the sharper of the two. This backend IS the human's
        // display and there is a human at a real keyboard, so a card can
        // always be drawn and answered -- and auto-approving every petition
        // on the machine that is somebody's actual desktop is the fail-open
        // posture this repo refuses everywhere else. It is not a warning: a
        // session that grants silently cannot be un-granted by noticing.
        if !matches!(consent, ConsentPolicy::Interactive) {
            return Err(DRM_NEEDS_INTERACTIVE_CONSENT.into());
        }
    }

    // `--keymap` names the keymap the *libinput* path resolves through, and
    // only the bare-metal backend has one. Nested gets its keysyms from the
    // host's already-interpreted `logical_key` (issue #118) and headless has
    // no keyboard at all, so in either mode the flag would be read, stored and
    // never consulted -- `--agent-cursor`'s posture, and `--size`'s before it.
    #[cfg(feature = "drm-backend")]
    if keymap.is_some() && !matches!(mode, Some(Mode::Drm)) {
        return Err(
            "`--keymap` requires `--drm`: only the bare-metal backend resolves scancodes \
             itself. A nested session takes its keysyms from the host compositor's own \
             interpretation and a headless session has no keyboard, so the keymap would be \
             compiled and never consulted."
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

    // The two valued status flags are refused without `--status` rather than
    // silently stored: a command line that set a height for a strip that will
    // never be drawn is a command line whose author believed something false,
    // and this is `--agent-cursor`'s posture (refuse, do not no-op) applied to
    // a pair of values instead of a mode.
    if !status_enabled && (status_height.is_some() || status_offset.is_some()) {
        return Err(
            "`--status-height` and `--status-utc-offset` need `--status`: without it no strip is \
             drawn and the value would be stored and never used."
                .into(),
        );
    }
    // `--backlight-device` without `--backlight` is refused for the reason
    // above, one notch sharper: it would name a panel this session has decided
    // not to touch, so the operator's command line states a preference nothing
    // will ever read.
    if !backlight_enabled && backlight_device.is_some() {
        return Err(
            "`--backlight-device` needs `--backlight`: without it the brightness keys are \
             delivered to the app and no device is ever written, so the name would be stored \
             and never used."
                .into(),
        );
    }
    let backlight = backlight_enabled.then_some(backlight::BacklightConfig {
        device: backlight_device,
    });

    // **`--backlight` is `--drm` only** (D-041, issue #303), refused on both
    // other backends at parse time on `--blank-idle`'s predicate -- "this run
    // drives the panel itself" is the same fact for both flags.
    //
    // The reason is sharper here than for `--blank-idle`, and it is not that
    // the write would fail: it would *succeed*. A nested `vitrind` writing
    // `/sys/class/backlight` would be changing the brightness of a screen it
    // does not own, while the host compositor acts on the same key -- two
    // actors on one panel with no arbitration, which is the exact failure
    // D-041's consume clause exists to prevent, arriving through the other
    // door. A headless run has no panel and no human looking at one.
    if !blank_has_an_output && backlight.is_some() {
        return Err(backlight::BACKLIGHT_NEEDS_THE_PANEL.into());
    }

    let status = status::StatusConfig {
        enabled: status_enabled,
        height: status_height.unwrap_or(status::DEFAULT_HEIGHT),
        utc_offset: status_offset.unwrap_or(status::UtcOffset::UTC),
    };

    let isolation = IsolationOptions {
        // The one place the CLI default is applied. `unwrap_or` and not
        // `unwrap_or_default`, so the safe value is spelled out at the site
        // that chooses it rather than hidden behind a trait.
        isolation: isolation_mode.unwrap_or(spawn::isolation::Isolation::Default),
        // Same rule, same reason: omitting `--landlock` asks for the highest
        // rung, and the weaker values have to be typed.
        landlock: landlock_mode.unwrap_or(vitrin_realm_init::LandlockRequest::Highest),
        realm_init,
    };

    // `--help`/`--version` already returned above, so only the run modes
    // remain to resolve; `--size` is meaningless without `--headless`.
    match (mode, size) {
        (Some(Mode::Nested), None) => Ok(Action::RunNested {
            consent,
            principals,
            recorder,
            realm,
            shim,
            isolation,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            lock: lock::LockConfig {
                chord: lock_chord,
                idle: lock_idle,
                passphrase: lock_passphrase,
                // Not `lock_on_seat_change`: the flag was refused above on this
                // mode, so this is the default and saying so literally keeps
                // the nested arm from ever growing a seat policy by accident.
                on_seat_change: lock::SeatChangePolicy::default(),
            },
            screenshot,
            status,
        }),
        (Some(Mode::Nested), Some(_)) => Err("`--size` is only valid with `--headless`".into()),
        // `--size` names a *virtual* output's dimensions. On bare metal the
        // size is the CRTC's active mode, read once and handed to every
        // realm's shim at `configure`; a flag that contradicted it would
        // either be ignored or would configure every app at a geometry the
        // panel is not showing.
        #[cfg(feature = "drm-backend")]
        (Some(Mode::Drm), Some(_)) => Err("`--size` is only valid with `--headless`: a \
                                           bare-metal session composes at the display's own \
                                           mode."
            .into()),
        #[cfg(feature = "drm-backend")]
        (Some(Mode::Drm), None) => Ok(Action::RunDrm {
            consent,
            principals,
            recorder,
            realm,
            shim,
            isolation,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            lock: lock::LockConfig {
                chord: lock_chord,
                idle: lock_idle,
                passphrase: lock_passphrase,
                // The one arm that reads the flag, because it is the one
                // backend with a seat to lose (issue #246).
                on_seat_change: lock_on_seat_change.unwrap_or_default(),
            },
            screenshot,
            status,
            blank_idle,
            backlight,
            keymap,
        }),
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
            isolation,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            agent_cursor,
            #[cfg(feature = "consent-injector")]
            consent_injector_fd,
            #[cfg(feature = "physical-input-injector")]
            physical_input_fd,
            screenshot,
            status,
        }),
        (None, Some(_)) => Err("`--size` requires `--headless`".into()),
        (None, None) => Err(format!("no mode given (expected one of {MODE_FLAGS})")),
    }
}

/// The `--lock-passphrase-file` refusal under `--drm` **without** a keymap
/// (WS-E.3.2).
///
/// A third reason, beside [`crate::lock::PASSPHRASE_NEEDS_A_KEYMAP`]'s and
/// [`LOCK_NEEDS_PHYSICAL_INPUT`]'s, and it is deliberately a separate constant
/// because the fix is different: headless can never take this flag, and a
/// bare-metal session can — by passing `--keymap`. A message telling the
/// operator their backend is wrong, when what is missing is one more flag,
/// would send them the wrong way.
#[cfg(feature = "drm-backend")]
const PASSPHRASE_NEEDS_A_DRM_KEYMAP: &str =
    "`--lock-passphrase-file` needs `--keymap` under `--drm`: without a keymap this session \
     resolves only the layout-invariant scancode table, which has no letters, no digits and no \
     punctuation in it -- so the passphrase could never be typed and the lock could never be \
     answered. Pass `--keymap PATH`, or drop the passphrase file for a privacy-screen lock any \
     keypress dismisses.";

/// The `--drm --consent=auto-approve` refusal (WS-E.3.2, issue #218 decision
/// 5).
///
/// [`HEADLESS_INTERACTIVE_REFUSAL`]'s inverse, and stated separately because
/// the reasoning runs the other way: that one refuses interactive consent on a
/// backend that cannot raise a prompt, this one refuses *silent* consent on
/// the one backend that always can.
#[cfg(feature = "drm-backend")]
const DRM_NEEDS_INTERACTIVE_CONSENT: &str =
    "`--drm` cannot serve `--consent=auto-approve`: this backend IS the human's display and \
     there is a human at a real keyboard, so every petition can be shown on a consent card and \
     answered. Granting silently on the machine that is somebody's actual desktop is the \
     fail-open posture `--headless`'s own refusal exists to prevent, taken in the other \
     direction. Use `--consent=interactive` (the default), or `--headless` for an unattended \
     run.";

/// The `--headless --consent=interactive` refusal, in one place so the
/// deployment build's message and the instrumented build's extended one
/// cannot drift in their shared sentence (issue #138). A plain build emits
/// exactly this and nothing more.
const HEADLESS_INTERACTIVE_REFUSAL: &str =
    "`--headless` cannot serve `--consent=interactive`: a headless core has no display to draw \
     the consent prompt and no physical input device to answer it, so every petition would pend \
     until it timed out. Use `--consent=auto-approve` for a headless run, or `--nested` to \
     answer prompts on screen.";

/// The `--lock-idle` / `--lock-chord` refusal under `--headless` (WS-E.2.2).
///
/// A different reason from [`crate::lock::PASSPHRASE_NEEDS_A_KEYMAP`] and
/// therefore a different message: that one is about the *alphabet*, this one is
/// about there being no input device at all. The idle raise is the dangerous
/// half — it fires on a timer with no input needed, so a headless session with
/// `--lock-idle` really would lock itself and then have nothing that could
/// dismiss it.
const LOCK_NEEDS_PHYSICAL_INPUT: &str =
    "the `--lock-*` flags require `--nested`: a headless session has no physical input device \
     at all (`SeatInput::physical` is private to the input module and this backend calls no \
     intake), so a lock it raised could never be answered and the session would wedge with \
     every key going nowhere. Run `--nested`, where a human is at the keyboard.";

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

/// Record the `--lock-chord` gesture (WS-E.2.2), rejecting a repeat flag, a
/// malformed spelling, and any key this build's input intake could not deliver.
///
/// The `--clipboard-key` precedent, one step up the vocabulary: this flag takes
/// a whole modifier chord rather than a bare key, so the error names the
/// *shape* as well as the accepted trigger list. The collision check against the
/// other three core-owned chords lives in `parse_args`, where every half is
/// resolved.
fn set_lock_chord(slot: &mut Option<chord::ModChord>, value: &str) -> Result<(), String> {
    set_mod_chord(slot, "--lock-chord", "ctrl+alt+delete", value)
}

/// Record a modifier-chord flag, rejecting a repeat and anything
/// [`chord::ModChord::parse`] will not take.
///
/// One helper for `--lock-chord` (WS-E.2.2) and `--screenshot-chord`
/// (WS-E.2.4): both take the same grammar from the same matcher, and two
/// copies of the error text would drift into telling an operator two different
/// vocabularies for one parser. `example` is the flag's own default, so the
/// message shows the shape of the thing that flag actually configures.
fn set_mod_chord(
    slot: &mut Option<chord::ModChord>,
    flag: &str,
    example: &str,
    value: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("`{flag}` given more than once"));
    }
    let parsed = chord::ModChord::parse(value).map_err(|err| {
        let accepted: Vec<&str> = chord::Trigger::vocabulary().collect();
        format!(
            "`{flag}` `{value}`: {err} (expected MOD[+MOD...]+KEY, e.g. \
             `{example}`; modifiers: ctrl, shift, alt, super; keys: {})",
            accepted.join(", ")
        )
    })?;
    *slot = Some(parsed);
    Ok(())
}

/// Record the `--lock-idle` timeout in whole seconds (WS-E.2.2), rejecting a
/// repeat flag and a zero.
///
/// **Zero is refused rather than read as "off"**, and that is the fail-closed
/// reading rather than pedantry: `--lock-idle 0` would lock the session on the
/// first dispatch round and every round after an unlock, which is a wedge
/// dressed as a configuration. Omitting the flag is how an operator says "no
/// idle lock", and there is exactly one spelling of it.
fn set_lock_idle(slot: &mut Option<Duration>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--lock-idle` given more than once".into());
    }
    let secs: u64 = value
        .parse()
        .map_err(|_| format!("`--lock-idle` `{value}` is not a whole number of seconds"))?;
    if secs == 0 {
        return Err(
            "`--lock-idle 0` would raise the lock on the first dispatch round and again after \
             every unlock. Omit the flag to run with no idle lock."
                .into(),
        );
    }
    *slot = Some(Duration::from_secs(secs));
    Ok(())
}

/// Record the `--blank-idle` timeout in whole seconds (WS-E.4.3, issue #223),
/// rejecting a repeat flag and a zero.
///
/// [`set_lock_idle`]'s shape and its zero refusal, for the same fail-closed
/// reason one step worse: `--blank-idle 0` would power the panel down on the
/// first dispatch round and again on the round after every wake, which is a
/// session whose display flickers off faster than a human can read it. Omitting
/// the flag is how an operator says "never blank", and there is exactly one
/// spelling of it.
fn set_blank_idle(slot: &mut Option<Duration>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--blank-idle` given more than once".into());
    }
    let secs: u64 = value
        .parse()
        .map_err(|_| format!("`--blank-idle` `{value}` is not a whole number of seconds"))?;
    if secs == 0 {
        return Err(
            "`--blank-idle 0` would power the panel down on the first dispatch round and again \
             after every wake. Omit the flag to run with no idle blank."
                .into(),
        );
    }
    *slot = Some(Duration::from_secs(secs));
    Ok(())
}

/// Record `--lock-on-seat-change` (issue #246), rejecting a repeat flag and an
/// unknown policy.
///
/// [`set_blank_idle`]'s shape, and the repeat refusal matters more here than on
/// a duration: two policies on one command line have no reading in which the
/// operator got what they asked for, and silently taking the last one would
/// hand a session a lock policy nobody chose — which is the whole complaint
/// this flag exists to answer.
fn set_seat_change_policy(
    slot: &mut Option<lock::SeatChangePolicy>,
    value: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--lock-on-seat-change` given more than once".into());
    }
    *slot = Some(lock::SeatChangePolicy::parse(value)?);
    Ok(())
}

/// Record `--status-height` (WS-E.2.3), rejecting a repeat flag and anything
/// outside the range the type size needs.
///
/// Clamped at parse time rather than at draw time, on `--lock-idle`'s
/// precedent: a session that came up with a strip too short to hold a digit
/// would be drawing a clipped number on a surface a human is trained to read as
/// authoritative, and a clipped digit is a wrong digit.
fn set_status_height(slot: &mut Option<u32>, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--status-height` given more than once".into());
    }
    let rows: u32 = value
        .parse()
        .map_err(|_| format!("`--status-height` `{value}` is not a whole number of rows"))?;
    if !(status::MIN_HEIGHT..=status::MAX_HEIGHT).contains(&rows) {
        return Err(status::HEIGHT_REFUSAL.into());
    }
    *slot = Some(rows);
    Ok(())
}

/// Record `--backlight-device` (D-041, issue #303), rejecting a repeat flag and
/// anything that is not an immediate child of `/sys/class/backlight`.
///
/// **Validated here rather than at first use**, on this parser's standing rule.
/// The *device* is a different question and is deliberately not asked here: a
/// name that is not a name is a configuration error the operator can fix from
/// the message, while a device that is missing or unwritable is a fact about
/// the machine at this instant, and refusing to start a display server over it
/// would be a far worse failure than the key doing nothing
/// ([`backlight::Backlight::probe`]).
fn set_backlight_device(
    slot: &mut Option<backlight::DeviceName>,
    value: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err("`--backlight-device` given more than once".into());
    }
    *slot = Some(backlight::DeviceName::parse(value)?);
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

/// Record the selected confinement, rejecting a repeat flag.
///
/// [`set_consent`]'s shape, for the same reason: a command line that names a
/// security-relevant mode twice is a command line whose author disagrees with
/// themselves, and quietly taking the last one is how a session ends up
/// confining less than the operator believes.
/// `--landlock`, on `set_isolation`'s rule: given twice is a refusal, never a
/// last-one-wins. Two spellings of the confinement on one command line is a
/// command line whose author disagreed with themselves, and picking either
/// one silently is how a session ends up weaker than the operator believes.
fn set_landlock(
    slot: &mut Option<vitrin_realm_init::LandlockRequest>,
    landlock: vitrin_realm_init::LandlockRequest,
) -> Result<(), String> {
    match slot {
        None => {
            *slot = Some(landlock);
            Ok(())
        }
        Some(_) => Err("`--landlock` given more than once".into()),
    }
}

fn set_isolation(
    slot: &mut Option<spawn::isolation::Isolation>,
    isolation: spawn::isolation::Isolation,
) -> Result<(), String> {
    match slot {
        None => {
            *slot = Some(isolation);
            Ok(())
        }
        Some(_) => Err("`--isolation` given more than once".into()),
    }
}

/// Record the selected [`Mode`], rejecting a second (or conflicting) mode flag.
/// The mode flags this build accepts, named in one place so the
/// "more than one mode" and "no mode given" messages cannot disagree with each
/// other or with what `parse_args` actually has an arm for.
#[cfg(not(feature = "drm-backend"))]
const MODE_FLAGS: &str = "`--nested`, `--headless`";
#[cfg(feature = "drm-backend")]
const MODE_FLAGS: &str = "`--nested`, `--headless`, `--drm`";

fn set_mode(slot: &mut Option<Mode>, mode: Mode) -> Result<(), String> {
    match slot {
        None => {
            *slot = Some(mode);
            Ok(())
        }
        Some(_) => Err(format!(
            "more than one mode given (expected one of {MODE_FLAGS})"
        )),
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
            #[cfg(feature = "drm-backend")]
            print!("{DRM_USAGE}");
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("vitrind {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Action::LockHash => {
            // Upstream of `init_tracing`, exactly as `Help` and
            // `--print-isolation` are: the one line this prints is redirected
            // straight into a file, and a log line interleaved into stdout
            // would produce a passphrase file that does not parse.
            match lock_hash_from_stdin() {
                Ok(line) => {
                    println!("{line}");
                    ExitCode::SUCCESS
                }
                Err(msg) => {
                    eprintln!("vitrind: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        Action::PrintIsolation => {
            // Deliberately upstream of `init_tracing`, exactly as `Help` and
            // `Version` are: this output is byte-compared against a checked-in
            // matrix, and tracing interleaved into stdout would make the
            // comparison depend on the log level.
            print!("{}", spawn::isolation::Report::probe().render());
            ExitCode::SUCCESS
        }
        Action::PrintFloor => {
            // Upstream of `init_tracing` on the same reasoning, and printed
            // by its own verb rather than as extra rows of
            // `--print-isolation`: that output is a kernel measurement and
            // this one is a build constant.
            print!("{}", spawn::isolation::render_floor());
            ExitCode::SUCCESS
        }
        Action::PrintSeccomp => {
            // Upstream of `init_tracing` for the same reason, and a build
            // constant like `--print-floor`: it prints the table this binary
            // would install, never what any kernel did with it.
            print!("{}", vitrin_realm_init::seccomp::render_table());
            ExitCode::SUCCESS
        }
        Action::RunNested {
            consent,
            principals,
            recorder,
            realm,
            shim,
            isolation,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            lock,
            screenshot,
            status,
        } => {
            init_tracing();
            let screenshot_chord = screenshot.chord;
            run_session(
                consent,
                principals,
                recorder,
                realm,
                shim,
                capture_dump,
                screenshot.dir,
                // `--backlight` is refused outside `--drm` at parse time, so
                // this arm can only ever pass `None`. Spelled at the call
                // rather than defaulted, so a later mode gaining the flag is a
                // compile error here instead of a silent omission.
                None,
                isolation,
                // `--consent-injector-fd` is refused with `--nested` at parse
                // time (issue #138), so a nested run is never instrumented.
                false,
                move |seed| {
                    backend::winit::run(
                        dead_man,
                        attention,
                        clipboard,
                        screenshot_chord,
                        lock,
                        status,
                        seed,
                    )
                },
            )
        }
        #[cfg(feature = "drm-backend")]
        Action::RunDrm {
            consent,
            principals,
            recorder,
            realm,
            shim,
            isolation,
            capture_dump,
            dead_man,
            attention,
            clipboard,
            lock,
            screenshot,
            status,
            blank_idle,
            backlight,
            keymap,
        } => {
            init_tracing();
            let screenshot_chord = screenshot.chord;
            tracing::warn!(
                "starting on BARE METAL: this process takes DRM master and libinput's devices \
                 for the whole seat. The realm's app runs as this uid, and at \
                 `--isolation=off` with no namespace, no Landlock ruleset and no seccomp \
                 filter at all (docs/book/src/limits.md). On a real seat logind ACLs \
                 /dev/input/event* to the logged-in user, so an app that can reach those \
                 device nodes reads everything typed, bypassing this core entirely. At \
                 `--isolation=default` the realm's own /dev is an enumerated tmpfs with no \
                 /dev/input in it and the Landlock ruleset grants no path outside the realm's \
                 view -- that hole is not created here; it becomes reachable here"
            );
            run_session(
                consent,
                principals,
                recorder,
                realm,
                shim,
                capture_dump,
                screenshot.dir,
                // The one arm that carries it: `--backlight` is `--drm` only.
                backlight,
                isolation,
                // Both injector channels are refused with `--drm` at parse
                // time, so a bare-metal run is never instrumented.
                false,
                move |seed| {
                    backend::drm::run(
                        dead_man,
                        attention,
                        clipboard,
                        screenshot_chord,
                        lock,
                        status,
                        blank_idle,
                        keymap,
                        seed,
                    )
                },
            )
        }
        Action::RunHeadless {
            size,
            consent,
            principals,
            recorder,
            realm,
            shim,
            isolation,
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
            screenshot,
            status,
        } => {
            init_tracing();
            let screenshot_chord = screenshot.chord;
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
                screenshot.dir,
                // `--backlight` is refused outside `--drm` at parse time, so
                // this arm can only ever pass `None`. Spelled at the call
                // rather than defaulted, so a later mode gaining the flag is a
                // compile error here instead of a silent omission.
                None,
                isolation,
                instrumented,
                move |seed| {
                    backend::headless::run(
                        size,
                        dead_man,
                        attention,
                        clipboard,
                        screenshot_chord,
                        agent_cursor,
                        status,
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

/// Read one passphrase from stdin and derive its `--lock-passphrase-file` line
/// (WS-E.2.2, issue #214).
///
/// One line, with a single trailing newline stripped (so `printf` and `echo`
/// both work) and **nothing else trimmed** — a passphrase may legitimately
/// begin or end with a space, and a generator that silently trimmed one would
/// produce a file the lock screen can never be unlocked against.
///
/// An empty passphrase is refused: it would hash fine and unlock on a bare
/// Enter, which is the privacy-screen behaviour an operator gets by **omitting**
/// `--lock-passphrase-file`. Two ways to spell the same thing, one of which
/// looks like authentication, is exactly the confusion this refusal removes.
///
/// The bytes are wiped before returning. Best-effort, for the reason
/// [`lock::passphrase::wipe`] states.
fn lock_hash_from_stdin() -> Result<String, String> {
    use std::io::Read;

    let mut buf = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buf)
        .map_err(|e| format!("could not read the passphrase from stdin: {e}"))?;
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.is_empty() {
        lock::passphrase::wipe(&mut buf);
        return Err(
            "the passphrase read from stdin is empty. Omit `--lock-passphrase-file` to run an \
             unauthenticated privacy screen; do not configure one that unlocks on a bare Enter."
                .into(),
        );
    }
    // **Refuse anything the lock screen could never accept back.** Hashing it
    // happily would produce a valid-looking file that locks a session with no
    // way into it -- the fail-open configuration trap `realm.rs` and
    // `deadman.rs` both refuse, arrived at from the other end. Two ways it
    // happens, both silent until the human is already locked out:
    //
    //  * longer than `MAX_ATTEMPT_BYTES`, which is the cap the unlock buffer
    //    stops accepting keystrokes at, so the typed attempt is truncated and
    //    can never hash to this;
    //  * a control character (an embedded tab or newline from a pasted or
    //    heredoc'd passphrase), which the unlock path has no key sequence for
    //    at all -- `invariant_keysym` cannot deliver one.
    if buf.len() > lock::passphrase::MAX_ATTEMPT_BYTES {
        let len = buf.len();
        lock::passphrase::wipe(&mut buf);
        return Err(format!(
            "the passphrase is {len} bytes and the unlock buffer stops at {}. It would hash \
             fine here and could never be typed back, locking the session with no way in.",
            lock::passphrase::MAX_ATTEMPT_BYTES
        ));
    }
    if let Some(bad) = buf.iter().find(|b| b.is_ascii_control()) {
        let bad = *bad;
        lock::passphrase::wipe(&mut buf);
        return Err(format!(
            "the passphrase contains a control byte (0x{bad:02x}) the unlock screen has no way \
             to type: no keymap resolves a control byte and `invariant_keysym` cannot deliver it. \
             Accepting it here would lock the session with no way in."
        ));
    }
    let line = lock::passphrase::hash_line(&buf).map_err(|e| format!("could not hash it: {e}"));
    lock::passphrase::wipe(&mut buf);
    line
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
// Nine parameters, two over clippy's default: the run's six configured paths,
// the consent policy, the instrumented-session marker (issue #138) and the
// backend closure. Bundling them into a struct would hide the one thing this
// signature is good for -- every startup input the session has is named here,
// in the order `run_session`'s doc comment explains them.
#[allow(clippy::too_many_arguments)]
fn run_session<R>(
    consent: ConsentPolicy,
    principals_path: Option<PathBuf>,
    recorder_path: Option<PathBuf>,
    realm_path: Option<PathBuf>,
    shim_path: Option<PathBuf>,
    capture_dump: Option<PathBuf>,
    screenshot_dir: Option<PathBuf>,
    backlight: Option<backlight::BacklightConfig>,
    isolation: IsolationOptions,
    consent_injector: bool,
    backend: R,
) -> ExitCode
where
    R: FnOnce(session::RuntimeSeed) -> (Recorder, Result<(), Box<dyn std::error::Error>>),
{
    // ---------------------------------------------------------------------
    // The isolation floor (P2.6.2, #186, D-036(6)). THE FIRST STATEMENT.
    // ---------------------------------------------------------------------
    //
    // Before `block_loop_signals`, before the consent banner (which spawns a
    // thread), and before the listener binds. Two reasons, both hard:
    // `Report::probe` forks and requires a single-threaded caller, and a
    // refusal must leave no stray artifact behind -- which is the same rule
    // the listener already follows by binding before the recorder opens.
    //
    // **Accepted cost, stated rather than discovered: a floor refusal is not
    // journaled.** The recorder does not exist yet and must not, because
    // creating it would be exactly the artifact this ordering exists to
    // avoid. That is consistent with every other pre-recorder refusal in this
    // function.
    let (isolation_applied, isolation_report) = match admit_isolation(&isolation) {
        Ok(pair) => pair,
        Err(()) => return ExitCode::FAILURE,
    };
    // The standing warning for an unconfined session -- a third instance of
    // the `AutoApproveBanner`/`InjectorBanner` shape, still not generalised
    // (three is where a pattern becomes a refactor, and the three differ in
    // what they say and when they stop). Held for the session's whole life:
    // a one-line startup notice scrolls off, and a session whose realms are
    // not confined at all should keep saying so.
    let _unconfined_banner = (isolation_applied.isolation() == spawn::isolation::Isolation::Off)
        .then(|| UnconfinedBanner::start(UNCONFINED_BANNER_INTERVAL));

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
    // The floor as one line, composed here so the recorder never has to know
    // what a `Mechanism` is.
    let isolation_floor = spawn::isolation::FLOOR
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("+");
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
        // Three separate answers on purpose (P2.6.2, #186): what was chosen,
        // what this build requires, what this machine can do. A log that
        // carried only the first could not tell a reader whether a session
        // ran unconfined because the operator asked or because the machine
        // could not do better.
        isolation: &isolation_applied.isolation().to_string(),
        isolation_floor: &isolation_floor,
        isolation_ceiling: &isolation_report.tier().to_string(),
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

    // The confinement inputs, resolved here for the same reason the shim is:
    // a core with no usable helper should learn it up front rather than at
    // the fork. Everything in here is a **core** input -- the realm names its
    // app, the core decides what confines it.
    let confinement = match isolation_applied.isolation() {
        spawn::isolation::Isolation::Off => None,
        spawn::isolation::Isolation::Default => {
            let realm_init = match isolation.realm_init.clone() {
                Some(path) => path,
                None => match default_realm_init_path() {
                    Ok(path) => path,
                    Err(err) => {
                        tracing::error!(
                            "fatal: cannot locate the default realm confinement helper: {err}; \
                             pass an explicit `--realm-init PATH`"
                        );
                        return ExitCode::FAILURE;
                    }
                },
            };
            let storage_root = match realm_storage_root() {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!("fatal: {err}");
                    return ExitCode::FAILURE;
                }
            };
            // The canary set: absolute host paths that must be unreachable
            // from inside every realm, checked on every single spawn rather
            // than once in CI. Two of them are this core's own artifacts --
            // the socket that *is* the control plane and the log that records
            // every decision -- and the third is the operator's home, which
            // is what "confined" means to a human.
            let mut canaries = vec![
                match vitrin_ipc::paths::core_socket_path() {
                    Ok(sock) => sock,
                    Err(err) => {
                        tracing::error!("fatal: cannot name this session's core socket: {err}");
                        return ExitCode::FAILURE;
                    }
                },
                path.clone(),
            ];
            if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
                canaries.push(PathBuf::from(home));
            }
            let render_nodes = enumerate_render_nodes();
            tracing::info!(
                realm_init = %realm_init.display(),
                storage = %storage_root.display(),
                canaries = canaries.len(),
                render_nodes = render_nodes.len(),
                "realm confinement helper (audited at spawn; its version must match this \
                 core's exactly). Realms' private storage lives under `storage` and has NO \
                 QUOTA -- it is disk-backed, realm-writable and unbounded"
            );
            Some(spawn::Confinement {
                realm_init,
                storage_root,
                canaries,
                render_nodes,
                tmpfs: vitrin_realm_init::TmpfsCaps::DEFAULT,
                landlock: isolation_applied.landlock(),
            })
        }
    };

    // The screenshot directory (WS-E.2.4, issue #216): opened and audited
    // ONCE, here, before the listener below accepts anyone -- so no client is
    // running when the path is resolved, and every later write is an `openat`
    // relative to the descriptor this returns rather than a second walk of a
    // name someone could have re-pointed in between.
    //
    // A refusal is fatal and names the reason. A screenshot key writing
    // somewhere the operator did not ask for is the failure the whole audit
    // exists to prevent, and "start anyway with the key disabled" would be a
    // session that silently does not do what its command line says.
    // One refusal before the directory is even opened, and it is a
    // confinement refusal rather than a screenshot one: a `--screenshot-dir`
    // *inside* a realm's private storage would be bound READ-WRITE into that
    // realm, so the realm could read -- and rewrite -- every screenshot this
    // session takes of every other realm. `limits.md` narrows the screenshot
    // residual to "same-uid processes outside every realm"; that sentence is
    // only true if this path cannot be placed inside one.
    if let Some((path, root)) = screenshot_dir
        .as_ref()
        .zip(confinement.as_ref().map(|c| &c.storage_root))
    {
        // Compared on the paths as given plus their canonical forms, because
        // neither may exist yet and `canonicalize` on a missing path answers
        // nothing. Both spellings have to miss for the path to be accepted.
        let candidates = [
            (path.clone(), root.clone()),
            (
                std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()),
                std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()),
            ),
        ];
        if candidates.iter().any(|(p, r)| p.starts_with(r)) {
            tracing::error!(
                "fatal: `--screenshot-dir {}` is inside the realms' private storage ({}), which \
                 is bound read-write into the realm it belongs to. A realm would be able to \
                 read and rewrite every screenshot of every other realm. Choose a directory \
                 outside {}",
                path.display(),
                root.display(),
                root.display()
            );
            return ExitCode::FAILURE;
        }
    }

    let screenshot_writer = match screenshot_dir {
        Some(path) => match screenshot::ScreenshotDir::open(&path) {
            // The descriptor is handed straight to its worker thread and this
            // thread never holds it again (issue #240): the encode that used to
            // stall the compositor for 71.7 ms per press happens there. A
            // session that cannot spawn that thread refuses to start, on the
            // same reasoning the audit failure below does -- a screenshot key
            // that silently does nothing is the failure this whole section
            // exists to prevent.
            Ok(dir) => match screenshot::ScreenshotWriter::spawn(dir) {
                Ok(writer) => {
                    tracing::info!(
                        dir = %writer.path().display(),
                        "screenshot key armed: it writes the REALM VIEW -- no trusted band, no \
                         consent prompt, no lock screen, no status strip, no agent cursor. The \
                         band's colour is this session's secret and the confined app can read \
                         any file this core writes (see docs/book/src/limits.md)"
                    );
                    Some(writer)
                }
                Err(err) => {
                    tracing::error!(
                        "fatal: `--screenshot-dir {}`: cannot start the screenshot writer \
                         thread: {err}",
                        path.display()
                    );
                    return ExitCode::FAILURE;
                }
            },
            Err(err) => {
                tracing::error!("fatal: `--screenshot-dir {}`: {err}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    // **The backlight, probed once and never refused** (D-041, issue #303).
    //
    // Unlike `--screenshot-dir` above, nothing here can be fatal, and that is
    // the decision rather than an omission: a screenshot key with nowhere to
    // write would silently drop the human's evidence, where a brightness key
    // with nothing to write to merely does what it did in every checkout before
    // D-041. The operator is told which device was chosen -- the auto-pick is
    // sorted-first and can be the wrong panel on a laptop with both
    // `acpi_video0` and `intel_backlight` -- and told loudly when there is
    // nothing to write, because "the brightness key works" is a property of
    // this machine's udev rules rather than of this checkout.
    let backlight = backlight.map(|config| {
        let light = backlight::Backlight::new(config.device);
        match light.probe() {
            backlight::Probe::Ready {
                device,
                max,
                current,
            } => tracing::info!(
                device,
                max,
                current,
                step_percent = backlight::STEP_PERCENT,
                floor_percent = backlight::FLOOR_PERCENT,
                "brightness keys armed: XF86MonBrightnessUp/Down are CONSUMED by this core \
                 and no longer reach any app. They write /sys/class/backlight and nothing \
                 else -- they do not blank, they do nothing for an external display, and \
                 there is a floor below which this core will not drive the panel"
            ),
            backlight::Probe::Absent(reason) => tracing::warn!(
                reason = reason.label(),
                root = backlight::SYSFS_ROOT,
                "brightness keys armed but there is nothing to write: the keys are CONSUMED \
                 and will do nothing. `not_writable` usually means this uid is not in the \
                 `video` group and no udev rule tagged the seat; `no_device` means this \
                 machine has no internal panel this core can see. Drop `--backlight` to give \
                 the keys back to the app"
            ),
        }
        light
    });

    let seed = session::RuntimeSeed {
        listener,
        verifier,
        petitions: petitions::PetitionRegistry::new(consent, petitions::PetitionConfig::default()),
        grants: grants::GrantTable::new(),
        realms,
        recorder,
        shim,
        confinement,
        indicator,
        capture_dump,
        screenshot_writer,
        backlight,
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
/// Measure this kernel, relate it to what this build applies, and either
/// return the admitted selection or emit the refusal (D-036(6)).
///
/// Four branches, and the copy differs on all four because the operator's
/// next move does:
///
/// - **A, `off`.** Accepted on every machine, in every mode. It asks for
///   nothing, so nothing can be missing. Warned about loudly, here and every
///   60 seconds after.
/// - **B, `default` admitted.** The floor is met. If the machine will not
///   reach `Tier::IntraUser`, this is also where the *forecast* is emitted --
///   the build that still works announcing the build that will not.
/// - **C, a mechanism could not be measured.** Refused. Treating "unknown" as
///   "fine" is the silent degradation D-020(6) forbids.
/// - **D, a mechanism was measured and is missing.** Refused, with the remedy
///   derived from the sysctls actually read.
fn admit_isolation(
    options: &IsolationOptions,
) -> Result<(spawn::isolation::Applied, spawn::isolation::Report), ()> {
    let report = spawn::isolation::Report::probe();
    match spawn::isolation::admit(options.isolation, options.landlock, &report) {
        Ok(applied) if applied.isolation() == spawn::isolation::Isolation::Off => {
            warn_unconfined();
            Ok((applied, report))
        }
        Ok(applied) => {
            tracing::info!(
                isolation = %applied.isolation(),
                landlock = %applied.landlock(),
                kernel = %report.kernel_release,
                "realms will be confined: each gets its own user, mount, PID, IPC, UTS and \
                 network namespace, an identity uid/gid map, zero capabilities, a Landlock \
                 ruleset enforced before the shim's execve, and a seccomp-bpf DENY-LIST \
                 installed immediately after it. The deny-list is a named-class claim and not \
                 a completeness one: `vitrind --print-seccomp` prints every row it closes, and \
                 the rest of the kernel's syscall surface is unenumerated -- a realm is \
                 filtered against a named list, NOT syscall-confined. No `applied_profile` is \
                 printed here on purpose: it names the rung a realm OBTAINED, and no realm \
                 exists yet -- the ladder's landing is per-spawn. Whatever it says, it is not \
                 a tier name either, because `intra-user` means namespaces PLUS Landlock PLUS \
                 seccomp and a profile string names only the Landlock rung"
            );
            // **Loud, and at WARN**, because every one of these is a session
            // that will journal something weaker than a reader of the default
            // would assume -- and the number is in the message rather than
            // only in a per-realm entry, so it is in the first screen of log
            // an operator sees rather than buried in a spawn.
            match applied.landlock() {
                vitrin_realm_init::LandlockRequest::Highest => {}
                vitrin_realm_init::LandlockRequest::Off => tracing::warn!(
                    "LANDLOCK IS OFF (--landlock=off): realms this session spawns get NO \
                     filesystem ruleset. They are confined by the mount table alone, so a \
                     write inside the realm's own view is bounded by what was mounted and by \
                     nothing else -- no TRUNCATE denial, no per-path read set. Every realm \
                     journals `applied_profile=namespaces-only` so no entry from this session \
                     can be read as a confined one. This exists as the per-rung gates' \
                     positive control, not as an operating mode."
                ),
                vitrin_realm_init::LandlockRequest::CappedAt(rung) => {
                    // `0` for a kernel that would not answer at all, which
                    // is not a rung: the comparison below then reads "the cap
                    // changes nothing", and it is right -- a machine whose
                    // ABI query failed has already been refused by `admit`
                    // unless `--landlock=off` was given, and this arm is not
                    // that one.
                    let kernel = report.landlock_abi.unwrap_or_default();
                    // A cap **below the build's own floor** is the sharpest
                    // case and gets its own line: the flag is the one way to
                    // reach a rung this build refuses to start a kernel at, so
                    // a session running there is running below the confinement
                    // this build declares. It is still permitted, because the
                    // per-rung measurements are what the flag exists for --
                    // but it is never passed over in silence.
                    if rung < vitrin_realm_init::LANDLOCK_MIN_ABI {
                        tracing::warn!(
                            requested_rung = rung,
                            floor = vitrin_realm_init::LANDLOCK_MIN_ABI,
                            kernel_abi = kernel,
                            "LANDLOCK IS PINNED BELOW THIS BUILD'S OWN FLOOR \
                             (--landlock=abi:{rung}, floor abi:{}). A kernel that reported \
                             rung {rung} would be REFUSED at startup; this flag reaches the \
                             same domain by choice. It is a measurement instrument, not a \
                             hardening option, and no confinement claim this build publishes \
                             applies to the realms this session spawns.",
                            vitrin_realm_init::LANDLOCK_MIN_ABI
                        );
                    }
                    if kernel > rung {
                        tracing::warn!(
                            requested_rung = rung,
                            kernel_abi = kernel,
                            "LANDLOCK IS PINNED BELOW THIS KERNEL (--landlock=abi:{rung}): this \
                             kernel offers ABI {kernel} and this session will build its realms' \
                             rulesets at rung {rung}, reproducing an ABI-{rung} kernel -- \
                             INCLUDING that kernel's strictness. What moves between rung \
                             {rung} and rung {kernel}, where it applies: 2 REFER, which goes \
                             the OTHER way (a domain that does not handle REFER forbids \
                             rename/link across directories entirely, so a rung-1 session is \
                             STRICTER here and breaks apps that write by rename-into-place); \
                             3 TRUNCATE (without it a read-granted file can still be \
                             destroyed); 5 IOCTL_DEV; 6 abstract-socket and signal scoping; 9 \
                             RESOLVE_UNIX. The cap is a measurement instrument; it is not a \
                             hardening option."
                        );
                    } else {
                        tracing::warn!(
                            requested_rung = rung,
                            kernel_abi = kernel,
                            "`--landlock=abi:{rung}` was given and this kernel offers no more \
                             than rung {kernel}, so the cap changes nothing. Said out loud \
                             rather than passed over: a cap that did nothing is a measurement \
                             that proves nothing."
                        );
                    }
                }
            }
            if let Some(forecast) = spawn::isolation::forecast(&report) {
                tracing::warn!("{forecast}");
            }
            Ok((applied, report))
        }
        Err(refusal) => {
            tracing::error!(
                "fatal: {refusal} Pass `--isolation=off` to start an UNCONFINED session \
                 anyway, or -- for a Landlock refusal specifically -- `--landlock=off` to \
                 start a session whose realms have namespaces and no ruleset. Both are \
                 weaker than what was asked for and both say so in every journal entry. \
                 `vitrind --print-isolation` shows every row behind this answer and \
                 `vitrind --print-floor` what this build requires."
            );
            Err(())
        }
    }
}

/// The realm's GPU access, enumerated and audited **once, at startup**,
/// before any client is running -- the same posture `--screenshot-dir` takes.
///
/// Render nodes only, never `card*` or `controlD*`: a render node grants
/// rendering and buffer allocation, a primary node grants mode setting and
/// therefore the display itself.
///
/// **The cost is real and is taken deliberately.** Omitting the node
/// altogether gives the smallest mount table and the strongest claim -- and
/// silently disables the dmabuf zero-copy path plus every bit of GPU
/// acceleration inside every confined realm, which is an unadvertised
/// regression that hardware dogfooding would attribute to the wrong change.
/// So the node is bound, and the GPU ioctl surface and cross-realm
/// GPU-memory side channels it carries are real and unaddressed here.
fn enumerate_render_nodes() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    let mut nodes: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        // Audited before it is offered, and "audited" has to mean the same
        // thing here as it does for `command` and for a `binds` entry --
        // otherwise the word is doing work the code is not. An adversarial
        // review found it was: this was a file-type test through
        // `fs::metadata`, which *follows symlinks*, with no writer check at
        // all. Two things are fixed:
        //
        //   1. `symlink_metadata`, so a symlink planted at `renderD128` is
        //      refused rather than silently followed to whatever it names.
        //   2. The trusted-writer walk over every directory on the path.
        //      Whoever can write `/dev` or `/dev/dri` can put a node of their
        //      choosing where this core will find it, and the node is bound
        //      READ-WRITE into every confined realm.
        //
        // What is deliberately NOT checked is the node's own mode bits. A
        // render node is typically `crw-rw---- root:video`, i.e.
        // group-writable by design, and refusing that would refuse every GPU
        // on every ordinary desktop. The bits behind a device node are the
        // distro's statement about who may talk to the driver; the path is
        // ours to verify.
        .filter(|p| audit_render_node(p))
        .collect();
    // Sorted, so the mount table -- and therefore the child's mountinfo
    // fingerprint -- is the same on two runs of the same machine.
    nodes.sort();
    nodes
}

/// One `/dev/dri/renderD*` entry, against the same trusted-writer rule
/// `command` and every `binds` source get.
///
/// Returns `false` and says why rather than refusing the session: a machine
/// with one odd entry under `/dev/dri` should start without a GPU in its
/// realms, not fail to start. That asymmetry with `binds` is deliberate --
/// a bind source is something the operator asked for by name, and a render
/// node is something this core went looking for.
fn audit_render_node(path: &Path) -> bool {
    let euid = rustix::process::geteuid().as_raw();
    // `symlink_metadata`, never `metadata`: following a symlink here would
    // audit the target's type and then bind the link's name.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::warn!(node = %path.display(), "ignoring a /dev/dri entry: {err}");
            return false;
        }
    };
    if !std::os::unix::fs::FileTypeExt::is_char_device(&meta.file_type()) {
        tracing::warn!(
            node = %path.display(),
            "ignoring a /dev/dri entry that is not a character device (a symlink or a regular \
             file somebody planted must not reach the mount table as though it were a GPU)"
        );
        return false;
    }
    for dir in path.ancestors().skip(1) {
        let st = match rustix::fs::stat(dir) {
            Ok(st) => st,
            Err(err) => {
                tracing::warn!(
                    node = %path.display(),
                    "ignoring a /dev/dri entry: cannot stat {} ({err})",
                    dir.display()
                );
                return false;
            }
        };
        if let Some(fault) = realm::untrusted_writer(st.st_uid, st.st_mode, euid, true) {
            tracing::warn!(
                node = %path.display(),
                "ignoring a /dev/dri entry: directory {} is {fault}. This node would be bound \
                 READ-WRITE into every confined realm, so whoever can write a directory on its \
                 path chooses what the realms talk to",
                dir.display()
            );
            return false;
        }
    }
    true
}

/// A `vitrin-realm-init` beside this running `vitrind`, on exactly
/// [`default_shim_path`]'s reasoning and with one extra refusal.
///
/// The extra refusal is `current_exe`'s `" (deleted)"` spelling: on Linux the
/// kernel appends it when the running binary has been unlinked, and a path
/// built from that names a file that does not exist. It matters more here
/// than for the shim because the helper's version must match this core's
/// exactly, so "cannot find it" and "found the wrong one" would otherwise be
/// hard to tell apart at the point of failure.
fn default_realm_init_path() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate the running vitrind: {e}"))?;
    if exe.as_os_str().to_string_lossy().ends_with(" (deleted)") {
        return Err(format!(
            "the running vitrind ({}) has been unlinked, so no sibling path can be derived \
             from it; pass an explicit `--realm-init PATH`",
            exe.display()
        ));
    }
    let dir = exe
        .parent()
        .ok_or_else(|| format!("vitrind {} has no parent directory", exe.display()))?;
    Ok(dir.join("vitrin-realm-init"))
}

/// `$XDG_DATA_HOME/vitrin/realms`, the parent of every realm's private
/// storage.
///
/// Deliberately **not** under `$XDG_RUNTIME_DIR`: that tree is a tmpfs the
/// session manager clears at logout, and a realm's `HOME` that vanished
/// between sessions would be a home directory in name only.
fn realm_storage_root() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                "neither $XDG_DATA_HOME nor $HOME is set, so this core cannot name a place for \
                 realms' private storage"
                    .to_string()
            })?;
            PathBuf::from(home).join(".local/share")
        }
    };
    Ok(base.join("vitrin/realms"))
}

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

/// How often an unconfined session repeats its warning.
const UNCONFINED_BANNER_INTERVAL: Duration = AUTO_APPROVE_BANNER_INTERVAL;

/// The `--isolation=off` session's standing warning (P2.6.2, #186, D-036(6)).
///
/// A **third instance** of [`AutoApproveBanner`]'s repeating-warning shape,
/// and still deliberately not a generalisation of it. Three is usually where
/// a pattern earns a refactor; here the three differ in what they say, when
/// they stop, and -- for the injector -- whether they exist in the build at
/// all, so the shared part is thirty lines of thread plumbing and the
/// unshared part is the security claim. Rewriting the one mechanism in the
/// core that prints a standing security warning is not something to buy
/// inside a confinement change.
///
/// Unlike the injector's, this one ships in every build: `--isolation=off` is
/// a deployment flag, not a test hook.
struct UnconfinedBanner {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl UnconfinedBanner {
    fn start(interval: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("unconfined-banner".into())
            .spawn(move || {
                let (lock, cvar) = &*worker;
                let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*stopped {
                    let (guard, _timeout) = cvar
                        .wait_timeout(stopped, interval)
                        .unwrap_or_else(|e| e.into_inner());
                    stopped = guard;
                    if !*stopped {
                        warn_unconfined();
                    }
                }
            })
            .inspect_err(|err| {
                tracing::error!(
                    "this session is UNCONFINED but its repeating warning could not be started \
                     ({err}); the startup banner is the only warning this run will emit"
                );
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for UnconfinedBanner {
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

/// The unconfined-session warning text, in one place so the startup emission
/// and every repeat are literally the same message.
fn warn_unconfined() {
    tracing::warn!(
        "ISOLATION IS OFF (--isolation=off): every realm this session spawns runs with THIS \
         CORE'S OWN AUTHORITY -- no user, mount, PID, IPC, UTS or network namespace, the whole \
         filesystem visible, every socket on the machine reachable, and the operator's \
         supplementary groups (video, render, input, docker) intact. A realm's app can read \
         this core's socket, this run's flight-recorder log, and the operator's home \
         directory. This is the pre-2.6.2 spawn path, kept because the confinement gate needs \
         a positive control; it is not a rung of the default/hardened/paranoid dial and it is \
         never a deployed session."
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
///
/// **Colour is decided here rather than left to the subscriber's default**
/// (issue #251). See [`ansi_wanted`] for the whole rule and the reason.
fn init_tracing() {
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let ansi = ansi_wanted(
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
        std::env::var_os("NO_COLOR"),
    );
    diagnostic_subscriber(std::io::stderr, ansi, filter).init();
    log_build_identity();
}

/// Whether this run may write ANSI colour into its diagnostics.
///
/// # A redirected log is an artefact somebody has to grep (issue #251)
///
/// `tracing-subscriber` 0.3 decides colour from `cfg!(feature = "ansi")` and
/// `NO_COLOR` alone — **it never asks whether the writer is a terminal**. So
/// every run redirected to a file got SGR escapes interleaved into it, and the
/// bring-up runbook makes exactly such a file the only account of what
/// happened (`docs/drm-bringup.md`: "`tee` is not optional"). The escapes land
/// *between* a field's name and its `=`, so `grep 'connector='` over the second
/// DRM run's log matched **zero** lines while `grep connector` matched two:
/// the operator had to know to `sed 's/\x1b\[[0-9;]*m//g'` first, and a field
/// nobody can grep is a field the record paraphrases instead of quoting.
///
/// # Two switches, both one-way, so there is nothing for them to disagree about
///
/// `NO_COLOR` is the conventional opt-out and this function keeps honouring it,
/// which it has to do by hand: `with_ansi` *overrides* the subscriber's own
/// `NO_COLOR` check, so calling it at all makes this the only place that
/// decision is taken. Both inputs can only ever turn colour **off** — a tty
/// with `NO_COLOR` set is plain, and a pipe is plain whatever `NO_COLOR` says —
/// so the answer is their conjunction and neither "wins" over the other. There
/// is deliberately **no flag to force colour on**: the case a flag would serve
/// is a human watching a pipe, and a human watching a pipe is a human whose log
/// file is being written for later.
///
/// `no_color` is passed in rather than read here so the tests are hermetic —
/// mutating the environment mid-test is a data race the 2024 edition makes
/// `unsafe` for good reason, and this decision is worth testing without one.
fn ansi_wanted(stderr_is_terminal: bool, no_color: Option<std::ffi::OsString>) -> bool {
    // NO_COLOR's own rule: *set to a non-empty value* disables colour, so an
    // empty `NO_COLOR=` is not an opt-out (no-color.org, and the behaviour
    // tracing-subscriber implements internally).
    stderr_is_terminal && no_color.is_none_or(|value| value.is_empty())
}

/// The diagnostic subscriber, as one expression both `main` and the tests
/// build — so the test asserts on the formatter this binary actually installs
/// rather than on a replica of it.
fn diagnostic_subscriber<W>(
    writer: W,
    ansi: bool,
    filter: tracing_subscriber::EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(ansi)
        .finish()
}

/// **Say which binary this is, in the first lines of every run** (WS-E.3.5).
///
/// Zero risk and it is first on the list for a measured reason. The bare-metal
/// backend's first run on hardware pinned a core at 99% CPU and produced a
/// preserved log, and the log could not answer the one question that decides
/// what the measurement means: **was it a debug build?** The pixel loops on
/// that session's frame path cost ~6 ms per frame compiled `-O` and ~283 ms
/// compiled `-O0`, a factor of ~50, and the observed cadence was 383 ms per
/// frame. Cargo overwrites fingerprint directories in place, so the answer was
/// not recoverable from the filesystem afterwards either.
///
/// A performance report from a binary nobody can identify is not a measurement,
/// so the binary identifies itself. `debug_assertions` rather than a build
/// script constant, because it is the flag that actually governs whether the
/// hot loops were optimised.
fn log_build_identity() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        profile,
        exe = ?std::env::current_exe().ok(),
        "vitrind starting. A `debug` profile runs this core's CPU composite roughly 50x \
         slower than `release`; do not read a performance measurement from one"
    );
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
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                isolation: IsolationOptions::default(),
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
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                isolation: IsolationOptions::default(),
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
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
            })
        );
        assert_eq!(
            parse_args(["--headless", "--consent=auto-approve", "--size", "640x480"]),
            Ok(Action::RunHeadless {
                isolation: IsolationOptions::default(),
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
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
                })
            );
        }
        assert_eq!(
            parse_args(["--nested", "--consent=interactive"]),
            Ok(Action::RunNested {
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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

    /// **The bare-metal refusal table** (WS-E.3.2, issue #218), in the shape
    /// every other entry in this table is tested in: parse the command line,
    /// assert the refusal names both the flag and the way out.
    ///
    /// Each of these is a flag that would otherwise be *accepted and ignored*,
    /// which is how an operator comes to believe a session is configured
    /// differently than it is — the trap `--size` set the precedent for and
    /// `--agent-cursor` followed.
    #[cfg(feature = "drm-backend")]
    #[test]
    fn the_drm_refusal_table_refuses_at_parse_time_rather_than_ignoring() {
        // A plain bare-metal run parses, with interactive consent by default.
        assert!(parse_args(["--drm"]).is_ok());
        assert!(parse_args(["--drm", "--consent=interactive"]).is_ok());

        // `--size`: the mode's size is the panel's.
        let err = parse_args(["--drm", "--size", "640x480"]).expect_err("--size is headless-only");
        assert!(
            err.contains("--size") && err.contains("--headless"),
            "{err}"
        );

        // `--agent-cursor`: drawn with no opt-in where a human is watching.
        let err =
            parse_args(["--drm", "--agent-cursor"]).expect_err("--agent-cursor is headless-only");
        assert!(
            err.contains("--agent-cursor") && err.contains("--headless"),
            "{err}"
        );

        // `--consent=auto-approve`: the inverse of the headless refusal, and
        // the message must name the way out rather than only the conflict.
        let err = parse_args(["--drm", "--consent=auto-approve"])
            .expect_err("a bare-metal session must not grant silently");
        assert!(err.contains("auto-approve"), "{err}");
        assert!(err.contains("--consent=interactive"), "{err}");

        // `--keymap` belongs to this backend and to no other.
        for args in [
            vec!["--nested", "--keymap", "/etc/vitrin/keymap.xkb"],
            vec!["--headless", "--consent=auto-approve", "--keymap", "/k.xkb"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must refuse a keymap it cannot use"));
            assert!(err.contains("--keymap") && err.contains("--drm"), "{err}");
        }
        assert!(parse_args(["--drm", "--keymap", "/etc/vitrin/keymap.xkb"]).is_ok());

        // The passphrase needs an alphabet. Refused without `--keymap`, and
        // accepted with one -- so the refusal is about the keymap and not
        // about the backend.
        let err = parse_args(["--drm", "--lock-passphrase-file", "/etc/vitrin/pass"])
            .expect_err("a passphrase nobody can type is a lock nobody can answer");
        assert!(err.contains("--keymap"), "{err}");
        assert!(err.contains("--lock-passphrase-file"), "{err}");
        assert!(parse_args([
            "--drm",
            "--keymap",
            "/etc/vitrin/keymap.xkb",
            "--lock-passphrase-file",
            "/etc/vitrin/pass",
        ])
        .is_ok());

        // The other `--lock-*` flags are refused under `--headless` and
        // ACCEPTED here: a human is at a real keyboard, so a lock this
        // session raises can be dismissed.
        assert!(parse_args(["--drm", "--lock-idle", "300"]).is_ok());
        assert!(parse_args(["--drm", "--lock-chord", "ctrl+alt+delete"]).is_ok());

        // **`--blank-idle` is `--drm` only** (WS-E.4.3, issue #223), both
        // spellings, and refused everywhere else naming the reason.
        assert!(parse_args(["--drm", "--blank-idle", "300"]).is_ok());
        assert!(parse_args(["--drm", "--blank-idle=300"]).is_ok());
        assert_eq!(
            blank_of(&parse_args(["--drm", "--blank-idle", "300"]).unwrap()),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            blank_of(&parse_args(["--drm"]).unwrap()),
            None,
            "off by default: a core that guessed an idle timeout is a core that blanked \
             somebody's demo"
        );
        // Zero is refused rather than read as off, on `--lock-idle 0`'s
        // precedent and for a worse consequence: it would power the panel down
        // on the first dispatch round and again after every wake.
        let err = parse_args(["--drm", "--blank-idle", "0"]).expect_err("zero is a wedge");
        assert!(err.contains("--blank-idle 0"), "{err}");
        assert!(
            parse_args(["--drm", "--blank-idle", "300", "--blank-idle", "600"]).is_err(),
            "a repeat flag is an error, like every other one-shot flag here"
        );
        assert!(parse_args(["--drm", "--blank-idle", "abc"]).is_err());

        // **`--lock-on-seat-change` is `--drm` only** (issue #246), for the
        // same reason and by the same predicate: no other backend receives a
        // seat event, so the policy would be stored and never read.
        for policy in ["immediate", "idle", "never"] {
            assert!(
                parse_args(["--drm", "--lock-on-seat-change", policy]).is_ok(),
                "{policy} must parse on the one backend that has a seat"
            );
        }
        assert_eq!(
            seat_policy_of(&parse_args(["--drm", "--lock-on-seat-change=immediate"]).unwrap()),
            lock::SeatChangePolicy::Immediate
        );
        assert_eq!(
            seat_policy_of(&parse_args(["--drm", "--lock-on-seat-change", "idle"]).unwrap()),
            lock::SeatChangePolicy::Idle
        );
        assert_eq!(
            seat_policy_of(&parse_args(["--drm"]).unwrap()),
            lock::SeatChangePolicy::Never,
            "the default is D-030(2)'s behaviour: a switch away does not lock the session"
        );
        let err = parse_args(["--drm", "--lock-on-seat-change", "sometimes"])
            .expect_err("the vocabulary is closed");
        assert!(err.contains("immediate") && err.contains("never"), "{err}");
        assert!(
            parse_args([
                "--drm",
                "--lock-on-seat-change",
                "idle",
                "--lock-on-seat-change",
                "never"
            ])
            .is_err(),
            "two policies on one command line have no reading in which the operator got what \
             they asked for"
        );
        assert!(parse_args(["--drm", "--lock-on-seat-change"]).is_err());

        // ...and both injector channels stay headless-only, which the
        // existing guards already enforce -- asserted here so that "the
        // bare-metal table" is complete in one place.
        #[cfg(feature = "consent-injector")]
        {
            let err = parse_args(["--drm", "--consent-injector-fd", "7"])
                .expect_err("the consent channel is headless-only");
            assert!(err.contains("--headless"), "{err}");
        }
        #[cfg(feature = "physical-input-injector")]
        {
            let err = parse_args(["--drm", "--physical-input-fd", "7"])
                .expect_err("the physical-input channel is headless-only");
            assert!(err.contains("--headless"), "{err}");
        }

        // Two modes is still one error, and the message lists what this build
        // actually has an arm for.
        let err = parse_args(["--drm", "--nested"]).expect_err("two modes");
        assert!(
            err.contains("--drm"),
            "the mode list must name every mode: {err}"
        );
    }

    /// **The VT escape reserves `f1`..`f12` from every other core chord, on
    /// bare metal only** (WS-E.3.5, D-031).
    ///
    /// Five flags, one for each core-owned chord, because the consequence is
    /// the same for all five and it is not "two keys do two things": the
    /// dead-man watcher detects in the router's UNCONDITIONAL observe tap, so
    /// a chord sharing a key with `Ctrl-Alt-F<n>` arms the human's off-switch
    /// **every single time they leave the VT**, and no hook ordering can stop
    /// it because nothing may blind that tap.
    #[cfg(feature = "drm-backend")]
    #[test]
    fn the_vt_escape_reserves_the_function_keys_on_bare_metal() {
        for (args, flag) in [
            (vec!["--drm", "--dead-man-chord", "f5"], "--dead-man-chord"),
            (vec!["--drm", "--lock-chord", "ctrl+alt+f5"], "--lock-chord"),
            (vec!["--drm", "--clipboard-key", "f5"], "--clipboard-key"),
            (
                vec![
                    "--drm",
                    "--screenshot-chord",
                    "ctrl+f5",
                    "--screenshot-dir",
                    "/tmp/shots",
                ],
                "--screenshot-chord",
            ),
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must be refused: f5 is the VT escape's"));
            assert!(err.contains(flag), "the refusal must name the flag: {err}");
            assert!(
                err.contains("Ctrl-Alt-F5"),
                "the refusal must name the chord the key is taken by: {err}"
            );
            assert!(
                err.contains("observe tap"),
                "the WHY must be named, not just the collision: {err}"
            );
        }

        // Both ends of the range, so an off-by-one in the reservation is not
        // hidden by the middle.
        for key in ["f1", "f12"] {
            assert!(
                parse_args(["--drm", "--dead-man-chord", key]).is_err(),
                "{key} must be reserved"
            );
        }
        // ...and a key just outside it is still available, so this is a
        // reservation rather than a blanket refusal of everything.
        assert!(parse_args(["--drm", "--dead-man-chord", "insert"]).is_err()); // clipboard's
        assert!(parse_args(["--drm", "--dead-man-chord", "home"]).is_ok());
        // The shipped defaults collide with nothing: esc, super, insert,
        // ctrl+print, ctrl+alt+delete -- no F-key among them.
        assert!(parse_args(["--drm"]).is_ok());
        // `--attention-chord` is in the reservation list and is deliberately
        // NOT tested for a refusal: `AttentionChord`'s vocabulary is `super`
        // and `rsuper` only, so the collision is unreachable today. It is
        // listed in the check for the same defence-in-depth reason the
        // dead-man/attention pair above it is -- a vocabulary that grows
        // must not silently disarm the escape -- and saying so here is what
        // keeps a reader from writing a test that can never go red.
        assert!(parse_args(["--drm", "--attention-chord", "f5"]).is_err());
    }

    // -- P2.6.2 (#186): the confinement selector -------------------------

    /// The selector this task's whole posture rests on: **omitting it
    /// confines**.
    #[test]
    fn omitting_the_isolation_flag_selects_confinement_in_every_mode() {
        for argv in [
            vec!["--nested"],
            vec!["--headless", "--consent=auto-approve"],
        ] {
            let action = parse_args(argv.clone()).expect("parses");
            assert_eq!(
                isolation_of(&action),
                IsolationOptions::default(),
                "{argv:?} did not default to confinement"
            );
        }
        // And the default really is `default` rather than whatever `Isolation`
        // would have defaulted to -- it has no `Default`, deliberately, so
        // this is the only place the value is chosen.
        assert_eq!(
            IsolationOptions::default().isolation,
            spawn::isolation::Isolation::Default
        );
        assert_eq!(IsolationOptions::default().realm_init, None);
    }

    #[test]
    fn isolation_takes_both_spellings_refuses_a_repeat_and_names_the_retired_token() {
        for argv in [
            vec!["--nested", "--isolation", "off"],
            vec!["--nested", "--isolation=off"],
        ] {
            let action = parse_args(argv.clone()).expect("parses");
            assert_eq!(
                isolation_of(&action).isolation,
                spawn::isolation::Isolation::Off,
                "{argv:?}"
            );
        }
        // The `--shim` precedent: no value, an empty value, and a repeat.
        assert!(parse_args(["--nested", "--isolation"]).is_err());
        assert!(parse_args(["--nested", "--isolation="]).is_err());
        assert!(
            parse_args(["--nested", "--isolation=off", "--isolation=default"])
                .unwrap_err()
                .contains("more than once")
        );
        // `none` by name, on `parse_consent`'s copy precedent: everybody who
        // read D-020(6) will type it, and the message has to say the token
        // moved rather than that they mistyped.
        let retired = parse_args(["--nested", "--isolation=none"]).unwrap_err();
        assert!(retired.contains("--isolation=off"), "{retired}");
        // Non-vacuity: an unrelated value gets the generic message instead.
        let other = parse_args(["--nested", "--isolation=paranoid"]).unwrap_err();
        assert!(!other.contains("D-020(6)"), "{other}");
    }

    #[test]
    fn realm_init_parses_both_spellings_and_defaults_to_a_sibling() {
        for argv in [
            vec![
                "--nested",
                "--realm-init",
                "/usr/lib/vitrin/vitrin-realm-init",
            ],
            vec!["--nested", "--realm-init=/usr/lib/vitrin/vitrin-realm-init"],
        ] {
            let action = parse_args(argv.clone()).expect("parses");
            assert_eq!(
                isolation_of(&action).realm_init.as_deref(),
                Some(Path::new("/usr/lib/vitrin/vitrin-realm-init")),
                "{argv:?}"
            );
        }
        assert!(parse_args(["--nested", "--realm-init"]).is_err());
        assert!(parse_args(["--nested", "--realm-init="]).is_err());
        assert!(
            parse_args(["--nested", "--realm-init=/a", "--realm-init=/b"])
                .unwrap_err()
                .contains("--realm-init")
        );
        // The default is resolved at startup, not at parse time, exactly as
        // `--shim`'s is.
        assert_eq!(
            isolation_of(&parse_args(["--nested"]).unwrap()).realm_init,
            None
        );
    }

    #[test]
    fn the_floor_is_answerable_from_any_command_line_and_is_not_a_kernel_row() {
        // `--print-isolation`'s precedent: a question about this binary must
        // not be answerable only on a command line that would otherwise have
        // run a session.
        assert_eq!(parse_args(["--print-floor"]), Ok(Action::PrintFloor));
        assert_eq!(
            parse_args(["--nested", "--print-floor"]),
            Ok(Action::PrintFloor)
        );
        assert_eq!(
            parse_args(["--print-floor", "--size"]),
            Ok(Action::PrintFloor),
            "it must win over a later parse error, like --print-isolation does"
        );
        assert!(USAGE.contains("--print-floor"));
        assert!(USAGE.contains("--isolation"));
        assert!(USAGE.contains("--realm-init"));
        // The cost D-036 says must be said out loud or it reads as an
        // oversight: `off` is not a rung of the default/hardened/paranoid dial.
        assert!(
            USAGE.contains("paranoid"),
            "--help must say that `off` is not a rung of D-010's dial"
        );
    }

    /// The `isolation` field of whichever run variant an `Action` is.
    fn isolation_of(action: &Action) -> IsolationOptions {
        match action {
            Action::RunNested { isolation, .. } | Action::RunHeadless { isolation, .. } => {
                isolation.clone()
            }
            #[cfg(feature = "drm-backend")]
            Action::RunDrm { isolation, .. } => isolation.clone(),
            other => panic!("not a run action: {other:?}"),
        }
    }

    /// **The reservation does not reach the other two backends** — the control
    /// that stops the test above passing against a blanket refusal.
    ///
    /// The `--headless` skip the lock chord's own check already sets, for the
    /// same reason applied to a different fact: the VT escape does not exist
    /// on nested or headless, because only a process holding DRM master can
    /// implement a chord the kernel stops handling once it does. Refusing
    /// `--dead-man-chord f5` there would be refusing a collision that cannot
    /// happen.
    #[test]
    fn the_vt_reservation_does_not_reach_the_other_backends() {
        assert!(parse_args(["--nested", "--dead-man-chord", "f5"]).is_ok());
        assert!(parse_args([
            "--headless",
            "--consent=auto-approve",
            "--dead-man-chord",
            "f5"
        ])
        .is_ok());
        assert!(parse_args(["--nested", "--lock-chord", "ctrl+alt+f5"]).is_ok());
        assert!(parse_args(["--nested", "--clipboard-key", "f5"]).is_ok());
    }

    /// **A build without the backend cannot even name its flags**, the
    /// `--consent-injector-fd` precedent applied to a deployment feature.
    ///
    /// It matters for the same reason: a flag that is recognised-and-ignored
    /// makes an operator believe something false about a running process, and
    /// `/proc/<pid>/cmdline` is what tells them otherwise.
    #[cfg(not(feature = "drm-backend"))]
    #[test]
    fn a_build_without_the_drm_backend_cannot_name_its_flags() {
        for args in [
            vec!["--drm"],
            vec!["--nested", "--keymap", "/etc/vitrin/keymap.xkb"],
        ] {
            let err = parse_args(args.clone())
                .expect_err(&format!("{args:?} must not parse without the backend"));
            assert!(
                err.contains("unknown argument"),
                "the flag must be an unknown argument, not a recognised-and-ignored one: {err}"
            );
        }
        // ...and the mode list an error names must not advertise it either.
        let err = parse_args::<[&str; 0]>([]).expect_err("no mode given");
        assert!(!err.contains("--drm"), "{err}");
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
                // `..` covers `isolation`, which this row says nothing about.
                // It briefly read `isolation: IsolationOptions::default()`,
                // which is a function call in pattern position and does not
                // compile at all -- landing green because NO CI job builds
                // the `consent-injector` feature, so this test had never once
                // been compiled, let alone run. That is issue #288's failure
                // class one level below a skip, and the job step added
                // alongside this fix is what makes it visible.
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
                isolation: IsolationOptions::default(),
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
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
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
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: None,
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
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
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::AutoApprove,
                principals: None,
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: Some(PathBuf::from("/tmp/realm.toml")),
                shim: None,
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
                })
            );
        }
        // Valid with the nested mode too, and the default is `None`.
        assert_eq!(
            parse_args(["--nested", "--shim=/opt/vitrin/vitrin-shim"]),
            Ok(Action::RunNested {
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: Some(PathBuf::from("/opt/vitrin/vitrin-shim")),
                capture_dump: None,
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
                })
            );
        }
        // Valid nested too, and the default really is `None`.
        assert_eq!(
            parse_args(["--nested", "--capture-dump=/tmp/x.rgba"]),
            Ok(Action::RunNested {
                isolation: IsolationOptions::default(),
                consent: ConsentPolicy::Interactive,
                principals: None,
                recorder: None,
                realm: None,
                shim: None,
                capture_dump: Some(PathBuf::from("/tmp/x.rgba")),
                dead_man: DeadManConfig::default(),
                attention: default_attention(),
                clipboard: default_clipboard(),
                lock: default_lock(),
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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
            // No `--screenshot-dir`: this fixture is about the R6 registry
            // guard, and a screenshot directory would be a second thing that
            // could fail the startup it is measuring.
            None,
            // No backlight, for the reason above and one more: probing sysfs
            // here would read the machine running the suite.
            None,
            // The confinement selector this fixture runs under. `off`
            // deliberately: the assertion is about the auto-approve refusal,
            // and a machine whose kernel refuses namespaces would otherwise
            // fail this test for an unrelated reason.
            IsolationOptions {
                isolation: spawn::isolation::Isolation::Off,
                landlock: vitrin_realm_init::LandlockRequest::Highest,
                realm_init: None,
            },
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
            None,
            // No backlight: this test never reaches an input path, and a
            // session that probed sysfs here would be reading the machine
            // running the suite.
            None,
            // `off`, for the same reason as the fixture above: this test is
            // about auto-approve's admission, not about the kernel's.
            IsolationOptions {
                isolation: spawn::isolation::Isolation::Off,
                landlock: vitrin_realm_init::LandlockRequest::Highest,
                realm_init: None,
            },
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
                    isolation: IsolationOptions::default(),
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
                    screenshot: default_screenshot(),
                    status: status::StatusConfig::off(),
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
                isolation: IsolationOptions::default(),
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
                screenshot: default_screenshot(),
                status: status::StatusConfig::off(),
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

    /// `--status` (WS-E.2.3, issue #215): off by default, accepted in **both**
    /// modes, and its two valued companions refused without it.
    ///
    /// Both modes, unlike `--agent-cursor` and unlike the `--lock-*` family:
    /// the strip needs neither a physical input device nor a host window, so a
    /// per-backend refusal would be a refusal with no reason behind it. What is
    /// asserted here is that the default really is off — the property
    /// `tests/integration/test_real_trust_band.py` depends on.
    #[test]
    fn the_status_flag_is_off_by_default_in_both_modes() {
        let headless = |args: Vec<&str>| match parse_args(args.clone()) {
            Ok(Action::RunHeadless { status, .. }) => status,
            other => panic!("{args:?} must parse as a headless run: {other:?}"),
        };
        let nested = |args: Vec<&str>| match parse_args(args.clone()) {
            Ok(Action::RunNested { status, .. }) => status,
            other => panic!("{args:?} must parse as a nested run: {other:?}"),
        };

        assert_eq!(
            headless(vec!["--headless", "--consent=auto-approve"]),
            status::StatusConfig::off()
        );
        assert_eq!(nested(vec!["--nested"]), status::StatusConfig::off());

        let on = headless(vec!["--headless", "--consent=auto-approve", "--status"]);
        assert!(on.enabled);
        assert_eq!(on.height, status::DEFAULT_HEIGHT);
        assert_eq!(on.utc_offset, status::UtcOffset::UTC);
        assert!(nested(vec!["--nested", "--status"]).enabled);
        // A boolean switch repeated says the same thing twice.
        assert!(nested(vec!["--nested", "--status", "--status"]).enabled);

        // The flags are named in the help text, so an operator can find them.
        assert!(USAGE.contains("--status"));
        assert!(USAGE.contains("--status-height"));
        assert!(USAGE.contains("--status-utc-offset"));
    }

    #[test]
    fn the_status_height_and_offset_are_validated_at_parse_time() {
        let of = |args: Vec<&str>| match parse_args(args.clone()) {
            Ok(Action::RunNested { status, .. }) => status,
            other => panic!("{args:?} must parse as a nested run: {other:?}"),
        };
        assert_eq!(
            of(vec!["--nested", "--status", "--status-height", "32"]).height,
            32
        );
        assert_eq!(
            of(vec![
                "--nested",
                "--status",
                "--status-utc-offset",
                "+09:00"
            ])
            .utc_offset,
            status::UtcOffset::parse("+09:00").unwrap()
        );

        // Out of range, in both directions, with the reason named.
        for bad in ["15", "65", "0", "nonsense"] {
            let err = parse_args(["--nested", "--status", "--status-height", bad])
                .expect_err("an out-of-range strip height must not start a session");
            assert!(err.contains("--status-height"), "{err}");
        }
        assert!(parse_args(["--nested", "--status", "--status-height", "16"]).is_ok());
        assert!(parse_args(["--nested", "--status", "--status-height", "64"]).is_ok());

        let err = parse_args(["--nested", "--status", "--status-utc-offset", "+15:00"])
            .expect_err("an offset outside the civil range must be refused");
        assert_eq!(err, status::sample::OFFSET_REFUSAL);

        // Repeats are refused rather than silently taking the last value.
        assert!(parse_args([
            "--nested",
            "--status",
            "--status-height",
            "20",
            "--status-height",
            "24"
        ])
        .is_err());
        assert!(parse_args([
            "--nested",
            "--status",
            "--status-utc-offset",
            "UTC",
            "--status-utc-offset",
            "Z"
        ])
        .is_err());

        // A value with no `--status` is a command line whose author believed
        // something false, so it is refused rather than stored and ignored.
        let orphan = parse_args(["--nested", "--status-height", "20"])
            .expect_err("a height without a strip must be refused");
        assert!(orphan.contains("--status"), "{orphan}");
        assert!(parse_args(["--nested", "--status-utc-offset", "+09:00"]).is_err());
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

    /// The lock policy a command line with no `--lock-*` flag resolves to:
    /// the default chord armed, no idle raise, no passphrase.
    /// The screenshot policy a command line with no `--screenshot-*` flag
    /// resolves to: the default chord and **no directory**, so the key is
    /// consumed and writes nothing (WS-E.2.4).
    fn default_screenshot() -> screenshot::ScreenshotConfig {
        screenshot::ScreenshotConfig::default()
    }

    fn default_lock() -> lock::LockConfig {
        lock::LockConfig {
            chord: chord::ModChord::parse(lock::DEFAULT_LOCK_CHORD).expect("the default parses"),
            idle: None,
            passphrase: None,
            // D-030(2)'s behaviour: a VT switch away does not lock the session
            // (issue #246). Spelled rather than `..Default::default()`, so a
            // change to the default is a change to this line and every parse
            // test that compares against it.
            on_seat_change: lock::SeatChangePolicy::Never,
        }
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

    /// The `--blank-idle` a bare-metal run resolved (WS-E.4.3).
    #[cfg(feature = "drm-backend")]
    fn blank_of(action: &Action) -> Option<Duration> {
        match action {
            Action::RunDrm { blank_idle, .. } => *blank_idle,
            other => panic!("not a bare-metal run action: {other:?}"),
        }
    }

    /// **`--blank-idle` is refused on every backend that does not own a display
    /// controller** (WS-E.4.3, issue #223), and refused rather than accepted as
    /// a silent no-op.
    ///
    /// The `--agent-cursor` and `--size` posture, taken here for a reason that
    /// is sharper on headless than on either of those: the flag would not merely
    /// be inert, it would **wedge the session dark**, because that backend's
    /// hook stack carries no lock gate and therefore nothing at all writes the
    /// activity clock a wake reads.
    ///
    /// Asserted in a build **without** `drm-backend` too, where there is no
    /// mode the flag could be valid in at all -- that arm has its own `cfg`, and
    /// a `cfg` nothing exercises is a refusal that can rot.
    #[test]
    fn blank_idle_is_refused_on_every_backend_without_an_output() {
        for args in [
            vec!["--nested", "--blank-idle", "300"],
            vec!["--nested", "--blank-idle=300"],
            vec!["--headless", "--blank-idle", "300"],
        ] {
            let err = parse_args(args.clone())
                .expect_err("a backend with no display controller cannot blank one");
            assert!(
                err.contains("--blank-idle") && err.contains("--drm"),
                "the refusal must name the flag and the one backend it is valid on: {err}"
            );
        }
        // Named in the help text, so an operator can find it -- and in `USAGE`
        // rather than `DRM_USAGE`, because the parser has an arm for the flag in
        // every build and answers a *reason* rather than `unknown argument`.
        assert!(USAGE.contains("--blank-idle"));
        assert!(
            USAGE.contains("IT DOES NOT LOCK"),
            "the help must say, where an operator will actually read it, that an idle blank \
             leaves the session unlocked. That consequence is published rather than softened"
        );

        // The headless message must say WHY, because "no output" is only half
        // of it and the other half is the one that wedges.
        let err = parse_args(["--headless", "--blank-idle", "300"]).unwrap_err();
        assert!(
            err.contains("lock gate") || err.contains("activity clock"),
            "the headless refusal must name the missing activity clock, not only the missing \
             display: a reader who fixes the display half would still ship a session that \
             goes dark and never comes back. Got: {err}"
        );
    }

    /// The backlight configuration a bare-metal run resolved (D-041, #303).
    #[cfg(feature = "drm-backend")]
    fn backlight_of(action: &Action) -> Option<backlight::BacklightConfig> {
        match action {
            Action::RunDrm { backlight, .. } => backlight.clone(),
            other => panic!("not a bare-metal run action: {other:?}"),
        }
    }

    /// **`--backlight` is refused on every backend that does not drive the
    /// human's panel** (D-041, issue #303), and refused rather than accepted as
    /// a silent no-op.
    ///
    /// `--blank-idle`'s posture with a different reason, and the difference is
    /// worth the test: a nested write would not be inert, it would **succeed**,
    /// on a panel this core does not own and while the host compositor acts on
    /// the same key.
    ///
    /// Asserted in a build **without** `drm-backend` too, where there is no
    /// mode the flag could be valid in at all.
    #[test]
    fn backlight_is_refused_on_every_backend_without_the_panel() {
        for args in [
            vec!["--nested", "--backlight"],
            vec!["--headless", "--consent=auto-approve", "--backlight"],
            vec![
                "--nested",
                "--backlight",
                "--backlight-device",
                "intel_backlight",
            ],
        ] {
            let err = parse_args(args.clone())
                .expect_err("a backend that does not drive the panel cannot dim it");
            assert!(
                err.contains("--backlight") && err.contains("--drm"),
                "the refusal must name the flag and the one backend it is valid on: {err}"
            );
        }
        // In `USAGE` rather than `DRM_USAGE`, for `--blank-idle`'s reason: the
        // parser has an arm for the flag in every build and answers a *reason*
        // rather than `unknown argument`.
        assert!(USAGE.contains("--backlight"));
        assert!(USAGE.contains("--backlight-device"));
        assert!(
            USAGE.contains("THE TWO KEYS ARE CONSUMED"),
            "the help must say, where an operator will actually read it, that the brightness \
             keys stop reaching their apps. That is a reversal of what the previous release \
             shipped and it is published rather than softened"
        );
        assert!(
            USAGE.contains("IT DOES NOTHING FOR AN EXTERNAL DISPLAY"),
            "...and that it does nothing for an external display, which is the half of this \
             feature a human is most likely to meet first"
        );
        assert!(
            USAGE.contains("NEVER REACHES 0"),
            "...and that there is a floor, because a black panel is indistinguishable from a \
             blanked one"
        );
    }

    /// **The keys are not taken from the app unless the operator asked**, and
    /// the device name needs the opt-in that makes it readable.
    #[test]
    fn the_backlight_flags_default_off_and_the_device_needs_the_switch() {
        let err = parse_args(["--nested", "--backlight-device", "intel_backlight"])
            .expect_err("a device name with no feature to use it must not be stored");
        assert!(err.contains("--backlight-device"), "{err}");
        assert!(err.contains("needs `--backlight`"), "{err}");

        let err = parse_args([
            "--nested",
            "--backlight",
            "--backlight-device",
            "intel_backlight",
            "--backlight-device",
            "acpi_video0",
        ])
        .expect_err("a valued flag repeated must not silently pick a winner");
        assert!(
            err.contains("--drm") || err.contains("given more than once"),
            "{err}"
        );
        assert!(
            parse_args(["--nested", "--backlight-device"]).is_err(),
            "a bare valued flag takes the next argument; there is none"
        );
    }

    /// A device name is a **name**, never a path, and that is refused at parse
    /// time rather than discovered by a `join` that left the fixed root.
    #[test]
    fn a_backlight_device_is_never_a_path() {
        for bad in ["../../etc", "a/b", ".", "..", "intel backlight", ""] {
            let err = parse_args(["--nested", "--backlight", "--backlight-device", bad])
                .expect_err("a path must not be accepted as a device name");
            assert!(
                err.contains("--backlight-device") || err.contains("--drm"),
                "{bad:?}: {err}"
            );
        }
    }

    /// The flag resolves on `--drm`, in both spellings, and is `None` by
    /// default -- which is the state in which both keys still reach the app.
    #[cfg(feature = "drm-backend")]
    #[test]
    fn the_backlight_flag_resolves_on_bare_metal() {
        assert_eq!(
            backlight_of(&parse_args(["--drm"]).expect("defaults parse")),
            None,
            "off by default: without the flag the brightness keys are delivered as before"
        );
        assert_eq!(
            backlight_of(&parse_args(["--drm", "--backlight"]).expect("the flag parses")),
            Some(backlight::BacklightConfig { device: None }),
            "`--backlight` alone means the auto-pick"
        );
        for args in [
            vec![
                "--drm",
                "--backlight",
                "--backlight-device",
                "intel_backlight",
            ],
            vec!["--drm", "--backlight", "--backlight-device=intel_backlight"],
        ] {
            assert_eq!(
                backlight_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}"))),
                Some(backlight::BacklightConfig {
                    device: Some(
                        backlight::DeviceName::parse("intel_backlight").expect("a name is a name")
                    ),
                }),
            );
        }
        // Idempotent, on `--status`'s precedent: a switch repeated says the
        // same thing twice.
        assert_eq!(
            backlight_of(&parse_args(["--drm", "--backlight", "--backlight"]).expect("idempotent")),
            Some(backlight::BacklightConfig { device: None })
        );
    }

    /// The lock policy a run resolved, whichever mode it named.
    fn lock_of(action: &Action) -> lock::LockConfig {
        match action {
            Action::RunNested { lock, .. } => lock.clone(),
            other => panic!("not a nested run action: {other:?}"),
        }
    }

    /// The seat-change policy a bare-metal run resolved (issue #246).
    #[cfg(feature = "drm-backend")]
    fn seat_policy_of(action: &Action) -> lock::SeatChangePolicy {
        match action {
            Action::RunDrm { lock, .. } => lock.on_seat_change,
            other => panic!("not a bare-metal run action: {other:?}"),
        }
    }

    /// **`--lock-on-seat-change` is refused on every backend with no seat**
    /// (issue #246), rather than accepted as a silent no-op.
    ///
    /// `--blank-idle`'s posture, one step milder in consequence and identical
    /// in shape: a nested session never receives a seat event, so the policy
    /// could only ever be stored and never read — and an operator who believes
    /// their nested session locks itself when they alt-tab away has been told
    /// something false by their own command line.
    ///
    /// Asserted in a build **without** `drm-backend` too, where there is no
    /// mode the flag could be valid in at all.
    #[test]
    fn the_seat_change_policy_is_refused_on_every_backend_without_a_seat() {
        for args in [
            vec!["--nested", "--lock-on-seat-change", "immediate"],
            vec!["--nested", "--lock-on-seat-change=immediate"],
            vec!["--nested", "--lock-on-seat-change", "never"],
            vec!["--headless", "--lock-on-seat-change", "idle"],
        ] {
            let err = parse_args(args.clone())
                .expect_err("a backend with no seat cannot have a seat-change policy");
            assert!(
                err.contains("--lock-on-seat-change") && err.contains("--drm"),
                "the refusal must name the flag and the one backend it is valid on: {err}"
            );
        }
        // Even the DEFAULT value is refused when named: the refusal is about a
        // flag being stored and never read, and `never` is not an exception to
        // that -- accepting it would teach an operator that the flag works
        // here, and they would go on to write `immediate`.
        assert!(parse_args(["--nested", "--lock-on-seat-change=never"]).is_err());

        // Named in `USAGE` rather than a bare-metal-only help text, because
        // the parser has an arm for the flag in every build and answers a
        // *reason* rather than `unknown argument`.
        assert!(USAGE.contains("--lock-on-seat-change"));
        for policy in ["immediate", "idle", "never"] {
            assert!(
                USAGE.contains(policy),
                "the help must name the whole vocabulary: {policy} is missing"
            );
        }
        assert!(
            USAGE.contains("never a way past a lock screen"),
            "the help must say the thing no policy changes, where an operator picking one will \
             read it"
        );
    }

    #[test]
    fn the_lock_flags_parse_both_spellings_and_default_independently() {
        // The default, pinned for the reason the dead-man's and the attention
        // key's are: a session that came up with a different lock chord than
        // the documented one is a surprise at the moment somebody is trying to
        // lock their screen.
        let defaults = lock_of(&parse_args(["--nested"]).expect("defaults parse"));
        assert_eq!(defaults.chord.spelling(), "ctrl+alt+delete");
        assert_eq!(defaults.idle, None, "no idle raise unless asked for");
        assert_eq!(defaults.passphrase, None);
        assert_eq!(
            defaults.on_seat_change,
            lock::SeatChangePolicy::Never,
            "a VT switch away does not lock the session unless somebody says otherwise \
             (D-030(2), issue #246)"
        );

        for args in [
            vec!["--nested", "--lock-chord", "super+f12"],
            vec!["--nested", "--lock-chord=super+f12"],
        ] {
            let cfg =
                lock_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(cfg.chord.spelling(), "super+f12");
            // The other halves keep their defaults rather than being reset.
            assert_eq!(cfg.idle, None);
            assert_eq!(dead_man_of(&parse_args(args).unwrap()).chord.name(), "esc");
        }
        for args in [
            vec!["--nested", "--lock-idle", "300"],
            vec!["--nested", "--lock-idle=300"],
        ] {
            let cfg =
                lock_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(cfg.idle, Some(Duration::from_secs(300)));
            assert_eq!(cfg.chord.spelling(), "ctrl+alt+delete");
        }
        for args in [
            vec![
                "--nested",
                "--lock-passphrase-file",
                "/etc/vitrin/lock.hash",
            ],
            vec!["--nested", "--lock-passphrase-file=/etc/vitrin/lock.hash"],
        ] {
            let cfg =
                lock_of(&parse_args(args.clone()).unwrap_or_else(|e| panic!("{args:?}: {e}")));
            assert_eq!(
                cfg.passphrase,
                Some(PathBuf::from("/etc/vitrin/lock.hash")),
                "{args:?}"
            );
        }
    }

    /// **Issue #214 acceptance criterion 6**: a passphrase on a backend that
    /// cannot deliver the alphabet is refused AT STARTUP, with a message naming
    /// the reason, and the process exits non-zero.
    ///
    /// `parse_args` returning `Err` *is* the non-zero exit: `main` prints it
    /// and returns `ExitCode::from(2)`, which
    /// `a_parse_error_is_a_non_zero_exit_and_not_a_run` pins separately.
    ///
    /// The reason has to be *named*, not merely refused, because an operator
    /// who reads only "not supported" will reasonably try to work around it —
    /// and the workaround (grow a keymap) is a decision WS-E Stage 3 owns.
    #[test]
    fn a_passphrase_is_refused_on_a_backend_that_cannot_deliver_the_alphabet() {
        let err = parse_args([
            "--headless",
            "--consent=auto-approve",
            "--lock-passphrase-file",
            "/etc/vitrin/lock.hash",
        ])
        .expect_err("a headless session could never type a passphrase");
        assert!(err.contains("keymap"), "the reason must be named: {err}");
        assert!(err.contains("letters and digits"), "{err}");
        assert!(err.contains("--nested"), "the way out must be named: {err}");

        // ...and the same file is accepted under `--nested`, so the refusal is
        // about the BACKEND and not about the flag being unimplemented.
        assert!(parse_args([
            "--nested",
            "--lock-passphrase-file",
            "/etc/vitrin/lock.hash"
        ])
        .is_ok());
    }

    #[test]
    fn an_idle_lock_is_refused_headless_for_a_different_named_reason() {
        // Not the same refusal as the passphrase's, and deliberately not the
        // same message: this one is about there being no input device at all,
        // so a headless `--lock-idle` would fire on a timer and then have
        // nothing that could dismiss it.
        let err = parse_args(["--headless", "--consent=auto-approve", "--lock-idle", "60"])
            .expect_err("a headless session could never dismiss a lock");
        assert!(err.contains("no physical input device"), "{err}");
        assert!(err.contains("wedge"), "{err}");

        // The passphrase's message wins when both are given: it names the
        // sharper fact.
        let both = parse_args([
            "--headless",
            "--consent=auto-approve",
            "--lock-idle",
            "60",
            "--lock-passphrase-file",
            "/etc/vitrin/lock.hash",
        ])
        .expect_err("both are refused");
        assert!(both.contains("keymap"), "{both}");
    }

    #[test]
    fn a_session_never_comes_up_with_a_lock_chord_that_cannot_fire() {
        // (1) Not a chord at all: a bare key belongs to `deadman::Chord`, and
        // this parser refuses to become a second spelling of one.
        let err = parse_args(["--nested", "--lock-chord", "delete"])
            .expect_err("a bare key is not a modifier chord");
        assert!(err.contains("at least one modifier"), "{err}");
        assert!(err.contains("MOD[+MOD...]+KEY"), "{err}");

        // (2) A key outside the vocabulary. The message must list what is in
        // it, so an operator is not left guessing which keys the core can even
        // see without a keymap.
        let err = parse_args(["--nested", "--lock-chord", "ctrl+alt+q"])
            .expect_err("`q` is layout-dependent and not in the vocabulary");
        assert!(err.contains("unknown chord key"), "{err}");
        assert!(err.contains("delete") && err.contains("f12"), "{err}");

        // (3) The collision refusal, and it is REACHABLE from a plausible
        // command line: `insert` is in both `deadman::Chord`'s vocabulary and
        // `chord::Trigger`'s, so this is a mistake an operator can make today.
        let err = parse_args([
            "--nested",
            "--lock-chord",
            "ctrl+alt+insert",
            "--clipboard-key",
            "insert",
        ])
        .expect_err("the lock chord and the clipboard chord share a key");
        assert!(err.contains("--clipboard-key"), "{err}");
        assert!(err.contains("observe tap"), "the WHY must be named: {err}");

        // ...and against the dead-man chord, which is the one that matters
        // most: a lock chord sharing the off-switch's key would arm the
        // off-switch every time the human locked their screen.
        let err = parse_args([
            "--nested",
            "--lock-chord",
            "ctrl+alt+f12",
            "--dead-man-chord",
            "f12",
        ])
        .expect_err("the lock chord and the dead-man chord share a key");
        assert!(err.contains("--dead-man-chord"), "{err}");

        // The default chord collides with nothing a default command line has.
        assert!(parse_args(["--nested"]).is_ok());
    }

    /// The screenshot policy a run resolved, whichever mode it named.
    fn screenshot_of(action: &Action) -> screenshot::ScreenshotConfig {
        match action {
            Action::RunNested { screenshot, .. } | Action::RunHeadless { screenshot, .. } => {
                screenshot.clone()
            }
            other => panic!("not a run action: {other:?}"),
        }
    }

    /// **The fifth core-owned chord's startup refusals** (WS-E.2.4, #216).
    ///
    /// Every one of them is a fail-open configuration trap this parser exists
    /// to close: a screenshot key that silently never fires, or one that arms
    /// the human's off-switch instead.
    #[test]
    fn a_session_never_comes_up_with_a_screenshot_key_that_cannot_fire() {
        // The default, pinned for the reason the other four are: a session that
        // came up with a different chord than the documented one surprises the
        // human at the moment they are trying to use it.
        let defaults = screenshot_of(&parse_args(["--nested"]).expect("defaults parse"));
        assert_eq!(defaults.chord.spelling(), "ctrl+print");
        assert_eq!(
            defaults.dir, None,
            "no directory unless asked for: the key is consumed and writes nothing"
        );

        // (1) A bare key is not a chord. `crate::chord` refuses one by
        // construction, so #216's proposed bare `print` is inexpressible here
        // rather than special-cased.
        let err = parse_args(["--nested", "--screenshot-chord", "print"])
            .expect_err("a bare key is not a modifier chord");
        assert!(err.contains("at least one modifier"), "{err}");

        // (2) A key outside the vocabulary, with the vocabulary in the message.
        let err = parse_args(["--nested", "--screenshot-chord", "ctrl+q"])
            .expect_err("`q` is layout-dependent");
        assert!(err.contains("unknown chord key"), "{err}");
        assert!(err.contains("print"), "the new key must be offered: {err}");

        // (3) The collision refusal, against each of the four chords that came
        // before, and REACHABLE from a plausible command line in every case:
        // `print` is now in `chord::Trigger`'s vocabulary, so an operator can
        // aim two gestures at it today.
        for (flag, value) in [
            ("--clipboard-key", "print"),
            ("--lock-chord", "ctrl+alt+print"),
        ] {
            let err = parse_args([
                "--nested",
                "--screenshot-dir",
                "/tmp",
                "--screenshot-chord",
                "ctrl+print",
                flag,
                value,
            ])
            .expect_err("two core-owned gestures on one key");
            assert!(err.contains("print"), "{err}");
            assert!(err.contains("observe tap"), "the WHY must be named: {err}");
        }
        // ...and against the off-switch, which is the one that matters most.
        let err = parse_args([
            "--nested",
            "--screenshot-dir",
            "/tmp",
            "--screenshot-chord",
            "ctrl+f12",
            "--dead-man-chord",
            "f12",
        ])
        .expect_err("the screenshot chord and the dead-man chord share a key");
        assert!(err.contains("--dead-man-chord"), "{err}");

        // (4) A configured gesture with nowhere to write. The chord would be
        // consumed and produce nothing, which is a key that silently does not
        // work -- the exact class of trap (1)-(3) exist to close.
        let err = parse_args(["--nested", "--screenshot-chord", "ctrl+f11"])
            .expect_err("a chord with no directory writes nothing");
        assert!(err.contains("--screenshot-dir"), "{err}");
        assert!(err.contains("silently"), "the WHY must be named: {err}");

        // The other direction is fine: a directory alone arms the default
        // chord, which is the ordinary way to turn the feature on.
        let cfg = screenshot_of(
            &parse_args(["--nested", "--screenshot-dir", "/tmp/shots"]).expect("dir alone parses"),
        );
        assert_eq!(cfg.dir.as_deref(), Some(Path::new("/tmp/shots")));
        assert_eq!(cfg.chord.spelling(), "ctrl+print");

        // Both spellings of both flags, and a repeat is refused rather than
        // silently picking a winner.
        for args in [
            vec![
                "--headless",
                "--consent=auto-approve",
                "--screenshot-dir",
                "/tmp/shots",
                "--screenshot-chord",
                "super+f5",
            ],
            vec![
                "--headless",
                "--consent=auto-approve",
                "--screenshot-dir=/tmp/shots",
                "--screenshot-chord=super+f5",
            ],
        ] {
            let cfg = screenshot_of(&parse_args(args.clone()).expect("both spellings parse"));
            assert_eq!(
                cfg.dir.as_deref(),
                Some(Path::new("/tmp/shots")),
                "{args:?}"
            );
            assert_eq!(cfg.chord.spelling(), "super+f5", "{args:?}");
        }
        for (flag, value) in [
            ("--screenshot-dir", "/tmp/shots"),
            ("--screenshot-chord", "ctrl+f11"),
        ] {
            let err = parse_args(["--nested", flag, value, flag, value])
                .expect_err("a valued flag repeated must not silently pick a winner");
            assert!(err.contains("given more than once"), "{err}");
            assert!(
                parse_args(["--nested", flag]).is_err(),
                "a bare flag takes the next argument; there is none"
            );
        }
    }

    #[test]
    fn a_zero_idle_timeout_is_refused_rather_than_read_as_off() {
        // `--lock-idle 0` would relock on the first round after every unlock.
        // Refusing it means there is exactly one spelling of "no idle lock".
        let err = parse_args(["--nested", "--lock-idle", "0"]).expect_err("zero is a wedge");
        assert!(err.contains("Omit the flag"), "{err}");
        assert!(parse_args(["--nested", "--lock-idle", "-1"]).is_err());
        assert!(parse_args(["--nested", "--lock-idle", "abc"]).is_err());
    }

    #[test]
    fn the_lock_flags_are_refused_twice_and_bare() {
        for flag in ["--lock-chord", "--lock-idle", "--lock-passphrase-file"] {
            assert!(
                parse_args(["--nested", flag]).is_err(),
                "`{flag}` takes the next argument; there is none"
            );
        }
        for (flag, a, b) in [
            ("--lock-chord", "ctrl+alt+delete", "ctrl+alt+f12"),
            ("--lock-idle", "60", "120"),
            ("--lock-passphrase-file", "/a/b", "/c/d"),
        ] {
            let err = parse_args(["--nested", flag, a, flag, b])
                .expect_err("a valued flag repeated must not silently pick a winner");
            assert!(err.contains("given more than once"), "{flag}: {err}");
        }
    }

    #[test]
    fn lock_hash_is_a_print_and_exit_action_independent_of_mode() {
        // The `--print-isolation` posture: it answers a question about a file
        // format, so it must answer it whatever else is on the command line --
        // including a command line that would otherwise be refused.
        assert_eq!(parse_args(["--lock-hash"]), Ok(Action::LockHash));
        assert_eq!(
            parse_args(["--headless", "--lock-hash"]),
            Ok(Action::LockHash)
        );
        assert_eq!(
            parse_args(["--nested", "--size", "1x1", "--lock-hash"]),
            Ok(Action::LockHash),
            "it wins over a refusal it is unrelated to"
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
        // Both together. `home` rather than `delete` since WS-E.2.2: the
        // default lock chord is `ctrl+alt+delete`, and the startup collision
        // check refuses a dead-man chord that shares its trigger key -- because
        // the switch detects in the router's unconditional observe tap, so
        // every lock gesture would arm the human's off-switch. Changing the
        // example is the honest fix; weakening the check would not be.
        let config = dead_man_of(
            &parse_args(["--nested", "--dead-man-chord=home", "--dead-man-hold=2500"])
                .expect("both parse"),
        );
        assert_eq!(config.chord.name(), "home");
        assert_eq!(config.hold, std::time::Duration::from_millis(2500));

        // ...and the collision the change above is about, asserted rather than
        // implied: `--dead-man-chord delete` against the DEFAULT lock chord is
        // a refusal an operator can reach with no `--lock-*` flag at all.
        let err = parse_args(["--nested", "--dead-man-chord=delete"])
            .expect_err("the default lock chord's key is spoken for");
        assert!(err.contains("--lock-chord"), "{err}");
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

    /// A writer that hands every `make_writer` call the same buffer, so a test
    /// can read back exactly the bytes the subscriber emitted.
    ///
    /// `Clone` rather than a borrow because `MakeWriter`'s blanket impl for a
    /// closure is the only one that fits a shared buffer: `Arc<Mutex<Vec<u8>>>`
    /// satisfies neither the `Arc<W>` impl (it needs `&W: Write`) nor the
    /// `Mutex<W>` one (it needs the `Mutex` itself, unwrapped).
    #[derive(Clone)]
    struct SharedLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedLog {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Emit one line through the real subscriber builder and return it.
    ///
    /// `with_default` rather than a global install: the global slot is
    /// one-shot per process and these are two cases in one test binary.
    fn capture_one_line(ansi: bool) -> String {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = SharedLog(std::sync::Arc::clone(&buf));
        let subscriber = diagnostic_subscriber(
            move || sink.clone(),
            ansi,
            tracing_subscriber::EnvFilter::new("info"),
        );
        tracing::subscriber::with_default(subscriber, || {
            // The runbook's own field, verbatim, because it is the one the
            // second DRM run could not grep back out of its log.
            tracing::info!(connector = "eDP-1", "mode set");
        });
        let bytes = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8(bytes).expect("the formatter emits UTF-8")
    }

    /// **A redirected log is greppable, and a terminal still gets colour**
    /// (issue #251).
    ///
    /// What CI can prove is the formatting layer, which needs no DRM device,
    /// no seat and no GPU: given a non-tty writer the line carries a literal
    /// `connector=` and no `ESC[` byte anywhere. What CI **cannot** prove is
    /// the other half of the acceptance criterion — that `grep 'connector='`
    /// over a redirected `vitrind` stderr returns the `mode set:` line — since
    /// no CI runner reaches the DRM path that emits it at all.
    #[test]
    fn a_redirected_log_is_greppable_and_a_terminal_still_gets_colour() {
        let plain = capture_one_line(false);
        assert!(
            plain.contains("connector=\"eDP-1\""),
            "a redirected log must carry the field name, its `=` and its value as adjacent \
             literal bytes, or the runbook's own grep finds nothing and the record paraphrases \
             what it cannot quote. Got: {plain:?}"
        );
        assert!(
            !plain.contains('\u{1b}'),
            "an escape byte anywhere in a redirected log is the defect: it separates a field \
             name from its `=`. Got: {plain:?}"
        );

        // Colour is not removed from interactive use -- only from the case
        // where the bytes are going into a file.
        let coloured = capture_one_line(true);
        assert!(
            coloured.contains('\u{1b}'),
            "a terminal must still get colour; the fix is a tty test, not a removal. Got: \
             {coloured:?}"
        );
    }

    /// The tty test and `NO_COLOR` are both one-way switches, so the rule is
    /// their conjunction (issue #251).
    #[test]
    fn colour_needs_a_terminal_and_an_unset_no_color() {
        assert!(
            ansi_wanted(true, None),
            "an interactive run keeps its colour"
        );
        assert!(
            !ansi_wanted(false, None),
            "a pipe or a file gets no escapes, which is the whole fix"
        );
        // NO_COLOR is honoured BY HAND because `with_ansi` overrides the
        // subscriber's own check; forgetting it would silently repeal a
        // conventional opt-out this binary used to respect.
        assert!(
            !ansi_wanted(true, Some(std::ffi::OsString::from("1"))),
            "NO_COLOR must still turn colour off on a terminal"
        );
        // ...and its own rule: only a NON-EMPTY value opts out.
        assert!(
            ansi_wanted(true, Some(std::ffi::OsString::new())),
            "an empty NO_COLOR is not an opt-out (no-color.org)"
        );
        assert!(!ansi_wanted(false, Some(std::ffi::OsString::from("1"))));
    }
}
