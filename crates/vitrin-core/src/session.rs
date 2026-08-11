// SPDX-License-Identifier: MPL-2.0
//! The runtime wiring: the session state every backend's event loop carries,
//! and the sources that make the capability kernel reachable from the wire
//! (P1.M1.1, issue #77).
//!
//! Before this module existed, `vitrind` was a compositor with a capability
//! kernel bolted to the side of it and no wire between them: the principal
//! protocol server, the petition registry, the grant table and the shim
//! server were each exercised only by their own tests, because nothing at
//! runtime accepted a connection or serviced a socketpair. This module is
//! that wire.
//!
//! # The shape, and why the backend closure had to be inverted
//!
//! calloop fixes **one** state type per `EventLoop`, and every source the
//! runtime needs — the listener, each principal connection, the shim
//! socketpair, the expiry sweep — must be inserted into the same loop the
//! backend already drives. So the kernel's state has to live in the same
//! struct as the backend's.
//!
//! [`crate::run_session`] used to hand the backend a `FnOnce() -> Result<()>`
//! that constructed its own loop and its own state and told nobody. It now
//! builds a [`RuntimeSeed`] — the listener and the kernel's long-lived state
//! — and hands it *into* the backend, which owns it as a [`Runtime`] field on
//! its own state struct and hands the [`Recorder`] back on the way out so the
//! run's footer is still written by the same code that opened the log.
//!
//! Two traits keep the wiring backend-agnostic:
//!
//! - [`Presenter`] is everything the runtime needs from a backend: the scene
//!   to commit into, the size it composes at, a recomposite, and a readback
//!   of the latest completed realm view. Both backends serve captures: the
//!   headless backend reads back its retained pixman framebuffer, and the
//!   nested backend — though EGL/GLES-bound with no retained image of the
//!   presented window — composes the bare realm view on the CPU from its
//!   scene ([`Presenter::view_rgba`] → [`Scene::compose`], P1.3.8/issue #116).
//!   The two produce byte-identical bytes for the same scene, so `--nested`
//!   answers `vitrin_view.frame_ready` (and, since the same `no_surface`
//!   judgement gates actuation, an agent can observe *and* actuate nested)
//!   exactly as `--headless` does, the overlay excluded from both by
//!   `Scene::compose` sitting upstream of the output-stage fork.
//! - [`RuntimeHost`] is the backend's state type, split into its runtime half
//!   and its presentation half. The split is the whole point: building a
//!   [`ServerCtx`] borrows the petition registry, the grant table and the
//!   recorder mutably *while* the realm view borrows the presenter's cached
//!   readback, and those two halves have to be provably disjoint fields. The
//!   alternative — a `RefCell` — was rejected: a `BorrowMutError` inside a
//!   compositor dispatch is a hang, and this core's single-threaded
//!   discipline is structural rather than checked at runtime.
//!
//! # Coalesced presentation (the trap `backend::headless`'s test warns about)
//!
//! `backend/headless.rs` carries a reference wiring in a test that
//! composites synchronously on every latched commit, under an explicit
//! "TEST-ONLY SHAPE" warning. The warning is about amplification: after one
//! legal `attach`, a bare repaint `commit` is a 12-byte wire message that
//! stays legal forever, so compositing per commit sells a hostile shim a
//! full-output composite — megabytes of write traffic plus a readback, on the
//! single-threaded loop every *agent* also shares — for twelve bytes.
//!
//! So the runtime never composites in dispatch. A latched commit sets
//! [`Runtime::dirty`] and nothing else; [`post_dispatch`] runs once per
//! calloop dispatch round and does at most one redraw, one readback refresh,
//! and one [`ShimServer::presented`] call — which sends *every* owed
//! `frame_done`, one per commit, in commit order. N commits in one round
//! therefore cost one composite and still produce N frame callbacks: the
//! amplification is gone and the shim's pacing is untouched.
//!
//! # Why connections need an outbox
//!
//! A registered connection can normally only be written to from inside its
//! own dispatch callback (`vitrin_ipc::reply` needs the `NoIoDrop` guard the
//! callback is handed). Two things the core must send have no inbound
//! message to ride:
//!
//! - **To the shim**: routed seat events and `frame_done`. The shim sits
//!   blocked in `recv` — that is what waiting for input *is* — and both are
//!   caused by other peers or by the compositor's own cadence. Deferring
//!   them to its next readiness would deliver input only to shims that did
//!   not need any.
//! - **To a principal**: a petition's deferred [`Resolution`]. A `timed_out`
//!   terminal is produced by a timer, and a human's approval by a click on
//!   the consent surface — neither is a reply to anything the agent sent, and
//!   an agent awaiting consent is by definition idle.
//!
//! Hence [`vitrin_ipc::Outbox`]: a queue plus a wakeup fd the connection's
//! own source owns, so a push from outside dispatch schedules the dispatch
//! that performs it. Nothing bypasses the transport's backpressure rules —
//! the drain goes through the same `send_or_queue` as every reply — the
//! outbox only supplies the occasion to write.

use std::collections::BTreeMap;
use std::error::Error;
use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use vitrin_ipc::{
    Connection, ConnectionEvent, ConnectionSource, Listener, ListenerEvent, ListenerSource, Outbox,
};

use crate::capture::RealmViewFrame;
use crate::consent::grab::ConsentGrab;
use crate::consent::ConsentSurface;
use crate::dmabuf::DmabufImporter;
use crate::enforcement::{LaunchAsk, LaunchRefusal, LayoutAct, LayoutMode};
use crate::grants::{GrantTable, RealmId};
use crate::identity::StaticVerifier;
use crate::input::{InputRouter, PhysicalPresenceMap, PreemptionHook, SeatInput};
use crate::lifecycle::{
    DeathCause, Hangup, RealmLifecycle, RealmTeardown, RetainedOutput, ShutdownTiming,
};
use crate::petitions::{ConnectionId, PetitionRegistry, PromptRoute, Resolution};
use crate::principal::{PrincipalServer, ServerCtx};
use crate::realm::RealmRegistry;
use crate::recorder::Recorder;
use crate::scene::Scene;
use crate::shim::ShimServer;
use crate::spawn::{self, SpawnPaths};

/// How often the two advisory expiry sweeps run.
///
/// Both sweeps are **advisory and never load-bearing**: the enforcement
/// chokepoint re-checks a grant's expiry at use time, and a petition past its
/// deadline is refused by the same predicate the sweep uses. The sweeps exist
/// so a deadline the *client* is waiting on produces its wire terminal
/// (`vitrin_grant.resolved(timed_out)`) and its flight-recorder entry
/// promptly, rather than only when someone happens to touch the row.
///
/// One second is chosen against that job. It bounds how late a `timed_out`
/// can reach an agent that is doing nothing else, which is the only case
/// where lateness is observable at all; anything finer would wake an idle
/// core to walk two empty maps for no benefit, and `record_expiry_sweep`
/// deliberately writes nothing for an empty sweep so a quiet session's log
/// stays quiet.
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// How long an accepted connection may stay unauthenticated before the core
/// closes it (protocol conventions 7.1, and the seam
/// [`crate::principal`]'s module docs names for this wiring).
///
/// Without it, an unauthenticated peer costs a file descriptor, a
/// `PrincipalServer`, and a `ConnectionId` for as long as it cares to hold
/// them, and a peer that merely connects in a loop is a resource-exhaustion
/// primitive that needs no protocol knowledge at all. Ten seconds is far
/// longer than a `hello` round trip on a local socket and far shorter than
/// any interactive timeout, so it can only ever fire on a peer that is not
/// trying to authenticate.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

/// What [`crate::run_session`] builds before any backend exists, and hands
/// into the backend to become the loop-resident [`Runtime`].
///
/// It is a separate type from [`Runtime`] for one reason: the input router's
/// preemption hook is backend-specific (nested stacks a consent gate over the
/// dead-man watcher; headless has no physical input device at all), so the
/// router cannot be built until the backend is. The seed carries everything
/// that *can* be built first — including the [`Listener`], bound early on
/// purpose so a second core against the same runtime tree refuses before it
/// has created anything.
pub(crate) struct RuntimeSeed {
    pub listener: Listener,
    pub verifier: StaticVerifier,
    pub petitions: PetitionRegistry,
    pub grants: GrantTable,
    pub realms: RealmRegistry,
    pub recorder: Recorder,
    /// The core-known shim binary the spawn manager execs to hold fd 3, which
    /// in turn execs the realm's app (issue #103). Resolved by `main` from
    /// `--shim` (or its default) and carried here so [`start_realm`] can build
    /// the spawn's [`SpawnPaths`] from it.
    pub shim: PathBuf,
    /// This session's trusted consent indicator (issue #85), minted at startup
    /// before the listener accepts anyone. Carried here — the one thing
    /// `run_session` hands the backend — so a single ceremony establishes it
    /// and whichever backend runs frames its prompts in it. `Copy`, so reading
    /// it out for the [`ConsentSurface`] takes nothing from the seed the
    /// runtime consumes.
    pub indicator: crate::consent::TrustedIndicator,
    /// Optional `--capture-dump PATH` target (P1.8.5, issue #107): when set,
    /// every redraw also writes each live realm's freshly composited
    /// realm-view readback — the raw RGBA that realm's capture cache entry is
    /// refreshed from — to **`PATH.<realm-id>`**. It is the **core-internal
    /// capture**, taken before `render_frame`, the memfd, the wire and the
    /// SDK decode ever run, so an agent's `observe()` frame can be compared
    /// against it to prove the grant/capture path adds no distortion against
    /// a real app. A diagnostic knob, not a wire feature; `None` in every
    /// ordinary run.
    ///
    /// **The realm suffix is not cosmetic** (WS-E.1.3, issue #209). While a
    /// session held one realm, `PATH` unambiguously named the one view the
    /// M1.3 fidelity gate compares against. With N realms an unqualified
    /// dump names *a* view and the gate's ground truth becomes a guess, so
    /// nothing is written to the bare `PATH` at all — every dump names the
    /// realm it is of. See [`capture_dump_path`].
    pub capture_dump: Option<PathBuf>,
    /// The audited `--screenshot-dir` (WS-E.2.4, issue #216), already opened
    /// and validated by `main`'s argument parser, or `None` when the operator
    /// passed no flag -- in which case the screenshot key writes nothing.
    ///
    /// Carried as an **open descriptor**, not a path: by the time it reaches
    /// here the only thing that can be done with it is write a file the core
    /// itself named, into the directory the core itself resolved before any
    /// client existed.
    pub screenshot_dir: Option<crate::screenshot::ScreenshotDir>,
}

/// Everything one running session owns that is not presentation, living in
/// the backend's calloop state type.
///
/// Generic over the preemption hook so the session's input router is the
/// backend's own router — the one the consent grab and the dead-man watcher
/// are already stacked into — rather than a second router the two could drift
/// apart from. One router serves however many realms the session holds, with
/// one seat's state per realm inside it; which realm a delivery reaches is
/// [`seat_target`]'s answer for the *human's* input and the grant's own realm
/// for an agent's ([`route_seat`]).
pub(crate) struct Runtime<H: PreemptionHook> {
    /// The capability kernel's long-lived state. Grouped in its own struct so
    /// a [`ServerCtx`] can be built from one disjoint field borrow while the
    /// realm half and the presenter are borrowed separately.
    pub kernel: Kernel,
    /// The core socket, until [`install`] moves it into a [`ListenerSource`].
    listener: Option<Listener>,
    /// Live principal connections, keyed by the id the petition registry
    /// minted at accept — which is also how a deferred [`Resolution`] finds
    /// its way back to the connection that petitioned.
    conns: BTreeMap<ConnectionId, PrincipalConn>,
    /// **The live shim sessions**, keyed by the realm id the wire names —
    /// one entry per realm [`start_realm_in`] has forked (WS-E.1.2, issue
    /// #208; this was a bare `Option<RealmRuntime>` while a session held
    /// exactly one realm).
    ///
    /// Distinct from [`Kernel::realms`], and the two answer different
    /// questions: the *registry* answers "does this realm exist, and does it
    /// admit petitions" and holds every configured realm from load to the
    /// end of the session; this map answers "is there a shim session for it
    /// right now" and holds only realms that were forked and have not been
    /// torn down. An `Exited` realm is in the registry and, after
    /// [`shutdown_realm`], not here.
    ///
    /// Empty until the fork, and the ordering behind that is the module's
    /// central invariant rather than an initialization detail: [`install`]
    /// must have registered each shim socketpair's source *before* its fork,
    /// because a shim whose connection nothing services blocks on `configure`
    /// forever. Spawning first and wiring after is a permanent, silent hang.
    pub realms: BTreeMap<RealmId, RealmRuntime>,
    /// The session's input router: chokepoint-admitted agent actuations and
    /// (nested) physical input converge here before delivery to the shim,
    /// each addressed by its own rule and each landing in its own realm's
    /// seat state (WS-E.1.6).
    pub router: InputRouter<H>,
    /// The core-known shim binary the spawn manager execs (issue #103), from
    /// the seed. [`start_realm`] reads it to build the spawn's [`SpawnPaths`];
    /// tests that call [`start_realm_in`] with explicit paths never consult it.
    shim: PathBuf,
    /// Set by a latched commit, cleared by [`post_dispatch`]. The whole of
    /// the anti-amplification defence the module docs describe.
    pub dirty: bool,
    /// **Each live realm's** latest completed view, refreshed at redraw time
    /// and never on the capture path, so `capture` stays the pure read of
    /// "what the compositor last finished" that keeps goldens deterministic.
    ///
    /// Keyed by realm since WS-E.1.3 (issue #209), and that is decision 1
    /// made storable: with one entry for the session, an `observe` grant over
    /// realm A returned realm B's pixels the instant B committed. A realm
    /// with no entry has no frame to serve and its grant meets the
    /// chokepoint's `no_surface` refusal.
    ///
    /// [`post_dispatch`] both fills and *prunes* this map: a realm with no
    /// live shim session has its entry removed, so a dead realm's last frame
    /// cannot sit here waiting for a predicate to fail open. That is the same
    /// defence-in-depth posture `RetainedOutput::scrub_retained_frame` takes
    /// for the headless framebuffer, applied to the capture cache.
    view_cache: BTreeMap<RealmId, Vec<u8>>,
    /// The `--capture-dump PATH` diagnostic target from the seed (P1.8.5),
    /// `None` in every ordinary run. When set, [`post_dispatch`] mirrors each
    /// realm's refreshed [`Self::view_cache`] entry to `PATH.<realm-id>` —
    /// see [`RuntimeSeed::capture_dump`] and [`capture_dump_path`].
    capture_dump: Option<PathBuf>,
    /// This session's monotonic zero, for presentation timestamps.
    epoch: Instant,
}

/// The capability kernel's state: one verifier, one petition registry, one
/// grant table, one realm registry, one recorder, for the whole session.
pub(crate) struct Kernel {
    /// One verifier serves every connection, loaded once at startup. Loading
    /// it per connection would open a TOCTOU window against the very registry
    /// the R6 auto-approve guard audited at startup — a guard auditing a
    /// document nobody reads.
    pub verifier: StaticVerifier,
    pub petitions: PetitionRegistry,
    pub grants: GrantTable,
    pub realms: RealmRegistry,
    pub recorder: Recorder,
    /// Physical-input presence **per realm**, fed at the router's hook point;
    /// the chokepoint's `preempted` judgement reads it (WS-E.1.6, issue #212 —
    /// it was one session-wide tracker, which refused an agent in realm B
    /// because a human was typing in realm A).
    ///
    /// **The router's own map, not a second one.** [`Runtime::new`] takes this
    /// handle out of the router it is handed
    /// ([`InputRouter::presence`]) rather than minting one, so a kernel that
    /// judges `preempted` against a map nothing writes is unconstructible. It
    /// was constructible until issue #212's review, and every shipped
    /// `vitrind` was in exactly that state: `PresenceHook` was an optional
    /// member of the hook stack and no backend included it, so `preempted`
    /// could not fire while the book described it as live behaviour.
    pub presence: std::rc::Rc<std::cell::RefCell<PhysicalPresenceMap>>,
    /// **The human's attention signal** (WS-E.1.7, issue #232): the short,
    /// single-use window a core-owned Super tap opens, in which the two layout
    /// verbs are not refused `preempted`.
    ///
    /// **The hook's own signal, not a second one.** [`Runtime::new`] takes this
    /// handle out of the router it is handed
    /// ([`InputRouter::attention`]) rather than minting one, so a kernel that
    /// judges the exemption against a signal nothing opens is unconstructible
    /// — the mistake presence really did ship with until issue #212's review.
    /// A backend whose stack carries no
    /// [`AttentionHook`](crate::attention::AttentionHook) gets a detached
    /// signal that never opens, which is the honest answer for a build with no
    /// physical input device rather than a special case in the chokepoint.
    pub attention: std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>,
    /// **The human's clipboard chords** (WS-E.2.1, issue #213): the queue the
    /// hook writes a gesture into and [`drain_clipboard_gestures`] drains.
    ///
    /// Taken out of the router on the same terms as `attention` above, and for
    /// the same reason: a kernel draining a signal the hook does not write is a
    /// clipboard whose chords silently do nothing, with every test green. A
    /// backend whose stack carries no
    /// [`ClipboardHook`](crate::clipboard::ClipboardHook) gets a detached
    /// signal nothing ever queues into.
    pub clipboard: std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>,
    /// **The session's one clipboard slot** (WS-E.2.1, issue #213, D-024).
    ///
    /// Kernel state rather than router state, because it holds application
    /// bytes and the router must never learn about authority or about realms.
    /// It is the one piece of session state the dead-man switch's grant sweep
    /// cannot reach, which is why [`crate::deadman::apply`] clears it
    /// explicitly.
    pub clipboard_slot: crate::clipboard::ClipboardSlot,
    /// **The human's screenshot chord** (WS-E.2.4, issue #216): the queue the
    /// hook writes a gesture into and [`drain_screenshot_gestures`] drains.
    ///
    /// Taken out of the router on the same terms as `clipboard` above, and for
    /// the same reason.
    pub screenshot: std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>,
    /// **Where a screenshot goes**, or `None` when the operator passed no
    /// `--screenshot-dir` and the feature is therefore off (WS-E.2.4).
    ///
    /// Kernel state rather than router state for [`Self::clipboard_slot`]'s
    /// reason and one more: it is the *only* place in the session from which a
    /// screenshot can be written, and the type it holds has no method that
    /// takes a path. The dead-man switch deliberately does **not** clear it --
    /// the off-switch destroys authority, and a human's own screenshot key is
    /// not authority.
    pub screenshot_dir: Option<crate::screenshot::ScreenshotDir>,
}

/// The realm's live shim session: the protocol server, the out-of-band send
/// handle for its connection, and the lifecycle that owns the process.
pub(crate) struct RealmRuntime {
    /// The realm's process, runtime tree and `flock`, from spawn to grave.
    ///
    /// Everything that ends a realm goes through here — EOF, a transport
    /// fault, a shim protocol violation, `SIGCHLD`, and the shutdown ladder
    /// — because the transition is latched inside it and the latch is what
    /// makes "a realm dies once" true across five independent observers.
    pub life: RealmLifecycle,
    /// `Option` because [`ShimServer::connection_closed`] consumes the server
    /// by value, and because the realm-teardown funnel this becomes takes
    /// `&mut Option<ShimServer>` for the same reason.
    pub server: Option<ShimServer>,
    /// Pushes seat events and `frame_done` at a shim that is not talking.
    pub outbox: Outbox,
    /// **This realm's arrangement** (WS-E.1.4, issue #210): whether its view
    /// size tracks the output's or is left alone.
    ///
    /// Born [`LayoutMode::Fullscreen`], because that is the state
    /// [`start_realm_in`] already puts a realm in — it configures every shim
    /// with the output's size — and a field whose initial value did not
    /// describe the world would be a lie a later `set_fullscreen(fullscreen)`
    /// would silently correct.
    ///
    /// Lives here rather than in the scene set because it is a fact about
    /// what the *shim* was told, not about pixels: the scene composes the
    /// same way in both arrangements, and the whole difference is whether
    /// `configure` is re-sent. Dying with the realm is correct — an exited
    /// realm is never relaunched (IDL `vitrin_launcher.launch`), so there is
    /// no arrangement to carry forward.
    ///
    /// **Written by [`apply_layout`], read by [`apply_output_resize`]**, and
    /// the read is the whole reason the field exists rather than the write.
    /// `set_fullscreen` normatively promises that a fullscreen realm's view
    /// size *tracks* the output's — `configure` on entry "and again whenever
    /// the output resizes while the realm is in it" — so a resize has to know
    /// which realms are in it. A write-only version of this field would leave
    /// that second half stated on four surfaces and implemented on none.
    pub arrangement: LayoutMode,
}
// The shim connection's calloop registration is deliberately **not** a field
// here. Removing that token is how the core hangs up on a live shim — the
// `ConnectionSource` owns the `Connection`, so removal drops it and that drop
// is the shim's EOF, rung 0 of the shutdown ladder — and the ladder lives in
// `RealmLifecycle`. So the token is handed to `Hangup::registered` at adoption
// and owned there, giving the hangup exactly one owner. Keeping a copy here
// as well would invite a second hangup path that skips the death latch.

/// One accepted principal connection.
struct PrincipalConn {
    server: PrincipalServer,
    /// The connection's own source, so a **core-initiated** close (a protocol
    /// violation) can remove it. Transport-initiated closes remove the source
    /// themselves; see [`dispatch_principal`] for why that asymmetry must not
    /// be got backwards.
    token: RegistrationToken,
    /// Out-of-dispatch sends: deferred petition resolutions.
    outbox: Outbox,
    /// The unauthenticated-phase deadline, disarmed the moment
    /// [`PrincipalServer::is_bound`] first returns true.
    deadline: Option<RegistrationToken>,
}

/// What the runtime needs from a presentation backend.
///
/// Deliberately small: the runtime must not be able to reach into a backend's
/// renderer, and a backend must not be able to reach into the grant table.
/// Everything here is either "what the shim commits into" or "what an
/// authorized capture reads back".
/// Whether a [`Presenter::redraw`] actually put a frame on the output, or
/// merely asked an external frame clock to do so later.
///
/// This distinction exists because the two backends own their cadence
/// differently, and every realm's `frame_done` must follow the *composite*
/// rather than the request for one. Headless composites synchronously — the
/// completed composite is the output cadence. Nested hands the request to
/// the host compositor and is told later, via `WinitEvent::Redraw`.
///
/// Collapsing the two (emitting `frame_done` as soon as a redraw was
/// *requested*) is silently wrong rather than loudly wrong: a shim that
/// paces itself on `frame_done` — which is the whole point of the callback —
/// is handed a fresh permit per dispatch round with no composite in between,
/// so it stops throttling and spins as fast as the loop will dispatch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presentation {
    /// The composite finished inside this `redraw` call. Any owed
    /// `frame_done` is due now.
    Completed,
    /// The composite was requested on an external frame clock and has not
    /// happened yet. The backend owes [`emit_presented`] when it does.
    Scheduled,
}

pub(crate) trait Presenter {
    /// **This realm's** scene — what [`ShimServer::handle_message`] commits
    /// into for the realm whose connection it is servicing, minted empty on
    /// first use.
    ///
    /// Keyed by realm since WS-E.1.3 (issue #209). Before that there was one
    /// scene for the whole session and every realm's shim committed into it,
    /// so the last committer owned the only surface and an `observe` grant
    /// over realm A served realm B's pixels the instant B painted. Naming
    /// the realm here is what makes that impossible rather than merely
    /// unlikely; [`RealmScenes::bound`] is the deliberately differently-named
    /// accessor the *output* composites through, so a capture path reaching
    /// for the output's scene reads as the mistake it is.
    ///
    /// [`RealmScenes::bound`]: crate::scene::RealmScenes::bound
    fn scene_mut(&mut self, realm: &RealmId) -> &mut Scene;
    /// This realm's scene for reading, or `None` when it has none — the
    /// realm has never committed and has never held the output.
    ///
    /// Split from [`Self::scene_mut`] because the runtime's one caller is
    /// [`RealmLifecycle::view_is_live`], the single fact behind every
    /// `no_surface` refusal, and asking that question must not *create* a
    /// scene for a realm that has none. `None` and "an empty scene" give the
    /// same answer here; the difference is that this one cannot mint state
    /// from a read.
    ///
    /// [`RealmLifecycle::view_is_live`]: crate::lifecycle::RealmLifecycle::view_is_live
    fn scene(&self, realm: &RealmId) -> Option<&Scene>;
    /// The realm the output is bound to, or `None` before any realm has
    /// attached.
    fn focused(&self) -> Option<&RealmId>;
    /// Bind the output to `realm`: the mechanism, not the policy.
    ///
    /// **Who** may call this was WS-E.1.4's question (issue #210) and is now
    /// answered: a principal holding the `layout_focus` grant verb, through
    /// the `vitrin_layout_focus` facet, the enforcement chokepoint and
    /// [`apply_layout`]. **Where the human's input goes as a result** is
    /// answered on the same line — [`seat_target`] follows this binding, which
    /// is D-018(2)'s fifth ordering rule. Where an *agent's* actuation goes is
    /// deliberately **not** answered here: since WS-E.1.6 (issue #212) it goes
    /// to the realm its own grant names, so moving the output moves the human
    /// and nobody else.
    ///
    /// Two other callers remain and neither is a policy:
    /// [`start_one_realm_in`] at first attach and
    /// [`rebind_output_after_death`] when the bound realm dies, both taking
    /// the first still-serving realm in id order — the placeholder that
    /// answers when no client has chosen.
    fn bind_output(&mut self, realm: &RealmId);
    /// Bind the output to **no realm**, so it composites the deterministic
    /// background and [`Self::focused`] answers `None`.
    ///
    /// The way out of "bound to a realm that is gone", and nothing more.
    /// [`rebind_output_after_death`] is the one caller and it reaches for this
    /// only when no realm is still serving — the moment one is,
    /// [`Self::bind_output`] moves the output there instead. Separate from
    /// `bind_output` rather than folded into it as an `Option` argument so
    /// that "which principal may bind, under what verb" stays a question about
    /// one named verb — which is exactly the shape the answer took: a
    /// `layout_focus` holder binds, and there is no request through which one
    /// unbinds.
    fn unbind_output(&mut self);
    /// The size the realm view composes at: the virtual output for headless,
    /// the host window for nested. One output, so one size, shared by every
    /// realm and handed to every realm's shim at `configure` — decision 3 of
    /// issue #209 keeps it that way (one bound realm, no stacking, no
    /// overlap, no resize). The input router maps view coordinates to
    /// surface coordinates against it.
    fn view_size(&self) -> (u32, u32);
    /// **Record a new output size.** The backend that owns the output has
    /// already changed it — this is the propagation, not the resize itself:
    /// it hands the new size to the scene registry, which bumps every realm's
    /// `layout_generation` (D-018(5)).
    ///
    /// Reached only through [`apply_output_resize`], which is also what
    /// re-configures every fullscreen realm's shim. That pairing is the whole
    /// reason this is a trait method rather than a line in each backend's own
    /// resize handler: a backend that propagated the size without the
    /// re-configure would leave `set_fullscreen`'s normative promise —
    /// `configure` re-sent "whenever the output resizes while the realm is in
    /// it" — false, silently, and only on the backend whose output can
    /// actually resize.
    fn set_view_size(&mut self, size: (u32, u32));
    /// Recomposite. Called at most once per dispatch round, from
    /// [`post_dispatch`] and never from a message callback.
    ///
    /// The returned [`Presentation`] is what gates the realm's `frame_done`,
    /// and it is not decoration: a backend whose frame clock is external
    /// returns [`Presentation::Scheduled`] and owes the runtime an
    /// [`emit_presented`] call when the composite actually lands. Answering
    /// [`Presentation::Completed`] without having composited tells a paced
    /// shim its frame reached a display that never drew it, which is how a
    /// throttled client becomes an unthrottled one.
    fn redraw(&mut self) -> Result<Presentation, Box<dyn Error>>;
    /// **This realm's** latest completed view as tightly packed RGBA8888, or
    /// `None` when there is no view to serve.
    ///
    /// Keyed by realm since WS-E.1.3, and this is the read side of decision
    /// 1: what a capture serves is a function of the realm the *grant*
    /// names. The bound realm's answer is the output's own composition
    /// (headless reads back its retained pixman framebuffer; nested composes
    /// the bare scene on the CPU); a hidden realm's is that realm's scene
    /// composed at the same view size. Both go through [`Scene::compose`], so
    /// the two can no more drift from each other than the two backends can
    /// (P1.3.8), and both are overlay-free because `Scene::compose` is
    /// upstream of the output-stage fork.
    ///
    /// `None` is not a failure and must not be treated as one: it is the
    /// honest answer for a realm with no scene at all, or for a degenerate
    /// view (a minimized nested window, a readback failure). A capture then
    /// meets the chokepoint's existing `no_surface` refusal, which is the
    /// correct outcome — better than a black frame an agent would read as the
    /// realm's actual content.
    fn view_rgba(&mut self, realm: &RealmId) -> Option<Vec<u8>>;
    /// Lend **the dying realm's** scene, the retained framebuffer, and the
    /// dmabuf importer to `f`, all borrowed **together** for the one call.
    ///
    /// A callback rather than a returned tuple — unlike every other
    /// [`Presenter`] method — because the concrete importer a GPU-backed
    /// embedder lends here (P1.3.5, issue #117) wraps a
    /// `renderer: &mut GlesRenderer` field that has nowhere to live except a
    /// local of *this* call: a `dyn DmabufImporter` trait object erases that
    /// concrete type, and a `Box<dyn DmabufImporter + '_>` handed back by
    /// value looks like the obvious fix but is not one — dropck cannot see
    /// through the erased type to know its drop glue is trivial, so it
    /// requires the box's borrowed lifetime to survive strictly past the
    /// box's own drop point, which conflicts with every caller that also
    /// needs the *same* borrowed data (the scene, the backend) free again
    /// afterward. Scoping the concrete importer as this method's own local
    /// and lending it out only as a bare `&mut dyn DmabufImporter` — a
    /// reference has no drop glue regardless of what it points to — sidesteps
    /// that hazard entirely and is why every implementation of both this
    /// method and [`Self::scene_and_importer`] follows the same shape.
    ///
    /// One call rather than separate accessors because [`RealmTeardown`]
    /// holds all three at once: `RealmLifecycle::die` takes the realm's
    /// surface out of the scene, scrubs its last painted frame out of the
    /// retained image, and drops any retained zero-copy GPU content, inside
    /// a single latched transition, so a backend that could only lend one at
    /// a time would make the teardown funnel unbuildable — and the way out
    /// of *that* is a second, unlatched path beside the funnel, which is the
    /// one thing this teardown must not grow.
    ///
    /// `None` for the retained half is the nested backend's honest answer:
    /// it composites into the host compositor's surface and retains no
    /// readable image to scrub. That costs nothing even though nested now
    /// *does* serve captures ([`Self::view_rgba`], P1.3.8): its capture is
    /// composed on demand from the live scene, and the death funnel clears the
    /// realm's surface out of that scene before the next compose — so the dead
    /// realm's pixels are gone by construction, with `view_is_live` gating the
    /// capture shut on top of that. The retained-image scrub is a headless
    /// need: only a *held* framebuffer can keep a dead realm's last painted
    /// frame.
    ///
    /// `None` for the importer half is the headless posture (no GPU
    /// renderer exists at all): there is never any retained zero-copy
    /// content to drop.
    ///
    /// **The scene and the importer handed out are `realm`'s own**
    /// (WS-E.1.3). Before that both were the session's: one scene, so a
    /// realm's death cleared whatever surface happened to be committed — a
    /// sibling's included — and one retained zero-copy slot, so a realm's
    /// death dropped whichever realm's GPU texture happened to be resident.
    /// Both directions were fail-closed, and both are gone: a death takes
    /// exactly the dying realm's surface and exactly its own retained import
    /// (`RealmGpuContent::slot_mut`). The retained *framebuffer* half is
    /// still the **output's** and is scrubbed on any realm's death, which is
    /// over-broad and stays that way on purpose — see [`with_realm_teardown`].
    fn teardown_view<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(
            &'v mut Scene,
            Option<&'v mut dyn RetainedOutput>,
            Option<&'v mut dyn DmabufImporter>,
        ) -> R,
    ) -> R;
    /// Lend **this realm's** scene and a dmabuf importer bound to this
    /// backend's live GPU renderer to `f`, both borrowed **together**, for one
    /// shim dispatch (P1.3.5's zero-copy path, issue #117).
    ///
    /// `realm` is the realm whose shim connection is being dispatched
    /// ([`dispatch_shim`] carries it in the source's callback), so a commit
    /// lands in that realm's own scene and nowhere else.
    ///
    /// A callback for the reason [`Self::teardown_view`]'s docs give in
    /// full: the concrete importer's `renderer: &mut GlesRenderer` field is
    /// constructed fresh inside this call and can only be lent out as a
    /// bare `&mut dyn DmabufImporter` scoped to it, never boxed and handed
    /// back by value.
    ///
    /// One method rather than two accessors for the same reason
    /// [`Self::teardown_view`] is: the scene and the importer are two
    /// disjoint field borrows behind a single embedder type, and a caller
    /// that needs both (every shim message dispatch does) cannot take them
    /// through two separate `&mut self` calls.
    ///
    /// The default is the headless posture: no GPU renderer exists, so
    /// every `kind=dmabuf` commit resolves as the designed
    /// `import_failed` shm fallback, exactly as before this method existed.
    fn scene_and_importer<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(&'v mut Scene, Option<&'v mut dyn DmabufImporter>) -> R,
    ) -> R {
        f(self.scene_mut(realm), None)
    }
    /// Ask the backend to schedule a presentation, for backends whose frame
    /// clock is external (nested: the host compositor's redraw request). The
    /// default is the headless posture, where a completed composite *is* the
    /// output cadence and nothing further needs scheduling.
    fn request_present(&mut self) {}
    /// Hand the backend the **agent-owned** pointer position
    /// ([`InputRouter::agent_pointer`]) for the agent cursor sprite, and say
    /// whether the sprite the next composite would draw actually changed.
    ///
    /// Called once per dispatch round from [`post_dispatch`], *before* the
    /// dirty gate, because an agent moving its pointer is a visible change
    /// with nothing else to announce it: the confined app need not commit a
    /// new frame just because something hovered over it, so without this a
    /// pointer move would show up whenever the scene next happened to change
    /// — the same trap `backend::winit`'s `TextureKey` was written for.
    ///
    /// This is the **drawing** side of the pointer and nothing more. Delivery
    /// to the shim stays one shared position per realm view; see
    /// [`crate::cursor`] for the whole distinction and why the sprite may not
    /// be drawn on [`InputRouter::pointer`].
    ///
    /// **Gated on the bound realm since WS-E.1.3.** [`post_dispatch`] passes
    /// `None` unless the router's currently bound realm is the one the output
    /// is bound to, because the sprite is drawn in *output* coordinates over
    /// the *output's* realm: drawing a hidden realm's pointer would paint a
    /// crosshair at coordinates that mean nothing in the picture the human is
    /// looking at. The consequence — an agent actuating in a hidden realm
    /// draws no sprite, so the human loses the one visual signal D-019 exists
    /// to give them — is real, is not fixed here, and is published in
    /// `docs/book/src/limits.md`.
    ///
    /// `true` means the caller should mark the frame dirty and request a
    /// present. Backends that composite no agent cursor answer `false`
    /// forever, which costs the dispatch round nothing.
    ///
    /// [`InputRouter::agent_pointer`]: crate::input::InputRouter::agent_pointer
    /// [`InputRouter::pointer`]: crate::input::InputRouter
    fn set_agent_cursor(&mut self, pos: Option<(f64, f64)>) -> bool;

    /// Offer this round's **attention-window** state (WS-E.1.7, issue #232):
    /// whether the core should draw the marker that says the human's attention
    /// key is live right now.
    ///
    /// Drawn **beside** the reserved trusted band and never in it, and only on
    /// the human-visible output stage
    /// ([`crate::backend::human_visible_from_view`]) — so, exactly like the
    /// consent card and the trust band, it cannot reach a capture and an agent
    /// cannot observe the human's attention presses through `frame_ready`.
    ///
    /// `true` means the caller should mark the frame dirty and present. The
    /// window closes by *time*, so an utterly idle session can leave the marker
    /// up until the next round; nested redraws at the host's frame cadence, and
    /// the marker asserts nothing about authenticity, so a stale one is a
    /// cosmetic cost rather than a security one. What the human gets from it is
    /// the negative: **a focus change that happened with no marker up was not
    /// theirs.**
    ///
    /// Backends that composite no marker answer `false` forever, which costs
    /// the dispatch round nothing.
    fn set_attention(&mut self, open: bool) -> bool;

    /// Re-sample the **status strip** (WS-E.2.3, issue #215) and say whether
    /// anything it draws changed.
    ///
    /// Unlike [`Self::set_attention`] this takes no value: what the strip shows
    /// is the wall clock, the battery and the realm the *output* is bound to,
    /// and the view is the thing that knows the last of those. So the round
    /// asks the view to look, rather than computing a value the view already
    /// holds half of.
    ///
    /// `now` and `mono` are passed in rather than read here so a test can
    /// advance either independently — the same discipline
    /// [`crate::status::sample::Sampler`] is written to.
    ///
    /// `true` means the caller should mark the frame dirty and present. A
    /// session with `--status` off answers `false` forever **and reads no clock
    /// and opens no file** — the strip's whole cost, syscalls included, is
    /// behind the flag.
    fn refresh_status(&mut self, now: std::time::SystemTime, mono: Instant) -> bool;

    /// **The two pointer-constraint gates only the output's owner can answer**
    /// (WS-E.4.2, issue #222): whether an overlay needs the compositor this
    /// round, and whether the output is active at all.
    ///
    /// One method returning a struct rather than two predicates, for the reason
    /// `crate::input::PresentationGates` is a struct: the same two facts are
    /// read by the reconciler here *and*, on bare metal, by the composite that
    /// decides whether to draw the human's cursor, and a future gate must not
    /// be silently answered by only one of them.
    ///
    /// The default is the headless posture — no overlay exists to raise, no
    /// output to pause — and it is safe rather than merely convenient: both
    /// defaults are the *permissive* values, so a backend that forgot to
    /// override this could only ever leave a constraint active longer than it
    /// should. On every backend CI can run that costs nothing at all, because
    /// `window_pixels` is handed `None` for the human cursor there and there is
    /// no sprite to hide.
    fn output_gates(&self) -> crate::input::OutputGates {
        crate::input::OutputGates::default()
    }
}

/// A backend state type that carries a [`Runtime`], split into provably
/// disjoint halves.
///
/// [`split`](RuntimeHost::split) exists because of the borrow the
/// [`ServerCtx`] construction needs — see the module docs. An implementation
/// is always two field borrows and nothing else; if it ever needs to be
/// cleverer than that, the state struct has grown an aliasing problem that
/// belongs fixed in the struct rather than hidden here.
pub(crate) trait RuntimeHost: Sized + 'static {
    type Hook: PreemptionHook;
    type View: Presenter;

    fn split(&mut self) -> (&mut Runtime<Self::Hook>, &mut Self::View);

    fn runtime(&mut self) -> &mut Runtime<Self::Hook> {
        self.split().0
    }

    /// The loop this state is driven by, for inserting per-connection
    /// sources from inside a dispatch callback.
    fn loop_handle(&self) -> LoopHandle<'static, Self>;

    /// Stop the loop. The runtime reaches for this only on a condition that
    /// makes the session unable to continue (a failed composite), never on
    /// anything a peer can cause: a misbehaving peer kills its own connection
    /// and nothing else.
    fn stop(&mut self, fatal: Option<Box<dyn Error>>);

    /// Service one turn of interactive consent (issue #90): retire stale
    /// prompts, apply the input grab's human decisions, and raise the next
    /// pending petition's prompt. Called once per dispatch round from
    /// [`post_dispatch`], before the dirty gate.
    ///
    /// The default is a **no-op**, which is correct for any backend that
    /// cannot host a prompt: headless has no display to draw a consent card
    /// on and no physical input device to answer it, and the test host has no
    /// grab. Only the nested backend — where the human physically is —
    /// overrides it. `main` refuses `--headless --consent=interactive` at
    /// startup (issue #90 scope 4), so no petition can silently pend under a
    /// backend that could never raise a prompt to answer it.
    fn service_consent(&mut self, _now: Instant) {}

    /// Service one turn of the lock screen (WS-E.2.2, issue #214): raise it if
    /// the session has gone idle, mirror the gate's state onto the surface, and
    /// journal the facts the gate queued. Called once per dispatch round from
    /// [`post_dispatch`], immediately after [`Self::service_consent`] and
    /// before the dirty gate.
    ///
    /// **After consent, deliberately.** A round that raises a prompt and then
    /// locks must end with the lock on top, and — more importantly — the
    /// journal must read in the order the human experienced: the petitioner was
    /// shown a card, and then the session locked with nobody there to answer it.
    ///
    /// The default is a **no-op**, on [`Self::service_consent`]'s terms and for
    /// a sharper reason: a backend with no physical input device could raise a
    /// lock it has no way to dismiss, which is a wedge rather than a
    /// degradation. `main` therefore refuses every `--lock-*` flag with
    /// `--headless` at startup, so no configuration can reach this default with
    /// a lock armed behind it.
    fn service_lock(&mut self, _now: Instant) {}

    /// Service the **screen's own lifecycle** for this round (WS-E.4.3, issue
    /// #223): the idle blank's countdown and cover, and the post-hoc detection
    /// of a suspend that just ended.
    ///
    /// Takes both clocks because it needs both and the round already reads both:
    /// `now` is `CLOCK_MONOTONIC`, which does not advance across a suspend, and
    /// `wall` is `CLOCK_REALTIME`, which does. Their disagreement **is** the
    /// resume detector ([`crate::backend::blank::ResumeWatch`]), and there is
    /// nothing else to detect one with — the core speaks no D-Bus, and libseat
    /// delivers no event for a suspend at all.
    ///
    /// The default is a **no-op**, which is correct and not merely convenient:
    /// only a backend that owns a display controller can power a panel down, and
    /// only a backend whose hook stack contains the lock gate has anything
    /// writing the activity clock a blank is postponed and woken by. `main`
    /// refuses `--blank-idle` on both other backends at startup
    /// ([`crate::backend::blank::BLANK_NEEDS_THE_OUTPUT`]), so no configuration
    /// can reach this default with a blank armed behind it.
    fn service_screen(&mut self, _wall: std::time::SystemTime, _now: Instant) {}
}

impl<H: PreemptionHook> Runtime<H> {
    /// Build the loop-resident runtime from the seed and the backend's own
    /// input router.
    pub fn new(seed: RuntimeSeed, router: InputRouter<H>) -> Self {
        // Before the router moves into the struct: the kernel's presence map
        // *is* the router's, so there is no wiring step a backend can skip.
        let presence = router.presence();
        // ...and the attention signal, the same way and for the same reason
        // (WS-E.1.7). `None` when this backend's stack carries no
        // `AttentionHook` -- a plain headless run, which has no physical input
        // device to press a chord on -- in which case the kernel gets a signal
        // nothing ever writes, so the chokepoint's exemption arm is the same
        // code in every build and "this backend has no attention key" is a fact
        // about the hook stack rather than about the enforcement path.
        let attention = router.attention().unwrap_or_else(|| {
            std::rc::Rc::new(std::cell::RefCell::new(
                crate::attention::AttentionSignal::detached(),
            ))
        });
        // ...and the clipboard chord queue, the same way and for the same
        // reason (WS-E.2.1). A detached signal is the honest answer for a
        // backend with no physical input device: the slot still exists and the
        // chokepoint-independent clipboard code is the same in every build,
        // and "this backend has no clipboard chord" stays a fact about the hook
        // stack.
        let clipboard = router.clipboard().unwrap_or_else(|| {
            std::rc::Rc::new(std::cell::RefCell::new(
                crate::clipboard::ClipboardSignal::detached(),
            ))
        });
        // ...and the screenshot chord queue (WS-E.2.4), on identical terms. A
        // detached signal is the honest answer for a backend with no physical
        // input device, and it is *also* what a session with no
        // `--screenshot-dir` gets to drain: the two absences are separate facts
        // -- "this build can press the key" and "this session has somewhere to
        // put the file" -- and collapsing them would make one flag mean both.
        let screenshot = router.screenshot().unwrap_or_else(|| {
            std::rc::Rc::new(std::cell::RefCell::new(
                crate::screenshot::ScreenshotSignal::detached(),
            ))
        });
        let RuntimeSeed {
            listener,
            verifier,
            petitions,
            grants,
            realms,
            recorder,
            shim,
            // Presentation state, not kernel state: the backend read it out of
            // the seed (by `Copy`) into its `ConsentSurface` before handing the
            // rest here, so the runtime deliberately drops it.
            indicator: _,
            capture_dump,
            screenshot_dir,
        } = seed;
        Self {
            kernel: Kernel {
                verifier,
                petitions,
                grants,
                realms,
                recorder,
                presence,
                attention,
                clipboard,
                clipboard_slot: crate::clipboard::ClipboardSlot::new(),
                screenshot,
                screenshot_dir,
            },
            listener: Some(listener),
            conns: BTreeMap::new(),
            realms: BTreeMap::new(),
            router,
            shim,
            dirty: false,
            view_cache: BTreeMap::new(),
            capture_dump,
            epoch: Instant::now(),
        }
    }

    /// Give the recorder back to [`crate::run_session`], which opened it and
    /// owes it a footer.
    ///
    /// Consuming `self` is what puts "the runtime is over" before "the log is
    /// closed": the connections, the shim server and the grant table all drop
    /// here, so nothing can still be recording when `run_ended` is written.
    ///
    /// # Shutdown is the fourth close path, and it tears down like the others
    ///
    /// [`close_principal`] handles the three close paths a *running* loop
    /// sees (peer EOF, transport fault, core-initiated close after a
    /// protocol violation). A core that stops with connections still open —
    /// `SIGTERM` on a live session, which is the ordinary way a session ends
    /// — is the fourth, and it used to be the one that skipped teardown
    /// entirely: `self.conns` was dropped silently, so an agent holding a
    /// grant at shutdown left no `grant_removed` and no `connection_teardown`
    /// in the log.
    ///
    /// That mattered for the record rather than for the authority. The grant
    /// table dies with the process either way, so nothing survived to be
    /// exercised — but the flight recorder is the artifact that has to
    /// reconstruct the session afterwards, and a log whose last word on a
    /// grant is `petition_resolved` says that grant was still live when the
    /// log ended. It could not say whether the core released it or lost track
    /// of it. Tearing down here makes every grant's end explicit in the one
    /// place that outlives the process.
    ///
    /// Ordering: this runs while the recorder is still open and before
    /// `run_session` writes `run_ended`, which is the only window where these
    /// entries can land in the run they belong to. Source removal is
    /// deliberately not attempted — the loop has already stopped, and there
    /// is no registration left to leak.
    pub fn into_recorder(mut self) -> Recorder {
        self.teardown_open_connections();
        self.kernel.recorder
    }

    /// Run [`PrincipalServer::teardown`] for every connection still open,
    /// emptying the table.
    ///
    /// Split out of [`into_recorder`](Self::into_recorder) rather than
    /// inlined so the shutdown close path is reachable from a test without
    /// consuming the runtime — the same reason it is a named path and not a
    /// `Drop` impl. A `Drop` could not record anything anyway: it would run
    /// after `run_session` had taken the recorder back.
    fn teardown_open_connections(&mut self) {
        let open = std::mem::take(&mut self.conns);
        for (id, mut conn) in open {
            tracing::debug!(%id, "tearing down principal connection at shutdown");
            conn.server.teardown(
                &mut self.kernel.petitions,
                &mut self.kernel.grants,
                &mut self.kernel.recorder,
            );
        }
    }

    /// Apply one completed dead-man chord to this session's authority
    /// (P1.7.3): revoke every grant, deny every pending petition, seal the
    /// table so a decision in flight cannot hand the authority back, and tell
    /// each petitioner over the wire.
    ///
    /// This is the call the nested backend's `on_trigger` could not make
    /// before the runtime existed — there was no grant table in the process
    /// to revoke, so a completed chord logged loudly and did nothing. The
    /// delivery loop is not optional: `apply` deliberately does not record
    /// the denials it produces, because the delivery funnel is where this
    /// codebase records resolutions, and skipping it would leave agents
    /// waiting forever on a petition a human already answered with the
    /// off-switch.
    pub fn apply_dead_man(&mut self, trigger: &crate::deadman::Trigger, now: Instant) {
        let effect = crate::deadman::apply(
            trigger,
            &mut self.kernel.grants,
            &mut self.kernel.petitions,
            &self.kernel.attention,
            &mut self.kernel.clipboard_slot,
            &mut self.kernel.recorder,
            now,
        );
        let revoked = effect.revoked.len();
        let denied = effect.denied.len();
        // **And every pointer constraint in the session** (WS-E.4.2, issue
        // #222), which the grant sweep above cannot and must not reach: a
        // constraint is asked for by the confined APP over its shim connection
        // and is derived from no grant row, so `revoke_principal` never sees
        // one. The precedent for "a thing that is not a grant but must still
        // go" is one line inside `deadman::apply` — the clipboard slot, cleared
        // there for exactly this reason.
        //
        // Session-wide, deliberately: a constraint in a realm the chord was not
        // held over goes too. The switch is session-wide by construction and a
        // locked pointer anywhere is part of what the human is taking back.
        let withdrawn = self.router.constraints().borrow_mut().withdraw_all();
        let constraints = withdrawn.len();
        send_constraint_verdicts(&self.realms, withdrawn);
        for resolution in effect.denied {
            deliver(self, resolution, now);
        }
        tracing::warn!(
            chord = trigger.chord,
            held_ms = trigger.held.as_millis(),
            revoked_grants = revoked,
            denied_petitions = denied,
            withdrawn_pointer_constraints = constraints,
            "dead-man chord completed: every grant in this session is revoked and the grant \
             table is sealed"
        );
    }

    /// Live principal connections — for tests and for shutdown accounting.
    /// Test-only, and genuinely so: no production path asks how many
    /// connections are open — the runtime acts on connections by id, never in
    /// aggregate. It exists because the teardown tests assert the map empties,
    /// which is the observable that distinguishes "the grant rows went" from
    /// "the connection went and took its rows with it".
    ///
    /// The attribute is verified rather than assumed: deleting it warns.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn connection_count(&self) -> usize {
        self.conns.len()
    }
}

/// Insert every runtime source into the backend's loop: the core socket's
/// listener and the advisory expiry sweep.
///
/// Call this **before** anything spawns a realm. A shim spawned into a loop
/// that is not yet servicing its socketpair wedges permanently: it blocks on
/// `configure` and then on the core's replies, with no timeout anywhere on
/// its side, and the only thing that ever breaks the wedge is the core
/// closing the connection. That ordering is the whole reason issue #31 left
/// `spawn_realm` uncalled.
pub(crate) fn install<H>(
    handle: &LoopHandle<'static, H>,
    runtime: &mut Runtime<H::Hook>,
) -> Result<(), Box<dyn Error>>
where
    H: RuntimeHost,
{
    let listener = runtime
        .listener
        .take()
        .expect("install runs exactly once per session");
    let socket = listener.socket_path().to_path_buf();
    handle.insert_source(
        ListenerSource::new(listener)?,
        |event, (), host: &mut H| match event {
            ListenerEvent::Incoming(conn) => accept_principal(conn, host),
            // Never fatal: a transient `accept(2)` failure (EMFILE and
            // friends) costs one connection, and a core that tore its loop
            // down over it would convert a load spike into an outage.
            ListenerEvent::AcceptError(err) => {
                tracing::warn!(%err, "accept failed; still listening");
            }
        },
    )?;
    tracing::info!(socket = %socket.display(), "listening for principal connections");

    handle.insert_source(
        Timer::from_duration(SWEEP_INTERVAL),
        |_now, _, host: &mut H| {
            sweep(host);
            TimeoutAction::ToDuration(SWEEP_INTERVAL)
        },
    )?;
    tracing::debug!(
        interval_ms = SWEEP_INTERVAL.as_millis(),
        "expiry sweeps armed (advisory: authority is re-checked at use time)"
    );
    Ok(())
}

/// How often a `--status` session wakes the loop so the strip's clock can move.
///
/// One second. The strip shows `HH:MM` with no seconds
/// ([`crate::status::sample::ClockReading`]), so this is not a repaint cadence:
/// it is the bound on how *stale* the displayed minute may be, and
/// [`post_dispatch`] marks the frame dirty only on the minute that actually
/// rolls over. The cost is one timerfd wakeup per second while the strip is on,
/// and **nothing at all when it is off** — [`arm_status_tick`] is only called
/// for a session that asked for a strip.
const STATUS_TICK: Duration = Duration::from_secs(1);

/// Arm the status strip's tick. Called by a backend's `run_inner` only when
/// `--status` is on.
///
/// The callback is deliberately empty: every dispatch round ends in
/// [`post_dispatch`], which is where the strip is re-sampled and where the
/// decision to repaint is made. A timer that did the sampling itself would be a
/// second place that decision lives, which is how the two backends' strips would
/// come to disagree.
pub(crate) fn arm_status_tick<H>(handle: &LoopHandle<'static, H>) -> Result<(), Box<dyn Error>>
where
    H: RuntimeHost,
{
    handle.insert_source(
        Timer::from_duration(STATUS_TICK),
        |_now, _, _host: &mut H| TimeoutAction::ToDuration(STATUS_TICK),
    )?;
    tracing::debug!(
        interval_ms = STATUS_TICK.as_millis(),
        "status strip armed: the loop wakes once a second so the clock can move"
    );
    Ok(())
}

/// **Spawn every configured realm and put each on the loop.** The other half
/// of the runtime wiring: before this, a running `vitrind` forked nothing and
/// the whole shim half of the core was reachable only from tests.
///
/// Call this **after** [`install`] and **before** `event_loop.run`. The
/// ordering is the trap this task exists around, and it is permanent rather
/// than slow if got wrong: the shim blocks on `configure` and then on every
/// reply, with no timeout anywhere on its side, so a shim spawned into a loop
/// that is not yet servicing its socketpair wedges until someone kills the
/// core. Nothing here blocks for longer than a `configure` write into an
/// empty kernel buffer, and the loop starts immediately after.
///
/// # The sequence, and why each step is where it is
///
/// 1. `spawn_realm` forks and execs, taking the realm's `flock` before it
///    touches the realm's runtime directory and journalling `realm_spawned`.
/// 2. `start_shim_session` sends `configure` on the **still-blocking** fd.
///    This is the counter-intuitive step: the instinct is to register the
///    reader first, but `ConnectionSource` takes the `Connection` **by
///    value** and flips it non-blocking, after which there is no way to
///    reach it outside a dispatch callback — so the send would become
///    unreachable. A few dozen bytes into an empty buffer on a freshly
///    created socketpair cannot park the compositor, which is what makes
///    the blocking send safe.
/// 3. `into_parts` disarms the spawn's kill-on-drop guard and hands every
///    resource the realm owns to [`RealmLifecycle`], which owns them to the
///    end. Dropping a `SpawnedRealm` instead would kill and reap the child —
///    correct on the error paths above, fatal on this one.
/// 4. `RealmLifecycle::adopt` places the connection into a
///    [`ConnectionSource`] and keeps the registration as its [`Hangup`], so
///    the ladder's first rung really does hang up. The realm's id is carried
///    **into the source's callback**, which is how [`dispatch_shim`] knows
///    whose message it is handling once more than one shim is attached.
/// 5. `mark_running` moves the realm out of `Configured`. Easy to forget and
///    **silent** if forgotten — `Configured` still admits petitions, so
///    nothing fails; the only symptom is a wrong `RealmState` in the flight
///    recorder.
pub(crate) fn start_realm<H: RuntimeHost>(host: &mut H) -> Result<(), Box<dyn Error>> {
    // The shim binary is a core input (`--shim`), carried in the runtime; the
    // app it will exec is each realm's `command`. Clone it out before the
    // `start_realm_in` borrow, which takes `host` again.
    let shim = host.runtime().shim.clone();
    start_realm_in(host, &SpawnPaths::from_env(shim)?)
}

/// [`start_realm`] against an explicit runtime tree.
///
/// Split out for the same reason [`vitrin_ipc::paths`] ships an `*_in` form
/// of every helper: a spawn that reads `$XDG_RUNTIME_DIR` at the moment it
/// forks cannot be tested against a scratch tree without mutating the
/// process environment, and this crate's tests run in one process.
///
/// # One failure stops the session, deliberately — and takes its siblings
/// # with it
///
/// Any realm that fails to spawn aborts the whole startup. The alternative
/// — come up with the realms that worked — would hand the operator a
/// session that is silently missing an app, and the audit posture this core
/// takes at load time (`realm.toml` refuses rather than defaults) says the
/// opposite. It does mean one mistyped `command` in the twelfth table stops
/// the desktop coming up at all; that is the cost, and it is the same one
/// `RealmRegistry::load` already imposes.
///
/// **The realms already forked are torn down here, before the error is
/// returned.** Not left to the caller: neither backend reaches its
/// [`shutdown_realm`] on this path — both `return Err(err)` the moment
/// startup fails, because the loop they would otherwise have run has not
/// started. Without the teardown, a failure on realm 3 of 5 leaves realms 1
/// and 2 running: two shim processes with their apps, two runtime trees and
/// two held `flock`s, outliving the core that forked them and owning
/// directories the next core will find locked. A caller that *does* reach
/// its own `shutdown_realm` sees an empty map and does nothing, so the
/// teardown is idempotent rather than duplicated.
///
/// One residual, stated rather than implied — and enumerated by *failure
/// site*, because "post-spawn" is more than one place and an enumeration
/// that presents itself as complete had better be:
///
/// - **Inside `spawn_realm`.** The realm that failed cleans up its own
///   spawn: `RuntimeDirGuard` removes the tree, `GuardedChild` kills and
///   reaps the child. Nothing is left.
/// - **After the spawn committed** — every later step in
///   [`start_one_realm_in`]: the `configure` write
///   (`SpawnedRealm::start_shim_session`), and the connection-placement
///   sequence that follows it (`into_parts` → [`RealmLifecycle::adopt`] →
///   `ConnectionSource::with_outbox` → `insert_source`). All of them kill
///   and reap the child and release the `flock`, and all of them leave
///   **that one realm's runtime directory** for the next run's stale-tree
///   purge. The first does it through `SpawnedRealm`'s drop; the second
///   does it because `adopt` kills and reaps on its own refusal path
///   (`into_parts` has disarmed `GuardedChild` by then, and a bare `Child`
///   drops without waiting — so the guarantee is written there rather than
///   inherited).
/// - **After the realm is in the map.** Nothing here can fail: the insert
///   and `mark_running` are infallible, and a registry that does not know
///   the realm logs rather than returns.
///
/// So the leftover is exactly one directory per realm that got as far as
/// forking and no further — never a process, never a lock, never a
/// half-registered event source.
pub(crate) fn start_realm_in<H: RuntimeHost>(
    host: &mut H,
    paths: &SpawnPaths,
) -> Result<(), Box<dyn Error>> {
    // NOT per-realm, and still deliberately so after WS-E.1.3: the view size
    // is the *output's*, shared by every realm, because there is one output
    // and decision 3 keeps it that way — one bound realm, no stacking, no
    // overlap, no resize. Every realm now has its own scene and its own
    // capture, but every one of them composes at this one geometry, so every
    // shim is still configured identically.
    let (width, height) = {
        let (_, view) = host.split();
        view.view_size()
    };

    // Collected before the loop: `spawn_realm` needs `&Realm` out of the
    // registry while the body below takes `host` mutably again, and the
    // registry itself is written by `mark_running` on the way out.
    // **Templates are skipped** (WS-E.1.1, issue #207): a realm declared
    // `autostart = false` exists to be launched *from*, so startup forking
    // it would be the one thing the key says not to do. It stays in the
    // registry, addressable and petitionable, in `RealmState::Template`.
    // `RealmRegistry::from_specs` refuses a file where *every* realm is a
    // template, so this filter cannot empty the list on a loaded config.
    let configured: Vec<RealmId> = host
        .runtime()
        .kernel
        .realms
        .iter()
        .filter(|realm| realm.state() != crate::realm::RealmState::Template)
        .map(|realm| realm.id().clone())
        .collect();
    if configured.is_empty() {
        return Err("no realm is configured for this session".into());
    }

    for realm_id in configured {
        if let Err(err) = start_one_realm_in(host, paths, &realm_id, width, height) {
            // Blocking, and safe to be: the event loop has not started (this
            // runs between `install` and `event_loop.run`), so there is no
            // dispatch to stall — and rung 0 still needs the loop *handle*,
            // which is alive in the backend's scope either way.
            tracing::error!(
                realm = %realm_id,
                %err,
                "a realm failed to start; tearing down the realms already forked"
            );
            shutdown_realm(host);
            return Err(err);
        }
    }
    Ok(())
}

/// One realm's whole spawn-and-attach sequence — the body [`start_realm_in`]
/// runs per configured realm.
///
/// Every step below really was already per-realm, which is the half of
/// `realm.rs`'s "a deletion rather than a re-plumbing" claim that held: the
/// spawn derives its runtime directory, `flock` and socket from the realm id
/// (`vitrin_ipc::paths`), the lifecycle owns exactly one realm's resources,
/// and the registry's transition takes the id. What did *not* hold was where
/// the result is stored — see [`Runtime::realms`].
fn start_one_realm_in<H: RuntimeHost>(
    host: &mut H,
    paths: &SpawnPaths,
    realm_id: &RealmId,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn Error>> {
    let spawned = {
        let runtime = host.runtime();
        // Disjoint field borrows: `spawn_realm` reads the realm and writes
        // the log, and both live in `Kernel`.
        let Kernel {
            realms, recorder, ..
        } = &mut runtime.kernel;
        let realm = realms
            .get(realm_id.as_str())
            .ok_or_else(|| format!("realm {realm_id} vanished from the registry mid-startup"))?;
        // The origin is a required argument, so "startup forked it" is
        // stated rather than inferred from the absence of a principal
        // (WS-E.1.1) — the journal's `spawned_by` field comes from here.
        spawn::spawn_realm(realm, paths, recorder, spawn::SpawnOrigin::Startup)?
    };
    attach_spawned_realm(host, spawned, width, height)
}

/// **Steps 2–5 of [`start_one_realm_in`]**: everything after the fork.
///
/// Split out for [`launch_realm`], the wire-reachable spawn path (WS-E.1.1,
/// issue #207), which has to fork *inside* the enforcement chokepoint's
/// launch sink — a `launched` event is a terminal, so a failure discovered
/// after the reply could not be voiced — and then attach the child once the
/// dispatch borrows have ended. Both callers run identical code from here
/// on, which is the point: one attach sequence, one place the shim session,
/// the loop registration and the registry transition are wired together.
fn attach_spawned_realm<H: RuntimeHost>(
    host: &mut H,
    mut spawned: spawn::SpawnedRealm,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn Error>> {
    // Read back rather than assumed: this is the id the spawn actually
    // derived every path it owns from — the realm's runtime directory, its
    // `flock`, its private `wayland-0` — and the one `configure` carries to
    // the shim. Keying the runtime map on the lookup id while the
    // filesystem used another would be invisible until a teardown went
    // looking for the wrong tree.
    let realm_id = &spawned.realm_id().clone();
    let pid = spawned.pid();

    // Step 2: `configure` on the still-blocking fd, before calloop owns it.
    let server = spawned.start_shim_session(width, height)?;

    let parts = spawned.into_parts();
    let handle = host.loop_handle();
    // `adopt` hands the connection to this closure and takes back the way to
    // release it again, so there is no window in which the lifecycle holds a
    // realm it cannot hang up on.
    let mut registered = None;
    let dispatched_realm = realm_id.clone();
    let life = RealmLifecycle::adopt(parts, |connection| {
        let (source, outbox) = ConnectionSource::with_outbox(connection)?;
        // The realm id rides in the callback's captured state: a
        // `ConnectionSource` carries no metadata of its own, and without
        // this `dispatch_shim` would have to guess which of N attached
        // shims it is servicing — which, with one realm, it never had to.
        let token = handle.insert_source(source, move |event, conn, host: &mut H| {
            dispatch_shim(host, &dispatched_realm, event, conn)
        })?;
        registered = Some(outbox);
        let releaser = handle.clone();
        Ok::<_, Box<dyn Error>>(Hangup::registered(move || releaser.remove(token)))
    })?;
    let outbox = registered.expect("adopt runs `place` exactly once on the success path");

    // **The output's first binding**, and a placeholder in exactly the sense
    // [`seat_target`]'s fallback is one: the first realm to attach gets the
    // output. Who *may* move it deliberately is answered by the `layout_focus`
    // verb (WS-E.1.4, issue #210, reaching the presenter through
    // [`apply_layout`]); this is what answers when nobody has.
    //
    // Only *this* caller is conditional on nothing being bound yet. The other
    // one, [`rebind_output_after_death`], moves the output when the realm
    // holding it stops serving, because "bound once, never moved" left the
    // output stuck on a corpse for the rest of the session.
    //
    // Deliberately the same realm `seat_target` names — realms attach in the
    // registry's id order, and `seat_target` takes the first still-serving
    // realm in id order — so the realm a human is looking at is the realm the
    // human's own input reaches. (An *agent's* actuation follows its grant's
    // realm and never this, since WS-E.1.6.)
    {
        let (runtime, view) = host.split();
        if view.focused().is_none() {
            view.bind_output(realm_id);
            // ...and the ROUTER is told in the same breath, because it keeps
            // its own record of who holds the human's attention and it is the
            // only thing that knows which presses that realm is holding.
            //
            // Binding the scene alone leaves the two disagreeing: the scene
            // shows realm-0 and physical input reaches it (`physical_seat_target`
            // reads the realms and the scenes, not this), while the router still
            // reads `None`. The first `layout_focus` then finds no debtor —
            // `InputRouter::bind_to` returns early on `self.bound.replace(..)?`
            // — and pays no releases, so a key held across the FIRST switch of
            // a session latches forever in the app being left. That is the
            // common case, not an edge one: the first switch is the one that
            // moves you off the realm you started in.
            let owed = runtime.router.bind_to(realm_id);
            debug_assert!(
                owed.is_none(),
                "the first bind can owe nothing: this branch runs only when the scene has \
                 no binding, so no realm has held the human's attention yet"
            );
            tracing::info!(realm = %realm_id, "output bound to the first realm to attach");
        }
        // The bound realm's view has changed under the output; nothing has
        // composited it yet.
        runtime.dirty = true;
    }

    let runtime = host.runtime();
    runtime.realms.insert(
        realm_id.clone(),
        RealmRuntime {
            life,
            server: Some(server),
            outbox,
            // The spawn `configure` above carried the output's size, so the
            // realm really is in the fullscreen arrangement here.
            arrangement: LayoutMode::Fullscreen,
        },
    );
    if !runtime.kernel.realms.mark_running(realm_id, pid) {
        // A realm the registry does not know is a wiring bug rather than a
        // runtime condition, and it is invisible without this: `Configured`
        // admits petitions exactly as `Running` does, so the session would
        // work and only the log would lie.
        tracing::error!(
            realm = %realm_id,
            pid,
            "spawned a realm the registry does not know; its recorded state will be wrong"
        );
    }
    tracing::info!(
        realm = %realm_id,
        pid,
        "realm spawned and its shim session attached to the event loop"
    );
    Ok(())
}

/// A `SIGCHLD` arrived: poll **every** live realm for an exit.
///
/// Speculative and cheap by design — `SIGCHLD` says only that *some* child
/// changed state, so the reaper asks `waitpid` rather than guessing. A realm
/// already reaped answers immediately.
///
/// **Every realm, not the first one that answers.** With one realm a single
/// `waitpid` was complete; with N it is not, and stopping early is a silent
/// failure rather than a loud one: the unpolled realm leaves a zombie and
/// the registry goes on calling it `Running`, so petitions for a dead realm
/// keep resolving and the only trace is the absent `realm_exited` entry in
/// the journal. Signals also coalesce — one `SIGCHLD` may stand for several
/// children — so "one signal, one reap" was never a safe reading even
/// before this.
pub(crate) fn reap_realm<H: RuntimeHost>(host: &mut H) {
    for realm_id in live_realm_ids(host) {
        with_realm_teardown(host, &realm_id, |life, teardown| {
            life.poll_exit(teardown);
        });
    }
}

/// Tear every realm down on the way out of the session: the shutdown ladder
/// per realm, then each realm's runtime tree.
///
/// **Blocks**, deliberately, which is why it must run after `event_loop.run`
/// has returned and never from inside a dispatch: the ladder waits out a
/// hangup grace period and then a `SIGTERM` grace period, and doing that
/// inside a live compositor loop would stall every other peer. It must also
/// run before the recorder is handed back, so each realm's `realm_died` and
/// `realm_exited` entries land in the run they belong to.
///
/// The ladders run in id order, one after another, so the worst-case exit
/// time is N realms' grace periods rather than one. Running them
/// concurrently would mean interleaving blocking waits on a single-threaded
/// core, and the grace periods only elapse in full for a shim that ignores
/// both EOF and `SIGTERM` — which is a defect, not the ordinary path.
pub(crate) fn shutdown_realm<H: RuntimeHost>(host: &mut H) {
    for realm_id in live_realm_ids(host) {
        let rung = with_realm_teardown(host, &realm_id, |life, teardown| {
            let rung = life.shutdown(ShutdownTiming::default(), teardown);
            (life.pid(), rung)
        });
        if let Some((pid, rung)) = rung {
            tracing::info!(realm = %realm_id, pid, ?rung, "realm torn down");
        }
    }
    // Dropped only now: each lifecycle holds its realm's `flock`, and
    // releasing one before its runtime tree is gone would let a second core
    // call that tree stale while this one is still taking it apart.
    host.runtime().realms.clear();
}

/// Every realm with a live shim session, in id order.
///
/// Materialized into a `Vec` because every caller then takes `host` mutably
/// per realm — [`with_realm_teardown`] borrows the whole session — so
/// iterating the map in place would alias it. Id order rather than insertion
/// order so a shutdown, a reap and a log read the same way twice.
fn live_realm_ids<H: RuntimeHost>(host: &mut H) -> Vec<RealmId> {
    host.runtime().realms.keys().cloned().collect()
}

/// Run `f` against **this realm's** lifecycle with a [`RealmTeardown`] built
/// from the whole session — the one borrow shape every death path needs.
/// `None` when the realm has no live shim session (never forked, or already
/// torn down).
///
/// Every field comes from a distinct place (the presenter's scene, retained
/// image and dmabuf importer, the realm's shim server, the runtime's router,
/// the kernel's registry and recorder), so this exists to assemble that
/// borrow once rather than four times, and to make sure no death path can
/// quietly leave one of them out. On headless `importer` is always `None`:
/// there is no GPU renderer to have imported anything, so every
/// `kind=dmabuf` commit already resolved as the designed `import_failed` shm
/// fallback and there is no zero-copy content to drop.
///
/// **The `scene` and `importer` halves are now this realm's own** (WS-E.1.3):
/// a death clears exactly the dying realm's surface and drops exactly its own
/// retained zero-copy import, leaving every sibling's composed view
/// byte-identical and every sibling's GPU texture resident. They used to be
/// the session's single scene and single retained slot, so a death took
/// whichever realm's happened to be there — fail-closed, and stated as such,
/// but gone now.
///
/// **The `retained` half is still the output's, and is still scrubbed on any
/// realm's death.** That is over-broad — a hidden realm dying blanks the
/// bound realm's last painted frame out of the headless framebuffer — and it
/// stays that way on purpose, for two reasons. It is the fail-*closed*
/// direction (a scrub can only remove pixels, never serve a dead realm's),
/// and [`close_realm`] marks the frame dirty and requests a present on the
/// same path, so the very next round recomposites the bound realm and the
/// blank is not observable to a capture that could not already refuse. The
/// alternative — scrubbing only when the *bound* realm dies — is a second
/// predicate about whose pixels are in that buffer, which is precisely what
/// `RetainedOutput`'s docs refuse to grow.
///
/// The `router` half, by contrast, **is** scoped: it is one router, but its
/// per-shim-generation state belongs to one realm at a time, and
/// [`crate::input::InputRouter::reset_for`] clears it only for the realm
/// that owns it. Clearing it unconditionally is not the same benign
/// direction as clearing the scene: it forgets a *surviving* app's held
/// keys, whose releases are then dropped as unpaired and latch down for
/// good.
fn with_realm_teardown<H: RuntimeHost, T>(
    host: &mut H,
    realm_id: &RealmId,
    f: impl FnOnce(&mut RealmLifecycle, &mut RealmTeardown<'_, '_, H::Hook>) -> T,
) -> Option<T> {
    let out = {
        let (runtime, view) = host.split();
        view.teardown_view(realm_id, move |scene, retained, importer| {
            let Runtime {
                kernel,
                realms,
                router,
                ..
            } = runtime;
            let realm = realms.get_mut(realm_id)?;
            let mut teardown = RealmTeardown {
                scene,
                shim: &mut realm.server,
                importer,
                router,
                retained,
                clipboard: &mut kernel.clipboard_slot,
                realms: &mut kernel.realms,
                recorder: &mut kernel.recorder,
            };
            Some(f(&mut realm.life, &mut teardown))
        })
    };
    // Here rather than in `close_realm` because this is the funnel every death
    // path shares -- `close_realm`, `reap_realm` and `shutdown_realm` all
    // arrive through it, and a rebind wired to one of the three would leave a
    // `SIGCHLD`-observed death holding the output.
    rebind_output_after_death(host);
    out
}

/// **The output must not stay bound to a realm that is gone.**
///
/// [`RealmScenes::bind`] was the only writer of the binding, and nothing
/// cleared it: [`start_one_realm_in`] bound the first realm to attach, and
/// when that realm's app exited the teardown funnel cleared its *scene* while
/// the binding survived. Every later composite then rendered the empty scene
/// — the deterministic background — for the rest of the session, and
/// [`post_dispatch`]'s bound-realm gate suppressed the agent-cursor sprite on
/// the strength of a `focused()` that named a corpse, all while live siblings
/// ran. That is a stuck output, not a policy.
///
/// # The narrowest behaviour, and why this one
///
/// **Move the output to the first still-serving realm in id order; unbind only
/// when there is none.** That is [`seat_target`]'s rule, letter for letter,
/// and choosing it is the whole of the argument:
///
/// - It is not a focus *policy*. Who may move the output deliberately is the
///   `layout_focus` verb's answer (WS-E.1.4, issue #210), and this is not that
///   path: it is the same placeholder [`start_one_realm_in`] applies at first
///   attach, now also applied when the realm it picked stops existing. A realm
///   dying is not a principal exercising authority, so it must not consult
///   one.
/// - It keeps the realm a human **watches** and the realm their own input
///   **reaches** equal. Those are one question ([`seat_target`] answers both),
///   so picking any other realm here would make the human watch realm X and
///   type into realm Y — the split D-018(2)'s fifth ordering rule forbids,
///   arrived at by a death instead of by a verb. (An *agent's* actuation is
///   unaffected either way: since WS-E.1.6 it follows its grant's realm, so a
///   death that moves the output moves nothing of theirs.)
/// - Unbinding instead — the other narrow option — is a different way of being
///   stuck: it composites the deterministic background while a live sibling
///   paints, which is the state this function exists to leave. So it is the
///   answer only when nothing is serving, where [`seat_target`] also answers
///   `None` and the two still agree.
///
/// A no-op unless the realm the output is bound to has actually lost its shim
/// session, so the ordinary path — a `SIGCHLD` poll that finds nothing, a
/// sibling dying — costs one map lookup and changes nothing.
fn rebind_output_after_death<H: RuntimeHost>(host: &mut H) {
    let (runtime, view) = host.split();
    // `server.is_none()` is the same fact `seat_target` filters on and the
    // same one `refresh_view_cache` prunes on: `RealmLifecycle::die` takes the
    // `ShimServer` out of the runtime entry, and no other path does.
    let bound_is_gone = view.focused().is_some_and(|bound| {
        runtime
            .realms
            .get(bound)
            .is_none_or(|realm| realm.server.is_none())
    });
    if !bound_is_gone {
        return;
    }
    // `None` for the binding, deliberately: the bound realm is the one that
    // just died, so asking `seat_target` to follow it would answer with the
    // corpse's own id. This is the fallback half of that function — the
    // first still-serving realm in id order — asked for explicitly.
    match seat_target(&runtime.realms, None).map(|(realm_id, _)| realm_id.clone()) {
        Some(next) => {
            view.bind_output(&next);
            tracing::info!(
                realm = %next,
                "the realm holding the output died; the output moves to the first \
                 still-serving realm in id order, and the human's input with it"
            );
        }
        None => {
            view.unbind_output();
            tracing::info!(
                "the realm holding the output died and no realm is still serving; the output \
                 shows the deterministic background"
            );
        }
    }
    // The window is showing a different realm now, and on a backend whose
    // frame clock is external the dirty flag alone composites nothing --
    // exactly the pairing `close_realm` documents.
    runtime.dirty = true;
    view.request_present();
}

/// The advisory expiry sweeps, one timer tick.
///
/// Petitions first, deliberately: a petition that times out emits the
/// client's terminal, and running it before the grant sweep keeps the log's
/// causal order — a petition dies, then rows die — matching what a
/// reconstruction expects.
fn sweep<H: RuntimeHost>(host: &mut H) {
    sweep_at(host, Instant::now());
}

/// [`sweep`] with the instant injected, so its call sites are testable.
///
/// Split out because the clipboard's expiry lives here and "cleared after two
/// minutes" cannot be asserted against a function that reads the wall clock
/// itself. The split is the whole point: the bug this fixes was never in
/// `ClipboardSlot::expire`, which had a passing unit test — it was in **where
/// expire was called from**, and a test that cannot drive the call site cannot
/// see that class of defect at all.
fn sweep_at<H: RuntimeHost>(host: &mut H, now: Instant) {
    let runtime = host.runtime();

    for resolution in runtime.kernel.petitions.expire_due(now) {
        deliver(runtime, resolution, now);
    }

    // `record_expiry_sweep` writes nothing for an empty sweep, which is why
    // this can run every second without turning a quiet session's log into a
    // heartbeat file.
    let expired = runtime.kernel.grants.expire_due(now);
    runtime.kernel.recorder.record_expiry_sweep(&expired);

    // **The clipboard slot expires HERE, on the timer, not on input.**
    //
    // It was originally expired inside `drain_clipboard_gestures`, which made
    // "cleared after two minutes" false in the one case that matters: a human
    // copies a password and walks away. That function runs only on a physical
    // input turn, and returns early unless that turn carried a clipboard
    // chord -- so with nobody at the keyboard the plaintext stayed resident in
    // `vitrind`'s heap for the life of the session, while `limits.md`, D-024(5)
    // and the code's own comment all said it was gone. The timeout was a read
    // filter wearing a clear's clothes.
    //
    // This sweep already exists, already runs every second whether or not
    // anything happened, and is already the home of expiry. The cost of that
    // granularity is stated rather than hidden: the slot is cleared within one
    // second of its deadline, never before it.
    if runtime.kernel.clipboard_slot.expire(now) {
        tracing::debug!("clipboard slot expired on the sweep");
    }
}

/// Route one deferred [`Resolution`] to the connection that petitioned.
///
/// The connection may be gone — an agent that disconnects while its petition
/// is pending is entirely ordinary — and that case is deliberately **not**
/// checked here: `deliver_resolution` refuses it typed and records
/// `PetitionUndelivered` itself, from the one funnel that covers every
/// refusal reason. A second check here would be a second place a human's
/// decision can be quietly destroyed.
pub(crate) fn deliver<H: PreemptionHook>(
    runtime: &mut Runtime<H>,
    resolution: Resolution,
    now: Instant,
) {
    let Runtime { kernel, conns, .. } = runtime;
    let Some(conn) = conns.get_mut(&resolution.connection) else {
        // Teardown already withdrew this connection's pending petitions, so a
        // resolution addressed to a forgotten connection means the sweep and
        // the close raced — not that a decision was lost. Nothing to send and
        // nothing to record.
        return;
    };
    let outbox = conn.outbox.clone();
    let mut send = |frame: &[u8], fd: Option<BorrowedFd<'_>>| {
        debug_assert!(
            fd.is_none(),
            "no version-1 event sent outside dispatch carries an fd"
        );
        outbox.send(frame)
    };
    if let Err(err) = conn.server.deliver_resolution(
        resolution,
        &mut kernel.grants,
        &mut kernel.recorder,
        now,
        &mut send,
    ) {
        // Already recorded by `deliver_resolution`'s own funnel; logged here
        // because a routing failure is an embedder fault worth an operator's
        // attention, not merely a reconstruction detail.
        tracing::warn!(%err, "petition resolution could not be delivered");
    }
}

/// Whether a card raised **this round** would reach the human's eyes
/// (WS-E.3.3, D-030(4)).
///
/// [`ConsentGrab::raise`] does five things in one call so that "visible",
/// "input grabbed", "`consent_held` holds" and "the log says so" cannot drift
/// apart — and the first of those five is the one an embedder can be wrong
/// about. `raise` writes `Event::ConsentTransition { state: Shown }`, which is
/// the flight recorder's record that **a human was asked**. Raising against a
/// screen this session no longer owns puts a falsehood in the one artifact that
/// has to reconstruct the session afterwards, and it also sets `prompt_shown`,
/// so the enforcement chokepoint starts refusing that principal `consent_held`
/// citing a card nobody can see.
///
/// # Why this is a parameter and not a field of [`ConsentGrab`]
///
/// [`ConsentGrab::set_view`]'s docs make the argument: a fact fed in from
/// "somewhere else" is a correctness hazard rather than a stale cache, and the
/// view is only safe because it is co-fed with the routing call in the same
/// step. A `lit`/`dark` field on the grab would have no such co-feed and would
/// be exactly that drift-prone shadow state. So the embedder answers the
/// question once per round, from the same fact it presents by.
///
/// # The third variant arrived, and D-030(4) said which change would owe it
///
/// This paragraph used to say there was deliberately no variant for a dark
/// panel, on the ground that "this core has no DPMS: it never sets a
/// display-power state, never reads one". That was true and is now false —
/// WS-E.4.3 (issue #223) implements the idle blank, and D-030(4) deferred "a
/// dark-output gate — to whichever change implements DPMS, **as that change's
/// own acceptance criterion**". [`PromptVisibility::ScreenIsDark`] is that gate.
///
/// It is a separate variant rather than a reuse of
/// [`PromptVisibility::ScreenNotOurs`] because the two facts differ and the
/// flight recorder has to be able to say which: a paused session's card can
/// never reach a panel and never will, while a dark session's reaches it the
/// instant the human touches anything. `ScreenIsDark` is therefore strictly
/// *less* obstructive than `ScreenNotOurs`, and saying so is the honest record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptVisibility {
    /// The embedder is presenting to a display it owns, so a card raised this
    /// round reaches it. Every backend that can host a prompt at all answers
    /// this in the ordinary case.
    Reachable,
    /// The seat has taken this session's devices away — a VT switch on the
    /// bare-metal backend — so nothing composited this round reaches a panel
    /// and no physical input can arrive to answer it. The petition stays
    /// pending, unshown and unjournalled, and the ordinary advisory sweep
    /// resolves it `timed_out`, which reaches the agent as a refusal.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "only a seat can take a screen away, and only the bare-metal backend has \
                      one; a default build compiles this path and tests it through the rig"
        )
    )]
    ScreenNotOurs,
    /// The session powered its own panel down after `--blank-idle` seconds with
    /// no physical input (WS-E.4.3, [`crate::backend::blank`]). A card
    /// composited now is on a display that is off, so raising one would write
    /// `consent_transition{shown}` — the flight recorder's record that **a human
    /// was asked** — about a human who is looking at a dark screen, and would
    /// set `prompt_shown` so the chokepoint starts refusing that principal
    /// `consent_held` citing a card nobody can see.
    ///
    /// The petition stays pending, unshown and unjournalled, and the ordinary
    /// advisory sweep resolves it `timed_out`, which reaches the agent as a
    /// refusal. Fail-closed, and the same treatment a paused seat gets.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "only a display controller can be powered down, and only the bare-metal \
                      backend owns one; a default build compiles this path and tests it \
                      through the rig"
        )
    )]
    ScreenIsDark,
}

/// Service one turn of interactive consent: retire a prompt whose petition
/// already left the table, apply every human decision the input grab produced,
/// then raise the front-of-queue petition's prompt if none is up. Reports
/// whether anything changed, so the caller can mark the frame dirty and
/// present the transition.
///
/// This is the call that closes issue #90's gap: [`ConsentGrab::raise`] had no
/// caller outside tests, so under `--consent=interactive` every petition
/// pended until the armed sweep timed it out. The renderer (P1.7.1), the input
/// grab (P1.7.2) and revocation (P1.7.3) all landed unwired; this is the one
/// site that drives them from the running loop, and the nested backend's
/// [`RuntimeHost::service_consent`] override is its only production caller (the
/// backends that cannot host a prompt inherit the trait's no-op default).
///
/// # Why the four steps are in this order
///
/// The three grab helpers each borrow two disjoint fields of
/// `runtime.kernel` (the petition registry and the recorder) plus the `consent`
/// surface, which is why `grab`, `consent` and `runtime` are distinct
/// parameters rather than reached through one `&mut self`: it is what lets the
/// borrow checker see the registry borrow inside a grab call as disjoint from
/// the `recorder` borrow beside it. [`deliver`] borrows `runtime` wholesale, so
/// each `resolve_human` borrow must end before its `deliver` call — the match
/// arm below is what closes it.
///
/// 1. [`ConsentGrab::retire_stale`] first, so a prompt whose petition died via
///    the sweep (timeout), a withdrawal, or a dead-man denial is taken down and
///    the queue is freed *before* a decision or a fresh raise is considered.
/// 2. Drain decisions. [`PetitionRegistry::resolve_human`] fails closed
///    (`NotPending`) on a petition that raced a timeout or a withdrawal, so a
///    stale decision can never land on the wrong petition — the exactly-once
///    guard, typed, and the reason a [`Decision`](crate::consent::grab::Decision)
///    carries its own petition id rather than being applied to whatever is
///    pending now.
/// 3. `retire_stale` again: a just-decided petition has left the pending table,
///    so its card comes down *this* round and the queue advances immediately
///    rather than one turn late.
/// 4. Raise the front prompt **only when none is up**, and **only onto a
///    screen this session still owns** ([`PromptVisibility`]). The first makes
///    one-prompt-at-a-time structural, and is also what stops a busy-spin:
///    re-raising an already-shown petition returns `Some(route)`, which would
///    otherwise set `changed = true` on every idle round and keep the frame
///    perpetually dirty. The second is D-030(4), and it is deliberately the
///    *only* step gated — see [`PromptVisibility`].
pub(crate) fn service_consent_round<H: PreemptionHook>(
    grab: &mut ConsentGrab,
    runtime: &mut Runtime<H>,
    consent: &mut ConsentSurface,
    now: Instant,
    visibility: PromptVisibility,
) -> bool {
    // Take down a prompt whose petition already left the table (timed out via
    // the sweep, withdrawn with its connection, or dead-man-denied), freeing
    // the queue for the next petition before anything else runs.
    let mut changed = grab.retire_stale(&mut runtime.kernel.petitions, consent);

    while let Some(decision) = grab.take_decision() {
        // `resolve_human` fails closed on a petition that raced a timeout or a
        // withdrawal, so a stale decision drained here can never land on the
        // wrong petition; the `Err` arm's borrow ends before `deliver` takes
        // `runtime` wholesale on the `Ok` arm.
        match runtime
            .kernel
            .petitions
            .resolve_human(decision.petition, decision.choice)
        {
            Ok(resolution) => {
                deliver(runtime, resolution, now);
                changed = true;
            }
            Err(err) => tracing::debug!(
                ?err,
                petition = %decision.petition,
                "consent decision no longer applies to its petition"
            ),
        }
    }

    // A just-decided petition has left the table: take its card down so the
    // queue advances on this same round rather than the next one.
    changed |= grab.retire_stale(&mut runtime.kernel.petitions, consent);

    // One prompt at a time, made structural — and the guard that keeps this
    // from busy-spinning, since re-raising an already-armed petition would
    // return `Some(route)` and set `changed` every round.
    //
    // **And only onto a screen this session still owns** (D-030(4)). Step 4
    // alone is gated: `retire_stale` and the decision drain above must keep
    // running through a pause, or a dead petition's card stays composited and
    // the queue never advances.
    if visibility == PromptVisibility::Reachable && grab.armed_petition().is_none() {
        if let Some(front) = runtime.kernel.petitions.front_pending() {
            // `raise` borrows two disjoint fields of `runtime.kernel` (the
            // petition registry and the recorder) alongside the `consent`
            // param — allowed because they are distinct fields, and the reason
            // this is a free function over three params rather than a method.
            if let Some(route) = grab.raise(
                front,
                now,
                &mut runtime.kernel.petitions,
                consent,
                &mut runtime.kernel.recorder,
            ) {
                announce_prompt(runtime, route);
                changed = true;
            }
        }
    }

    changed
}

/// Drive the lock screen for one dispatch round (WS-E.2.2, issue #214):
/// raise it if the session has gone idle, mirror the gate's state onto the
/// surface, and journal every fact the gate produced.
///
/// Returns whether human-visible output changed, so the caller marks the frame
/// dirty — [`service_consent_round`]'s contract, so a lock appearing and a
/// prompt appearing take the same path to the screen.
///
/// # Why the embedder does this and the gate cannot
///
/// The gate runs inside [`crate::input::InputRouter::route_physical`], which
/// holds no recorder and no realm registry and must never grow either — the
/// division [`crate::clipboard`] and [`crate::attention`] already make. So
/// [`LockScreen`] decides *whether* the session is locked and queues the facts;
/// this function, which owns the registry and the recorder, decides what the
/// card says and what the journal records.
///
/// # The two states are mirrored, never duplicated
///
/// [`LockScreen`] is the single source of truth: [`LockSurface`] is told what
/// it already decided. A surface raised without the gate would be pixels with
/// no grab behind them — a lock screen an app can type through, which is worse
/// than no lock at all — and a gate raised without the surface would consume
/// every key with nothing on screen to explain why. Both are impossible while
/// this is the only place either is raised in production.
pub(crate) fn service_lock_round<H: PreemptionHook>(
    screen: &mut crate::lock::LockScreen,
    surface: &mut crate::lock::LockSurface,
    runtime: &mut Runtime<H>,
    // Required, not `Option`: an unlock has to restart the consent guard (see
    // the `Unlocked` arm below), and a parameter a caller may omit is the shape
    // this codebase has already had to un-ship twice -- an optional hook that
    // no backend stacked, and a defaulted trait method that silently disabled
    // the attention key. A caller with no prompt passes its own grab anyway.
    grab: &std::cell::RefCell<ConsentGrab>,
    deadman_chord: &'static str,
    now: Instant,
) -> bool {
    // The idle raise first, so a session that went idle this round locks on
    // this round's frame rather than the next one.
    screen.tick(now);

    // The realms named on the card: every realm still admitting petitions, in
    // registry order. Deliberately the *live* set rather than every configured
    // row — an exited realm keeps its id in the registry
    // (`RealmRegistry::mark_exited`), and naming a dead app on a lock screen
    // would tell a returning human their session is holding something it is
    // not.
    let realms: Vec<crate::grants::RealmId> = runtime
        .kernel
        .realms
        .iter()
        .filter(|realm| realm.admits_petitions())
        .map(|realm| realm.id().clone())
        .collect();

    let mut changed = false;
    match screen.cause() {
        Some(cause) => {
            let content = crate::lock::LockContent {
                cause,
                realms: realms.clone(),
                unlock: screen.unlock_method(),
                deadman_chord,
            };
            // `raise` is idempotent for identical content, so calling it every
            // round costs nothing and does not invalidate the raster at frame
            // cadence. It is still worth calling every round: a realm dying
            // behind the lock changes what the card should say.
            let before = surface.generation();
            surface.raise(content);
            changed |= surface.generation() != before;
        }
        None => {
            let before = surface.generation();
            surface.lower();
            changed |= surface.generation() != before;
        }
    }

    // Journal last, from facts the gate queued rather than from the state
    // above: an unlock that raced a realm death must still produce its
    // entries, and deriving them from the surface would lose the *attempts*
    // entirely (a wrong passphrase changes no pixel).
    for entry in screen.take_journal() {
        let event = match entry {
            crate::lock::LockJournal::Locked { cause } => crate::recorder::Event::SessionLocked {
                cause: cause.label(),
                passphrase: screen.unlock_method() == crate::lock::UnlockMethod::Passphrase,
                realms: realms.len(),
            },
            crate::lock::LockJournal::Attempted { accepted } => {
                crate::recorder::Event::UnlockAttempted { accepted }
            }
            crate::lock::LockJournal::Unlocked => {
                // **The consent guard restarts here, not at raise.** A prompt
                // raised while the lock was up spent its whole
                // `GUARD_INTERVAL` behind an opaque cover, so without this the
                // human returns, unlocks, sees a card for the first time, and
                // the first pointer press commits it with a guard that expired
                // while nobody could see the card. The lock kept it
                // *unanswerable*, which is not the same as *visible*, and the
                // guard is about visible.
                //
                // Only the guard moves; the petition's own deadline does not.
                // A lock does not buy the human more time to decide.
                //
                // Through [`screen_became_visible`] since WS-E.4.3, not
                // `restart_guard` directly: "the human can see this card again
                // as of now" is now true on four occasions -- an unlock, a seat
                // return, an idle-blank wake and a resume -- and four naked
                // calls would be four places for the fifth one to be forgotten.
                screen_became_visible(grab, now);
                crate::recorder::Event::SessionUnlocked
            }
        };
        runtime.kernel.recorder.record(event);
    }
    changed
}

/// Drive the **idle blank** for one dispatch round (WS-E.4.3, issue #223):
/// advance the countdown, abandon a wake that never completed, mirror the phase
/// onto the cover, and **say so in the log and the flight recorder** (issues
/// #258 and #259).
///
/// Returns whether human-visible output changed, so the caller marks the frame
/// dirty and asks for a present — [`service_consent_round`]'s and
/// [`service_lock_round`]'s contract, so a cover appearing and a card appearing
/// take the same path to the screen.
///
/// # No timer of its own, and that is the point
///
/// This is called from [`post_dispatch`], the loop's per-iteration callback, and
/// the loop is woken at least once a second by the session's own unconditional
/// sweep ([`SWEEP_INTERVAL`]). [`crate::lock::LockScreen::tick`]'s docs give the
/// rule and this inherits it: the round already samples one instant, and a
/// second clock would be a second thing to keep in step. The cost is up to one
/// second of lateness against a multi-minute timeout, which is not a number
/// anyone can see.
///
/// # The two states are mirrored, never duplicated
///
/// [`crate::backend::blank::SessionActivity`] is the single source of truth and
/// [`crate::backend::blank::BlankSurface`] is told what it already decided —
/// [`service_lock_round`]'s discipline exactly. A cover raised without the state
/// machine would be a black screen nothing can lift; a state machine that went
/// dark without the cover would power the panel down over the human's last
/// screenful, which is the disclosure the cover exists to prevent.
///
/// # It does NOT touch the lock, in either direction
///
/// On idle the screen goes dark and the session stays unlocked (Taha,
/// 2026-08-10). The two share [`crate::backend::blank::SessionActivity`]'s clock
/// and nothing else, and in particular this must never suppress
/// `LockScreen::tick`: a session with `--blank-idle 300 --lock-idle 600` would
/// then never lock, because the blank would silently disable the lock — the
/// class of unchosen behaviour D-030(2) was written to catch.
///
/// # The observability half lives here, not in the backend (issues #258, #259)
///
/// The blank's *power-down* line stays in [`crate::backend::drm`], because only
/// that site knows whether `DrmSurface::clear` was accepted. Everything else a
/// human or a journal reader needs to reconstruct the dark window is emitted
/// from here, for two reasons that point the same way: this is the one function
/// both the bare-metal backend and the session rig drive, so CI can hold it —
/// and the wake's own resolution is a *state machine* fact rather than a device
/// fact, so it belongs where the state machine is read. A wake line written in
/// `DrmState` would be a line no test in this workspace could ever see, which is
/// how a silent unblank shipped in the first place.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the one production caller is the bare-metal backend's `service_screen`, \
                  because only a backend that owns a display controller can blank one; a \
                  default build compiles this and drives it through the session rig's own \
                  override, which is where CI's coverage of the state machine lives"
    )
)]
pub(crate) fn service_blank_round<H: PreemptionHook>(
    runtime: &mut Runtime<H>,
    activity: &mut crate::backend::blank::SessionActivity,
    surface: &mut crate::backend::blank::BlankSurface,
    now: Instant,
) -> bool {
    // The countdown first, so a session that went idle this round covers on
    // this round's frame rather than the next one.
    activity.tick(now);
    // A wake that outran its deadline with no flip behind it is abandoned, so
    // the session is not left in a state where every subsequent press is
    // swallowed as "still waking". Fail open on input; the property that stops
    // a clickjack is the consent guard restart, not the consume.
    //
    // **Silent here on purpose since #258**: abandoning the wake is exactly the
    // failed unblank, so it is said once, below, in the arm that also knows how
    // long the panel was dark -- rather than twice, in two different voices.
    if activity.wake_expired(now) {
        activity.force_lit();
    }
    let changed = surface.set_covering(activity.is_covering());
    journal_screen_transition(runtime, activity, now);
    changed
}

/// Say what just happened to the human's panel — once, in the log and in the
/// flight recorder (issues #258 and #259).
///
/// Split out of [`service_blank_round`] because it is the only part that needs
/// the runtime at all, and because the round's three lines above should stay
/// readable as the state machine they are.
///
/// **The grant count is read only on a transition.** It is an O(rows) scan and
/// this runs every dispatch round; a session with no `--blank-idle` produces no
/// transition ever, so it pays one `Option` compare.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "reached only through `service_blank_round`, which carries the same note"
    )
)]
fn journal_screen_transition<H: PreemptionHook>(
    runtime: &mut Runtime<H>,
    activity: &mut crate::backend::blank::SessionActivity,
    now: Instant,
) {
    use crate::backend::blank::{ScreenTransition, WakeOutcome};

    let Some(transition) = activity.take_transition(now) else {
        return;
    };
    // `Active` rows only, through the same liveness every other read surface
    // folds in: a revoked, expired or spent row holds nothing, and counting one
    // would put a number on this entry that overstates what could see the human.
    let live_grants = runtime
        .kernel
        .grants
        .rows(now)
        .filter(|(_, state)| *state == crate::grants::GrantState::Active)
        .count();
    match transition {
        ScreenTransition::Blanked => {
            runtime
                .kernel
                .recorder
                .record(crate::recorder::Event::ScreenBlanked { live_grants });
        }
        ScreenTransition::Woke { dark, outcome } => {
            let dark_ms = u64::try_from(dark.as_millis()).unwrap_or(u64::MAX);
            match outcome {
                // **The wake line #258 asks for**, at the blank's own level. It
                // names what woke the panel (physical input -- nothing else can:
                // an agent's actuation never reaches the clock, and there is no
                // verb in the IDL for "power the human's display") and that the
                // modeset was accepted. It claims nothing further, deliberately:
                // the session was never locked, so this is not evidence about
                // *who* pressed the key, and the grants below were live the
                // whole time rather than restored by coming back.
                WakeOutcome::FlipLanded => tracing::info!(
                    dark_ms,
                    live_grants,
                    "the panel is lit again: physical input woke the session and the modeset \
                     was accepted. Nothing about authority changed across the dark window -- \
                     the session was never locked (idle blanks, it does not lock), so this is \
                     no evidence of WHO woke it, and every grant counted here was live \
                     throughout rather than restored by the wake"
                ),
                // ...and the failed one, distinguishable, which is the whole
                // point: before this, a wake that worked and a modeset that left
                // the panel dark produced identical output -- none.
                WakeOutcome::NoFlip => tracing::warn!(
                    dark_ms,
                    live_grants,
                    deadline_ms = crate::backend::blank::WAKE_DEADLINE.as_millis(),
                    "THE WAKE WAS NOT CONFIRMED: no flip landed within the deadline, so the \
                     panel may still be dark with this session running -- the state \
                     docs/book/src/recovery.md calls indistinguishable from a wedge. Physical \
                     input is being delivered again regardless, because a dark session that \
                     also swallows input is worse"
                ),
                // Neither a wake nor a failure: the human left for another VT
                // while the panel was dark, and this session dropped a blank it
                // could not have undone (`clear()` answers `DeviceInactive`).
                WakeOutcome::SeatLost => tracing::info!(
                    dark_ms,
                    "the idle blank ended because the seat took the panel away, not because \
                     the screen came back; the activate arm's ordinary repaint is what puts a \
                     picture back"
                ),
            }
            runtime
                .kernel
                .recorder
                .record(crate::recorder::Event::ScreenWoke {
                    dark_ms,
                    outcome: outcome.label(),
                    live_grants,
                });
        }
    }
}

/// **The seat took this session's devices away, or gave them back** — the
/// activity clock's half of a `SessionEvent`, and the one place the instant it
/// is stamped with is sampled (issue #257).
///
/// # Why this is a free function, and why it takes no `now`
///
/// A free function for [`suspend_physical_seat`]'s reason: `DrmState` needs a
/// real `DrmDevice`, `LibSeatSession`, `GbmDevice` and `GlesRenderer`, so
/// anything left inside its `handle_session_event` is unreachable by every test
/// in this workspace — and this arm silently regressing would take nothing red
/// with it. It did, and this function is the fix.
///
/// **No `now` parameter, and that absence is the fix rather than a style
/// choice.** The activate arm used to pass `self.now`, the cell
/// `route_physical_inputs` samples once per *input turn*. A paused session sees
/// no input turn — the keypress that switches the VT back is delivered to
/// whichever session is currently active, never to this one — so that cell still
/// held an instant from before the absence. The clock was therefore "reset" to a
/// moment already past the idle threshold, and:
///
/// * with `--blank-idle 60` the panel went dark **1.5 s** after the human came
///   back, measured on the L4 rung on 2026-08-11 (issue #257's evidence);
/// * and the answer for `--lock-idle` is **not different, it is worse** — the
///   lock's idle raise reads `last_activity` off the same shared record, so the
///   same stale stamp hands a returning human a passphrase prompt they never
///   asked for. That is the stronger claim, so it is stated rather than left
///   implied: one stamp, two timers, one defect.
///
/// # Seat activation counts as activity, and that is the decision (#257)
///
/// A reactivation is always *caused* by a human acting on this machine, even
/// though this session structurally cannot observe the event that caused it. An
/// idle timer asks "is anybody there"; somebody switching to this VT has just
/// answered it. The countdown therefore restarts from the return rather than
/// resuming one frozen mid-way, which is what
/// [`crate::backend::blank::SessionActivity::set_seat_absent`] already intended
/// and documented before this — the intent was right and the stamp was stale.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the only production caller is the bare-metal backend's session-event \
                  handler, because only that backend has a seat; the behaviour is held in \
                  every build by this module's own tests"
    )
)]
pub(crate) fn note_seat_presence(lock: &std::cell::RefCell<crate::lock::LockScreen>, absent: bool) {
    lock.borrow_mut().set_seat_absent(absent, Instant::now());
}

/// **The human can see this session's trusted surfaces again as of `now`** —
/// the one call three different returns make.
///
/// [`ConsentGrab::restart_guard`] clears `armed` and restarts `GUARD_INTERVAL`
/// without touching the petition's own deadline. It is owed whenever "raised"
/// and "visible" came apart and have just come back together, and by WS-E.4.3
/// there are three ways that happens:
///
/// 1. the lock lowered ([`service_lock_round`]'s `Unlocked` arm);
/// 2. the seat came back after a VT switch ([`resume_physical_seat`]);
/// 3. the screen woke from an idle blank, or the machine resumed from a suspend
///    (`crate::backend::drm`).
///
/// Three naked `restart_guard` calls were already a smell; five would mean "the
/// human can see this card again" was asserted from five places, and the sixth
/// would be the one that forgot. So the fact has one name and one site.
///
/// **What it closes**, in the blank's case: a press armed on Allow before the
/// panel went dark, released after the human comes back with the pointer
/// unmoved, would otherwise commit a grant decided against a card that spent its
/// whole guard interval on a screen that was off. `commit` re-checks only the
/// last *physical* pointer position, and going dark does not reset one.
///
/// **Only the guard moves.** The petition's deadline is deliberately untouched:
/// it bounds how long the human has to decide, and a blank, a VT switch or a
/// suspend does not buy them more of it. Refreshing it would also let an agent
/// extend its own petition's life by inducing churn.
pub(crate) fn screen_became_visible(grab: &std::cell::RefCell<ConsentGrab>, now: Instant) -> bool {
    let restarted = grab.borrow_mut().restart_guard(now);
    if restarted {
        tracing::debug!("consent guard restarted: a prompt became visible again");
    }
    restarted
}

/// Send the petitioner its `vitrin_consent.state(shown)`, the wire half of
/// raising a prompt — the one part [`ConsentGrab::raise`] leaves to the caller
/// because only the connection's own [`PrincipalServer`] may speak on the wire.
///
/// Mirrors [`deliver`]'s out-of-dispatch outbox pattern: the petitioner is by
/// definition idle (an agent awaiting consent has nothing in flight), so the
/// `shown` event has no inbound message to ride and goes through the outbox.
///
/// Failure is an **expected** race, not a fault: the petitioner can die in the
/// instant its prompt goes up, and [`PrincipalServer::emit_consent_shown`]
/// reports that as `ConnectionDead`. The flight-recorder `Shown` entry that
/// `raise` already wrote is the source of truth for "a human was asked", so a
/// lost wire event is logged at debug and nothing more — warning here would
/// cry wolf on an ordinary disconnect.
pub(crate) fn announce_prompt<H: PreemptionHook>(runtime: &mut Runtime<H>, route: PromptRoute) {
    let Some(conn) = runtime.conns.get_mut(&route.connection) else {
        return;
    };
    let outbox = conn.outbox.clone();
    let mut send = |frame: &[u8], fd: Option<BorrowedFd<'_>>| {
        debug_assert!(fd.is_none(), "no version-1 consent event carries an fd");
        outbox.send(frame)
    };
    if let Err(err) = conn.server.emit_consent_shown(route, &mut send) {
        tracing::debug!(%err, "consent prompt shown-event not delivered to its petitioner");
    }
}

/// Accept one principal connection and drive a [`PrincipalServer`] on it.
fn accept_principal<H: RuntimeHost>(conn: Connection, host: &mut H) {
    let (source, outbox) = match ConnectionSource::with_outbox(conn) {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(%err, "could not wrap an accepted connection; dropping it");
            return;
        }
    };
    // Read *after* wrapping and before insertion: `peer_cred` is what the
    // transport recorded at `accept4`, the "captured at accept" guarantee the
    // identity layer's sender-constraint triple rests on. Nothing later can
    // re-derive it honestly.
    let peer = source.peer_cred();
    let id = host.runtime().kernel.petitions.register_connection();

    let handle = host.loop_handle();
    let token = match handle.insert_source(source, move |event, conn, host: &mut H| {
        dispatch_principal(host, id, event, conn)
    }) {
        Ok(token) => token,
        Err(err) => {
            tracing::warn!(%err, "could not register an accepted connection; dropping it");
            return;
        }
    };
    // Armed at accept, disarmed at `bound`: an unauthenticated peer must not
    // be able to hold a descriptor and a connection id indefinitely.
    let deadline = handle
        .insert_source(
            Timer::from_duration(HANDSHAKE_DEADLINE),
            move |_, _, host| {
                expire_handshake(host, id);
                TimeoutAction::Drop
            },
        )
        .inspect_err(|err| {
            tracing::warn!(%err, "unauthenticated-phase deadline could not be armed");
        })
        .ok();

    host.runtime().conns.insert(
        id,
        PrincipalConn {
            server: PrincipalServer::new(peer, id),
            token,
            outbox,
            deadline,
        },
    );
    tracing::info!(connection = %id, peer_uid = peer.uid, "principal connection accepted");
}

/// The unauthenticated-phase deadline fired: close the connection unless it
/// bound in the meantime.
///
/// The `is_bound` re-check is not belt-and-braces. The disarm below runs on
/// the dispatch that binds, but calloop is free to have already queued this
/// timer's readiness in the same round, so a connection that authenticated
/// legitimately can still reach here.
fn expire_handshake<H: RuntimeHost>(host: &mut H, id: ConnectionId) {
    let runtime = host.runtime();
    let Some(conn) = runtime.conns.get(&id) else {
        return;
    };
    if conn.server.is_bound() {
        return;
    }
    tracing::warn!(
        connection = %id,
        timeout_s = HANDSHAKE_DEADLINE.as_secs(),
        "principal connection closed: no handshake within the unauthenticated-phase deadline"
    );
    close_principal(host, id, CloseCause::CoreInitiated);
}

/// Who decided a principal connection ends — which determines whether the
/// core must remove the source or the transport already did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseCause {
    /// The core decided: a protocol violation's goodbye, or the
    /// unauthenticated-phase deadline. The source is still registered and
    /// **the core must remove it**.
    CoreInitiated,
    /// The transport decided: clean EOF or a terminal fault. The source has
    /// already removed itself and the token must **not** be removed again.
    TransportInitiated,
}

/// Forget one principal connection, running the teardown the grant table's
/// version-1 lifetime rule depends on.
///
/// [`PrincipalServer::teardown`] must run on **every** close, for any reason.
/// It withdraws the connection's pending petitions (silently — the petitioner
/// is gone, so there is nobody to prompt for) and *removes* the grant rows it
/// held, which is what makes "all of a principal's grants die with its
/// connection" true rather than aspirational. Miss it and the rows survive
/// until their own expiry, the pending petitions keep counting against that
/// identity's admission cap, and the flight recorder cannot see a principal
/// leave.
///
/// It is idempotent and leaves the server `Dead`, which is why every one of
/// the three close paths calls it rather than trying to prove exactly one
/// fires.
fn close_principal<H: RuntimeHost>(host: &mut H, id: ConnectionId, cause: CloseCause) {
    let handle = host.loop_handle();
    let runtime = host.runtime();
    let Some(mut conn) = runtime.conns.remove(&id) else {
        return;
    };
    conn.server.teardown(
        &mut runtime.kernel.petitions,
        &mut runtime.kernel.grants,
        &mut runtime.kernel.recorder,
    );
    if let Some(deadline) = conn.deadline.take() {
        handle.remove(deadline);
    }
    // The asymmetry that must not be inverted: removing an already
    // self-removed token is a use-after-free of a registration slot, and
    // *failing* to remove a still-registered one leaks a source that will
    // dispatch into a connection the core has forgotten.
    if cause == CloseCause::CoreInitiated {
        handle.remove(conn.token);
    }
}

/// Dispatch one event from a principal connection.
fn dispatch_principal<H: RuntimeHost>(
    host: &mut H,
    id: ConnectionId,
    event: ConnectionEvent,
    conn: &mut calloop::generic::NoIoDrop<Connection>,
) {
    match event {
        ConnectionEvent::Message(msg) => {
            // Chokepoint-admitted actuations land here first rather than
            // going straight to the shim: `ServerCtx` already borrows the
            // petition registry, grant table and recorder mutably, so a sink
            // that also touched the router and the shim's outbox would alias
            // the same `Runtime`. Collecting into a local and routing after
            // the borrows end costs one small allocation per actuating
            // message and keeps the borrow structure honest.
            //
            // **Each event carries its own destination** since WS-E.1.6: the
            // realm the grant it was admitted under names. A bare
            // `Vec<SeatInput>` made "which realm" a question the delivery site
            // had to answer for itself, and there was exactly one answer it
            // could give — whichever realm the session was showing.
            let mut seat: Vec<(RealmId, SeatInput)> = Vec::new();
            // Chokepoint-admitted layout acts, collected for exactly the
            // reason `seat` is: applying one needs `&mut` on the presenter
            // and on a realm's outbox, and `ServerCtx` already holds the
            // kernel mutably. Applied by `apply_layout` once the borrows
            // end, in the order the chokepoint admitted them.
            let mut layout_acts: Vec<LayoutAct> = Vec::new();
            // Chokepoint-admitted launches, forked already and waiting to be
            // attached (WS-E.1.1). Collected for the same borrow reason the
            // two sinks above are — attaching needs the loop handle and the
            // whole runtime — but with one difference stated at
            // [`PendingLaunch`]: the part that could *refuse* has already
            // run, inside the sink, because `launched` is a terminal.
            let mut launches: Vec<PendingLaunch> = Vec::new();
            let now = Instant::now();
            let outcome = {
                let (runtime, view) = host.split();
                let (width, height) = view.view_size();
                // Reborrowed shared for the rest of this block: both closures
                // below only *read* the presenter, and a scene must never be
                // minted by the act of asking whether a realm has one.
                let view = &*view;
                let Runtime {
                    kernel,
                    conns,
                    view_cache,
                    realms,
                    shim,
                    ..
                } = runtime;
                // **The single fact behind every `no_surface` refusal.**
                // `RealmLifecycle::view_is_live` is that fact and this is
                // the one place the runtime derives it — deliberately not
                // `scene().surface_size()`, which is only *half* of it: a
                // realm can be dead with its scene not yet recomposited,
                // and asking the scene would then photograph a corpse. A
                // realm the runtime has no shim session for is likewise not
                // live.
                //
                // **Asked per realm, never "is any realm live".** With one
                // realm those were the same question; with several they are
                // not, and `any` is fail-**open** across realms — realm A's
                // grant would clear the gate because sibling B is alive, and
                // then capture the scene B committed into. So the answer is
                // a *function of the realm id*, and
                // `PrincipalServer::serve_facet_use` applies it to the realm
                // the grant row names. It is asked against **that realm's own
                // scene** since WS-E.1.3; before that there was one scene to
                // ask, which is why the leak below existed at all.
                let realm_is_live = |realm_id: &RealmId| {
                    let Some(scene) = view.scene(realm_id) else {
                        // No scene at all: the realm has never committed and
                        // has never held the output. Nothing to photograph.
                        return false;
                    };
                    realms
                        .get(realm_id)
                        .is_some_and(|realm| realm.life.view_is_live(scene))
                };
                // **THE selection decision 1 is about, and it is deliberately
                // a function of the realm id rather than a value.**
                //
                // A capture must return *the granted realm's* pixels, never
                // whatever is on the output. Until WS-E.1.3 this was one
                // `Option<RealmViewFrame>` built from one session-wide cache,
                // so an `observe` grant over realm A returned realm B's frame
                // the instant B committed — nothing in that code was wrong
                // (there was one realm) and nothing in it prevented the leak
                // either. Making the frame a function of the id is what makes
                // the leak unrepresentable: there is no "the view" left to
                // hand the chokepoint by mistake.
                //
                // `PrincipalServer::serve_facet_use` is the one caller, and
                // it applies this to the realm the **grant row** names, on
                // the same line it applies `realm_is_live` — the two halves
                // of "the view of the realm this grant is about" resolved
                // together, beside each other, or not at all.
                //
                // One `(width, height)` for every realm: there is one output
                // and decision 3 keeps it that way, so every realm's view is
                // composed at the output's size (`start_realm_in` configures
                // every shim with it).
                //
                // The cache itself is refreshed at redraw time, never here,
                // so capture stays a pure read of the last completed frame.
                let realm_view = |realm_id: &RealmId| {
                    view_cache.get(realm_id).map(|rgba| RealmViewFrame {
                        rgba: rgba.as_slice(),
                        width,
                        height,
                    })
                };
                // **Where the human's own hand currently is** — the realm
                // physical input follows, which a `layout_focus` holder moves.
                // The chokepoint consults it for one judgement only:
                // `preempted` for a *layout* request, the one
                // attention-contending use that steals from wherever the human already is rather
                // than acting on the realm its grant names. Asked once per
                // dispatch turn because it is one answer for the whole session
                // — unlike liveness, which is a question per realm.
                //
                // Until WS-E.1.6 this was the write-side *comparison* ("does
                // the session's one seat serve this grant's realm"), and an
                // actuation that failed it was refused `internal`. Seat
                // delivery is per realm now, so there is nothing to compare.
                let physical_realm =
                    seat_target(realms, view.focused()).map(|(realm_id, _)| realm_id);
                let Some(state) = conns.get_mut(&id) else {
                    return;
                };
                // The realm is cloned into the batch rather than borrowed: the
                // batch outlives this borrow of `realms`, and `route_seat`
                // needs to know each event's destination after the connection
                // dispatch has finished.
                let mut actuations =
                    |realm: &RealmId, input: SeatInput| seat.push((realm.clone(), input));
                let mut layout = |act: LayoutAct| layout_acts.push(act);
                // **The launch sink** (WS-E.1.1): the one closure in this
                // core through which a wire request can make the trusted
                // core fork. It borrows the realm registry *shared* — the
                // same borrow `ServerCtx::realms` takes for petition
                // admission — which is exactly why instance ids are minted
                // through a `Cell` on the registry rather than from a
                // counter someone else could keep: one naming authority,
                // reachable from both.
                let registry = &kernel.realms;
                let shim_bin = shim.as_path();
                let mut launch = |ask: LaunchAsk<'_>| {
                    // Resolved per launch rather than per dispatch: this
                    // reads `$XDG_RUNTIME_DIR` and can fail, and a launch is
                    // rate-limited and rare while messages are not. A failure
                    // is the IDL's `internal` -- a session whose runtime tree
                    // cannot be named can create nothing.
                    let paths = SpawnPaths::from_env(shim_bin.to_path_buf()).map_err(|err| {
                        tracing::error!(%err, "launch could not name this session's runtime tree");
                        LaunchRefusal::Internal
                    })?;
                    launch_realm(registry, &paths, &mut launches, ask)
                };
                // Borrowed for this one message's dispatch and dropped with
                // `ctx` at the end of the block. Nothing between here and
                // there routes input — the router's own `borrow_mut` runs in
                // `route_physical_turn`/`route_seat`, after the connection
                // dispatch has finished — so this borrow cannot collide with
                // the tap that writes the map.
                let presence = kernel.presence.borrow();
                // The handle, cloned rather than borrowed: the chokepoint
                // *writes* it (claiming the human's attention window at step-6
                // admission), and `kernel` is already borrowed field-wise for
                // the grant table and the recorder. Nothing else in this turn
                // holds a borrow of the cell — the hook that opens a window
                // runs in `route_physical_turn`, after connection dispatch has
                // finished.
                let attention = std::rc::Rc::clone(&kernel.attention);
                let mut ctx = ServerCtx {
                    verifier: &kernel.verifier,
                    petitions: &mut kernel.petitions,
                    realms: registry,
                    grants: &mut kernel.grants,
                    now,
                    realm_view: &realm_view,
                    realm_is_live: &realm_is_live,
                    physical_realm,
                    presence: &presence,
                    attention: &attention,
                    actuations: &mut actuations,
                    layout: &mut layout,
                    launch: &mut launch,
                    recorder: &mut kernel.recorder,
                };
                let mut send =
                    |frame: &[u8], fd: Option<BorrowedFd<'_>>| vitrin_ipc::reply(conn, frame, fd);
                let result = state.server.handle_message(msg, &mut ctx, &mut send);
                // Disarm the moment the handshake succeeds, per conventions
                // 7.1 — the deadline exists to bound the *unauthenticated*
                // phase, not the session.
                let disarm = result.is_ok() && state.server.is_bound() && state.deadline.is_some();
                (result.is_err(), disarm, state.deadline)
            };
            let (fatal, disarm, deadline) = outcome;
            if disarm {
                if let Some(token) = deadline {
                    host.loop_handle().remove(token);
                    if let Some(state) = host.runtime().conns.get_mut(&id) {
                        state.deadline = None;
                    }
                }
            }
            if fatal {
                // **Launches first, even here — especially here.** A launch
                // that reached this point has already forked and exec'd a
                // process; whatever happened to the socket afterwards, that
                // process exists and the journal owes an entry naming who
                // asked for it. Returning without this dropped the whole
                // `Vec<PendingLaunch>` and every `SpawnRecord` in it, so a
                // grant holder could defeat the one compensating control this
                // verb offers — "the log can always answer who started this
                // app" — by making its own connection fault in the same turn
                // it launched. Issue #207's review found it; the `#[must_use]`
                // that was supposed to prevent it says nothing about a value
                // moved into a struct field inside a `Vec`.
                //
                // Attaching (rather than killing) the realm is the same answer
                // the rest of this design gives: a launched realm outlives the
                // connection that asked for it, exactly as one from
                // `realm.toml` outlives the startup that read it.
                apply_launches(host, launches);
                // The goodbye is already on the wire and the violation is
                // already logged; `handle_message` cannot run teardown
                // because it holds no kernel state. This is the third close
                // path — the one that reaches teardown only because the
                // embedder brings it here — and the source is still
                // registered, so the core removes it.
                close_principal(host, id, CloseCause::CoreInitiated);
                return;
            }
            // Layout first, then input, and the reason survived WS-E.1.6
            // in a narrower form. An admitted actuation now carries its own
            // realm, so this ordering no longer decides where it lands — but
            // a `focus` in the same dispatch turn still has to move the
            // binding before anything else runs, because `apply_layout` is
            // where the realm being left is paid the physical presses it is
            // holding. Deferring that past the round's deliveries would leave
            // the drain chasing a binding that had already moved.
            apply_layout(host, layout_acts);
            route_seat(host, seat);
            // **Last**, and it does not compete with the two above: a
            // launched realm is new, so no act in this turn can be about it
            // and nothing here can move the output or the seat. Running it
            // after keeps the ordering rule the two above encode — layout
            // before deliveries — untouched by a third participant.
            apply_launches(host, launches);
        }
        // Both terminal variants: the source has already removed itself, so
        // the core only forgets the connection and tears it down.
        ConnectionEvent::Disconnected => {
            tracing::info!(connection = %id, "principal connection closed");
            close_principal(host, id, CloseCause::TransportInitiated);
        }
        ConnectionEvent::Fault(reason) => {
            tracing::info!(connection = %id, %reason, "principal connection terminated");
            close_principal(host, id, CloseCause::TransportInitiated);
        }
    }
}

/// **The realm the human's own physical input goes to.** The realm the output
/// is bound to, when one is bound and still serving; otherwise the first
/// still-serving realm in id order.
///
/// # One question, not two, since WS-E.1.6 (issue #212)
///
/// This used to be "the realm *every* seat delivery goes to" — the human's and
/// every agent's alike — because the router held one realm's worth of state
/// and so exactly one realm could be its target. That is no longer true: the
/// router keeps a seat per realm and an admitted actuation carries the realm
/// its grant names ([`route_seat`]). So this function answers the *physical*
/// half only, which is the half no grant addresses.
///
/// # The binding half is not a placeholder — it is D-018's fifth ordering rule
///
/// A `layout_focus` holder moves the output binding
/// ([`Presenter::bind_output`]), and this function is what makes the human's
/// own keyboard and pointer move with it. Serving the verb without this
/// would let a holder show realm A while the human's keystrokes kept
/// reaching realm B, which is focus theft in its sharpest form and is
/// exactly what "focus is one act" exists to forbid (IDL
/// `vitrin_layout_focus`, SCENE AUTHORITY's fifth rule). So the binding is
/// consulted **first**, and no verb set separates the two halves.
///
/// # The fallback half is a default, and now a small one
///
/// With nothing bound — before the first realm attaches, or after the bound
/// one died with no sibling to take the output — this answers "the first
/// still-serving realm in id order". A human types into whatever that names,
/// and mis-aiming it is a **usability** bug: no authority rides on it, because
/// physical input is authorized by nothing and addressed by attention alone.
/// The bug it used to be able to cause — an *agent's* authorized keystroke
/// landing in a realm its grant does not name — is structurally gone, because
/// the agent path no longer consults this function at all.
///
/// The relationship with the output binding is the reverse of what it once
/// was: [`start_one_realm_in`] and [`rebind_output_after_death`] pick a
/// realm and *bind* it, and this function follows that binding rather than
/// re-deriving the same rule beside it. Both orders keep "the realm a human
/// is watching" and "the realm the human's input reaches" equal; only this
/// one keeps them equal after a client moves the binding.
///
/// "Still serving" rather than plain "first": a dead realm keeps its map
/// entry until shutdown (the registry and the runtime both outlive the
/// process), so skipping to the first realm that still holds a
/// [`ShimServer`] is what keeps one realm's death from silently swallowing
/// the human's input for the rest of the session.
///
/// # What the chokepoint still asks it
///
/// One thing: `preempted` for a **layout** request
/// ([`enforcement::UseEnv::physical_realm`], fed from [`dispatch_principal`]).
/// A layout act is not delivered into a realm; it moves what the human is
/// looking at, so the human it can steal from is the one wherever this
/// function points. The comparison it used to feed — "does the session's one
/// seat serve this grant's realm", refused `internal` when it did not — is
/// deleted, and with it the cross-principal denial-of-service surface a
/// `layout_focus` holder had over *other* principals' actuations.
///
/// # Private, and that is the fifth ordering rule's enforcement
///
/// `focused` is an `Option` because [`rebind_output_after_death`] genuinely
/// has no binding to consult — it runs *because* the bound realm just died,
/// so following the old binding would answer with the corpse. That argument
/// is therefore a real one, and it is also the exact shape a reverting edit
/// takes: passing `None` at a delivery site silently restores "type into
/// whichever realm sorts first". So this function is private to this module,
/// and the only thing outside it may call is [`physical_seat_target`], which
/// takes the scene registry and has no such argument to get wrong.
///
/// [`enforcement::UseEnv::physical_realm`]: crate::enforcement::UseEnv::physical_realm
fn seat_target<'r>(
    realms: &'r BTreeMap<RealmId, RealmRuntime>,
    focused: Option<&RealmId>,
) -> Option<(&'r RealmId, &'r RealmRuntime)> {
    // The bound realm first, and only if it is still serving: a realm whose
    // shim died keeps its map entry, and following the binding into a corpse
    // would swallow every actuation aimed at the session — the exact failure
    // "still serving" was added to prevent on the fallback path.
    if let Some(bound) = focused {
        if let Some((id, realm)) = realms.get_key_value(bound) {
            if realm.server.is_some() {
                return Some((id, realm));
            }
        }
    }
    realms.iter().find(|(_, realm)| realm.server.is_some())
}

/// Carry out chokepoint-admitted layout acts (WS-E.1.4, issue #210).
///
/// **This function makes no authority judgement**, exactly as [`route_seat`]
/// makes none: an act reached here only by being admitted at the single
/// enforcement chokepoint, and a delivery site that re-decided would be the
/// second enforcement site this crate does not have. What it does re-check
/// is *liveness*, which is not authority: a realm's shim can be torn down
/// between admission and application, and binding the output to a corpse or
/// writing `configure` into a dead outbox is a runtime condition rather than
/// a refusal to voice.
///
/// # Why the output binding is (almost) the whole of "focus"
///
/// `Presenter::bind_output` is what a `focus` *decides*, and it is enough for
/// the routing half because [`seat_target`] follows the binding — so the
/// human's own keyboard and pointer move with the picture, in one act, per
/// SCENE AUTHORITY's fifth ordering rule.
///
/// It is not enough for the realm being left. That realm's pairing tables are
/// a record of the presses **its app** was told about, and the human's next
/// release is addressed to the realm that just gained the binding. Every
/// physical entry left behind is then a press whose release the losing app can
/// never receive — a latched `Ctrl`, or a wedged pointer grab, in an app the
/// human can no longer even see, since the output has moved. So the losing
/// realm is paid what it is owed *here*, before the binding moves, through the
/// same seat funnel an ordinary release takes: [`InputRouter::bind_to`] drains
/// the keys and the buttons and hands them back, naming the realm they are
/// owed to.
///
/// **An agent's held presses in that realm are deliberately untouched**, which
/// is what changed at WS-E.1.6: the agent still reaches the realm it holds a
/// grant over, so it can still release its own key, and synthesising one here
/// would invent an act the principal never performed and attribute it to the
/// human on the wire and in the journal.
///
/// This is the same failure WS-E.1.2 fixed one layer down (a sibling realm's
/// death resetting the shared router and latching a key in the survivor), and
/// [`InputRouter::bind_to`]'s own docs used to be able to say "the only way
/// the target changes today is the previous one dying, which has already
/// reset". Serving `layout_focus` made that false, which is what makes this
/// arm's drain load-bearing rather than defensive.
///
/// # What no layout act can reach
///
/// The consent surface, the trust indicator and the agent-cursor sprite are
/// composited at the **output stage**, downstream of the `Scene::compose`
/// this function's binding selects (P1.7.1, D-019(3)). So invariants 1 and 3
/// of D-018(2) are not checks performed here — there is no expressible act
/// whose application could reach them, which is a stronger property than a
/// guard and is why no guard appears below.
fn apply_layout<H: RuntimeHost>(host: &mut H, acts: Vec<LayoutAct>) {
    if acts.is_empty() {
        return;
    }
    let (runtime, view) = host.split();
    let view_size = view.view_size();
    let mut rebound = false;
    for act in acts {
        match act {
            LayoutAct::Focus { realm } => {
                // Liveness, not authority (see above). A realm whose shim
                // died after admission would otherwise take the output into
                // the state `rebind_output_after_death` exists to leave.
                if runtime
                    .realms
                    .get(&realm)
                    .is_none_or(|r| r.server.is_none())
                {
                    tracing::debug!(
                        %realm,
                        "focus admitted for a realm whose shim is gone; the output stays put"
                    );
                    continue;
                }
                if view.focused() == Some(&realm) {
                    continue;
                }
                // The releases the realm losing the human's attention is
                // owed, delivered before the binding moves (see above).
                // `bind_to` names its own debtor and hands back exactly what
                // that app is owed, so nothing here has to guess which app saw
                // the presses — or which of them were the human's.
                let Runtime {
                    router,
                    realms,
                    kernel,
                    ..
                } = &mut *runtime;
                if let Some((losing, owed)) = router.bind_to(&realm) {
                    // `bind_to` has already forgotten `losing`'s physical
                    // presence — one act with the drain, inside the router, so
                    // no call site can pay the releases and leave the realm
                    // "owned" (`InputRouter::forget_presence_of`).
                    for delivery in owed {
                        tracing::debug!(
                            realm = %losing,
                            "releasing a press held across a focus change so it cannot latch \
                             in an app the human can no longer see"
                        );
                        deliver_seat_to(realms, &mut kernel.recorder, &losing, &delivery);
                    }
                }
                tracing::info!(%realm, "layout_focus: the output and the human's input move here");
                view.bind_output(&realm);
                rebound = true;
            }
            LayoutAct::Arrange { realm, mode } => {
                let Some(entry) = runtime.realms.get_mut(&realm) else {
                    continue;
                };
                entry.arrangement = mode;
                // Only fullscreen imposes a size. Windowed is an *absence*:
                // the core sends nothing and the realm keeps whatever size
                // it has, which is what "the core never invents a window
                // size" means in code (IDL `set_fullscreen`).
                if mode != LayoutMode::Fullscreen {
                    continue;
                }
                reconfigure_realm(&realm, entry, view_size);
            }
        }
    }
    if rebound {
        // The output shows a different realm now, and on a backend whose
        // frame clock is external the dirty flag alone composites nothing —
        // the same pairing `rebind_output_after_death` documents.
        runtime.dirty = true;
        view.request_present();
    }
}

/// **One forked-but-not-yet-attached realm**, carried out of the
/// enforcement chokepoint's launch sink so [`apply_launches`] can finish it
/// once the dispatch borrows have ended (WS-E.1.1, issue #207).
///
/// The split is not a convenience. `vitrin_launcher.launched` is a
/// **terminal**, so everything that can refuse a launch has to happen
/// before the reply — the realm cap, and the fork itself, which is why the
/// fork is inside the sink rather than here. What is left over is the
/// attach sequence, and none of it can turn a forked realm back into a
/// refusal: it either serves, or the realm dies the way any realm dies.
struct PendingLaunch {
    /// The journal entry the spawn owes. The sink cannot write it — a
    /// `ServerCtx` already holds the recorder mutably — so it travels here
    /// and is `#[must_use]` the whole way (`spawn::SpawnRecord`).
    record: spawn::SpawnRecord,
    /// The realm the instance's configuration came from.
    template: RealmId,
    /// The core-minted instance id, already sent to the client. `None` when
    /// the spawn failed, in which case the client already has its
    /// `refused(realm_launch, internal)` and only the journal is left.
    spawned: Option<(crate::realm::MintedRealmId, spawn::SpawnedRealm)>,
}

/// **The wire-reachable spawn path**: what the chokepoint's launch sink
/// does once a `vitrin_launcher.launch` has passed the whole authority
/// chain (WS-E.1.1, issue #207).
///
/// Everything refusable is here, synchronously, because the caller is about
/// to send a terminal event:
///
/// 1. **The cap.** [`crate::realm::MAX_REALMS`] against
///    `RealmRegistry::capacity_used` plus whatever this dispatch turn has
///    already queued. Refused `capacity` — a policy answer, which is why
///    the IDL gave it a code of its own rather than folding it into
///    `internal`.
/// 2. **The id.** Minted by the registry, never supplied: the return type
///    is `MintedRealmId`, which nothing outside `crate::realm` constructs.
/// 3. **The fork.** `spawn::spawn_realm_deferring_journal` runs the same
///    PRD Doc 2 §4.1 sequence startup runs, including the spawn-time
///    re-audit of the program — so a `command` that became writable since
///    load is refused here rather than exec'd hours later. Any failure is
///    the IDL's `internal`.
///
/// **The command is not a parameter and cannot be.** The only realm this
/// function reads is `ask.template`, which the chokepoint resolved from the
/// grant row; `launch` carries no arguments on the wire, so there is no
/// path by which a principal names the program.
fn launch_realm(
    realms: &crate::realm::RealmRegistry,
    paths: &SpawnPaths,
    queued: &mut Vec<PendingLaunch>,
    ask: LaunchAsk<'_>,
) -> Result<crate::realm::MintedRealmId, LaunchRefusal> {
    // The cap first: a refusal that creates nothing must not first create a
    // runtime directory. `queued` is counted because the registry insert is
    // deferred to `apply_launches` — one dispatch turn carries at most one
    // launch today, and counting it anyway is what keeps that from being a
    // load-bearing assumption.
    if realms.capacity_used() + queued.len() >= crate::realm::MAX_REALMS {
        tracing::info!(
            template = %ask.template,
            principal = %ask.principal,
            cap = crate::realm::MAX_REALMS,
            "refusing a launch: the session is at its realm capacity"
        );
        return Err(LaunchRefusal::Capacity);
    }
    let Some(minted) = realms.mint_instance(ask.template) else {
        // Unreachable: the template came from a grant row, and rows are
        // only minted over realms the registry resolved. Fail closed.
        tracing::error!(
            template = %ask.template,
            "launch admitted over a realm the registry does not hold"
        );
        return Err(LaunchRefusal::Internal);
    };
    // The spawn reads the *template's* configuration and writes the
    // *instance's* paths, so it is handed a realm value built from both.
    // Deliberately built here rather than inserted into the registry first:
    // a registry that gained a row for a fork that then failed would answer
    // petitions about a realm that never existed.
    let Some(instance) = realms.instance_of(ask.template, &minted) else {
        tracing::error!(template = %ask.template, "template vanished between mint and spawn");
        return Err(LaunchRefusal::Internal);
    };
    let (result, record) = spawn::spawn_realm_deferring_journal(
        &instance,
        paths,
        spawn::SpawnOrigin::Launch {
            principal: ask.principal,
            grant: ask.grant,
        },
        |name| std::env::var(name).ok(),
    );
    match result {
        Ok(spawned) => {
            queued.push(PendingLaunch {
                record,
                template: ask.template.clone(),
                spawned: Some((minted.clone(), spawned)),
            });
            Ok(minted)
        }
        Err(err) => {
            tracing::warn!(
                instance = %minted,
                principal = %ask.principal,
                %err,
                "a launch could not fork; refusing internal"
            );
            queued.push(PendingLaunch {
                record,
                template: ask.template.clone(),
                spawned: None,
            });
            Err(LaunchRefusal::Internal)
        }
    }
}

/// Finish every launch this dispatch turn forked: journal it, enter it in
/// the registry, and attach its shim session to the loop.
///
/// Runs after the connection dispatch's borrows have ended, for exactly the
/// reason [`apply_layout`] does — attaching needs the loop handle, the
/// presenter and the whole runtime, and `ServerCtx` holds the kernel
/// mutably. **Before the next message is dispatched**, which is what makes
/// the deferral invisible on the wire: the client received `launched(id)`
/// and the realm is in the registry before anything it sends next is read.
///
/// A failure here is a realm that died immediately, not a launch that
/// should have been refused: the client already holds its terminal, and the
/// answer it gets from then on is `unavailable` (the row is removed) or
/// `no_surface` — the same answers any realm whose shim dies produces.
fn apply_launches<H: RuntimeHost>(host: &mut H, launches: Vec<PendingLaunch>) {
    if launches.is_empty() {
        return;
    }
    let (width, height) = {
        let (_, view) = host.split();
        view.view_size()
    };
    for launch in launches {
        // The journal first, always, and for both outcomes: what the
        // trusted core executed — or tried to — is the most
        // security-relevant act of a session, and a later failure must not
        // be able to swallow the entry naming who asked.
        launch.record.journal(&mut host.runtime().kernel.recorder);
        let Some((minted, spawned)) = launch.spawned else {
            continue;
        };
        if !host
            .runtime()
            .kernel
            .realms
            .insert_instance(&launch.template, minted.clone())
        {
            tracing::error!(
                template = %launch.template,
                instance = %minted,
                "template vanished before its instance could be registered"
            );
            continue;
        }
        if let Err(err) = attach_spawned_realm(host, spawned, width, height) {
            tracing::error!(
                instance = %minted,
                %err,
                "a launched realm forked but could not be attached; dropping it"
            );
            // Removed rather than marked `Exited`: `Exited` carries the pid
            // of a process that *served* the realm, and this one never did.
            // The client sees `unavailable`, which is what an unknown name
            // and a vacant realm both answer — deliberately indistinguishable
            // (IDL `get_realm`).
            host.runtime().kernel.realms.remove_instance(&minted);
        }
    }
}

/// Re-send `configure` at `size` to one realm's shim, if that changes what
/// the shim was last told.
///
/// The one place a realm's view size is imposed. Two callers, and the pair is
/// exactly what `set_fullscreen`'s normative wire semantics say: entering the
/// fullscreen arrangement ([`apply_layout`]) and every later output resize
/// while the realm is still in it ([`apply_output_resize`]). A failure is a
/// runtime condition, not an authority answer — the shim's death is the
/// transport's to classify — so it is logged and swallowed here, exactly as
/// the seat delivery path does.
fn reconfigure_realm(realm: &RealmId, entry: &mut RealmRuntime, size: (u32, u32)) {
    let Some(server) = entry.server.as_mut() else {
        return;
    };
    let outbox = &entry.outbox;
    let mut send = |frame: &[u8]| outbox.send(frame);
    match server.reconfigure(size.0, size.1, &mut send) {
        Ok(true) => tracing::info!(
            %realm, w = size.0, h = size.1,
            "the realm's view now tracks the output"
        ),
        // Already that size: the two arrangements are indistinguishable
        // here and the IDL says so.
        Ok(false) => {}
        Err(err) => tracing::warn!(
            %realm, %err,
            "re-configuring the realm failed; the shim's death is the transport's to classify"
        ),
    }
}

/// **The output resized.** Propagate the new size to every realm's scene
/// geometry, and re-configure every realm currently in the fullscreen
/// arrangement.
///
/// The second half is the normative half. `vitrin_layout_arrange.set_fullscreen`
/// says, in the IDL's own words, that fullscreen means the realm's view size
/// *tracks* the output's: `configure` carries the output's size on entering
/// the mode "and again whenever the output resizes while the realm is in it".
/// Entering the mode is [`apply_layout`]'s; this function is the "and again",
/// and without it [`RealmRuntime::arrangement`] would be a field nothing ever
/// read and four surfaces would be describing behaviour that did not exist.
///
/// **Every fullscreen realm, not just the bound one.** Hidden realms compose
/// at the output's size too (one output, one size — `Presenter::view_size`),
/// so a hidden realm left at the old size would be letterboxed the moment it
/// was focused, which is precisely what `windowed` means and precisely what
/// it did not ask for. A windowed realm is skipped: the core imposes no size
/// on it at all, and `Scene::compose` letterboxes whatever it keeps
/// committing.
///
/// Called by the backend that owns the output, on its own resize event — the
/// nested backend's `Resized` handler is the only production caller, because
/// the headless virtual output never resizes.
pub(crate) fn apply_output_resize<H: RuntimeHost>(host: &mut H, size: (u32, u32)) {
    let (runtime, view) = host.split();
    // The scene registry first: it bumps every realm's `layout_generation`
    // (D-018(5), decision 4) — deliberately not `Scene::generation`, which
    // the damage path keys on and which a resize is not a repaint of.
    view.set_view_size(size);
    for (realm_id, entry) in runtime.realms.iter_mut() {
        if entry.arrangement != LayoutMode::Fullscreen {
            continue;
        }
        reconfigure_realm(realm_id, entry, size);
    }
}

/// Hand one already-routed seat delivery to **this named realm's** shim, and
/// journal it.
///
/// The funnel for every *single* delivery, whatever produced it: the human's
/// own physical input ([`deliver_physical`]) and the releases a focus change
/// owes the realm it moved away from ([`apply_layout`]). [`route_seat`]'s
/// batch loop takes the same three steps in the same order and additionally
/// stops producing for a shim that has stopped reading, which is a property
/// of a batch and has no meaning for one event. Sharing this keeps the flight
/// recorder's `seat_delivered` entry — the only place the unforgeable
/// physical-vs-emulated distinction is investigable after an incident — from
/// being written by sites that could drift.
///
/// Recorded only when the frame actually went out: a seat the shim has not
/// minted yet drops the event, and nothing was delivered, so nothing is
/// audited as delivered.
fn deliver_seat_to(
    realms: &BTreeMap<RealmId, RealmRuntime>,
    recorder: &mut crate::recorder::Recorder,
    realm_id: &RealmId,
    delivery: &crate::input::SeatDelivery,
) {
    let Some(realm) = realms.get(realm_id) else {
        return;
    };
    let Some(server) = realm.server.as_ref() else {
        return;
    };
    let mut send = |frame: &[u8]| realm.outbox.send(frame);
    match server.deliver_seat_event(delivery, &mut send) {
        Ok(sent) => {
            if sent {
                crate::input::record_seat_delivery(recorder, realm_id, delivery);
            }
        }
        Err(err) => {
            tracing::warn!(realm = %realm_id, %err, "seat delivery to the realm failed");
        }
    }
}

/// **Where the human's own physical input goes**, and the only way anything
/// outside this module may ask.
///
/// [`seat_target`] is private precisely so this is the only answer available
/// to a backend: it takes the scene registry rather than an
/// `Option<&RealmId>`, so "supply the binding yourself" — and in particular
/// "supply `None`" — is not expressible at a call site. That is D-018(2)'s
/// fifth ordering rule made structural: reverting a backend to the
/// pre-WS-E.1.4 behaviour of typing into whichever realm sorts first is a
/// compile error rather than a one-character edit, and the only remaining
/// place the binding could be dropped is this function's own body, which
/// `the_humans_input_follows_the_realm_a_focus_holder_bound` drives over a
/// real wire.
///
/// Fed by the nested backend at all three of its seat sites — the surface
/// geometry it maps with, the router generation it binds, and the shim it
/// delivers to — so the three cannot disagree about which realm this turn is
/// about.
pub(crate) fn physical_seat_target<'r>(
    realms: &'r BTreeMap<RealmId, RealmRuntime>,
    scenes: &crate::scene::realms::RealmScenes,
) -> Option<(&'r RealmId, &'r RealmRuntime)> {
    seat_target(realms, scenes.focused())
}

/// Hand one routed **physical** seat event to the realm the output is bound
/// to, and journal it.
///
/// Physical input reaches the realm's seat over the same outbox an agent's
/// chokepoint-admitted actuation uses; the origin tag bound at intake rides
/// the wire unchanged (B2). This is the *only* site that produces
/// `origin="physical"` at runtime — a human's input reaching the app is the
/// half of the physical-vs-emulated audit that never crosses a chokepoint —
/// so sharing [`deliver_seat_to`] with the agent path keeps the two from
/// silently diverging.
///
/// A free function here rather than a method on the nested backend for two
/// reasons. It is the physical twin of [`route_seat`] and belongs beside it;
/// and the backend's own methods need a display, so a test can reach this and
/// cannot reach them — which is the difference between the fifth ordering
/// rule being *tested* and being asserted about a function nobody calls.
pub(crate) fn deliver_physical(
    realms: &BTreeMap<RealmId, RealmRuntime>,
    scenes: &crate::scene::realms::RealmScenes,
    recorder: &mut crate::recorder::Recorder,
    delivery: crate::input::SeatDelivery,
) {
    let Some((realm_id, _)) = physical_seat_target(realms, scenes) else {
        tracing::trace!(
            origin = ?delivery.origin(),
            "routed input dropped: no serving realm attached"
        );
        return;
    };
    deliver_seat_to(realms, recorder, realm_id, &delivery);
}

/// One turn of the human's own physical input: bind the human's attention to
/// the realm the output shows, pay whatever the realm being left is owed, map
/// through the target realm's geometry, route, and deliver.
///
/// **The display-free tail of the nested backend's input handler**, split out
/// for the reason [`crate::backend::winit::route_turn`] and `deadman_tick`
/// were: CI has no display (D-019(4)), so anything left inside a
/// `NestedState` method is unreachable by every test in this workspace. That
/// is how D-018(2)'s fifth ordering rule came to have production wiring a
/// reviewer could revert with the suite still green.
///
/// All three seat questions this turn asks are answered by
/// [`physical_seat_target`] — the surface geometry to map with, the realm to
/// bind the human's attention to, and the shim to deliver to — so they cannot
/// disagree about which realm the human is typing into.
///
/// `switch` is `None` on a backend with no dead-man watcher stacked (a
/// `physical-input-injector` headless build); see
/// [`crate::backend::winit::route_turn`].
///
/// `now` is **this turn's one clock sample**, and it is a parameter rather
/// than something read here because the embedder has already taken it for the
/// consent grab and the dead-man watcher and every event of the turn must be
/// judged against the same instant. It is pushed straight into the router's
/// presence tap ([`InputRouter::observe_at`]) before anything is routed: the
/// tap's timestamps and the chokepoint's `preempted` window are then one
/// timeline by construction, and there is no separate "remember to advance the
/// presence clock" step an embedder can omit — which is the class of omission
/// that left `preempted` unreachable in every shipped build until issue #212's
/// review.
///
/// # The drain, and why it is here as well as in [`apply_layout`]
///
/// [`InputRouter::bind_to`] hands back the physical presses the realm being
/// left is owed, and they are delivered *to that realm* before this turn's
/// events are routed. In the ordinary run `apply_layout` has already paid
/// them at the instant the binding moved, so this finds nothing — but the
/// binding can also move without any layout act at all
/// (`rebind_output_after_death`, or the first realm attaching), and a drain
/// that only ran on the verb would miss those. Paying twice is impossible:
/// the drains empty the table.
pub(crate) fn route_physical_turn<H: crate::input::PreemptionHook>(
    runtime: &mut Runtime<H>,
    scenes: &crate::scene::realms::RealmScenes,
    switch: Option<&std::cell::RefCell<crate::deadman::DeadManSwitch>>,
    inputs: impl IntoIterator<Item = SeatInput>,
    view: (u32, u32),
    now: Instant,
) {
    let Runtime {
        router,
        realms,
        kernel,
        ..
    } = runtime;
    // The turn's instant, into the tap, before a single event of it is
    // observed (doc comment above).
    router.observe_at(now);
    // **The seat target's own surface geometry** (WS-E.1.3): the router maps
    // view coordinates to surface coordinates through `layout::place`, so it
    // must be handed the geometry of the surface the event is about. With one
    // scene those were the same thing; with several, a hidden realm's
    // committed size would silently place a human's click for the app being
    // typed into.
    let surface = physical_seat_target(realms, scenes)
        .and_then(|(realm_id, _)| scenes.scene(realm_id))
        .and_then(|scene| scene.surface_size());
    // Before the routing, not after: the routing writes this turn's presses
    // into the bound realm's pairing table, and that debt has to be on record
    // as *this realm's* from the first one.
    if let Some((realm_id, _)) = physical_seat_target(realms, scenes) {
        let realm_id = realm_id.clone();
        if let Some((losing, owed)) = router.bind_to(&realm_id) {
            // The human's attention left `losing`, so their next release is
            // addressed elsewhere: pay that app its releases. `bind_to` has
            // already dropped that realm's physical presence, or a button held
            // across the move would keep it "owned" — and every agent in it
            // refused `preempted` — for the stale-hold ceiling with nobody
            // touching it (`InputRouter::forget_presence_of`).
            for delivery in owed {
                tracing::debug!(
                    realm = %losing,
                    "releasing a press held across a binding change so it cannot latch in an \
                     app the human can no longer see"
                );
                deliver_seat_to(realms, &mut kernel.recorder, &losing, &delivery);
            }
        }
    }
    crate::backend::winit::route_turn(router, switch, inputs, view, surface, &mut |delivery| {
        deliver_physical(realms, scenes, &mut kernel.recorder, delivery)
    });
    // After the routing, never before: the press the attention hook gated is
    // one of *this* turn's events, and the window it opens is measured from the
    // human's gesture rather than from the delivery round.
    open_attention_window(runtime, now);
    // ...and the clipboard chords, on the same terms and for the same reason
    // (WS-E.2.1, issue #213): the hook queued a gesture, and only here is the
    // realm the output is bound to, the shim connection and the slot all in
    // scope at once.
    drain_clipboard_gestures(runtime, scenes, now);
    // ...and the screenshot chord, last (WS-E.2.4, issue #216). Last because
    // it is the only one of the three that writes to a filesystem: whatever
    // its latency, an attention window and a clipboard gesture from the same
    // turn are already resolved before a `write(2)` is attempted.
    drain_screenshot_gestures(runtime, scenes, view);
}

/// **The seat has taken this session's input devices away** — a VT switch on
/// bare metal — so cancel the gesture in progress and pay every app the
/// releases it will otherwise never hear (WS-E.3.3, D-030(8)).
///
/// Returns how many releases were delivered, which is what makes this
/// assertable: the counterpart drains empty the router's tables, so a test that
/// only read the tables afterwards could not tell "released" from "forgotten".
///
/// # A free function here rather than a method on the backend
///
/// The same split [`route_physical_turn`] and `crate::backend::winit::route_turn`
/// were made for, and for the same reason: `DrmState` cannot be constructed
/// without a real `DrmDevice`, a `LibSeatSession`, a `GbmDevice` and a
/// `GlesRenderer`, so anything left inside its `handle_session_event` is
/// unreachable by every test in this workspace. That is how D-018(2)'s fifth
/// ordering rule came to have production wiring a reviewer could revert with
/// the suite still green, and a pause handler is exactly the same shape of
/// risk — nothing goes red when it stops draining.
///
/// # What it does, in this order, and why the order is load-bearing
///
/// 1. **The dead-man hold is forgotten first.** A chord held when the VT
///    switches away produces no release here, so a hold left armed would either
///    fire with no gesture behind it or wedge in a state only a release can
///    leave. Forgetting it *before* the drain means a chord in progress is
///    already cancelled when the releases go out —
///    `crate::backend::winit::NestedState::handle_focus` states the identical
///    rule for a lost host focus.
/// 2. **Keys, then buttons**, to the realm the human's attention is bound to,
///    through the funnel an ordinary release takes. Keys first is
///    [`InputRouter::bind_to`]'s order: a keyboard latch is the state that
///    misbehaves worst.
/// 3. **Every chord matcher forgets its physical state.** The drain above pays
///    the *app* its held presses, but those deliveries are `SeatDelivery`s
///    handed straight to the funnel — they are not `SeatInput`s and they never
///    re-enter the hook stack, so each matcher keeps whatever it believed at
///    the instant the devices went away. That is a live defect rather than a
///    hypothetical, and the VT escape makes it fire on the very first use: a
///    human leaving this VT is holding **Ctrl and Alt by construction**, so
///    without this their next bare F5 switches VT, their next bare Insert
///    fires a clipboard gesture, and their next bare Delete raises the lock.
///    See [`crate::chord::ChordMatcher::forget_physical_state`].
///
/// # Buttons too, unlike nested focus loss, and that asymmetry is the decision
///
/// `handle_focus` drains keys and deliberately **not** buttons, because winit
/// reports pointer state separately and the host keeps sending motion, so a
/// synthetic button release would end a drag the human is still making. On a
/// seat pause there is no such drag to end: `libinput_suspend` has closed the
/// devices, no further motion or release can arrive for the whole pause, and
/// the human's next press after the switch back is a new gesture. A held button
/// left behind therefore wedges the app's implicit pointer grab with nothing
/// that can ever pay it down — the exact case
/// [`InputRouter::release_physical_buttons`] was added for (issue #212,
/// decision 3), and until this call site it had exactly one caller.
///
/// **And the gesture in flight** ([`InputRouter::end_physical_gesture`],
/// WS-E.4.2, issue #222), which follows the buttons for the same reason and
/// with the same asymmetry against nested focus loss. It lands harder here
/// than either: `libinput_suspend` has closed the devices, so no update and no
/// end can arrive for the whole pause, and a `gesture_begin` with no
/// `gesture_end` leaves the app accumulating a pinch that nothing can finish.
/// It is ended `cancelled`, because the human did not let go — the same
/// published trade the keys and buttons make.
///
/// # Agents are untouched
///
/// Only the human's presses are drained, by the same rule
/// [`InputRouter::release_physical_keys`] states: the seat leaving is not part
/// of an agent's actuation path, and a release synthesised for an agent's key
/// would reach the shim and the flight recorder tagged as the human's.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the caller is the bare-metal backend's PauseSession arm; a default build \
                  compiles this and runs its behavioural test without a seat"
    )
)]
pub(crate) fn suspend_physical_seat<H: crate::input::PreemptionHook>(
    runtime: &mut Runtime<H>,
    scenes: &crate::scene::realms::RealmScenes,
    switch: &std::cell::RefCell<crate::deadman::DeadManSwitch>,
) -> usize {
    // Before the drain, deliberately (doc comment above).
    switch.borrow_mut().forget_hold();
    // Step 3 (doc comment above): the two matchers the *kernel* holds. The
    // lock's and the VT escape's are forgotten by the backend's pause arm,
    // which is where those two live; these are here because this is the one
    // function that already has the kernel in hand, and because a default
    // build has a behavioural test for it and no seat.
    runtime
        .kernel
        .clipboard
        .borrow_mut()
        .forget_physical_state();
    runtime
        .kernel
        .screenshot
        .borrow_mut()
        .forget_physical_state();

    let Runtime {
        router,
        realms,
        kernel,
        ..
    } = runtime;
    let Some(bound) = router.bound_realm().cloned() else {
        return 0;
    };
    let mut owed = router.release_physical_keys(&bound);
    owed.extend(router.release_physical_buttons(&bound));
    // And the gesture in flight, `cancelled` (WS-E.4.2, issue #222). Same
    // argument as the buttons one paragraph up, and it lands harder: the
    // devices are closed for the whole pause, so no update and no end can
    // ever arrive to finish a pinch the app is still accumulating.
    owed.extend(router.end_physical_gesture(&bound));
    // **And the pointer constraint** (WS-E.4.2, issue #222), which is the same
    // latch shape one step further out: `libinput_suspend` has closed the
    // devices, so for the whole pause no motion can carry the pointer out of
    // the region and no withdrawal can arrive from an app that is not being
    // told anything. Left recorded, it would be an app believing it holds a
    // lock over a seat that is gone. Withdrawn rather than merely deactivated,
    // and the shim is TOLD (`withdrawn`) rather than left to guess.
    //
    // It is also transient over the same interval — `output_gates().active` is
    // false while the seat is away, so the sprite is back on the first frame
    // after the pause regardless — but the record removal is what stops the
    // constraint silently reactivating when the human comes back to a session
    // whose app was never told anything happened.
    let withdrawn = router.constraints().borrow_mut().withdraw(&bound);
    send_constraint_verdicts(realms, withdrawn.into_iter().collect());
    let count = owed.len();
    for delivery in owed {
        tracing::debug!(
            realm = %bound,
            "releasing a press held across a session switch so it cannot latch in the app"
        );
        deliver_physical(realms, scenes, &mut kernel.recorder, delivery);
    }
    count
}

/// **The seat has given the devices back**: restart the consent guard, because
/// a card that was up across the pause has only now become visible again
/// (WS-E.3.3, D-030(5)).
///
/// Returns whether there was a prompt to restart. The production caller
/// discards it — the logging this function owes is done here, via `debug!`,
/// precisely so a backend cannot forget it. The return value exists for the
/// test, which has no other way to observe that the guard was restarted.
///
/// [`ConsentGrab::restart_guard`]'s docs enumerate one way "raised" and
/// "visible" come apart — WS-E.2.2's opaque lock cover. A seat pause is the
/// second, and it is worse in two ways. It is produced entirely outside this
/// core, so nothing in the loop can decline it; and unlike the lock, which
/// keeps a covered prompt *unanswerable* because `LockGate` is outermost, a
/// paused session's grab is untouched — the press a human armed on Allow before
/// switching away survives the switch, because `commit` re-checks only the last
/// **physical** pointer position and a pause does not reset one. So the first
/// release after the switch back would commit a decision armed on a card that
/// spent its guard on somebody else's VT. `restart_guard` closes both halves:
/// it moves `raised_at` and it clears `armed`.
///
/// **Only the guard restarts.** The petition's own deadline is deliberately
/// untouched, exactly as the unlock path leaves it: it bounds how long the human
/// has to decide, and a VT switch does not buy them more of it. Refreshing it
/// would also let an agent extend its own petition's life by inducing VT churn,
/// and would break the invariant `a_grab_never_outlives_its_petitions_deadline`
/// pins.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the caller is the bare-metal backend's ActivateSession arm; a default build \
                  compiles this and runs its behavioural test without a seat"
    )
)]
pub(crate) fn resume_physical_seat(grab: &std::cell::RefCell<ConsentGrab>, now: Instant) -> bool {
    // Delegated rather than open-coded (WS-E.4.3): "the human can see this card
    // again as of now" is one fact with one implementation, and a seat return is
    // one of the three ways it becomes true. See [`screen_became_visible`].
    screen_became_visible(grab, now)
}

/// Turn the human's queued screenshot chords into files on disk (WS-E.2.4,
/// issue #216).
///
/// **The hook could not do any of this and must never be able to**, exactly as
/// [`drain_clipboard_gestures`] explains: the gate runs inside
/// `InputRouter::route_physical`, which is never told a realm, so it records
/// *that a gesture happened* and this function — which owns the realm registry,
/// the view cache and the audited directory — decides *what* is written.
///
/// **The pixels come from [`Runtime::view_cache`]**, the same map the
/// enforcement chokepoint's `realm_view` is built from, for the realm
/// [`physical_seat_target`] names — the realm the human's own keystrokes go to,
/// which is the one they are looking at. Two consequences worth stating:
///
/// - It is the **realm view**, so no trusted band, no consent card, no lock
///   cover, no status strip and no agent cursor is in it. That is a security
///   decision with a real usability cost, argued in full in
///   [`crate::screenshot`] and published in `docs/book/src/limits.md`.
/// - It is the **latest completed** composite, never a fresh render — capture's
///   own contract (`crate::capture`), inherited for free by reading the same
///   cache rather than composing again.
///
/// **No grant is consulted, because there is nothing here to consult one
/// with.** There is no principal, no facet and no verb in this function, and
/// `enforcement.rs`'s source scan asserts that this file and
/// `crate::screenshot` never name the chokepoint's identifiers.
fn drain_screenshot_gestures<H: PreemptionHook>(
    runtime: &mut Runtime<H>,
    scenes: &crate::scene::realms::RealmScenes,
    view: (u32, u32),
) {
    let gestures = runtime.kernel.screenshot.borrow_mut().take_pending();
    if gestures.is_empty() {
        return;
    }
    let target = physical_seat_target(&runtime.realms, scenes).map(|(id, _)| id.clone());
    // This turn's output size, the same one the router mapped coordinates
    // against. A cache entry refreshed at a *different* size -- possible for
    // exactly one round across a resize -- fails the encoder's own length
    // check and is journalled `encode_failed` rather than written at the wrong
    // geometry.
    let (width, height) = view;
    for gesture in gestures {
        let kernel = &mut runtime.kernel;
        // Journalled rather than silent in every refusing branch: a human
        // whose screenshot did not appear has no other instrument, and a
        // dropped screenshot on a full disk is exactly the failure someone
        // discovers a week later.
        let fail = |kernel: &mut Kernel, realm: Option<&RealmId>, reason: &'static str| {
            tracing::warn!(reason, "screenshot not written");
            kernel
                .recorder
                .record(crate::recorder::Event::ScreenshotFailed { realm, reason });
        };
        let Some(dir) = kernel.screenshot_dir.as_mut() else {
            // The key fired and the session has nowhere to put a file. Not an
            // error the operator can act on mid-session, but the alternative
            // is a key that silently does nothing, which is worse.
            fail(kernel, target.as_ref(), "no_screenshot_dir");
            continue;
        };
        let Some(realm_id) = target.as_ref() else {
            fail(kernel, None, "no_bound_realm");
            continue;
        };
        let Some(rgba) = runtime.view_cache.get(realm_id) else {
            // The realm has no live view: it has never composited, or its shim
            // is gone. The same fact the chokepoint turns into `no_surface`.
            fail(kernel, Some(realm_id), "no_view");
            continue;
        };
        match crate::screenshot::capture_to_file(dir, gesture, rgba, width, height) {
            Ok((file, digest)) => {
                tracing::info!(realm = %realm_id, file, "screenshot written");
                kernel
                    .recorder
                    .record(crate::recorder::Event::ScreenshotWritten {
                        realm: realm_id,
                        width,
                        height,
                        digest: &digest,
                        file: &file,
                    });
            }
            Err(reason) => fail(kernel, Some(realm_id), reason),
        }
    }
}

/// Turn the human's queued clipboard chords into wire traffic (WS-E.2.1,
/// issue #213).
///
/// **The hook could not do any of this and must never be able to.** It runs
/// inside `InputRouter::route_physical`, which is never told a realm — the
/// property that makes the consent grab and the dead-man switch session-wide by
/// construction ([`crate::input::PreemptionHook`]'s trait docs). So the gate
/// records *that a gesture happened* and this function, which owns the realm
/// registry and the slot, decides *where*.
///
/// **Both gestures aim at the realm the output is bound to**, resolved through
/// [`physical_seat_target`] — the same function that decides where the human's
/// keystrokes go. That is not a convenience: a clipboard gesture that promoted
/// from a realm the human was not looking at would be a channel out of a hidden
/// realm, which is the one thing a human-driven mediator must not be.
///
/// **One gesture transfers nothing.** `Promote` writes the slot and sends
/// nothing to any other realm; `Offer` reads it ([`ClipboardSlot::peek`] takes
/// `&self`) and writes nothing. The chords cannot both fire from one press —
/// [`crate::chord`] compares modifier sets for equality — so the property holds
/// at the matcher, here, and in the types, independently.
fn drain_clipboard_gestures<H: PreemptionHook>(
    runtime: &mut Runtime<H>,
    scenes: &crate::scene::realms::RealmScenes,
    now: Instant,
) {
    let gestures = runtime.kernel.clipboard.borrow_mut().take_pending();
    if gestures.is_empty() {
        return;
    }
    // Expire before anything reads the slot, so a gesture arriving in the same
    // turn as the deadline cannot read a stale slot. This is a SECOND line of
    // defence, not the clear: `sweep` owns that and runs without input, which
    // is what makes "cleared after two minutes" true for a human who copies
    // and walks away.
    if runtime.kernel.clipboard_slot.expire(now) {
        tracing::debug!("clipboard slot expired at gesture time");
    }
    let target = physical_seat_target(&runtime.realms, scenes).map(|(id, _)| id.clone());
    for gesture in gestures {
        let Runtime { realms, kernel, .. } = runtime;
        let Some(realm_id) = target.as_ref() else {
            // No realm is bound, so there is nothing to ask and nothing to
            // offer. Journalled rather than silent: the human has no other
            // instrument for "why did my copy do nothing".
            kernel
                .recorder
                .record(crate::recorder::Event::ClipboardRefused {
                    realm: None,
                    gesture: gesture.label(),
                    reason: "no_bound_realm",
                });
            continue;
        };
        let refuse = |kernel: &mut Kernel, reason: &'static str| {
            kernel
                .recorder
                .record(crate::recorder::Event::ClipboardRefused {
                    realm: Some(realm_id),
                    gesture: gesture.label(),
                    reason,
                });
        };
        let Some(realm) = realms.get(realm_id) else {
            refuse(kernel, "no_realm");
            continue;
        };
        let Some(server) = realm.server.as_ref() else {
            refuse(kernel, "no_shim");
            continue;
        };
        let mut send = |frame: &[u8]| realm.outbox.send(frame);
        match gesture {
            crate::clipboard::ClipboardGesture::Promote => {
                // The serial is minted *before* the send and superseding is its
                // whole job: a second press while the first answer is in flight
                // makes that answer stale by the human's own account.
                let serial = kernel.clipboard_slot.open_promotion(realm_id.clone());
                if let Err(err) = server.send_request_selection(serial, &mut send) {
                    tracing::warn!(realm = %realm_id, %err, "request_selection could not be sent");
                    kernel.clipboard_slot.abandon_promotion();
                    refuse(kernel, "send_failed");
                }
            }
            crate::clipboard::ClipboardGesture::Offer => {
                // Read, encode and send inside one borrow of the slot. That
                // keeps the slot's OWN copy singular; it does **not** mean no
                // other copy exists, and an earlier version of this comment
                // claimed it did. `send_offer_selection` does `data.to_owned()`,
                // `OfferSelection::encode` copies again into a `Vec<u8>`, and
                // `Outbox::send` does `frame.to_vec()` a third time -- none of
                // them zeroised on free, while the shim half `memset`s every
                // buffer that held a selection. Closing that asymmetry means
                // zeroising on the core's send path too, which is real work and
                // is not done here; it is published rather than implied away.
                // The journal fields are copied out first because they are the
                // only things that outlive the borrow.
                // `Err(reason)` rather than `None`, so the journal can tell an
                // EMPTY slot from a FULL one whose send failed. Collapsing both
                // to `empty_slot` made the flight recorder assert the slot was
                // empty when it was full -- and the `Promote` branch above
                // already had the right label (`send_failed`), so the
                // vocabulary existed and simply was not reached.
                let sent = match kernel.clipboard_slot.peek(now) {
                    None => Err("empty_slot"),
                    Some(offered) => {
                        let record = (offered.bytes, offered.digest, offered.source.clone());
                        match server.send_offer_selection(offered.mime, offered.data, &mut send) {
                            Ok(()) => Ok(record),
                            Err(err) => {
                                tracing::warn!(
                                    realm = %realm_id,
                                    %err,
                                    "offer_selection could not be sent"
                                );
                                Err("send_failed")
                            }
                        }
                    }
                };
                match sent {
                    Ok((bytes, digest, source)) => {
                        kernel
                            .recorder
                            .record(crate::recorder::Event::ClipboardOffered {
                                realm: realm_id,
                                source: &source,
                                mime: crate::clipboard::CLIPBOARD_MIME,
                                bytes,
                                digest: &digest,
                            });
                        tracing::info!(
                            realm = %realm_id,
                            bytes,
                            "clipboard offered to the realm the human is looking at"
                        );
                    }
                    Err(reason) => refuse(kernel, reason),
                }
            }
        }
    }
}

/// Fold a shim's `selection` answer into the slot (WS-E.2.1, issue #213).
///
/// **This is where "the core pulls" stops being a rule and becomes a type.**
/// The answer can only reach [`ClipboardSlot::fill`] through
/// [`ClipboardSlot::claim_answer`], which matches the serial *and* the realm of
/// a promotion this core itself opened on a human's gesture, and hands back a
/// [`PendingPromotion`](crate::clipboard::PendingPromotion) that `fill` consumes
/// by value. An unsolicited `selection` — from a compromised shim, or from a
/// well-meaning one racing a superseded gesture — finds no ticket and does
/// nothing at all.
///
/// The core re-judges the MIME type and the length whatever the shim's `status`
/// claimed, because a shim is untrusted and `ok` is a claim rather than a
/// credential.
fn apply_selection_answer<H: RuntimeHost>(host: &mut H, realm_id: &RealmId, now: Instant) {
    let Runtime { realms, kernel, .. } = host.runtime();
    let Some(answer) = realms
        .get_mut(realm_id)
        .and_then(|realm| realm.server.as_mut())
        .and_then(|server| server.take_selection_answer())
    else {
        return;
    };
    let Some(ticket) = kernel.clipboard_slot.claim_answer(realm_id, answer.serial) else {
        tracing::debug!(
            realm = %realm_id,
            "selection answer discarded: no promotion of this core's is waiting for it"
        );
        return;
    };
    use vitrin_protocol::generated::vitrin_shim_session::SelectionStatus;
    // The shim's own refusing statuses, taken at face value only in the
    // direction that refuses: they can never *cause* a fill.
    let refused = match answer.status {
        SelectionStatus::Ok => None,
        SelectionStatus::Empty => Some("empty"),
        SelectionStatus::WrongType => Some("wrong_type"),
        SelectionStatus::TooLarge => Some("too_large"),
    };
    let outcome = match refused {
        Some(reason) => Err(reason),
        None => kernel
            .clipboard_slot
            .fill(ticket, &answer.mime, &answer.data, now)
            .map_err(|refusal| refusal.label()),
    };
    match outcome {
        Ok(promoted) => {
            kernel
                .recorder
                .record(crate::recorder::Event::ClipboardPromoted {
                    realm: realm_id,
                    mime: promoted.mime,
                    bytes: promoted.bytes,
                    digest: &promoted.digest,
                });
            tracing::info!(
                realm = %realm_id,
                bytes = promoted.bytes,
                "clipboard promoted from the realm the human is looking at"
            );
        }
        Err(reason) => {
            kernel
                .recorder
                .record(crate::recorder::Event::ClipboardRefused {
                    realm: Some(realm_id),
                    gesture: crate::clipboard::ClipboardGesture::Promote.label(),
                    reason,
                });
            tracing::debug!(realm = %realm_id, reason, "clipboard promotion refused");
        }
    }
}

/// **Deliver every owed `pointer_constraint_state`** (WS-E.4.2, issue #222).
///
/// The one place a constraint verdict reaches a wire, so that "the core sends
/// at most one of these per transition" is a property of one function rather
/// than of six call sites. A realm with no shim (dead, or never started) is
/// skipped in silence: there is nobody to tell, which is the same fact
/// `InputRouter::reset_for`'s silent `forget` records.
///
/// **A send failure is never fatal here.** The shim has stopped reading; the
/// transport's own slow-reader policy kills the connection on the next dispatch
/// through the one funnel that classifies deaths, and taking the session down
/// over an app that will not listen to being told its lock ended is the wrong
/// trade. The record is already gone or already updated either way — the wire
/// is the *report*, not the state.
fn send_constraint_verdicts(
    realms: &BTreeMap<RealmId, RealmRuntime>,
    verdicts: Vec<crate::input::ConstraintVerdict>,
) {
    for verdict in verdicts {
        let Some(realm) = realms.get(&verdict.realm) else {
            continue;
        };
        let Some(server) = realm.server.as_ref() else {
            continue;
        };
        let mut send = |frame: &[u8]| realm.outbox.send(frame);
        match server.send_pointer_constraint_state(verdict.serial, verdict.state, &mut send) {
            Ok(()) => tracing::debug!(
                realm = %verdict.realm,
                serial = verdict.serial,
                state = ?verdict.state,
                "pointer constraint state sent"
            ),
            Err(err) => tracing::warn!(
                realm = %verdict.realm,
                %err,
                "pointer_constraint_state could not be sent"
            ),
        }
    }
}

/// Fold a shim's parked `pointer_constraint` ask into the table (WS-E.4.2,
/// issue #222), and send whatever that ask settles by itself.
///
/// The sibling of [`apply_selection_answer`] and drained on the same turn, from
/// the same place in [`dispatch_shim`], for the same reason: an ask parked by a
/// shim the core is about to bury must not reach the session's state.
///
/// What it does **not** do is decide whether the constraint is in force. That is
/// derived, and [`reconcile_pointer_constraints`] answers it on the same
/// dispatch round — so an ask and its `active` can arrive one after the other
/// without this function knowing anything about overlays or focus.
fn apply_pointer_constraint_ask<H: RuntimeHost>(host: &mut H, realm_id: &RealmId) {
    let Runtime { realms, router, .. } = host.runtime();
    let Some(ask) = realms
        .get_mut(realm_id)
        .and_then(|realm| realm.server.as_mut())
        .and_then(|server| server.take_pointer_constraint_ask())
    else {
        return;
    };
    let owed = router.constraints().borrow_mut().ask(realm_id, ask);
    send_constraint_verdicts(realms, owed);
}

/// **Recompute every pointer constraint from live state and tell the shims what
/// changed** (WS-E.4.2, issue #222).
///
/// Level-triggered, edge-reported, and called once per dispatch round from
/// [`post_dispatch`] **before the dirty gate** — the same position and the same
/// argument as `set_agent_cursor`: a constraint going inactive because the
/// human switched realms is a change nothing else in the round would announce,
/// and an app owes no commit for it.
///
/// **This is the app-facing half of the derived design.** The human-facing half
/// — whether the core draws its own cursor sprite — is
/// `crate::input::hides_human_sprite`, evaluated separately inside the
/// bare-metal composite from the same gates. Neither is *called* to announce a
/// deactivation, so neither can be stranded by a call site that forgot: the
/// consent card, the lock cover, the core notice, the dead-man hold, the realm
/// switch, the unmapped surface and the paused output all reach both through
/// [`Presenter::output_gates`] and the scene, with no code of their own.
fn reconcile_pointer_constraints<H: RuntimeHost>(host: &mut H) {
    let (runtime, view) = host.split();
    let focused = view.focused().cloned();
    let gates = crate::input::PresentationGates {
        focused: focused.as_ref(),
        output: view.output_gates(),
        surface: focused
            .as_ref()
            .and_then(|realm| view.scene(realm))
            .and_then(crate::scene::Scene::surface_size),
        view: view.view_size(),
        // The HUMAN's pointer, never the shared one: an agent's actuation must
        // not be able to activate a lock, because activating one hides the
        // human's own cursor (`RealmSeat::human_pointer`).
        pointer: focused
            .as_ref()
            .and_then(|realm| runtime.router.human_pointer(realm)),
    };
    let owed = runtime.router.constraints().borrow_mut().reconcile(&gates);
    // **The frame, not just the record.** `set_agent_cursor` twenty lines below
    // takes exactly this shape for exactly this reason, and the sprite needs it
    // more: its visibility is DERIVED per composed frame, so the predicate can
    // never answer stale — but on a damage-driven backend nothing else in this
    // round asks for a frame after a constraint ends, and the confined app owes
    // no commit for having been unlocked. Without this the panel keeps the last
    // frame composed while the constraint was active: the one with no cursor in
    // it. Found by driving each deactivation path and watching the present
    // counter stay at zero, after the derived design had already been reviewed
    // and called sound.
    if runtime
        .router
        .constraints()
        .borrow_mut()
        .take_repaint_owed()
    {
        runtime.dirty = true;
        view.request_present();
    }
    if owed.is_empty() {
        return;
    }
    send_constraint_verdicts(&runtime.realms, owed);
}

/// Turn a gated attention press into an open window (WS-E.1.7, issue #232):
/// resolve who holds layout authority, tell exactly them, and open the window
/// naming exactly that set.
///
/// **The hook cannot do any of this**, which is why the work is here. The
/// preemption hook runs inside `InputRouter::route_physical`, and the router
/// holds no authority state and must never grow any ([`crate::input`]'s module
/// docs) — so the gate records a pending press and the embedder, which owns the
/// grant table and the connections, resolves it. Exactly the division
/// [`crate::deadman::Trigger`] makes, for exactly the same reason.
///
/// **Delivery is filtered, and the filter is not an authority check.**
/// [`GrantTable::holds_verb`] answers "should this connection be *told*". An
/// unconditional event would be a free, silent keystroke-timing oracle for
/// every connected client — the same hazard consuming the key closes on the
/// app side. Whether anything may then *happen* stays the chokepoint's, at step
/// 5c, against the same delivered-to set.
///
/// **A press nobody could use opens nothing.** With no layout holder the set is
/// empty, no principal could ever claim the window, and calling that "open"
/// would overstate what the human's gesture did. The journal entry is written
/// either way, so a human asking why their switch did nothing can see that the
/// key fired and that nobody was listening.
fn open_attention_window<H: PreemptionHook>(runtime: &mut Runtime<H>, now: Instant) {
    let Some(pressed_at) = runtime.kernel.attention.borrow_mut().take_pending() else {
        return;
    };
    let chord = runtime.kernel.attention.borrow().chord().name();
    let Runtime { kernel, conns, .. } = runtime;
    let mut delivered: std::collections::BTreeSet<crate::identity::PrincipalIdentity> =
        std::collections::BTreeSet::new();
    let mut notified = 0usize;
    for conn in conns.values_mut() {
        let Some(identity) = conn.server.bound_identity().cloned() else {
            continue;
        };
        if !crate::attention::EXEMPT_VERBS
            .iter()
            .any(|verb| kernel.grants.holds_verb(&identity, *verb, now))
        {
            continue;
        }
        let outbox = conn.outbox.clone();
        let mut send = |frame: &[u8], fd: Option<BorrowedFd<'_>>| {
            debug_assert!(
                fd.is_none(),
                "no version-1 event sent outside dispatch carries an fd"
            );
            outbox.send(frame)
        };
        match conn.server.deliver_attention(&mut send) {
            Ok(true) => {
                notified += 1;
                delivered.insert(identity);
            }
            // Not bound after all (a handshake that raced this turn): nothing
            // was sent, so nothing may claim on its behalf.
            Ok(false) => {}
            Err(err) => {
                // The connection is dying; its teardown entry follows. A lost
                // attention event costs one window the client can neither
                // observe nor be harmed by missing.
                tracing::warn!(%err, "attention event could not be delivered");
            }
        }
    }
    let opened = !delivered.is_empty();
    if opened {
        kernel.attention.borrow_mut().open(pressed_at, delivered);
    }
    kernel
        .recorder
        .record(crate::recorder::Event::AttentionPressed {
            chord,
            opened,
            notified,
        });
    tracing::debug!(
        chord,
        opened,
        notified,
        "attention key pressed: the human's hand is off the app they are in"
    );
}

/// Route chokepoint-admitted actuations through the session's router toward
/// **each one's own granted realm's** shim seat.
///
/// The router is the same one the backend's physical input flows through, so
/// within a realm the implicit grab and the pointer state are shared between
/// an agent's actuations and a human's — which is what makes the preemption
/// hook meaningful. The origin tag rides the wire on every event and is never
/// constructed or rewritten here (backward requirement B2): this path only
/// addresses.
///
/// Every event that actually reaches the shim's seat is recorded, tagged with
/// that origin ([`crate::recorder::Event::SeatDelivered`], issue #83): the
/// unforgeable physical-vs-agent distinction the type system enforces is only
/// investigable after an incident if the flight recorder wrote it down.
/// Shape only — the kind and the tag, never coordinates, keysym, or typed
/// bytes — so the audit entry can never become a keylogger.
///
/// # Which realm, with more than one attached (WS-E.1.6, issue #212)
///
/// **The realm named on the pair**, which the chokepoint took from the grant
/// row the use was admitted under ([`enforcement::UseEnv::grant_realm`], fed
/// from [`dispatch_principal`]'s actuation sink). Not [`seat_target`], which
/// is now the *physical* path's question alone: an agent must be able to work
/// in a realm the human is not looking at, and that is the whole
/// concurrent-operation claim.
///
/// Until this landed the session had one delivery target, so an actuation
/// whose grant named any other realm was refused `internal` at the
/// chokepoint's step 5d rather than delivered into a sibling's app. That
/// stopgap is **gone**: the realm travels with the event, so there is nothing
/// left to compare and nothing to refuse.
///
/// Nothing here re-checks the authority that named the realm, and nothing here
/// should — a delivery site that made its own authority judgement would be the
/// second enforcement site this crate does not have. What it does re-check is
/// **liveness**, which is not authority: a realm whose shim died between
/// admission and delivery drops the event, exactly as it always did.
///
/// [`enforcement::UseEnv::grant_realm`]: crate::enforcement::UseEnv::grant_realm
fn route_seat<H: RuntimeHost>(host: &mut H, seat: Vec<(RealmId, SeatInput)>) {
    if seat.is_empty() {
        return;
    }
    let (runtime, view) = host.split();
    let view_size = view.view_size();
    let Runtime {
        router,
        realms,
        kernel,
        ..
    } = runtime;
    // A shim that has stopped reading gets nothing more this round -- a
    // property of a *batch*, and now a property per realm rather than per
    // round: one wedged realm must not silence a sibling's actuations.
    let mut wedged: Vec<RealmId> = Vec::new();
    for (target, input) in seat {
        if wedged.contains(&target) {
            continue;
        }
        // Liveness, never authority. A realm's shim can die between admission
        // and delivery, which is a runtime condition; the authority question
        // was settled at the chokepoint and is not re-asked here.
        let Some((realm_id, realm)) = realms.get_key_value(&target) else {
            continue;
        };
        let Some(server) = realm.server.as_ref() else {
            continue;
        };
        // **The granted realm's own surface geometry** (WS-E.1.3). The router
        // maps view coordinates to surface coordinates through
        // `layout::place`, so it must be handed the geometry of the surface
        // the event is about -- which is this grant's realm, whether or not
        // that realm is the one on screen.
        let surface = view.scene(realm_id).and_then(|scene| scene.surface_size());
        let Some(delivery) = router.route_emulated(realm_id, input, view_size, surface) else {
            continue;
        };
        let mut send = |frame: &[u8]| realm.outbox.send(frame);
        match server.deliver_seat_event(&delivery, &mut send) {
            // Recorded only when it went out (a seat the shim has not minted
            // yet drops the event — nothing was delivered, so nothing is
            // audited as delivered). One funnel with the physical path.
            Ok(sent) => {
                if sent {
                    crate::input::record_seat_delivery(&mut kernel.recorder, realm_id, &delivery);
                }
            }
            Err(err) => {
                // The shim has stopped reading. Stop producing for it; the
                // transport's own slow-reader policy kills the connection on
                // the next dispatch, through the one funnel that classifies
                // deaths.
                tracing::warn!(realm = %realm_id, %err, "seat delivery to the realm failed");
                wedged.push(target);
            }
        }
    }
}

/// Dispatch one event from **this realm's** shim connection.
///
/// `realm_id` is not derivable from anything else here: a
/// `ConnectionSource` hands the callback its event and the connection, and
/// nothing that identifies the peer. It is carried in the closure the
/// source was registered with ([`start_one_realm_in`]), which is the whole
/// of "carry a RealmId in the callback data" — and the seam that did not
/// exist while a session held exactly one shim.
fn dispatch_shim<H: RuntimeHost>(
    host: &mut H,
    realm_id: &RealmId,
    event: ConnectionEvent,
    conn: &mut calloop::generic::NoIoDrop<Connection>,
) {
    match event {
        ConnectionEvent::Message(msg) => {
            let (runtime, view) = host.split();
            let Runtime { realms, dirty, .. } = runtime;
            let Some(realm) = realms.get_mut(realm_id) else {
                return;
            };
            let Some(server) = realm.server.as_mut() else {
                return;
            };
            let mut send = |frame: &[u8]| vitrin_ipc::reply(conn, frame, None);
            // The nested backend hands back a real importer bound to its
            // live GLES renderer here (issue #117/P1.3.5); headless has no
            // GPU renderer at all and inherits the trait's `None` default.
            // Either way a `kind=dmabuf` commit with no import capability
            // resolves as the designed `import_failed` shm fallback.
            //
            // Scoped to this inner block, and dispatched before it ends:
            // `importer` owns a `Box<dyn DmabufImporter + '_>`, whose drop
            // glue is opaque to dropck (the concrete embedder type is
            // erased), so the borrow of `view` it carries must end at an
            // explicit point rather than at its last syntactic use — and
            // `view.request_present()`/`close_realm(host, ..)` below both
            // need `view`/`host` free again.
            // **This realm's** scene, named by the id the source's callback
            // carried (WS-E.1.3): a commit lands in the committing realm's
            // own scene, so the last committer no longer owns the only
            // surface in the session.
            let outcome = view.scene_and_importer(realm_id, |scene, importer| {
                server.handle_message(msg, scene, importer, &mut send)
            });
            match outcome {
                // THE anti-amplification line. Not a redraw, not a
                // `presented` — a flag. See the module docs.
                Ok(true) => {
                    *dirty = true;
                    view.request_present();
                }
                Ok(false) => {}
                Err(fault) => {
                    tracing::warn!(%fault, "shim connection fatal");
                    // The core is closing, not the transport: the source is
                    // still registered, and the `Hangup` the lifecycle holds
                    // is what retires it.
                    close_realm(host, realm_id, DeathCause::of_shim_fault(fault));
                    return;
                }
            }
            // Only on a live connection, and only after the message was
            // accepted: a `selection` answer parked by a shim the core is about
            // to bury must not fill the human's clipboard (WS-E.2.1).
            apply_selection_answer(host, realm_id, Instant::now());
            // ...and its sibling, on the same terms and for the same reason
            // (WS-E.4.2): a `pointer_constraint` ask from a shim that has just
            // violated the protocol must not reach the constraint table.
            apply_pointer_constraint_ask(host, realm_id);
        }
        ConnectionEvent::Disconnected => {
            tracing::info!(realm = %realm_id, "shim connection closed");
            close_realm(host, realm_id, DeathCause::ConnectionClosed);
        }
        ConnectionEvent::Fault(reason) => {
            tracing::warn!(realm = %realm_id, %reason, "shim connection terminated");
            // The transport's classification, not a second opinion of it.
            close_realm(host, realm_id, DeathCause::from(&reason));
        }
    }
}

/// The realm's shim connection ended — EOF, a transport fault, or a shim
/// protocol violation — routed into the realm-lifecycle funnel.
///
/// This is a *routing* function and nothing more, which is the point. Every
/// consequence of a realm dying lives in
/// [`RealmLifecycle::note_connection_closed`]: the surface leaves the scene
/// (making the chokepoint's `no_surface` refusal true), the retained
/// framebuffer is scrubbed so no readback can serve a dead realm's last
/// painted frame, the router's seat state is reset, the registry is marked
/// exited so petitions for this realm resolve `unavailable`, `realm_died` is
/// journalled, the shim is reaped if it is already a corpse and asked to
/// leave if it is not — all behind a death latch, so the five independent
/// observers of a realm's death bury it exactly once.
///
/// A shorter version of this function that cleared the scene and reset the
/// router itself would look equivalent and would silently drop four of
/// those. It would also be a *second* death path beside the latched one,
/// which is how a realm ends up dead in one place and alive in the one still
/// serving frames.
fn close_realm<H: RuntimeHost>(host: &mut H, realm_id: &RealmId, cause: DeathCause) {
    with_realm_teardown(host, realm_id, |life, teardown| {
        life.note_connection_closed(cause, teardown);
    });
    // Recomposite without the dead realm's surface. The scene is already
    // clear, but both backends composite on demand, so without a redraw the
    // dead realm's pixels would stay on the human-visible output until
    // something else happened to damage the scene.
    //
    // **Exactly what a latched commit does**, and for the same reason: the
    // dirty flag alone is only half of it. `post_dispatch` consumes the flag
    // by calling [`Presenter::redraw`], which on a backend whose frame clock
    // is external answers `Scheduled` and composites nothing — the nested
    // backend's `redraw` deliberately does not touch the window, because the
    // host compositor owns the clock. [`Presenter::request_present`] is the
    // half that actually asks the host for a frame, and it is a no-op on
    // headless, where a completed composite *is* the cadence. Setting the
    // flag without requesting the present left a dead app's last frame on a
    // nested human's screen until an unrelated resize, focus change or
    // petition happened along.
    let (runtime, view) = host.split();
    // The slot cannot outlive the realm that authored its bytes (WS-E.2.1,
    // D-024(5)): keeping them would mean the core holding application content
    // nothing left in the session can account for, and offering them onward
    // would be a channel out of a realm that no longer exists. An outstanding
    // promotion addressed here is abandoned on the same line.
    if runtime.kernel.clipboard_slot.forget_realm(realm_id) {
        tracing::info!(
            realm = %realm_id,
            "clipboard slot cleared: the realm its contents came from has died"
        );
    }
    runtime.dirty = true;
    view.request_present();
}

/// The once-per-dispatch-round presentation step.
///
/// Both backends pass this as `event_loop.run`'s post-dispatch callback, and
/// it is the only place in the runtime that composites. Everything about the
/// anti-amplification argument in the module docs lives here: a dispatch
/// round may have latched a hundred commits, and this runs one redraw, one
/// readback, and one [`ShimServer::presented`] — which still emits one
/// `frame_done` per commit, in commit order, so the shim's pacing sees the
/// true output cadence and only the *composites* are coalesced.
/// Discharge every realm's owed frame callbacks against a composite that has
/// **actually happened**.
///
/// Two callers, one per frame-clock posture, and the split is the whole
/// reason this is a free function rather than inline in [`post_dispatch`]:
///
/// - [`post_dispatch`], when [`Presenter::redraw`] answered
///   [`Presentation::Completed`] — the headless posture, where the composite
///   is synchronous.
/// - The nested backend, from its own `WinitEvent::Redraw` handler, once the
///   host compositor has actually drawn — the posture where `redraw` answers
///   [`Presentation::Scheduled`] and the real composite arrives later.
///
/// It emits one `frame_done` per owed commit, in commit order, so a dispatch
/// round that latched a hundred commits still pays one composite while every
/// commit gets the callback it is owed. Coalescing the composites is the
/// anti-amplification defense; coalescing the *callbacks* would break pacing,
/// which is why `ShimServer::presented` batches rather than collapses.
///
/// **Every attached realm is paid, not the first, and visibility is not a
/// condition** (WS-E.1.3 decision 2). One composite discharges whatever each
/// realm is owed, bound or hidden. A realm skipped here is a shim that paces
/// on `frame_done` and never hears one, which is a permanent stall of that
/// app with nothing in the log to say so — and, once captures are per realm,
/// it is worse than a stall: a Wayland client that stops repainting leaves
/// its capture a **stale frame**, which `vitrin_grant.refusal.no_surface`
/// forbids in as many words ("never a stale frame"). Refusing capture on a
/// hidden realm instead was the alternative and it is a lie, because the
/// realm *has* a surface.
///
/// So a hidden realm is paced by the same real clock the bound one is: one
/// permit per **completed output composite**, never per dispatch round, which
/// is exactly the rule [`Presentation`] exists to keep. The cost — up to
/// [`crate::realm::MAX_REALMS`] apps rendering at the output's rate whether or
/// not anyone is looking — is deliberate, untraded, and published in
/// `docs/book/src/limits.md`.
pub(crate) fn emit_presented<H: PreemptionHook>(runtime: &mut Runtime<H>) {
    let Runtime { realms, epoch, .. } = runtime;
    let time_ms = epoch.elapsed().as_millis() as u32;
    for (realm_id, realm) in realms.iter_mut() {
        let Some(server) = realm.server.as_mut() else {
            continue;
        };
        if !server.wants_presentation() {
            continue;
        }
        let outbox = &realm.outbox;
        let mut send = |frame: &[u8]| outbox.send(frame);
        if let Err(err) = server.presented(time_ms, &mut send) {
            // One realm's dead socket must not cost the others their
            // callbacks: the transport's own slow-reader policy kills that
            // connection on its next dispatch, through the one funnel that
            // classifies deaths.
            tracing::warn!(realm = %realm_id, %err, "frame_done delivery to the realm failed");
        }
    }
}

pub(crate) fn post_dispatch<H: RuntimeHost>(host: &mut H) {
    // First, before the dirty gate: raising or lowering a consent prompt is
    // exactly what makes the frame dirty, so it must run before the gate below
    // reads `dirty`. Backends that cannot host a prompt inherit the trait's
    // no-op and pay nothing here.
    host.service_consent(Instant::now());
    // ...and the lock, on the same terms and immediately after: raising or
    // lowering it is exactly what makes the frame dirty. Sampling `Instant::now()`
    // a second time rather than threading one turn instant through both is the
    // shape `service_consent` already set here; the two are microseconds apart
    // and neither compares against the other's sample.
    host.service_lock(Instant::now());
    // ...and the screen's own lifecycle (WS-E.4.3, issue #223), immediately
    // after the lock and on the same terms: raising or lowering the idle cover
    // is exactly what makes the frame dirty, so it must run before the gate
    // below reads `dirty`.
    //
    // **After the lock, deliberately, and it is the same reason the lock is
    // after consent**: the two share an activity clock and nothing else, and
    // the journal must read in the order the human experienced. A round in
    // which the session both locks and blanks locked first and then went dark
    // — never "the screen went dark and then, behind it, something locked".
    //
    // Both clocks are sampled here rather than inside, because the resume
    // detector's whole substance is their disagreement and a detector that read
    // them at two different points in the round would manufacture skew of its
    // own. Backends that own no display inherit the trait's no-op and pay two
    // `clock_gettime` calls, which is what `refresh_status` below already pays
    // unconditionally.
    host.service_screen(std::time::SystemTime::now(), Instant::now());
    // ...and the pointer constraints, immediately after both, because both are
    // gates it derives from: a card raised or a cover lowered one line above
    // changes what every recorded constraint is worth. Before the dirty gate
    // for `set_agent_cursor`'s reason — nothing else in this round would say a
    // lock ended, and the confined app owes no commit for it.
    reconcile_pointer_constraints(host);
    let fatal = {
        let (runtime, view) = host.split();
        // Also before the dirty gate, and for the same shape of reason: an
        // agent that moved its pointer changed the human-visible output, and
        // nothing else in this round will say so (the app owes no commit for
        // a hover). Drawing only — the shim's delivery already happened
        // inside the round, through `route_seat`, and is unaffected.
        //
        // **Resolved from the bound realm** (WS-E.1.3, sharpened by
        // WS-E.1.6): the sprite is painted in the output's coordinates over
        // the output's realm, so the position drawn must be *that realm's*
        // agent pointer. An agent pointing inside a hidden realm has no
        // position that means anything in the picture the human is looking at,
        // and drawing one anyway would put a crosshair over an unrelated app.
        // Since the router keeps one agent pointer per realm, this is a lookup
        // by the realm the output shows rather than a filter on a single
        // session-wide position — a hidden realm's agent motion can no longer
        // even overwrite the position the visible realm would draw. The
        // consequence, that a hidden realm's agent draws nothing at all, is a
        // real weakening of D-019 and is published as a limit, not smoothed
        // over.
        let cursor = view
            .focused()
            .cloned()
            .and_then(|focused| runtime.router.agent_pointer(&focused));
        if view.set_agent_cursor(cursor) {
            runtime.dirty = true;
            view.request_present();
        }
        // ...and the attention marker, on the same terms and before the same
        // gate: the human's window opening or closing changes the
        // human-visible output and nothing else in this round says so.
        if view.set_attention(runtime.kernel.attention.borrow().is_open(Instant::now())) {
            runtime.dirty = true;
            view.request_present();
        }
        // ...and the status strip (WS-E.2.3), on the same terms and before the
        // same gate. It answers `false` — and touches neither clock nor
        // filesystem — unless `--status` is on, so a session without a strip
        // pays no SAMPLING here: no sysfs read, no snapshot, no raster.
        //
        // It does not pay *nothing*, and an earlier version of this comment
        // said it did. Both clocks below are arguments, so they are evaluated
        // at this call site on every dispatch round whether or not a strip
        // exists — two `clock_gettime` calls, which is why the claim is
        // narrowed here rather than left to read as "a strip-less session
        // never asks the time".
        //
        // The wall clock is read once, at this one site, so the strip's
        // contents cannot depend on where in the round they were sampled.
        if view.refresh_status(std::time::SystemTime::now(), Instant::now()) {
            runtime.dirty = true;
            view.request_present();
        }
        if !runtime.dirty {
            return;
        }
        runtime.dirty = false;
        match view.redraw() {
            Ok(presentation) => {
                // Refreshed here and nowhere else: capture reads this cache,
                // so a refresh on the request path would make an agent's
                // capture trigger a composite and make goldens depend on
                // request timing.
                //
                // **Every live realm, not the bound one** (decision 1). A
                // hidden realm's `observe` grant must serve that realm's own
                // pixels, so its view is composed here too — the same site,
                // the same completed-composite occasion, the same purity
                // rule. The bound realm's entry is the output's own
                // composition (headless reads its retained framebuffer back);
                // the rest are `Scene::compose` of their own scenes, which is
                // byte-identical machinery.
                //
                // Also a **prune**: a realm whose shim session is gone
                // (`server.is_none()`, the fact the death funnel sets) has
                // its entry dropped rather than left behind. `realm_is_live`
                // already refuses a dead realm's capture, so this is defence
                // in depth of exactly the kind
                // `RetainedOutput::scrub_retained_frame` is for the headless
                // framebuffer — the bytes behind the predicate are gone, not
                // merely unreachable.
                refresh_view_cache(runtime, view);
                // Only a composite that actually happened discharges the
                // realm's owed `frame_done`. On a backend whose clock is
                // external this is `Scheduled`, and the frame callbacks stay
                // owed until that backend calls `emit_presented` itself.
                if presentation == Presentation::Completed {
                    emit_presented(runtime);
                }
                None
            }
            // A composite that fails is not a peer's doing and leaves the
            // session unable to present anything: it is the one condition
            // this module stops the loop for.
            Err(err) => Some(err),
        }
    };
    if let Some(err) = fatal {
        host.stop(Some(err));
    }
}

/// Recompose **every live realm's** view into [`Runtime::view_cache`], and
/// mirror each one to its `--capture-dump` file.
///
/// Split out of [`post_dispatch`] because it is now a loop with a prune
/// rather than one assignment, and because "which realms does the cache hold"
/// is a confidentiality statement worth reading in one place:
///
/// - **Fill** — one entry per realm with a live shim session, composed at the
///   output's view size. A realm whose view is degenerate (a minimized nested
///   window, a readback failure, a realm with no scene) gets no entry, which
///   the chokepoint turns into `no_surface`.
/// - **Prune** — every other entry is removed, so a dead realm's last frame
///   does not sit in the map behind a predicate. `realm_is_live` already
///   refuses it; this makes the bytes gone as well as unreachable.
///
/// # "Live" is `server.is_some()`, not "has a map entry"
///
/// **No production path ever removes a key from [`Runtime::realms`]** — the
/// entry outlives the process by design, so a `waitpid` and a shutdown ladder
/// can still find it, and [`close_realm`] leaves it in place with
/// `server: None`. Deriving the live set from `realms.keys()` therefore made
/// the prune a no-op that could never fire, with the defence-in-depth claim
/// above stated anyway and a test that only passed because it removed the key
/// by hand — a state the runtime does not produce.
///
/// `server.is_none()` is what actually marks a dead realm: [`RealmLifecycle`]'s
/// death funnel takes the [`ShimServer`] out of the entry
/// (`teardown.shim.take()`) and nothing else does. It is the same fact
/// [`seat_target`] filters on and the same one [`rebind_output_after_death`]
/// tests, so the three cannot disagree about which realms are gone.
///
/// Cost: one `Scene::compose` per live realm per **dirty** round — not per
/// output frame, and nothing at all on an idle session. That is decision 2's
/// bill and it is published as a limit.
fn refresh_view_cache<K: PreemptionHook, P: Presenter>(runtime: &mut Runtime<K>, view: &mut P) {
    let live: Vec<RealmId> = runtime
        .realms
        .iter()
        .filter(|(_, realm)| realm.server.is_some())
        .map(|(realm_id, _)| realm_id.clone())
        .collect();
    runtime.view_cache.retain(|realm, _| live.contains(realm));
    for realm_id in live {
        match view.view_rgba(&realm_id) {
            Some(rgba) => {
                if let Some(base) = runtime.capture_dump.as_deref() {
                    // The `--capture-dump` diagnostic (P1.8.5): mirror the
                    // same freshly refreshed readback to a file. Written
                    // here, off the redraw path and NOT on the capture
                    // request, so the dumped frame and the frame an
                    // `observe()` later serves come from one and the same
                    // cache entry — the two diverge only in the transport
                    // between them (`render_frame`, the memfd, the wire, the
                    // SDK decode), which is exactly the "adds no distortion"
                    // claim the SSIM comparison tests. A write failure is
                    // logged, never fatal: a broken diagnostic must not take
                    // a session down.
                    write_capture_dump(&capture_dump_path(base, &realm_id), &rgba);
                }
                runtime.view_cache.insert(realm_id, rgba);
            }
            None => {
                runtime.view_cache.remove(&realm_id);
            }
        }
    }
}

/// Where a realm's `--capture-dump` frame is written: the operator's `PATH`
/// with `.<realm-id>` appended.
///
/// **Every dump names its realm, and the bare `PATH` is never written**
/// (WS-E.1.3). While a session held one realm, `PATH` unambiguously named the
/// one view the M1.3 fidelity gate (`tests/integration/test_real_capture_fidelity.py`)
/// compares an agent's `observe()` against. With N realms it would name *a*
/// view — whichever realm the writer happened to mean — and a gate whose
/// ground truth is a guess is worse than no gate. Suffixing rather than
/// silently keeping `PATH` for the bound realm is the choice that makes a
/// mismatch between the dump's realm and the grant's realm impossible to
/// write by accident.
///
/// Appended to the whole path, not swapped into the extension: a realm id is
/// at most 64 bytes over `[A-Za-z0-9._-]` and never `.` or `..`
/// (`crate::realm::validate_realm`, which defers to
/// `vitrin_ipc::paths::shim_runtime_dir_in` so the rule has one definition),
/// so the result is always a distinct, path-safe sibling of the target —
/// including for a `PATH` that already has an extension, where
/// `with_extension` would have eaten it.
///
/// The charset is not `[a-z0-9-]`, which this comment used to claim. `.` and
/// `_` are legal in an id, and the argument survives that: `.` is legal
/// *inside* an id but a bare `.` or `..` is refused outright, and every legal
/// id is exactly the single path component the realm's own runtime directory
/// is named with — so no id can add a separator, escape the parent directory,
/// or name the parent itself. What it does **not** promise is that the
/// suffixed path fits any particular filesystem's name limit; a 64-byte id on
/// a `PATH` whose basename is already near `NAME_MAX` fails the write, which
/// [`write_capture_dump`] logs rather than treating as fatal — this is the
/// `--capture-dump` diagnostic, and a broken diagnostic must not take a
/// session down.
fn capture_dump_path(base: &std::path::Path, realm: &RealmId) -> PathBuf {
    let mut named = base.as_os_str().to_owned();
    named.push(".");
    named.push(realm.as_str());
    PathBuf::from(named)
}

/// Write the core-internal capture (`--capture-dump`, P1.8.5) atomically.
///
/// The bytes are the raw RGBA realm-view readback — `width * height * 4`,
/// rows top-down, exactly what one [`Runtime::view_cache`] entry holds.
/// Written to a
/// sibling temp and renamed into place so a reader (the P1.8.5 fidelity test)
/// never observes a half-written frame; the rename is atomic within a
/// directory, and the single-threaded loop means there is never a second
/// writer to race. Any I/O error is logged and swallowed: this is a
/// diagnostic, and a session must never die because a debug dump could not be
/// written.
///
/// The temp name is the target with `.part` **appended** (not an extension
/// swap): appending a non-empty suffix can never collide with the target, so
/// the atomicity holds for *every* dump path — including one that itself ends
/// in `.tmp`, where a `with_extension("tmp")` would have named the target
/// itself and written straight onto the reader-visible file.
fn write_capture_dump(path: &std::path::Path, rgba: &[u8]) {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = std::path::PathBuf::from(tmp);
    let write = std::fs::write(&tmp, rgba).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(err) = write {
        tracing::warn!(
            path = %path.display(),
            "capture-dump write failed: {err} (diagnostic only; the session continues)"
        );
        // Best-effort cleanup so a failed rename does not strand the temp file.
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;

    /// A lock surface with nothing raised, for composite assertions about the
    /// *other* overlays. A fresh one per call: [`crate::lock::LockSurface`]
    /// carries a generation counter and a raster cache, so a shared instance
    /// would let one caller's raise change what the next one measures.
    /// A status strip that is off — `--status` is opt-in.
    fn no_status() -> crate::status::StatusStrip {
        crate::status::StatusStrip::new(crate::status::StatusConfig::off())
    }

    fn no_lock() -> crate::lock::LockSurface {
        crate::lock::LockSurface::new(crate::consent::TrustedIndicator::for_test())
    }

    use calloop::{EventLoop, LoopSignal};
    use vitrin_ipc::Connection;
    use vitrin_protocol::generated::vitrin_grant::Outcome;
    use vitrin_protocol::generated::{
        vitrin_grant, vitrin_handshake, vitrin_principal, vitrin_realm,
    };
    use vitrin_protocol::wire::HEADER_LEN;

    use crate::consent::grab::Decision;
    use crate::consent::Choice;
    use crate::grants::PersistenceRung;
    use crate::identity::{
        PrincipalIdentity, StaticPrincipal, StaticVerifier, STATIC_TOKEN_SCHEME,
    };
    use crate::input::NoopHook;
    use crate::petitions::PetitionConfig;
    use crate::principal::HANDSHAKE_ID;
    use vitrin_protocol::generated::vitrin_grant::{Persistence, Verb};
    use vitrin_protocol::generated::PROTOCOL_VERSION;

    use super::*;

    const TOKEN: &str = "9b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a";
    const DEMO_IDENTITY: &str = "vitrin://local/agent/demo";
    /// A **second** principal, so a test can put one principal's consent
    /// prompt on screen while a *different* principal drives layout at it.
    /// With one identity that scenario is unreachable: the chokepoint's
    /// `consent_held` gate refuses a principal's own uses while its own
    /// prompt is up, so a single-identity rig could only ever test the gate,
    /// never the invariant behind it.
    const OTHER_IDENTITY: &str = "vitrin://local/agent/other";
    const OTHER_TOKEN: &str = "1a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d5e6f7081";
    const VIEW: (u32, u32) = (64, 40);
    /// The wire id the rig's petition mints for its `vitrin_grant`.
    const GRANT_ID: u32 = 4;

    /// A [`Presenter`] with no renderer: the runtime's contract with a
    /// backend is four small methods, and driving them with a counter is what
    /// makes "how many composites did one dispatch round cost" an assertable
    /// number rather than something a GPU hides.
    struct TestView {
        scenes: crate::scene::RealmScenes,
        redraws: usize,
        /// How many times the backend was asked to *schedule* a
        /// presentation. Distinct from [`Self::redraws`] on purpose: on a
        /// backend whose frame clock is external, `redraw` composites
        /// nothing and this is the only call that reaches the host
        /// compositor at all.
        presents: usize,
        /// Which frame-clock posture to answer with. Defaults to the
        /// headless one; the pacing test flips it to the nested one.
        posture: Presentation,
        /// The consent surface [`service_consent_round`] raises prompts on.
        /// Present on every test host — the nested backend carries one too —
        /// but only exercised when a grab is attached; the consent tests
        /// assert a card was raised on it.
        consent: ConsentSurface,
        /// The output's size. A **field** rather than the `VIEW` constant
        /// since WS-E.1.4: the two arrangements `set_fullscreen` chooses
        /// between are indistinguishable while the output's size and the
        /// realm's size are equal (IDL `set_fullscreen`), so a rig that
        /// could not resize its output could not tell them apart at all.
        /// The nested backend's `Resized` handler is what moves this in
        /// production.
        size: (u32, u32),
        /// The last position [`Presenter::set_agent_cursor`] was **offered**,
        /// `None` when it has not been called since a test cleared it.
        ///
        /// Recorded rather than acted on: this view composites nothing, and
        /// what WS-E.1.3 changed is which position `post_dispatch` decides to
        /// offer at all (the bound-realm gate). `Some(None)` — offered, and
        /// the offer was "draw nothing" — is a different fact from `None`,
        /// which is "the gate was never reached".
        cursor_offered: Option<Option<(f64, f64)>>,
        /// The last value [`Presenter::set_attention`] was offered — so a test
        /// can say what the human would have been shown, without this rig
        /// growing a second compositor to draw it with.
        attention: bool,
        /// Whether the lock cover is up, for [`Presenter::output_gates`]
        /// (WS-E.4.2). A plain flag rather than a real
        /// [`crate::lock::LockSurface`], because this rig composites nothing
        /// and what the pointer-constraint tests need is the *gate*, which is
        /// one boolean either way. The consent half of the same gate is read
        /// off the real [`Self::consent`], which a grab really does raise.
        lock_raised: bool,
        /// Whether the output can be presented to — the bare-metal
        /// `DrmOutput::active` in miniature, cleared by the seat's pause arm.
        /// `true` by default, which is the posture of every backend with no
        /// seat to lose.
        output_active: bool,
        /// **The idle blank's cover** (WS-E.4.3, issue #223), and unlike
        /// [`Self::lock_raised`] this is the **real**
        /// [`crate::backend::blank::BlankSurface`] rather than a flag.
        ///
        /// It has to be real, because the property CI is asked to prove about
        /// the blank is a statement about *composited bytes*: that a covered
        /// frame carries no pixel of the realm view and still carries an intact
        /// trusted band. A boolean could not answer that, and a second
        /// compositor written here to answer it would be the drift D-019 names.
        /// [`Self::human_visible`] therefore passes this one through the very
        /// function both shipped backends compose with.
        blank: crate::backend::blank::BlankSurface,
    }

    impl TestView {
        /// This realm's committed surface size, if it has one — the read the
        /// death-path tests use to check that a realm really painted and
        /// that the teardown funnel really took *its* surface away.
        fn surface_of(&self, realm: &RealmId) -> Option<(u32, u32)> {
            self.scenes.scene(realm).and_then(|s| s.surface_size())
        }

        /// **One frame of human-visible output**, through the very function
        /// both shipped backends compose theirs with
        /// ([`crate::backend::compose_human_visible`]).
        ///
        /// Not a second compositor. The D-018(2) invariant tests need real
        /// composited bytes and this rig otherwise has none; reimplementing
        /// the overlay stack here would test a copy of the compositor
        /// against itself, which is the drift D-019's cost note names. What
        /// this adds is the *call*, on the scene the output binding selects
        /// — the one thing a `layout_focus` holder can move.
        fn human_visible(&mut self) -> Vec<u8> {
            let (w, h) = self.size;
            crate::backend::compose_human_visible(
                self.scenes.bound(),
                &mut self.consent,
                &mut no_lock(),
                &self.blank,
                &mut no_status(),
                w,
                h,
                false,
            )
        }

        /// The bare realm view a capture of `realm` is taken from — the
        /// upstream half of the same pair, so a test can say "the overlay is
        /// in one and not the other" about two real buffers.
        fn capture_view(&self, realm: &RealmId) -> Vec<u8> {
            let (w, h) = self.size;
            self.scenes
                .scene(realm)
                .map(|scene| scene.compose(w, h))
                .unwrap_or_else(|| crate::test_pattern::render(w, h))
        }
    }

    impl Presenter for TestView {
        /// The attention marker is presentation the invariant tests never
        /// assert on; what they *do* assert is that no arrangement puts
        /// anything into a capture, and this rig's `human_visible` goes
        /// through the real `compose_human_visible` either way.
        fn set_attention(&mut self, open: bool) -> bool {
            let changed = self.attention != open;
            self.attention = open;
            changed
        }
        /// The status strip is off in this rig, for the same reason it is off
        /// by default in a real session: these tests compare human-visible
        /// output against captures byte for byte, and a clock in that
        /// comparison would make them a function of wall time.
        fn refresh_status(&mut self, _now: std::time::SystemTime, _mono: Instant) -> bool {
            false
        }
        /// The pointer-constraint gates (WS-E.4.2, issue #222).
        ///
        /// The consent term reads the **real** surface a grab raises a card on,
        /// so `a_consent_card_deactivates_a_pointer_constraint` drives the
        /// production path rather than a fixture; the other two are flags this
        /// rig has no compositor to derive.
        fn output_gates(&self) -> crate::input::OutputGates {
            crate::input::OutputGates {
                overlay_up: self.consent.prompt().is_some() || self.lock_raised,
                active: self.output_active,
                // Read off the real cover this rig composites through, so a
                // constraint test and the blank's own composite test cannot
                // disagree about whether the screen is dark.
                dark: self.blank.is_covering(),
            }
        }
        fn scene_mut(&mut self, realm: &RealmId) -> &mut Scene {
            self.scenes.scene_mut(realm)
        }
        fn scene(&self, realm: &RealmId) -> Option<&Scene> {
            self.scenes.scene(realm)
        }
        fn focused(&self) -> Option<&RealmId> {
            self.scenes.focused()
        }
        fn bind_output(&mut self, realm: &RealmId) {
            self.scenes.bind(realm);
        }
        fn unbind_output(&mut self) {
            self.scenes.unbind();
        }
        fn view_size(&self) -> (u32, u32) {
            self.size
        }
        /// This rig's output size is a plain field (the nested backend reads
        /// its host window instead), so this records it as well as
        /// propagating it — the same two facts, from the one call
        /// [`apply_output_resize`] makes.
        fn set_view_size(&mut self, size: (u32, u32)) {
            self.size = size;
            self.scenes.set_view_size(size);
        }
        /// Counts composites and reports whichever posture the test asked
        /// for: `Completed` (headless, the default) or `Scheduled` (nested,
        /// where the host compositor draws later).
        fn redraw(&mut self) -> Result<Presentation, Box<dyn Error>> {
            self.redraws += 1;
            Ok(self.posture)
        }
        /// **This realm's own** composition, exactly as both shipped
        /// backends serve — never a fixed frame for the session. A realm
        /// with no scene has no view, which the chokepoint turns into
        /// `no_surface`.
        ///
        /// Composed rather than stubbed because the cross-realm capture
        /// tests below are byte-exact: a `TestView` that answered the same
        /// bytes for every realm would let a runtime that handed out the
        /// wrong realm's frame pass them.
        fn view_rgba(&mut self, realm: &RealmId) -> Option<Vec<u8>> {
            Some(self.scenes.scene(realm)?.compose(VIEW.0, VIEW.1))
        }
        /// Counts the requests the nested backend turns into
        /// `Window::request_redraw`. Overridden rather than inherited as the
        /// trait's headless no-op, because "did this state change ever reach
        /// the compositor" is only assertable if something counts it.
        fn request_present(&mut self) {
            self.presents += 1;
        }
        /// This view composites no agent cursor — it has no framebuffer to
        /// paint one into — so it answers `false` and the dispatch round is
        /// unchanged by the sprite existing. Stated as an implementation
        /// rather than inherited from a trait default so that adding a
        /// presentation path is a compile error until someone decides
        /// whether it draws the sprite.
        fn set_agent_cursor(&mut self, pos: Option<(f64, f64)>) -> bool {
            self.cursor_offered = Some(pos);
            false
        }
        /// No retained image to scrub: this view keeps a counter, not a
        /// framebuffer. No GPU renderer either, so no importer.
        fn teardown_view<R>(
            &mut self,
            realm: &RealmId,
            f: impl for<'v> FnOnce(
                &'v mut Scene,
                Option<&'v mut dyn RetainedOutput>,
                Option<&'v mut dyn DmabufImporter>,
            ) -> R,
        ) -> R {
            f(self.scenes.scene_mut(realm), None, None)
        }
    }

    /// The rig's hook stack: the two real hooks whose signals the kernel reads
    /// back out of the router, and nothing above them.
    ///
    /// Not `NoopHook`, and for one reason stated twice. `Runtime::new` takes
    /// the attention signal (WS-E.1.7) and the screenshot signal (WS-E.2.4)
    /// *out of the router it is handed*; a rig whose stack carried neither
    /// would get detached signals nothing ever writes, and every test of either
    /// mechanism would be asserting against a rig that structurally cannot do
    /// the thing under test. The consent grab, the dead-man watcher and the
    /// lock stay out, exactly as they were: this rig has no display to raise a
    /// prompt on.
    type RigHook = crate::screenshot::ScreenshotHook<crate::attention::AttentionHook<NoopHook>>;

    struct TestHost {
        runtime: Runtime<RigHook>,
        view: TestView,
        handle: LoopHandle<'static, TestHost>,
        signal: LoopSignal,
        fatal: Option<Box<dyn Error>>,
        /// The consent input grab, when a test attaches one. `None` by
        /// default, so every existing test gets the trait's no-op
        /// [`RuntimeHost::service_consent`] and is unaffected; a consent test
        /// attaches one with [`Rig::attach_grab`] and the override below then
        /// drives [`service_consent_round`] each dispatch round, exactly as
        /// the nested backend does.
        grab: Option<Rc<RefCell<ConsentGrab>>>,
        /// What this rig's `service_consent` answers for
        /// [`PromptVisibility`] — the bare-metal backend's `DrmOutput::active`
        /// in miniature. `Reachable` by default, so every existing consent test
        /// is unaffected; a pause test flips it.
        ///
        /// **Not the whole answer since WS-E.4.3**: a blank test wants the
        /// visibility the *production* backend derives, not a fixture, so
        /// [`TestHost::service_consent`] resolves `ScreenIsDark` off the real
        /// cover exactly as `DrmState::service_consent` does, and falls back to
        /// this field otherwise.
        screen: PromptVisibility,
        /// The session's activity clock and blank state machine (WS-E.4.3,
        /// issue #223), when a test attaches one.
        ///
        /// `None` by default, so every existing test gets the trait's no-op
        /// [`RuntimeHost::service_screen`] and is unaffected — [`Self::grab`]'s
        /// posture, for the same reason. A blank test attaches one with
        /// [`Rig::attach_blank`] and the override below then drives
        /// [`service_blank_round`] each dispatch round, exactly as the
        /// bare-metal backend does.
        activity: Option<Rc<RefCell<crate::backend::blank::SessionActivity>>>,
        resume: crate::backend::blank::ResumeWatch,
        /// The wall clock this rig reports to [`RuntimeHost::service_screen`],
        /// overriding `SystemTime::now()` when a test sets it.
        ///
        /// Required for the resume detector, whose whole substance is the wall
        /// clock advancing further than the monotonic one: a test that had to
        /// *actually suspend the machine* to produce that would be no test.
        wall: Option<std::time::SystemTime>,
    }

    impl RuntimeHost for TestHost {
        type Hook = RigHook;
        type View = TestView;

        fn split(&mut self) -> (&mut Runtime<RigHook>, &mut TestView) {
            (&mut self.runtime, &mut self.view)
        }
        fn loop_handle(&self) -> LoopHandle<'static, Self> {
            self.handle.clone()
        }
        fn stop(&mut self, fatal: Option<Box<dyn Error>>) {
            self.fatal = fatal;
            self.signal.stop();
        }

        /// The nested backend's override in miniature: with a grab attached,
        /// run [`service_consent_round`] and mark the frame dirty on a change.
        /// The `Rc` is cloned first so the `RefMut` borrows nothing of `self`,
        /// leaving `runtime` and `view.consent` as disjoint field borrows —
        /// the same shape [`crate::backend::winit::NestedState`] uses.
        fn service_consent(&mut self, now: Instant) {
            let Some(grab) = self.grab.clone() else {
                return;
            };
            // The bare-metal backend's resolution, in miniature and in the same
            // order (WS-E.4.3): a paused screen first, then a dark one, then
            // reachable. Derived from the real cover rather than from a field,
            // so this rig cannot claim a visibility its own composite
            // contradicts.
            let visibility = if self.view.blank.is_covering() {
                PromptVisibility::ScreenIsDark
            } else {
                self.screen
            };
            let mut grab = grab.borrow_mut();
            if service_consent_round(
                &mut grab,
                &mut self.runtime,
                &mut self.view.consent,
                now,
                visibility,
            ) {
                self.runtime.dirty = true;
            }
        }

        /// The bare-metal backend's [`RuntimeHost::service_screen`] in
        /// miniature: the resume detector, then the blank's round.
        fn service_screen(&mut self, wall: std::time::SystemTime, now: Instant) {
            let Some(activity) = self.activity.clone() else {
                return;
            };
            let wall = self.wall.unwrap_or(wall);
            if let Some(_gap) = self.resume.sample(wall, now) {
                if let Some(grab) = self.grab.clone() {
                    screen_became_visible(&grab, now);
                }
                self.runtime.dirty = true;
            }
            // Scoped exactly as the bare-metal override scopes it, and for the
            // reason written up there: the guard is on the very cell a present
            // reads, so holding it across one is a panic in the compositor. The
            // two implementations are kept the same shape deliberately -- a rig
            // that was safe by accident would stop being a model of the thing it
            // stands in for.
            let changed = {
                let mut activity = activity.borrow_mut();
                service_blank_round(&mut self.runtime, &mut activity, &mut self.view.blank, now)
            };
            if changed {
                self.runtime.dirty = true;
            }
        }
    }

    /// A scratch directory, a bound listener inside it, and the socket path a
    /// client connects to.
    fn scratch_listener(label: &str) -> (PathBuf, Listener) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-session-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = Listener::bind(dir.join("core.sock")).expect("bind scratch core socket");
        (dir, listener)
    }

    fn demo_verifier() -> StaticVerifier {
        StaticVerifier::from_rows(
            vec![
                StaticPrincipal {
                    identity: PrincipalIdentity::parse(DEMO_IDENTITY).unwrap(),
                    token: TOKEN.as_bytes().to_vec(),
                    uid: None,
                },
                StaticPrincipal {
                    identity: PrincipalIdentity::parse(OTHER_IDENTITY).unwrap(),
                    token: OTHER_TOKEN.as_bytes().to_vec(),
                    uid: None,
                },
            ],
            rustix::process::geteuid().as_raw(),
        )
        .unwrap()
    }

    /// **Capture the log lines a block of code emits, on this thread only.**
    ///
    /// Two of issue #258's acceptance criteria are about log lines and nothing
    /// else — a successful wake and a failed one must be *distinguishable* —
    /// and the only honest way to assert that is to read what was emitted.
    ///
    /// Thread-local ([`tracing::subscriber::set_default`]) rather than global,
    /// for two reasons: `main.rs`'s auto-approve banner test already owns the
    /// one `set_global_default` this test binary may install and says so in its
    /// own comment, and everything under test here is emitted on the thread
    /// driving the rig's rounds.
    struct LogCapture {
        lines: std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>,
        /// Restores the previous default when the capture goes out of scope.
        _guard: tracing::subscriber::DefaultGuard,
    }

    impl LogCapture {
        fn install() -> Self {
            use tracing_subscriber::layer::SubscriberExt;

            let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let subscriber =
                tracing_subscriber::registry().with(CaptureLayer(std::sync::Arc::clone(&lines)));
            let guard = tracing::subscriber::set_default(subscriber);
            Self {
                lines,
                _guard: guard,
            }
        }

        /// Everything captured so far, draining the buffer.
        fn take(&self) -> Vec<(tracing::Level, String)> {
            std::mem::take(&mut *self.lines.lock().unwrap_or_else(|e| e.into_inner()))
        }

        /// Whether any captured line contains `needle`, without draining — for
        /// asserting that something has **not** been said yet.
        fn contains(&self, needle: &str) -> bool {
            self.lines
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|(_, message)| message.contains(needle))
        }
    }

    struct CaptureLayer(std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut message = MessageField(String::new());
            event.record(&mut message);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((*event.metadata().level(), message.0));
        }
    }

    /// Pulls the formatted `message` out of one event; the structured fields are
    /// deliberately ignored, because what these tests assert is the sentence a
    /// human reads in the log.
    struct MessageField(String);

    impl tracing::field::Visit for MessageField {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    /// A whole session on a scratch socket: the loop, the host, and the log
    /// path so a test can read back what the run recorded.
    struct Rig {
        event_loop: EventLoop<'static, TestHost>,
        host: TestHost,
        socket: PathBuf,
        dir: PathBuf,
        log: PathBuf,
    }

    impl Rig {
        fn new(label: &str, policy: ConsentPolicyArg) -> Self {
            let (dir, listener) = scratch_listener(label);
            let socket = listener.socket_path().to_path_buf();
            let (recorder, log) = crate::recorder::tests::scratch_recorder(label);
            let seed = RuntimeSeed {
                listener,
                verifier: demo_verifier(),
                petitions: PetitionRegistry::new(policy.policy, policy.config),
                grants: GrantTable::new(),
                realms: crate::realm::tests::registry_with(&[crate::realm::WELL_KNOWN_REALM_ID]),
                recorder,
                shim: crate::spawn::tests::mock_shim_bin(),
                indicator: crate::consent::TrustedIndicator::for_test(),
                capture_dump: None,
                screenshot_dir: None,
            };
            let event_loop: EventLoop<'static, TestHost> =
                EventLoop::try_new().expect("event loop");
            let handle = event_loop.handle();
            let mut host = TestHost {
                // **The real hook stack's innermost member**, on the router's
                // own clock cell (WS-E.1.7). Not `detached(NoopHook)`: with no
                // `AttentionHook` in the stack `Runtime::new` mints a detached
                // signal nothing ever opens, and every attention test here
                // would be asserting against a rig that structurally cannot do
                // the thing under test. The consent grab and the dead-man
                // watcher stay out, exactly as they were: this rig has no
                // display to raise a prompt on.
                runtime: Runtime::new(seed, {
                    let now = std::rc::Rc::new(std::cell::Cell::new(Instant::now()));
                    InputRouter::new(
                        std::rc::Rc::new(std::cell::RefCell::new(
                            crate::input::PhysicalPresenceMap::new(),
                        )),
                        std::rc::Rc::clone(&now),
                        crate::screenshot::ScreenshotHook::new(
                            std::rc::Rc::new(std::cell::RefCell::new(
                                crate::screenshot::ScreenshotSignal::new(
                                    crate::chord::ModChord::parse(
                                        crate::screenshot::DEFAULT_SCREENSHOT_CHORD,
                                    )
                                    .expect("the default screenshot chord parses"),
                                )
                                .expect("one binding is never a duplicate"),
                            )),
                            crate::attention::AttentionHook::new(
                                std::rc::Rc::new(std::cell::RefCell::new(
                                    crate::attention::AttentionSignal::new(
                                        crate::attention::AttentionChord::parse(
                                            crate::attention::DEFAULT_CHORD,
                                        )
                                        .expect("the default attention chord parses"),
                                    ),
                                )),
                                now,
                                NoopHook,
                            ),
                        ),
                    )
                }),
                view: TestView {
                    scenes: crate::scene::RealmScenes::new(VIEW),
                    redraws: 0,
                    presents: 0,
                    posture: Presentation::Completed,
                    consent: ConsentSurface::new(crate::consent::TrustedIndicator::for_test()),
                    size: VIEW,
                    cursor_offered: None,
                    attention: false,
                    lock_raised: false,
                    output_active: true,
                    blank: crate::backend::blank::BlankSurface::for_test(),
                },
                handle: handle.clone(),
                signal: event_loop.get_signal(),
                fatal: None,
                grab: None,
                screen: PromptVisibility::Reachable,
                activity: None,
                resume: crate::backend::blank::ResumeWatch::new(),
                wall: None,
            };
            install(&handle, &mut host.runtime).expect("install runtime sources");
            Rig {
                event_loop,
                host,
                socket,
                dir,
                log,
            }
        }

        /// Attach a consent input grab and return a shared handle to it.
        ///
        /// This is what turns [`TestHost::service_consent`] from the trait's
        /// no-op into the driven path: with a grab attached, every
        /// [`post_dispatch`] runs [`service_consent_round`] against it, so a
        /// pending interactive petition really is raised over the shipped
        /// wire. The returned `Rc` lets a test read the armed state and inject
        /// a human decision through the grab's test seam, exactly where a
        /// physical click would have deposited one.
        fn attach_grab(&mut self) -> Rc<RefCell<ConsentGrab>> {
            let grab = Rc::new(RefCell::new(ConsentGrab::new()));
            self.host.grab = Some(Rc::clone(&grab));
            grab
        }

        /// Arm the idle blank and return a shared handle to its state
        /// (WS-E.4.3, issue #223).
        ///
        /// [`Self::attach_grab`]'s shape and its purpose: this is what turns
        /// [`TestHost::service_screen`] from the trait's no-op into the driven
        /// path, so every [`post_dispatch`] runs [`service_blank_round`] against
        /// the real [`crate::backend::blank::BlankSurface`] the rig composites
        /// through — the fake presenter #223's acceptance criteria ask for.
        fn attach_blank(
            &mut self,
            after: Duration,
        ) -> Rc<RefCell<crate::backend::blank::SessionActivity>> {
            self.attach_blank_seeded(after, Instant::now())
        }

        /// The same, with the activity clock seeded at a chosen instant.
        ///
        /// The seam #257's test needs: "the human has been away longer than the
        /// idle timeout" is a statement about how old the clock is, and the rig
        /// runs on the real one. Seeding it in the past says that in a
        /// millisecond instead of five minutes, and it is the *production*
        /// constructor either way — [`crate::backend::blank::SessionActivity::new`]
        /// takes the seed for exactly this reason.
        fn attach_blank_seeded(
            &mut self,
            after: Duration,
            seeded: Instant,
        ) -> Rc<RefCell<crate::backend::blank::SessionActivity>> {
            let activity = Rc::new(RefCell::new(crate::backend::blank::SessionActivity::new(
                Some(after),
                seeded,
            )));
            self.host.activity = Some(Rc::clone(&activity));
            activity
        }

        /// Fork the real mock-shim binary into this rig's scratch runtime
        /// tree and put it on the loop, through the same
        /// [`start_realm_in`] the shipped backends call.
        ///
        /// The mock shim is both the `--shim` binary (the core's direct child,
        /// holding fd 3) and the realm's `command` app stand-in: the fixture
        /// flags in `args` ride the app-argument tail, which the mock scans
        /// (issue #103). The point is that the test drives the production spawn
        /// path rather than a hand-assembled `RealmRuntime`.
        fn start_realm(&mut self, args: &[&str]) {
            self.start_realms(&[(crate::realm::WELL_KNOWN_REALM_ID, args)]);
        }

        /// The same, for **N** realms in one session (WS-E.1.2): each id
        /// gets its own mock shim with its own fixture flags, and one
        /// `start_realm_in` forks them all — the production loop, not N
        /// calls to a single-realm path.
        fn start_realms(&mut self, realms: &[(&str, &[&str])]) {
            let mock = crate::spawn::tests::mock_shim_bin();
            let spec: Vec<(&str, PathBuf, &[&str])> = realms
                .iter()
                .map(|(id, args)| (*id, mock.clone(), *args))
                .collect();
            self.configure_realms(&spec);
            self.spawn_configured()
                .expect("every realm must spawn and attach");
        }

        /// Put exactly these realms in the registry, each with its **own**
        /// program — the seam [`Self::start_realms`] does not need and the
        /// partial-startup test does: a realm whose `command` does not
        /// resolve is refused by `spawn_realm`'s program audit, which is how
        /// a mid-loop spawn failure is produced deterministically rather
        /// than by racing something.
        fn configure_realms(&mut self, realms: &[(&str, PathBuf, &[&str])]) {
            self.host.runtime.kernel.realms = crate::realm::tests::registry_of(
                realms
                    .iter()
                    .map(|(id, command, args)| {
                        crate::realm::tests::realm_with_spawn(
                            id,
                            command,
                            &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                            &[],
                        )
                    })
                    .collect(),
            );
        }

        /// Run the production startup loop against this rig's scratch tree.
        fn spawn_configured(&mut self) -> Result<(), Box<dyn Error>> {
            let mock = crate::spawn::tests::mock_shim_bin();
            start_realm_in(&mut self.host, &SpawnPaths::under(&self.dir, &mock))
        }

        /// This rig's live shim session for `id`, or panic naming it —
        /// the successor of `runtime.realm.as_ref().expect(..)`, which
        /// could only ever mean one realm.
        fn realm(&self, id: &str) -> &RealmRuntime {
            self.host
                .runtime
                .realms
                .get(&RealmId::new(id))
                .unwrap_or_else(|| panic!("realm {id} is attached"))
        }

        /// Drive the real loop for `budget`, exactly as `run` does — every
        /// timer in these tests fires because calloop fired it.
        fn pump(&mut self, budget: Duration) {
            let deadline = Instant::now() + budget;
            while Instant::now() < deadline {
                self.event_loop
                    .dispatch(Some(Duration::from_millis(20)), &mut self.host)
                    .expect("dispatch");
                post_dispatch(&mut self.host);
            }
        }

        /// Pump until `done`, or fail. A condition rather than a sleep: the
        /// things these tests wait for (a shim finishing its animation, a
        /// child appearing) have no fixed duration, and a fixed budget would
        /// be either flaky or slow.
        fn pump_until(&mut self, budget: Duration, done: impl Fn(&TestHost) -> bool) {
            let deadline = Instant::now() + budget;
            while !done(&self.host) {
                assert!(Instant::now() < deadline, "timed out pumping the loop");
                self.event_loop
                    .dispatch(Some(Duration::from_millis(20)), &mut self.host)
                    .expect("dispatch");
                post_dispatch(&mut self.host);
            }
        }

        /// Every recorded entry, parsed. Closes the log first: a footer-less
        /// file is not what a reader would ever see.
        fn entries(&mut self) -> Vec<crate::recorder::tests::Json> {
            self.host.runtime.kernel.recorder.finish();
            crate::recorder::tests::read_log(&self.log)
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            crate::recorder::tests::cleanup(&self.log);
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    /// **The human's screenshot chord writes exactly one file of the focused
    /// realm's view, journals it once, and consults no grant** (WS-E.2.4,
    /// issue #216).
    ///
    /// The whole chain in one process, through the production entry points:
    /// real physical key events (from `input::physical_key`, the only
    /// physical-tagging path in the crate) into
    /// [`route_physical_turn`], through the rig's real
    /// [`ScreenshotHook`](crate::screenshot::ScreenshotHook), out of
    /// [`drain_screenshot_gestures`], onto a real file in a real audited
    /// directory, and into the real flight recorder.
    ///
    /// What each assertion is evidence *from*:
    ///
    /// 1. **One press, one file** — read off the directory, not off a counter.
    /// 2. **Its bytes are the focused realm's view** — compared against a fresh
    ///    encode of `Scene::compose` for that realm, so a screenshot of the
    ///    wrong realm, or of a stale cache entry, fails. This is the assertion
    ///    that would go green vacuously if it compared the file to itself.
    /// 3. **Exactly one `screenshot_written`, and its digest is the digest of
    ///    the bytes on disk** — so the journal identifies the artifact rather
    ///    than describing an intention.
    /// 4. **No grant, no principal, no use decision.** The rig has a grant
    ///    table and a petition registry and this press touches neither: the
    ///    journal carries no `use_decision` and no `grant_*` entry at all. A
    ///    negative, so it is paired with the positive above — a run in which
    ///    nothing happened would satisfy it.
    #[test]
    fn the_screenshot_chord_writes_the_focused_realms_view_and_touches_no_grant() {
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "screenshot-chord",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve", "--seat"];
        rig.start_realms(&[("realm-a", serve), ("realm-b", serve)]);
        rig.pump(Duration::from_millis(400));
        let (a, b) = (RealmId::new("realm-a"), RealmId::new("realm-b"));
        // Two realms with visibly different content, so "it shot the focused
        // one" is distinguishable from "it shot a realm".
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);

        // The audited directory, opened exactly as `run_session` opens it.
        let shots = rig.dir.join("shots");
        std::fs::create_dir_all(&shots).expect("mkdir");
        rig.host.runtime.kernel.screenshot_dir =
            Some(crate::screenshot::ScreenshotDir::open(&shots).expect("a clean private dir"));

        let chord = crate::chord::ModChord::parse(crate::screenshot::DEFAULT_SCREENSHOT_CHORD)
            .expect("the default chord parses");
        let (mods, trigger) = chord.scancodes();
        let switch = std::cell::RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        ));
        let press = |rig: &mut Rig, evdev: u32, state: KeyState| {
            route_physical_turn(
                &mut rig.host.runtime,
                &rig.host.view.scenes,
                Some(&switch),
                crate::input::physical_key(evdev, None, state),
                VIEW,
                Instant::now(),
            );
        };
        for evdev in &mods {
            press(&mut rig, *evdev, KeyState::Pressed);
        }
        press(&mut rig, trigger, KeyState::Pressed);
        press(&mut rig, trigger, KeyState::Released);
        for evdev in mods.iter().rev() {
            press(&mut rig, *evdev, KeyState::Released);
        }

        // 1. One press, one file.
        let written: Vec<_> = std::fs::read_dir(&shots)
            .expect("readdir")
            .map(|e| e.expect("entry").path())
            .collect();
        assert_eq!(
            written.len(),
            1,
            "one chord must write exactly one file, got {written:?}"
        );
        let bytes = std::fs::read(&written[0]).expect("read the screenshot");

        // 2. The bytes are the FOCUSED realm's view, re-encoded independently.
        let focused = rig
            .host
            .view
            .scenes
            .focused()
            .cloned()
            .expect("a realm is bound");
        assert_eq!(
            focused, a,
            "the rig starts with the output on the first realm"
        );
        let expect_rgb = |realm: &RealmId| -> Vec<u8> {
            rig.host
                .view
                .scenes
                .scene(realm)
                .expect("the realm has a scene")
                .compose(VIEW.0, VIEW.1)
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        };
        assert_eq!(
            bytes,
            vitrin_png::encode_rgb(VIEW.0, VIEW.1, &expect_rgb(&a)),
            "the file must be the focused realm's view, byte for byte"
        );
        assert_ne!(
            bytes,
            vitrin_png::encode_rgb(VIEW.0, VIEW.1, &expect_rgb(&b)),
            "...and the two realms really do compose differently, so the assertion \
             above is about which realm was shot"
        );

        // 3. One journal entry, whose digest identifies the bytes on disk.
        let entries = rig.entries();
        let of_kind = |kind: &str| -> Vec<&crate::recorder::tests::Json> {
            entries.iter().filter(|e| e.str("kind") == kind).collect()
        };
        let shots_logged = of_kind("screenshot_written");
        assert_eq!(shots_logged.len(), 1, "exactly one entry per trigger");
        let entry = shots_logged[0];
        assert_eq!(entry.str("realm"), "realm-a");
        assert_eq!(entry.u64("width"), u64::from(VIEW.0));
        assert_eq!(entry.u64("height"), u64::from(VIEW.1));
        assert_eq!(
            entry.str("digest"),
            crate::recorder::ObservationDigest::of(&bytes).to_hex(),
            "the journal's digest must be the digest of the file on disk"
        );
        assert_eq!(
            Some(entry.str("file")),
            written[0].file_name().and_then(|n| n.to_str()),
            "the entry names the file it wrote"
        );
        assert!(
            of_kind("screenshot_failed").is_empty(),
            "nothing failed on the way"
        );

        // 4. Nothing about authority happened.
        for kind in ["use_decision", "grant_minted", "grant_removed"] {
            assert!(
                of_kind(kind).is_empty(),
                "a human screenshot recorded a `{kind}`: it holds no grant and is no \
                 principal"
            );
        }
    }

    /// **The embedder round mirrors the gate onto the surface and writes the
    /// journal the gate cannot** (WS-E.2.2, issue #214).
    ///
    /// `LockScreen` is the single source of truth: it decides whether the
    /// session is locked and queues the facts; `service_lock_round` decides
    /// what the card says and what the flight recorder records. A surface
    /// raised without the gate is pixels with no grab behind them — a lock
    /// screen an app can type through — and a gate raised without the surface
    /// is every key vanishing with nothing on screen to explain it. This drives
    /// the real function against the real registry and reads the real log.
    ///
    /// Note what is asserted about the failed attempt: **exactly one entry, and
    /// it carries nothing about what was typed.** Issue #214 asks for the count;
    /// the secrecy half is checked here because a length is a real narrowing of
    /// an offline search and nothing here needs one.
    #[test]
    fn the_lock_round_mirrors_the_gate_and_journals_every_attempt() {
        use crate::lock::{LockCause, LockScreen, LockSurface};

        let mut rig = Rig::new(
            "lock-round",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let t0 = Instant::now();
        let mut screen = LockScreen::new(
            crate::chord::ModChord::parse(crate::lock::DEFAULT_LOCK_CHORD).unwrap(),
            None,
            None,
            Rc::new(RefCell::new(crate::backend::blank::SessionActivity::new(
                None, t0,
            ))),
        );
        let mut surface = LockSurface::new(crate::consent::TrustedIndicator::for_test());
        // No prompt is up in these cases, so `restart_guard` answers `false`;
        // the parameter is required rather than optional precisely so a caller
        // cannot forget the case where one IS up.
        let grab_for_lock_tests = std::cell::RefCell::new(ConsentGrab::new());

        // Unlocked: the round changes nothing and raises nothing.
        assert!(!service_lock_round(
            &mut screen,
            &mut surface,
            &mut rig.host.runtime,
            &grab_for_lock_tests,
            "esc",
            t0
        ));
        assert!(!surface.is_raised());

        // Locked: the round raises the surface once, and only once.
        screen.raise(LockCause::Chord);
        assert!(service_lock_round(
            &mut screen,
            &mut surface,
            &mut rig.host.runtime,
            &grab_for_lock_tests,
            "esc",
            t0
        ));
        assert!(surface.is_raised(), "the pixels must follow the gate");
        assert!(
            !service_lock_round(
                &mut screen,
                &mut surface,
                &mut rig.host.runtime,
                &grab_for_lock_tests,
                "esc",
                t0
            ),
            "an idempotent round must not invalidate the raster at frame cadence"
        );

        // One failed attempt, then an accepted one, journalled through the
        // round rather than by the gate.
        screen.journal_for_test(crate::lock::LockJournal::Attempted { accepted: false });
        screen.journal_for_test(crate::lock::LockJournal::Attempted { accepted: true });
        screen.journal_for_test(crate::lock::LockJournal::Unlocked);
        service_lock_round(
            &mut screen,
            &mut surface,
            &mut rig.host.runtime,
            &grab_for_lock_tests,
            "esc",
            t0,
        );

        let entries = rig.entries();
        let of_kind = |kind: &str| -> Vec<&crate::recorder::tests::Json> {
            entries.iter().filter(|e| e.str("kind") == kind).collect()
        };
        let locked = of_kind("session_locked");
        assert_eq!(locked.len(), 1, "one lock, one entry");
        assert_eq!(locked[0].str("cause"), "chord");
        assert!(!locked[0].bool("passphrase"));
        assert_eq!(locked[0].u64("realms"), 1);

        let attempts = of_kind("unlock_attempted");
        assert_eq!(
            attempts.len(),
            2,
            "one entry per attempt, never a summary: the rate is the signal"
        );
        assert!(!attempts[0].bool("accepted"));
        assert!(attempts[1].bool("accepted"));
        for attempt in &attempts {
            // The secrecy contract: no bytes, no digest, and NOT a length.
            for forbidden in ["bytes", "digest", "length", "passphrase", "attempt"] {
                assert!(
                    attempt.path(forbidden).is_none(),
                    "an unlock attempt must carry nothing about what was typed ({forbidden})"
                );
            }
        }
        assert_eq!(of_kind("session_unlocked").len(), 1);
    }

    struct ConsentPolicyArg {
        policy: crate::petitions::ConsentPolicy,
        config: PetitionConfig,
    }

    /// A client that speaks the wire the way an agent does.
    fn agent(socket: &std::path::Path) -> Connection {
        Connection::connect(socket).expect("connect to the core socket")
    }

    fn send_preamble(client: &mut Connection) {
        let hello = vitrin_handshake::requests::Hello {
            version: PROTOCOL_VERSION,
            principal: 2,
            identity: DEMO_IDENTITY.into(),
            credential_type: STATIC_TOKEN_SCHEME.into(),
            credential: TOKEN.into(),
        };
        client
            .send_message(&hello.encode(HANDSHAKE_ID), None)
            .expect("hello");
        let get_realm = vitrin_principal::requests::GetRealm {
            realm: 3,
            name: crate::realm::WELL_KNOWN_REALM_ID.into(),
        };
        client
            .send_message(&get_realm.encode(2), None)
            .expect("get_realm");
    }

    fn send_petition(client: &mut Connection) {
        let req = vitrin_realm::requests::RequestGrant {
            grant: 4,
            consent: 5,
            view: 6,
            pointer: 7,
            text: 8,
            resource: String::new(),
            verbs: Verb::OBSERVE,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: Persistence::WhileRunning,
            flags: 0,
        };
        client
            .send_message(&req.encode(3), None)
            .expect("request_grant");
    }

    /// Read frames until a `vitrin_grant.resolved` arrives, returning its
    /// outcome. Blocking: every caller has already pumped the loop long
    /// enough that the bytes are in the socket.
    fn await_resolution(client: &mut Connection) -> Outcome {
        await_resolution_at(client, GRANT_ID)
    }

    /// The same, for a grant at an arbitrary wire id — a session with more
    /// than one realm has more than one grant, at ids climbing above the
    /// watermark.
    fn await_resolution_at(client: &mut Connection, grant_id: u32) -> Outcome {
        for _ in 0..32 {
            let msg = client
                .recv_message()
                .expect("client receive")
                .expect("the core must not have hung up");
            // Object id as well as opcode: opcodes are per-interface, so
            // matching on the opcode alone would decode some other
            // interface's event as a resolution.
            if msg.header.object_id == grant_id
                && msg.header.opcode == vitrin_grant::events::Resolved::OPCODE
                && msg.bytes.len() >= HEADER_LEN
            {
                let (_, event) = vitrin_grant::events::Resolved::decode(&msg.bytes, msg.fd)
                    .expect("decode resolved");
                return event.outcome;
            }
        }
        panic!("no vitrin_grant.resolved arrived");
    }

    /// Capture through the wire on the facet `view_id` names, and read the
    /// delivered memfd back.
    ///
    /// The bytes, not the dimensions: what WS-E.1.3 is about is *whose*
    /// pixels arrive, and two realms at one output size have identical
    /// dimensions — a test that asserted only `width`/`height` would pass
    /// against the very leak this closes.
    fn capture_bytes(rig: &mut Rig, client: &mut Connection, view_id: u32) -> Vec<u8> {
        use std::os::unix::fs::FileExt;
        client
            .send_message(
                &vitrin_protocol::generated::vitrin_view::requests::CaptureFrame {}.encode(view_id),
                None,
            )
            .expect("capture_frame");
        rig.pump(Duration::from_millis(200));
        for _ in 0..32 {
            let msg = client
                .recv_message()
                .expect("client receive")
                .expect("the core must not have hung up");
            if msg.header.object_id != view_id {
                continue;
            }
            let (_, frame) = vitrin_protocol::generated::vitrin_view::events::FrameReady::decode(
                &msg.bytes, msg.fd,
            )
            .expect("a well-formed frame_ready");
            let len = (frame.stride * frame.height) as usize;
            let mut bytes = vec![0u8; len];
            std::fs::File::from(frame.fd)
                .read_exact_at(&mut bytes, 0)
                .expect("the sealed memfd must be readable");
            return bytes;
        }
        panic!("no frame_ready arrived on view {view_id}");
    }

    /// **Acceptance criterion 3.** A pending petition resolves `timed_out`
    /// because the *armed calloop timer* fired, and the terminal reaches the
    /// agent over the wire.
    ///
    /// Nothing here reaches the petition registry's expiry entry point, and
    /// [`the_expiry_sweep_has_no_second_caller`] is what keeps that true: the
    /// whole point of the criterion is that a test which reaches past the
    /// timer and pokes the function proves the *function* works while the
    /// wiring could be missing entirely — which is exactly the state this
    /// issue found the core in.
    #[test]
    fn a_pending_petition_times_out_through_the_armed_sweep() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "timeout",
            ConsentPolicyArg {
                // Interactive, so the petition pends for a human who never
                // comes -- the only way to reach the timeout at all.
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig {
                    consent_timeout: Duration::from_millis(200),
                    ..PetitionConfig::default()
                },
            },
        );
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);

        // Long enough for the accept, the handshake, the petition, the
        // 200 ms consent deadline, and at least one 1 s sweep tick.
        rig.pump(Duration::from_millis(2500));

        assert_eq!(
            await_resolution(&mut client),
            Outcome::TimedOut,
            "the armed sweep must resolve an unanswered petition and tell the agent"
        );
        // And the registry really is empty afterwards, so the sweep consumed
        // the petition rather than merely reporting on it.
        assert_eq!(rig.host.runtime.kernel.petitions.pending_total(), 0);
    }

    /// The armed sweep is the only path that expires petitions.
    ///
    /// This mirrors `enforcement`'s single-path guard, and for the same
    /// reason: acceptance criterion 3 forbids any test from calling the
    /// registry's expiry entry point directly, and a rule that lives only in
    /// an issue is a rule that dies with the next contributor who has not
    /// read it. Greping the source makes the criterion enforceable by CI.
    ///
    /// The name it looks for is assembled at runtime rather than written
    /// here, so this guard is not its own first hit.
    #[test]
    fn the_expiry_sweep_has_no_second_caller() {
        let source = include_str!("session.rs");
        let (production, tests) = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("this module ends with its test module");
        // Assembled rather than written out, so this guard's own prose is
        // not the hit it reports.
        let needle = format!("expire{}due", "_");
        assert!(
            !tests.contains(&needle),
            "a test in this module reaches past the armed timer and calls {needle} directly. \
             That proves the function works while the wiring could be missing entirely, which \
             is the exact state issue #77 found this core in"
        );
        // ...and in production it has exactly one call site each, in `sweep`.
        assert_eq!(
            production.matches(&format!("{needle}(now)")).count(),
            2,
            "petitions and grants are swept from `sweep` and nowhere else"
        );
    }

    /// **Acceptance criterion 4.** Closing a connection removes its grant
    /// rows — the version-1 rule that a principal's authority dies with its
    /// connection.
    #[test]
    fn closing_a_connection_removes_its_grant_rows() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "teardown",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(400));

        assert_eq!(
            await_resolution(&mut client),
            Outcome::Granted,
            "auto-approve grants the demo principal's petition"
        );
        let now = Instant::now();
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(now).count(),
            1,
            "a granted petition mints exactly one row"
        );

        // Clean EOF: the transport's `Disconnected`, one of the three close
        // paths that must reach teardown.
        drop(client);
        rig.pump(Duration::from_millis(300));

        assert_eq!(
            rig.host.runtime.connection_count(),
            0,
            "the closed connection must be forgotten"
        );
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(now).count(),
            0,
            "teardown must remove the connection's grant rows, not leave them to expire"
        );
    }

    /// **Two cores, one runtime tree: the second refuses rather than
    /// destroying the first's.**
    ///
    /// The refusal is only half the claim, and the half a lazy test stops
    /// at. The other half is that the first core is *undamaged* — its realm
    /// directory and socket still there, its shim still alive, and its
    /// listener still serving a fresh agent afterwards — because the failure
    /// this guards against is not "two cores ran" but "core B unlinked core
    /// A's socket and recursively deleted the realm directory A's live shim
    /// is bound to, then bound `core.sock` itself, so new agents reached B
    /// while the authority they think they hold lives in A".
    ///
    /// The mechanism is `Listener::bind`'s `flock` on `core.sock.lock`, and
    /// task 7 is discharged by *calling* it: the lock was already written
    /// and TOCTOU-hardened, it simply had no caller outside `vitrin-ipc`'s
    /// own tests. `flock` rather than a pidfile because the kernel releases
    /// it on process death including `SIGKILL`, so there is no stale state
    /// and no pid-reuse heuristic.
    #[test]
    fn a_second_core_on_one_runtime_tree_refuses_and_leaves_the_first_serving() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "single-core",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        rig.start_realm(&["--serve", "--animate", "100000"]);
        let shim = rig.realm(crate::realm::WELL_KNOWN_REALM_ID).life.pid();
        let realm_dir = rig
            .dir
            .join("vitrin-0")
            .join(crate::realm::WELL_KNOWN_REALM_ID);
        rig.pump(Duration::from_millis(200));
        assert!(realm_dir.is_dir(), "the first core's realm tree exists");

        // Core B, against the same tree.
        let refusal = match Listener::bind(&rig.socket) {
            Ok(_) => panic!("a second core must not bind a tree another core holds"),
            Err(err) => err,
        };
        assert_eq!(
            refusal.kind(),
            std::io::ErrorKind::AddrInUse,
            "the refusal must be AddrInUse, which is what `run_session` reports as fatal: {refusal}"
        );

        // ...and the first core is untouched.
        assert!(
            rig.socket.exists(),
            "the refused core must not have unlinked the live core's socket"
        );
        assert!(
            realm_dir.is_dir(),
            "the refused core must not have purged a realm directory a live shim is serving"
        );
        assert!(
            process_is_alive(shim),
            "the refused core must not have disturbed the live core's shim"
        );

        // Still *serving*, not merely still running: a fresh agent completes
        // a handshake and a petition after the refusal.
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut client),
            Outcome::Granted,
            "the surviving core must still serve new agents after refusing a second core"
        );

        drop(client);
        shutdown_realm(&mut rig.host);
    }

    /// A peer that connects and says nothing is closed, rather than holding a
    /// descriptor and a connection id for as long as it likes.
    #[test]
    fn an_unauthenticated_peer_does_not_hold_a_connection_forever() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "silent",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let _client = agent(&rig.socket);
        rig.pump(Duration::from_millis(200));
        assert_eq!(
            rig.host.runtime.connection_count(),
            1,
            "the connection is accepted and kept while the handshake could still arrive"
        );
        // The production deadline is 10 s, which no test may sit through, so
        // this asserts the *arming* rather than the elapse: a connection with
        // no armed deadline is the regression that matters, and it is
        // visible here.
        assert!(
            rig.host
                .runtime
                .conns
                .values()
                .all(|c| c.deadline.is_some()),
            "every accepted connection must carry an armed unauthenticated-phase deadline"
        );
    }

    /// The shim socketpair is really serviced by the loop: a **forked**
    /// mock shim completes its whole session — `configure`, surface, N paced
    /// frames — against the runtime rather than against a test harness.
    ///
    /// This is the wedge (T1) asserted rather than reasoned about, and it is
    /// asserted against the real path: [`start_realm_in`] forks and execs the
    /// mock-shim binary, sends `configure` on the still-blocking fd, and
    /// hands the connection to the loop. Every wait on the shim's side is a
    /// blocking `recv` with no timeout anywhere, so a loop that did not
    /// service the socketpair would not fail this test — it would hang it
    /// forever.
    ///
    /// The pacing assertion is the second half: each frame's `frame_done` is
    /// produced by [`post_dispatch`] and pushed through the realm's
    /// [`Outbox`], which is the only reason a shim blocked in `recv` ever
    /// hears from a compositor it is not talking to.
    #[test]
    fn a_forked_shim_runs_a_whole_session_over_the_runtime_loop() {
        let _fd = crate::capture::tests::fd_lock();
        const FRAMES: u32 = 3;
        let mut rig = Rig::new(
            "shim",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        rig.start_realm(&["--serve", "--animate", &FRAMES.to_string()]);

        let pid = rig.realm(crate::realm::WELL_KNOWN_REALM_ID).life.pid();

        // Every frame the shim was asked for must come back as a composite.
        // It paces on `frame_done`, so it cannot start frame N+1 until the
        // core presented frame N -- which means reaching FRAMES composites
        // proves the whole round trip ran FRAMES times, and would hang
        // rather than fail if the loop stopped servicing the socketpair.
        rig.pump_until(Duration::from_secs(10), |host| {
            host.view.redraws >= FRAMES as usize
        });
        assert!(
            rig.host.view.redraws <= FRAMES as usize + 1,
            "presentation must be coalesced: {} composites for {FRAMES} paced commits",
            rig.host.view.redraws
        );

        // The shim is still alive here -- `--animate N` falls into a drain
        // loop and serves until the core hangs up -- so this exercises the
        // ladder's live-shim path rather than an already-dead one.
        assert!(
            process_is_alive(pid),
            "the shim serves until the core hangs up"
        );
        shutdown_realm(&mut rig.host);
        assert!(
            !process_is_alive(pid),
            "the shutdown ladder must leave no shim behind"
        );

        // The death went through the lifecycle funnel, not a private path:
        // the registry says exited, so a petition for this realm now
        // resolves `unavailable`.
        assert!(
            !rig.host
                .runtime
                .kernel
                .realms
                .get(crate::realm::WELL_KNOWN_REALM_ID)
                .expect("the realm is registered")
                .admits_petitions(),
            "a dead realm must stop admitting petitions -- which only \
             RealmLifecycle's death funnel does"
        );

        let entries = rig.entries();
        for expected in ["realm_spawned", "realm_died", "realm_exited"] {
            assert!(
                entries.iter().any(|e| e.str("kind") == expected),
                "the run must journal {expected}; got {:?}",
                entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
            );
        }
    }

    // ------------------------------------------------------------------
    // Realm launch (WS-E.1.1, issue #207)
    // ------------------------------------------------------------------

    /// **Startup forks the realms that autostart and no others**, and a
    /// template is still addressable while not running.
    ///
    /// The failure this guards is silent in both directions: a startup that
    /// forked templates would run apps the operator asked it to hold back,
    /// and one that refused to *register* them would make `realm_launch`
    /// unpetitionable over exactly the realms it exists for.
    #[test]
    fn startup_forks_autostarting_realms_and_leaves_templates_alone() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "templates",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let mock = crate::spawn::tests::mock_shim_bin();
        rig.host.runtime.kernel.realms = crate::realm::tests::registry_of(vec![
            crate::realm::tests::realm_with_spawn(
                crate::realm::WELL_KNOWN_REALM_ID,
                &mock,
                &["--serve".to_string()],
                &[],
            ),
            crate::realm::tests::template_with_spawn("kiosk", &mock, &["--serve".to_string()]),
        ]);
        rig.spawn_configured().expect("startup must succeed");

        assert!(
            rig.host
                .runtime
                .realms
                .contains_key(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)),
            "an autostarting realm must have a live shim session"
        );
        assert!(
            !rig.host.runtime.realms.contains_key(&RealmId::new("kiosk")),
            "a template must NOT be forked at startup: that is what the key says"
        );
        // ...and it is still a realm a petition can name, which is the half
        // that makes the verb usable at all.
        assert_eq!(
            rig.host.runtime.kernel.realms.resolve_for_petition("kiosk"),
            Some(&RealmId::new("kiosk"))
        );
        assert_eq!(
            rig.entries()
                .iter()
                .filter(|e| e.str("kind") == "realm_spawned")
                .count(),
            1,
            "exactly one realm may have been spawned"
        );
        shutdown_realm(&mut rig.host);
    }

    /// **An admitted launch really forks a shim, from the template's
    /// configuration, under a core-minted id** — the production path
    /// (`launch_realm` → `apply_launches` → `attach_spawned_realm`) driven
    /// against the real mock-shim binary and the rig's own runtime tree.
    ///
    /// The wire half is `principal.rs`'s (`launched` is a terminal naming a
    /// minted id) and the mock-free half is
    /// `tests/integration/test_launch.py`'s. What is asserted here is the
    /// middle: that the two ends are connected by an actual process.
    #[test]
    fn a_launch_forks_the_templates_program_into_a_new_realm() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "launch",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let mock = crate::spawn::tests::mock_shim_bin();
        rig.host.runtime.kernel.realms = crate::realm::tests::registry_of(vec![
            crate::realm::tests::realm_with_spawn(
                crate::realm::WELL_KNOWN_REALM_ID,
                &mock,
                &["--serve".to_string()],
                &[],
            ),
            crate::realm::tests::template_with_spawn("kiosk", &mock, &["--serve".to_string()]),
        ]);
        rig.spawn_configured().expect("startup must succeed");

        let who = crate::identity::PrincipalIdentity::parse("vitrin://local/agent/demo")
            .expect("fixture identity");
        let grant = crate::grants::GrantId::from_u64_for_test(9);
        let paths = SpawnPaths::under(&rig.dir, &mock);
        let mut queued = Vec::new();
        let minted = launch_realm(
            &rig.host.runtime.kernel.realms,
            &paths,
            &mut queued,
            LaunchAsk {
                template: &RealmId::new("kiosk"),
                principal: &who,
                grant,
            },
        )
        .expect("the launch must be served");
        assert_eq!(
            minted.to_string(),
            "kiosk.1",
            "the id is minted by the registry, not supplied"
        );
        apply_launches(&mut rig.host, queued);

        // A live shim session under the minted id, registered and running --
        // the three things that make it a realm rather than a name.
        let pid = rig
            .host
            .runtime
            .realms
            .get(&RealmId::new("kiosk.1"))
            .expect("the launched realm has a live shim session")
            .life
            .pid();
        assert!(process_is_alive(pid), "the launch must have forked");
        assert!(matches!(
            rig.host
                .runtime
                .kernel
                .realms
                .get("kiosk.1")
                .expect("the instance is registered")
                .state(),
            crate::realm::RealmState::Running { .. }
        ));
        // The instance runs the TEMPLATE's program -- the command never came
        // off the wire, and there is no wire in this test at all.
        assert_eq!(
            rig.host
                .runtime
                .kernel
                .realms
                .get("kiosk.1")
                .unwrap()
                .spawn()
                .command(),
            mock.as_path()
        );
        // ...and the journal says who asked, which is the question the whole
        // entry was extended for.
        let entries = rig.entries();
        let launched = entries
            .iter()
            .find(|e| e.str("kind") == "realm_spawned" && e.str("realm") == "kiosk.1")
            .expect("the launch is journaled");
        assert_eq!(launched.str("spawned_by"), "realm_launch");
        assert_eq!(launched.str("principal"), "vitrin://local/agent/demo");
        assert_eq!(launched.str("grant_id"), "grant-9");
        shutdown_realm(&mut rig.host);
    }

    /// **A session at [`crate::realm::MAX_REALMS`] refuses `capacity`, and
    /// creates nothing while doing it.**
    ///
    /// `capacity` rather than `internal` because it is a policy answer: the
    /// deployment is full, retrying is legal once a realm exits, and the
    /// IDL gave it its own code for exactly that reason.
    #[test]
    fn a_launch_past_the_realm_cap_refuses_capacity_and_forks_nothing() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "launch-cap",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let mock = crate::spawn::tests::mock_shim_bin();
        // A registry already at the cap, with nothing forked: the cap is a
        // question about rows, and this test is about the answer rather than
        // about sixteen live shims.
        let full: Vec<crate::realm::Realm> = (0..crate::realm::MAX_REALMS)
            .map(|i| crate::realm::tests::template_with_spawn(&format!("r{i}"), &mock, &[]))
            .collect();
        rig.host.runtime.kernel.realms = crate::realm::tests::registry_of(full);

        let who = crate::identity::PrincipalIdentity::parse("vitrin://local/agent/demo")
            .expect("fixture identity");
        let paths = SpawnPaths::under(&rig.dir, &mock);
        let mut queued = Vec::new();
        let refusal = launch_realm(
            &rig.host.runtime.kernel.realms,
            &paths,
            &mut queued,
            LaunchAsk {
                template: &RealmId::new("r0"),
                principal: &who,
                grant: crate::grants::GrantId::from_u64_for_test(1),
            },
        )
        .expect_err("a full session must refuse");
        assert_eq!(refusal, LaunchRefusal::Capacity);
        assert!(
            queued.is_empty(),
            "a refusal that creates nothing must not queue a spawn to attach"
        );
        assert_eq!(
            rig.host.runtime.kernel.realms.len(),
            crate::realm::MAX_REALMS,
            "no row may be minted for a launch that was refused"
        );
    }

    // ------------------------------------------------------------------
    // Multi-realm (WS-E.1.2, issue #208)
    // ------------------------------------------------------------------

    /// **Three configured realms become three forked shims, three runtime
    /// trees and three private sockets** — the paths half of `realm.rs`'s
    /// "a deletion rather than a re-plumbing" claim, which really was a
    /// deletion: `vitrin_ipc::paths` derives every path from the realm id,
    /// so N realms are N trees with no new path code.
    ///
    /// Driven through [`start_realm_in`]'s own loop, so what is asserted is
    /// the production spawn path with more than one realm in the registry
    /// rather than three calls to a single-realm path.
    #[test]
    fn three_configured_realms_spawn_three_shims_with_three_private_trees() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "multi-spawn",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve", "--animate", "100000"];
        rig.start_realms(&[("realm-0", serve), ("editor", serve), ("browser", serve)]);

        // Three live shim sessions, three distinct pids, three distinct ids.
        let ids: Vec<String> = rig
            .host
            .runtime
            .realms
            .keys()
            .map(|id| id.as_str().to_string())
            .collect();
        assert_eq!(ids, ["browser", "editor", "realm-0"]);
        let pids: std::collections::BTreeSet<u32> = ["realm-0", "editor", "browser"]
            .iter()
            .map(|id| rig.realm(id).life.pid())
            .collect();
        assert_eq!(pids.len(), 3, "three realms must be three processes");
        for pid in &pids {
            assert!(process_is_alive(*pid), "every spawned shim must be alive");
        }

        // Three distinct `$XDG_RUNTIME_DIR/vitrin-0/<id>/` trees, and three
        // distinct `wayland-0` paths injected as each shim's own
        // `WAYLAND_DISPLAY`.
        //
        // **What the per-realm socket buys is addressing, not
        // confinement.** Every realm's shim and app run with the core's own
        // uid and the core's whole filesystem view (D9, no sandbox:
        // `docs/book/src/limits.md`), so an app that ignores
        // `WAYLAND_DISPLAY` and opens a sibling realm's `wayland-0` by a
        // path it guessed is stopped by nothing here — and this change made
        // that worse by multiplying the number of such paths. What the
        // separate address *does* guarantee is that a well-behaved client
        // reaches its own shim, and that a shim's Wayland universe contains
        // only its own app: there is nothing to enumerate over the protocol.
        // Structural scoping is that; it is not filesystem isolation, and
        // this test asserts only the former.
        //
        // Asserted from `/proc/<pid>/environ` rather than from the string
        // the core would have built, because the claim is about what the
        // child actually got. The socket *file* is bound by the shim, and
        // `vitrin-mock-shim` binds none — that half belongs to the C shim
        // and to `tests/integration/test_real_app.py`; what the core owns is
        // the per-realm tree and the per-realm address, and that is what
        // this asserts.
        let mut sockets = std::collections::BTreeSet::new();
        for id in ["realm-0", "editor", "browser"] {
            let dir = rig.dir.join("vitrin-0").join(id);
            assert!(dir.is_dir(), "{id} must have its own runtime tree");
            assert_eq!(
                rig.realm(id).life.runtime_dir(),
                dir,
                "{id}'s lifecycle must own the tree its id names"
            );
            let env = environ_of(rig.realm(id).life.pid());
            let display = env
                .iter()
                .find_map(|kv| kv.strip_prefix("WAYLAND_DISPLAY="))
                .unwrap_or_else(|| panic!("{id}'s shim must be handed a WAYLAND_DISPLAY"));
            assert_eq!(
                std::path::Path::new(display),
                dir.join("wayland-0"),
                "{id} must be pointed at its own socket, not a shared one"
            );
            sockets.insert(display.to_string());
        }
        assert_eq!(sockets.len(), 3, "the sockets must not be shared");

        // Every realm is journalled as spawned, with its own id.
        shutdown_realm(&mut rig.host);
        let entries = rig.entries();
        let spawned: std::collections::BTreeSet<String> =
            crate::recorder::tests::of_kind(&entries, "realm_spawned")
                .iter()
                .map(|e| e.str("realm").to_string())
                .collect();
        assert_eq!(
            spawned,
            ["browser", "editor", "realm-0"]
                .iter()
                .map(|s| s.to_string())
                .collect::<std::collections::BTreeSet<_>>(),
            "each realm must produce its own realm_spawned record"
        );
        // ...and the shutdown ladder ran once per realm, not once.
        for pid in &pids {
            assert!(
                !process_is_alive(*pid),
                "the shutdown ladder must leave no shim behind, in any realm"
            );
        }
    }

    /// **A realm that fails to spawn takes the already-forked ones with
    /// it** (WS-E.1.2 review, HIGH 3).
    ///
    /// Startup is a loop now, so it has a partial state that a single spawn
    /// never had: realms 1..k running when realm k+1 fails. Neither backend
    /// reaches its own `shutdown_realm` from there — both `return Err(err)`
    /// before the event loop starts — so if [`start_realm_in`] does not tear
    /// down what it started, the failure leaves shim processes, runtime
    /// trees and held `flock`s outliving the core that forked them, and the
    /// next core finds those directories locked by nobody.
    ///
    /// The failure is produced by a `command` that does not resolve, which
    /// `spawn_realm`'s program audit refuses before it creates anything —
    /// deterministic, and it leaves the *failing* realm with nothing of its
    /// own to clean up, so what the assertions below see is exactly the
    /// sibling teardown.
    #[test]
    fn a_realm_that_fails_to_spawn_tears_down_the_realms_already_forked() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "partial-start",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let mock = crate::spawn::tests::mock_shim_bin();
        let missing = rig.dir.join("no-such-program");
        assert!(!missing.exists());
        // Id order is spawn order (`BTreeMap`), so "aaa" and "bbb" fork and
        // attach before "zzz" is refused.
        let serve: &[&str] = &["--serve", "--animate", "100000"];
        rig.configure_realms(&[
            ("aaa", mock.clone(), serve),
            ("bbb", mock.clone(), serve),
            ("zzz", missing.clone(), &[]),
        ]);

        let err = rig
            .spawn_configured()
            .expect_err("a realm whose command does not resolve must abort startup")
            .to_string();
        assert!(
            err.contains("no-such-program"),
            "the error must name the program that could not be spawned: {err}"
        );

        // Two realms really were forked -- otherwise this test would pass
        // vacuously on a loop that never got that far.
        let entries = rig.entries();
        let spawned = crate::recorder::tests::of_kind(&entries, "realm_spawned");
        let pids: Vec<(String, u32)> = spawned
            .iter()
            .map(|e| (e.str("realm").to_string(), e.u64("pid") as u32))
            .collect();
        assert_eq!(
            pids.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["aaa", "bbb"],
            "the two realms before the failure must have been forked"
        );

        // ...and none of them is still running, still holding a tree, or
        // still in the runtime map.
        for (id, pid) in &pids {
            assert!(
                !process_is_alive(*pid),
                "{id}'s shim must not outlive the core that forked it"
            );
            let dir = rig.dir.join("vitrin-0").join(id);
            assert!(
                !dir.exists(),
                "{id}'s runtime tree must be gone, not left for the next core to find locked"
            );
        }
        assert!(
            rig.host.runtime.realms.is_empty(),
            "the runtime must hold no realm after a failed startup"
        );
        // Idempotent: the backend's own shutdown path finds nothing to do,
        // rather than running a second ladder over torn-down realms.
        shutdown_realm(&mut rig.host);
    }

    /// **Killing one realm's app leaves the others `Running`** — issue
    /// #208's third acceptance criterion, and the one the re-plumbing is
    /// most likely to get wrong: `SIGCHLD` says only that *some* child
    /// changed state, so a reaper that stopped at the first exit would leave
    /// a zombie and a realm the registry still called `Running`.
    ///
    /// Asserted through [`reap_realm`], the function the backends' `SIGCHLD`
    /// source calls, rather than by poking the lifecycle directly.
    #[test]
    fn killing_one_realm_leaves_its_siblings_running_and_petitionable() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "multi-death",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        // `--seat` so a routed event can actually land, which is how the
        // survivors' *serving* is asserted below rather than only their
        // registry state.
        let serve: &[&str] = &["--serve", "--seat", "--animate", "100000"];
        rig.start_realms(&[("realm-0", serve), ("editor", serve), ("browser", serve)]);
        // Deliberately the FIRST realm in id order ("browser" < "editor" <
        // "realm-0"): `route_seat` and the nested backend's delivery sink
        // both take the first still-serving realm, so killing the first is
        // what would expose a skip that only tolerated the *last* realm
        // dying.
        let victim = rig.realm("browser").life.pid();
        let survivors = [
            rig.realm("realm-0").life.pid(),
            rig.realm("editor").life.pid(),
        ];
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            host.runtime
                .realms
                .values()
                .filter_map(|realm| realm.server.as_ref())
                .filter(|server| server.seat_minted())
                .count()
                == 3
        });

        // A real kill, not a simulated one: the shim dies, the kernel closes
        // its end of the socketpair, and both death signals (EOF and
        // SIGCHLD) become available to the loop.
        assert_eq!(
            unsafe { libc::kill(victim as libc::pid_t, libc::SIGKILL) },
            0,
            "the victim must be signalable"
        );
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            !host
                .runtime
                .kernel
                .realms
                .get("browser")
                .expect("browser is registered")
                .admits_petitions()
        });
        // The SIGCHLD source's own entry point, over every live realm.
        reap_realm(&mut rig.host);

        // Exactly one realm died, and it is the one that was killed.
        assert!(
            !rig.host
                .runtime
                .kernel
                .realms
                .get("browser")
                .unwrap()
                .admits_petitions(),
            "the killed realm must be vacant"
        );
        for id in ["realm-0", "editor"] {
            let realm = rig.host.runtime.kernel.realms.get(id).unwrap();
            assert!(
                matches!(realm.state(), crate::realm::RealmState::Running { .. }),
                "{id} must still be Running, got {:?}",
                realm.state()
            );
            assert!(realm.admits_petitions(), "{id} must still answer petitions");
        }
        for pid in survivors {
            assert!(
                process_is_alive(pid),
                "a sibling's death must not touch a survivor's process"
            );
        }
        // No zombie: the reaper polled every realm, so the victim is gone
        // from /proc entirely rather than left unwaited.
        assert!(
            !process_is_alive(victim),
            "the killed shim must be reaped, not left a zombie -- which is what a \
             reaper that stopped at the first realm would leave behind"
        );

        // ...and a routed actuation still reaches a surviving realm. The
        // delivery target is "the first realm that still holds a shim
        // server", so a naive `.next()` would have handed every event to the
        // corpse and dropped it silently — the exact class of failure this
        // change's placeholder routing could hide.
        route_seat(
            &mut rig.host,
            vec![(
                RealmId::new("editor"),
                SeatInput::emulated(crate::input::SeatInputKind::Text {
                    text: "after".into(),
                }),
            )],
        );

        shutdown_realm(&mut rig.host);
        let entries = rig.entries();
        let died = crate::recorder::tests::of_kind(&entries, "realm_died");
        let for_victim: Vec<_> = died
            .iter()
            .filter(|e| e.str("realm") == "browser")
            .collect();
        assert_eq!(
            for_victim.len(),
            1,
            "exactly one realm_died for the killed realm (the death latch); got {died:#?}"
        );
        assert_eq!(
            crate::recorder::tests::of_kind(&entries, "seat_delivered").len(),
            1,
            "an actuation after a realm's death must still be delivered to a survivor"
        );
    }

    /// **A sibling realm's death does not disturb the surviving realm's
    /// seat state** (WS-E.1.2 review, HIGH 2) — driven through the real
    /// death path, not through [`InputRouter::reset_for`] directly.
    ///
    /// One router serves the session; a realm dying used to clear it
    /// unconditionally, so realm A's exit forgot that realm B's app was
    /// holding a key down. The release then arrives unpaired at B's seat and
    /// is dropped, and the key latches in a live app with nothing in the
    /// journal to say so — no delivery happened, so nothing was recorded.
    ///
    /// The victim is deliberately **not** the realm the actuation names: the
    /// grant below is over `browser`, and `realm-0` is the sibling whose
    /// death must be inconsequential. Killing `browser` instead would prove
    /// nothing — clearing *is* right there.
    #[test]
    fn a_realms_death_does_not_clear_a_surviving_realms_held_key() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "multi-router",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve", "--seat", "--animate", "100000"];
        rig.start_realms(&[("realm-0", serve), ("browser", serve)]);
        let sibling = rig.realm("realm-0").life.pid();
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            host.runtime
                .realms
                .values()
                .filter_map(|realm| realm.server.as_ref())
                .filter(|server| server.seat_minted())
                .count()
                == 2
        });

        // A key press through the production actuation route, so the router
        // records the delivery debt exactly as a live agent would make it.
        const SHIFT_L: u32 = 0xffe1;
        route_seat(
            &mut rig.host,
            vec![(
                RealmId::new("browser"),
                SeatInput::emulated(crate::input::SeatInputKind::Key {
                    source: crate::input::KeySource::Keysym,
                    keysym: SHIFT_L,
                    state: vitrin_protocol::generated::vitrin_shim_seat::KeyState::Pressed,
                }),
            )],
        );
        assert_eq!(
            rig.host
                .runtime
                .router
                .held_keys(&RealmId::new("browser"))
                .len(),
            1,
            "the press was delivered into the realm the grant named, so its release is \
             owed there"
        );
        assert!(
            rig.host
                .runtime
                .router
                .held_keys(&RealmId::new("realm-0"))
                .is_empty(),
            "and nothing was recorded against the sibling: an actuation is addressed by \
             its grant's realm, never by whichever realm the session happens to serve"
        );

        // The *other* realm dies, through the real funnel: a real kill, the
        // socketpair EOF the loop dispatches, `close_realm`, the death latch,
        // `ShimServer::connection_closed`.
        assert_eq!(
            unsafe { libc::kill(sibling as libc::pid_t, libc::SIGKILL) },
            0,
            "the sibling must be signalable"
        );
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            !host
                .runtime
                .kernel
                .realms
                .get("realm-0")
                .expect("realm-0 is registered")
                .admits_petitions()
        });
        reap_realm(&mut rig.host);

        assert_eq!(
            rig.host
                .runtime
                .router
                .held_keys(&RealmId::new("browser"))
                .len(),
            1,
            "a sibling's death must not forget what the survivor's app is holding -- a \
             session-wide reset here latches the key down in a live app forever"
        );

        // The proof at the app: the release still pairs, so it is still
        // delivered rather than dropped as unpaired.
        route_seat(
            &mut rig.host,
            vec![(
                RealmId::new("browser"),
                SeatInput::emulated(crate::input::SeatInputKind::Key {
                    source: crate::input::KeySource::Keysym,
                    keysym: SHIFT_L,
                    state: vitrin_protocol::generated::vitrin_shim_seat::KeyState::Released,
                }),
            )],
        );
        assert!(
            rig.host
                .runtime
                .router
                .held_keys(&RealmId::new("browser"))
                .is_empty(),
            "the release paired with the press"
        );

        shutdown_realm(&mut rig.host);
        let entries = rig.entries();
        assert_eq!(
            crate::recorder::tests::of_kind(&entries, "seat_delivered").len(),
            2,
            "both the press and its release reached the survivor's seat"
        );
    }

    /// Issue #83: an event the core delivers to the shim's seat is journaled
    /// with the origin intake bound (backward requirement B2). Until this
    /// landed the flight recorder wrote nothing about seat delivery, so the
    /// unforgeable physical-vs-agent tag was unauditable after the fact.
    #[test]
    fn a_delivered_seat_event_is_journaled_with_its_origin() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "seat-audit",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        // `--seat`: the shim mints its seat, so a routed event lands rather
        // than dropping. Without a minted seat `deliver_seat_event` returns
        // `Ok(false)` and nothing is journaled — the negative half of the
        // contract, which is why the record sits behind `if sent`.
        rig.start_realm(&["--serve", "--seat"]);
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            host.runtime
                .realms
                .values()
                .filter_map(|r| r.server.as_ref())
                .any(|s| s.seat_minted())
        });

        // Two agent-originated events through the production route. Text never
        // needs a committed surface to route, so the seat mint is the only
        // precondition; the origin travels unrewritten from `emulated` to the
        // journal.
        route_seat(
            &mut rig.host,
            vec![
                (
                    RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
                    SeatInput::emulated(crate::input::SeatInputKind::Text { text: "hi".into() }),
                ),
                (
                    RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
                    SeatInput::emulated(crate::input::SeatInputKind::Text {
                        text: "there".into(),
                    }),
                ),
            ],
        );
        shutdown_realm(&mut rig.host);

        let entries = rig.entries();
        let delivered = crate::recorder::tests::of_kind(&entries, "seat_delivered");
        assert_eq!(
            delivered.len(),
            2,
            "both delivered events must be journaled; got {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );
        for e in delivered {
            assert_eq!(e.str("event"), "text");
            assert_eq!(e.str("origin"), "emulated", "the tag intake bound (B2)");
        }
        // The typed strings never reach the log: shape only, never a keylogger.
        let raw = std::fs::read_to_string(&rig.log).unwrap();
        assert!(
            !raw.contains("there"),
            "a delivery entry must not carry the typed text"
        );
    }

    /// `pstree` in miniature, and the criterion it discharges: a spawned
    /// realm really is `core -> shim -> app`, read out of `/proc` rather than
    /// out of a description.
    ///
    /// `/proc` rather than shelling out to `pstree`, which is not guaranteed
    /// to be installed and whose output is not a stable contract.
    #[test]
    fn a_spawned_realm_is_a_real_process_tree_and_the_app_holds_no_core_socket() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "pstree",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        // `--spawn-app` makes the third tier real rather than described.
        rig.start_realm(&["--serve", "--spawn-app"]);
        let core = std::process::id();
        let shim = rig.realm(crate::realm::WELL_KNOWN_REALM_ID).life.pid();

        // Let the shim come up and fork its app.
        let app = {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                rig.pump(Duration::from_millis(50));
                if let Some(app) = children_of(shim).first().copied() {
                    break app;
                }
                assert!(
                    Instant::now() < deadline,
                    "the shim never forked an app; children_of({shim}) stayed empty"
                );
            }
        };

        assert_eq!(parent_of(shim), Some(core), "the shim's parent is the core");
        assert_eq!(parent_of(app), Some(shim), "the app's parent is the shim");

        // The counterpart of `spawn`'s no-inherit test, against the wired
        // path: the app must not hold a descriptor onto the core.
        let core_inode = socket_inodes_of(shim);
        let app_inode = socket_inodes_of(app);
        assert!(
            core_inode.is_disjoint(&app_inode),
            "the app inherited a socket the shim holds to the core: {:?}",
            core_inode.intersection(&app_inode).collect::<Vec<_>>()
        );

        shutdown_realm(&mut rig.host);
        assert!(!process_is_alive(shim), "the shim must be reaped");
    }

    /// `/proc/<pid>/status`' `PPid`.
    fn parent_of(pid: u32) -> Option<u32> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))?
            .trim()
            .parse()
            .ok()
    }

    /// Direct children, via the kernel's own `children` file.
    fn children_of(pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return out;
        };
        for task in tasks.flatten() {
            if let Ok(children) = std::fs::read_to_string(task.path().join("children")) {
                out.extend(
                    children
                        .split_whitespace()
                        .filter_map(|p| p.parse::<u32>().ok()),
                );
            }
        }
        out
    }

    /// Every socket inode this process holds a descriptor on, from
    /// `/proc/<pid>/fd`. Two processes sharing a socketpair share an inode
    /// number, so set intersection answers "did this fd cross the fork".
    fn socket_inodes_of(pid: u32) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return out;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                let target = target.to_string_lossy().into_owned();
                if target.starts_with("socket:") {
                    out.insert(target);
                }
            }
        }
        out
    }

    /// Whether `pid` still exists. Reaped children vanish from `/proc`.
    fn process_is_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// One process's environment as `NAME=value` strings, from
    /// `/proc/<pid>/environ` (NUL-separated). Read from the kernel rather
    /// than reconstructed, so what is asserted is what the child got.
    fn environ_of(pid: u32) -> Vec<String> {
        let raw = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        raw.split(|b| *b == 0)
            .filter(|kv| !kv.is_empty())
            .map(|kv| String::from_utf8_lossy(kv).into_owned())
            .collect()
    }

    /// A dispatch round costs at most one composite, however dirty it got.
    ///
    /// This is the trap `backend::headless`'s TEST-ONLY warning names: a
    /// runtime that composited per latched commit would sell a hostile shim a
    /// full-output composite for a 12-byte repaint message. The runtime marks
    /// the scene dirty and lets [`post_dispatch`] do the one redraw.
    #[test]
    fn a_dispatch_round_costs_at_most_one_composite() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "coalesce",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        // Whatever set it, and however many times.
        for _ in 0..100 {
            rig.host.runtime.dirty = true;
        }
        post_dispatch(&mut rig.host);
        assert_eq!(rig.host.view.redraws, 1);
        // And a clean round composites nothing at all, so an idle core with
        // an idle shim burns no CPU.
        post_dispatch(&mut rig.host);
        assert_eq!(rig.host.view.redraws, 1);
    }

    /// **A scheduled composite is not a completed one, and must not
    /// discharge the realm's frame callbacks.**
    ///
    /// The nested backend does not own its frame clock: `Presenter::redraw`
    /// composites nothing and answers [`Presentation::Scheduled`], while the
    /// host compositor draws later and `NestedState::redraw` then calls
    /// [`emit_presented`]. If `post_dispatch` emitted `frame_done` at
    /// *request* time instead, a shim that paces itself on the callback —
    /// which is what the callback is for — would collect a fresh permit every
    /// dispatch round with no composite in between, and stop throttling.
    ///
    /// The bug this pins is silent rather than loud: nothing errors, the
    /// frames still arrive, and the only symptom is a nested session where a
    /// paced client spins as fast as the loop will dispatch it. Asserted on
    /// the owed-callback count, because that is the observable the shim's
    /// pacing actually reads.
    #[test]
    fn a_scheduled_composite_does_not_discharge_the_frame_callbacks() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "pacing",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        // The deferred posture goes on *before* the loop runs: with it set,
        // no amount of pumping may ever discharge a callback, which is the
        // whole claim. (Set afterwards, `post_dispatch` would have paid them
        // during the pump and the assertion below would race.)
        rig.host.view.posture = Presentation::Scheduled;
        rig.start_realm(&["--serve", "--animate", "1"]);
        rig.pump_until(Duration::from_secs(10), |host: &TestHost| {
            host.runtime
                .realms
                .values()
                .filter_map(|realm| realm.server.as_ref())
                .any(|server| server.wants_presentation())
        });

        assert!(
            rig.host.view.redraws > 0,
            "the rounds still cost their composite requests"
        );
        assert!(
            rig.realm(crate::realm::WELL_KNOWN_REALM_ID)
                .server
                .as_ref()
                .is_some_and(|server| server.wants_presentation()),
            "a scheduled composite leaves the frame callbacks owed -- emitting them here \
             is the silent pacing bug: the shim would be told a frame it never saw was presented"
        );

        // And the debt is discharged the moment a composite really lands,
        // which is the other half of the contract: the callbacks are delayed,
        // never dropped.
        emit_presented(&mut rig.host.runtime);
        assert!(
            !rig.realm(crate::realm::WELL_KNOWN_REALM_ID)
                .server
                .as_ref()
                .is_some_and(|server| server.wants_presentation()),
            "a real composite must pay every owed callback"
        );
    }

    /// **A realm's death reaches the compositor, so its last frame leaves
    /// the human's screen.**
    ///
    /// [`close_realm`] takes the dead realm's surface out of the scene, so
    /// the *next* composite shows an empty view — but only if a next
    /// composite happens. Marking the frame dirty is not enough on a backend
    /// whose frame clock is external: [`post_dispatch`] consumes the flag by
    /// calling [`Presenter::redraw`], which in the nested posture answers
    /// [`Presentation::Scheduled`] and deliberately composites nothing. The
    /// call that actually asks the host compositor for a frame is
    /// [`Presenter::request_present`], and `close_realm` did not make it —
    /// so a killed app's last painted frame stayed on screen until an
    /// unrelated resize, focus change or petition happened along, looking
    /// for all the world like a live window.
    ///
    /// Asserted in the nested posture, because that is the one where the two
    /// calls differ; headless inherits `request_present` as a no-op and was
    /// never affected, which is exactly why the gap was invisible.
    #[test]
    fn a_dead_realm_asks_the_backend_for_the_frame_that_clears_it() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "close-present",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        rig.host.view.posture = Presentation::Scheduled;
        rig.start_realm(&["--serve", "--animate", "100000"]);
        // Let the shim commit at least one frame, so there really are dead
        // pixels for the teardown to clear rather than an empty scene.
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        rig.pump_until(Duration::from_secs(10), |host| {
            host.view.surface_of(&realm).is_some()
        });
        assert!(
            rig.host.view.surface_of(&realm).is_some(),
            "fixture check: the realm painted something before it died"
        );

        rig.host.runtime.dirty = false;
        let presents_before = rig.host.view.presents;
        close_realm(
            &mut rig.host,
            &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
            DeathCause::ConnectionClosed,
        );

        assert!(
            rig.host.view.surface_of(&realm).is_none(),
            "fixture check: the death funnel takes the surface out of the scene"
        );
        assert!(
            rig.host.runtime.dirty,
            "realm death must mark the frame dirty"
        );
        // At least one, not exactly one. Two paths ask for the frame on this
        // fixture and both are wanted: `close_realm` asks because the dead
        // realm's surface left the scene, and `rebind_output_after_death` asks
        // because the output was bound to that realm and is not any more (here
        // there is no sibling, so it unbinds). A duplicate `request_redraw` is
        // coalesced by the host compositor into one frame; a *missing* one is
        // the defect this test is about.
        assert!(
            rig.host.view.presents > presents_before,
            "realm death never reached the compositor: on the nested backend the dead \
             app's last frame stays on the human's screen until something unrelated \
             happens to ask for a redraw"
        );
    }

    /// **Shutdown is the fourth close path, and it tears down too.**
    ///
    /// `close_principal` covers the three a running loop sees; a core stopped
    /// by `SIGTERM` with an agent still connected is the fourth, and it used
    /// to drop the connection table silently. The grant table dies with the
    /// process either way, so nothing survived to be misused — but the flight
    /// recorder is what has to reconstruct the session afterwards, and a log
    /// whose last word on a grant is `petition_resolved` cannot say whether
    /// the core released it or simply lost track of it.
    #[test]
    fn shutdown_tears_down_a_connection_that_still_holds_a_grant() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "shutdown-teardown",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut client), Outcome::Granted);

        let now = Instant::now();
        assert_eq!(rig.host.runtime.kernel.grants.rows(now).count(), 1);

        // The agent never disconnects: the core stops underneath it, which is
        // what `SIGTERM` on a live session does.
        rig.host.runtime.teardown_open_connections();

        assert_eq!(
            rig.host.runtime.connection_count(),
            0,
            "shutdown must forget the connection"
        );
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(now).count(),
            0,
            "shutdown must remove the grant rows, not drop the table and leave the log \
             claiming the grant was still live"
        );

        let entries = rig.entries();
        assert!(
            entries
                .iter()
                .any(|entry| entry.str("kind") == "grant_removed"),
            "the run's log must record the grant's end; without it a reader cannot tell a \
             released grant from one the core lost track of. entries: {entries:#?}"
        );
        drop(client);
    }

    // ------------------------------------------------------------------
    // Interactive consent, end to end (issue #90)
    // ------------------------------------------------------------------
    //
    // These drive the SHIPPED `post_dispatch` → `service_consent` path with a
    // real grab attached, so the orchestration `service_consent_round` owns —
    // raise the front petition, notify the petitioner, drain a human decision
    // into `resolve_human`, deliver the resolution, lower the card — runs over
    // the real wire to a real agent. The click-to-decision half is covered
    // exhaustively by `consent::grab`'s own tests; here a decision is injected
    // through the grab's test seam, exactly where a physical click would have
    // deposited one.

    /// Pump until the attached grab has a prompt up, or fail.
    fn pump_until_armed(
        rig: &mut Rig,
        grab: &Rc<RefCell<ConsentGrab>>,
    ) -> crate::petitions::PetitionId {
        rig.pump_until(Duration::from_secs(5), |_| {
            grab.borrow().armed_petition().is_some()
        });
        grab.borrow().armed_petition().expect("a prompt is up")
    }

    /// **Acceptance: an interactive petition puts a prompt on screen and tells
    /// the agent.** Nothing here reaches `raise` directly — the running loop's
    /// `service_consent` does, which is the wiring issue #90 was filed for.
    #[test]
    fn an_interactive_petition_raises_a_prompt_and_notifies_the_agent() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-raise",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);

        let petition = pump_until_armed(&mut rig, &grab);

        // The grab holds this petition's prompt, and the surface really has a
        // card to draw (its origin resolves only when a prompt is up).
        assert_eq!(grab.borrow().armed_petition(), Some(petition));
        assert!(
            rig.host.view.consent.card_origin(VIEW.0, VIEW.1).is_some(),
            "raising a prompt must put a card on the consent surface"
        );

        // `shown` rides the outbox (an out-of-dispatch send), so it is only
        // written once the connection source dispatches again after the raise.
        // `pump_until_armed` stops the instant the prompt is up, before that
        // flush — so pump once more to put the frame on the wire, else the
        // blocking read below would wait for a frame still sitting in the
        // outbox.
        rig.pump(Duration::from_millis(200));

        // The petitioner heard about it over the wire: `service_consent`
        // called `emit_consent_shown`, which pushed `vitrin_consent.state`
        // through the outbox. Read past the `queued` the admission already
        // sent and confirm the `shown` this task wired lands too.
        use vitrin_protocol::generated::vitrin_consent::{events::State, ConsentState};
        let mut saw_shown = false;
        for _ in 0..32 {
            let msg = client
                .recv_message()
                .expect("client receive")
                .expect("the core must not have hung up");
            if msg.header.object_id == 5 && msg.header.opcode == State::OPCODE {
                let (_, event) = State::decode(&msg.bytes, msg.fd).expect("decode consent state");
                if event.state == ConsentState::Shown {
                    saw_shown = true;
                    break;
                }
            }
        }
        assert!(
            saw_shown,
            "the agent must receive vitrin_consent.state(shown)"
        );

        // ...and the flight recorder — the source of truth for "a human was
        // asked", written by `raise` itself — carries the transition.
        let entries = rig.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.str("kind") == "consent_transition" && e.str("state") == "shown"),
            "the run must journal a consent_transition{{shown}}; got {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );
    }

    /// **No prompt is raised onto a screen this session no longer owns, and
    /// the flight recorder does not say one was** (WS-E.3.3, D-030(4)).
    ///
    /// The falsehood this closes was live on this branch before it landed.
    /// `post_dispatch` calls `service_consent` unconditionally and the calloop
    /// loop keeps dispatching through a seat pause, so an agent petitioning
    /// while the human was on another VT got a card composited into a frame
    /// `should_queue_flip` would never flip — while `raise` wrote
    /// `consent_transition{shown}`, set `prompt_shown` so the chokepoint
    /// refused that principal `consent_held`, and told the petitioner `shown`
    /// over the wire. Authority still failed closed (the sweep times the
    /// petition out); the **record** was the lie, and the record is the one
    /// artifact that has to reconstruct the session afterwards.
    ///
    /// Both directions in one test, deliberately. The negative half alone
    /// passes in a run where the consent machinery is simply broken, so it is
    /// paired with the same petition being raised the moment the seat returns
    /// — no new petition, no reconnect, only the visibility answer changing.
    #[test]
    fn no_prompt_is_raised_while_the_screen_is_not_ours() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-paused",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        // The seat took the devices away before the petition arrived.
        rig.host.screen = PromptVisibility::ScreenNotOurs;

        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(400));

        // The petition really did arrive and really is pending — otherwise
        // everything below is vacuous.
        let front = rig
            .host
            .runtime
            .kernel
            .petitions
            .front_pending()
            .expect("the petition must be queued and awaiting a human");
        assert!(
            grab.borrow().armed_petition().is_none(),
            "a card was raised while the seat held this session's devices: nothing composited \
             this round reaches a panel, so no human could have seen it"
        );
        assert!(
            rig.host.view.consent.card_origin(VIEW.0, VIEW.1).is_none(),
            "the consent surface must have nothing on it while the screen is not ours"
        );
        let petitioner =
            PrincipalIdentity::parse(DEMO_IDENTITY).expect("the rig's demo identity parses");
        assert!(
            !rig.host.runtime.kernel.petitions.prompt_up_for(&petitioner),
            "`prompt_shown` was set for a card that reached no panel, so the enforcement \
             chokepoint would refuse this principal `consent_held` citing it"
        );
        let entries = rig.entries();
        assert!(
            !entries
                .iter()
                .any(|e| e.str("kind") == "consent_transition" && e.str("state") == "shown"),
            "the flight recorder journalled `shown` for a prompt that never reached a display; \
             got {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );

        // The seat gives the devices back. Same petition, same connection —
        // only the visibility answer changed.
        rig.host.screen = PromptVisibility::Reachable;
        let petition = pump_until_armed(&mut rig, &grab);
        assert_eq!(
            petition, front,
            "the petition that waited is the one raised"
        );
        assert!(
            rig.host.view.consent.card_origin(VIEW.0, VIEW.1).is_some(),
            "the card must go up the moment the screen is ours again"
        );
        let entries = rig.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.str("kind") == "consent_transition" && e.str("state") == "shown"),
            "and only then may the run journal consent_transition{{shown}}; got {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );
    }

    /// **The idle blank's whole state machine, driven through the real
    /// dispatch round against the rig's presenter** (WS-E.4.3, issue #223).
    ///
    /// This is the half of #223 CI can carry, and the criterion the issue
    /// states in as many words: the timer, the cover and the transitions, on the
    /// existing headless/synthetic pattern. What it deliberately does **not**
    /// touch is `DrmSurface::clear` — no runner has a display controller, and
    /// `DrmState` cannot be constructed without a real `DrmDevice`,
    /// `LibSeatSession`, `GbmDevice` and `GlesRenderer`. The display-power call
    /// itself is held by a source assertion in `backend::drm`, exactly as
    /// D-030's three gates are, and by nothing else until the owner runs the
    /// hardware checklist.
    ///
    /// Every transition is asserted through [`post_dispatch`] rather than by
    /// poking the state machine, because the thing that would silently break is
    /// the *wiring*: a `service_screen` that stopped being called leaves
    /// `blank.rs`'s own unit tests entirely green.
    #[test]
    fn the_idle_blank_covers_the_output_and_a_human_takes_it_back() {
        use crate::backend::blank::Phase;

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "idle-blank",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let activity = rig.attach_blank(Duration::from_secs(300));
        let t0 = activity.borrow().last_activity();

        // A round before the deadline changes nothing, which is the control:
        // without it every assertion below would pass against a cover that was
        // always up.
        rig.pump(Duration::from_millis(50));
        assert_eq!(activity.borrow().phase(), Phase::Lit);
        let lit = rig.host.view.human_visible();

        // The deadline passes. The rig's clock is the real one, so the round is
        // driven with a stamp far enough back to be idle -- the same seam
        // `LockScreen`'s own tests use.
        activity.borrow_mut().tick(t0 + Duration::from_secs(300));
        rig.pump(Duration::from_millis(50));
        assert_eq!(
            activity.borrow().phase(),
            Phase::Covering,
            "the round must raise the cover before anything powers a panel down"
        );

        // ...and the cover really is in the frame the human would be shown,
        // through the same `compose_human_visible` both shipped backends use.
        let covered = rig.host.view.human_visible();
        assert_ne!(
            lit, covered,
            "a covered frame must differ from a lit one, or the cover reached no composite"
        );
        let band = (VIEW.0 as usize) * (crate::consent::TRUST_BAND_HEIGHT as usize) * 4;
        assert!(
            covered[band..]
                .chunks_exact(4)
                .all(|px| px == [0x00, 0x00, 0x00, 0xff]),
            "every row below the trusted band must be the cover"
        );

        // The panel goes dark once the cover's own flip lands, and only then.
        assert!(!activity.borrow().dpms_owed());
        activity.borrow_mut().note_frame_queued();
        assert!(activity.borrow().dpms_owed());
        activity.borrow_mut().went_dark();
        assert!(
            rig.host.view.output_gates().dark,
            "a dark output must be visible to the pointer-constraint reconciler through the \
             SAME field the composite reads, or a locked pointer survives a blank the human \
             can neither see nor free"
        );

        // The human comes back. Any physical event does it; here the state
        // machine is driven directly, because what this test is about is the
        // round's reaction rather than the gate's verdict
        // (`lock::gate::tests::every_physical_event_postpones_the_blank_and_wakes_a_dark_screen`
        // holds that half against the real router stack).
        assert!(activity
            .borrow_mut()
            .note_physical(Instant::now())
            .consumes());
        rig.pump(Duration::from_millis(50));
        assert_eq!(activity.borrow().phase(), Phase::Waking);
        assert_eq!(
            rig.host.view.human_visible(),
            lit,
            "the cover must come down on the wake, or the frame that re-enables the display \
             is the black one"
        );
        assert!(!rig.host.view.output_gates().dark);

        // ...and the wake ends at the first completed flip, not before.
        assert!(activity.borrow_mut().note_flip_completed());
        rig.pump(Duration::from_millis(50));
        assert_eq!(activity.borrow().phase(), Phase::Lit);
    }

    /// **Coming back from another VT is activity: neither the panel nor the
    /// lock may fire on the round after the human returns** (issue #257).
    ///
    /// The defect this pins is not a timer bug. A paused session never sees the
    /// input that reactivates it — the chord that switches the VT back is
    /// delivered to whichever session is *currently* active — so the activate
    /// arm had nothing but `self.now`, the input turn's clock cell, which for a
    /// returning session still held an instant from before the absence. Resetting
    /// the countdown to an already-expired instant is worse than not resetting
    /// it: with `--blank-idle 60` the L4 rung measured the panel going dark
    /// **1.5 s** after the human came back.
    ///
    /// **Both timers are asserted, because both read the one clock**, and the
    /// lock's is the stronger claim: a panel that blanks on return is an
    /// annoyance, while a session that demands a passphrase because somebody
    /// came back to their own screen is the idle lock firing at exactly the
    /// moment it should not. The answer is therefore the *same* for the two,
    /// and this is where "the same" is checked rather than asserted in prose.
    ///
    /// Driven through [`note_seat_presence`], which is the production body of
    /// both bare-metal seat arms: `DrmState` needs a real `DrmDevice`,
    /// `LibSeatSession`, `GbmDevice` and `GlesRenderer`, so anything left inside
    /// `handle_session_event` is unreachable by every test in this workspace.
    #[test]
    fn returning_from_another_vt_restarts_both_idle_countdowns() {
        use crate::backend::blank::Phase;

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "seat-return-idle",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let idle = Duration::from_secs(60);
        // The clock as a returning session finds it: last touched long before
        // the human left, because nothing they did on the other VT reached here.
        let stale = Instant::now() - Duration::from_secs(600);

        // **The vacuity control.** Without it every assertion below would pass
        // against a rig that simply never blanks. Same clock age, no seat event:
        // the very next round covers the output.
        let control = rig.attach_blank_seeded(idle, stale);
        rig.pump(Duration::from_millis(50));
        assert_eq!(
            control.borrow().phase(),
            Phase::Covering,
            "a clock this old must blank on the next round, or this test proves nothing"
        );

        // Now the same clock age, reached the way a returning human reaches it.
        let activity = rig.attach_blank_seeded(idle, stale);
        let lock = RefCell::new(crate::lock::LockScreen::new(
            crate::chord::ModChord::parse(crate::lock::DEFAULT_LOCK_CHORD)
                .expect("the default lock chord parses"),
            Some(idle),
            None,
            Rc::clone(&activity),
        ));

        note_seat_presence(&lock, true);
        assert!(
            activity.borrow().seat_absent(),
            "the pause must reach the shared activity record"
        );
        rig.pump(Duration::from_millis(50));
        assert_eq!(
            activity.borrow().phase(),
            Phase::Lit,
            "time on another VT is not idle time, and a paused session must hold no blank it \
             cannot undo"
        );
        assert!(
            !lock.borrow_mut().tick(Instant::now()),
            "and the lock must not raise itself on a VT nobody is looking at"
        );

        // The seat gives the devices back. This is the whole fix: the instant
        // stamped here comes from `note_seat_presence` itself, not from any
        // cell a paused session could not have refreshed.
        note_seat_presence(&lock, false);
        assert!(!activity.borrow().seat_absent());

        rig.pump(Duration::from_millis(50));
        assert_eq!(
            activity.borrow().phase(),
            Phase::Lit,
            "#257: the panel blanked on the round after the human came back. The idle clock \
             was reset to an instant from before the absence, which is already past the \
             threshold"
        );
        assert!(
            !rig.host.view.blank.is_covering(),
            "and no cover reached the composite either"
        );
        assert!(
            !lock.borrow_mut().tick(Instant::now()),
            "#257, the stronger half: the idle lock armed on return, so the human is asked for \
             a passphrase for the offence of coming back to their own screen"
        );
        assert!(lock.borrow().cause().is_none());

        // ...and the countdown really did restart rather than being disabled:
        // an idle session still blanks, measured from the return.
        assert!(
            activity
                .borrow_mut()
                .tick(Instant::now() + idle + Duration::from_secs(1)),
            "the fix must restart the countdown, not switch it off"
        );
    }

    /// **A wake says so, a failed wake says something else, and both reach the
    /// flight recorder** (issues #258 and #259).
    ///
    /// Before this, a successful unblank and a modeset that left the panel dark
    /// produced identical output: none. That is not an ordinary missing log
    /// line — `docs/book/src/recovery.md` names the second one the worst
    /// credible outcome of its L4 rung and says in as many words that it is
    /// indistinguishable from a wedge, so the only instrument that could tell
    /// them apart was a human looking at the panel.
    ///
    /// Driven through the rig's real dispatch round for the success path, so
    /// what is asserted is the **wiring** and not a function called by hand; the
    /// failure path is driven by calling the same round function with a
    /// synthetic clock, because [`crate::backend::blank::WAKE_DEADLINE`] is two
    /// real seconds and no test may sit through one to learn something the
    /// injected instant already says.
    #[test]
    fn a_wake_and_a_failed_wake_are_logged_and_journalled_differently() {
        use crate::backend::blank::{BlankSurface, SessionActivity, WAKE_DEADLINE};

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "blank-observability",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let activity = rig.attach_blank(Duration::from_secs(300));
        let t0 = activity.borrow().last_activity();

        // --- the blank, and the entry it owes ------------------------------
        let log = LogCapture::install();
        activity.borrow_mut().tick(t0 + Duration::from_secs(300));
        rig.pump(Duration::from_millis(50));

        // The panel powers down (the bare-metal half), the human presses a key,
        // and the flip that re-enables the CRTC lands.
        activity.borrow_mut().note_frame_queued();
        activity.borrow_mut().went_dark();
        assert!(activity
            .borrow_mut()
            .note_physical(Instant::now())
            .consumes());
        assert!(
            !log.contains("the panel is lit again"),
            "a press that starts a wake is not a wake: logging here would make a successful \
             unblank and a failed one produce the same line, which is the defect one rung down \
             from the silence"
        );
        assert!(activity.borrow_mut().note_flip_completed());
        rig.pump(Duration::from_millis(50));

        let lines = log.take();
        let wake = lines
            .iter()
            .find(|(_, message)| message.contains("the panel is lit again"))
            .expect("#258: a successful wake must emit a line -- silence is what shipped");
        assert_eq!(
            wake.0,
            tracing::Level::INFO,
            "the wake belongs at the blank's own level"
        );
        assert!(
            wake.1.contains("was never locked") && wake.1.contains("live throughout"),
            "the wake line must not imply anything the wake does not restore: the session was \
             never locked, so this is no evidence of WHO woke it, and the grants were live the \
             whole time rather than restored by it. Got: {}",
            wake.1
        );

        let entries = rig.entries();
        let blanked = crate::recorder::tests::of_kind(&entries, "screen_blanked");
        let woke = crate::recorder::tests::of_kind(&entries, "screen_woke");
        assert_eq!(
            blanked.len(),
            1,
            "#259: the panel going dark left no flight-recorder entry; got kinds {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );
        assert_eq!(woke.len(), 1, "#259: neither did the wake");
        assert!(
            !blanked[0].bool("locked"),
            "the entry must say the blank did NOT lock (D-033), or a reader infers it from an \
             absence"
        );
        assert_eq!(woke[0].str("outcome"), "flip_landed");
        // Present-and-numeric rather than a value: the rig runs on the real
        // clock, so the only honest claim about the span is that it is recorded.
        woke[0].u64("dark_ms");
        for entry in blanked.iter().chain(woke.iter()) {
            assert_eq!(
                entry.u64("live_grants"),
                0,
                "both entries must carry whether any grant was live across the dark window, \
                 and this session minted none"
            );
            // `SessionLocked`'s shape: a session-level fact names no principal
            // and no realm, because no wire message can blank this panel.
            for absent in ["principal", "realm", "grant"] {
                assert!(
                    entry.path(absent).is_none(),
                    "a session-level entry must carry no `{absent}` field"
                );
            }
        }

        // --- the failed wake, on a synthetic clock -------------------------
        let mut activity = SessionActivity::new(Some(Duration::from_secs(60)), t0);
        let mut surface = BlankSurface::for_test();
        let log = LogCapture::install();

        service_blank_round(
            &mut rig.host.runtime,
            &mut activity,
            &mut surface,
            t0 + Duration::from_secs(60),
        );
        assert!(activity
            .note_physical(t0 + Duration::from_secs(90))
            .consumes());
        // The deadline passes with no flip behind it: the modeset that would
        // light the panel was never confirmed.
        service_blank_round(
            &mut rig.host.runtime,
            &mut activity,
            &mut surface,
            t0 + Duration::from_secs(90) + WAKE_DEADLINE,
        );

        let lines = log.take();
        let failed = lines
            .iter()
            .find(|(_, message)| message.contains("THE WAKE WAS NOT CONFIRMED"))
            .expect("#258: a failed wake must be distinguishable from a successful one");
        assert_eq!(
            failed.0,
            tracing::Level::WARN,
            "a modeset that may have left the panel dark is the exact case worth a WARN"
        );
        assert!(
            !lines
                .iter()
                .any(|(_, message)| message.contains("the panel is lit again")),
            "a failed wake must never emit the success line"
        );

        let entries = rig.entries();
        let woke = crate::recorder::tests::of_kind(&entries, "screen_woke");
        assert_eq!(woke.len(), 2, "the abandoned wake owes an entry too");
        assert_eq!(
            woke[1].str("outcome"),
            "no_flip",
            "#259: an abandoned wake journalled as `flip_landed` would be the recorder claiming \
             the human got their screen back on the one path where they may not have"
        );
    }

    /// **A consent prompt cannot be resolved across a blank** (WS-E.4.3, issue
    /// #223's named acceptance criterion; D-030(4)'s deferred dark-output gate,
    /// discharged).
    ///
    /// Two independent failures, and both have to be closed or the criterion is
    /// only half met:
    ///
    /// * **A card must not be raised onto a dark panel at all.** `raise` writes
    ///   `consent_transition{shown}` — the flight recorder's record that *a
    ///   human was asked* — and sets `prompt_shown`, so the enforcement
    ///   chokepoint starts refusing that principal `consent_held` citing a card
    ///   nobody can see. `no_prompt_is_raised_while_the_screen_is_not_ours`
    ///   holds the identical property for a seat pause; this is the same
    ///   falsehood reachable from a *timer* rather than from a human's chord,
    ///   which makes it routine rather than occasional.
    /// * **A press armed before the blank must not commit after it.** The human
    ///   left with the pointer over Allow, the panel went dark, and `commit`
    ///   re-checks only the last *physical* pointer position — which going dark
    ///   does not reset. Without the guard restart the first release after they
    ///   come back grants authority decided against a card that spent its whole
    ///   guard interval on a screen that was off.
    ///
    /// The positive half is in the same test on purpose: the negative alone
    /// passes in a run where the consent machinery is simply broken, so the same
    /// petition — no reconnect, no second petition — is raised the moment the
    /// screen comes back.
    #[test]
    fn a_consent_prompt_cannot_be_resolved_across_a_blank() {
        use crate::backend::blank::Phase;

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-blank",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let activity = rig.attach_blank(Duration::from_secs(300));
        let t0 = activity.borrow().last_activity();

        // The screen is already dark when the petition arrives.
        {
            let mut a = activity.borrow_mut();
            assert!(a.tick(t0 + Duration::from_secs(300)));
            a.note_frame_queued();
            a.went_dark();
            assert_eq!(a.phase(), Phase::Dark);
        }

        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(400));

        // The petition really did arrive and really is pending -- otherwise
        // everything below is vacuous.
        let front = rig
            .host
            .runtime
            .kernel
            .petitions
            .front_pending()
            .expect("the petition must be queued and awaiting a human");
        assert!(
            grab.borrow().armed_petition().is_none(),
            "a card was raised onto a panel this session had powered down: no human could \
             have seen it, and the record would say one was asked"
        );
        assert!(
            rig.host.view.consent.card_origin(VIEW.0, VIEW.1).is_none(),
            "the consent surface must have nothing on it while the screen is dark"
        );
        let petitioner =
            PrincipalIdentity::parse(DEMO_IDENTITY).expect("the rig's demo identity parses");
        assert!(
            !rig.host.runtime.kernel.petitions.prompt_up_for(&petitioner),
            "`prompt_shown` was set for a card on a dark screen, so the enforcement chokepoint \
             would refuse this principal `consent_held` citing it"
        );
        let entries = rig.entries();
        assert!(
            !entries
                .iter()
                .any(|e| e.str("kind") == "consent_transition" && e.str("state") == "shown"),
            "the flight recorder journalled `shown` for a prompt on a screen that was off; \
             got {:?}",
            entries.iter().map(|e| e.str("kind")).collect::<Vec<_>>()
        );

        // The human touches something. The cover comes down, the panel comes
        // back, and the SAME petition is raised -- no reconnect, no second
        // petition, only the visibility answer changing.
        activity.borrow_mut().note_physical(Instant::now());
        activity.borrow_mut().note_flip_completed();
        let petition = pump_until_armed(&mut rig, &grab);
        assert_eq!(
            petition, front,
            "the petition that waited out the blank is the one raised"
        );
        assert!(
            rig.host.view.consent.card_origin(VIEW.0, VIEW.1).is_some(),
            "the card must go up the moment the panel is lit again"
        );

        // The guard half of "cannot be resolved across a blank" is held one
        // module over, against the real grab and a real armed press:
        // `consent::grab::tests::a_prompt_that_spanned_an_idle_blank_gets_a_fresh_guard_and_loses_its_armed_press`.
        // It is there rather than here because the property is about
        // `ConsentGrab`'s own judgement of a press, and this rig has no pointer
        // to press with -- but it is the same `screen_became_visible` call the
        // wake makes on bare metal, so the two halves meet.
    }

    /// **A seat pause pays the app every press the human is holding — keys
    /// and buttons — and cancels the dead-man gesture in progress**
    /// (WS-E.3.3, D-030(8); criteria (a) and (b) of issue #219).
    ///
    /// Driven through [`suspend_physical_seat`], which is the production body
    /// of the bare-metal `PauseSession` arm. It is a free function for exactly
    /// this reason: `DrmState` needs a real `DrmDevice`, `LibSeatSession`,
    /// `GbmDevice` and `GlesRenderer`, so anything left inside its
    /// `handle_session_event` is unreachable by every test in this workspace
    /// — and a pause handler that silently stopped draining would take nothing
    /// red with it.
    ///
    /// The presses are made by real physical events through
    /// [`route_physical_turn`], so the router's tables are populated by the
    /// production intake rather than by the test: a test that pushed its own
    /// entries into the pairing table would be asserting about a fixture.
    ///
    /// **Buttons are the half the nested precedent does not pay.**
    /// `handle_focus` drains keys only, because winit keeps sending pointer
    /// events and a synthetic release would end a live drag. A seat pause has
    /// closed the devices, so there is no drag left to end and a held button
    /// wedges the app's implicit pointer grab with nothing that can ever pay
    /// it down.
    #[test]
    fn a_paused_seat_pays_out_held_presses_and_forgets_the_dead_man_hold() {
        use crate::input::tests::physical_for_test;
        use crate::input::SeatInputKind;
        use vitrin_protocol::generated::vitrin_actuator_pointer::ButtonState;
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "seat-pause-drain",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        rig.start_realm(&["--serve", "--seat"]);
        rig.pump(Duration::from_millis(400));
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        // A committed surface, or a button press is a matte hit and never
        // reaches the pairing table at all.
        commit_into(&mut rig, &realm, VIEW.0, VIEW.1, 0x33);

        let switch = std::cell::RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        ));
        let turn = |rig: &mut Rig, inputs: Vec<crate::input::SeatInput>| {
            route_physical_turn(
                &mut rig.host.runtime,
                &rig.host.view.scenes,
                Some(&switch),
                inputs,
                VIEW,
                Instant::now(),
            );
        };

        // A key the human is holding, a pointer inside the surface, a button
        // held on top of it — the mid-drag, mid-chord state a VT switch
        // routinely lands in.
        turn(
            &mut rig,
            crate::input::physical_key(28, None, KeyState::Pressed),
        );
        turn(
            &mut rig,
            vec![physical_for_test(SeatInputKind::Motion { x: 8.0, y: 8.0 })],
        );
        turn(
            &mut rig,
            vec![physical_for_test(SeatInputKind::Button {
                button: 0x110,
                state: ButtonState::Pressed,
            })],
        );
        // ...and the dead-man chord going down. Fed to the switch's own
        // detection entry point, which is exactly what
        // `DeadManWatcher::observe` calls in production: this rig's hook stack
        // is the screenshot/attention pair (it has no display, so it carries
        // no watcher), and a hold armed any other way would be a fixture
        // rather than the switch's own state.
        switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), Instant::now());

        assert_eq!(
            rig.host.runtime.router.held_keys(&realm).len(),
            1,
            "the ordinary key must be in the pairing table"
        );
        assert_eq!(
            rig.host.runtime.router.held_buttons(&realm).len(),
            1,
            "the button must be in the implicit-grab table"
        );
        assert!(
            switch.borrow().deadline().is_some(),
            "the chord press must have armed a hold, or half this test is about nothing"
        );
        let delivered_before = rig
            .entries()
            .iter()
            .filter(|e| e.str("kind") == "seat_delivered")
            .count();

        // The seat takes the devices away.
        let released = suspend_physical_seat(&mut rig.host.runtime, &rig.host.view.scenes, &switch);

        assert_eq!(
            released, 2,
            "one key and one button are owed a release; the count is read off the deliveries \
             rather than off the tables, so `forgotten` cannot pass as `released`"
        );
        assert!(
            rig.host.runtime.router.held_keys(&realm).is_empty(),
            "a key left in the pairing table across a VT switch stays latched down in the \
             confined app indefinitely"
        );
        assert!(
            rig.host.runtime.router.held_buttons(&realm).is_empty(),
            "a button left held across a VT switch wedges the app's implicit pointer grab"
        );
        assert!(
            switch.borrow().deadline().is_none(),
            "an armed hold that survives the switch either fires with no gesture behind it or \
             wedges in a state only a release can leave"
        );
        // The releases really went to the app, not merely out of the tables.
        let delivered_after = rig
            .entries()
            .iter()
            .filter(|e| e.str("kind") == "seat_delivered")
            .count();
        assert_eq!(
            delivered_after - delivered_before,
            2,
            "the drained presses must reach the realm's shim through the ordinary delivery \
             funnel and be journalled there"
        );

        // ---------------------------------------------------------------
        // ...and every chord matcher forgot what it believed (WS-E.3.5).
        // ---------------------------------------------------------------
        //
        // The drain above pays the *app* its held presses, but those go
        // straight to the delivery funnel as `SeatDelivery`s -- they are not
        // `SeatInput`s and they never re-enter the hook stack -- so each
        // matcher keeps whatever modifiers it believed were down at the
        // instant the devices went away. **This was live on this branch**, and
        // the VT escape makes it fire on the very first use: a human leaving
        // this VT is holding ctrl+alt by construction, because that is how
        // they left.
        //
        // Driven on the screenshot chord because this rig's hook stack is the
        // screenshot/attention pair, so it is the matcher whose `observe` the
        // production intake actually reaches here. The clipboard's and the
        // lock's are the same mechanism with different keys; the VT escape's
        // has its own test beside it.
        let press = |keysym: u32| {
            physical_for_test(SeatInputKind::Key {
                source: crate::input::KeySource::Keysym,
                keysym,
                state: KeyState::Pressed,
            })
        };
        const CTRL_L: u32 = 0xffe3;
        const PRINT: u32 = 0xff61;

        // The control: with Ctrl genuinely held, the default `ctrl+print`
        // chord fires. Without this the assertion below would pass against a
        // rig whose screenshot chord never worked at all.
        // Counted off the journal rather than off `take_pending`, because
        // `route_physical_turn` drains the queue inside the turn: this rig has
        // no `--screenshot-dir`, so a fired gesture lands as exactly one
        // `screenshot_failed{no_screenshot_dir}` entry.
        let fired = |rig: &mut Rig| {
            rig.entries()
                .iter()
                .filter(|e| e.str("kind") == "screenshot_failed")
                .count()
        };
        turn(&mut rig, vec![press(CTRL_L)]);
        turn(&mut rig, vec![press(PRINT)]);
        assert_eq!(
            fired(&mut rig),
            1,
            "the screenshot chord must fire while Ctrl is really held, or the assertion below \
             is vacuous"
        );

        // The seat leaves again. No release for Ctrl will ever arrive.
        suspend_physical_seat(&mut rig.host.runtime, &rig.host.view.scenes, &switch);

        turn(&mut rig, vec![press(PRINT)]);
        assert_eq!(
            fired(&mut rig),
            1,
            "a stale modifier bit survived the pause: the human's next BARE key now fires a \
             chord they did not make. On bare metal that means a bare F5 switches VT and a \
             bare Delete raises the lock screen"
        );
    }

    /// **An answer given just before a VT switch is still honoured.**
    ///
    /// D-030(4) argues that **only step 4** of [`service_consent_round`] — the
    /// raise — is gated on visibility, and that gating the whole round would be
    /// an over-reach. Nothing held that. Inserting an early return at the top
    /// of the round left the entire workspace green, and the consequence is
    /// concrete: step 2 is the decision drain, so a human who pressed Allow and
    /// switched VT a moment later would have their answer sit undrained for the
    /// whole absence while the independent timeout sweep ran underneath it —
    /// the agent refused `timed_out` for a petition the human had *granted*.
    ///
    /// The press happens while the screen is ours; the seat leaves immediately
    /// after, exactly as a `Ctrl-Alt-F2` a beat after a click would.
    #[test]
    fn a_decision_taken_before_the_seat_left_is_still_drained() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-answered-then-paused",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        let petition = pump_until_armed(&mut rig, &grab);

        // The human answers...
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::Once),
        });
        // ...and switches VT before the next dispatch round drains it.
        rig.host.screen = PromptVisibility::ScreenNotOurs;
        rig.pump(Duration::from_millis(400));

        // The core's own state FIRST, deliberately. `await_resolution` blocks
        // on the wire, so a round that never drains the decision hangs it
        // rather than failing it -- which in CI burns the job's timeout
        // instead of reporting a defect. Asserting the grant table first turns
        // the same break into an immediate, legible failure.
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(Instant::now()).count(),
            1,
            "an Allow the human gave while the screen WAS theirs must still be drained after \
             the seat leaves: gating the whole consent round on visibility, rather than the \
             raise alone, silently converts a granted petition into a timed_out refusal"
        );
        assert_eq!(
            await_resolution(&mut client),
            Outcome::Granted,
            "...and the agent must be told, so it stops waiting on a petition already decided"
        );
    }

    /// **A human Allow grants the petition, over the wire.** The decision is
    /// injected as a click would deposit it; `service_consent` drains it,
    /// `resolve_human` accepts it, `deliver` mints the grant and sends
    /// `resolved(granted)`, and the card comes down the same round.
    #[test]
    fn a_human_allow_grants_the_petition_over_the_wire() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-allow",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        let petition = pump_until_armed(&mut rig, &grab);

        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::Once),
        });
        rig.pump(Duration::from_millis(400));

        assert_eq!(
            await_resolution(&mut client),
            Outcome::Granted,
            "a human Allow must grant the petition and tell the agent"
        );
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(Instant::now()).count(),
            1,
            "an approved petition mints exactly one grant row"
        );
        assert!(
            grab.borrow().armed_petition().is_none(),
            "the decided petition's card must be lowered, freeing the queue"
        );
    }

    /// **A human Deny refuses the petition, over the wire, and mints nothing.**
    #[test]
    fn a_human_deny_refuses_the_petition_over_the_wire() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-deny",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        let petition = pump_until_armed(&mut rig, &grab);

        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Deny,
        });
        rig.pump(Duration::from_millis(400));

        assert_eq!(
            await_resolution(&mut client),
            Outcome::Denied,
            "a human Deny must refuse the petition and tell the agent"
        );
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(Instant::now()).count(),
            0,
            "a denied petition must mint no grant row"
        );
        assert!(
            grab.borrow().armed_petition().is_none(),
            "the refused petition's card must be lowered too"
        );
    }

    /// **A raised prompt does not keep the frame dirty.** Re-raising the
    /// already-armed petition returns its route, which would set `changed` and
    /// mark the frame dirty every idle round — a busy-spin. `service_consent`
    /// must report no change when nothing happened, and leave the same prompt
    /// up.
    #[test]
    fn a_raised_prompt_does_not_keep_the_frame_dirty() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "consent-idle",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        let petition = pump_until_armed(&mut rig, &grab);

        // A fresh, quiet round with the prompt already up and no new decision.
        rig.host.runtime.dirty = false;
        rig.host.service_consent(Instant::now());

        assert!(
            !rig.host.runtime.dirty,
            "an unchanged consent round must not dirty the frame (no busy-spin)"
        );
        assert_eq!(
            grab.borrow().armed_petition(),
            Some(petition),
            "the same prompt must still be up"
        );
        drop(client);
    }

    #[test]
    fn capture_dump_writes_the_readback_atomically_and_leaves_no_temp() {
        // The `--capture-dump` diagnostic (P1.8.5): the bytes handed in land at
        // the path verbatim, and the sibling `.tmp` the atomic write goes
        // through is gone afterwards — a reader polling the path never sees a
        // half-written frame or a stray temp.
        let dir = std::env::temp_dir().join(format!("vitrin-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("internal.rgba");
        let frame: Vec<u8> = (0..64u32).flat_map(|i| (i as u8).to_le_bytes()).collect();

        write_capture_dump(&path, &frame);

        assert_eq!(
            std::fs::read(&path).expect("the dump landed"),
            frame,
            "the dumped bytes must be exactly the readback handed in"
        );
        assert!(
            !path.with_extension("tmp").exists(),
            "the atomic-write temp must not survive a successful write"
        );

        // A second write overwrites in place (each redraw refreshes it).
        let frame2: Vec<u8> = frame.iter().rev().copied().collect();
        write_capture_dump(&path, &frame2);
        assert_eq!(std::fs::read(&path).expect("second dump"), frame2);

        // A target whose own name ends in `.tmp` must still write atomically:
        // the temp is the target with `.part` APPENDED, which cannot equal the
        // target, so a `with_extension("tmp")` self-collision is impossible.
        let tmpish = dir.join("frame.tmp");
        write_capture_dump(&tmpish, &frame);
        assert_eq!(
            std::fs::read(&tmpish).expect("the .tmp-named dump landed"),
            frame,
            "a dump path ending in .tmp must still receive the frame whole",
        );
        assert!(
            !dir.join("frame.tmp.part").exists(),
            "the appended-suffix temp must not survive a successful write",
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// **Every dump names its realm, and nothing is written to the bare
    /// path** (WS-E.1.3).
    #[test]
    fn a_capture_dump_path_names_its_realm() {
        let base = std::path::Path::new("/tmp/vitrin/internal.rgba");
        assert_eq!(
            capture_dump_path(base, &RealmId::new("realm-0")),
            PathBuf::from("/tmp/vitrin/internal.rgba.realm-0")
        );
        // Two realms can never collide on one file, which is the whole
        // point: an unqualified dump named *a* view.
        assert_ne!(
            capture_dump_path(base, &RealmId::new("realm-0")),
            capture_dump_path(base, &RealmId::new("editor"))
        );
        // The suffix is appended to the whole path, never swapped into the
        // extension, so a base that already has one keeps it.
        assert_eq!(
            capture_dump_path(std::path::Path::new("dump.rgba"), &RealmId::new("editor")),
            PathBuf::from("dump.rgba.editor")
        );
    }

    // ---------------------------------------------------------------------
    // WS-E.1.3 (issue #209): one scene per realm, one bound to the output.
    // ---------------------------------------------------------------------

    /// Commit a deterministic fixture into `realm`'s scene through the same
    /// [`Scene::commit`] seam `ShimServer::handle_message` drives.
    ///
    /// Sized rather than coloured, because [`client_pixels`] is a pure
    /// function of `(x, y)`: two different sizes give two different composed
    /// views (one fills the view exactly, the other letterboxes), and both are
    /// recomputable in the assertion without a second copy of the fixture.
    ///
    /// [`client_pixels`]: crate::scene::tests::client_pixels
    fn commit_fixture<H: RuntimeHost>(host: &mut H, realm: &RealmId, (w, h): (u32, u32)) {
        let (_, view) = host.split();
        view.scene_mut(realm).commit(
            crate::scene::SurfaceContent::from_rgba(crate::scene::tests::client_pixels(w, h), w, h)
                .expect("well-formed fixture"),
        );
    }

    /// What a realm's view composes to at the test host's view size — the
    /// bytes any honest capture of that realm must return.
    fn expected_view((w, h): (u32, u32)) -> Vec<u8> {
        let mut scene = Scene::new();
        scene.commit(
            crate::scene::SurfaceContent::from_rgba(crate::scene::tests::client_pixels(w, h), w, h)
                .expect("well-formed fixture"),
        );
        scene.compose(VIEW.0, VIEW.1)
    }

    /// **THE confidentiality property (decision 1), byte-exact.**
    ///
    /// Two live realms commit different fixtures and **A is bound to the
    /// output**. A capture under a grant over A returns A's bytes; a capture
    /// under a grant over B returns B's bytes. Neither returns the other's,
    /// and neither returns "whatever is on the output".
    ///
    /// This is the test that fails by construction against the code this
    /// issue replaces: with one session-wide scene and one `view_cache`, both
    /// realms' captures were the *same* frame — the last committer's — so a
    /// grant over the hidden realm returned the bound realm's pixels. The
    /// bug needed two live realms to express at all, which is why nothing
    /// before WS-E.1.2 could have caught it and nothing in WS-E.1.2 did.
    ///
    /// Driven through the shipped runtime rather than the presenter alone:
    /// `post_dispatch` fills the per-realm cache, `dispatch_principal`
    /// resolves the frame by realm id, and the chokepoint decides — so a
    /// regression anywhere along that path fails here, not only one in the
    /// scene set.
    #[test]
    fn a_capture_returns_the_granted_realms_pixels_and_never_the_outputs() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "cross-realm-capture",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        // Two live shim sessions, so both realms are live for
        // `RealmLifecycle::view_is_live` and neither capture can be refused
        // `no_surface` for a reason that has nothing to do with selection.
        let serve: &[&str] = &["--serve"];
        rig.start_realms(&[("realm-0", serve), ("realm-b", serve)]);

        let bound = RealmId::new("realm-0");
        let hidden = RealmId::new("realm-b");
        // The first realm to attach holds the output, in id order.
        assert_eq!(
            rig.host.view.focused(),
            Some(&bound),
            "the output binds to the first realm to attach"
        );

        // Different fixtures: A exactly fills the view, B is letterboxed.
        const A: (u32, u32) = VIEW;
        const B: (u32, u32) = (VIEW.0 / 2, VIEW.1 / 2);
        commit_fixture(&mut rig.host, &bound, A);
        commit_fixture(&mut rig.host, &hidden, B);
        let (want_a, want_b) = (expected_view(A), expected_view(B));
        assert_ne!(want_a, want_b, "the two fixtures must actually differ");

        // One dirty round refreshes **both** realms' caches off the same
        // completed composite.
        rig.host.runtime.dirty = true;
        post_dispatch(&mut rig.host);
        assert_eq!(rig.host.runtime.view_cache.get(&bound), Some(&want_a));
        assert_eq!(
            rig.host.runtime.view_cache.get(&hidden),
            Some(&want_b),
            "a hidden realm's view is composed too, or its capture goes stale"
        );

        // Now over the wire, through a real grant on each realm.
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        rig.pump(Duration::from_millis(200));
        assert_eq!(await_resolution(&mut client), Outcome::Granted);

        // A second realm handle and a second grant, at ids above the
        // watermark.
        client
            .send_message(
                &vitrin_principal::requests::GetRealm {
                    realm: 9,
                    name: "realm-b".into(),
                }
                .encode(2),
                None,
            )
            .expect("get_realm realm-b");
        client
            .send_message(
                &vitrin_realm::requests::RequestGrant {
                    grant: 10,
                    consent: 11,
                    view: 12,
                    pointer: 13,
                    text: 14,
                    resource: String::new(),
                    verbs: Verb::OBSERVE,
                    expiry_ms: 0,
                    max_event_rate: 0,
                    persistence: Persistence::WhileRunning,
                    flags: 0,
                }
                .encode(9),
                None,
            )
            .expect("request_grant realm-b");
        rig.pump(Duration::from_millis(200));
        assert_eq!(
            await_resolution_at(&mut client, 10),
            Outcome::Granted,
            "the second realm's petition must be granted"
        );

        // The bound realm's view id is 6, the hidden realm's is 12.
        let (wire_a, wire_b) = (
            crate::capture::tests::xrgb_of(&want_a),
            crate::capture::tests::xrgb_of(&want_b),
        );
        let got_a = capture_bytes(&mut rig, &mut client, 6);
        let got_b = capture_bytes(&mut rig, &mut client, 12);
        // Diagnosed rather than dumped: two frames of a few hundred KiB in
        // an `assert_eq!` message is a wall of bytes nobody reads, and the
        // fact that matters is *whose* pixels arrived.
        let whose = |got: &[u8]| match got {
            g if g == wire_a => "the BOUND realm's",
            g if g == wire_b => "the HIDDEN realm's",
            _ => "neither realm's",
        };
        assert_eq!(
            whose(&got_a),
            "the BOUND realm's",
            "a capture over the bound realm must be the bound realm's own pixels"
        );
        assert_eq!(
            whose(&got_b),
            "the HIDDEN realm's",
            "a capture over the HIDDEN realm returned {} pixels: with one frame for the \
             session a grant over a hidden realm is served whatever is on the output, \
             which is the cross-realm leak WS-E.1.3 exists to close",
            whose(&got_b)
        );
        assert_ne!(got_a, got_b);
    }

    /// **A hidden realm keeps being paced, and its capture keeps moving.**
    ///
    /// Decision 2: a Wayland client throttles on `frame_done`, so a realm
    /// that stops being paced stops repainting and its capture becomes a
    /// stale frame — which `refusal.no_surface` forbids in as many words.
    /// Both halves are asserted: the hidden realm's shim really is handed
    /// frame callbacks off the output's completed composite, and its cached
    /// view really changes when it repaints.
    #[test]
    fn a_hidden_realm_is_paced_and_its_capture_keeps_changing() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "hidden-pacing",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        // Both realms animate, so both owe frame callbacks continuously.
        let serve: &[&str] = &["--serve", "--animate", "100000"];
        rig.start_realms(&[("realm-0", serve), ("realm-b", serve)]);
        let hidden = RealmId::new("realm-b");
        assert_ne!(
            rig.host.view.focused(),
            Some(&hidden),
            "fixture check: realm-b must NOT be the realm on the output"
        );

        // The hidden realm's app commits: a shim that were never paced would
        // publish its first frame and then stall on the frame callback.
        rig.pump_until(Duration::from_secs(10), |host| {
            host.view.surface_of(&hidden).is_some()
        });
        rig.pump(Duration::from_millis(200));
        let first = rig
            .host
            .runtime
            .view_cache
            .get(&hidden)
            .cloned()
            .expect("the hidden realm's view is cached");

        // Over a window, the hidden realm's *cached capture* changes: it is
        // still receiving `frame_done` and still repainting.
        let mut moved = false;
        for _ in 0..40 {
            rig.pump(Duration::from_millis(50));
            if rig.host.runtime.view_cache.get(&hidden) != Some(&first) {
                moved = true;
                break;
            }
        }
        assert!(
            moved,
            "a hidden realm's capture never changed: its shim is not being paced, so its \
             frames are stale -- the exact thing decision 2 refuses to ship"
        );
        // ...and the bound realm has a cached view of its own, so the frame
        // the hidden realm's capture read was not the only one in the map.
        //
        // This is a weaker counterweight than the real-app gate's, and saying
        // so is the point: it asserts *presence*, not that the two frames
        // differ. What rules out "the whole session happens to be animating
        // one realm" is `RealTwoRealmsHiddenKeepsPainting`, which pairs a
        // STATIC bound realm with an animating hidden one and requires the
        // bound realm's capture not to move. In-crate, both realms are driven
        // by the same synthetic commit, so that asymmetry is unavailable here.
        assert!(
            rig.host
                .runtime
                .view_cache
                .contains_key(&RealmId::new("realm-0")),
            "the bound realm keeps its own cached view"
        );
    }

    /// **A dead realm's cached frame is removed, not merely refused.**
    ///
    /// `realm_is_live` already refuses a dead realm's capture; this is the
    /// defence-in-depth half — the same posture
    /// `RetainedOutput::scrub_retained_frame` takes for the headless
    /// framebuffer — applied to the per-realm capture cache, so the bytes are
    /// gone rather than only unreachable.
    ///
    /// **Driven entirely by the production death path**, which is what makes
    /// it evidence at all. It used to reach in and `realms.remove(&doomed)`
    /// afterwards, manufacturing a state no production path produces —
    /// [`close_realm`] leaves the entry with `server: None`, and nothing in the
    /// runtime ever removes a key from [`Runtime::realms`] before shutdown. The
    /// prune it was checking was keyed on `realms.keys()` and so could never
    /// fire; the test passed by fabricating its own precondition.
    #[test]
    fn a_realm_with_no_live_shim_session_loses_its_cached_frame() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "cache-prune",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve"];
        rig.start_realms(&[("realm-0", serve), ("realm-b", serve)]);
        let doomed = RealmId::new("realm-b");
        commit_fixture(&mut rig.host, &doomed, VIEW);
        rig.host.runtime.dirty = true;
        post_dispatch(&mut rig.host);
        assert!(
            rig.host.runtime.view_cache.contains_key(&doomed),
            "fixture check: the realm's frame is cached before it dies"
        );

        // The realm's shim session ends. Nothing else: the runtime entry
        // stays, exactly as it does in production.
        close_realm(&mut rig.host, &doomed, DeathCause::ConnectionClosed);
        assert!(
            rig.host.runtime.realms.contains_key(&doomed),
            "fixture check: the death path leaves the runtime entry in place -- if this ever \
             stops being true, `refresh_view_cache`'s live set is keyed on the wrong fact"
        );
        rig.host.runtime.dirty = true;
        post_dispatch(&mut rig.host);
        assert!(
            !rig.host.runtime.view_cache.contains_key(&doomed),
            "a realm with no live shim session must not keep a frame in the capture cache"
        );
        assert!(
            rig.host
                .runtime
                .view_cache
                .contains_key(&RealmId::new("realm-0")),
            "and the survivor keeps its own"
        );
    }

    /// **The output does not stay bound to a realm that is gone.**
    ///
    /// `RealmScenes::bind` was the only writer of the binding and nothing
    /// cleared it, so when the bound realm's app exited the teardown funnel
    /// cleared that realm's scene and left the output pointed at it: every
    /// later composite rendered the empty scene — the deterministic background
    /// — for the rest of the session while a live sibling painted, and
    /// `post_dispatch`'s bound-realm gate went on suppressing the agent cursor
    /// against a `focused()` that named a corpse.
    ///
    /// Three things are asserted, and the third is the one that keeps the
    /// stopgap honest:
    ///
    /// 1. The output moves to the survivor rather than staying on the corpse.
    /// 2. What it composites is the survivor's own pixels, not the background
    ///    — byte-exact, because "bound somewhere" is not the claim.
    /// 3. It moves to the realm [`seat_target`] serves — so the realm a human
    ///    watches and the realm their own keystrokes reach stay equal, which
    ///    is D-018(2)'s fifth ordering rule arrived at by a death rather than
    ///    by a verb. A rebind that broke the agreement would leave a human
    ///    watching one realm while typing into another.
    #[test]
    fn the_output_leaves_a_dead_realm_for_a_live_sibling() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "bound-realm-death",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve"];
        rig.start_realms(&[("realm-0", serve), ("realm-b", serve)]);
        let (bound, survivor) = (RealmId::new("realm-0"), RealmId::new("realm-b"));
        assert_eq!(
            rig.host.view.focused(),
            Some(&bound),
            "fixture check: the output binds to the first realm to attach"
        );

        // Both realms paint, and the survivor's fixture is deliberately not
        // the bound realm's, so "the output shows the survivor" is a claim
        // about bytes rather than about a name.
        const A: (u32, u32) = VIEW;
        const B: (u32, u32) = (VIEW.0 / 2, VIEW.1 / 2);
        commit_fixture(&mut rig.host, &bound, A);
        commit_fixture(&mut rig.host, &survivor, B);
        let want_b = expected_view(B);
        assert_ne!(
            want_b,
            crate::test_pattern::render(VIEW.0, VIEW.1),
            "fixture check: the survivor's view must differ from the empty-scene background, \
             or the assertion below cannot tell a rebind from a stuck output"
        );

        // The realm holding the output dies. Nothing else moves.
        close_realm(&mut rig.host, &bound, DeathCause::ConnectionClosed);
        assert_eq!(
            rig.host.view.focused(),
            Some(&survivor),
            "the output must not stay bound to a realm whose shim session is gone"
        );
        assert_eq!(
            seat_target(&rig.host.runtime.realms, rig.host.view.focused())
                .map(|(realm_id, _)| realm_id),
            Some(&survivor),
            "and it must land on the realm the human's own input follows, or the human \
             watches one realm while typing into another"
        );

        // What the output composites is the survivor's own view, not the
        // deterministic background.
        assert_eq!(
            rig.host.view.scenes.bound().compose(VIEW.0, VIEW.1),
            want_b,
            "the output composites the survivor's pixels; a binding left on the dead realm \
             renders its cleared scene -- the background -- for the rest of the session"
        );

        // The survivor dies too: nothing is serving, so the output is bound
        // to no realm and shows the background. `seat_target` answers `None`
        // on the same fact, so the two still agree.
        close_realm(&mut rig.host, &survivor, DeathCause::ConnectionClosed);
        assert_eq!(rig.host.view.focused(), None);
        assert_eq!(
            seat_target(&rig.host.runtime.realms, rig.host.view.focused())
                .map(|(realm_id, _)| realm_id),
            None
        );
        assert_eq!(
            rig.host.view.scenes.bound().compose(VIEW.0, VIEW.1),
            crate::test_pattern::render(VIEW.0, VIEW.1),
            "with no realm serving the output is the documented deterministic background"
        );
    }

    /// **The agent cursor is drawn only for the realm on the output**
    /// (D-019, weakened deliberately by WS-E.1.3 and published as a limit).
    ///
    /// The sprite is painted in the output's coordinates over the output's
    /// realm, so a pointer owed to a hidden realm has no position that means
    /// anything in the picture the human is looking at. Asserted at the one
    /// site that decides it, `post_dispatch`, because the gate is a
    /// comparison between two facts neither backend owns alone.
    #[test]
    fn the_agent_cursor_is_offered_only_for_the_bound_realm() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "cursor-bound",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve"];
        rig.start_realms(&[("realm-0", serve), ("realm-b", serve)]);
        let bound = RealmId::new("realm-0");
        let hidden = RealmId::new("realm-b");
        assert_eq!(rig.host.view.focused(), Some(&bound));

        // An agent moves its pointer inside the realm on the output: the
        // sprite's position is offered.
        rig.host.runtime.router.route_emulated(
            &bound,
            SeatInput::emulated(crate::input::SeatInputKind::Motion { x: 12.0, y: 9.0 }),
            VIEW,
            Some(VIEW),
        );
        assert_eq!(
            rig.host.runtime.router.agent_pointer(&bound),
            Some((12.0, 9.0)),
            "fixture check: the router holds an agent-owned position for that realm"
        );
        rig.host.view.cursor_offered = None;
        post_dispatch(&mut rig.host);
        assert_eq!(
            rig.host.view.cursor_offered,
            Some(Some((12.0, 9.0))),
            "the bound realm's agent pointer must reach the sprite"
        );

        // A second agent motion, this time inside the HIDDEN realm. It must
        // not be offered -- it is a position in another realm's view -- and,
        // since WS-E.1.6, it must not disturb the bound realm's position
        // either: the two realms hold two pointers.
        rig.host.runtime.router.route_emulated(
            &hidden,
            SeatInput::emulated(crate::input::SeatInputKind::Motion { x: 30.0, y: 20.0 }),
            VIEW,
            Some(VIEW),
        );
        assert_eq!(
            rig.host.runtime.router.agent_pointer(&bound),
            Some((12.0, 9.0)),
            "a hidden realm's agent motion must not move the visible realm's sprite"
        );
        rig.host.view.cursor_offered = None;
        post_dispatch(&mut rig.host);
        assert_eq!(
            rig.host.view.cursor_offered,
            Some(Some((12.0, 9.0))),
            "the visible realm's own sprite is still the one drawn"
        );

        // ...and with the output on the hidden realm's side of the pair, the
        // sprite that is drawn is THAT realm's.
        rig.host.view.bind_output(&hidden);
        rig.host.view.cursor_offered = None;
        post_dispatch(&mut rig.host);
        assert_eq!(
            rig.host.view.cursor_offered,
            Some(Some((30.0, 20.0))),
            "the sprite follows the output's realm, not a session-wide position"
        );

        // A realm with no agent motion at all offers nothing, which is what
        // keeps a crosshair off an app no agent is pointing into.
        let untouched = RealmId::new("realm-c");
        rig.host.view.scenes.bind(&untouched);
        rig.host.view.cursor_offered = None;
        post_dispatch(&mut rig.host);
        assert_eq!(rig.host.view.cursor_offered, Some(None));
    }

    // ---------------------------------------------------------------------
    // WS-E.1.4 (issue #210): the served layout verbs, and D-018(2)'s
    // ordering invariants tested AS invariants
    // ---------------------------------------------------------------------
    //
    // D-018's own cost note says of its four unpurchasable ordering rules
    // that "none of the four is tested *as an invariant* against a client
    // trying to violate it, and none can be until something outside the core
    // can arrange realms". Serving `layout_focus` and `layout_arrange` is
    // that moment, so these tests are the discharge of that note.
    //
    // Two disciplines they all keep, because a test that keeps neither is
    // the kind that has already been caught vacuous in this workstream:
    //
    // 1. **The arrangement is driven through the wire**, on a real socket,
    //    by a real client holding a real grant, through `request_grant` ->
    //    the consent path -> `get_layout_*` -> `focus`/`set_fullscreen` ->
    //    the enforcement chokepoint -> `apply_layout`. Nothing calls
    //    `Presenter::bind_output` by hand to manufacture the precondition.
    // 2. **The verb set is the maximum this core serves**, so a passing
    //    assertion says "no grant buys this" rather than "the grant I
    //    happened to construct did not".

    /// Every verb this core serves, in one petition — the widest authority
    /// a grant can carry here. A property proved against this set is proved
    /// against every subset, which is what makes these tests statements
    /// about *no grant* rather than about one.
    fn max_verbs() -> Verb {
        Verb::OBSERVE
            | Verb::ACTUATE_POINTER
            | Verb::ACTUATE_TEXT
            | Verb::LAYOUT_ARRANGE
            | Verb::LAYOUT_FOCUS
    }

    /// Wire ids a layout client uses, above the five `request_grant` mints.
    const FOCUS_FACET: u32 = 9;
    const ARRANGE_FACET: u32 = 10;

    /// The `hello` + `get_realm` preamble for an arbitrary identity and
    /// realm, so a test can put **two** principals on one session (the
    /// single-identity `send_preamble` cannot: the chokepoint's
    /// `consent_held` gate would refuse the layout holder's every request
    /// while the prompt under test was up).
    fn send_preamble_as(client: &mut Connection, identity: &str, token: &str, realm: &str) {
        let hello = vitrin_handshake::requests::Hello {
            version: PROTOCOL_VERSION,
            principal: 2,
            identity: identity.into(),
            credential_type: STATIC_TOKEN_SCHEME.into(),
            credential: token.into(),
        };
        client
            .send_message(&hello.encode(HANDSHAKE_ID), None)
            .expect("hello");
        let get_realm = vitrin_principal::requests::GetRealm {
            realm: 3,
            name: realm.into(),
        };
        client
            .send_message(&get_realm.encode(2), None)
            .expect("get_realm");
    }

    /// Petition for `verbs` over the realm handle at id 3.
    fn send_petition_for(client: &mut Connection, verbs: Verb) {
        let req = vitrin_realm::requests::RequestGrant {
            grant: 4,
            consent: 5,
            view: 6,
            pointer: 7,
            text: 8,
            resource: String::new(),
            verbs,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: Persistence::WhileRunning,
            flags: 0,
        };
        client
            .send_message(&req.encode(3), None)
            .expect("request_grant");
    }

    /// A **second** petition on the same connection, above the ids the first
    /// one and the two layout facets took. The watermark never rewinds, so
    /// the ids are simply the next five; nothing reads the resolution, which
    /// is the point — this exists to leave a prompt on screen for *this*
    /// principal, which is the fact the chokepoint's step 5b reads.
    fn send_second_petition_for(client: &mut Connection, verbs: Verb) {
        let req = vitrin_realm::requests::RequestGrant {
            grant: 11,
            consent: 12,
            view: 13,
            pointer: 14,
            text: 15,
            resource: String::new(),
            verbs,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: Persistence::WhileRunning,
            flags: 0,
        };
        client
            .send_message(&req.encode(3), None)
            .expect("second request_grant");
    }

    /// Mint both layout facets on the grant at [`GRANT_ID`]. Structural
    /// mints: legal whatever the grant holds, no reply to wait for.
    fn mint_layout_facets(client: &mut Connection) {
        use vitrin_protocol::generated::vitrin_grant::requests::{
            GetLayoutArrange, GetLayoutFocus,
        };
        client
            .send_message(
                &GetLayoutFocus {
                    layout_focus: FOCUS_FACET,
                }
                .encode(GRANT_ID),
                None,
            )
            .expect("get_layout_focus");
        client
            .send_message(
                &GetLayoutArrange {
                    layout_arrange: ARRANGE_FACET,
                }
                .encode(GRANT_ID),
                None,
            )
            .expect("get_layout_arrange");
    }

    fn send_focus(client: &mut Connection) {
        use vitrin_protocol::generated::vitrin_layout_focus::requests::Focus;
        client
            .send_message(&Focus {}.encode(FOCUS_FACET), None)
            .expect("focus");
    }

    fn send_set_fullscreen(
        client: &mut Connection,
        mode: vitrin_protocol::generated::vitrin_layout_arrange::Mode,
    ) {
        use vitrin_protocol::generated::vitrin_layout_arrange::requests::SetFullscreen;
        client
            .send_message(&SetFullscreen { mode }.encode(ARRANGE_FACET), None)
            .expect("set_fullscreen");
    }

    /// A client that holds [`max_verbs`] over `realm`, auto-approved, with both
    /// layout facets minted — the "worst case holder" every invariant test
    /// below is written against.
    fn layout_holder(rig: &mut Rig, identity: &str, token: &str, realm: &str) -> Connection {
        let mut client = agent(&rig.socket);
        send_preamble_as(&mut client, identity, token, realm);
        send_petition_for(&mut client, max_verbs());
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut client),
            Outcome::Granted,
            "the rig's auto-approve policy must grant the maximum served verb set; \
             a refusal here means a verb left SERVED_VERB_BITS and this whole test file \
             is no longer testing what it claims"
        );
        mint_layout_facets(&mut client);
        rig.pump(Duration::from_millis(200));
        client
    }

    /// The rig every invariant test below uses: auto-approve (so the holder's
    /// own grant needs no human), two realms with real forked mock shims, and
    /// the holder's grant over `realm-a`.
    fn two_realm_rig(label: &str) -> (Rig, RealmId, RealmId) {
        let mut rig = Rig::new(
            label,
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        rig.start_realms(&[
            ("realm-a", &["--serve", "--seat"]),
            ("realm-b", &["--serve"]),
        ]);
        rig.pump(Duration::from_millis(400));
        (rig, RealmId::new("realm-a"), RealmId::new("realm-b"))
    }

    /// Commit a distinguishable surface into `realm`'s scene at `(w, h)`.
    ///
    /// This is the **app's** behaviour, not the client's: an app answers a
    /// `configure` with a buffer of some size, and the arrangement under test
    /// is what the core does with that buffer. Driving it here is the same
    /// accommodation every scene test in this crate makes.
    fn commit_into(rig: &mut Rig, realm: &RealmId, w: u32, h: u32, tint: u8) {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[tint, (i % 251) as u8, 0x40, 0xff]);
        }
        rig.host.view.scenes.scene_mut(realm).commit(
            crate::scene::SurfaceContent::from_rgba(rgba, w, h).expect("well-formed content"),
        );
        // One dirty round refreshes every live realm's cache entry off the
        // same completed composite, which is what makes the realm *live* for
        // the chokepoint's `no_surface` gate. Without it every layout request
        // below would be refused `no_surface` — correctly, and for a reason
        // that has nothing to do with what these tests are about.
        rig.host.runtime.dirty = true;
        post_dispatch(&mut rig.host);
    }

    /// **Every arrangement the two served verbs can express**, as a sequence
    /// of wire requests. Finite and small by construction — that is the whole
    /// of decision 3's "the only arrangement this scene model can express" —
    /// so a test can sweep the *entire* space rather than sample it.
    fn every_arrangement() -> Vec<&'static str> {
        vec![
            "focus",
            "fullscreen",
            "windowed",
            "focus",
            "windowed",
            "fullscreen",
            "fullscreen",
            "focus",
            "windowed",
        ]
    }

    fn drive_arrangement(rig: &mut Rig, client: &mut Connection, step: &str) {
        match step {
            "focus" => send_focus(client),
            "fullscreen" => send_set_fullscreen(
                client,
                vitrin_protocol::generated::vitrin_layout_arrange::Mode::Fullscreen,
            ),
            "windowed" => send_set_fullscreen(
                client,
                vitrin_protocol::generated::vitrin_layout_arrange::Mode::Windowed,
            ),
            other => panic!("unknown arrangement step {other}"),
        }
        rig.pump(Duration::from_millis(200));
    }

    /// **D-018(2) invariants 1 and 3, as invariants.** A client holding the
    /// maximum verb set drives every arrangement it can express while another
    /// principal's consent prompt is on screen; the card and the trust band
    /// come out byte-identical every time.
    ///
    /// What makes this a test of the *invariant* rather than of a happy path:
    /// the sweep covers the whole expressible arrangement space (see
    /// [`every_arrangement`]) at the widest verb set this core grants, and the
    /// comparison is against the card rasterized independently — so a card
    /// that moved, shrank, was scrolled off, or was covered by realm content
    /// fails, and so does a trust band a fullscreened realm painted over.
    #[test]
    fn no_arrangement_at_the_maximum_verb_set_can_touch_the_consent_card() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, b) = two_realm_rig("layout-inv-card");
        // An output the 560px consent card actually fits inside, with realm
        // view around it — so "the card is intact" is a statement about
        // composition and not an artifact of cropping. Applied through the
        // production resize path.
        const OUT: (u32, u32) = (800, 600);
        apply_output_resize(&mut rig.host, OUT);
        commit_into(&mut rig, &a, OUT.0, OUT.1, 0x11);
        commit_into(&mut rig, &b, OUT.0 / 2, OUT.1 / 2, 0x99);
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-b");

        // A *different* principal's prompt goes up and stays up. Different,
        // because `consent_held` refuses a principal's own uses while its own
        // prompt is up — so a single-identity rig would test that gate and
        // never reach the invariant behind it.
        rig.host
            .view
            .consent
            .show_for_test(crate::consent::tests::prompt_fixture());
        let card = crate::consent::render::rasterize(&crate::consent::tests::prompt_fixture());
        let (cx, cy) = rig
            .host
            .view
            .consent
            .card_origin(OUT.0, OUT.1)
            .expect("a prompt is up, so the card has an origin");
        assert!(
            cx >= 0 && cy >= 0,
            "the card must fit in the {OUT:?} output"
        );
        let band = crate::consent::TrustedIndicator::for_test().color();

        let mut checked = 0;
        for step in every_arrangement() {
            drive_arrangement(&mut rig, &mut holder, step);
            let output = rig.host.view.human_visible();

            // Invariant 1: the trust indicator composites above every
            // principal's content, whatever is arranged under it.
            assert_eq!(
                &output[..4],
                &band[..],
                "after `{step}` the trust band no longer owns pixel (0,0): an arrangement \
                 reached above the one strip the human reads the session colour from"
            );

            // Invariant 3: no arrangement occludes, fullscreens over, or
            // resizes away the consent surface. Byte-exact, row by row,
            // against an independently rasterized card.
            assert_eq!(
                rig.host.view.consent.card_origin(OUT.0, OUT.1),
                Some((cx, cy)),
                "after `{step}` the card moved: its geometry must not be a function of \
                 anything a layout holder can set"
            );
            for row in 0..card.height {
                let d = ((cy as u32 + row) as usize * OUT.0 as usize + cx as usize) * 4;
                let s = row as usize * card.width as usize * 4;
                let run = card.width as usize * 4;
                assert_eq!(
                    &output[d..d + run],
                    &card.rgba[s..s + run],
                    "after `{step}`, card row {row} is not the card: realm content reached \
                     over the consent surface"
                );
            }
            checked += 1;
        }
        assert_eq!(
            checked,
            every_arrangement().len(),
            "the sweep must cover the whole expressible arrangement space"
        );
    }

    /// **D-018(2) invariant 2, as an invariant.** Which surface an input
    /// event reaches is decided by the core's own geometry, and a layout
    /// holder has no vocabulary for saying otherwise.
    ///
    /// Two halves, because the invariant has two:
    ///
    /// - **There is no stacking to claim.** Neither served facet defines any
    ///   request beyond the one it ships, so "a client's claimed stacking"
    ///   is not merely ignored, it is unstateable. This half is the guard
    ///   against the partial-verb hazard issue #210 names as its most
    ///   fragile point ("nothing structurally stops a later issue adding a
    ///   `place` request while the scene still cannot honour it"): adding
    ///   one turns this red.
    /// - **What a holder *can* move, it moves wholly.** A `focus` moves the
    ///   output binding, and the realm input reaches follows it — never
    ///   diverging, in either direction, at any point in the sweep. A
    ///   divergence is precisely the state SCENE AUTHORITY's fifth ordering
    ///   rule forbids: keys reaching a realm the human cannot see.
    #[test]
    fn the_core_not_the_client_decides_which_surface_input_reaches() {
        use vitrin_protocol::generated::{vitrin_layout_arrange, vitrin_layout_focus};

        let _fd = crate::capture::tests::fd_lock();

        // Half one: the vocabulary. Opcodes are implicit document order and
        // append-only, so "the last defined opcode is 0" is exactly "there is
        // one request".
        assert_eq!(
            vitrin_layout_focus::requests::Focus::OPCODE,
            0,
            "focus must be vitrin_layout_focus's first request"
        );
        assert_eq!(
            vitrin_layout_arrange::requests::SetFullscreen::OPCODE,
            0,
            "set_fullscreen must be vitrin_layout_arrange's first request"
        );
        // ...and the count. `MESSAGE_COUNT` is generated from the IDL, so a
        // `place`, `raise` or `resize` request appended to either interface
        // moves it and fails here with a message naming why that is not a
        // free addition.
        // Re-pinned 36 -> 37 by WS-E.1.7 (issue #232), and the decision it
        // demanded was made rather than skipped: the added message is
        // `vitrin_principal.attention`, an argument-free EVENT on the
        // connection's own object. It is not a request on either layout
        // interface, it adds no arrangement this scene cannot honour, and it
        // allocates no verb bit -- so D-018(2) invariant 2 is untouched. What
        // it does do is make `preempted` CONDITIONAL for the two layout verbs;
        // that is D-023, not this invariant.
        // Re-pinned 37 -> 40 by WS-E.2.1 (issue #213), with the same decision
        // taken rather than skipped: the three added messages are
        // `vitrin_shim_session.request_selection`, `.selection` and
        // `.offer_selection` -- two events and one request, all on the SHIM
        // bootstrap object, on the shim connection class, which no principal
        // can address at all. None is a request on either layout interface,
        // none adds an arrangement this scene cannot honour, and none allocates
        // a verb bit (`Verb::VALID_MASK` is still 575). D-018(2) invariant 2 is
        // untouched. What they do add is a cross-realm channel the human drives
        // with two physical chords; that is D-024, not this invariant.
        // Re-pinned 40 -> 45 by WS-E.4.2 (issue #222), decision taken rather
        // than skipped: the five added messages are `relative_motion`,
        // `gesture_begin`, `gesture_swipe_update`, `gesture_pinch_update` and
        // `gesture_end` -- all EVENTS on `vitrin_shim_seat`, an interface the
        // schema forbids from defining any request at all (B2), on the shim
        // connection class no principal can address. None is a request on
        // either layout interface, none adds an arrangement this scene cannot
        // honour, and none allocates a verb bit (`Verb::VALID_MASK` is still
        // 575). D-018(2) invariant 2 is untouched. What they do add is a
        // pairing obligation on the core -- one `gesture_end` per
        // `gesture_begin` delivered, on every path input is taken away; that
        // is D-032, not this invariant.
        // Re-pinned 45 -> 47 by WS-E.4.2's second half (issue #222), and this
        // one needed the decision made rather than waved through, because it
        // is the first addition since #213 that includes a REQUEST and the
        // first ever that touches input ROUTING. The two added messages are
        // `vitrin_shim_session.pointer_constraint` (a request) and
        // `.pointer_constraint_state` (an event), both on the shim bootstrap
        // object, on the shim connection class no principal can address.
        // Neither is a request on either layout interface, neither adds an
        // arrangement this scene cannot honour, and neither allocates a verb
        // bit (`Verb::VALID_MASK` is still 575) -- so D-018(2) invariant 2's
        // first half is untouched.
        // Its SECOND half is the one worth stating: a pointer constraint
        // changes what the APP is told, never what the core believes. The
        // core's own hit test still decides which surface an input event
        // reaches; an active constraint is applied where the app-facing
        // position is MINTED, downstream of every gate and of the core's own
        // geometry, and it is gated on `Origin::Physical` so a confined app
        // cannot re-express a principal's actuation either. A constraint is
        // also derived from no grant, so it adds no verb whose requests the
        // server could fail to enforce. That is D-032, not this invariant.
        assert_eq!(
            vitrin_protocol::generated::MESSAGE_COUNT,
            47,
            "a message was added to the IDL. If it is a request on \
             vitrin_layout_arrange or vitrin_layout_focus, D-018(2) invariant 2 is at \
             stake: this scene shows one realm, unstacked and unoverlapped, so it cannot \
             honour place/resize/raise/stacking, and a granted verb whose requests the \
             server cannot carry out breaks the IDL's own 'a deployment MUST NOT grant a \
             verb it does not enforce'. Re-pin this number only after deciding that."
        );

        // Half two: the behaviour, over the wire.
        let (mut rig, a, b) = two_realm_rig("layout-inv-hittest");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-b");

        for step in every_arrangement() {
            drive_arrangement(&mut rig, &mut holder, step);
            let shown = rig.host.view.focused().cloned();
            // Through the production entry point, not `seat_target` with a
            // binding this test supplied: the whole claim is that the two
            // cannot come apart, and asking a function for the answer you
            // handed it proves nothing.
            let reached = physical_seat_target(&rig.host.runtime.realms, &rig.host.view.scenes)
                .map(|(realm_id, _)| realm_id.clone());
            assert_eq!(
                reached, shown,
                "after `{step}` the realm the output shows and the realm the human's input \
                 reaches are different realms; that split is focus theft in its sharpest \
                 form and no verb set may produce it"
            );
        }
        // ...and the holder really did move it: it holds a grant over
        // realm-b, and realm-a is the first still-serving realm in id order,
        // so a binding that never moved would still name realm-a.
        assert_eq!(
            rig.host.view.focused(),
            Some(&b),
            "the sweep's focus requests must have moved the output to the granted realm; \
             `realm-a` sorts first, so this failing means nothing moved at all"
        );
        assert_ne!(a, b);
    }

    /// **D-018(2) invariant 4, as an invariant.** No arrangement puts an
    /// agent principal's cursor into any principal's captured frame.
    ///
    /// D-019 made this invariant non-vacuous by compositing an agent's own
    /// cursor at all; serving `layout_focus` makes it *purchasable-looking*,
    /// because a holder now chooses which realm the sprite is drawn over
    /// (D-019's WS-E.1.3 amendment: the sprite draws only for the realm on
    /// the output). Two principals here, which is the shape the invariant is
    /// about: one actuates in `realm-b` and drives the sprite, the other only
    /// observes `realm-a`.
    ///
    /// **The sprite is proved live before anything is asserted about it.**
    /// The first draft of this test set `set_agent_cursor` by hand and swept;
    /// the very next `post_dispatch` cleared the offer, so every capture was
    /// compared against a sprite that had never existed — a mutation that
    /// leaked the sprite into captures still passed. What makes it real is
    /// driving an actual `vitrin_actuator_pointer.move` over the wire and
    /// asserting the presenter was *offered* a position, every round.
    #[test]
    fn no_arrangement_puts_an_agent_cursor_into_any_realms_capture() {
        use vitrin_protocol::generated::vitrin_actuator_pointer::requests::Move;

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, b) = two_realm_rig("layout-inv-cursor");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0 / 2, VIEW.1 / 2, 0x99);

        // The actuating principal, holding everything, over realm-b.
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-b");
        // A *different* principal that only watches realm-a. Its capture is
        // the "another principal's captured frame" the invariant names.
        let mut watcher = agent(&rig.socket);
        send_preamble_as(&mut watcher, OTHER_IDENTITY, OTHER_TOKEN, "realm-a");
        send_petition_for(&mut watcher, Verb::OBSERVE);
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut watcher), Outcome::Granted);

        // Focus first: the sprite is drawn only for the realm on the output
        // (D-019, amended by WS-E.1.3), so the holder's actuation has to be
        // into the realm this test then inspects.
        send_focus(&mut holder);
        rig.pump(Duration::from_millis(300));
        assert_eq!(rig.host.view.focused(), Some(&b));

        // The bare truth each capture must keep equalling, in the wire's own
        // pixel order (`capture::render_frame` converts; comparing a readback
        // against a wire frame without this compares two encodings).
        let bare_a = crate::capture::tests::xrgb_of(&rig.host.view.capture_view(&a));
        let bare_b = crate::capture::tests::xrgb_of(&rig.host.view.capture_view(&b));
        assert_ne!(
            bare_a, bare_b,
            "the two realms must differ, or this proves nothing"
        );

        for step in every_arrangement() {
            // A real agent pointer move, over the wire, every round: this is
            // what puts a sprite on the human-visible output at all.
            rig.host.view.cursor_offered = None;
            holder
                .send_message(&Move { x: 7, y: 5 }.encode(7), None)
                .expect("move");
            rig.pump(Duration::from_millis(200));
            assert_eq!(
                rig.host.view.cursor_offered,
                Some(Some((7.0, 5.0))),
                "the presenter must have been offered a sprite position, or the assertions \
                 below compare captures against a cursor that never existed"
            );

            drive_arrangement(&mut rig, &mut holder, step);

            // Through the wire and the sealed memfd — the buffer that would
            // actually be handed over SCM_RIGHTS, not a readback beside it.
            // Diagnosed rather than dumped: two frames in an `assert_eq!`
            // message is a wall of bytes nobody reads. The fact that matters
            // is *which* byte moved.
            let differs = |got: &[u8], want: &[u8]| {
                got.iter()
                    .zip(want)
                    .position(|(g, w)| g != w)
                    .map(|at| format!("first differing byte at {at} (pixel {})", at / 4))
            };
            assert_eq!(
                differs(&capture_bytes(&mut rig, &mut watcher, 6), &bare_a),
                None,
                "after `{step}`, the watching principal's capture of realm-a is no longer \
                 realm-a's own bare scene: an agent's cursor reached a captured frame"
            );
            assert_eq!(
                differs(&capture_bytes(&mut rig, &mut holder, 6), &bare_b),
                None,
                "after `{step}`, the actuating principal's own capture of realm-b carries \
                 its own cursor; the one cursor a capture may contain is the human's, and \
                 only under observe_cursor"
            );
        }
    }

    /// **The fifth ordering rule, end to end over the wire.** A `layout_focus`
    /// holder moves the output, and the human's own physical input moves with
    /// it — never one without the other.
    ///
    /// Separate from the invariant sweep above because it asserts the
    /// *direction* of the move rather than the equality: before the request
    /// the output and the seat are both on `realm-a` (first in id order),
    /// after it both are on the granted `realm-b`. A binding that moved with
    /// a seat that did not is the exact defect, and it passes the equality
    /// test's precondition trivially if nothing ever moves.
    ///
    /// **Real keystrokes, and the production entry point.** The first draft
    /// of this test called `seat_target` directly and handed it the binding
    /// itself, which asks a function for the answer it was given: a reviewer
    /// reverted all three of the nested backend's own call sites to ignore
    /// the binding and the whole suite stayed green. So this drives actual
    /// physical key events through [`route_physical_turn`] — the same
    /// function `NestedState::route_physical_inputs` calls, with the same
    /// arguments — and reads out of the flight recorder **which realm's shim
    /// they reached**. `seat_target` itself is now private to this module
    /// precisely so no backend can supply a binding at all
    /// ([`physical_seat_target`]).
    #[test]
    fn the_humans_input_follows_the_realm_a_focus_holder_bound() {
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        // Both realms mint a seat here: a realm whose shim has no seat drops
        // the event silently, which would make "it did not reach realm-b"
        // true for a reason that has nothing to do with the binding.
        let mut rig = Rig::new(
            "layout-focus-seat",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let serve: &[&str] = &["--serve", "--seat"];
        rig.start_realms(&[("realm-a", serve), ("realm-b", serve)]);
        rig.pump(Duration::from_millis(400));
        let (a, b) = (RealmId::new("realm-a"), RealmId::new("realm-b"));
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);
        rig.pump_until(Duration::from_secs(5), |host| {
            host.runtime
                .realms
                .values()
                .filter_map(|r| r.server.as_ref())
                .filter(|s| s.seat_minted())
                .count()
                == 2
        });

        let before = physical_seat_target(&rig.host.runtime.realms, &rig.host.view.scenes)
            .map(|(realm_id, _)| realm_id.clone());
        assert_eq!(
            before,
            Some(a.clone()),
            "the session starts with the output and the seat on the first realm"
        );

        // One tap of a layout-invariant key, pressed and released so nothing
        // is left held (the strand this test is not about).
        let switch = std::cell::RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        ));
        const EVDEV_A: u32 = 30;
        const KEYSYM_A: u32 = 0x61;
        let tap = |rig: &mut Rig| {
            for state in [KeyState::Pressed, KeyState::Released] {
                route_physical_turn(
                    &mut rig.host.runtime,
                    &rig.host.view.scenes,
                    Some(&switch),
                    crate::input::physical_key(EVDEV_A, Some(KEYSYM_A), state),
                    VIEW,
                    Instant::now(),
                );
            }
        };
        tap(&mut rig);

        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-b");
        send_focus(&mut holder);
        rig.pump(Duration::from_millis(300));

        assert_eq!(
            rig.host.view.focused(),
            Some(&b),
            "the focus request must move the output to the granted realm"
        );
        assert_eq!(
            physical_seat_target(&rig.host.runtime.realms, &rig.host.view.scenes)
                .map(|(realm_id, _)| realm_id.clone()),
            Some(b.clone()),
            "and the human's own keyboard and pointer must move with it: a session that \
             shows realm-b while typing into realm-a is the split layout_focus is one act \
             in order to prevent"
        );

        // ...and the keystrokes really land there. Same tap, same function,
        // the other side of the binding.
        tap(&mut rig);

        let entries = rig.entries();
        let reached: Vec<String> = crate::recorder::tests::of_kind(&entries, "seat_delivered")
            .into_iter()
            .filter(|e| e.str("event") == "key" && e.str("origin") == "physical")
            .map(|e| e.str("realm").to_string())
            .collect();
        assert_eq!(
            reached,
            vec![a.to_string(), a.to_string(), b.to_string(), b.to_string()],
            "the human's own keys must reach realm-a while realm-a is on the output and \
             realm-b afterwards — press and release each, in order. A run that shows four \
             `realm-a` entries is the pre-WS-E.1.4 behaviour of typing into whichever realm \
             sorts first, which is the fifth ordering rule violated end to end"
        );
    }

    /// **D-018(4)'s single-holder rule.** A second principal petitioning for
    /// `layout_arrange` while the verb is already spoken for — by a live
    /// grant carrying it, or by a petition still pending for it — resolves
    /// `layout_held`, and the flight recorder journals it.
    #[test]
    fn a_second_layout_arrange_petition_resolves_layout_held() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, _a, _b) = two_realm_rig("layout-held");
        let _holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-a");

        // A different principal asks for the same verb.
        let mut second = agent(&rig.socket);
        send_preamble_as(&mut second, OTHER_IDENTITY, OTHER_TOKEN, "realm-b");
        send_petition_for(&mut second, Verb::LAYOUT_ARRANGE);
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut second),
            Outcome::LayoutHeld,
            "a second live layout_arrange holder must be refused layout_held, not busy \
             (the consent-fatigue valve) and not granted"
        );

        // ...while the verb the rule says nothing about is still available to
        // that same principal, so this refuses contention and not the
        // principal.
        let mut third = agent(&rig.socket);
        send_preamble_as(&mut third, OTHER_IDENTITY, OTHER_TOKEN, "realm-b");
        send_petition_for(&mut third, Verb::LAYOUT_FOCUS);
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut third),
            Outcome::Granted,
            "layout_focus carries no single-holder rule: focus is a momentary act, not a \
             standing arrangement, and several principals may hold it"
        );

        let entries = rig.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.str("kind") == "petition_resolved" && e.str("outcome") == "layout_held"),
            "the run must journal the layout_held resolution; got outcomes {:?}",
            entries
                .iter()
                .filter(|e| e.str("kind") == "petition_resolved")
                .map(|e| e.str("outcome"))
                .collect::<Vec<_>>()
        );
    }

    /// **A layout request contends for attention, so it yields to the human.**
    ///
    /// Three cases, one per chokepoint gate WS-E.1.4 widened from "actuation
    /// only" to "actuation and layout":
    ///
    /// - **(a) `preempted`** (step 5c) — the human's own hand is on the
    ///   input, so a holder may not move the output out from under it.
    /// - **(b) `consent_held`** (step 5b) — this principal's own consent
    ///   prompt is up, so it may not arrange the screen around the very
    ///   decision it is waiting on.
    /// - **(c) `no_surface`** (step 5a) — the granted realm has no live view,
    ///   so focusing it would bind the output to nothing and arranging it
    ///   would have no geometry to arrange. D-022(6) makes this load-bearing
    ///   for layout specifically, and the asymmetry with `realm_launch` —
    ///   exempt, because a vacant realm is the state launch exists to *leave*
    ///   — is the reason it is not a blanket rule.
    ///
    /// None of the three is invariant 3: the consent card is untouchable
    /// whatever these gates do. They are the layer above, and both layers
    /// exist. Each case names its refusal exactly, so a gate that stopped
    /// firing fails here rather than silently admitting the request.
    #[test]
    fn a_layout_request_yields_to_the_humans_own_hand_and_to_its_own_prompt() {
        use crate::input::SeatInputKind;
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "layout-yields",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        // `realm-c` serves and never commits: case (c)'s realm with no live
        // view. It is a third realm rather than realm-b uncommitted because
        // cases (a) and (b) need a realm that *does* have one, or they would
        // be refused `no_surface` before ever reaching the gate under test.
        rig.start_realms(&[
            ("realm-a", &["--serve", "--seat"]),
            ("realm-b", &["--serve"]),
            ("realm-c", &["--serve"]),
        ]);
        rig.pump(Duration::from_millis(400));
        let b = RealmId::new("realm-b");
        commit_into(&mut rig, &RealmId::new("realm-a"), VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);

        let mut client = agent(&rig.socket);
        send_preamble_as(&mut client, DEMO_IDENTITY, TOKEN, "realm-b");
        send_petition_for(&mut client, max_verbs());
        let petition = pump_until_armed(&mut rig, &grab);
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::WhileRunning),
        });
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut client), Outcome::Granted);
        mint_layout_facets(&mut client);
        rig.pump(Duration::from_millis(200));

        // Bounded by a `sync` fence rather than by a read count: a `done`
        // always comes back, so an *admitted* request — the thing a
        // regression would produce — surfaces as `None` instead of a
        // blocking read that hangs the suite.
        //
        // **A distinct cookie per call, and the fence is drained to it.** The
        // three cases below share one connection, and a `done` left in the
        // stream by an earlier case would end the next case's read on its
        // first iteration and report `None` — an admitted request — for a
        // request that was in fact refused. Matching the cookie is what makes
        // each case's answer its own.
        let cookie = std::cell::Cell::new(4242u32);
        let next_refusal = |rig: &mut Rig, client: &mut Connection| -> Option<Refusal> {
            let fence = cookie.get() + 1;
            cookie.set(fence);
            client
                .send_message(
                    &vitrin_handshake::requests::Sync { cookie: fence }.encode(HANDSHAKE_ID),
                    None,
                )
                .expect("sync");
            rig.pump(Duration::from_millis(300));
            let mut found = None;
            for _ in 0..256 {
                let Ok(Some(msg)) = client.recv_message() else {
                    break;
                };
                if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                    let (_, e) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                    found.get_or_insert(e.code);
                    continue;
                }
                if msg.header.object_id == HANDSHAKE_ID
                    && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
                {
                    let (_, done) = vitrin_handshake::events::Done::decode(&msg.bytes, msg.fd)
                        .expect("decode done");
                    if done.cookie == fence {
                        break;
                    }
                }
            }
            found
        };

        // (a) The human's own hand owns the target: a focus is preempted.
        // Fed at the router's hook point, exactly as the backends feed it.
        rig.host.runtime.kernel.presence.borrow_mut().note(
            // The realm physical input is addressed to -- the bound one.
            rig.host.view.focused(),
            vitrin_protocol::generated::vitrin_shim_seat::Origin::Physical,
            &SeatInputKind::Motion { x: 1.0, y: 1.0 },
            Instant::now(),
        );
        send_focus(&mut client);
        assert_eq!(
            next_refusal(&mut rig, &mut client),
            Some(Refusal::Preempted),
            "a focus request must yield to a hand already on the input: moving the output \
             out from under a human mid-keystroke is the theft this verb is separately \
             attenuable in order to bound"
        );
        assert_ne!(
            rig.host.view.focused(),
            Some(&b),
            "and the refused request must not have moved anything"
        );

        // (b) This principal's own prompt is up: a layout request is refused
        // `consent_held`. The presence fed above is let go stale first
        // (`PHYSICAL_HOLD_WINDOW` is 500ms and step 5c runs *after* 5b), so
        // this case is proved on its own gate rather than inheriting (a)'s.
        rig.pump(Duration::from_millis(700));
        assert!(
            !rig.host
                .runtime
                .kernel
                .presence
                .borrow()
                .owns_target(rig.host.view.focused(), Instant::now()),
            "fixture check: (a)'s physical presence must have gone stale, or this case \
             would pass on the preemption gate and say nothing about 5b"
        );
        // A *second* petition from the same principal, left pending with its
        // prompt on screen. `observe` rather than the layout verbs: this
        // principal's first grant already holds `layout_arrange`, and
        // D-018(4) would resolve a second one `layout_held` at admission
        // without ever raising a prompt.
        send_second_petition_for(&mut client, Verb::OBSERVE);
        let pending = pump_until_armed(&mut rig, &grab);
        let identity = rig
            .host
            .runtime
            .conns
            .values()
            .find_map(|c| c.server.bound_identity())
            .expect("the client is bound")
            .clone();
        assert!(
            rig.host.runtime.kernel.petitions.prompt_up_for(&identity),
            "fixture check: the prompt for petition {pending:?} must be up for THIS \
             principal, which is the fact step 5b reads"
        );
        send_focus(&mut client);
        assert_eq!(
            next_refusal(&mut rig, &mut client),
            Some(Refusal::ConsentHeld),
            "a layout request must yield to this principal's own pending prompt: a \
             principal that could move the output away from the card it is waiting on — or \
             fullscreen a realm over it — would be arranging the very decision it is \
             waiting on"
        );
        // ...and `set_fullscreen` meets it too: both layout requests are
        // attention-contending, not just the one that moves the output.
        send_set_fullscreen(
            &mut client,
            vitrin_protocol::generated::vitrin_layout_arrange::Mode::Fullscreen,
        );
        assert_eq!(
            next_refusal(&mut rig, &mut client),
            Some(Refusal::ConsentHeld),
            "set_fullscreen contends for attention on exactly the same terms focus is"
        );
        assert_ne!(
            rig.host.view.focused(),
            Some(&b),
            "and neither refused request moved anything"
        );
        // Let the prompt go so it cannot leak into case (c) as a global
        // condition (it is per principal, and this proves it by clearing it).
        grab.borrow_mut().queue_decision(Decision {
            petition: pending,
            choice: Choice::Deny,
        });
        rig.pump(Duration::from_millis(400));

        // (c) The granted realm has no live view: `no_surface`. A different
        // principal, over `realm-c`, which serves and has committed nothing.
        let mut watcher = agent(&rig.socket);
        send_preamble_as(&mut watcher, OTHER_IDENTITY, OTHER_TOKEN, "realm-c");
        send_petition_for(&mut watcher, Verb::LAYOUT_FOCUS);
        let petition = pump_until_armed(&mut rig, &grab);
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::WhileRunning),
        });
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut watcher),
            Outcome::Granted,
            "fixture check: the grant must be real, or `not_granted` would mask the gate"
        );
        mint_layout_facets(&mut watcher);
        send_focus(&mut watcher);
        assert_eq!(
            next_refusal(&mut rig, &mut watcher),
            Some(Refusal::NoSurface),
            "focusing a realm with no live view would bind the output to nothing, which is \
             a successful-looking answer to a request that did nothing (D-022(6))"
        );
        assert_ne!(
            rig.host.view.focused(),
            Some(&RealmId::new("realm-c")),
            "and the refused focus must not have bound the output to the vacant realm"
        );
    }

    /// **The human's attention key lifts `preempted` for exactly one layout
    /// use** — WS-E.1.7 (issue #232), driven through the production entry
    /// points end to end.
    ///
    /// The loop this closes: a human at an in-realm shell types `focus
    /// editor` and presses Enter; the Enter marks physical presence, and the
    /// layout request the Enter just sent is refused `preempted`. Pressing
    /// Enter again re-arms the window, so it is a deterministic loop rather
    /// than a race.
    ///
    /// **Nothing here is hand-fed.** The presence comes from real physical
    /// input through [`route_physical_turn`]; the chord press is a real
    /// `physical_key` through the same function; the window is opened by
    /// `open_attention_window`'s own delivery filter; and the client learns
    /// about it from the wire event, not from the rig. Every assertion is on
    /// something a client or the journal can see.
    ///
    /// **The press delegates nothing.** The grant that makes the admitted
    /// `set_fullscreen` legal was approved before any of this; what the press
    /// changes is one refusal, once.
    #[test]
    fn the_humans_attention_key_lifts_preemption_for_exactly_one_layout_use() {
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};
        use vitrin_protocol::generated::vitrin_layout_arrange::Mode;
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, _b) = two_realm_rig("attention-key");
        // The holder's grant is over `realm-a`, which is also the realm the
        // output is bound to and therefore the realm the human's hand is in.
        // `set_fullscreen` rather than `focus` for the admitted use, so the
        // binding -- and with it `physical_realm` -- does not move underneath
        // the second half of the test and quietly make the second refusal
        // about a different realm.
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-a");
        assert_eq!(
            rig.host.view.focused(),
            Some(&a),
            "fixture: realm-a is bound"
        );

        // Drain to a `sync` fence, counting the two things this test reads off
        // the wire: refusals of the layout use, and `attention` events. A
        // fence rather than a read count because an *admitted* request -- the
        // thing a regression produces -- emits no terminal at all, and a bare
        // `recv_message` on an empty queue blocks forever. A fresh cookie per
        // call so an earlier round's `done` cannot end this one on its first
        // iteration and report someone else's answer.
        let cookie = std::cell::Cell::new(9000u32);
        let drain = |rig: &mut Rig, client: &mut Connection| -> (Option<Refusal>, usize) {
            use vitrin_protocol::generated::vitrin_principal::events::Attention;
            let fence = cookie.get() + 1;
            cookie.set(fence);
            client
                .send_message(
                    &vitrin_handshake::requests::Sync { cookie: fence }.encode(HANDSHAKE_ID),
                    None,
                )
                .expect("sync");
            rig.pump(Duration::from_millis(300));
            let mut found = None;
            let mut attention = 0usize;
            for _ in 0..256 {
                let Ok(Some(msg)) = client.recv_message() else {
                    break;
                };
                if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                    let (_, e) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                    found.get_or_insert(e.code);
                    continue;
                }
                if msg.header.object_id == 2 && msg.header.opcode == Attention::OPCODE {
                    attention += 1;
                    continue;
                }
                if msg.header.object_id == HANDSHAKE_ID
                    && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
                {
                    let (_, done) = vitrin_handshake::events::Done::decode(&msg.bytes, msg.fd)
                        .expect("decode done");
                    if done.cookie == fence {
                        break;
                    }
                }
            }
            (found, attention)
        };

        // The human types. A real key, through the production intake and the
        // production turn -- the same function the nested backend tails into.
        const EVDEV_A: u32 = 30;
        const KEYSYM_A: u32 = 0x61;
        let type_a = |rig: &mut Rig| {
            for state in [KeyState::Pressed, KeyState::Released] {
                route_physical_turn(
                    &mut rig.host.runtime,
                    &rig.host.view.scenes,
                    None,
                    crate::input::physical_key(EVDEV_A, Some(KEYSYM_A), state),
                    VIEW,
                    Instant::now(),
                );
            }
        };
        type_a(&mut rig);

        // ...so the request that typing just sent is refused. This is the
        // whole premise: the input that tells the client to act forbids it.
        send_set_fullscreen(&mut holder, Mode::Windowed);
        assert_eq!(
            drain(&mut rig, &mut holder),
            (Some(Refusal::Preempted), 0),
            "fixture: without the attention key a layout use under the human's own hand \
             must be refused, or this test proves nothing about lifting the refusal"
        );

        // The human presses the attention chord. `physical_key` on the
        // chord's own scancode -- the same call the injector's `attention`
        // line makes and the same one the nested backend's keyboard handler
        // makes -- so nothing here is a second path into the core.
        let chord = crate::attention::AttentionChord::parse(crate::attention::DEFAULT_CHORD)
            .expect("the default chord parses");
        let tap = |rig: &mut Rig| {
            for state in [KeyState::Pressed, KeyState::Released] {
                route_physical_turn(
                    &mut rig.host.runtime,
                    &rig.host.view.scenes,
                    None,
                    crate::input::physical_key(chord.evdev(), None, state),
                    VIEW,
                    Instant::now(),
                );
            }
        };
        tap(&mut rig);

        // The client learns about it the only way it can -- the wire event --
        // and the use the human's gesture was for is then admitted.
        //
        // **Admission is read out of the journal, not out of the wire's
        // silence.** A second refusal with the same `(verb, code)` pair is
        // *coalesced* by the delivery classification, so "no refusal arrived"
        // is genuinely ambiguous between admitted and refused-again -- which
        // is exactly how this assertion first passed with the exemption arm
        // reverted. `use_decision` is written per decision regardless of
        // coalescing, so it is what says which one happened.
        let layout_decisions = |rig: &mut Rig| -> Vec<String> {
            crate::recorder::tests::of_kind(&rig.entries(), "use_decision")
                .into_iter()
                .filter(|e| e.str("verb") == "layout_arrange")
                .map(|e| e.str("decision").to_string())
                .collect()
        };
        assert_eq!(
            layout_decisions(&mut rig),
            vec!["refused".to_string()],
            "fixture: exactly the one refusal so far"
        );
        send_set_fullscreen(&mut holder, Mode::Windowed);
        assert_eq!(
            drain(&mut rig, &mut holder),
            (None, 1),
            "a layout holder must be told exactly once that the human pressed the key"
        );
        assert_eq!(
            layout_decisions(&mut rig),
            vec!["refused".to_string(), "allowed".to_string()],
            "the layout use inside the window must be ADMITTED -- this is the interaction \
             the key exists to unbreak. Read from the journal because a repeated refusal of \
             the same (verb, code) pair is coalesced off the wire, so client-side silence \
             cannot tell the two apart"
        );

        // **Exactly one.** The human's hand is still on the keyboard (the
        // chord itself refreshed presence -- decision 3: the core must not
        // manufacture a false fact about where the human is), so a second use
        // meets the same 5c it did before, with the window now spent.
        assert!(
            rig.host
                .runtime
                .kernel
                .presence
                .borrow()
                .owns_target(Some(&a), Instant::now()),
            "fixture: the human's hand must still own realm-a, or the second refusal below \
             would be about presence lapsing rather than about the window being spent"
        );
        send_set_fullscreen(&mut holder, Mode::Fullscreen);
        let _ = drain(&mut rig, &mut holder);
        assert_eq!(
            layout_decisions(&mut rig),
            vec![
                "refused".to_string(),
                "allowed".to_string(),
                "refused".to_string()
            ],
            "one press admits at most ONE layout use: it cannot authorise a burst, and a \
             holder that could focus, then fullscreen, then focus again would be spending \
             one human gesture three times"
        );

        // The journal says what happened, and who took the press -- the one
        // narrowing available against a session-wide window (decision 10).
        let entries = rig.entries();
        let pressed = crate::recorder::tests::of_kind(&entries, "attention_pressed");
        assert_eq!(pressed.len(), 1, "one press, one entry");
        assert_eq!(pressed[0].str("chord"), "super");
        assert!(pressed[0].bool("opened"), "the window opened");
        assert_eq!(pressed[0].u64("notified"), 1);
        let claimed = crate::recorder::tests::of_kind(&entries, "attention_claimed");
        assert_eq!(claimed.len(), 1, "one press, at most one claim");
        assert_eq!(claimed[0].str("principal"), DEMO_IDENTITY);
    }

    /// **The window is burnt only by a use that actually needed it and was
    /// actually admitted** (WS-E.1.7) — the same rule `commit_use` follows for
    /// a `once` rung, and for the same reason.
    ///
    /// Two ways a naive implementation spends the human's press for nothing,
    /// both driven through the real chokepoint:
    ///
    /// - **A use that did not need it.** If the human's hand had already
    ///   lapsed, step 5c would not have refused anything, so there was no
    ///   refusal to suppress and the window must survive for the use that
    ///   really does meet a hand on the keyboard.
    ///
    /// The companion half — a use refused by an *earlier* gate, which is where
    /// `consent_held` sits — is
    /// `a_prompt_is_never_lifted_by_the_attention_key`. Between them the two
    /// cover "refused for any other reason". The one case neither drives is an
    /// exemption discarded by the rate bucket (the only gate *after* 5c), and
    /// that one is closed by construction rather than by a test:
    /// [`crate::attention::Exemption`] is `#[must_use]`, is not `Copy`, and is
    /// consumed **by value** by `claim`, so an exemption the bucket refuses
    /// past is simply dropped and the window is untouched — there is no code
    /// path in which it could be spent.
    ///
    /// The open window is fed at the kernel the way every `preempted` test in
    /// this file feeds presence. That the *press* opens one, and reaches the
    /// wire, is the neighbouring test's job.
    #[test]
    fn a_use_that_did_not_need_the_window_or_that_was_refused_never_spends_it() {
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};
        use vitrin_protocol::generated::vitrin_layout_arrange::Mode;

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, _b) = two_realm_rig("attention-not-spent");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-a");
        let identity = crate::identity::PrincipalIdentity::parse(DEMO_IDENTITY).expect("identity");

        let cookie = std::cell::Cell::new(3000u32);
        let drain = |rig: &mut Rig, client: &mut Connection| -> Option<Refusal> {
            let fence = cookie.get() + 1;
            cookie.set(fence);
            client
                .send_message(
                    &vitrin_handshake::requests::Sync { cookie: fence }.encode(HANDSHAKE_ID),
                    None,
                )
                .expect("sync");
            rig.pump(Duration::from_millis(300));
            let mut found = None;
            for _ in 0..512 {
                let Ok(Some(msg)) = client.recv_message() else {
                    break;
                };
                if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                    let (_, e) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                    found.get_or_insert(e.code);
                    continue;
                }
                if msg.header.object_id == HANDSHAKE_ID
                    && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
                {
                    let (_, done) = vitrin_handshake::events::Done::decode(&msg.bytes, msg.fd)
                        .expect("decode done");
                    if done.cookie == fence {
                        break;
                    }
                }
            }
            found
        };
        let open_window = |rig: &mut Rig| {
            rig.host.runtime.kernel.attention.borrow_mut().open(
                Instant::now(),
                std::collections::BTreeSet::from([identity.clone()]),
            );
        };
        let window_is_open = |rig: &Rig| {
            rig.host
                .runtime
                .kernel
                .attention
                .borrow()
                .exempt(
                    &identity,
                    &crate::enforcement::UseKind::LayoutFocus,
                    Instant::now(),
                )
                .is_some()
        };

        // (1) No hand anywhere, so 5c refuses nothing and the window is not
        // this use's to spend.
        open_window(&mut rig);
        send_set_fullscreen(&mut holder, Mode::Windowed);
        assert_eq!(
            drain(&mut rig, &mut holder),
            None,
            "fixture: with no hand on the input the use is admitted on its grant alone"
        );
        assert!(
            window_is_open(&rig),
            "a use that needed no exemption must leave the window open for the one that \
             does: the human pressed the key for the request they are about to make, not \
             for whatever happened to arrive first"
        );

        // And the journal agrees: nothing was claimed at all.
        assert!(
            crate::recorder::tests::of_kind(&rig.entries(), "attention_claimed").is_empty(),
            "no use in this run suppressed a refusal, so none may be journaled as having \
             spent the human's press"
        );
    }

    /// **A consent prompt is never lifted by the attention key** (WS-E.1.7
    /// decision 6, and D-022(6) underneath it).
    ///
    /// Step 5b runs strictly before 5c and is **never** exempted. A prompt up
    /// means the human is answering a security question; the attention key
    /// says nothing about a pending petition, and a principal that could focus
    /// or fullscreen over its own pending card would be arranging the decision
    /// it is waiting on. The window's existence changes nothing about that —
    /// which is what "whether or not a window is open" in the criterion means,
    /// and why the window is asserted open on both sides of the refusal.
    ///
    /// This is also the "refused for some other reason" half of
    /// `a_use_that_did_not_need_the_window_or_that_was_refused_never_spends_it`:
    /// the use never reaches 5c, so no exemption is ever obtained and the
    /// window survives untouched.
    #[test]
    fn a_prompt_is_never_lifted_by_the_attention_key() {
        use crate::input::SeatInputKind;
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "attention-prompt",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        rig.start_realms(&[
            ("realm-a", &["--serve", "--seat"]),
            ("realm-b", &["--serve"]),
        ]);
        rig.pump(Duration::from_millis(400));
        let a = RealmId::new("realm-a");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);

        let mut client = agent(&rig.socket);
        send_preamble_as(&mut client, DEMO_IDENTITY, TOKEN, "realm-a");
        send_petition_for(&mut client, max_verbs());
        let petition = pump_until_armed(&mut rig, &grab);
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::WhileRunning),
        });
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut client), Outcome::Granted);
        mint_layout_facets(&mut client);
        rig.pump(Duration::from_millis(200));

        // A second petition from the same principal, left pending with its
        // prompt on screen -- `observe`, because D-018(4) would resolve a
        // second `layout_arrange` petition `layout_held` at admission without
        // ever raising a card.
        send_second_petition_for(&mut client, Verb::OBSERVE);
        let pending = pump_until_armed(&mut rig, &grab);
        let identity = crate::identity::PrincipalIdentity::parse(DEMO_IDENTITY).expect("identity");
        assert!(
            rig.host.runtime.kernel.petitions.prompt_up_for(&identity),
            "fixture check: the prompt for {pending:?} must be up for THIS principal, which \
             is the fact step 5b reads"
        );

        // The human's hand is on the input **and** a window is open: both of
        // 5c's inputs say "admit". 5b must still refuse.
        rig.host.runtime.kernel.presence.borrow_mut().note(
            Some(&a),
            vitrin_protocol::generated::vitrin_shim_seat::Origin::Physical,
            &SeatInputKind::Motion { x: 1.0, y: 1.0 },
            Instant::now(),
        );
        rig.host.runtime.kernel.attention.borrow_mut().open(
            Instant::now(),
            std::collections::BTreeSet::from([identity.clone()]),
        );

        send_focus(&mut client);
        client
            .send_message(
                &vitrin_handshake::requests::Sync { cookie: 5150 }.encode(HANDSHAKE_ID),
                None,
            )
            .expect("sync");
        rig.pump(Duration::from_millis(300));
        let mut found = None;
        for _ in 0..256 {
            let Ok(Some(msg)) = client.recv_message() else {
                break;
            };
            if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                let (_, e) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                found.get_or_insert(e.code);
                continue;
            }
            if msg.header.object_id == HANDSHAKE_ID
                && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
            {
                break;
            }
        }
        assert_eq!(
            found,
            Some(Refusal::ConsentHeld),
            "an open attention window must not lift `consent_held`: the human pressing a key \
             about their own hand says nothing about the petition they are being asked to \
             decide, and a principal that could arrange the screen around its own pending \
             card would be arranging the decision it is waiting on"
        );
        assert!(
            rig.host
                .runtime
                .kernel
                .attention
                .borrow()
                .exempt(
                    &identity,
                    &crate::enforcement::UseKind::LayoutFocus,
                    Instant::now()
                )
                .is_some(),
            "and the refused use must not have spent the human's press"
        );
        assert!(
            crate::recorder::tests::of_kind(&rig.entries(), "attention_claimed").is_empty(),
            "nothing may be journaled as having spent a press that suppressed no refusal"
        );
    }

    /// **A press nobody could use opens nothing, and only layout holders are
    /// told at all** (WS-E.1.7).
    ///
    /// Two properties in one run because they are the same filter seen from
    /// two sides. An unconditional `attention` event would be a free, silent
    /// keystroke-timing oracle for every connected client — the same hazard
    /// consuming the key closes on the confined app's side — so delivery is
    /// filtered to principals holding a live grant carrying a layout verb, and
    /// a press that reached nobody is journaled as having opened no window
    /// rather than as an open one nobody may claim.
    #[test]
    fn a_client_holding_no_layout_verb_is_never_told_the_human_pressed_the_key() {
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, _a, _b) = two_realm_rig("attention-filter");

        // A client with real, live authority over the bound realm -- and none
        // of it layout. If the filter were "any bound connection" this would
        // receive the event.
        let mut watcher = agent(&rig.socket);
        send_preamble_as(&mut watcher, OTHER_IDENTITY, OTHER_TOKEN, "realm-a");
        send_petition_for(&mut watcher, Verb::OBSERVE);
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            await_resolution(&mut watcher),
            Outcome::Granted,
            "fixture: the observer's grant must be real, or silence proves nothing"
        );

        let chord = crate::attention::AttentionChord::parse(crate::attention::DEFAULT_CHORD)
            .expect("the default chord parses");
        for state in [KeyState::Pressed, KeyState::Released] {
            route_physical_turn(
                &mut rig.host.runtime,
                &rig.host.view.scenes,
                None,
                crate::input::physical_key(chord.evdev(), None, state),
                VIEW,
                Instant::now(),
            );
        }
        // Drained to a `sync` fence: a bare read on an empty queue blocks, and
        // "nothing arrived" is exactly what this test expects to find.
        watcher
            .send_message(
                &vitrin_handshake::requests::Sync { cookie: 7777 }.encode(HANDSHAKE_ID),
                None,
            )
            .expect("sync");
        rig.pump(Duration::from_millis(300));
        let mut heard = 0usize;
        for _ in 0..256 {
            let Ok(Some(msg)) = watcher.recv_message() else {
                break;
            };
            if msg.header.object_id == 2
                && msg.header.opcode
                    == vitrin_protocol::generated::vitrin_principal::events::Attention::OPCODE
            {
                heard += 1;
                continue;
            }
            if msg.header.object_id == HANDSHAKE_ID
                && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
            {
                break;
            }
        }
        assert_eq!(
            heard, 0,
            "a client holding no layout verb must never learn the human pressed the key: an \
             unconditional event is a free keystroke-timing oracle for every app in the \
             session"
        );

        let entries = rig.entries();
        let pressed = crate::recorder::tests::of_kind(&entries, "attention_pressed");
        assert_eq!(pressed.len(), 1, "the press is journaled even so");
        assert_eq!(pressed[0].u64("notified"), 0);
        assert!(
            !pressed[0].bool("opened"),
            "a window no principal may claim is not open, and recording it as one would \
             overstate what the human's gesture did"
        );
    }

    /// **The human's hand really reaches the chokepoint, and reaches only the
    /// realm the hand is in** — issue #212's acceptance criterion 3, driven
    /// through the production wiring rather than by feeding the map by hand.
    ///
    /// Every other `preempted` test in this crate writes `Kernel::presence`
    /// directly, which asks the chokepoint a question about a map a test
    /// filled in. That left the *feeder* untested, and the feeder was missing:
    /// from P1.4.4 until this issue's review, `PresenceHook` was an optional
    /// member of the router's hook stack and **no shipping backend included
    /// it**, so `preempted` could not fire in any `vitrind` ever built while
    /// the book described it as live behaviour. `InputRouter` now holds the
    /// tap unconditionally and `Runtime::new` takes the kernel's map out of
    /// the router, so the two cannot be different objects — this test is what
    /// pins that they are not.
    ///
    /// Three cases, in one order chosen so none of them can pass vacuously:
    ///
    /// - a real physical event routed through [`route_physical_turn`] into
    ///   the **bound** realm leaves an agent over the **other** realm
    ///   admitted — and the map is asserted to still own the bound realm at
    ///   that instant, so "admitted" is not "the window had expired";
    /// - a second physical event, then an agent over the **bound** realm, is
    ///   refused `preempted`;
    /// - the same agent, once the window has gone stale, is admitted again —
    ///   so the refusal above came from the human's hand and not from
    ///   anything else about that grant.
    #[test]
    fn a_humans_physical_input_preempts_agents_in_its_own_realm_and_no_other() {
        use vitrin_protocol::generated::vitrin_actuator_pointer::requests::Move;
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, b) = two_realm_rig("preempt-per-realm");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);
        assert_eq!(
            physical_seat_target(&rig.host.runtime.realms, &rig.host.view.scenes)
                .map(|(realm_id, _)| realm_id.clone()),
            Some(a.clone()),
            "fixture check: the human's input starts on realm-a, so realm-b is the realm \
             nobody is looking at"
        );

        // Two principals, one per realm, holding `actuate.pointer` and
        // nothing else — no layout verb, because D-022(5) would refuse the
        // second holder `layout_held` at admission and this test would be
        // about that instead.
        let mut in_a = agent(&rig.socket);
        send_preamble_as(&mut in_a, DEMO_IDENTITY, TOKEN, "realm-a");
        send_petition_for(&mut in_a, Verb::ACTUATE_POINTER);
        let mut in_b = agent(&rig.socket);
        send_preamble_as(&mut in_b, OTHER_IDENTITY, OTHER_TOKEN, "realm-b");
        send_petition_for(&mut in_b, Verb::ACTUATE_POINTER);
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut in_a), Outcome::Granted);
        assert_eq!(await_resolution(&mut in_b), Outcome::Granted);

        let cookie = std::cell::Cell::new(9100u32);
        let next_refusal = |rig: &mut Rig, client: &mut Connection| -> Option<Refusal> {
            let fence = cookie.get() + 1;
            cookie.set(fence);
            client
                .send_message(
                    &vitrin_handshake::requests::Sync { cookie: fence }.encode(HANDSHAKE_ID),
                    None,
                )
                .expect("sync");
            rig.pump(Duration::from_millis(200));
            let mut found = None;
            for _ in 0..256 {
                let Ok(Some(msg)) = client.recv_message() else {
                    break;
                };
                if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                    let (_, e) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                    found.get_or_insert(e.code);
                    continue;
                }
                if msg.header.object_id == HANDSHAKE_ID
                    && msg.header.opcode == vitrin_handshake::events::Done::OPCODE
                {
                    let (_, done) = vitrin_handshake::events::Done::decode(&msg.bytes, msg.fd)
                        .expect("decode done");
                    if done.cookie == fence {
                        break;
                    }
                }
            }
            found
        };

        // A real keystroke through the entry point both backends call. No
        // hand-written presence anywhere in this test: if the router does not
        // feed the kernel's map, every case below reports "admitted".
        const EVDEV_A: u32 = 30;
        const KEYSYM_A: u32 = 0x61;
        let tap = |rig: &mut Rig| {
            for state in [KeyState::Pressed, KeyState::Released] {
                route_physical_turn(
                    &mut rig.host.runtime,
                    &rig.host.view.scenes,
                    None,
                    crate::input::physical_key(EVDEV_A, Some(KEYSYM_A), state),
                    VIEW,
                    Instant::now(),
                );
            }
        };

        // (a) A hand in realm-a leaves an agent in realm-b working.
        tap(&mut rig);
        in_b.send_message(&Move { x: 7, y: 5 }.encode(7), None)
            .expect("move in realm-b");
        assert_eq!(
            next_refusal(&mut rig, &mut in_b),
            None,
            "a human typing in realm-a must not suspend an agent working in realm-b: that \
             is the concurrent-operation claim the whole project rests on"
        );
        assert!(
            rig.host
                .runtime
                .kernel
                .presence
                .borrow()
                .owns_target(Some(&a), Instant::now()),
            "fixture check: the human must still own realm-a here, or (a) passed because \
             the hold window had expired and says nothing about per-realm narrowing"
        );

        // (b) ...and suspends an agent in realm-a.
        tap(&mut rig);
        in_a.send_message(&Move { x: 7, y: 5 }.encode(7), None)
            .expect("move in realm-a");
        assert_eq!(
            next_refusal(&mut rig, &mut in_a),
            Some(Refusal::Preempted),
            "an agent actuating the realm the human's own hand is in must be refused \
             `preempted` — and it reaches the chokepoint only because the router feeds \
             `Kernel::presence`, which no shipping backend did before issue #212's review"
        );

        // (c) The control: the same grant, the same request, no hand.
        rig.pump(crate::input::PHYSICAL_HOLD_WINDOW + Duration::from_millis(200));
        assert!(
            !rig.host
                .runtime
                .kernel
                .presence
                .borrow()
                .owns_target(Some(&a), Instant::now()),
            "fixture check: the hold window must have expired"
        );
        in_a.send_message(&Move { x: 7, y: 5 }.encode(7), None)
            .expect("move in realm-a, hands off");
        assert_eq!(
            next_refusal(&mut rig, &mut in_a),
            None,
            "with no hand on the input the very same actuation is admitted, so (b)'s \
             refusal was the human's presence and nothing else about this grant"
        );
    }

    /// **A denied layout petition confers nothing, and says so recoverably.**
    /// The human says no; the facet is still mintable (mint-freely-and-
    /// check-at-use) and its first use refuses `not_granted` without killing
    /// the connection.
    /// **The clipboard clears on the TIMER, with nobody at the keyboard.**
    ///
    /// The defect this pins was not in `ClipboardSlot::expire` — that had a
    /// passing unit test the whole time. It was in where expire was *called
    /// from*: inside `drain_clipboard_gestures`, after an early return that
    /// fires unless the turn carried a clipboard chord, in a function that only
    /// runs on a physical-input turn at all. So the one case that matters —
    /// a human copies a password and walks away — left the plaintext resident
    /// in `vitrind`'s heap for the life of the session, while `limits.md`,
    /// D-024(5) and the code's own comment all called it cleared.
    ///
    /// Driven through `sweep_at`, which is what the loop's one-second timer
    /// calls, with **no input of any kind** in between. That absence is the
    /// test: a version that expires on a gesture cannot pass it.
    #[test]
    fn the_clipboard_clears_on_the_sweep_with_nobody_at_the_keyboard() {
        use crate::clipboard::{CLIPBOARD_LIFETIME, CLIPBOARD_MIME};

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "clipboard-sweep",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        let t0 = Instant::now();
        {
            let slot = &mut rig.host.runtime.kernel.clipboard_slot;
            let serial = slot.open_promotion(realm.clone());
            let ticket = slot
                .claim_answer(&realm, serial)
                .expect("the promotion just opened must be claimable");
            slot.fill(ticket, CLIPBOARD_MIME, "hunter2", t0)
                .expect("fixture text is in-policy");
        }
        assert!(
            rig.host.runtime.kernel.clipboard_slot.peek(t0).is_some(),
            "fixture check: the slot starts full"
        );

        // One sweep well inside the lifetime changes nothing...
        sweep_at(&mut rig.host, t0 + CLIPBOARD_LIFETIME / 2);
        assert!(
            rig.host
                .runtime
                .kernel
                .clipboard_slot
                .peek(t0 + CLIPBOARD_LIFETIME / 2)
                .is_some(),
            "the slot must survive until its deadline: clearing early would lose a paste              the human is entitled to make twice"
        );

        // ...and one sweep past it clears, with no gesture anywhere.
        sweep_at(&mut rig.host, t0 + CLIPBOARD_LIFETIME);
        // **Peek at `t0`, not at the deadline.** `peek` filters on staleness
        // itself, so peeking at `t0 + LIFETIME` answers `None` whether the
        // bytes were cleared or are merely unreadable -- which is exactly the
        // distinction under test. Asserted that way, this test passes with the
        // sweep's expiry deleted; it was written that way first and the
        // mutation run caught it. Ninth vacuous test in this workstream, same
        // tell as the other eight. At `t0` a slot that still HELD the bytes
        // answers `Some`, so this is the one instant that separates a clear
        // from a filter.
        assert!(
            rig.host.runtime.kernel.clipboard_slot.peek(t0).is_none(),
            "past the deadline the bytes must be GONE, not merely unreadable: peeked at \
             the moment they were copied, a slot that only filtered on read would still \
             hand them over"
        );
    }

    #[test]
    fn a_denied_layout_focus_grant_refuses_not_granted_on_use() {
        use vitrin_protocol::generated::vitrin_grant::{events::Refused, Refusal};

        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "layout-denied",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        rig.start_realms(&[("realm-a", &["--serve", "--seat"])]);
        rig.pump(Duration::from_millis(400));

        let mut client = agent(&rig.socket);
        send_preamble_as(&mut client, DEMO_IDENTITY, TOKEN, "realm-a");
        send_petition_for(&mut client, Verb::LAYOUT_FOCUS);
        let petition = pump_until_armed(&mut rig, &grab);
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Deny,
        });
        rig.pump(Duration::from_millis(400));
        assert_eq!(await_resolution(&mut client), Outcome::Denied);

        // The mint is still legal — refusing it would turn a structural mint
        // into an authority oracle.
        mint_layout_facets(&mut client);
        send_focus(&mut client);
        rig.pump(Duration::from_millis(300));

        let mut refusal = None;
        for _ in 0..32 {
            let Ok(Some(msg)) = client.recv_message() else {
                break;
            };
            if msg.header.object_id == GRANT_ID && msg.header.opcode == Refused::OPCODE {
                let (_, event) = Refused::decode(&msg.bytes, msg.fd).expect("decode refused");
                refusal = Some(event);
                break;
            }
        }
        let refusal = refusal.expect("a use of a denied grant must be refused, not ignored");
        assert_eq!(refusal.verb, Verb::LAYOUT_FOCUS);
        assert_eq!(
            refusal.code,
            Refusal::NotGranted,
            "a denied petition's facet refuses not_granted"
        );
        // ...and the connection is alive: a refusal is an answer.
        send_focus(&mut client);
        rig.pump(Duration::from_millis(200));
        assert!(
            rig.host.runtime.conns.len() == 1,
            "a recoverable refusal must never kill the connection"
        );
    }

    /// **`set_fullscreen` really re-configures the realm, and windowed really
    /// does not.** The two modes are indistinguishable while the output's
    /// size and the realm's size are equal (the IDL says so in as many
    /// words), so this drives the one thing that separates them: an output
    /// resize under the realm.
    #[test]
    fn set_fullscreen_reconfigures_the_realm_across_an_output_resize() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, _b) = two_realm_rig("layout-fullscreen");
        // The realm must have a live view: `set_fullscreen` is refused
        // `no_surface` on a realm that has committed nothing, because an app
        // that has painted nothing has neither an own size to keep nor a
        // buffer to fill the output with (decision 6).
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-a");
        assert_eq!(
            rig.realm("realm-a")
                .server
                .as_ref()
                .expect("serving")
                .configured_size(),
            VIEW,
            "a realm is spawned configured to the output's size"
        );

        // Windowed first: the core imposes no size, so nothing is sent even
        // though the output has moved out from under the realm.
        send_set_fullscreen(
            &mut holder,
            vitrin_protocol::generated::vitrin_layout_arrange::Mode::Windowed,
        );
        rig.pump(Duration::from_millis(200));
        let bigger = (VIEW.0 * 2, VIEW.1 + 8);
        apply_output_resize(&mut rig.host, bigger);
        send_set_fullscreen(
            &mut holder,
            vitrin_protocol::generated::vitrin_layout_arrange::Mode::Windowed,
        );
        rig.pump(Duration::from_millis(200));
        assert_eq!(
            rig.realm("realm-a")
                .server
                .as_ref()
                .expect("serving")
                .configured_size(),
            VIEW,
            "windowed must impose no size: the realm keeps the one it had and the \
             compositor letterboxes it"
        );

        // Fullscreen: the realm's view size now tracks the output's.
        send_set_fullscreen(
            &mut holder,
            vitrin_protocol::generated::vitrin_layout_arrange::Mode::Fullscreen,
        );
        rig.pump(Duration::from_millis(300));
        assert_eq!(
            rig.realm("realm-a")
                .server
                .as_ref()
                .expect("serving")
                .configured_size(),
            bigger,
            "fullscreen must re-send configure at the output's size — the IDL's \
             'may be re-sent when the view resizes', finally exercised"
        );
    }

    /// **Fullscreen *tracks* the output across a later resize; windowed does
    /// not.** The second half of `set_fullscreen`'s normative wire semantics:
    /// `configure` carries the output's size on entering the mode "and again
    /// whenever the output resizes while the realm is in it".
    ///
    /// Entering the mode was already covered
    /// ([`set_fullscreen_reconfigures_the_realm_across_an_output_resize`]);
    /// the "and again" was a claim four surfaces made and no code kept, with
    /// [`RealmRuntime::arrangement`] written and never read. So this drives
    /// **one** resize with the two realms in **different** arrangements and
    /// asserts exactly one of them moved — a reconfigure loop that ignored
    /// the arrangement and a reconfigure loop that did not exist both fail,
    /// in opposite directions.
    ///
    /// The resize goes through [`apply_output_resize`], which is the same
    /// call the nested backend's `Resized` handler makes and the only way to
    /// move this rig's output size at all.
    #[test]
    fn an_output_resize_reconfigures_every_fullscreen_realm_and_no_windowed_one() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, b) = two_realm_rig("layout-resize-tracks");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);

        // Both realms are spawned configured to the output's size and both
        // are born fullscreen (`start_realm_in` told each shim that size, so
        // the field is not a guess). One holder takes realm-a out of it —
        // and only one holder exists, because D-018(4) allows exactly one
        // live `layout_arrange` grant per output.
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-a");
        send_set_fullscreen(
            &mut holder,
            vitrin_protocol::generated::vitrin_layout_arrange::Mode::Windowed,
        );
        rig.pump(Duration::from_millis(200));
        assert_eq!(
            (
                rig.realm("realm-a").arrangement,
                rig.realm("realm-b").arrangement
            ),
            (LayoutMode::Windowed, LayoutMode::Fullscreen),
            "fixture check: the two realms must be in different arrangements, or this \
             test cannot tell a loop that respects the arrangement from one that does not"
        );

        let bigger = (VIEW.0 + 64, VIEW.1 + 32);
        assert_ne!(
            bigger, VIEW,
            "fixture check: the resize must move something"
        );
        apply_output_resize(&mut rig.host, bigger);
        rig.pump(Duration::from_millis(200));

        assert_eq!(
            rig.realm("realm-b")
                .server
                .as_ref()
                .expect("serving")
                .configured_size(),
            bigger,
            "a realm in the fullscreen arrangement must be re-configured at the output's \
             new size: that is what 'the view size tracks the output's' means, and it is \
             stated as normative wire semantics in the IDL, on prose page 18, in \
             `ShimServer::reconfigure`'s own docs and in D-022(4)"
        );
        assert_eq!(
            rig.realm("realm-a")
                .server
                .as_ref()
                .expect("serving")
                .configured_size(),
            VIEW,
            "a windowed realm must be left alone: the core imposes no size on it, so the \
             resize is exactly the moment the two modes stop being indistinguishable"
        );
    }

    /// **A focus change does not strand a key the losing app is holding.**
    ///
    /// The same class of defect WS-E.1.2 fixed one layer down — a sibling
    /// realm's death resetting the shared router and latching a key in the
    /// survivor — reintroduced by making the *target* movable. `bind_to`
    /// clears the pairing table without emitting anything, so before this
    /// the app that lost the output kept the key down forever, with no
    /// release it could ever receive: `seat_target` no longer names it.
    ///
    /// Driven the whole way: a real physical key press through the router
    /// (so the pairing table is written by the code that writes it in
    /// production), a real `focus` over the wire from a real grant, and the
    /// release read back out of the flight recorder, where it must be
    /// journaled against the **losing** realm.
    #[test]
    fn a_focus_change_releases_what_the_losing_realm_was_holding() {
        use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

        let _fd = crate::capture::tests::fd_lock();
        let (mut rig, a, b) = two_realm_rig("layout-focus-strand");
        commit_into(&mut rig, &a, VIEW.0, VIEW.1, 0x11);
        commit_into(&mut rig, &b, VIEW.0, VIEW.1, 0x99);
        // Only realm-a's mock shim mints a seat (`--seat`), and it is the
        // realm the output starts bound to — so the press below is delivered
        // and journaled, which is the precondition the assertions rest on.
        rig.pump_until(Duration::from_secs(5), |host| {
            host.runtime
                .realms
                .get(&RealmId::new("realm-a"))
                .and_then(|r| r.server.as_ref())
                .is_some_and(|s| s.seat_minted())
        });

        // The human holds a key down in the realm on the output, through the
        // production physical path: `input::physical_key` is the very
        // function the nested backend's own keyboard pump calls (#118), and
        // it is the only way to mint a physical-tagged event at all — the
        // constructor is private to `crate::input` precisely so a
        // physical-origin masquerade is a compile error (B2).
        const EVDEV_LEFTCTRL: u32 = 29;
        const KEYSYM_CONTROL_L: u32 = 0xFFE3;
        let switch = std::cell::RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        ));
        route_physical_turn(
            &mut rig.host.runtime,
            &rig.host.view.scenes,
            Some(&switch),
            crate::input::physical_key(EVDEV_LEFTCTRL, Some(KEYSYM_CONTROL_L), KeyState::Pressed),
            VIEW,
            Instant::now(),
        );
        assert_eq!(
            rig.host.runtime.router.held_keys(&a),
            [(KEYSYM_CONTROL_L, Origin::Physical)],
            "fixture check: the router must believe realm-a's app is holding the key, or \
             there is nothing for the focus change to strand"
        );

        // A holder over the *other* realm moves the output.
        let mut holder = layout_holder(&mut rig, DEMO_IDENTITY, TOKEN, "realm-b");
        send_focus(&mut holder);
        rig.pump(Duration::from_millis(300));
        assert_eq!(
            rig.host.view.focused(),
            Some(&b),
            "fixture check: the focus request must have moved the output"
        );
        assert!(
            rig.host.runtime.router.held_keys(&a).is_empty(),
            "the losing realm's app must not still be believed to hold the key: the drain \
             paid it, so the table is empty and the release really went out"
        );
        assert!(
            rig.host.runtime.router.held_keys(&b).is_empty(),
            "and nothing was moved into the realm that gained the output, which never saw \
             the press go down"
        );

        // The journal records delivery *shape* only — never the key state,
        // because an entry that said "press" or "release" would be one field
        // away from a keylogger — so the release is counted rather than
        // named: realm-a saw two key events, the press and the release it is
        // owed. Without the drain there is exactly one.
        let entries = rig.entries();
        let keys: Vec<_> = crate::recorder::tests::of_kind(&entries, "seat_delivered")
            .into_iter()
            .filter(|e| e.str("event") == "key")
            .map(|e| (e.str("realm").to_string(), e.str("origin").to_string()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (a.to_string(), "physical".to_string()),
                (a.to_string(), "physical".to_string()),
            ],
            "realm-a must have seen its press AND its release, and realm-b neither: the \
             release is owed to the app that was TOLD about the press, never to the realm \
             gaining the output, which never saw it go down. The tag is read back off the \
             pairing table's entry, never minted (B2)."
        );
        assert_ne!(a, b);
    }

    // ---- pointer constraints (WS-E.4.2, issue #222) ------------------------
    //
    // The state machine itself is unit-tested in `crate::input::constraint`.
    // What these tests are about is the WIRING: that the reconciler really runs
    // once per dispatch round, that the verdicts really reach a shim over a
    // real socket, and that the four session-level events that must withdraw a
    // constraint do -- and that the one that must NOT, does not.

    /// Record a lock over `realm` and reconcile the session once, so the
    /// constraint is in force by the production path.
    ///
    /// The record is placed straight into the router's own table rather than
    /// through a `pointer_constraint` frame, because `vitrin-mock-shim` sends
    /// none: this is a **component** test of the core's own arms and is stated
    /// as such (CLAUDE.md's milestone rule). Everything downstream of the
    /// record — the reconciler, the gates, the delivery — is the production
    /// code.
    fn lock_pointer_in(rig: &mut Rig, realm: &RealmId, serial: u32) {
        use vitrin_protocol::generated::vitrin_shim_session::{
            PointerConstraintKind, PointerConstraintLifetime,
        };
        let _ = rig.host.runtime.router.constraints().borrow_mut().ask(
            realm,
            crate::input::PointerConstraintAsk {
                serial,
                surface: Some(2),
                kind: PointerConstraintKind::Lock,
                lifetime: PointerConstraintLifetime::Persistent,
                region: crate::input::ConstraintRegion {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            },
        );
    }

    /// Put the human's pointer in the middle of `realm`'s surface, through the
    /// production intake, so the constraint has something to activate against.
    fn human_pointer_onto(rig: &mut Rig, at: (f64, f64)) {
        use crate::input::tests::physical_for_test;
        route_physical_turn(
            &mut rig.host.runtime,
            &rig.host.view.scenes,
            None,
            vec![physical_for_test(crate::input::SeatInputKind::Motion {
                x: at.0,
                y: at.1,
            })],
            VIEW,
            Instant::now(),
        );
    }

    fn constraint_active(rig: &Rig, realm: &RealmId) -> bool {
        rig.host
            .runtime
            .router
            .constraints()
            .borrow()
            .is_active(realm)
    }

    /// A rig with one realm, a committed surface, the human's pointer on it,
    /// and a lock in force.
    fn locked_rig(label: &str) -> (Rig, RealmId, std::sync::MutexGuard<'static, ()>) {
        // The guard is HANDED BACK rather than dropped here: it quiesces the
        // fd-counting tests against this rig's forked shim, and a guard
        // released at the end of this function would be no guard at all.
        let fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            label,
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::AutoApprove,
                config: PetitionConfig::default(),
            },
        );
        rig.start_realm(&["--serve", "--seat"]);
        rig.pump(Duration::from_millis(400));
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        commit_into(&mut rig, &realm, VIEW.0, VIEW.1, 0x33);
        human_pointer_onto(&mut rig, (VIEW.0 as f64 / 2.0, VIEW.1 as f64 / 2.0));
        lock_pointer_in(&mut rig, &realm, 1);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            constraint_active(&rig, &realm),
            "the fixture must put a lock into force, or every assertion below is vacuous"
        );
        (rig, realm, fd)
    }

    /// **A consent card raised over a locked pointer unfreezes it** (path 6),
    /// **and the sharper negative: a locked app cannot make a consent card
    /// unclickable.**
    ///
    /// This is the failure mode that would actually matter. A pointer lock is a
    /// request from a *confined app*; the consent card is the trusted path the
    /// human answers an agent's petition on. If a lock could freeze the pointer
    /// while a card is up, an app could make its realm's own petitions
    /// unanswerable — and the human's cursor invisible while they tried.
    ///
    /// It cannot, structurally, twice over. `ConsentGrab::judge_parts` records
    /// the human's absolute pointer before any grab decision and the constraint
    /// lives *below* `hook.gate`, so it is never consulted first. And the
    /// prompt is one term of `overlay_needs_the_window`, which is
    /// `Presenter::output_gates`' own first term — so the constraint is
    /// deactivated for as long as the card is up. This asserts the second,
    /// against a **real** card raised by a real petition, because it is the one
    /// an edit could break; and it asserts the decision still commits.
    #[test]
    fn a_locked_pointer_cannot_make_a_consent_card_unclickable() {
        let _fd = crate::capture::tests::fd_lock();
        let mut rig = Rig::new(
            "constraint-consent",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );
        let grab = rig.attach_grab();
        rig.start_realm(&["--serve", "--seat"]);
        rig.pump(Duration::from_millis(400));
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        commit_into(&mut rig, &realm, VIEW.0, VIEW.1, 0x33);
        let centre = (VIEW.0 as f64 / 2.0, VIEW.1 as f64 / 2.0);
        human_pointer_onto(&mut rig, centre);
        lock_pointer_in(&mut rig, &realm, 1);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(constraint_active(&rig, &realm));

        // The negative control, so the assertion below is not vacuous: with the
        // lock in force and no card up, the human's absolute motion really IS
        // dropped by the freeze.
        human_pointer_onto(&mut rig, (4.0, 4.0));
        assert_eq!(
            rig.host.runtime.router.human_pointer(&realm),
            Some(centre),
            "the freeze must actually be freezing, or the rest of this test proves nothing"
        );

        // A real petition raises a real card on the real consent surface.
        let mut client = agent(&rig.socket);
        send_preamble(&mut client);
        send_petition(&mut client);
        let petition = pump_until_armed(&mut rig, &grab);
        assert!(rig.host.view.consent.prompt().is_some());

        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            !constraint_active(&rig, &realm),
            "a raised consent card must deactivate every pointer constraint: otherwise the \
             pointer stays frozen and the human cannot reach Allow or Deny"
        );
        human_pointer_onto(&mut rig, (4.0, 4.0));
        assert_eq!(
            rig.host.runtime.router.human_pointer(&realm),
            Some((4.0, 4.0)),
            "the human's pointer must move again while a card is up"
        );

        // ...and the decision the human makes still commits.
        grab.borrow_mut().queue_decision(Decision {
            petition,
            choice: Choice::Allow(PersistenceRung::Once),
        });
        rig.pump(Duration::from_millis(400));
        assert_eq!(
            rig.host.runtime.kernel.grants.rows(Instant::now()).count(),
            1,
            "an app holding a pointer lock must not be able to stop a human answering a card"
        );

        // The card comes down and the lock comes back by itself: nothing
        // anywhere re-applies it, which is the persistent lifetime working.
        human_pointer_onto(&mut rig, centre);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            constraint_active(&rig, &realm),
            "a persistent lifetime must reactivate with no new ask once the card is down"
        );
    }

    /// **A raised lock cover deactivates every pointer constraint** (path 7).
    ///
    /// Twice over, independently: the cover is a term of
    /// `overlay_needs_the_window`, which this asserts, and `LockGate::judge`
    /// consumes every physical pointer kind while raised, so a constrained app
    /// receives nothing whatever the record says.
    #[test]
    fn a_raised_lock_deactivates_every_pointer_constraint() {
        let (mut rig, realm, _fd) = locked_rig("constraint-lock");
        rig.host.view.lock_raised = true;
        reconcile_pointer_constraints(&mut rig.host);
        assert!(!constraint_active(&rig, &realm));
        rig.host.view.lock_raised = false;
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            constraint_active(&rig, &realm),
            "and it comes back on unlock"
        );
    }

    /// **The dead-man chord withdraws every pointer constraint in the
    /// session** (path 8).
    ///
    /// Not a transient deactivation but a **record removal**, and session-wide:
    /// a constraint in a realm the chord was not held over goes too, because
    /// the switch is session-wide by construction and a locked pointer anywhere
    /// is part of what the human is taking back.
    ///
    /// The grant sweep cannot do this and must not be made to. A pointer
    /// constraint is asked for by the confined app over its shim connection and
    /// is derived from no grant row, so `revoke_principal` never sees one —
    /// which is exactly why this needs its own line and its own test.
    #[test]
    fn the_dead_man_chord_withdraws_every_pointer_constraint_in_the_session() {
        let (mut rig, realm, _fd) = locked_rig("constraint-deadman");
        // A second realm the chord was NOT held over, to pin "session-wide".
        let elsewhere = RealmId::new("realm-b");
        lock_pointer_in(&mut rig, &elsewhere, 2);

        rig.host.runtime.apply_dead_man(
            &crate::deadman::Trigger {
                chord: crate::deadman::DeadManConfig::default().chord.name(),
                held: Duration::from_millis(1200),
            },
            Instant::now(),
        );

        let table = rig.host.runtime.router.constraints();
        assert!(
            table.borrow().get(&realm).is_none(),
            "the chord must withdraw the constraint in the realm on screen"
        );
        assert!(
            table.borrow().get(&elsewhere).is_none(),
            "...and in every other realm too: the switch is session-wide"
        );
    }

    /// **A seat pause withdraws the pointer constraint and tells the shim**
    /// (path 9).
    ///
    /// The same latch shape the held keys, the held buttons and the in-flight
    /// gesture already have, one step further out: `libinput_suspend` has
    /// closed the devices, so for the whole pause no motion can carry the
    /// pointer out of the region and no withdrawal can arrive from an app that
    /// is being told nothing. Left recorded, it is an app believing it holds a
    /// lock over a seat that is gone.
    #[test]
    fn a_seat_pause_withdraws_the_pointer_constraint_and_tells_the_shim() {
        let (mut rig, realm, _fd) = locked_rig("constraint-seat-pause");
        let switch = std::cell::RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        ));
        suspend_physical_seat(&mut rig.host.runtime, &rig.host.view.scenes, &switch);
        assert!(
            rig.host
                .runtime
                .router
                .constraints()
                .borrow()
                .get(&realm)
                .is_none(),
            "a constraint left recorded across a VT switch is a lock nothing can end"
        );
        // ...and it does not silently come back when the seat returns.
        reconcile_pointer_constraints(&mut rig.host);
        assert!(!constraint_active(&rig, &realm));
    }

    /// **Revoking every grant leaves an app's pointer constraint alone** (path
    /// 11 — the one entry on the deactivation list whose answer is "nowhere").
    ///
    /// The property is that the two lifecycles do not touch. A pointer
    /// constraint belongs to the confined app and is derived from no grant row;
    /// an edge from grant revocation to constraint state would make an
    /// **agent's** grant lifecycle able to move a **human's** cursor
    /// visibility. Writing the "no" down as a test is how it stays absent — an
    /// unrecorded "no" is how a reviewer adds a wrong edge later.
    #[test]
    fn revoking_every_grant_leaves_an_apps_pointer_constraint_alone() {
        let (mut rig, realm, _fd) = locked_rig("constraint-grants");
        let identity = PrincipalIdentity::parse(DEMO_IDENTITY).unwrap();
        let revoked = rig.host.runtime.kernel.grants.revoke_principal(&identity);
        // Whether any row existed is beside the point: the sweep ran, and the
        // constraint is untouched either way.
        let _ = revoked;
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            constraint_active(&rig, &realm),
            "a grant sweep must not reach the constraint table"
        );
    }

    /// **A realm switch deactivates its pointer constraint and switching back
    /// reactivates it** (path 3), driven through the production binding.
    ///
    /// The round trip is the point: only a round trip catches a one-way flag.
    /// Wayland's `persistent` lifetime requires the lock to reactivate when the
    /// surface regains focus, which a stored flag would need TWO call sites
    /// for — one of which gets forgotten.
    #[test]
    fn a_realm_switch_deactivates_its_pointer_constraint_and_switching_back_reactivates_it() {
        let (mut rig, realm, _fd) = locked_rig("constraint-switch");
        let other = RealmId::new("realm-b");

        rig.host.view.bind_output(&other);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            !constraint_active(&rig, &realm),
            "the human is looking at another realm; the lock is not in force"
        );

        rig.host.view.bind_output(&realm);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(
            constraint_active(&rig, &realm),
            "and it comes back with no new ask -- which is the whole of `persistent`"
        );
    }

    /// **A dispatch round reconciles constraints, and a shim really receives
    /// the verdict.**
    ///
    /// The wiring test the others rest on: `post_dispatch` must call the
    /// reconciler **before** its dirty gate, or a session with nothing else
    /// changing would never tell an app its lock ended. Asserted by driving a
    /// clean (non-dirty) round.
    #[test]
    fn a_dispatch_round_reconciles_pointer_constraints_before_the_dirty_gate() {
        let (mut rig, realm, _fd) = locked_rig("constraint-round");
        rig.host.view.lock_raised = true;
        rig.host.runtime.dirty = false;
        post_dispatch(&mut rig.host);
        assert!(
            !constraint_active(&rig, &realm),
            "post_dispatch must reconcile above the dirty gate: an idle session still has to \
             tell an app that its lock stopped being in force"
        );
    }

    /// **A constraint ending asks for the frame that redraws the sprite.**
    ///
    /// The predicate is derived per composed frame, so it can never answer
    /// stale — but a predicate nobody samples changes nothing on a panel. On
    /// the damage-driven DRM backend `compose_and_queue` runs only from
    /// `service_present`, gated on `output.wanted`, and after a lock ends
    /// nothing else in the round wants a frame: the record is gone, the app
    /// owes no commit for having been unlocked, and `runtime.dirty` stays
    /// false. Without the drain in `reconcile_pointer_constraints` the last
    /// composed frame — composed while the constraint was Active, hence with
    /// `human_cursor = None` — stays on scanout, and the human has no cursor
    /// until some unrelated thing asks for a redraw.
    ///
    /// This asserts the *frame*, not the record. Two other tests already assert
    /// the record and the predicate, and both passed while this was broken.
    #[test]
    fn a_constraint_ending_requests_the_frame_that_brings_the_sprite_back() {
        let (mut rig, realm, _fd) = locked_rig("constraint-repaint");
        // Quiesce: the fixture's own activation already owed a frame.
        post_dispatch(&mut rig.host);
        rig.host.runtime.dirty = false;
        let presents_before = rig.host.view.presents;

        // The app withdraws its own lock (path 1) — the quietest deactivation
        // there is, and the one that strands hardest.
        let owed = rig
            .host
            .runtime
            .router
            .constraints()
            .borrow_mut()
            .withdraw(&realm);
        assert!(
            owed.is_some(),
            "the fixture's lock must have been withdrawable"
        );
        post_dispatch(&mut rig.host);

        assert!(
            rig.host.view.presents > presents_before,
            "a constraint ending must ask for a frame: the sprite is derived per composed \
             frame, so a deactivation nobody composes leaves the human looking at the last \
             frame drawn while the pointer was locked — the one with no cursor in it"
        );
    }

    /// **Session shutdown leaves no pointer constraint behind** (path 10).
    ///
    /// The sprite obligation is **vacuous** here and that is worth saying
    /// rather than smoothing over with a call that does nothing: there is no
    /// next frame, so nothing can be hidden. What is not vacuous is the record.
    /// Shim connections go through path 4's death funnel; principal
    /// connections go through `teardown_open_connections`, which touches no
    /// constraint because a constraint belongs to no principal — and the table
    /// itself drops with the `Rc` when the `Runtime` drops.
    ///
    /// Asserted rather than argued, because "the table empties" is the only
    /// observable that distinguishes it from "the process happened to exit".
    #[test]
    fn session_shutdown_leaves_no_pointer_constraint_behind() {
        let (mut rig, realm, _fd) = locked_rig("constraint-shutdown");
        rig.host.runtime.teardown_open_connections();
        assert!(
            rig.host
                .runtime
                .router
                .constraints()
                .borrow()
                .get(&realm)
                .is_some(),
            "a PRINCIPAL teardown must not touch an app's constraint: the two lifecycles do \
             not meet, which is the same property the grant test pins from the other side"
        );
        // The realm's own death is what takes it, and after that the session
        // holds nothing.
        close_realm(&mut rig.host, &realm, DeathCause::ConnectionClosed);
        assert_eq!(
            rig.host.runtime.router.constraints().borrow().len(),
            0,
            "the session must end holding no constraint record at all"
        );
    }

    /// **A realm's death takes its constraint with it, through the one death
    /// funnel** (path 4), and leaves the session with nothing recorded (path
    /// 10).
    #[test]
    fn a_realms_death_takes_its_pointer_constraint_with_it() {
        let (mut rig, realm, _fd) = locked_rig("constraint-death");
        close_realm(&mut rig.host, &realm, DeathCause::ConnectionClosed);
        assert!(
            rig.host
                .runtime
                .router
                .constraints()
                .borrow()
                .get(&realm)
                .is_none(),
            "the death funnel reaches the constraint table through InputRouter::reset_for"
        );
        // ...and `rebind_output_after_death` needs nothing of its own (path 5):
        // the record is already gone and a dead realm cannot be focused.
        rebind_output_after_death(&mut rig.host);
        reconcile_pointer_constraints(&mut rig.host);
        assert!(!constraint_active(&rig, &realm));
    }
}
