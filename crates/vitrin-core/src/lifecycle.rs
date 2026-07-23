//! Realm lifecycle (P1.5.3, issue #32): the realm's **death paths** -- crash
//! detection, exit propagation, reaping, and the orderly shutdown ladder.
//!
//! [`crate::spawn`] launches a realm and hands it over unreaped
//! ([`SpawnedRealm::into_parts`]); this module owns it from there until
//! there is nothing left of it. One type, [`RealmLifecycle`], holds every
//! resource a live realm consists of -- the core's end of the identity
//! socketpair, the shim process handle, the private runtime directory, and
//! the realm `flock` -- because they have to die together and in an order,
//! and split ownership is how one of them gets forgotten.
//!
//! Everything here is mechanism. Nothing consults or amends the grant
//! table, and nothing decides whether a use is permitted: a realm's death
//! *changes a fact* the enforcement chokepoint reads, and the chokepoint
//! goes on making the only authority judgement there is (see "The
//! `no_surface` chain" below).
//!
//! # Settled: no restart policy
//!
//! A crash means the surface is gone and the event is logged. There is no
//! supervision, no backoff, no restart -- that is a later concern, and a
//! half-policy invented here would be the wrong half. This is why
//! [`RealmState::Exited`] is terminal and why the realm stops admitting
//! petitions at that point rather than reporting itself merely unpainted.
//!
//! # Two independent death signals, and why both are load-bearing
//!
//! A realm can die in two directions and the core learns about it through
//! two unrelated kernel mechanisms, either of which may arrive first and
//! **both of which usually arrive**:
//!
//! - **End-of-stream on the identity socketpair** -- the shim's end of fd 3
//!   is gone. Delivered as ordinary readability on the connection the
//!   compositor's event loop already polls, in loop order, with the rest of
//!   that connection's traffic.
//! - **`SIGCHLD`** -- the shim process changed state. Delivered by the
//!   kernel to the process, asynchronously, and observed here through
//!   `calloop`'s `signalfd` source ([`child_signal_source`]) so no signal
//!   *handler* exists at all.
//!
//! **Which is authoritative: neither, for the other's question.** They
//! answer different ones, and the state machine treats them as such:
//!
//! - **EOF is authoritative for liveness.** It is what removes the surface
//!   from the scene, and it is the only one of the two that fires for a
//!   shim that abandons its core connection *while still running* -- a
//!   process that closes fd 3 and keeps going can never paint again, so
//!   the realm is over even though nothing has exited. Waiting for a
//!   `SIGCHLD` in that case would serve a stale frame for as long as the
//!   shim felt like living, which is precisely the failure this task
//!   exists to make impossible.
//! - **`waitpid` is authoritative for the process.** It is what produces
//!   the exit status and what prevents a zombie. `SIGCHLD` is *only a hint
//!   that `waitpid` may now succeed* -- it is never itself recorded as a
//!   death, and the reaper never trusts it as evidence that a particular
//!   child has exited. Every reap in this module is a real
//!   `waitpid(WNOHANG)` ([`Child::try_wait`]).
//!
//! **Neither implies the other**, which is why the module needs both:
//!
//! | | fd 3 closed | fd 3 open |
//! |---|---|---|
//! | **process alive** | EOF only: a shim that hung up but lives. Marked dead, asked to leave, reaped by the following `SIGCHLD`. | alive |
//! | **process dead** | the ordinary case: EOF *and* `SIGCHLD`, in either order | `SIGCHLD` only: the shim leaked fd 3 to another process before dying, so nothing closes it. Without the signal this realm would look alive forever. |
//!
//! That bottom-right cell is the entire reason the `SIGCHLD` source is not
//! redundant with EOF. It requires a shim that passes its core connection
//! to another process over `SCM_RIGHTS` -- a violation of the shim's
//! `FD_CLOEXEC` obligation, but the shim is **outside the TCB**, so "a
//! compliant shim would not" is not a reason to be unable to detect it.
//!
//! ## Idempotence: one latch, one handle
//!
//! Both signals funnel into one private function, [`RealmLifecycle::die`],
//! and it is latched on [`RealmLifecycle::death`]: the first observation
//! from either direction performs the whole transition (surface out of the
//! scene, realm marked exited, one `realm_died` entry) and every later one
//! is a no-op returning `false`. The reap is latched by a different and
//! equally simple thing -- the `Option<Child>` handle, taken on the
//! successful `waitpid` -- so a process can no more be waited on twice than
//! a realm can be buried twice. Order of arrival is therefore irrelevant,
//! double arrival is free, and a death is never double-counted.
//!
//! Keeping the [`Child`] rather than a bare pid is also what makes
//! signalling safe: an unreaped child's pid is reserved by the kernel until
//! it is waited on, so it cannot have been recycled onto an unrelated
//! process. Once reaped the handle is `None` and no code path can signal
//! that number again.
//!
//! # The `no_surface` chain (this task makes an existing refusal *true*)
//!
//! `vitrin_grant.refusal.no_surface` already exists in the IDL and already
//! means exactly this: "the realm has no surface (its shim crashed or
//! exited); never a stale frame". [`crate::enforcement`] already refuses it
//! for capture and actuation alike whenever the realm view is not live.
//! **No taxonomy is added here.** What was missing is that nothing made the
//! realm's death drive that state; this module is that link:
//!
//! 1. A death signal arrives (EOF or `SIGCHLD` -> `waitpid`).
//! 2. [`RealmLifecycle::die`] runs the realm through
//!    [`ShimServer::connection_closed`] -- the pre-existing teardown funnel,
//!    not a new one -- which calls [`Scene::clear_surface`], retires any
//!    retained zero-copy content, and resets the input router. It then
//!    scrubs the backend's **retained frame** ([`RetainedOutput`]), which
//!    clearing the scene does not do: [`Scene::compose`] is pure and so
//!    self-heals, but a framebuffer is a cache of a past composition and
//!    would otherwise hold the dead realm's last painted pixels for the
//!    rest of the session.
//! 3. The scene now has no committed surface, so
//!    [`RealmLifecycle::view_is_live`] answers `false` and the embedder
//!    passes `realm_view: None` for every subsequent dispatch turn.
//! 4. The chokepoint refuses `no_surface`. A capture gets **no
//!    `frame_ready` at all** -- not a stale frame, not the background, not
//!    a fabricated one.
//!
//! Step 3 is the only new seam, and it is deliberately a *fact* rather than
//! a judgement: [`view_is_live`] reports whether the realm has a live view,
//! and the chokepoint alone decides what that means for authority. There is
//! still exactly one enforcement site. Step 2's scrub is **not** a second
//! predicate and is never consulted -- it removes the bytes rather than
//! answering a question about them, so the chain has one authority and two
//! layers.
//!
//! [`view_is_live`]: RealmLifecycle::view_is_live
//!
//! # Shutdown ordering (decided here)
//!
//! `core exit -> shim exit -> app exit`, as a ladder whose first rung is not
//! a signal at all. [`RealmLifecycle::shutdown`]:
//!
//! 0. **Close the core's end of the identity socketpair.** For a shim that
//!    serves until its core hangs up -- which is the contract, and what
//!    `vitrin-mock-shim` and the wlroots shim (E6) both do -- *this is the
//!    shutdown signal*, and everything below is the fallback for one that
//!    does not. It is first because it is the only rung that is not a
//!    demand: the shim learns its core is gone and tears its app down the
//!    same way it would if the core had crashed, so the orderly path and
//!    the crash path exercise the same shim code.
//! 1. **Wait [`ShutdownTiming::hangup_grace`] (250 ms).** A shim exiting on
//!    EOF is gone in single-digit milliseconds; 250 ms is two orders of
//!    magnitude of headroom on a loaded CI runner and imperceptible to an
//!    operator. It is a ceiling, never a delay -- the ladder returns the
//!    instant the child is reaped.
//! 2. **`SIGTERM`, then wait [`ShutdownTiming::term_grace`] (2 s).**
//!    `SIGTERM` because it is the signal a process may *handle*, which is
//!    the whole point of asking before insisting.
//!    2 s because the shim's teardown obligation is memory-only -- close
//!    the app-facing socket, signal the app, reap it -- with no disk flush
//!    and no network round trip. `systemd`'s 90 s default and Docker's 10 s
//!    are tuned for daemons that persist state; a display shim is not one,
//!    and a core that took 90 s to exit would read as hung.
//! 3. **`SIGKILL`, then a blocking wait.** Not refusable, not blockable,
//!    not ignorable, so this rung always terminates.
//!
//! **What if the shim ignores `SIGTERM`?** It gets rung 3, and this is a
//! supported outcome rather than a defect: a shim may legitimately install
//! its own handler, and the grace period exists precisely to let a
//! legitimate handler finish. Two things had to be true for rung 2 to be a
//! real request rather than a silent no-op, and both are guaranteed by
//! [`crate::spawn`]'s post-fork closure rather than assumed here: the
//! child's signal **dispositions** are reset to `SIG_DFL` (issue #31), so
//! an operator's `vitrind &` cannot make the realm immune by inherited
//! `SIG_IGN`; and the child's signal **mask** is cleared (this task), so
//! the `SIGINT`/`SIGTERM` both backends block for their own `signalfd`
//! source -- plus the `SIGCHLD` this module blocks -- cannot cross the fork
//! and leave `SIGTERM` pending forever in the realm. Without that second
//! reset every realm this core spawns would take rung 3 every time, and
//! the ladder would be decorative.
//!
//! **The residual, stated rather than implied:** rung 3 kills the shim, and
//! a shim killed with `SIGKILL` tears nothing down. Its app is reparented
//! to init and survives the session. The core does not hunt grandchildren:
//! finding them means scanning `/proc` for a moving target, and killing
//! processes the core did not create is supervision policy, which is out of
//! scope by decision. The structural fix is a per-realm pid namespace,
//! where the shim is pid 1 and its death takes the namespace with it --
//! which arrives with the D9 sandboxing work ([`crate::spawn`]'s security
//! posture section), not with a `/proc` walk here. On the ordinary paths --
//! rungs 0 through 2, and every clean crash -- the shim reaps its own app,
//! which is what the acceptance test asserts.
//!
//! # The runtime tree: removed at the core's exit, kept across a crash
//!
//! [`crate::spawn`] left "removal at orderly exit" to this task
//! ([`RuntimeDirGuard::keep`] disarms the guard that would otherwise remove
//! it). The rule:
//!
//! - **A crash does not remove it.** The session is still running, so the
//!   directory is evidence -- it holds the socket the app was talking to,
//!   and post-mortem debugging of *why the shim died* is exactly when an
//!   operator wants it. Deleting evidence on the abnormal path is the wrong
//!   default. It may also still be in use: an orphaned app (the residual
//!   above) still has that socket path open, and pulling it out from under
//!   the app converts a clean "peer gone" into path confusion.
//! - **Shutdown removes it**, after the shim is confirmed reaped. The core
//!   is exiting; nothing of this realm should outlive it.
//! - **Either way the next start self-heals**, because
//!   [`crate::spawn`]'s stale-directory purge proves the previous owner is
//!   gone with the realm `flock` and then deletes. Removal here is tidiness
//!   with a safety argument, never the thing correctness rests on.
//!
//! The removal itself reuses [`crate::spawn::remove_runtime_dir`] rather
//! than calling `remove_dir_all`, so the crate's one recursive delete stays
//! one routine with one set of proofs. The realm `flock` is released
//! **last**, after the delete: releasing it earlier would let a second core
//! decide this realm is stale and start purging a tree this one is still
//! taking apart.
//!
//! # Clocks and blocking
//!
//! Unlike the grant table, the petition registry, and the chokepoint --
//! which never read a clock, so their decisions are reproducible from an
//! injected instant -- the shutdown ladder is inherently a wall-clock
//! affair, and it blocks. That is deliberate and confined to
//! [`RealmLifecycle::shutdown`], which runs on the way out, after the event
//! loop has stopped. **Nothing in the death-detection path blocks or
//! sleeps**: [`RealmLifecycle::note_connection_closed`] and
//! [`RealmLifecycle::poll_exit`] run inside the compositor loop and use
//! only non-blocking `waitpid(WNOHANG)` and a non-blocking `kill`, so a
//! dying realm can never park the compositor. [`ShutdownTiming`] is a
//! parameter so the ladder's *escalation* can be tested without testing the
//! clock.

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use calloop::signals::{Signal, Signals};
use vitrin_ipc::{Connection, TransportError};

use crate::dmabuf::DmabufImporter;
use crate::grants::RealmId;
use crate::input::{InputRouter, PreemptionHook};
use crate::realm::RealmRegistry;
use crate::recorder::{Event, Recorder};
use crate::scene::Scene;
use crate::shim::ShimFault;
use crate::shim::ShimServer;
use crate::spawn::{self, SpawnedParts};

/// How often the shutdown ladder re-checks whether the shim has exited.
/// Small enough that a prompt exit is not padded by the poll interval,
/// large enough not to spin: the ladder is on the exit path, not a hot one.
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The shutdown ladder's grace periods (module docs argue the values).
/// A parameter rather than two constants so tests can exercise the
/// *escalation* -- which rung a given shim ends on -- without waiting on
/// the production clock, and so the M1.1 wiring can tune it without
/// touching this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShutdownTiming {
    /// How long a shim gets to notice its core connection closed and exit
    /// on its own, before being asked (rung 1).
    pub hangup_grace: Duration,
    /// How long a shim gets after `SIGTERM` before `SIGKILL` (rung 2->3).
    pub term_grace: Duration,
}

impl Default for ShutdownTiming {
    fn default() -> Self {
        Self {
            hangup_grace: Duration::from_millis(250),
            term_grace: Duration::from_secs(2),
        }
    }
}

/// How a realm's shim process ended -- the classification the flight
/// recorder states and the only thing this module concludes from a
/// `waitpid` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitClass {
    /// Ran to completion and returned this status code. `code == 0` is the
    /// only *clean* end a realm has.
    Exited { code: i32 },
    /// Terminated by this signal -- `kill -9` of a crashing shim, or the
    /// ladder's own last rung (`core_initiated` on the log entry is what
    /// distinguishes those two, because the signal number cannot).
    Signaled { signal: i32 },
    /// A status that is neither, which POSIX does not describe for a
    /// waited-on child. Represented rather than panicked on: this is the
    /// TCB, and an unexpected kernel status is a thing to record, not a
    /// reason to abort a session.
    Unknown,
}

impl ExitClass {
    /// Classify a reaped child's status.
    fn of(status: ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            ExitClass::Exited { code }
        } else if let Some(signal) = status.signal() {
            ExitClass::Signaled { signal }
        } else {
            ExitClass::Unknown
        }
    }

    /// The fixed label the log carries -- a closed vocabulary a reader
    /// switches on, never free-form `Display` text.
    pub fn disposition(self) -> &'static str {
        match self {
            ExitClass::Exited { .. } => "exited",
            ExitClass::Signaled { .. } => "signaled",
            ExitClass::Unknown => "unknown",
        }
    }

    /// The exit code, for a normal exit.
    pub fn code(self) -> Option<i32> {
        match self {
            ExitClass::Exited { code } => Some(code),
            _ => None,
        }
    }

    /// The terminating signal, for a signalled exit.
    pub fn signal(self) -> Option<i32> {
        match self {
            ExitClass::Signaled { signal } => Some(signal),
            _ => None,
        }
    }

    /// Whether the realm's app ended the way a well-behaved one does.
    /// Reported, never acted on -- the MVP has no restart policy, so a
    /// clean exit and a crash have exactly the same consequence.
    pub fn is_clean(self) -> bool {
        matches!(self, ExitClass::Exited { code: 0 })
    }
}

impl fmt::Display for ExitClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitClass::Exited { code } => write!(f, "exited with status {code}"),
            ExitClass::Signaled { signal } => write!(f, "killed by signal {signal}"),
            ExitClass::Unknown => write!(f, "ended with an unrecognized status"),
        }
    }
}

/// Which observation ended the realm -- the first one to arrive, since the
/// transition is latched (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeathCause {
    /// End-of-stream on the identity socketpair: the shim's end of fd 3 is
    /// gone. Says nothing about whether the process is.
    ConnectionClosed,
    /// The shim connection ended on a transport fault or a protocol
    /// violation (`vitrin-ipc`'s `DisconnectReason` funnel, or
    /// `ShimViolation`'s log-and-close). Distinguished from a clean EOF
    /// because "the shim misbehaved" and "the shim finished" are different
    /// facts about the same realm.
    ConnectionFault,
    /// `waitpid` reaped the shim: the process is gone.
    ProcessExit,
    /// The core is exiting and tore the realm down deliberately.
    Shutdown,
}

impl From<&vitrin_ipc::DisconnectReason> for DeathCause {
    /// Classify the transport's own disconnect verdict.
    ///
    /// The mapping is the transport's, not a second opinion:
    /// `vitrin-ipc` has already sorted every way a connection can end into
    /// [`DisconnectReason`](vitrin_ipc::DisconnectReason), and logs
    /// `PeerAborted` at a lower level than the rest precisely because it is
    /// *crash-shaped rather than hostile*. This preserves that one
    /// distinction and collapses the rest, because that is the only
    /// distinction a realm's death has to make: "the shim went away" versus
    /// "the shim misbehaved".
    ///
    /// A `kill -9` mid-message lands in the first bucket, which is worth
    /// stating because it is not obvious: the kernel delivers the shim's
    /// partial frame and the transport reports `Eof` -- a close *inside* a
    /// frame -- rather than a clean end-of-stream. The realm crashed; it
    /// did not violate anything.
    fn from(reason: &vitrin_ipc::DisconnectReason) -> Self {
        match reason {
            vitrin_ipc::DisconnectReason::PeerAborted(_) => DeathCause::ConnectionClosed,
            _ => DeathCause::ConnectionFault,
        }
    }
}

impl DeathCause {
    /// Classify a transport-level end of connection exactly as
    /// `vitrin-ipc`'s own event-loop policy does -- one classification,
    /// reached through the transport's, never a second opinion of it.
    pub fn of_transport(err: TransportError) -> Self {
        Self::from(&vitrin_ipc::DisconnectReason::from(err))
    }

    /// Classify a shim-server fault. The enum already draws exactly the
    /// line a realm's death needs: a protocol violation is the shim
    /// misbehaving, a transport error is the shim going away -- including
    /// the write that fails with `EPIPE` because the peer is already gone,
    /// which is how the core usually notices a `kill -9` first.
    pub fn of_shim_fault(fault: ShimFault) -> Self {
        match fault {
            ShimFault::Violation(_) => DeathCause::ConnectionFault,
            ShimFault::Transport(err) => Self::of_transport(err),
        }
    }

    /// The fixed label the log carries.
    pub fn label(self) -> &'static str {
        match self {
            DeathCause::ConnectionClosed => "connection_closed",
            DeathCause::ConnectionFault => "connection_fault",
            DeathCause::ProcessExit => "process_exit",
            DeathCause::Shutdown => "shutdown",
        }
    }
}

/// Which rung of the shutdown ladder actually ended the shim (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownRung {
    /// Already reaped before shutdown began -- it had crashed or exited
    /// earlier in the session.
    AlreadyDead,
    /// Rung 0/1: exited on its own once its core connection closed. The
    /// contract, and the path a compliant shim takes.
    Hangup,
    /// Rung 2: exited on `SIGTERM`.
    Term,
    /// Rung 3: ignored `SIGTERM` for the whole grace period and was
    /// `SIGKILL`ed. Its app, if it had one, is orphaned (module docs).
    Kill,
}

/// Everything a realm's death has to touch, borrowed for one call.
///
/// Bundled into one struct for the same reason
/// [`ShimServer::connection_closed`] takes the input router: an embedder
/// that could clear the scene but forget the registry, or record the death
/// but leave the surface up, would have a realm that is dead in one place
/// and alive in another -- and the one that stayed alive would be the one
/// serving frames.
pub(crate) struct RealmTeardown<'a, 'v, H: PreemptionHook> {
    /// The realm's scene. Its surface leaves here, and nothing composites
    /// client content afterwards.
    ///
    /// `'v`, not `'a`, together with [`Self::importer`] and
    /// [`Self::retained`] below: those three come from
    /// [`session::Presenter::teardown_view`], a *callback* rather than a
    /// returned value (see that method's docs for why), so the lifetime it
    /// hands them out under is a fresh one scoped to that one call — never
    /// provably the same region as `'a`, which is `host`'s own borrow in
    /// [`session::with_realm_teardown`]. A single shared lifetime parameter
    /// here would force the two to unify, which is unsatisfiable for any
    /// `'a` shorter than `'static` once `'v` is universally quantified —
    /// the two are independent on purpose.
    pub scene: &'v mut Scene,
    /// The realm's shim protocol server, if the session ever came up.
    /// Taken (not borrowed) by the teardown funnel, which consumes it --
    /// so an embedder cannot keep serving a dead shim's object graph.
    pub shim: &'a mut Option<ShimServer>,
    /// The connection's dmabuf importer, if the embedder has a GPU: any
    /// retained zero-copy content is dropped with the realm. `'v` — see
    /// [`Self::scene`].
    pub importer: Option<&'v mut dyn DmabufImporter>,
    /// The realm's input router: seat state must not survive into a next
    /// shim generation.
    pub router: &'a mut InputRouter<H>,
    /// The backend's retained output, if this embedder composites into one:
    /// the last completed frame is scrubbed with the realm, so no readback
    /// can return a pixel the dead shim painted. `'v` — see [`Self::scene`].
    ///
    /// Optional and a trait object for exactly the reasons [`Self::importer`]
    /// is: not every embedder has one, and `lifecycle` must not name a
    /// backend type.
    pub retained: Option<&'v mut dyn RetainedOutput>,
    /// The core's single realm registry: where the terminal state lands,
    /// and through it the single vacancy predicate.
    pub realms: &'a mut RealmRegistry,
    /// The run's single flight-recorder handle.
    pub recorder: &'a mut Recorder,
}

/// The compositor's **retained output**: the framebuffer a capture reads
/// back, as opposed to the [`Scene`] a frame is composed *from*.
///
/// The two go stale independently, and only one of them is self-healing.
/// [`Scene::compose`] is pure -- clear the surface and it yields the
/// deterministic background on the very next call -- but a retained
/// framebuffer is a *cache of a past composition*, and
/// [`crate::capture`]'s pixel source is documented as "always retained
/// output, never a fresh render". So a realm whose shim just died leaves
/// its last painted frame sitting in that buffer, byte for byte, until
/// something composites over it, and the headless backend composites only
/// on a scene commit -- which a dead realm will never send again.
///
/// # This is defense in depth, and deliberately not a second predicate
///
/// [`RealmLifecycle::view_is_live`] remains the *single* fact behind every
/// `no_surface` refusal, and the enforcement chokepoint remains the single
/// place that judges it. This trait adds no predicate, answers no authority
/// question, and is never consulted: it is a scrub, so that the bytes
/// behind the one predicate are harmless even if a future embedder wires
/// the predicate wrong. One boolean deep is thin for the property this
/// task exists to guarantee; a boolean plus "the buffer no longer holds
/// those pixels" is two.
pub(crate) trait RetainedOutput {
    /// Scrub the retained frame back to the empty-scene background.
    ///
    /// After this returns `Ok`, a readback cannot yield any pixel the dead
    /// realm's shim painted. Implementations composite an **empty**
    /// [`Scene`] through the same [`Scene::compose`] every frame goes
    /// through, rather than defining a second idea of what an unpainted
    /// realm looks like.
    fn scrub_retained_frame(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

/// **How the core hangs up on a shim.** Whatever holds the core's end of
/// the identity socketpair, reduced to the one operation the shutdown
/// ladder needs from it: release it, so the shim's `read` returns EOF.
///
/// # Why this is not just a `Connection`
///
/// It was, and for a core with no event loop that was the right shape:
/// `self.connection = None` in [`RealmLifecycle::die`] dropped the last
/// descriptor and *that drop* was rung 0/1 of the ladder -- the hangup a
/// compliant shim exits on, and the reason rungs 2 and 3 are rare.
///
/// The runtime loop cannot hold the connection that way. `ConnectionSource`
/// takes a `Connection` **by value** and calloop never gives it back, and
/// the alternative -- a borrowed-fd source -- throws away `vitrin-ipc`'s
/// `DisconnectReason` classification, which is the single funnel every
/// realm death is classified through. So at runtime the calloop
/// registration *is* the ownership, and hanging up means removing the
/// source.
///
/// Duplicating the descriptor to keep a second handle here would look like
/// the easy answer and is the one genuinely wrong one: two live core-side
/// fds means the shim never sees EOF, so rung 0 silently stops working and
/// every clean shutdown degrades to waiting out `hangup_grace` and then
/// `SIGTERM` -- slower, less polite, and invisible, because nothing fails
/// and nothing is logged.
///
/// Making this an enum rather than leaving the embedder to remember a
/// second call is deliberate for the same reason: a `RealmLifecycle`
/// holding neither the connection nor a way to close it would be a ladder
/// whose first rung is a no-op, and that is not a failure any test can see.
pub(crate) enum Hangup {
    /// The core still holds the socketpair end itself. Dropping the
    /// `Connection` closes it.
    Owned(Connection),
    /// The connection lives inside an event source; the action retires that
    /// registration, which drops the source and with it the `Connection`.
    ///
    /// Idempotent by construction on calloop: `LoopHandle::remove` of a
    /// token whose source has already removed itself is a no-op, because
    /// registration tokens are versioned slotmap keys and a retired one
    /// never matches a later occupant of the same slot. That is what lets
    /// [`RealmLifecycle::die`] fire this unconditionally without having to
    /// know which side of the socketpair closed first -- a question the
    /// death latch deliberately does not answer.
    Registered(Box<dyn FnOnce()>),
}

impl Hangup {
    /// The core holds the connection directly.
    pub fn owned(connection: Connection) -> Self {
        Hangup::Owned(connection)
    }

    /// An event-source registration owns the connection; `release` retires
    /// it.
    pub fn registered(release: impl FnOnce() + 'static) -> Self {
        Hangup::Registered(Box::new(release))
    }

    /// Release the core's end. Consumes `self` because a hangup happens
    /// once: the death latch in [`RealmLifecycle::die`] is what guarantees
    /// that, and taking by value is what makes the guarantee visible here.
    fn hang_up(self) {
        match self {
            // The drop *is* the hangup. Named rather than left implicit so
            // this does not read as a discarded value someone could tidy
            // into a `_`.
            Hangup::Owned(connection) => drop(connection),
            Hangup::Registered(release) => release(),
        }
    }
}

impl fmt::Debug for Hangup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hangup::Owned(_) => f.write_str("Hangup::Owned"),
            Hangup::Registered(_) => f.write_str("Hangup::Registered"),
        }
    }
}

/// One live realm, from the moment [`crate::spawn`] hands it over until
/// nothing of it is left: the core's end of the identity socketpair, the
/// shim process, the private runtime directory, and the realm `flock`.
///
/// Owning all four together is the point (module docs): they have to die in
/// an order, and the `flock` in particular has to outlive the directory it
/// protects.
pub(crate) struct RealmLifecycle {
    realm_id: RealmId,
    /// The shim's pid. Retained after reaping for the log only -- once
    /// [`Self::child`] is `None` this number must never be signalled,
    /// because the kernel may have recycled it.
    pid: u32,
    /// The unreaped process handle; `None` once `waitpid` has collected it.
    /// **The reap latch**: a taken handle cannot be waited on twice, and
    /// its presence is what makes signalling `pid` free of the pid-reuse
    /// race.
    child: Option<Child>,
    /// How this realm's core-side socketpair end gets released; `None` once
    /// the realm is dead, because releasing it is precisely what dying
    /// does. See [`Hangup`] for why this is an abstraction rather than the
    /// `Connection` it used to be.
    hangup: Option<Hangup>,
    runtime_dir: PathBuf,
    /// Held until the realm is fully torn down, and released *after* the
    /// runtime directory is removed (module docs). Never read: its
    /// existence is the effect.
    _realm_lock: fs::File,
    /// **The death latch**: `Some` from the first death observation
    /// onwards, whichever direction it came from. Everything the
    /// transition does happens exactly once, when this goes from `None`.
    death: Option<DeathCause>,
    /// The classified exit status, once the process has been reaped.
    exit: Option<ExitClass>,
    /// Every signal this core actually delivered to this shim, deduplicated.
    ///
    /// A *set of signals* rather than a "the core signalled at some point"
    /// latch, because [`Self::core_initiated`] has to answer a strictly
    /// narrower question -- did the core send **the signal that actually
    /// ended this process** -- and a latch answers a wider one that is
    /// false whenever the two differ. Two reachable paths make them differ,
    /// one of them on every ordinary crash:
    ///
    /// - The core asks a hung-up-but-living shim to leave (`SIGTERM`, in
    ///   [`Self::note_connection_closed`]) and an operator then `kill -9`s
    ///   it. A latch would report the resulting `signal: 9` as the core's
    ///   doing.
    /// - A plain external `kill -9` of a live shim. The kernel releases a
    ///   dying task's descriptors (`exit_files`) **before** it makes the
    ///   task reapable (`exit_notify`), so the socketpair EOF genuinely
    ///   precedes `waitpid` succeeding -- measured at 348/400 on this
    ///   kernel. `note_connection_closed` therefore finds the child
    ///   unreaped on the ordinary crash path and sends its `SIGTERM` into a
    ///   process already inside `do_exit`. Harmless as a signal, fatal as
    ///   evidence: a latch would mark the run's one honest record of an
    ///   external kill as core-initiated.
    ///
    /// Recorded only on a `kill(2)` that *succeeded*: a signal the kernel
    /// refused was never delivered and must not be claimed.
    core_signals: Vec<i32>,
    /// Whether the runtime tree has already been removed, so a second
    /// [`RealmLifecycle::shutdown`] does not log a failure to delete
    /// something that is correctly gone.
    runtime_dir_removed: bool,
}

impl RealmLifecycle {
    /// Adopt a freshly spawned realm. The realm is alive from here.
    ///
    /// `place` is handed the core's end of the identity socketpair and must
    /// answer with the [`Hangup`] that releases it again -- either
    /// [`Hangup::owned`], keeping it here, or [`Hangup::registered`] after
    /// moving it into an event source. Passing the connection *through* a
    /// closure rather than taking it as a field is the whole point: there is
    /// no way to construct a `RealmLifecycle` without saying how it hangs
    /// up, so the ladder's first rung cannot become a silent no-op (see
    /// [`Hangup`]).
    ///
    /// Fallible because placing the connection generally is -- registering
    /// an event source can fail. On `Err` the closure has already consumed
    /// the connection, so the shim gets its EOF and the caller is left with
    /// a realm it must tear down by process signal alone; the `SpawnedParts`
    /// are dropped here, which kills and reaps the child (`GuardedChild` has
    /// already been disarmed, so this is `Child`'s own drop -- the caller
    /// owns the refusal path).
    pub fn adopt<E>(
        parts: SpawnedParts,
        place: impl FnOnce(Connection) -> Result<Hangup, E>,
    ) -> Result<Self, E> {
        let hangup = place(parts.connection)?;
        Ok(Self {
            realm_id: parts.realm_id,
            pid: parts.child.id(),
            child: Some(parts.child),
            hangup: Some(hangup),
            runtime_dir: parts.runtime_dir,
            _realm_lock: parts.realm_lock,
            death: None,
            exit: None,
            core_signals: Vec::new(),
            runtime_dir_removed: false,
        })
    }

    pub fn realm_id(&self) -> &RealmId {
        &self.realm_id
    }

    /// The shim's pid. Still meaningful after death for the log; never a
    /// signal target once the process has been reaped.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the realm has not (yet) been observed to die.
    pub fn is_alive(&self) -> bool {
        self.death.is_none()
    }

    /// What ended the realm, if anything has.
    pub fn death_cause(&self) -> Option<DeathCause> {
        self.death
    }

    /// The classified exit status, once the shim has been reaped. `None`
    /// while it is alive **and** while it is dead-but-unreaped -- a shim
    /// that hung up and kept running is in the second state.
    pub fn exit(&self) -> Option<ExitClass> {
        self.exit
    }

    /// Whether the shim process has been reaped. When this is `true` there
    /// is provably no zombie for this realm: the core waited on it.
    pub fn is_reaped(&self) -> bool {
        self.child.is_none()
    }

    /// **Did the core send the signal that ended this shim?** The one
    /// question `realm_exited.core_initiated` answers, and the only thing
    /// that can distinguish the ladder's last rung from an operator's
    /// `kill -9` -- the signal number is `9` either way.
    ///
    /// Deliberately computed from the terminating signal rather than
    /// latched when a signal is sent (see [`Self::core_signals`]): the core
    /// signalling *something* at *some point* is a different and wider fact
    /// than the core having caused *this* death, and reporting the wider
    /// one would let the log claim kills the core did not make.
    ///
    /// `false` for a shim that ran to completion (`disposition: exited`),
    /// including one that exited from inside its own `SIGTERM` handler: no
    /// signal terminated it, so there is no signal to attribute. That the
    /// core drove the teardown is already stated, once, by
    /// `realm_died.cause = "shutdown"` -- this field is not a second way to
    /// say it.
    pub fn core_initiated(&self) -> bool {
        match self.exit {
            Some(ExitClass::Signaled { signal }) => self.core_signals.contains(&signal),
            _ => false,
        }
    }

    /// The runtime directory this realm was given, while it still exists.
    pub fn runtime_dir(&self) -> &std::path::Path {
        &self.runtime_dir
    }

    /// The core's end of the identity socketpair, for an embedder that kept
    /// it here ([`Hangup::Owned`]) to read and write directly. `None` once
    /// the realm is dead -- which is also how such an embedder learns to
    /// stop servicing it.
    ///
    /// Also `None`, while perfectly alive, for a realm whose connection was
    /// placed in an event source ([`Hangup::Registered`]): there the source
    /// owns it and reaching it outside a dispatch callback is precisely what
    /// must not be possible. Callers that service the socketpair by hand --
    /// this module's own tests -- are the ones this exists for; the runtime
    /// loop never calls it.
    pub fn connection_mut(&mut self) -> Option<&mut Connection> {
        match self.hangup.as_mut() {
            Some(Hangup::Owned(connection)) => Some(connection),
            Some(Hangup::Registered(_)) | None => None,
        }
    }

    /// **Does this realm have a live view?** The single place the embedder
    /// derives `ServerCtx::realm_view`'s `Some`/`None` from, and therefore
    /// the fact behind every `no_surface` refusal (module docs).
    ///
    /// This is deliberately a *fact*, not a judgement: it reports whether
    /// there is anything to observe, and [`crate::enforcement`] alone
    /// decides what that means for authority. Adding a second
    /// authority-checking path is the one thing this crate does not do.
    ///
    /// Both conjuncts matter, and the first is defense in depth. A death
    /// already clears the scene through [`ShimServer::connection_closed`],
    /// so the second conjunct alone would usually be enough -- but "usually"
    /// is not the standard for the predicate that decides whether a dead
    /// realm can be photographed. A realm that died before its shim session
    /// ever came up has no `ShimServer` to run the funnel through, and a
    /// future embedder that composites something of its own into a realm's
    /// view would silently re-open the stale-frame hole. Fail closed: dead
    /// realms have no view, whatever the scene holds.
    pub fn view_is_live(&self, scene: &Scene) -> bool {
        self.death.is_none() && scene.surface_size().is_some()
    }

    /// The shim connection ended: end-of-stream, a transport fault, or a
    /// protocol violation. Returns whether *this* call was the death (i.e.
    /// the realm was not already dead).
    ///
    /// Runs inside the compositor loop, so it never waits for anything: the
    /// realm is marked dead and its surface removed synchronously (the
    /// security-relevant part, and it is all non-blocking), the process is
    /// reaped if it is already a corpse, and a shim that outlived its
    /// connection is merely *asked* to leave -- the following `SIGCHLD`
    /// collects it. The shutdown ladder finishes anything still standing.
    pub fn note_connection_closed<H: PreemptionHook>(
        &mut self,
        cause: DeathCause,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) -> bool {
        debug_assert!(
            matches!(
                cause,
                DeathCause::ConnectionClosed | DeathCause::ConnectionFault
            ),
            "note_connection_closed describes the connection ending, not {cause:?}"
        );
        let died = self.die(cause, teardown);
        // It may already be a corpse -- the ordinary case, where the shim
        // exited and the kernel closed fd 3 on the way out. Reap before
        // signalling so a zombie is never sent a signal it cannot receive.
        self.poll_exit(teardown);
        if self.child.is_some() {
            // A shim that outlived its core connection can never paint
            // again and must not keep holding the realm's runtime tree.
            // Asked, not killed: it may be mid-teardown of its own app,
            // and the ladder escalates later if it is not.
            //
            // "not been reaped", NOT "is still running": reaching here does
            // not mean the shim is alive. The kernel releases a dying
            // task's descriptors before it makes the task reapable, so on
            // an ordinary `kill -9` the EOF above arrives while the shim is
            // still inside `do_exit` and this `SIGTERM` lands on a process
            // that is already leaving. That costs nothing -- but it is
            // exactly why [`Self::core_initiated`] is derived from the
            // terminating signal instead of being latched here.
            tracing::info!(
                realm = %self.realm_id,
                pid = self.pid,
                "shim connection ended and its process has not been reaped; asking it to exit"
            );
            self.signal(libc::SIGTERM);
        }
        died
    }

    /// A `SIGCHLD` hint arrived (or the caller wants to check anyway):
    /// non-blocking `waitpid`. Returns whether *this* call reaped the shim.
    ///
    /// Safe and cheap to call speculatively, which is the point -- a
    /// `SIGCHLD` says only that *some* child changed state, so the reaper
    /// polls every realm it owns and lets `waitpid` be the authority
    /// (module docs). Already-reaped realms answer `false` immediately.
    pub fn poll_exit<H: PreemptionHook>(
        &mut self,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) -> bool {
        let Some(child) = self.child.as_mut() else {
            // Already reaped: the latch. Not an error, and not a second
            // death.
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.collect_exit(status, teardown);
                true
            }
            Ok(None) => false,
            Err(err) => {
                // The child is still ours and still unreaped; the shutdown
                // ladder will try again. Never fatal: a realm the core
                // cannot wait on must not take the session down.
                tracing::error!(
                    realm = %self.realm_id,
                    pid = self.pid,
                    %err,
                    "waitpid on the realm's shim failed"
                );
                false
            }
        }
    }

    /// Tear this realm down on the way out of the session: the ladder in
    /// the module docs (hang up, wait, `SIGTERM`, wait, `SIGKILL`), then
    /// remove the realm's runtime tree.
    ///
    /// **Blocks**, by design and only here: this runs after the event loop
    /// has stopped. Returns which rung ended the shim, which is worth
    /// logging -- a realm that regularly reaches [`ShutdownRung::Kill`] is
    /// a shim with a broken `SIGTERM` handler.
    pub fn shutdown<H: PreemptionHook>(
        &mut self,
        timing: ShutdownTiming,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) -> ShutdownRung {
        // Rung 0: the realm is dead as far as the core is concerned, and
        // the core's end of the socketpair closes inside `die` -- which is
        // the shim's own EOF and, for a compliant shim, the entire
        // shutdown signal.
        self.die(DeathCause::Shutdown, teardown);

        let rung = if self.child.is_none() {
            ShutdownRung::AlreadyDead
        } else if self.wait_for_exit(timing.hangup_grace, teardown) {
            ShutdownRung::Hangup
        } else {
            // Rung 2: ask. Real rather than decorative only because the
            // spawn path resets both the child's signal dispositions and
            // its signal mask (module docs).
            self.signal(libc::SIGTERM);
            if self.wait_for_exit(timing.term_grace, teardown) {
                ShutdownRung::Term
            } else {
                // Rung 3: insist. A shim may legitimately have installed a
                // SIGTERM handler; one that is still running after the
                // whole grace period is not going to finish.
                tracing::warn!(
                    realm = %self.realm_id,
                    pid = self.pid,
                    grace_ms = timing.term_grace.as_millis(),
                    "shim ignored SIGTERM for the whole grace period; sending SIGKILL \
                     (its app, if any, is orphaned -- see crate::lifecycle)"
                );
                self.force_kill(teardown);
                ShutdownRung::Kill
            }
        };

        // Only now, with the shim dead by every rung's construction (the
        // ladder ends in `SIGKILL`, which cannot be refused), and only on
        // this orderly path -- a crash keeps the tree (module docs). The
        // realm `flock` is still held and is released after this, when
        // `self` drops: a second core must not be able to call this tree
        // stale while it is being taken apart.
        self.remove_runtime_dir();
        rung
    }

    /// **The single death funnel.** Latched on [`Self::death`]: the first
    /// observation from either direction does the whole transition, every
    /// later one is a no-op. Returns whether this call was the death.
    fn die<H: PreemptionHook>(
        &mut self,
        cause: DeathCause,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) -> bool {
        if self.death.is_some() {
            return false;
        }
        self.death = Some(cause);

        // 1. The surface leaves the scene -- through the pre-existing
        //    teardown funnel, never a second one. This is what makes the
        //    chokepoint's `no_surface` true: after it there is no committed
        //    surface, no retained zero-copy content, and no seat state.
        match teardown.shim.take() {
            // `importer.take()`, not a reborrow: the importer is cleared
            // exactly once, with the realm, and this body runs exactly once
            // by the latch above. (A reborrow would also not compile --
            // `&mut` is invariant, so shortening the borrow inside a
            // `&mut RealmTeardown` is not allowed.)
            Some(shim) => {
                shim.connection_closed(teardown.scene, teardown.importer.take(), teardown.router)
            }
            None => {
                // The realm died before its shim session came up, so there
                // is no server to run the funnel through. Do the same
                // thing by hand rather than skipping it: a scene with no
                // surface is the invariant, not a side effect of having
                // had a server.
                teardown.scene.clear_surface();
                teardown.router.reset();
            }
        }

        // 1b. ...and the *retained* frame is scrubbed, which clearing the
        //     scene does not do. `Scene::compose` is pure, so the scene is
        //     already harmless; a backend framebuffer is a cache of a past
        //     composition and still holds the dead realm's last painted
        //     frame until something composites over it (see
        //     [`RetainedOutput`]). `take()` for the same reason the
        //     importer above uses it.
        //
        //     A failure here is loud but not fatal, and explicitly not
        //     fail-open: the chokepoint's `no_surface` refusal rests on
        //     `view_is_live`, which is already `false` -- this step only
        //     removes the bytes that refusal is protecting.
        if let Some(retained) = teardown.retained.take() {
            if let Err(err) = retained.scrub_retained_frame() {
                tracing::error!(
                    realm = %self.realm_id,
                    %err,
                    "could not scrub the dead realm's retained frame; captures still refuse \
                     no_surface (view_is_live is false), but the last painted frame is still \
                     resident in the backend's framebuffer"
                );
            }
        }

        // 2. The realm stops admitting petitions, through the single
        //    vacancy predicate (`Realm::admits_petitions`). No restart
        //    policy: this state is terminal.
        if !teardown.realms.mark_exited(&self.realm_id, self.pid) {
            // A realm the registry does not know is a wiring bug, not a
            // runtime condition -- but the surface is already down, which
            // is the fail-closed half.
            tracing::error!(
                realm = %self.realm_id,
                "died but is not in the realm registry; its surface is down regardless"
            );
        }

        // 3. Journal it. Exactly one of these per realm, by the latch.
        teardown.recorder.record(Event::RealmDied {
            realm: &self.realm_id,
            pid: self.pid,
            cause: cause.label(),
        });
        tracing::info!(
            realm = %self.realm_id,
            pid = self.pid,
            cause = cause.label(),
            "realm died; its surface has left the scene and captures now refuse no_surface"
        );

        // 4. The core's end of the identity socketpair closes here. For a
        //    shim that is still running this is *its* EOF -- exit
        //    propagation downward -- and for one that is already gone it
        //    is simply the last reference.
        //
        //    Unconditional, whichever side closed first and whichever
        //    variant holds it: dropping an already-EOF'd `Connection` is a
        //    no-op, and so is removing a calloop registration a source has
        //    already retired (see [`Hangup::Registered`]). The death latch
        //    above is what makes it happen exactly once.
        if let Some(hangup) = self.hangup.take() {
            hangup.hang_up();
        }
        true
    }

    /// Turn a reaped child's status into realm state and log entries: the
    /// single place a `waitpid` result is interpreted.
    fn collect_exit<H: PreemptionHook>(
        &mut self,
        status: ExitStatus,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) {
        // The reap latch: taken before anything else can fail, so no path
        // can wait on this child twice.
        self.child = None;
        let class = ExitClass::of(status);
        self.exit = Some(class);

        // The process ending is a death observation like any other and
        // goes through the same latch -- so a realm whose EOF arrived
        // first is not buried twice, and one whose SIGCHLD arrived first
        // is buried here.
        self.die(DeathCause::ProcessExit, teardown);

        teardown.recorder.record(Event::RealmExited {
            realm: &self.realm_id,
            pid: self.pid,
            disposition: class.disposition(),
            code: class.code(),
            signal: class.signal(),
            core_initiated: self.core_initiated(),
        });
        tracing::info!(
            realm = %self.realm_id,
            pid = self.pid,
            core_initiated = self.core_initiated(),
            "realm shim reaped: {class}"
        );
    }

    /// Poll for the shim's exit until `budget` runs out. Returns whether it
    /// exited in time. A ceiling, not a delay -- it returns the moment the
    /// child is reaped.
    fn wait_for_exit<H: PreemptionHook>(
        &mut self,
        budget: Duration,
        teardown: &mut RealmTeardown<'_, '_, H>,
    ) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            if self.child.is_none() {
                return true;
            }
            if self.poll_exit(teardown) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(REAP_POLL_INTERVAL);
        }
    }

    /// `SIGKILL` and a blocking wait. The rung that always terminates:
    /// `SIGKILL` can be neither caught, blocked, nor ignored, so the wait
    /// cannot hang on a cooperative peer.
    fn force_kill<H: PreemptionHook>(&mut self, teardown: &mut RealmTeardown<'_, '_, H>) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // `Child::kill` rather than a raw `kill(2)`: std owns this handle's
        // reaped-ness and refuses to signal a pid it has already waited on.
        match child.kill() {
            Ok(()) => self.note_core_signal(libc::SIGKILL),
            Err(err) => {
                tracing::error!(realm = %self.realm_id, pid = self.pid, %err, "SIGKILL failed")
            }
        }
        match child.wait() {
            Ok(status) => {
                // Put the handle back so `collect_exit` owns the whole
                // reap-and-latch sequence in one place; it takes it again
                // immediately. Cheap, and it keeps a single writer of the
                // latch.
                self.child = Some(child);
                self.collect_exit(status, teardown);
            }
            Err(err) => {
                // Put the handle back too. An uncollected status *is* a
                // zombie, so leaving the handle dropped here would make
                // `is_reaped()` -- documented as "provably no zombie: the
                // core waited on it" -- assert exactly the thing that did
                // not happen, and would strand the realm with no
                // `realm_exited` entry, which the recorder documents as the
                // log's evidence that nothing was left behind. Restored, the
                // guarantee stays honest and `Drop`'s last-resort wait still
                // has something to collect with.
                self.child = Some(child);
                tracing::error!(
                    realm = %self.realm_id,
                    pid = self.pid,
                    %err,
                    "waiting on the SIGKILLed shim failed; it may be left unreaped"
                );
            }
        }
    }

    /// Remember a signal this core actually delivered to the shim, for
    /// [`Self::core_initiated`]. Deduplicated because the ladder can send
    /// the same signal twice and the set is only ever asked a membership
    /// question.
    fn note_core_signal(&mut self, signal: i32) {
        if !self.core_signals.contains(&signal) {
            self.core_signals.push(signal);
        }
    }

    /// Send `signal` to this realm's shim, if it is still unreaped.
    ///
    /// The `self.child.is_some()` guard is load-bearing, not defensive: an
    /// unreaped child's pid is reserved by the kernel until it is waited
    /// on, so it cannot have been recycled onto an unrelated process.
    /// Signalling a reaped pid would be signalling a stranger.
    fn signal(&mut self, signal: i32) {
        if self.child.is_none() {
            return;
        }
        let Ok(pid) = libc::pid_t::try_from(self.pid) else {
            tracing::error!(realm = %self.realm_id, pid = self.pid, "pid is out of range for kill(2)");
            return;
        };
        // SAFETY: `kill(2)` takes two integers, touches no memory, and
        // cannot alias anything. `pid` names a process this core forked and
        // has not reaped (checked immediately above), so it is this realm's
        // shim and not a recycled stranger.
        let rc = unsafe { libc::kill(pid, signal) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!(realm = %self.realm_id, pid = self.pid, signal, %err, "kill failed");
            return;
        }
        // Recorded *after* the syscall succeeded, never before it: this set
        // is evidence for [`Self::core_initiated`], and a signal the kernel
        // refused was never delivered.
        self.note_core_signal(signal);
    }

    /// Remove the realm's private runtime tree -- orderly shutdown only
    /// (module docs). Best effort with a loud failure: what is left behind
    /// is purged by the next start's stale check either way.
    fn remove_runtime_dir(&mut self) {
        if self.runtime_dir_removed {
            return;
        }
        self.runtime_dir_removed = true;
        match spawn::remove_runtime_dir(&self.runtime_dir) {
            Ok(()) => tracing::debug!(
                realm = %self.realm_id,
                path = %self.runtime_dir.display(),
                "realm runtime directory removed"
            ),
            Err(detail) => tracing::warn!(
                realm = %self.realm_id,
                path = %self.runtime_dir.display(),
                "realm runtime directory {detail}; the next start's stale purge will collect it"
            ),
        }
    }
}

impl Drop for RealmLifecycle {
    /// Last-resort hygiene, not a policy path: a [`RealmLifecycle`] dropped
    /// while its shim is still unreaped must not leave a zombie or a
    /// runaway behind. Reachable two ways -- a drop that skipped
    /// [`RealmLifecycle::shutdown`] entirely (an embedder bug, or a panic
    /// unwinding past it), and the ladder's own `wait` failing after it
    /// delivered `SIGKILL`, which puts the handle back rather than dropping
    /// it precisely so this net still has something to collect. `SIGKILL`
    /// and a blocking wait, because there is no longer anywhere to report a
    /// polite failure to.
    ///
    /// This deliberately does *not* remove the runtime directory: that
    /// decision belongs to `shutdown`, which knows the exit was orderly.
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        tracing::warn!(
            realm = %self.realm_id,
            pid = self.pid,
            "realm dropped with its shim still unreaped; SIGKILLing and waiting so no \
             zombie is left behind"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The core's `SIGCHLD` event source: `calloop`'s own `signalfd`-backed
/// [`Signals`], restricted to `SIGCHLD`.
///
/// **There is no signal handler.** `signalfd` turns signal delivery into
/// ordinary file-descriptor readability, dispatched by the same event loop
/// as everything else, so the async-signal-safety question -- what may a
/// handler do? -- never arises: nothing runs in signal context at all. That
/// is why this is `calloop`'s source rather than a self-pipe of our own,
/// and it costs no new dependency (`calloop` is the compositor's event loop
/// and its `signals` feature is already enabled for the backends'
/// `SIGINT`/`SIGTERM` shutdown source).
///
/// # The one obligation: `SIGCHLD` must be blocked on **every** thread
///
/// `signalfd` only ever sees a signal that is blocked. A process-directed
/// signal is delivered to *some* thread that has it unblocked, and
/// `SIGCHLD`'s default action is to be discarded -- so a single thread with
/// `SIGCHLD` unblocked is enough to make this source silently miss child
/// deaths. `Signals::new` masks the signal for the thread that calls it,
/// and threads inherit their creator's mask, so **this must be constructed
/// on the main thread before any thread is spawned** -- the same
/// requirement, and the same placement, as the backends' existing
/// `SIGINT`/`SIGTERM` source. The core really is multi-threaded (Smithay's
/// backends and their EGL/GL stacks spawn threads), so this is a live
/// hazard rather than a formality.
///
/// It is also why this module treats `SIGCHLD` as a *hint* and never as
/// evidence: [`RealmLifecycle::poll_exit`]'s `waitpid` is the authority,
/// and a missed signal costs a delayed reap, never a zombie -- the realm's
/// EOF path and the shutdown ladder both reap unconditionally.
///
/// The mask this installs is inherited across `fork` **and** `execve`,
/// which would leave every spawned realm unable to reap its own children;
/// [`crate::spawn`]'s post-fork closure clears the child's mask outright,
/// which is what keeps that from happening.
pub(crate) fn child_signal_source() -> calloop::Result<Signals> {
    Signals::new(&[Signal::SIGCHLD])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::os::fd::AsFd;
    use std::path::{Path, PathBuf};

    use vitrin_ipc::TransportError;
    use vitrin_protocol::generated::vitrin_grant::Refusal;

    use super::*;
    use std::ffi::OsString;

    use crate::capture::tests::fd_lock;
    use crate::input::NoopHook;
    use crate::realm::tests::realm_with_spawn;
    use crate::recorder::tests::{of_kind, read_log, scratch_recorder, Json};
    use crate::scene::Scene;
    use crate::shim::ShimServer;
    use crate::spawn::tests::{children_of, mock_shim_bin, ppid_of, scratch, wait_for, DEADLINE};
    use crate::spawn::{spawn_realm_with_env, SpawnPaths};

    /// The realm view every rig configures its shim at.
    const VIEW_W: u32 = 64;
    const VIEW_H: u32 = 48;

    /// Escalation timing for the ladder tests: short, because what is
    /// under test is *which rung a given shim ends on*, never the clock.
    /// The production values are asserted separately
    /// (`the_documented_grace_periods_are_the_defaults`) so shortening one
    /// here cannot quietly become shortening it in production.
    fn quick_timing() -> ShutdownTiming {
        ShutdownTiming {
            hangup_grace: Duration::from_millis(200),
            term_grace: Duration::from_millis(300),
        }
    }

    /// Number of open descriptors in this process, from `/proc/self/fd`.
    /// The `read_dir` handle appears in every measurement identically, so
    /// equality comparisons are exact. Only ever compared under
    /// [`fd_lock`], which every descriptor-opening test in this crate
    /// holds.
    fn open_fd_count() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd is readable on Linux")
            .count()
    }

    /// Descriptors currently open onto anything under `path`, by
    /// `readlink` target.
    ///
    /// The *attributable* half of the no-leak assertion, and the half that
    /// cannot be perturbed by an unrelated concurrent test: each rig owns a
    /// uniquely named scratch tree, so a descriptor whose target names it
    /// -- the realm lock, a runtime-directory handle, the shim's
    /// app-facing socket -- provably belongs to this realm's cycle and to
    /// nothing else. The process-global count beside it is the broader but
    /// weaker check.
    fn fds_under(path: &Path) -> Vec<String> {
        let needle = path.to_string_lossy().into_owned();
        std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd is readable on Linux")
            .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
            .map(|t| t.to_string_lossy().into_owned())
            .filter(|t| t.contains(&needle))
            .collect()
    }

    /// Whether `pid` is a process this core has not reaped -- i.e. alive
    /// **or a zombie**. Reading `/proc/<pid>/stat`'s state field rather
    /// than merely testing the directory's existence is the whole point:
    /// a zombie's `/proc` entry exists, so "the directory is gone" would
    /// pass for exactly the failure this suite must catch.
    fn proc_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The comm field may contain ')' and spaces, so the state is the
        // first token after the LAST ')'.
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().next()?.chars().next()
    }

    /// Is this pid a zombie right now?
    fn is_zombie(pid: u32) -> bool {
        proc_state(pid) == Some('Z')
    }

    /// Every unreaped child of this process, from procfs. `waitpid` is not
    /// usable for this assertion: the libtest harness is multi-threaded
    /// and other tests own children of their own, so a bare
    /// `waitpid(-1, WNOHANG)` would steal them. Procfs answers the exact
    /// question -- is *this* pid still ours and uncollected -- without
    /// touching anyone else's.
    fn unreaped_children() -> BTreeSet<u32> {
        children_of(std::process::id())
    }

    /// A stand-in for the backend's retained framebuffer, counting the
    /// scrubs the death funnel drives.
    ///
    /// A fake here and the real thing in `backend::headless` -- this file
    /// asserts that the funnel *drives the seam*, exactly once and on every
    /// death path; `backend::headless`'s own test asserts that the seam
    /// really removes the painted pixels. Splitting it that way keeps a
    /// pixman renderer out of the lifecycle suite while leaving both halves
    /// of the property pinned.
    struct FakeRetained {
        scrubs: usize,
        /// Make the scrub fail, to prove a failing backend cannot take the
        /// session down or stop the realm from dying.
        fails: bool,
    }

    impl FakeRetained {
        fn new() -> Self {
            Self {
                scrubs: 0,
                fails: false,
            }
        }
    }

    impl RetainedOutput for FakeRetained {
        fn scrub_retained_frame(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            self.scrubs += 1;
            if self.fails {
                return Err("the backend refused to recomposite".into());
            }
            Ok(())
        }
    }

    /// One lifecycle test's world: a scratch runtime tree, the run's
    /// flight recorder, and the realm-side state a death has to touch.
    struct Rig {
        base: PathBuf,
        recorder: Recorder,
        log: PathBuf,
        scene: Scene,
        shim: Option<ShimServer>,
        router: InputRouter<NoopHook>,
        retained: FakeRetained,
        realms: RealmRegistry,
    }

    impl Rig {
        fn new(label: &str) -> Self {
            let (recorder, log) = scratch_recorder(label);
            Self {
                base: scratch(),
                recorder,
                log,
                scene: Scene::new(),
                shim: None,
                router: InputRouter::new(NoopHook),
                retained: FakeRetained::new(),
                realms: crate::realm::tests::registry_with(&["realm-0"]),
            }
        }

        /// The bundle every death funnel takes.
        fn teardown(&mut self) -> RealmTeardown<'_, '_, NoopHook> {
            RealmTeardown {
                scene: &mut self.scene,
                shim: &mut self.shim,
                importer: None,
                router: &mut self.router,
                retained: Some(&mut self.retained),
                realms: &mut self.realms,
                recorder: &mut self.recorder,
            }
        }

        /// Every recorded entry, parsed. Closes the log first: a
        /// footer-less file is not what a reader would ever see.
        fn entries(&mut self) -> Vec<Json> {
            self.recorder.finish();
            read_log(&self.log)
        }

        /// Spawn the mock shim into this rig, bring its session up, and
        /// adopt it. The returned realm is live, its connection
        /// non-blocking (so [`service`] can drain without parking), and
        /// its `ShimServer` is in `self.shim` where the death funnel will
        /// consume it.
        fn spawn(&mut self, args: &[&str]) -> RealmLifecycle {
            let bin = mock_shim_bin();
            let realm = realm_with_spawn(
                "realm-0",
                &bin,
                &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                &[],
            );
            // The mock is both the `--shim` binary (direct child, fd 3) and
            // the `command` app stand-in; the fixture flags in `args` ride the
            // app-argument tail, which the mock scans (issue #103).
            let paths = SpawnPaths::under(&self.base, &bin);
            let mut spawned = spawn_realm_with_env(&realm, &paths, &mut self.recorder, |_| None)
                .expect("spawn must succeed against a scratch runtime tree");
            assert!(self.realms.mark_running(spawned.realm_id(), spawned.pid()));

            // Bring-up, which is also the readiness gate: a `create_surface`
            // arriving from the child proves it is executing its own code
            // (P1.5.2's lesson -- `/proc/<pid>/exe` flipping only proves
            // the kernel installed the image, with ld.so still running).
            let mut server = spawned
                .start_shim_session(VIEW_W, VIEW_H)
                .expect("configure must reach the shim over the inherited socketpair");
            let msg = spawned
                .connection_mut()
                .recv_message()
                .expect("the shim's first request must arrive")
                .expect("the shim must not hang up during bring-up");
            {
                let conn = spawned.connection_mut();
                server
                    .handle_message(msg, &mut self.scene, None, &mut |bytes: &[u8]| {
                        conn.send_message(bytes, None)
                    })
                    .expect("the shim's bring-up request must be well-formed");
            }
            self.shim = Some(server);

            // These tests service the socketpair by hand rather than
            // through an event loop, so the lifecycle keeps the connection
            // itself: `Hangup::Owned` is the variant `connection_mut` can
            // answer for, and dropping it is still rung 0.
            let mut life = RealmLifecycle::adopt(spawned.into_parts(), |conn| {
                Ok::<_, std::convert::Infallible>(Hangup::owned(conn))
            })
            .expect("placing an owned connection cannot fail");
            set_nonblocking(
                life.connection_mut()
                    .expect("a fresh realm has a connection"),
            );
            assert_eq!(life.realm_id().as_str(), "realm-0");
            life
        }

        /// Spawn an arbitrary program as this realm's shim and adopt it,
        /// **without** session bring-up.
        ///
        /// The ladder and hangup tests need a peer that behaves in a
        /// specific way at the *process* level -- ignores `SIGTERM`, or
        /// closes fd 3 and keeps running -- not one that speaks the shim
        /// protocol. `/bin/sh` is the fixture for those, and doing it with
        /// a shell rather than teaching the mock shim to misbehave keeps
        /// `unsafe` signal manipulation out of the fixture crate. It is
        /// the same trick `crate::spawn`'s own `SIG_IGN` test uses, for
        /// the same reason: `trap "" TERM` is a genuine `SIG_IGN`
        /// installed by the child itself after `execve`, so the peer is
        /// really unkillable by `SIGTERM` rather than simulating it.
        ///
        /// Since issue #103 the realm's **direct child is the shim**, so this
        /// misbehaving process is the `--shim` binary: `command`+`args` are
        /// the shim's own argv (`/bin/sh -c <script>`), placed before the
        /// `--`/app tail. The realm's `command` (the app) is an inert,
        /// audit-passing mock the shell never reaches -- the trailing `--
        /// <app>` are positional parameters the `-c <script>` ignores.
        fn spawn_program(&mut self, command: &str, args: &[&str]) -> RealmLifecycle {
            let realm = realm_with_spawn("realm-0", &mock_shim_bin(), &[], &[]);
            let mut shim_argv: Vec<OsString> = vec![OsString::from(command)];
            shim_argv.extend(args.iter().map(OsString::from));
            let paths = SpawnPaths::under_with_shim_argv(&self.base, shim_argv);
            let spawned = spawn_realm_with_env(&realm, &paths, &mut self.recorder, |_| None)
                .expect("spawn must succeed against a scratch runtime tree");
            assert!(self.realms.mark_running(spawned.realm_id(), spawned.pid()));
            // These tests service the socketpair by hand rather than
            // through an event loop, so the lifecycle keeps the connection
            // itself: `Hangup::Owned` is the variant `connection_mut` can
            // answer for, and dropping it is still rung 0.
            let mut life = RealmLifecycle::adopt(spawned.into_parts(), |conn| {
                Ok::<_, std::convert::Infallible>(Hangup::owned(conn))
            })
            .expect("placing an owned connection cannot fail");
            set_nonblocking(
                life.connection_mut()
                    .expect("a fresh realm has a connection"),
            );
            assert_eq!(life.realm_id().as_str(), "realm-0");
            life
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
            crate::recorder::tests::cleanup(&self.log);
        }
    }

    fn set_nonblocking(conn: &Connection) {
        let flags = rustix::fs::fcntl_getfl(conn.as_fd()).expect("fcntl F_GETFL");
        rustix::fs::fcntl_setfl(conn.as_fd(), flags | rustix::fs::OFlags::NONBLOCK)
            .expect("fcntl F_SETFL O_NONBLOCK");
    }

    /// One turn of the embedder's shim-connection service: drain whatever
    /// is pending into the `ShimServer`, then release any owed
    /// `frame_done` so the shim's paced animation can advance.
    ///
    /// Returns `Some(cause)` when the connection ended -- exactly the EOF /
    /// fault classification the M1.1 event source will make from
    /// `vitrin-ipc`'s `DisconnectReason` funnel.
    fn service(rig: &mut Rig, life: &mut RealmLifecycle) -> Option<DeathCause> {
        loop {
            // `?` would read as "no death", which is the right answer:
            // a realm whose connection the funnel already closed has
            // nothing left to service.
            let conn = life.connection_mut()?;
            let msg = match conn.recv_message() {
                Ok(Some(msg)) => msg,
                Ok(None) => return Some(DeathCause::ConnectionClosed),
                Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Nothing pending: release owed frame_dones and go.
                    if let Some(server) = rig.shim.as_mut() {
                        if server.wants_presentation() {
                            if let Err(err) = server
                                .presented(0, &mut |bytes: &[u8]| conn.send_message(bytes, None))
                            {
                                // Writing to a peer that is already gone:
                                // the usual way a `kill -9` surfaces first.
                                return Some(DeathCause::of_transport(err));
                            }
                        }
                    }
                    return None;
                }
                // Through the transport's own classification, which is
                // what the M1.1 event source will hand the funnel -- so
                // this exercises the real `DisconnectReason -> DeathCause`
                // mapping rather than a test-local opinion of it.
                Err(err) => return Some(DeathCause::of_transport(err)),
            };
            let Some(server) = rig.shim.as_mut() else {
                continue;
            };
            if let Err(fault) =
                server.handle_message(msg, &mut rig.scene, None, &mut |bytes: &[u8]| {
                    conn.send_message(bytes, None)
                })
            {
                return Some(DeathCause::of_shim_fault(fault));
            }
        }
    }

    /// Service the realm until its scene holds a committed client surface
    /// -- i.e. the realm has really painted, over the real wire, from a
    /// real forked shim. Without this a "the surface left the scene" test
    /// would be asserting that nothing became nothing.
    fn wait_until_painted(rig: &mut Rig, life: &mut RealmLifecycle) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            assert_eq!(
                service(rig, life),
                None,
                "the shim must stay connected while it paints"
            );
            if rig.scene.surface_size().is_some() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the realm to commit a surface"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Service the realm until its connection ends, then run the death
    /// funnel exactly as the M1.1 event source will. Returns the cause.
    fn wait_for_death(rig: &mut Rig, life: &mut RealmLifecycle) -> DeathCause {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(cause) = service(rig, life) {
                life.note_connection_closed(cause, &mut rig.teardown());
                return cause;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the shim connection to end"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Poll until the shim has been reaped (or fail). Stands in for the
    /// `SIGCHLD` source's callback, which does exactly this call.
    fn reap_within_deadline(rig: &mut Rig, life: &mut RealmLifecycle) {
        let deadline = Instant::now() + DEADLINE;
        while !life.is_reaped() {
            life.poll_exit(&mut rig.teardown());
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the shim to be reaped"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // -- acceptance: kill -9 mid-capture-loop ------------------------------

    #[test]
    fn kill_dash_9_mid_capture_loop_updates_the_scene_and_leaks_no_fd() {
        // The M1.3 chaos test, end to end and for real: a forked shim is
        // genuinely animating over the wire, gets SIGKILLed mid-frame, and
        // the core (a) survives, (b) logs it, (c) drops the surface, and
        // (d) gives every descriptor back.
        let _fd = fd_lock();
        let baseline = open_fd_count();
        let (pid, base_dir, entries) = {
            let mut rig = Rig::new("lifecycle-chaos");
            let mut life = rig.spawn(&["--serve", "--animate", "100000"]);
            let pid = life.pid();
            let base_dir = rig.base.clone();
            assert!(
                !fds_under(&base_dir).is_empty(),
                "a live realm really does hold descriptors into its own runtime tree \
                 (the realm lock at least), or the leak check below proves nothing"
            );

            // The realm really paints first: "the scene updates on death"
            // is only an assertion if something was on the scene, and
            // "never a stale frame" only means something if a real frame
            // was there to go stale.
            wait_until_painted(&mut rig, &mut life);
            assert!(life.view_is_live(&rig.scene), "the realm must be live");
            let painted = rig.scene.compose(VIEW_W, VIEW_H);
            assert_ne!(
                painted,
                crate::test_pattern::render(VIEW_W, VIEW_H),
                "the shim must have committed real content, not the empty-scene background"
            );

            // kill -9, mid-frame: the shim is inside its paced animation
            // loop with a commit outstanding.
            assert_eq!(
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) },
                0,
                "the shim must be killable"
            );

            let cause = wait_for_death(&mut rig, &mut life);
            assert_eq!(cause, DeathCause::ConnectionClosed);
            reap_within_deadline(&mut rig, &mut life);

            // (a) The core is alive -- it is executing this line, and it
            // got here without a SIGPIPE from writing to a dead peer.
            // (c) The scene updated: no surface, so nothing composites
            // client content ever again.
            assert!(!life.is_alive());
            assert!(!life.view_is_live(&rig.scene));
            assert_eq!(
                rig.scene.surface_size(),
                None,
                "the dead realm's surface must have left the scene"
            );
            assert!(
                rig.shim.is_none(),
                "the death funnel must consume the shim server"
            );

            // The exit is classified as what it was: a signal, number 9,
            // and NOT core-initiated -- the core observed this kill, it
            // did not send it.
            assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 9 }));
            assert!(!life.exit().unwrap().is_clean());
            assert!(!life.core_initiated());

            // Orderly shutdown of an already-dead realm: nothing to kill.
            assert_eq!(
                life.shutdown(quick_timing(), &mut rig.teardown()),
                ShutdownRung::AlreadyDead
            );
            (pid, base_dir, rig.entries())
        };

        // (b) It is in the log, once, with the exit classified.
        let died = of_kind(&entries, "realm_died");
        assert_eq!(died.len(), 1, "one death, one entry");
        assert_eq!(died[0].str("realm"), "realm-0");
        assert_eq!(died[0].u64("pid"), u64::from(pid));
        assert_eq!(died[0].str("cause"), "connection_closed");
        assert!(!died[0].bool("restarting"), "the MVP has no restart policy");

        let exited = of_kind(&entries, "realm_exited");
        assert_eq!(exited.len(), 1, "one process, one reap");
        assert_eq!(exited[0].str("disposition"), "signaled");
        assert_eq!(exited[0].u64("signal"), 9);
        assert!(exited[0].is_null("code"));
        assert!(
            !exited[0].bool("core_initiated"),
            "an external kill -9 must not be recorded as the core's doing"
        );

        // (d) No fd leak across the whole cycle: socketpair, memfds the
        // shim attached, the runtime-directory handles, the realm lock.
        // Attributably first -- nothing of this realm's own tree may still
        // be open, and no other test can affect that -- then the broader
        // process-global count.
        assert_eq!(
            fds_under(&base_dir),
            Vec::<String>::new(),
            "no descriptor into the dead realm's runtime tree may survive its teardown"
        );
        assert_eq!(
            open_fd_count(),
            baseline,
            "the whole spawn/paint/kill/reap/shutdown cycle must return every descriptor"
        );

        // And no zombie: the pid is gone entirely, not merely un-runnable.
        assert!(!is_zombie(pid), "the killed shim must not be left a zombie");
        assert!(!unreaped_children().contains(&pid));
    }

    // -- acceptance: the next capture refuses, and delivers NO frame -------

    #[test]
    fn the_capture_after_shim_death_refuses_no_surface_and_delivers_no_frame() {
        // The security property, through the REAL chokepoint on a REAL
        // dead shim. Asserted as "no frame_ready is delivered at all",
        // never as "the bytes differ": a stale frame that happened to
        // differ would pass a byte comparison and still be exactly the
        // defect this issue exists to prevent.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-no-surface");
        let mut life = rig.spawn(&["--serve", "--animate", "100000"]);
        wait_until_painted(&mut rig, &mut life);

        // Before: a capture on this live view really produces a frame.
        let view = rig.scene.compose(VIEW_W, VIEW_H);
        assert!(life.view_is_live(&rig.scene));
        let before = crate::principal::tests::capture_once(Some(crate::capture::RealmViewFrame {
            rgba: &view,
            width: VIEW_W,
            height: VIEW_H,
        }));
        assert!(
            matches!(
                before,
                crate::principal::tests::CaptureOutcome::Frame { .. }
            ),
            "the live realm must serve a frame, or the refusal below proves nothing: {before:?}"
        );

        // The shim dies mid-frame.
        assert_eq!(
            unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
            0
        );
        wait_for_death(&mut rig, &mut life);
        reap_within_deadline(&mut rig, &mut life);

        // After: the embedder derives `realm_view` from the ONE seam, and
        // it is now None -- so the pre-existing chokepoint refuses.
        assert!(!life.view_is_live(&rig.scene));
        let composed = rig.scene.compose(VIEW_W, VIEW_H);
        let after = crate::principal::tests::capture_once(life.view_is_live(&rig.scene).then_some(
            crate::capture::RealmViewFrame {
                rgba: &composed,
                width: VIEW_W,
                height: VIEW_H,
            },
        ));
        assert_eq!(
            after,
            crate::principal::tests::CaptureOutcome::Refused(Refusal::NoSurface),
            "a capture after shim death must refuse no_surface and deliver NO frame"
        );

        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    // -- acceptance: clean shutdown reaps shim and app ---------------------

    #[test]
    fn clean_shutdown_reaps_the_shim_and_its_app_leaving_no_zombie() {
        let _fd = fd_lock();
        let baseline = open_fd_count();
        let (shim_pid, app_pid) = {
            let mut rig = Rig::new("lifecycle-shutdown");
            let mut life = rig.spawn(&["--serve", "--spawn-app"]);
            let shim_pid = life.pid();
            let app_pid = wait_for("the shim to spawn its app", || {
                children_of(shim_pid).into_iter().next()
            });
            assert_eq!(ppid_of(app_pid), Some(shim_pid), "core -> shim -> app");
            let runtime_dir = life.runtime_dir().to_path_buf();
            assert!(runtime_dir.is_dir());

            // Rung 0: closing the core's end IS the shutdown signal for a
            // shim that serves until its core hangs up -- so a compliant
            // shim never sees a signal at all.
            let rung = life.shutdown(quick_timing(), &mut rig.teardown());
            assert_eq!(
                rung,
                ShutdownRung::Hangup,
                "a compliant shim must exit on its core's hangup, before any signal"
            );
            assert!(
                !life.core_initiated(),
                "the polite path must not have signalled anything"
            );
            assert_eq!(life.exit(), Some(ExitClass::Exited { code: 0 }));
            assert!(life.exit().unwrap().is_clean());
            assert!(life.is_reaped());

            // The orderly path removes the realm's runtime tree.
            assert!(
                !runtime_dir.exists(),
                "an orderly exit must remove the realm's runtime tree"
            );
            (shim_pid, app_pid)
        };

        // No zombies: neither pid is an uncollected child of this process,
        // and neither is in the Z state. The shim reaped its app (the
        // core -> shim -> app teardown chain); the core reaped the shim.
        assert!(
            !is_zombie(shim_pid),
            "the shim must be reaped, not a zombie"
        );
        assert!(!unreaped_children().contains(&shim_pid));
        wait_for("the app to be gone, and not a zombie", || {
            (proc_state(app_pid).is_none_or(|s| s != 'Z') && ppid_of(app_pid) != Some(shim_pid))
                .then_some(())
        });
        assert_eq!(
            open_fd_count(),
            baseline,
            "shutdown must leak no descriptor"
        );
    }

    // -- EOF vs SIGCHLD: both orderings, double arrival, idempotence -------

    #[test]
    fn eof_first_then_the_reap_is_one_death() {
        let _fd = fd_lock();
        let entries = {
            let mut rig = Rig::new("lifecycle-order-eof-first");
            let mut life = rig.spawn(&["--serve"]);

            // EOF first: wait for the connection to end, run the funnel,
            // and only then reap.
            assert_eq!(
                unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
                0
            );
            let cause = wait_for_death(&mut rig, &mut life);
            assert_eq!(cause, DeathCause::ConnectionClosed);
            assert_eq!(
                life.death_cause(),
                Some(DeathCause::ConnectionClosed),
                "the FIRST observation names the death"
            );

            reap_within_deadline(&mut rig, &mut life);
            assert_eq!(
                life.death_cause(),
                Some(DeathCause::ConnectionClosed),
                "the later reap must not rewrite the cause"
            );
            rig.entries()
        };
        assert_eq!(of_kind(&entries, "realm_died").len(), 1);
        assert_eq!(of_kind(&entries, "realm_exited").len(), 1);
        assert_eq!(
            of_kind(&entries, "realm_died")[0].str("cause"),
            "connection_closed"
        );
    }

    #[test]
    fn the_reap_first_then_eof_is_the_same_single_death() {
        // The other ordering. `poll_exit` sees the process die before
        // anything reads the socket, so the death is attributed to the
        // process -- and the EOF that follows changes nothing.
        let _fd = fd_lock();
        let entries = {
            let mut rig = Rig::new("lifecycle-order-reap-first");
            let mut life = rig.spawn(&["--serve"]);
            let pid = life.pid();
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);

            // Reap WITHOUT servicing the connection first: this is the
            // SIGCHLD-before-EOF ordering, and the death funnel has to run
            // from inside the reap path.
            reap_within_deadline(&mut rig, &mut life);
            assert_eq!(life.death_cause(), Some(DeathCause::ProcessExit));
            assert!(!life.is_alive(), "the reap alone must bury the realm");
            assert_eq!(
                rig.scene.surface_size(),
                None,
                "the reap path must remove the surface too, not only the EOF path"
            );

            // The EOF arrives afterwards (the connection is already
            // dropped by the funnel, so servicing it is a no-op) and a
            // redundant funnel call must change nothing.
            assert!(
                life.connection_mut().is_none(),
                "the death funnel closes the core's end of the identity socketpair"
            );
            assert!(
                !life.note_connection_closed(DeathCause::ConnectionClosed, &mut rig.teardown()),
                "a second death observation must report that it was not the death"
            );
            assert_eq!(life.death_cause(), Some(DeathCause::ProcessExit));
            rig.entries()
        };
        assert_eq!(
            of_kind(&entries, "realm_died").len(),
            1,
            "one death, one entry"
        );
        assert_eq!(of_kind(&entries, "realm_exited").len(), 1);
        assert_eq!(
            of_kind(&entries, "realm_died")[0].str("cause"),
            "process_exit"
        );
    }

    #[test]
    fn both_signals_and_repeated_calls_are_idempotent() {
        // The double-arrival case, hammered: EOF and the reap both arrive,
        // then both funnels are called again several times. Exactly one
        // death and exactly one reap survive.
        let _fd = fd_lock();
        let entries = {
            let mut rig = Rig::new("lifecycle-idempotent");
            let mut life = rig.spawn(&["--serve"]);
            assert_eq!(
                unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
                0
            );
            let first = wait_for_death(&mut rig, &mut life);
            reap_within_deadline(&mut rig, &mut life);

            for _ in 0..5 {
                assert!(
                    !life.note_connection_closed(DeathCause::ConnectionFault, &mut rig.teardown()),
                    "every repeat must be a no-op"
                );
                assert!(
                    !life.poll_exit(&mut rig.teardown()),
                    "a reaped child must never be waited on twice"
                );
            }
            assert_eq!(life.death_cause(), Some(first));
            assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 9 }));
            rig.entries()
        };
        assert_eq!(
            of_kind(&entries, "realm_died").len(),
            1,
            "a death is never double-counted, however many observations arrive"
        );
        assert_eq!(of_kind(&entries, "realm_exited").len(), 1);
    }

    // -- a shim that hangs up but keeps running ---------------------------

    #[test]
    fn a_shim_that_closes_its_core_fd_but_keeps_running_loses_its_surface_at_once() {
        // The EOF-without-SIGCHLD cell of the module's table. This shim is
        // perfectly alive; it simply can never paint again, so the realm
        // is over *now* -- waiting for a process death would serve a stale
        // frame for as long as it felt like living.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-hangup-alive");
        let mut life = rig.spawn_program(
            "/bin/sh",
            // Close fd 3, then become a single long-lived process (`exec`,
            // so there is no grandchild to orphan).
            &["-c", "exec 3<&- 2>/dev/null; exec sleep 300"],
        );
        let pid = life.pid();

        let cause = wait_for_death(&mut rig, &mut life);
        assert_eq!(cause, DeathCause::ConnectionClosed);
        assert!(!life.is_alive(), "the realm is over the moment it hangs up");
        assert!(!life.view_is_live(&rig.scene));

        // ...and the process really is still alive at the moment of death:
        // this is EOF WITHOUT an exit, which is the whole point.
        assert!(
            !life.is_reaped(),
            "the shim is still running: this case has no exit status yet"
        );
        assert_eq!(life.exit(), None);

        // It was asked to go (non-blocking, inside the loop). It obeys,
        // because the spawn path cleared the inherited SIGTERM mask.
        reap_within_deadline(&mut rig, &mut life);
        assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 15 }));
        assert!(
            life.core_initiated(),
            "the core sent that SIGTERM and the log must say so"
        );
        assert!(!is_zombie(pid));

        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    // -- attribution: whose signal actually ended the shim ------------------

    #[test]
    fn a_core_sigterm_followed_by_an_external_kill_is_not_recorded_as_the_cores_doing() {
        // The deterministic half of the attribution bug a per-realm latch
        // has. This shim hangs up but keeps running, so the core really
        // does send it a SIGTERM -- and it really does ignore it. An
        // operator then `kill -9`s it. The core sent A signal; it did not
        // send THE signal that ended this process, and `core_initiated` is
        // documented as answering the second question.
        //
        // Without per-signal attribution the log reads
        // `signal: 9, core_initiated: true` -- the core claiming a kill it
        // did not make, in the run's only record of an operator's kill.
        let _fd = fd_lock();
        let (pid, entries) = {
            let mut rig = Rig::new("lifecycle-attribution");
            let mut life = rig.spawn_program(
                "/bin/sh",
                // Close fd 3, ignore SIGTERM, then live on.
                &[
                    "-c",
                    r#"exec 3<&- 2>/dev/null; trap "" TERM; while :; do sleep 0.05; done"#,
                ],
            );
            let pid = life.pid();

            // The hangup: the core observes EOF and asks it to leave.
            assert_eq!(
                wait_for_death(&mut rig, &mut life),
                DeathCause::ConnectionClosed
            );
            assert!(!life.is_reaped(), "it ignores SIGTERM, so it is still here");

            // The core's SIGTERM really was delivered -- otherwise this
            // test would prove nothing about attribution.
            assert_eq!(
                life.core_signals,
                vec![libc::SIGTERM],
                "the core must really have sent SIGTERM for the misattribution to be possible"
            );

            // Somebody else ends it.
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);
            reap_within_deadline(&mut rig, &mut life);

            assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 9 }));
            assert!(
                !life.core_initiated(),
                "the core sent SIGTERM, not the SIGKILL that ended it: attribution is \
                 per-signal, not 'the core signalled at some point'"
            );
            (pid, rig.entries())
        };

        let exited = of_kind(&entries, "realm_exited");
        assert_eq!(exited.len(), 1);
        assert_eq!(exited[0].u64("signal"), 9);
        assert!(
            !exited[0].bool("core_initiated"),
            "the journal must not attribute an external kill -9 to the core merely because \
             the core had asked the same process to exit earlier"
        );
        assert!(!is_zombie(pid));
        assert!(!unreaped_children().contains(&pid));
    }

    #[test]
    fn the_core_sigterm_that_does_end_a_shim_is_still_attributed_to_the_core() {
        // The other side of the same razor: narrowing attribution must not
        // narrow it into uselessness. Here the core's SIGTERM IS the signal
        // that ends the shim, so the log must say so -- this is what keeps
        // the field able to distinguish the ladder's last rung from an
        // operator's kill, which is its whole reason to exist.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-attribution-true");
        let mut life =
            rig.spawn_program("/bin/sh", &["-c", "exec 3<&- 2>/dev/null; exec sleep 300"]);
        let pid = life.pid();

        wait_for_death(&mut rig, &mut life);
        reap_within_deadline(&mut rig, &mut life);

        assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 15 }));
        assert!(
            life.core_initiated(),
            "the core sent the SIGTERM that ended this shim"
        );
        assert!(!is_zombie(pid));
        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    #[test]
    fn a_signal_the_kernel_refused_is_never_claimed_as_the_cores() {
        // `core_signals` is evidence, so it records deliveries rather than
        // intentions: an out-of-range or otherwise refused `kill(2)` must
        // leave the set empty. Signal 0 is the probe signal -- it is
        // delivered to nothing -- so `kill` succeeds and it is recorded;
        // an invalid signal number is refused with EINVAL and must not be.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-refused-signal");
        let mut life = rig.spawn(&["--serve"]);

        life.signal(libc::SIGCHLD.wrapping_add(4096));
        assert!(
            life.core_signals.is_empty(),
            "a kill(2) the kernel refused was never delivered and must not be claimed"
        );

        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    // -- a shim that ignores SIGTERM --------------------------------------

    #[test]
    fn a_shim_that_ignores_sigterm_is_sigkilled_after_the_grace_period() {
        // A shim MAY legitimately install its own SIGTERM handler, so the
        // ladder cannot assume rung 2 works. `trap "" TERM` is SIG_IGN,
        // installed by the child itself after execve -- so this is a
        // genuinely unkillable-by-SIGTERM peer, not a simulated one.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-ignores-term");
        let mut life = rig.spawn_program(
            "/bin/sh",
            &["-c", r#"trap "" TERM; while :; do sleep 0.05; done"#],
        );
        let pid = life.pid();

        let started = Instant::now();
        let rung = life.shutdown(quick_timing(), &mut rig.teardown());
        let elapsed = started.elapsed();

        assert_eq!(
            rung,
            ShutdownRung::Kill,
            "a shim that ignores SIGTERM must be SIGKILLed"
        );
        assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 9 }));
        assert!(
            life.core_initiated(),
            "the core sent this SIGKILL; the log must distinguish it from an external kill -9"
        );
        // The ladder really waited before escalating -- it did not skip
        // straight to SIGKILL, which would make the polite rung a lie.
        assert!(
            elapsed >= quick_timing().hangup_grace + quick_timing().term_grace,
            "the ladder must spend both grace periods before SIGKILL, spent {elapsed:?}"
        );
        assert!(life.is_reaped());
        assert!(!is_zombie(pid), "even the SIGKILL rung leaves no zombie");
    }

    // -- the runtime tree: kept on crash, removed at orderly exit ----------

    #[test]
    fn the_runtime_tree_survives_a_crash_and_is_removed_at_the_cores_exit() {
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-runtime-tree");
        let mut life = rig.spawn(&["--serve"]);
        let runtime_dir = life.runtime_dir().to_path_buf();
        assert!(runtime_dir.is_dir(), "a live realm has its runtime tree");

        assert_eq!(
            unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
            0
        );
        wait_for_death(&mut rig, &mut life);
        reap_within_deadline(&mut rig, &mut life);

        // Decision (module docs): a crash KEEPS the tree. The session is
        // still running, so the directory is evidence -- and an orphaned
        // app may still hold that socket path open.
        assert!(
            runtime_dir.is_dir(),
            "a crash must leave the realm's runtime tree in place as evidence"
        );

        // The core's own exit removes it.
        life.shutdown(quick_timing(), &mut rig.teardown());
        assert!(
            !runtime_dir.exists(),
            "an orderly exit must remove the realm's runtime tree"
        );
    }

    // -- SIGCHLD through a real calloop loop -------------------------------

    #[test]
    fn sigchld_reaches_the_reaper_through_a_real_calloop_loop() {
        // The reaping path as it will actually run: calloop's signalfd
        // source, inserted into a real event loop, whose callback body is
        // the real `poll_exit` -- a real `waitpid` collecting a really
        // dead child. No signal handler exists anywhere in this path.
        //
        // # Why the wakeup is delivered thread-directed, and what that
        // # does and does not weaken
        //
        // `signalfd` only ever sees a signal blocked on EVERY thread
        // (`child_signal_source`'s docs). libtest spawns a thread per test
        // *regardless* of `--test-threads` -- measured: a re-exec'd
        // `--test-threads=1` inner run times out identically -- and the
        // harness's main thread does not block SIGCHLD. The kernel
        // therefore discards a real child's SIGCHLD at generation time
        // (its default action is "ignore" and not every thread blocks it),
        // so no test harness can observe one arriving at this source.
        //
        // That is the production hazard the module documents rather than a
        // shortcut around it: in production the source is built on the main
        // thread before the backend spawns any, and every later thread
        // inherits the mask. So this test delivers the *wakeup*
        // thread-directed -- `tgkill` at the very thread whose mask calloop
        // just installed -- and keeps everything else real: the source, the
        // loop, the dispatch, the callback, and a genuinely dead child that
        // a genuine `waitpid` collects. The single synthetic step is which
        // thread the signal was aimed at.
        //
        // And it is the step that matters least, by design: the module's
        // rule is that SIGCHLD is a *hint* and `waitpid` is the authority,
        // so a missed signal costs a delayed reap and never a zombie --
        // the EOF path and the shutdown ladder both reap unconditionally,
        // and those are tested against real child deaths throughout this
        // file.
        let _fd = fd_lock();

        /// The loop data: exactly what the M1.1 callback will close
        /// over. Owns its realm rather than borrowing it, because
        /// `EventLoop<'_, Data>` keeps `Data`'s type -- and so any lifetime
        /// in it -- alive for the loop's whole scope.
        struct Reaper {
            rig: Rig,
            life: RealmLifecycle,
            reaped: bool,
        }

        let mut event_loop: calloop::EventLoop<'static, Reaper> =
            calloop::EventLoop::try_new().expect("calloop event loop");
        let source = child_signal_source().expect("a SIGCHLD signalfd source");
        event_loop
            .handle()
            .insert_source(source, |event, _, state: &mut Reaper| {
                assert_eq!(event.signal(), Signal::SIGCHLD);
                // The callback's real body: waitpid every realm the core
                // owns. The signal says only that *some* child changed
                // state, so the authority is this call, not the signal.
                state.reaped |= state.life.poll_exit(&mut state.rig.teardown());
            })
            .expect("insert the SIGCHLD source");

        let mut rig = Rig::new("lifecycle-sigchld");
        let life = rig.spawn(&["--serve"]);
        let pid = life.pid();
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);

        // The child must really be dead before the wakeup, or the
        // callback's `waitpid` would correctly find nothing to collect.
        // An unreaped dead child is a zombie, which is exactly the state
        // this whole mechanism exists to not leave behind.
        wait_for("the killed shim to become an unreaped zombie", || {
            is_zombie(pid).then_some(())
        });

        // The wakeup, aimed at this thread (see the header).
        // SAFETY: `tgkill(2)` takes three integers, touches no memory, and
        // cannot alias anything. The target is this very process and this
        // very thread -- the one whose signal mask calloop's source just
        // installed -- so the signal is thread-directed and lands in this
        // thread's pending set, where its signalfd reads it.
        let rc = unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::pid_t;
            libc::syscall(
                libc::SYS_tgkill,
                std::process::id() as libc::pid_t,
                tid,
                libc::SIGCHLD,
            )
        };
        assert_eq!(rc, 0, "tgkill must deliver SIGCHLD to this thread");

        let deadline = Instant::now() + DEADLINE;
        let mut reaper = Reaper {
            rig,
            life,
            reaped: false,
        };
        while !reaper.reaped {
            event_loop
                .dispatch(Some(Duration::from_millis(50)), &mut reaper)
                .expect("dispatch");
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the SIGCHLD source to reap the shim"
            );
        }
        let Reaper {
            mut rig, mut life, ..
        } = reaper;

        // The reap really happened, through the loop, and it buried the
        // realm on its own -- nothing read the socketpair in this test.
        assert!(life.is_reaped());
        assert_eq!(life.exit(), Some(ExitClass::Signaled { signal: 9 }));
        assert!(!life.is_alive(), "the reap alone buries the realm");
        assert!(!life.view_is_live(&rig.scene));
        assert!(!is_zombie(pid), "the zombie above must have been collected");
        assert!(!unreaped_children().contains(&pid));

        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    // -- unit-level: classification, labels, the documented constants ------

    #[test]
    fn exit_status_is_classified_into_the_logs_closed_vocabulary() {
        use std::os::unix::process::ExitStatusExt;
        let exited = ExitClass::of(ExitStatus::from_raw(0));
        assert_eq!(exited, ExitClass::Exited { code: 0 });
        assert_eq!(exited.disposition(), "exited");
        assert_eq!(exited.code(), Some(0));
        assert_eq!(exited.signal(), None);
        assert!(exited.is_clean());

        // wait(2) encoding: low byte is the terminating signal.
        let killed = ExitClass::of(ExitStatus::from_raw(9));
        assert_eq!(killed, ExitClass::Signaled { signal: 9 });
        assert_eq!(killed.disposition(), "signaled");
        assert_eq!(killed.code(), None);
        assert_eq!(killed.signal(), Some(9));
        assert!(!killed.is_clean());

        // A nonzero normal exit is still an exit, and still not clean.
        let failed = ExitClass::of(ExitStatus::from_raw(3 << 8));
        assert_eq!(failed, ExitClass::Exited { code: 3 });
        assert!(!failed.is_clean());
    }

    #[test]
    fn death_causes_have_stable_log_labels() {
        // A reader switches on these strings; they are API.
        for (cause, label) in [
            (DeathCause::ConnectionClosed, "connection_closed"),
            (DeathCause::ConnectionFault, "connection_fault"),
            (DeathCause::ProcessExit, "process_exit"),
            (DeathCause::Shutdown, "shutdown"),
        ] {
            assert_eq!(cause.label(), label);
        }
    }

    #[test]
    fn the_documented_grace_periods_are_the_defaults() {
        // The ladder tests deliberately run on a short timing, so this is
        // what keeps the production values honest against the module docs.
        let timing = ShutdownTiming::default();
        assert_eq!(timing.hangup_grace, Duration::from_millis(250));
        assert_eq!(timing.term_grace, Duration::from_secs(2));
    }

    #[test]
    fn a_dead_realm_has_no_view_whatever_the_scene_holds() {
        // The defense-in-depth conjunct, isolated: even handed a scene
        // with a committed surface, a dead realm reports no live view.
        // Remove the `death.is_none()` arm and this fails -- which is its
        // whole job.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-view-predicate");
        let mut life = rig.spawn(&["--serve"]);

        let mut scene = Scene::new();
        scene.commit(
            crate::scene::SurfaceContent::from_rgba(
                crate::scene::tests::client_pixels(VIEW_W, VIEW_H),
                VIEW_W,
                VIEW_H,
            )
            .expect("well-formed content"),
        );
        assert!(
            life.view_is_live(&scene),
            "a live realm with a committed surface has a live view"
        );

        life.note_connection_closed(DeathCause::ConnectionClosed, &mut rig.teardown());
        assert!(
            !life.view_is_live(&scene),
            "a DEAD realm has no view even if some scene still holds a surface"
        );
        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    // -- the retained frame: scrubbed with the realm ----------------------

    #[test]
    fn the_death_funnel_scrubs_the_retained_frame_exactly_once() {
        // Clearing the scene is not enough on its own. `Scene::compose` is
        // pure, so the scene self-heals; the backend's framebuffer is a
        // cache of a past composition and keeps the dead realm's last
        // painted frame until something composites over it. The funnel
        // drives that, through the bundle, so an embedder cannot hold a
        // dead realm's pixels by forgetting a step.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-scrub");
        let mut life = rig.spawn(&["--serve", "--animate", "100000"]);
        wait_until_painted(&mut rig, &mut life);
        assert_eq!(
            rig.retained.scrubs, 0,
            "a live realm's retained frame is its own"
        );

        assert_eq!(
            unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
            0
        );
        wait_for_death(&mut rig, &mut life);
        assert_eq!(
            rig.retained.scrubs, 1,
            "the death funnel must scrub the retained frame"
        );

        // Latched with everything else the death does: repeated
        // observations and the shutdown ladder must not scrub again.
        reap_within_deadline(&mut rig, &mut life);
        for _ in 0..3 {
            life.note_connection_closed(DeathCause::ConnectionFault, &mut rig.teardown());
            life.poll_exit(&mut rig.teardown());
        }
        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
        assert_eq!(
            rig.retained.scrubs, 1,
            "the scrub rides the death latch: one death, one scrub"
        );
    }

    #[test]
    fn a_realm_that_dies_before_bring_up_still_scrubs_the_retained_frame() {
        // The no-`ShimServer` arm of the funnel. A realm that died before
        // its session came up has no server to run `connection_closed`
        // through, and that arm is exactly where a second, hand-rolled
        // teardown path forgets a step.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-scrub-early");
        let mut life = rig.spawn_program("/bin/sh", &["-c", "exec sleep 300"]);
        assert!(rig.shim.is_none(), "no session came up in this rig");

        life.note_connection_closed(DeathCause::ConnectionClosed, &mut rig.teardown());
        assert_eq!(
            rig.retained.scrubs, 1,
            "the server-less arm must scrub too: a realm with no shim session can still \
             have had something composited for it"
        );
        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    #[test]
    fn a_failing_scrub_does_not_stop_the_realm_from_dying() {
        // Fail-closed check on the defense-in-depth layer: the scrub is
        // best effort and loud, and a backend that cannot recomposite must
        // not keep a dead realm alive, abort the session, or -- above all
        // -- leave `view_is_live` true.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-scrub-fails");
        let mut life = rig.spawn(&["--serve", "--animate", "100000"]);
        wait_until_painted(&mut rig, &mut life);
        rig.retained.fails = true;

        assert_eq!(
            unsafe { libc::kill(life.pid() as libc::pid_t, libc::SIGKILL) },
            0
        );
        wait_for_death(&mut rig, &mut life);
        reap_within_deadline(&mut rig, &mut life);

        assert_eq!(rig.retained.scrubs, 1, "it was attempted");
        assert!(!life.is_alive(), "the realm still died");
        assert!(
            !life.view_is_live(&rig.scene),
            "the primary refusal must not depend on the backend's scrub succeeding"
        );
        assert_eq!(rig.scene.surface_size(), None);
        let _ = life.shutdown(quick_timing(), &mut rig.teardown());
    }

    #[test]
    fn a_realm_dropped_without_shutdown_still_leaves_no_zombie() {
        // The Drop safety net: an embedder bug or a panic unwinding past
        // shutdown must not accumulate processes. Not a policy path --
        // `shutdown` is -- but a zombie is worse than a rude kill.
        let _fd = fd_lock();
        let mut rig = Rig::new("lifecycle-drop-net");
        let life = rig.spawn(&["--serve"]);
        let pid = life.pid();
        assert!(proc_state(pid).is_some(), "the shim is running");

        drop(life);
        assert!(!is_zombie(pid), "Drop must reap, not merely kill");
        assert!(!unreaped_children().contains(&pid));
    }
}
