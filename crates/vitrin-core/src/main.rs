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
//!   subscriber, and — since P1.4.5 — one pure-Rust hash crate for the
//!   flight recorder's observation digests. Nothing else, at runtime or in
//!   tests. The headless capture golden asserts against the deterministic
//!   test pattern in-process, so it needs no image codec; PNG serialization
//!   is the SDK's job (P1.8.2), never the core. There is deliberately no
//!   serialization framework: the recorder's JSON-lines emitter is
//!   hand-rolled over a closed set of entry shapes.
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
//! realm spawn manager provides the inherited socketpair (P1.5.2).

mod backend;
/// Capture-frame mechanics (P1.3.6): the sealed-memfd pixel path behind
/// `vitrin_view.frame_ready`. Pure mechanics — the authority decision on
/// every capture lives in `enforcement` (P1.4.4), whose single-path test
/// pins that this module's entry has no other caller. Dead-code-allowed
/// outside tests for the same reason as `headless::render_once`: fully
/// exercised by its tests today, runtime-reachable when the M1.1 listener
/// wiring lands.
#[cfg_attr(not(test), allow(dead_code))]
mod capture;
/// The enforcement chokepoint (P1.4.4): THE single function every capture
/// and actuation passes through — `connection → principal → grant → verbs
/// → constraints` — with the per-grant token bucket, the one
/// `vitrin_grant.refused` emission site, and the admitted-operation
/// dispatch (frame delivery / origin-tagged actuation intake).
/// Dead-code-allowed outside tests for the same reason as `principal`,
/// which drives it end-to-end over socketpairs today; the M1.1 listener
/// wiring makes it runtime-reachable.
#[cfg_attr(not(test), allow(dead_code))]
mod enforcement;
/// The dmabuf import path (P1.3.5): the zero-copy mechanics behind the shim
/// server's `kind=dmabuf` commits — importer seam, hostile-fd probe, GLES
/// import + probe render, copy instrumentation. Dead-code-allowed outside
/// tests for the same reason as `shim`: exercised by its tests (CI-side
/// mocks plus the env-gated real-GPU tests) today, handed a live
/// `GlesRenderer` when the realm/backend wiring lands (P1.5.2).
#[cfg_attr(not(test), allow(dead_code))]
mod dmabuf;
/// Input intake & routing (P1.3.7): origin tagging at intake (backward
/// requirement B2), view→surface coordinate mapping, and the preemption
/// hook point. The nested backend feeds it host input at runtime; seat
/// delivery to a live shim connection arrives with P1.5.2.
mod input;
/// Realm lifecycle (P1.5.3): the realm's death paths — crash detection over
/// two independent signals (socketpair EOF and `SIGCHLD`, either of which
/// may arrive first), `waitpid`-authoritative reaping through calloop's
/// `signalfd` source so no zombie accumulates, the terminal realm state
/// that makes the chokepoint's existing `no_surface` refusal true, and the
/// orderly shutdown ladder (hang up → `SIGTERM` → `SIGKILL`) that removes
/// the realm's runtime tree on the way out. Dead-code-allowed outside tests
/// for the same reason as `spawn`, whose other half it is: exercised
/// end-to-end by its tests today (they really fork a shim, really `kill -9`
/// it mid-capture, and really assert the next capture refuses), and wired
/// into the session by the same M1.1 integration that calls `spawn_realm` —
/// see `run_session`.
#[cfg_attr(not(test), allow(dead_code))]
mod lifecycle;
/// The grant table v0 (P1.4.2): the in-memory PRD Doc 2 §5.2 grant store of
/// the capability kernel — rows keyed by `identity`'s verifier-canonical
/// principal, answering the enforcement chokepoint's grant-scoped use query
/// and admission commit (the P1.4.4 chain: connection → principal → grant →
/// verbs → constraints), plus the embedder-polled proactive expiry sweep.
/// Dead-code-allowed outside tests for the same reason as `capture`: fully
/// exercised by its tests today, consumed by the petition flow (P1.4.3) and
/// the enforcement chokepoint (P1.4.4).
#[cfg_attr(not(test), allow(dead_code))]
mod grants;
/// The identity layer of the capability kernel (P1.4.1): the pluggable
/// `Verifier` trait, the `principals.toml`-backed `StaticVerifier`, and the
/// principal-identity model every grant and enforcement decision keys on.
/// Dead-code-allowed outside tests for the same reason as `capture`: fully
/// exercised by its tests (and `principal`'s) today, consulted at runtime
/// when the principal listener wiring lands (M1.1 integration).
#[cfg_attr(not(test), allow(dead_code))]
mod identity;
/// The grant request flow (P1.4.3): the petition lifecycle state machine of
/// the capability kernel -- pending-petition registry, admission policy
/// (caps, `busy`, `unsupported`, `unavailable`), the consent-policy seam
/// (`--consent`), the consent timeout, and the build-gated scripted-consent
/// injector. Dead-code-allowed outside tests for the same reason as
/// `principal`, which drives it end-to-end over socketpairs today; the M1.1
/// listener wiring constructs the registry from the parsed consent policy
/// and polls its timeout.
#[cfg_attr(not(test), allow(dead_code))]
mod petitions;
/// The principal-connection protocol server (P1.4.1) and the wire half of
/// the petition flow (P1.4.3): the server side of the
/// P1.1.3 handshake state machine (`vitrin_handshake` + `vitrin_principal`),
/// where `identity` binds and where the per-connection object table enforces
/// sender-constrained handles. Dead-code-allowed outside tests for the same
/// reason as `shim`: exercised end-to-end by its tests over socketpairs
/// today, wired to the live listener (`ListenerSource`) at M1.1 integration
/// -- nothing at runtime accepts principal connections before then.
#[cfg_attr(not(test), allow(dead_code))]
mod principal;
/// The realm object and realm registry (P1.5.1): exactly one realm,
/// `realm-0`, built at startup from `realm.toml` -- the stable wire-visible
/// id `get_realm` addresses, the whole-realm grant scope grants attach to,
/// and the owner of the app's spawn configuration (which P1.5.2 executes;
/// nothing here forks). Also the single source of realm existence, which
/// petition admission consults for its `unavailable` judgement. Loaded at
/// startup below; dead-code-allowed outside tests for the same reason as
/// its siblings -- the registry becomes a `ServerCtx` field at the M1.1
/// listener wiring.
#[cfg_attr(not(test), allow(dead_code))]
mod realm;
/// The flight-recorder log v0 (P1.4.5): the journal seed — a JSON-lines
/// event log of handshakes, grant lifecycle transitions, consent decisions,
/// and every enforcement decision, carrying an observation digest on every
/// delivered capture and null-versioned epoch-reference fields (backward
/// requirement B1). Explicitly NOT the signed P6 journal: no signatures, no
/// tamper evidence, never consulted by an authority decision. The run's
/// single handle is created below in `main`; the per-connection emission
/// sites are exercised end-to-end by `principal`'s tests today and become
/// runtime-reachable with the M1.1 listener wiring, so the module is
/// dead-code-allowed outside tests like its siblings.
#[cfg_attr(not(test), allow(dead_code))]
mod recorder;
mod scene;
/// The shim-facing protocol server (P1.3.4): `vitrin_shim_session` +
/// `vitrin_shim_surface`, feeding `Scene::commit`. Dead-code-allowed outside
/// tests for the same reason as `capture`: fully exercised by its tests (and
/// the mock shim, `vitrin-mock-shim`) today, wired to a live shim connection
/// when the realm spawn manager inherits the socketpair at fork (P1.5.2) —
/// nothing at runtime creates a shim connection before then.
#[cfg_attr(not(test), allow(dead_code))]
mod shim;
/// The realm spawn model (P1.5.2): the core's only process-creating code —
/// `fork`/`exec` of the realm's shim with its end of the identity socketpair
/// placed at a fixed descriptor (identity assigned at fork, never claimed),
/// a private `0700` runtime directory, and an environment built from nothing
/// that names only that realm's own socket. Read its module docs before
/// believing any confinement claim: the D9 sandboxing deferral (no
/// namespaces, no seccomp, no Landlock) and the session-D-Bus hole are
/// stated there in full. Dead-code-allowed outside tests for the same reason
/// as `shim`: exercised end-to-end by its tests today (it really forks the
/// mock shim, which really forks an app), and wired into the session when
/// the event loop can service the connection — see `run_session`.
#[cfg_attr(not(test), allow(dead_code))]
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
                                consent surface) or `auto-approve` (every
                                petition granted as requested — headless CI
                                and demos ONLY; loudly logged).
    vitrind [--recorder PATH]   Flight-recorder log for this run (JSON
                                lines, appended). Default:
                                $XDG_RUNTIME_DIR/vitrin-0/flight-recorder-<pid>.jsonl
                                Startup FAILS if the log cannot be opened.
    vitrind [--realm PATH]      Realm configuration for this session (one
                                [[realm]] table: command, args, env_allow).
                                Default: $XDG_CONFIG_HOME/vitrin/realm.toml
                                Startup FAILS if it is missing or malformed.
    vitrind --help              Show this help.
    vitrind --version           Show the version.
";

/// Base name of the default flight-recorder log inside the core's runtime
/// directory; the pid keeps concurrent runs in separate files ("one log
/// file per run") without a randomness dependency.
const RECORDER_FILE_PREFIX: &str = "flight-recorder";

/// The default virtual-output size for `--headless` when `--size` is omitted;
/// matches the nested backend's initial window size so the two backends agree
/// on the same content by default.
const DEFAULT_HEADLESS_SIZE: (u32, u32) = (1280, 800);

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    RunNested {
        consent: ConsentPolicy,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
    },
    RunHeadless {
        size: (u32, u32),
        consent: ConsentPolicy,
        recorder: Option<PathBuf>,
        realm: Option<PathBuf>,
    },
    Help,
    Version,
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
fn parse_args<'a, I: IntoIterator<Item = &'a str>>(args: I) -> Result<Action, String> {
    let mut mode: Option<Mode> = None;
    let mut size: Option<(u32, u32)> = None;
    let mut consent: Option<ConsentPolicy> = None;
    let mut recorder: Option<PathBuf> = None;
    let mut realm: Option<PathBuf> = None;
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
            "--help" | "-h" => return Ok(Action::Help),
            "--version" | "-V" => return Ok(Action::Version),
            other => {
                if let Some(value) = other.strip_prefix("--consent=") {
                    set_consent(&mut consent, parse_consent(value)?)?;
                } else if let Some(value) = other.strip_prefix("--recorder=") {
                    set_path(&mut recorder, "--recorder", "log path", value)?;
                } else if let Some(value) = other.strip_prefix("--realm=") {
                    set_path(&mut realm, "--realm", "config path", value)?;
                } else {
                    return Err(format!("unknown argument `{other}`"));
                }
            }
        }
    }

    // The fail-closed default: nothing is granted without a consent
    // surface unless auto-approve was explicitly flagged.
    let consent = consent.unwrap_or(ConsentPolicy::Interactive);

    // `--help`/`--version` already returned above, so only the run modes
    // remain to resolve; `--size` is meaningless without `--headless`.
    match (mode, size) {
        (Some(Mode::Nested), None) => Ok(Action::RunNested {
            consent,
            recorder,
            realm,
        }),
        (Some(Mode::Nested), Some(_)) => Err("`--size` is only valid with `--headless`".into()),
        (Some(Mode::Headless), size) => Ok(Action::RunHeadless {
            size: size.unwrap_or(DEFAULT_HEADLESS_SIZE),
            consent,
            recorder,
            realm,
        }),
        (None, Some(_)) => Err("`--size` requires `--headless`".into()),
        (None, None) => Err("no mode given (expected `--nested` or `--headless`)".into()),
    }
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
        Action::RunNested {
            consent,
            recorder,
            realm,
        } => {
            init_tracing();
            announce_consent_policy(consent);
            run_session(consent, recorder, realm, backend::winit::run)
        }
        Action::RunHeadless {
            size,
            consent,
            recorder,
            realm,
        } => {
            init_tracing();
            announce_consent_policy(consent);
            run_session(consent, recorder, realm, || backend::headless::run(size))
        }
    }
}

/// Load the session's realm, open the run's flight recorder, run the
/// backend inside it, and close the log.
///
/// The realm goes first (P1.5.1): it is the only startup input whose
/// absence means the session has nothing to serve at all, so failing on it
/// before anything is created leaves no log file and no window behind from
/// a run that could never have worked.
///
/// The recorder brackets the whole session: creation failure is fatal
/// *before* the backend starts (P1.4.5 — an operator who asked for a flight
/// recorder and cannot have one must learn it before the session, not after
/// it is unreconstructable), and the closing entry reports how many entries
/// a mid-run write failure cost, since that is the one thing a truncated
/// log cannot say about itself. Closing goes through `Recorder::finish`,
/// which spends one forced recovery attempt first — a run that degraded
/// transiently can then still write its footer, and only a run that never
/// recovered ends with no file-only evidence at all (which the operator
/// message below says outright rather than implying otherwise).
///
/// Both the recorder handle and the realm registry stay here for now:
/// nothing at runtime accepts principal connections yet (the listener
/// wiring is M1.1 integration), and that wiring hands this same single
/// recorder handle and this same registry to each connection's
/// `ServerCtx`. The registry is also what the spawn manager launches from —
/// it is already the only place the realm's command lives.
///
/// # Where the spawn goes, and why it is not called here yet (P1.5.2)
///
/// `spawn::spawn_realm` belongs immediately after the recorder opens and
/// before `backend()` takes the thread: the realm must exist for the
/// session's whole life, and the spawn is a `realm_spawned` entry the log
/// wants before anything else happens.
///
/// It is deliberately not called at this commit, because a spawned shim
/// needs **an event loop that services its connection**: the shim blocks on
/// `configure` and then on the core's replies, so a backend that never
/// reads the socketpair would leave it wedged at startup forever. That is
/// the M1.1 integration gap (issue #77) that already leaves `shim` and the
/// listener unwired.
///
/// The other blocker P1.5.2 named is gone: `SIGCHLD` reaping and the whole
/// lifecycle are now `lifecycle` (P1.5.3, issue #32), which adopts the
/// `SpawnedRealm`'s unreaped `Child` and owns it to the end.
///
/// Until the wiring lands, both halves are exercised end-to-end by their
/// own tests, which really fork the mock shim, really place the socketpair
/// at fd 3, really `kill -9` it mid-capture, and really assert the next
/// capture refuses `no_surface` through the chokepoint.
///
/// Consequence worth stating plainly rather than discovering later:
/// **issue #31's "`pstree` shows core → shim → app" is satisfied by tests,
/// not by the shipped binary.** A running `vitrind` forks nothing.
///
/// # Where lifecycle plugs in (P1.5.3)
///
/// Three call sites, all in the same wiring:
///
/// - **`lifecycle::child_signal_source()` next to the backend's existing
///   `SIGINT`/`SIGTERM` source**, and for the same reason it sits there:
///   `signalfd` only sees signals blocked on *every* thread, and a mask
///   installed before the backend spawns any is inherited by all of them.
///   Its callback polls each realm's `RealmLifecycle::poll_exit`.
/// - **`RealmLifecycle::note_connection_closed` at the shim connection's
///   removal point**, where `vitrin-ipc`'s event source already funnels
///   EOF, transport faults, and protocol violations into one
///   `DisconnectReason`.
/// - **`RealmLifecycle::shutdown` here**, between `backend()` returning and
///   `recorder.finish()` below — after the loop has stopped (the ladder
///   blocks, deliberately, and must not do so inside a live compositor
///   loop) and before the log closes, so the realm's `realm_died` /
///   `realm_exited` entries land in the run they belong to rather than
///   after its footer.
///
/// The embedder that owns realms also derives `ServerCtx::realm_view` from
/// `RealmLifecycle::view_is_live` — the one seam that makes the
/// chokepoint's `no_surface` refusal true after a shim dies.
fn run_session<R>(
    consent: ConsentPolicy,
    recorder_path: Option<PathBuf>,
    realm_path: Option<PathBuf>,
    backend: R,
) -> ExitCode
where
    R: FnOnce() -> Result<(), Box<dyn std::error::Error>>,
{
    // The realm is resolved before anything else this run creates: a
    // session whose realm configuration is missing or malformed has
    // nothing to serve, and aborting here means no log file, no socket,
    // and no window are left behind by a run that could never work.
    let realms = match load_realms(realm_path.as_deref()) {
        Ok(realms) => realms,
        Err(()) => return ExitCode::FAILURE,
    };
    announce_realms(&realms);

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
    recorder.record(Event::RunStarted {
        pid: std::process::id(),
        core_version: env!("CARGO_PKG_VERSION"),
        consent_policy: match consent {
            ConsentPolicy::Interactive => "interactive",
            ConsentPolicy::AutoApprove => "auto-approve",
        },
    });

    let result = backend();

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
            "realm configured (not started: see run_session for where the spawn is wired)"
        );
    }
}

/// The loud startup announcement the consent policy owes (plan risk R6):
/// auto-approve must never run silently. The parsed policy is consumed by
/// the M1.1 listener wiring (which constructs the `PetitionRegistry` from
/// it); until that wiring lands, nothing at runtime accepts principal
/// connections, so the announcement is the flag's only runtime effect.
fn announce_consent_policy(policy: ConsentPolicy) {
    match policy {
        ConsentPolicy::AutoApprove => tracing::warn!(
            "CONSENT POLICY: AUTO-APPROVE -- every grant petition will be granted \
             as requested with NO human decision. Headless CI and demos only."
        ),
        ConsentPolicy::Interactive => tracing::info!(
            "consent policy: interactive (the consent renderer lands with E7; \
             until then petitions pend and resolve timed_out)"
        ),
    }
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
    use super::*;

    #[test]
    fn nested_flag_selects_nested_mode() {
        assert_eq!(
            parse_args(["--nested"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
    }

    #[test]
    fn headless_flag_defaults_to_1280x800() {
        assert_eq!(
            parse_args(["--headless"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
    }

    #[test]
    fn headless_size_is_parsed() {
        assert_eq!(
            parse_args(["--headless", "--size", "1280x800"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
        assert_eq!(
            parse_args(["--headless", "--size", "640x480"]),
            Ok(Action::RunHeadless {
                size: (640, 480),
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
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
                    consent: ConsentPolicy::AutoApprove,
                    recorder: None,
                    realm: None
                })
            );
        }
        assert_eq!(
            parse_args(["--nested", "--consent=interactive"]),
            Ok(Action::RunNested {
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
    }

    #[test]
    fn recorder_path_parses_both_spellings_and_defaults_to_none() {
        // Omitted, the run uses the default path under the core's runtime
        // directory (resolved at startup, not here) -- `None` is "not
        // given", never "no recorder".
        assert_eq!(
            parse_args(["--headless"]),
            Ok(Action::RunHeadless {
                size: (1280, 800),
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
        for args in [
            vec!["--headless", "--recorder", "/tmp/run.jsonl"],
            vec!["--headless", "--recorder=/tmp/run.jsonl"],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    consent: ConsentPolicy::Interactive,
                    recorder: Some(PathBuf::from("/tmp/run.jsonl")),
                    realm: None
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
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: None
            })
        );
    }

    #[test]
    fn realm_config_path_parses_both_spellings_and_defaults_to_none() {
        // `None` is "not given", never "no realm": omitted, startup
        // resolves the documented default path and still fails if nothing
        // is there.
        for args in [
            vec!["--headless", "--realm", "/etc/vitrin/realm.toml"],
            vec!["--headless", "--realm=/etc/vitrin/realm.toml"],
        ] {
            assert_eq!(
                parse_args(args),
                Ok(Action::RunHeadless {
                    size: (1280, 800),
                    consent: ConsentPolicy::Interactive,
                    recorder: None,
                    realm: Some(PathBuf::from("/etc/vitrin/realm.toml"))
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
                recorder: Some(PathBuf::from("/tmp/n.jsonl")),
                realm: Some(PathBuf::from("/tmp/realm.toml"))
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
            parse_args(["--headless", "--size", "2147483647x1"]),
            Ok(Action::RunHeadless {
                size: (2147483647, 1),
                consent: ConsentPolicy::Interactive,
                recorder: None,
                realm: None
            })
        );
    }

    #[test]
    fn size_without_headless_is_an_error() {
        assert!(parse_args(["--size", "1280x800"]).is_err());
        assert!(parse_args(["--nested", "--size", "1280x800"]).is_err());
    }

    #[test]
    fn help_and_version_win_over_mode() {
        assert_eq!(parse_args(["--nested", "--help"]), Ok(Action::Help));
        assert_eq!(parse_args(["--headless", "--help"]), Ok(Action::Help));
        assert_eq!(parse_args(["--version"]), Ok(Action::Version));
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

    #[test]
    fn duplicate_or_conflicting_mode_is_an_error() {
        assert!(parse_args(["--nested", "--nested"]).is_err());
        assert!(parse_args(["--headless", "--headless"]).is_err());
        assert!(parse_args(["--nested", "--headless"]).is_err());
    }
}
