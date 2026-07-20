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
//!   of the latest completed realm view. The nested backend implements it
//!   with [`Presenter::view_rgba`] returning `None` — it is EGL/GLES-bound
//!   and retains no readable image — so `--nested` cannot serve
//!   `vitrin_view.frame_ready` and those captures are refused rather than
//!   answered with invented pixels. A real, stated limitation of nested
//!   mode, not an oversight.
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
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use vitrin_ipc::{
    Connection, ConnectionEvent, ConnectionSource, Listener, ListenerEvent, ListenerSource, Outbox,
};

use crate::capture::RealmViewFrame;
use crate::grants::GrantTable;
use crate::identity::StaticVerifier;
use crate::input::{InputRouter, PhysicalPresence, PreemptionHook, SeatInput};
use crate::petitions::{ConnectionId, PetitionRegistry, Resolution};
use crate::principal::{PrincipalServer, ServerCtx};
use crate::realm::RealmRegistry;
use crate::recorder::Recorder;
use crate::scene::Scene;
use crate::shim::ShimServer;

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
    /// The realm's shim session, once something attaches one.
    /// [`attach_realm`] is the seam; nothing in this module spawns.
    pub realm: Option<RealmRuntime>,
    /// The realm's input router: chokepoint-admitted agent actuations and
    /// (nested) physical input converge here before delivery to the shim.
    pub router: InputRouter<H>,
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

/// The realm's live shim session: the protocol server plus the out-of-band
/// send handle for its connection.
///
/// Dead-code-allowed exactly like the modules whose runtime callers had not
/// arrived yet: the shim half of the loop is complete and tested through
/// [`attach_realm`], and gains its caller when `spawn::spawn_realm` is wired
/// (the other half of issue #77). Remove the attribute with that call, and let
/// the compiler confirm the path is live.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct RealmRuntime {
    /// `Option` because [`ShimServer::connection_closed`] consumes the server
    /// by value, and because the realm-teardown funnel this becomes takes
    /// `&mut Option<ShimServer>` for the same reason.
    pub server: Option<ShimServer>,
    /// Pushes seat events and `frame_done` at a shim that is not talking.
    pub outbox: Outbox,
    /// The shim connection's registration.
    ///
    /// **Removing this token is how the core hangs up on a live shim.** The
    /// `ConnectionSource` owns the `Connection`, so removal drops it, and
    /// that drop is the shim's EOF — rung 0 of the shutdown ladder. There is
    /// exactly one core-side descriptor for the socketpair and calloop has
    /// it; anything that wants to hang up must come through here rather than
    /// through a `Connection` it holds itself. Duplicating the fd to keep a
    /// second handle would be silently wrong: the shim would never see EOF
    /// and every clean shutdown would degrade to a `SIGTERM`.
    ///
    /// Unread until the shutdown ladder is wired -- which is the code that
    /// will read it, and the reason it is stored rather than discarded.
    #[allow(dead_code)]
    pub token: RegistrationToken,
}

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
pub(crate) trait Presenter {
    /// The realm's scene — what [`ShimServer::handle_message`] commits into.
    fn scene(&mut self) -> &mut Scene;
    /// The size the realm view composes at: the virtual output for headless,
    /// the host window for nested. The input router maps view coordinates to
    /// surface coordinates against it.
    fn view_size(&self) -> (u32, u32);
    /// Recomposite. Called at most once per dispatch round, from
    /// [`post_dispatch`] and never from a message callback.
    fn redraw(&mut self) -> Result<(), Box<dyn Error>>;
    /// The latest completed realm view as tightly packed RGBA8888, or `None`
    /// on a backend that has no readback path.
    ///
    /// `None` is not a failure and must not be treated as one: it is the
    /// honest answer for the nested backend. A capture then meets the
    /// chokepoint's existing `no_surface` refusal, which is the correct
    /// outcome — better than a black frame an agent would read as the realm's
    /// actual content.
    fn view_rgba(&mut self) -> Option<Vec<u8>>;
    /// Ask the backend to schedule a presentation, for backends whose frame
    /// clock is external (nested: the host compositor's redraw request). The
    /// default is the headless posture, where a completed composite *is* the
    /// output cadence and nothing further needs scheduling.
    #[cfg_attr(not(test), allow(dead_code))]
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
    pub fn into_recorder(self) -> Recorder {
        self.kernel.recorder
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

/// Attach the realm's shim session to the loop — the seam a spawn plugs into.
///
/// `connection` is the core-side end of the socketpair the shim inherited at
/// fork, and `server` is the [`ShimServer`] that has **already sent
/// `configure`** on it. That ordering is not negotiable and is the opposite
/// of the "register the reader first" instinct: `configure` is the core's
/// guaranteed-first message and it must go out on the still-blocking fd,
/// because [`ConnectionSource`] takes the `Connection` by value and flips it
/// non-blocking, after which there is no way to reach it outside a dispatch.
/// A few dozen bytes into an empty kernel buffer on a freshly created pair
/// cannot park the compositor, which is what makes the blocking send safe.
///
/// After this returns the loop is servicing the socketpair, so the shim's
/// blocking waits all terminate.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn attach_realm<H: RuntimeHost>(
    host: &mut H,
    connection: Connection,
    server: ShimServer,
) -> Result<(), Box<dyn Error>> {
    let (source, outbox) = ConnectionSource::with_outbox(connection)?;
    let handle = host.loop_handle();
    let token = handle.insert_source(source, |event, conn, host: &mut H| {
        dispatch_shim(host, event, conn)
    })?;
    host.runtime().realm = Some(RealmRuntime {
        server: Some(server),
        outbox,
        token,
    });
    tracing::info!("realm shim session attached to the event loop");
    Ok(())
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
fn deliver<H: PreemptionHook>(runtime: &mut Runtime<H>, resolution: Resolution, now: Instant) {
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
                // A realm view exists for capture only while a surface is
                // committed; with none, `realm_view` is `None` and the
                // chokepoint answers `no_surface`. The cache itself is
                // refreshed at redraw time, never here, so capture stays a
                // pure read.
                let live = view.scene().surface_size().is_some();
                let Runtime {
                    kernel,
                    conns,
                    view_cache,
                    ..
                } = runtime;
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
fn route_seat<H: RuntimeHost>(host: &mut H, seat: Vec<SeatInput>) {
    if seat.is_empty() {
        return;
    }
    let (runtime, view) = host.split();
    let view_size = view.view_size();
    let surface = view.scene().surface_size();
    let Runtime { router, realm, .. } = runtime;
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
        if let Err(err) = server.deliver_seat_event(&delivery, &mut send) {
            // The shim has stopped reading. Stop producing for it; the
            // transport's own slow-reader policy kills the connection on the
            // next dispatch, through the one funnel that classifies deaths.
            tracing::warn!(%err, "seat delivery to the realm failed");
            break;
        }
    }
}

/// Dispatch one event from the realm's shim connection.
#[cfg_attr(not(test), allow(dead_code))]
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
                    close_realm(host);
                }
            }
        }
        ConnectionEvent::Disconnected => {
            tracing::info!("shim connection closed");
            close_realm(host);
        }
        ConnectionEvent::Fault(reason) => {
            tracing::warn!(%reason, "shim connection terminated");
            close_realm(host);
        }
    }
}

/// The realm's shim connection is gone: drop its surface from the scene, its
/// seat state from the router, and forget the session.
///
/// Dropping the surface is what makes the chokepoint's `no_surface` refusal
/// true after a shim dies, so no capture can ever serve a dead realm's last
/// frame. The scene is marked dirty so the retained framebuffer is
/// recomposited without it on the next round rather than keeping those pixels
/// readable until something else happens to damage the scene.
///
/// This is a **placeholder for the realm-lifecycle funnel**
/// (`RealmLifecycle::note_connection_closed`), which additionally reaps the
/// child, marks the realm exited so petitions for it resolve `unavailable`,
/// and scrubs the retained output. Wiring lifecycle in is the other half of
/// issue #77; when it lands, this function's body becomes the call into that
/// funnel and must not stay as a second, quieter death path beside it.
#[cfg_attr(not(test), allow(dead_code))]
fn close_realm<H: RuntimeHost>(host: &mut H) {
    let (runtime, view) = host.split();
    let Runtime {
        realm,
        router,
        dirty,
        ..
    } = runtime;
    if let Some(realm) = realm.as_mut() {
        if let Some(server) = realm.server.take() {
            server.connection_closed(view.scene(), None, router);
        }
    }
    // The token is dropped rather than removed: both paths that reach here
    // are transport-initiated (EOF, fault) or follow a shim fault whose
    // source removes itself, so calloop has already retired the source.
    *realm = None;
    *dirty = true;
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
pub(crate) fn post_dispatch<H: RuntimeHost>(host: &mut H) {
    let fatal = {
        let (runtime, view) = host.split();
        if !runtime.dirty {
            return;
        }
        runtime.dirty = false;
        match view.redraw() {
            Ok(()) => {
                // Refreshed here and nowhere else: capture reads this cache,
                // so a refresh on the request path would make an agent's
                // capture trigger a composite and make goldens depend on
                // request timing.
                runtime.view_cache = view.view_rgba();
                let Runtime { realm, epoch, .. } = runtime;
                if let Some(realm) = realm.as_mut() {
                    if let Some(server) = realm.server.as_mut() {
                        if server.wants_presentation() {
                            let time_ms = epoch.elapsed().as_millis() as u32;
                            let outbox = &realm.outbox;
                            let mut send = |frame: &[u8]| outbox.send(frame);
                            if let Err(err) = server.presented(time_ms, &mut send) {
                                tracing::warn!(%err, "frame_done delivery to the realm failed");
                            }
                        }
                    }
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
    use std::path::PathBuf;
    use std::time::Duration;

    use calloop::{EventLoop, LoopSignal};
    use vitrin_ipc::Connection;
    use vitrin_protocol::generated::vitrin_grant::Outcome;
    use vitrin_protocol::generated::{
        vitrin_grant, vitrin_handshake, vitrin_principal, vitrin_realm,
    };
    use vitrin_protocol::wire::HEADER_LEN;

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
    }

    impl Presenter for TestView {
        fn scene(&mut self) -> &mut Scene {
            &mut self.scene
        }
        fn view_size(&self) -> (u32, u32) {
            VIEW
        }
        fn redraw(&mut self) -> Result<(), Box<dyn Error>> {
            self.redraws += 1;
            Ok(())
        }
        fn view_rgba(&mut self) -> Option<Vec<u8>> {
            Some(crate::test_pattern::render(VIEW.0, VIEW.1))
        }
    }

    struct TestHost {
        runtime: Runtime<NoopHook>,
        view: TestView,
        handle: LoopHandle<'static, TestHost>,
        signal: LoopSignal,
        fatal: Option<Box<dyn Error>>,
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
            };
            let event_loop: EventLoop<'static, TestHost> =
                EventLoop::try_new().expect("event loop");
            let handle = event_loop.handle();
            let mut host = TestHost {
                runtime: Runtime::new(seed, InputRouter::new(NoopHook)),
                view: TestView {
                    scene: Scene::new(),
                    redraws: 0,
                },
                handle: handle.clone(),
                signal: event_loop.get_signal(),
                fatal: None,
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

    /// The shim socketpair is really serviced by the loop: a mock shim
    /// completes its whole session — `configure`, surface, N paced frames —
    /// against the runtime rather than against a hand-rolled test harness.
    ///
    /// This is the wedge (T1) asserted rather than reasoned about. Every wait
    /// on the shim's side is a blocking `recv` with no timeout anywhere, so a
    /// loop that did not service the socketpair would not fail this test — it
    /// would hang it forever. The pacing assertion is the second half: each
    /// frame's `frame_done` is produced by [`post_dispatch`] and pushed
    /// through the realm's [`Outbox`], which is the only reason a shim
    /// blocked in `recv` ever hears from a compositor it is not talking to.
    #[test]
    fn a_mock_shim_runs_a_whole_session_over_the_runtime_loop() {
        use crate::shim::{ShimConfig, ShimServer};

        let _fd = crate::capture::tests::fd_lock();
        const FRAMES: u32 = 3;
        let mut rig = Rig::new(
            "shim",
            ConsentPolicyArg {
                policy: crate::petitions::ConsentPolicy::Interactive,
                config: PetitionConfig::default(),
            },
        );

        let (mut core_conn, shim_conn) = Connection::pair().expect("socketpair");
        let shim_thread = std::thread::spawn(move || {
            let mut shim = vitrin_mock_shim::MockShim::start(shim_conn)?;
            shim.run_paced_animation(FRAMES)
        });

        let server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: VIEW.0,
            height: VIEW.1,
        });
        // `configure` on the still-blocking fd, before calloop takes the
        // connection by value -- the ordering `attach_realm` documents.
        server
            .send_configure(&mut |frame| core_conn.send_message(frame, None))
            .expect("send configure");
        attach_realm(&mut rig.host, core_conn, server).expect("attach the realm");

        rig.pump(Duration::from_millis(3000));

        let stats = shim_thread
            .join()
            .expect("shim thread")
            .expect("mock shim animation");
        assert_eq!(
            stats.frames_rendered, FRAMES,
            "the shim must complete every frame, which it can only do if the loop really \
             serviced its socketpair and really sent its frame_dones"
        );
        assert!(
            rig.host.view.redraws >= 1,
            "a latched commit must have produced a composite -- from post_dispatch, not \
             from dispatch"
        );
        assert!(
            rig.host.view.redraws <= FRAMES as usize + 1,
            "presentation must be coalesced: {} composites for {FRAMES} commits",
            rig.host.view.redraws
        );
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
}
