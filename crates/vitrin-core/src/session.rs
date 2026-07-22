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
use crate::grants::GrantTable;
use crate::identity::StaticVerifier;
use crate::input::{InputRouter, PhysicalPresence, PreemptionHook, SeatInput};
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
}

/// Everything one running session owns that is not presentation, living in
/// the backend's calloop state type.
///
/// Generic over the preemption hook so the realm's input router is the
/// backend's own router — the one the consent grab and the dead-man watcher
/// are already stacked into — rather than a second router the two could drift
/// apart from.
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
    /// The realm's shim session, once [`start_realm_in`] has forked one.
    ///
    /// `None` until then, and the ordering behind that is the module's
    /// central invariant rather than an initialization detail: [`install`]
    /// must have registered the shim socketpair's source *before* the fork,
    /// because a shim whose connection nothing services blocks on `configure`
    /// forever. Spawning first and wiring after is a permanent, silent hang.
    pub realm: Option<RealmRuntime>,
    /// The realm's input router: chokepoint-admitted agent actuations and
    /// (nested) physical input converge here before delivery to the shim.
    pub router: InputRouter<H>,
    /// The core-known shim binary the spawn manager execs (issue #103), from
    /// the seed. [`start_realm`] reads it to build the spawn's [`SpawnPaths`];
    /// tests that call [`start_realm_in`] with explicit paths never consult it.
    shim: PathBuf,
    /// Set by a latched commit, cleared by [`post_dispatch`]. The whole of
    /// the anti-amplification defence the module docs describe.
    pub dirty: bool,
    /// The latest completed realm view, refreshed at redraw time and never on
    /// the capture path, so `capture` stays the pure read of "what the
    /// compositor last finished" that keeps goldens deterministic.
    view_cache: Option<Vec<u8>>,
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
    /// Physical-input presence, fed at the router's hook point; the
    /// chokepoint's `preempted` judgement reads it.
    pub presence: PhysicalPresence,
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
/// differently, and the realm's `frame_done` must follow the *composite*
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
    /// The realm's scene — what [`ShimServer::handle_message`] commits into.
    fn scene(&mut self) -> &mut Scene;
    /// The size the realm view composes at: the virtual output for headless,
    /// the host window for nested. The input router maps view coordinates to
    /// surface coordinates against it.
    fn view_size(&self) -> (u32, u32);
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
    /// The latest completed realm view as tightly packed RGBA8888, or `None`
    /// when there is no realm view to serve.
    ///
    /// Both backends produce a view: headless reads back its retained pixman
    /// framebuffer; nested composes the bare scene on the CPU
    /// ([`Scene::compose`], P1.3.8) — byte-identical for the same scene, and
    /// overlay-free on both because `Scene::compose` is upstream of the
    /// output-stage fork. `None` is not a failure and must not be treated as
    /// one: it is the honest answer for a degenerate view (a minimized nested
    /// window, a readback failure). A capture then meets the chokepoint's
    /// existing `no_surface` refusal, which is the correct outcome — better
    /// than a black frame an agent would read as the realm's actual content.
    fn view_rgba(&mut self) -> Option<Vec<u8>>;
    /// The scene and the retained framebuffer, borrowed **together**.
    ///
    /// One call rather than two accessors because [`RealmTeardown`] holds
    /// both at once: `RealmLifecycle::die` takes the realm's surface out of
    /// the scene and scrubs its last painted frame out of the retained
    /// image inside a single latched transition, so a backend that could
    /// only lend one at a time would make the teardown funnel unbuildable —
    /// and the way out of *that* is a second, unlatched scrub path beside
    /// the funnel, which is the one thing this teardown must not grow.
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
    fn teardown_view(&mut self) -> (&mut Scene, Option<&mut dyn RetainedOutput>);
    /// Ask the backend to schedule a presentation, for backends whose frame
    /// clock is external (nested: the host compositor's redraw request). The
    /// default is the headless posture, where a completed composite *is* the
    /// output cadence and nothing further needs scheduling.
    fn request_present(&mut self) {}
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
}

impl<H: PreemptionHook> Runtime<H> {
    /// Build the loop-resident runtime from the seed and the backend's own
    /// input router.
    pub fn new(seed: RuntimeSeed, router: InputRouter<H>) -> Self {
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
        } = seed;
        Self {
            kernel: Kernel {
                verifier,
                petitions,
                grants,
                realms,
                recorder,
                presence: PhysicalPresence::new(),
            },
            listener: Some(listener),
            conns: BTreeMap::new(),
            realm: None,
            router,
            shim,
            dirty: false,
            view_cache: None,
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
            &mut self.kernel.recorder,
            now,
        );
        let revoked = effect.revoked.len();
        let denied = effect.denied.len();
        for resolution in effect.denied {
            deliver(self, resolution, now);
        }
        tracing::warn!(
            chord = trigger.chord,
            held_ms = trigger.held.as_millis(),
            revoked_grants = revoked,
            denied_petitions = denied,
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

/// **Spawn the session's realm and put it on the loop.** The other half of
/// the runtime wiring: before this, a running `vitrind` forked nothing and
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
///    the ladder's first rung really does hang up.
/// 5. `mark_running` moves the realm out of `Configured`. Easy to forget and
///    **silent** if forgotten — `Configured` still admits petitions, so
///    nothing fails; the only symptom is a wrong `RealmState` in the flight
///    recorder.
pub(crate) fn start_realm<H: RuntimeHost>(host: &mut H) -> Result<(), Box<dyn Error>> {
    // The shim binary is a core input (`--shim`), carried in the runtime; the
    // app it will exec is the realm's `command`. Clone it out before the
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
pub(crate) fn start_realm_in<H: RuntimeHost>(
    host: &mut H,
    paths: &SpawnPaths,
) -> Result<(), Box<dyn Error>> {
    let (width, height) = {
        let (_, view) = host.split();
        view.view_size()
    };

    let (mut spawned, realm_id) = {
        let runtime = host.runtime();
        // Disjoint field borrows: `spawn_realm` reads the realm and writes
        // the log, and both live in `Kernel`.
        let Kernel {
            realms, recorder, ..
        } = &mut runtime.kernel;
        // Phase 1 serves exactly one realm (`RealmRegistry::load` enforces
        // it), so "the configured realm" is unambiguous. When that stops
        // being true this becomes a loop, and nothing else here changes:
        // every piece of state below is already per-realm.
        let realm = realms
            .iter()
            .next()
            .ok_or("no realm is configured for this session")?;
        let spawned = spawn::spawn_realm(realm, paths, recorder)?;
        let realm_id = spawned.realm_id().clone();
        (spawned, realm_id)
    };
    let pid = spawned.pid();

    // Step 2: `configure` on the still-blocking fd, before calloop owns it.
    let server = spawned.start_shim_session(width, height)?;

    let parts = spawned.into_parts();
    let handle = host.loop_handle();
    // `adopt` hands the connection to this closure and takes back the way to
    // release it again, so there is no window in which the lifecycle holds a
    // realm it cannot hang up on.
    let mut registered = None;
    let life = RealmLifecycle::adopt(parts, |connection| {
        let (source, outbox) = ConnectionSource::with_outbox(connection)?;
        let token = handle.insert_source(source, |event, conn, host: &mut H| {
            dispatch_shim(host, event, conn)
        })?;
        registered = Some(outbox);
        let releaser = handle.clone();
        Ok::<_, Box<dyn Error>>(Hangup::registered(move || releaser.remove(token)))
    })?;
    let outbox = registered.expect("adopt runs `place` exactly once on the success path");

    let runtime = host.runtime();
    runtime.realm = Some(RealmRuntime {
        life,
        server: Some(server),
        outbox,
    });
    if !runtime.kernel.realms.mark_running(&realm_id, pid) {
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

/// A `SIGCHLD` arrived: poll the realm for an exit.
///
/// Speculative and cheap by design — `SIGCHLD` says only that *some* child
/// changed state, so the reaper asks `waitpid` rather than guessing. A realm
/// already reaped answers immediately.
pub(crate) fn reap_realm<H: RuntimeHost>(host: &mut H) {
    with_realm_teardown(host, |life, teardown| {
        life.poll_exit(teardown);
    });
}

/// Tear the realm down on the way out of the session: the shutdown ladder,
/// then the realm's runtime tree.
///
/// **Blocks**, deliberately, which is why it must run after `event_loop.run`
/// has returned and never from inside a dispatch: the ladder waits out a
/// hangup grace period and then a `SIGTERM` grace period, and doing that
/// inside a live compositor loop would stall every other peer. It must also
/// run before the recorder is handed back, so the realm's `realm_died` and
/// `realm_exited` entries land in the run they belong to.
pub(crate) fn shutdown_realm<H: RuntimeHost>(host: &mut H) {
    let rung = with_realm_teardown(host, |life, teardown| {
        let rung = life.shutdown(ShutdownTiming::default(), teardown);
        (life.realm_id().clone(), life.pid(), rung)
    });
    if let Some((realm, pid, rung)) = rung {
        tracing::info!(%realm, pid, ?rung, "realm torn down");
    }
    // Dropped only now: the lifecycle holds the realm `flock`, and releasing
    // it before the runtime tree is gone would let a second core call a tree
    // stale while this one is still taking it apart.
    host.runtime().realm = None;
}

/// Run `f` against the realm's lifecycle with a [`RealmTeardown`] built from
/// the whole session — the one borrow shape every death path needs.
///
/// Every field comes from a distinct place (the presenter's scene and
/// retained image, the realm's shim server, the runtime's router, the
/// kernel's registry and recorder), so this exists to assemble that borrow
/// once rather than three times, and to make sure no death path can quietly
/// leave one of them out. `importer` is `None` on both backends: neither
/// runtime path threads a dmabuf importer today, so every `kind=dmabuf`
/// commit already resolves as the designed `import_failed` shm fallback and
/// there is no zero-copy content to drop.
fn with_realm_teardown<H: RuntimeHost, T>(
    host: &mut H,
    f: impl FnOnce(&mut RealmLifecycle, &mut RealmTeardown<'_, H::Hook>) -> T,
) -> Option<T> {
    let (runtime, view) = host.split();
    let (scene, retained) = view.teardown_view();
    let Runtime {
        kernel,
        realm,
        router,
        ..
    } = runtime;
    let realm = realm.as_mut()?;
    let mut teardown = RealmTeardown {
        scene,
        shim: &mut realm.server,
        importer: None,
        router,
        retained,
        realms: &mut kernel.realms,
        recorder: &mut kernel.recorder,
    };
    Some(f(&mut realm.life, &mut teardown))
}

/// The advisory expiry sweeps, one timer tick.
///
/// Petitions first, deliberately: a petition that times out emits the
/// client's terminal, and running it before the grant sweep keeps the log's
/// causal order — a petition dies, then rows die — matching what a
/// reconstruction expects.
fn sweep<H: RuntimeHost>(host: &mut H) {
    let now = Instant::now();
    let runtime = host.runtime();

    for resolution in runtime.kernel.petitions.expire_due(now) {
        deliver(runtime, resolution, now);
    }

    // `record_expiry_sweep` writes nothing for an empty sweep, which is why
    // this can run every second without turning a quiet session's log into a
    // heartbeat file.
    let expired = runtime.kernel.grants.expire_due(now);
    runtime.kernel.recorder.record_expiry_sweep(&expired);
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
/// 4. Raise the front prompt **only when none is up**. That makes
///    one-prompt-at-a-time structural, and it is also what stops a busy-spin:
///    re-raising an already-shown petition returns `Some(route)`, which would
///    otherwise set `changed = true` on every idle round and keep the frame
///    perpetually dirty.
pub(crate) fn service_consent_round<H: PreemptionHook>(
    grab: &mut ConsentGrab,
    runtime: &mut Runtime<H>,
    consent: &mut ConsentSurface,
    now: Instant,
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
    if grab.armed_petition().is_none() {
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
            let mut seat: Vec<SeatInput> = Vec::new();
            let now = Instant::now();
            let outcome = {
                let (runtime, view) = host.split();
                let (width, height) = view.view_size();
                let Runtime {
                    kernel,
                    conns,
                    view_cache,
                    realm,
                    ..
                } = runtime;
                // **The single fact behind every `no_surface` refusal.**
                // `RealmLifecycle::view_is_live` is that fact and this is
                // the one place the runtime derives `ServerCtx::realm_view`
                // from it — deliberately not `scene().surface_size()`,
                // which is only *half* of it: a realm can be dead with its
                // scene not yet recomposited, and asking the scene would
                // then photograph a corpse. No realm at all is likewise not
                // live.
                //
                // The cache itself is refreshed at redraw time, never here,
                // so capture stays a pure read of the last completed frame.
                let live = realm
                    .as_ref()
                    .is_some_and(|realm| realm.life.view_is_live(view.scene()));
                let Some(state) = conns.get_mut(&id) else {
                    return;
                };
                let realm_view =
                    view_cache
                        .as_deref()
                        .filter(|_| live)
                        .map(|rgba| RealmViewFrame {
                            rgba,
                            width,
                            height,
                        });
                let mut actuations = |input: SeatInput| seat.push(input);
                let mut ctx = ServerCtx {
                    verifier: &kernel.verifier,
                    petitions: &mut kernel.petitions,
                    realms: &kernel.realms,
                    grants: &mut kernel.grants,
                    now,
                    realm_view,
                    presence: &kernel.presence,
                    actuations: &mut actuations,
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
                // The goodbye is already on the wire and the violation is
                // already logged; `handle_message` cannot run teardown
                // because it holds no kernel state. This is the third close
                // path — the one that reaches teardown only because the
                // embedder brings it here — and the source is still
                // registered, so the core removes it.
                close_principal(host, id, CloseCause::CoreInitiated);
                return;
            }
            route_seat(host, seat);
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

/// Route chokepoint-admitted actuations through the realm's router toward the
/// shim's seat.
///
/// The router is the same one the backend's physical input flows through, so
/// implicit grabs and pointer state are shared between an agent's actuations
/// and a human's — which is what makes the preemption hook meaningful. The
/// origin tag rides the wire on every event and is never constructed or
/// rewritten here (backward requirement B2): this path only addresses.
///
/// Every event that actually reaches the shim's seat is recorded, tagged with
/// that origin ([`crate::recorder::Event::SeatDelivered`], issue #83): the
/// unforgeable physical-vs-agent distinction the type system enforces is only
/// investigable after an incident if the flight recorder wrote it down.
/// Shape only — the kind and the tag, never coordinates, keysym, or typed
/// bytes — so the audit entry can never become a keylogger.
fn route_seat<H: RuntimeHost>(host: &mut H, seat: Vec<SeatInput>) {
    if seat.is_empty() {
        return;
    }
    let (runtime, view) = host.split();
    let view_size = view.view_size();
    let surface = view.scene().surface_size();
    let Runtime {
        router,
        realm,
        kernel,
        ..
    } = runtime;
    let Some(realm) = realm.as_mut() else {
        return;
    };
    let Some(server) = realm.server.as_ref() else {
        return;
    };
    let outbox = &realm.outbox;
    for input in seat {
        let Some(delivery) = router.route(input, view_size, surface) else {
            continue;
        };
        let mut send = |frame: &[u8]| outbox.send(frame);
        match server.deliver_seat_event(&delivery, &mut send) {
            // Recorded only when it went out (a seat the shim has not minted
            // yet drops the event — nothing was delivered, so nothing is
            // audited as delivered). One funnel with the physical path.
            Ok(sent) => {
                if sent {
                    crate::input::record_seat_delivery(&mut kernel.recorder, &delivery);
                }
            }
            Err(err) => {
                // The shim has stopped reading. Stop producing for it; the
                // transport's own slow-reader policy kills the connection on
                // the next dispatch, through the one funnel that classifies
                // deaths.
                tracing::warn!(%err, "seat delivery to the realm failed");
                break;
            }
        }
    }
}

/// Dispatch one event from the realm's shim connection.
fn dispatch_shim<H: RuntimeHost>(
    host: &mut H,
    event: ConnectionEvent,
    conn: &mut calloop::generic::NoIoDrop<Connection>,
) {
    match event {
        ConnectionEvent::Message(msg) => {
            let (runtime, view) = host.split();
            let Runtime { realm, dirty, .. } = runtime;
            let Some(realm) = realm.as_mut() else {
                return;
            };
            let Some(server) = realm.server.as_mut() else {
                return;
            };
            let mut send = |frame: &[u8]| vitrin_ipc::reply(conn, frame, None);
            // No dmabuf importer on either backend's runtime path today
            // (headless has no GPU import at all, and the nested backend's
            // importer is not threaded here yet), so every `kind=dmabuf`
            // commit resolves as the designed `import_failed` shm fallback.
            match server.handle_message(msg, view.scene(), None, &mut send) {
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
                    close_realm(host, DeathCause::of_shim_fault(fault));
                }
            }
        }
        ConnectionEvent::Disconnected => {
            tracing::info!("shim connection closed");
            close_realm(host, DeathCause::ConnectionClosed);
        }
        ConnectionEvent::Fault(reason) => {
            tracing::warn!(%reason, "shim connection terminated");
            // The transport's classification, not a second opinion of it.
            close_realm(host, DeathCause::from(&reason));
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
fn close_realm<H: RuntimeHost>(host: &mut H, cause: DeathCause) {
    with_realm_teardown(host, |life, teardown| {
        life.note_connection_closed(cause, teardown);
    });
    // Recomposite without the dead realm's surface. The scene is already
    // clear, but this backend composites on demand, so without a redraw the
    // dead realm's pixels would stay on the human-visible output until
    // something else happened to damage the scene.
    host.runtime().dirty = true;
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
/// Discharge the realm's owed frame callbacks against a composite that has
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
pub(crate) fn emit_presented<H: PreemptionHook>(runtime: &mut Runtime<H>) {
    let Runtime { realm, epoch, .. } = runtime;
    let Some(realm) = realm.as_mut() else {
        return;
    };
    let Some(server) = realm.server.as_mut() else {
        return;
    };
    if !server.wants_presentation() {
        return;
    }
    let time_ms = epoch.elapsed().as_millis() as u32;
    let outbox = &realm.outbox;
    let mut send = |frame: &[u8]| outbox.send(frame);
    if let Err(err) = server.presented(time_ms, &mut send) {
        tracing::warn!(%err, "frame_done delivery to the realm failed");
    }
}

pub(crate) fn post_dispatch<H: RuntimeHost>(host: &mut H) {
    // First, before the dirty gate: raising or lowering a consent prompt is
    // exactly what makes the frame dirty, so it must run before the gate below
    // reads `dirty`. Backends that cannot host a prompt inherit the trait's
    // no-op and pay nothing here.
    host.service_consent(Instant::now());
    let fatal = {
        let (runtime, view) = host.split();
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
                runtime.view_cache = view.view_rgba();
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;

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
    const VIEW: (u32, u32) = (64, 40);
    /// The wire id the rig's petition mints for its `vitrin_grant`.
    const GRANT_ID: u32 = 4;

    /// A [`Presenter`] with no renderer: the runtime's contract with a
    /// backend is four small methods, and driving them with a counter is what
    /// makes "how many composites did one dispatch round cost" an assertable
    /// number rather than something a GPU hides.
    struct TestView {
        scene: Scene,
        redraws: usize,
        /// Which frame-clock posture to answer with. Defaults to the
        /// headless one; the pacing test flips it to the nested one.
        posture: Presentation,
        /// The consent surface [`service_consent_round`] raises prompts on.
        /// Present on every test host — the nested backend carries one too —
        /// but only exercised when a grab is attached; the consent tests
        /// assert a card was raised on it.
        consent: ConsentSurface,
    }

    impl Presenter for TestView {
        fn scene(&mut self) -> &mut Scene {
            &mut self.scene
        }
        fn view_size(&self) -> (u32, u32) {
            VIEW
        }
        /// Counts composites and reports whichever posture the test asked
        /// for: `Completed` (headless, the default) or `Scheduled` (nested,
        /// where the host compositor draws later).
        fn redraw(&mut self) -> Result<Presentation, Box<dyn Error>> {
            self.redraws += 1;
            Ok(self.posture)
        }
        fn view_rgba(&mut self) -> Option<Vec<u8>> {
            Some(crate::test_pattern::render(VIEW.0, VIEW.1))
        }
        /// No retained image to scrub: this view keeps a counter, not a
        /// framebuffer.
        fn teardown_view(&mut self) -> (&mut Scene, Option<&mut dyn RetainedOutput>) {
            (&mut self.scene, None)
        }
    }

    struct TestHost {
        runtime: Runtime<NoopHook>,
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
    }

    impl RuntimeHost for TestHost {
        type Hook = NoopHook;
        type View = TestView;

        fn split(&mut self) -> (&mut Runtime<NoopHook>, &mut TestView) {
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
            let mut grab = grab.borrow_mut();
            if service_consent_round(&mut grab, &mut self.runtime, &mut self.view.consent, now) {
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
            vec![StaticPrincipal {
                identity: PrincipalIdentity::parse(DEMO_IDENTITY).unwrap(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            }],
            rustix::process::geteuid().as_raw(),
        )
        .unwrap()
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
            };
            let event_loop: EventLoop<'static, TestHost> =
                EventLoop::try_new().expect("event loop");
            let handle = event_loop.handle();
            let mut host = TestHost {
                runtime: Runtime::new(seed, InputRouter::new(NoopHook)),
                view: TestView {
                    scene: Scene::new(),
                    redraws: 0,
                    posture: Presentation::Completed,
                    consent: ConsentSurface::new(crate::consent::TrustedIndicator::for_test()),
                },
                handle: handle.clone(),
                signal: event_loop.get_signal(),
                fatal: None,
                grab: None,
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
            let mock = crate::spawn::tests::mock_shim_bin();
            self.host.runtime.kernel.realms =
                crate::realm::tests::registry_of(vec![crate::realm::tests::realm_with_spawn(
                    crate::realm::WELL_KNOWN_REALM_ID,
                    &mock,
                    &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                    &[],
                )]);
            start_realm_in(&mut self.host, &SpawnPaths::under(&self.dir, &mock))
                .expect("the realm must spawn and attach");
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
        for _ in 0..32 {
            let msg = client
                .recv_message()
                .expect("client receive")
                .expect("the core must not have hung up");
            // Object id as well as opcode: opcodes are per-interface, so
            // matching on the opcode alone would decode some other
            // interface's event as a resolution.
            if msg.header.object_id == GRANT_ID
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
        let shim = rig
            .host
            .runtime
            .realm
            .as_ref()
            .expect("the realm is attached")
            .life
            .pid();
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

        let pid = rig
            .host
            .runtime
            .realm
            .as_ref()
            .expect("the realm is attached")
            .life
            .pid();

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
        rig.pump_until(Duration::from_secs(10), |host| {
            host.runtime
                .realm
                .as_ref()
                .and_then(|r| r.server.as_ref())
                .is_some_and(|s| s.seat_minted())
        });

        // Two agent-originated events through the production route. Text never
        // needs a committed surface to route, so the seat mint is the only
        // precondition; the origin travels unrewritten from `emulated` to the
        // journal.
        route_seat(
            &mut rig.host,
            vec![
                SeatInput::emulated(crate::input::SeatInputKind::Text { text: "hi".into() }),
                SeatInput::emulated(crate::input::SeatInputKind::Text {
                    text: "there".into(),
                }),
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
        let shim = rig
            .host
            .runtime
            .realm
            .as_ref()
            .expect("the realm is attached")
            .life
            .pid();

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
        rig.pump_until(Duration::from_secs(10), |host| {
            host.runtime
                .realm
                .as_ref()
                .and_then(|realm| realm.server.as_ref())
                .is_some_and(|server| server.wants_presentation())
        });

        assert!(
            rig.host.view.redraws > 0,
            "the rounds still cost their composite requests"
        );
        assert!(
            rig.host
                .runtime
                .realm
                .as_ref()
                .and_then(|realm| realm.server.as_ref())
                .is_some_and(|server| server.wants_presentation()),
            "a scheduled composite leaves the frame callbacks owed -- emitting them here \
             is the silent pacing bug: the shim would be told a frame it never saw was presented"
        );

        // And the debt is discharged the moment a composite really lands,
        // which is the other half of the contract: the callbacks are delayed,
        // never dropped.
        emit_presented(&mut rig.host.runtime);
        assert!(
            !rig.host
                .runtime
                .realm
                .as_ref()
                .and_then(|realm| realm.server.as_ref())
                .is_some_and(|server| server.wants_presentation()),
            "a real composite must pay every owed callback"
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
}
