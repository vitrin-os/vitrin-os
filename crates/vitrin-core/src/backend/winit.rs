//! Nested backend: the core runs as a client of the host compositor
//! (GNOME, Hyprland, …), presenting exactly one host window — the gamescope
//! nested-session pattern (PRD Doc 2 §4/§17). Rendering stays deliberately
//! trivial per plan risk R1: one window, one full-window texture blit of the
//! composed human-visible output ([`super::compose_human_visible`] — the
//! realm view of [`Scene::compose`], the same bytes the headless backend
//! retains for capture, P1.3.3, with the consent overlay on top, P1.7.1).
//!
//! The host window IS the human's display here, so it presents the
//! human-visible side of the output stage and there is no second window and
//! no second surface (decision D4). The consent overlay reaching this GL
//! texture is therefore exactly as far as it goes: **no capture path reads a
//! GL texture.** A capture served under `--nested` (P1.3.8, issue #116) is the
//! *bare realm view* composed on the CPU straight from the retained scene
//! ([`NestedView::view_rgba`] → [`Scene::compose`]) — the same bytes headless
//! retains for capture, and, because `Scene::compose` sits upstream of the
//! output-stage fork ([`super::human_visible_from_view`]), overlay-free by
//! construction. So the fork holds here exactly as it does in headless: the
//! human sees the GL window with its overlay; the agent sees the composed
//! realm view without it (see [`super`] and [`crate::consent`] for the fork).
//!
//! # This backend is where the human physically *is*
//!
//! Two consequences, both P1.7.3's ([`crate::deadman`]). Nested mode is the
//! only place a physical input device exists — `intake_physical` has exactly
//! one caller and it is here — so this is the only backend that can watch a
//! dead-man chord, and the only one that ever paints the hold indicator.
//! Headless is not missing a feature; it has no human at a keyboard to serve,
//! structurally rather than by omission.
//!
//! And because a held key produces **no** further events (Smithay 0.7.0's
//! winit backend filters autorepeat), this backend owes the switch a clock:
//! it arms a `calloop` timer at the hold's deadline and re-checks on every
//! input turn and every frame. See [`NestedState::deadman_tick`].
//!
//! The winit backend is EGL/GLES-bound by construction, so this path always
//! renders with [`GlesRenderer`]. The pixman software path (mandatory for
//! GPU-less CI) arrives with the headless backend in P1.3.2, where no host
//! GL surface exists.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle, LoopSignal};
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::context::GlAttributes;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Color32F, Frame, ImportMem, Renderer};
use smithay::backend::winit::{
    self as winit_backend, WinitEvent, WinitGraphicsBackend, WinitInput,
};
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};
use tracing::{debug, error, info, trace};

use crate::consent::grab::{ConsentGate, ConsentGrab};
use crate::consent::{ConsentSurface, TrustedIndicator};
use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch, Trigger};
use crate::input;
use crate::recorder::Recorder;
use crate::scene::Scene;
use crate::session::{self, Runtime, RuntimeSeed};

/// Initial logical window size; matches the planned headless default
/// (`--headless --size 1280x800`, P1.3.2) so nested and headless views of
/// the same content agree by default.
const INITIAL_SIZE: (f64, f64) = (1280.0, 800.0);

/// Background behind the composed view; only visible if the blit fails.
/// Deliberately near [`crate::scene::LETTERBOX_RGBA`] so nothing here reads
/// as client content.
const CLEAR_COLOR: Color32F = Color32F::new(0.06, 0.06, 0.08, 1.0);

/// Fallback frame budget (~60 Hz). Vsync'd blocking swaps are the primary
/// pacing mechanism, but Smithay's `vsync: true` only filters EGL config
/// selection — it never calls `eglSwapInterval` — so on hosts whose EGL
/// stack returns from swap immediately (Mesa software EGL under Xvfb,
/// GPU-less VMs on llvmpipe, X servers without vblank) the redraw chain
/// would otherwise spin unthrottled. Frames that complete faster than this
/// budget defer the next redraw to a timer instead. A real frame clock
/// (client damage + frame callbacks) replaces this in P1.3.4.
const FRAME_BUDGET: Duration = Duration::from_micros(16_667);

/// What the uploaded window texture was composed *for*: re-upload exactly
/// when one of these changes.
///
/// A named type rather than an inline tuple because it carries a correctness
/// property worth testing on its own — a key that failed to include the
/// consent generation would leave a prompt off the screen (or a decided
/// prompt on it) until the scene happened to change next, and GL presentation
/// cannot be driven on a CI runner with no display. See
/// `the_texture_key_changes_on_every_visible_transition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureKey {
    size: Size<i32, Physical>,
    /// [`Scene::generation`] at upload time.
    scene_generation: u64,
    /// [`ConsentSurface::generation`] at upload time. A separate counter
    /// rather than a merged one because the two changes have different
    /// consumers — see [`crate::consent`]'s redraw section.
    consent_generation: u64,
    /// The dead-man hold indicator's fill, quantized to
    /// [`HOLD_STEPS`] buckets, or `None` when no indicator is shown
    /// (P1.7.3).
    ///
    /// Quantized rather than carried as a float for two reasons that point
    /// the same way: an `f64` in a `PartialEq` cache key is a bug waiting
    /// for a rounding change, and bucketing bounds the re-uploads a hold
    /// costs to [`HOLD_STEPS`] over its whole duration instead of one per
    /// frame. The visible result is identical at this bar's size.
    hold_bucket: Option<u8>,
}

/// How many distinct fill levels the hold indicator has. Twenty steps across
/// a ~1 s hold is finer than a human can perceive as stepping, and caps the
/// texture re-uploads one hold can cost.
const HOLD_STEPS: f64 = 20.0;

impl TextureKey {
    /// The key describing what a texture composed *right now* would contain.
    fn current(
        size: Size<i32, Physical>,
        scene: &Scene,
        consent: &ConsentSurface,
        hold: Option<f64>,
    ) -> Self {
        Self {
            size,
            scene_generation: scene.generation(),
            consent_generation: consent.generation(),
            hold_bucket: hold.map(|p| (p.clamp(0.0, 1.0) * HOLD_STEPS) as u8),
        }
    }
}

/// The composed human-visible output ([`window_pixels`]) uploaded as a GLES
/// texture, remembered together with the key it was composed for, so resizes,
/// scene commits, and consent-prompt transitions all re-upload it.
struct SceneTexture {
    texture: GlesTexture,
    key: TextureKey,
}

/// The pixels this backend uploads as its window texture: the shared
/// human-visible composition ([`super::compose_human_visible`]) — realm view
/// plus the consent prompt, if one is up — and, above everything, the
/// dead-man hold indicator (P1.7.3) while the human is mid-gesture.
///
/// Split out of [`NestedState::try_redraw`] so it can be tested without a
/// display. Presenting those pixels needs an EGL/GLES context and a host
/// window, so CI cannot drive `try_redraw` end to end; what it *can* pin is
/// the two decisions that function makes — which pixels to upload (here) and
/// when to re-upload them ([`TextureKey`]) — leaving only the GL submit
/// itself uncovered. Before this split nothing constrained nested-mode
/// presentation at all: deleting the overlay from the upload left the whole
/// suite green.
///
/// **The hold indicator is applied here rather than in
/// [`super::human_visible_from_view`]**, which stays the one shared
/// consent-overlay step both backends reach. That is not a drift in "both
/// backends present the same output": the indicator reflects *physical input
/// state*, and headless has no physical input device at all — structurally,
/// since `SeatInput::physical` can only come from the intake this backend
/// alone calls ([`crate::input`]). There is no hold for headless to draw. It
/// inherits P1.7.1's fork either way: this is downstream of the point where
/// capture takes the bare realm view, so the indicator can no more reach
/// `vitrin_view.frame_ready` than the consent card can.
fn window_pixels(
    scene: &Scene,
    consent: &mut ConsentSurface,
    hold: Option<f64>,
    size: Size<i32, Physical>,
) -> Vec<u8> {
    let (w, h) = (size.w.max(0) as u32, size.h.max(0) as u32);
    let mut pixels = super::compose_human_visible(scene, consent, w, h);
    if let Some(progress) = hold {
        // Last, so a consent card can never hide the fact that the human is
        // mid-gesture on the off-switch.
        crate::deadman::composite_hold_indicator(&mut pixels, w, h, progress);
    }
    pixels
}

/// The pixels this backend serves as an **agent capture** (P1.3.8, issue
/// #116): the **bare realm view**, composed on the CPU straight from the
/// scene — and deliberately nothing else.
///
/// This is the capture counterpart of [`window_pixels`], and the contrast
/// between the two is the whole of the P1.7.1 fork made local to this backend:
///
/// - [`window_pixels`] is what the human sees — [`Scene::compose`] **plus** the
///   consent overlay (P1.7.1) **plus** the dead-man hold indicator (P1.7.3).
///   It is uploaded to the GL window.
/// - `capture_pixels` is what an agent observes — [`Scene::compose`] and
///   nothing over it. No consent card, no hold indicator, ever: those join
///   only in [`super::human_visible_from_view`] and [`window_pixels`], both
///   *downstream* of [`Scene::compose`], so the overlay is excluded here by
///   construction rather than by a check. This is the identical structural
///   argument the headless capture rests on, and it is why the two backends'
///   captures cannot drift: both are exactly [`Scene::compose`].
///
/// Split out of [`NestedView::view_rgba`] for the same reason
/// [`window_pixels`] was split out of [`NestedState::try_redraw`]: so the
/// decision it encodes — *serve the bare scene, never the presented window* —
/// is pinned by a test that needs no display, GL context, or host window. A
/// regression that reached for [`window_pixels`] here instead (leaking the
/// consent card into every capture) would otherwise pass the whole suite.
///
/// `None` for a degenerate (zero-sized, e.g. minimized) window: there is no
/// realm view, so the capture meets the chokepoint's `no_surface` refusal.
pub(crate) fn capture_pixels(scene: &Scene, size: Size<i32, Physical>) -> Option<Vec<u8>> {
    let (w, h) = (size.w.max(0) as u32, size.h.max(0) as u32);
    if w == 0 || h == 0 {
        return None;
    }
    // The bare realm view only — NOT `window_pixels`. Byte-for-byte what the
    // headless backend retains and reads back for the same scene.
    Some(scene.compose(w, h))
}

/// The nested backend's **presentation half**: the winit window + GLES
/// renderer pair, the realm's [`Scene`], the consent surface, and the
/// currently uploaded view texture.
///
/// Split out of [`NestedState`] for [`session::RuntimeHost::split`]: building
/// a `ServerCtx` borrows the capability kernel mutably while the realm view
/// borrows presentation state, so the two must be provably disjoint fields
/// rather than one flat struct (see [`crate::session`]).
pub(crate) struct NestedView {
    backend: WinitGraphicsBackend<GlesRenderer>,
    /// The realm's scene (P1.3.3) — the same composition implementation the
    /// headless backend retains for capture; this backend presents its
    /// output 1:1 in the host window. The shim-facing protocol server
    /// (P1.3.4) commits into it and requests a redraw.
    scene: Scene,
    /// The consent surface (P1.7.1): the prompt composited above the realm
    /// view in the host window. Driven now (issue #90): once per dispatch
    /// round [`session::RuntimeHost::service_consent`] calls
    /// [`session::service_consent_round`], which raises the front pending
    /// petition's card here and lowers it again when the petition is decided
    /// or leaves the table. It is empty only while no petition is pending.
    consent: ConsentSurface,
    texture: Option<SceneTexture>,
}

/// Per-run state of the nested backend: its presentation half, the session
/// runtime, and the dead-man / consent wiring the host window feeds.
pub(crate) struct NestedState {
    view: NestedView,
    /// The session runtime ([`crate::session`]): the core socket's listener,
    /// every accepted principal connection, the capability kernel, and the
    /// realm's shim session. It carries the router below rather than owning a
    /// second one, so an agent's chokepoint-admitted actuations and a human's
    /// physical input share one implicit-grab and pointer state — which is
    /// what makes the preemption hook mean anything.
    runtime: Runtime<ConsentGate<DeadManHook<input::NoopHook>>>,
    /// The consent input grab (P1.7.2), shared with the router's gate:
    /// while a prompt is up it owns physical input, and a click on one of
    /// the card's buttons becomes a petition decision here.
    ///
    /// Live and driven (issue #90). Every dispatch round
    /// [`session::RuntimeHost::service_consent`] borrows this grab and runs
    /// [`session::service_consent_round`]: it raises the front pending
    /// petition's prompt (seizing physical input through the shared
    /// [`ConsentGate`]), drains the decisions a physical click produced, and
    /// lowers a card whose petition has left the table. Shared with the router
    /// rather than attached later, because the alternative is a window in which
    /// a prompt could be shown with no grab behind it.
    grab: Rc<RefCell<ConsentGrab>>,
    /// The dispatch turn's instant, shared with the router's
    /// [`ConsentGate`]. The hook trait carries no clock by design, so the
    /// embedder that drives `route` advances this cell first — the same
    /// arrangement `input::PresenceHook` uses. The grab reads it for its
    /// guard interval (a press must not decide before the human could have
    /// read the card) and its deadline backstop (a grab must not outlive
    /// its petition).
    now: Rc<Cell<Instant>>,
    /// The dead-man switch (P1.7.3), shared with the router's innermost
    /// hook: the chord's hold state, the completed-chord trigger, and the
    /// hold indicator's progress.
    ///
    /// Live and whole — the chord is watched, the timer is armed, a
    /// completed hold fires, and the trigger now reaches
    /// [`crate::deadman::apply`] against this session's real grant table and
    /// petition registry (see [`DeadManHost::on_trigger`]), rather than
    /// being logged and dropped.
    deadman: Rc<RefCell<DeadManSwitch>>,
    /// Whether a one-shot timer is outstanding for the armed hold, so an
    /// arming keypress does not insert a fresh calloop source per event.
    deadman_timer_armed: bool,
    loop_signal: LoopSignal,
    loop_handle: LoopHandle<'static, NestedState>,
    /// Set when a render failure stops the loop, so [`run`] can propagate
    /// it as an error (and `main` as a non-zero exit) instead of masking a
    /// mid-run fatal as a clean shutdown.
    fatal: Option<Box<dyn Error>>,
}

/// Run the nested compositor loop until the host window is closed or a
/// SIGINT/SIGTERM arrives.
///
/// `dead_man` is the session's off-switch policy, already validated by
/// argument parsing (an unusable chord is a startup error, never a switch
/// that quietly cannot fire).
///
/// # What the M1.1 runtime wiring added
///
/// The loop now also carries the core socket's listener, every accepted
/// principal connection, the realm's shim socketpair, and the expiry sweep
/// ([`crate::session`]). The recorder travels through here rather than
/// staying in `run_session` because calloop fixes one state type per loop and
/// the whole capability kernel — the recorder with it — has to live in that
/// state; it is handed straight back so the run's footer is still written by
/// the code that opened the log.
///
/// **Nested mode serves captures too (P1.3.8, issue #116).** This backend is
/// EGL/GLES-bound and retains no readable image of the *presented* window, but
/// it does not need one: [`session::Presenter::view_rgba`] composes the bare
/// realm view on the CPU from the retained scene — byte-for-byte what headless
/// reads back, and structurally overlay-free (see that method). So
/// `vitrin_view.frame_ready` is answered here, and — because the same
/// `no_surface` judgement gates actuation — an agent can observe *and* actuate
/// under `--nested`, not only under `--headless`. Everything else — petitions,
/// consent, the physical dead-man switch — was already identical to headless.
pub fn run(dead_man: DeadManConfig, seed: RuntimeSeed) -> (Recorder, Result<(), Box<dyn Error>>) {
    // The seed is consumed the moment the state is built; until then it is
    // still ours, and either way the recorder must come back so `run_session`
    // can write the footer it owes. Threading it through two slots keeps `?`
    // usable for every startup step below instead of turning each into a
    // four-line `match`.
    let mut seed = Some(seed);
    let mut recovered = None;
    let result = run_inner(dead_man, &mut seed, &mut recovered);
    let recorder = recovered
        .or_else(|| seed.take().map(|seed| seed.recorder))
        .expect("the seed is either still unconsumed or its recorder was recovered");
    (recorder, result)
}

fn run_inner(
    dead_man: DeadManConfig,
    seed: &mut Option<RuntimeSeed>,
    recovered: &mut Option<Recorder>,
) -> Result<(), Box<dyn Error>> {
    let mut event_loop: EventLoop<'static, NestedState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Signal sources first, and this is the backend the ordering was written
    // for: the process is still single-threaded here, but Smithay's winit/EGL
    // stack below really does spawn threads, and `signalfd` only sees a
    // signal that is blocked on **every** thread. Install either of these
    // after `init_from_attributes_with_gl_attr` and the mask misses those
    // threads — for SIGCHLD that means a realm's exit is never noticed, with
    // nothing logged to say so.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.loop_signal.stop();
    })?;
    // SIGCHLD: the realm's shim exiting. A hint only — it says some child
    // changed state, never which — so the handler asks `waitpid`.
    loop_handle.insert_source(
        crate::lifecycle::child_signal_source()?,
        |_event, _, state: &mut NestedState| session::reap_realm(state),
    )?;

    // vsync on (Smithay's default is off): each frame chains the next via
    // `request_redraw`, and the blocking swap paces that chain to the
    // host's refresh rate. Because drivers are free to ignore the default
    // swap interval, [`FRAME_BUDGET`] additionally caps the chain when the
    // swap does not block (see its doc comment).
    let (backend, winit_source) = winit_backend::init_from_attributes_with_gl_attr::<GlesRenderer>(
        WinitWindow::default_attributes()
            .with_inner_size(LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1))
            .with_title("vitrind (nested)"),
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: true,
        },
    )?;
    info!(size = ?backend.window_size(), "nested backend initialized");

    loop_handle.insert_source(winit_source, |event, _, state| match event {
        WinitEvent::Redraw => state.redraw(),
        WinitEvent::Resized { size, .. } => {
            debug!(?size, "host window resized");
            // Drop the uploaded view; the next redraw recomposes it 1:1 at
            // the new size (kept pixel-exact for the P1.3.2/P1.3.6 goldens).
            state.view.texture = None;
            state.view.backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => {
            info!("host window close requested");
            state.loop_signal.stop();
        }
        // P1.3.7 input intake: nested-mode host events ARE the human
        // principal's input, origin-tagged `physical` at this single point
        // of entry (B2) — see `crate::input`.
        WinitEvent::Input(event) => state.handle_input(&event),
        // Not ignorable: a key held when focus leaves produces no release
        // event on either backend, so the dead-man switch must be told the
        // hold is no longer verifiable (see `handle_focus`).
        WinitEvent::Focus(focused) => state.handle_focus(focused),
    })?;

    let grab = Rc::new(RefCell::new(ConsentGrab::new()));
    let now = Rc::new(Cell::new(Instant::now()));
    let deadman = Rc::new(RefCell::new(DeadManSwitch::new(dead_man)));
    info!(
        chord = dead_man.chord.name(),
        hold_ms = dead_man.hold.as_millis(),
        "dead-man switch armed: holding this key revokes every grant in the session"
    );
    let router = input::InputRouter::new(ConsentGate::new(
        Rc::clone(&grab),
        Rc::clone(&now),
        DeadManHook::new(Rc::clone(&deadman), Rc::clone(&now), input::NoopHook),
    ));
    // The session's trusted indicator, minted in `run_session` before the
    // listener accepted anyone (issue #85). Read by `Copy` before the seed is
    // consumed into the runtime below.
    let indicator: TrustedIndicator = seed
        .as_ref()
        .expect("the seed is present until the state is built")
        .indicator;
    let mut state = NestedState {
        view: NestedView {
            backend,
            scene: Scene::new(),
            consent: ConsentSurface::new(indicator),
            texture: None,
        },
        runtime: Runtime::new(
            seed.take().expect("the seed is consumed exactly once"),
            router,
        ),
        grab,
        now,
        deadman,
        deadman_timer_armed: false,
        loop_signal: event_loop.get_signal(),
        loop_handle: event_loop.handle(),
        fatal: None,
    };

    // Kick off the redraw cycle; each completed frame requests the next,
    // so presentation is paced by the host compositor (60 Hz on a 60 Hz
    // host — winit redraws on Wayland are frame-callback driven), with
    // FRAME_BUDGET as the floor when the host doesn't throttle swaps.
    state.view.backend.window().request_redraw();

    // Every runtime source goes in before the loop starts, and before
    // anything spawns a realm: a shim whose socketpair is not being serviced
    // wedges permanently (see `session::install`).
    if let Err(err) = session::install(&loop_handle, &mut state.runtime) {
        *recovered = Some(state.runtime.into_recorder());
        return Err(err);
    }

    // The realm, only now: `install` has put the listener and the sweeps on
    // the loop, so it is ready to service the shim's socketpair, and
    // `event_loop.run` is the very next statement. Spawning before the reader
    // exists wedges the shim permanently rather than slowly — it blocks on
    // `configure` with no timeout on its side (trap T1).
    if let Err(err) = session::start_realm(&mut state) {
        error!("fatal: cannot start the session's realm: {err}");
        *recovered = Some(state.runtime.into_recorder());
        return Err(err);
    }

    let outcome = event_loop
        .run(None, &mut state, session::post_dispatch)
        .map_err(|err| Box::new(err) as Box<dyn Error>)
        .and_then(|()| match state.fatal.take() {
            Some(err) => Err(err),
            None => Ok(()),
        });
    // The shutdown ladder, after the loop has stopped and before the recorder
    // is handed back: it blocks by design, so it must not run inside a live
    // compositor loop, and its `realm_died` / `realm_exited` entries belong to
    // this run. The event loop is still alive in this scope, which is what
    // lets rung 0 retire the shim connection's registration.
    session::shutdown_realm(&mut state);

    // Recovered before the early return below, so a fatal run still returns
    // the recorder that must write the footer naming it.
    *recovered = Some(state.runtime.into_recorder());
    outcome?;
    info!("event loop stopped, shutting down");
    Ok(())
}

/// What the dead-man wiring needs from whoever owns the switch.
///
/// Exists so the timer path, its rescheduling, its bookkeeping and the
/// trigger disposal are **one implementation shared with production** rather
/// than a pattern the test re-types. The previous shape had all of it inline
/// in [`NestedState`], which no test could construct (it owns a GL context and
/// a host window); a review mutated every path here — the timer, both
/// backstops, the replay drain, and the sharing of the `Rc` between the router
/// hook and the backend — and the full workspace suite stayed green for all
/// five. This is the same treatment [`window_pixels`] was given for the same
/// reason.
pub(crate) trait DeadManHost {
    /// The switch, shared with the router's innermost hook. Sharing is the
    /// point: a host that hands out a *different* switch than its router
    /// watches is wired wrong, and `the_backends_hook_and_timer_share_one_switch`
    /// exists to catch exactly that.
    fn switch(&self) -> &Rc<RefCell<DeadManSwitch>>;
    /// Whether a one-shot timer is already outstanding, so an arming keypress
    /// does not insert a fresh calloop source per event.
    fn timer_armed(&mut self) -> &mut bool;
    /// Dispose of one completed chord. The nested backend applies it to the
    /// session's real authority through
    /// [`crate::session::Runtime::apply_dead_man`]; a test host counts it.
    fn on_trigger(&mut self, trigger: Trigger);
}

/// Complete the chord if due, dispose of any trigger, and keep the timer
/// armed. Idempotent and level-triggered; safe to call from anywhere.
pub(crate) fn deadman_tick<D: DeadManHost + 'static>(
    host: &mut D,
    handle: &LoopHandle<'static, D>,
    now: Instant,
) {
    let trigger = {
        let mut switch = host.switch().borrow_mut();
        switch.fire_if_due(now);
        switch.take_trigger()
    };
    if let Some(trigger) = trigger {
        host.on_trigger(trigger);
    }
    arm_deadman_timer(host, handle);
}

/// Arm a one-shot timer at the hold's deadline, if a hold is armed and no
/// timer is outstanding.
///
/// The callback reschedules itself rather than dropping when the hold is
/// still pending, which closes the gap an early or coarse wakeup would
/// otherwise leave: calloop is free to fire a timer marginally early, and a
/// `Drop` on a not-yet-due check would leave the switch with no timer at all
/// until the human pressed something else.
pub(crate) fn arm_deadman_timer<D: DeadManHost + 'static>(
    host: &mut D,
    handle: &LoopHandle<'static, D>,
) {
    if *host.timer_armed() {
        return;
    }
    let Some(deadline) = host.switch().borrow().deadline() else {
        return;
    };
    let armed = handle.insert_source(
        Timer::from_deadline(deadline),
        |_deadline, _, host: &mut D| {
            let trigger = {
                let mut switch = host.switch().borrow_mut();
                switch.fire_if_due(Instant::now());
                switch.take_trigger()
            };
            if let Some(trigger) = trigger {
                host.on_trigger(trigger);
            }
            // Read out into a local first: the `Ref` must be dropped before
            // `timer_armed` takes `&mut host`.
            let next = host.switch().borrow().deadline();
            match next {
                // Still held and not yet due: this wakeup was early.
                Some(next) => TimeoutAction::ToInstant(next),
                None => {
                    *host.timer_armed() = false;
                    TimeoutAction::Drop
                }
            }
        },
    );
    match armed {
        Ok(_token) => *host.timer_armed() = true,
        Err(err) => {
            // Not fatal, and deliberately not silent: the frame-cadence
            // backstop still completes the chord, so the switch degrades
            // from "fires at the deadline" to "fires within a frame".
            error!(
                "could not arm the dead-man timer ({}); the off-switch now depends on the \
                 frame-cadence backstop",
                err.error
            );
        }
    }
}

/// Route one turn's intake events, then re-route whatever the dead-man
/// watcher decided the app is owed.
///
/// The drain must run **after** the loop, never before it and never inside
/// it: the gate withholds a chord press until it knows whether the human is
/// tapping or holding, and a tap owes the app the press *and* its release, in
/// that order ([`crate::deadman`]). Re-routing rather than injecting is what
/// keeps every downstream policy applying to the replay — a prompt raised
/// between the press and the release consumes the replayed press exactly as
/// it would a fresh one, with no carve-out anywhere.
///
/// Free-standing and generic over the hook so a test can drive it with a spy
/// sink and assert the drain really happens: deleting it costs the confined
/// app its Escape key permanently, which is the exact harm tap-through
/// exists to prevent, and nothing else in the suite notices.
pub(crate) fn route_turn<H: input::PreemptionHook>(
    router: &mut input::InputRouter<H>,
    switch: &RefCell<DeadManSwitch>,
    inputs: impl IntoIterator<Item = input::SeatInput>,
    view: (u32, u32),
    surface: Option<(u32, u32)>,
    deliver: &mut dyn FnMut(input::SeatDelivery),
) {
    for input in inputs {
        if let Some(delivery) = router.route(input, view, surface) {
            deliver(delivery);
        }
    }
    // A separate statement from the loop so the switch's borrow ends before
    // routing re-enters the hook that holds it.
    let replay = switch.borrow_mut().take_replay();
    for replayed in replay {
        if let Some(delivery) = router.route(replayed, view, surface) {
            deliver(delivery);
        }
    }
}

impl DeadManHost for NestedState {
    fn switch(&self) -> &Rc<RefCell<DeadManSwitch>> {
        &self.deadman
    }

    fn timer_armed(&mut self) -> &mut bool {
        &mut self.deadman_timer_armed
    }

    /// The off-switch really revokes now.
    ///
    /// Until the runtime wiring landed there was no grant table in this
    /// process, so a completed chord could only be logged; that gap is closed
    /// here, through [`Runtime::apply_dead_man`], which revokes every grant,
    /// denies every pending petition, seals the table against a decision
    /// already in flight, and delivers each denial to its petitioner.
    fn on_trigger(&mut self, trigger: Trigger) {
        self.runtime.apply_dead_man(&trigger, Instant::now());
    }
}

impl NestedState {
    /// P1.3.7 input intake, nested mode. The host compositor delivered
    /// this event to the core's window, so it is the human principal's
    /// input (implicit principal, no identity ceremony — trusted as human,
    /// the documented MVP limitation; real physical-origin verification is
    /// Phase 3). [`input::intake_physical`] binds `origin=physical` at
    /// this single point of entry (B2) and the router maps it through the
    /// scene's current layout.
    ///
    /// No shim connection exists at runtime until the realm spawn manager
    /// (P1.5.2) inherits the socketpair at fork, so routed deliveries are
    /// trace-dropped here for now; the full route → encode →
    /// `ShimServer::deliver_seat_event` → wire path is exercised
    /// end-to-end by `crate::input`'s tests against the mock shim.
    fn handle_input(&mut self, event: &smithay::backend::input::InputEvent<WinitInput>) {
        let size = self.view.backend.window_size();
        let view = (size.w.max(0) as u32, size.h.max(0) as u32);
        // The consent grab hit-tests in this same view space, so it is fed
        // from this same local, on the line before the route that uses it.
        // A view sourced anywhere else could drift from the one the router
        // maps with, and a drifted hit test can turn a click aimed at Deny
        // into an Allow (see `consent::grab`).
        self.grab.borrow_mut().set_view(view);
        // One clock sample for the whole dispatch turn, taken before any
        // event of it is judged, so every event in this batch is judged
        // against the same instant (the clock discipline the rest of the
        // core follows). The grab and the dead-man watcher read it through
        // their hooks.
        self.now.set(Instant::now());
        // Route this turn's events, then drain the dead-man watcher's replay
        // (see `route_turn`, which owns both halves so they can be tested).
        let surface = self.view.scene.surface_size();
        // Disjoint field borrows: the router is handed to `route_turn` while
        // the delivery sink below reaches the realm's shim session. Both are
        // fields of the same `Runtime`, so they are split here rather than
        // reached through `&mut self` twice.
        let session::Runtime {
            router,
            realm,
            kernel,
            ..
        } = &mut self.runtime;
        // #118 wiring point (deferred): `intake_physical` resolves keyboard
        // events from the scancode alone, so it delivers only the
        // layout-invariant subset and drops text keys — Smithay 0.7.0's
        // `WinitKeyboardInputEvent` hides winit's interpreted `logical_key`
        // from us here. Closing the gap (issue #118, "own the winit glue")
        // means this backend owning the winit event loop so a
        // `WindowEvent::KeyboardInput`'s `logical_key` is in reach, resolving it
        // to an X keysym, and routing keyboard through
        // `input::physical_key(evdev, Some(keysym), state)` instead of the
        // scancode-only `intake_physical` arm. The resolution and delivery past
        // that seam are already whole and tested (`input`'s
        // `a_text_key_given_a_host_keysym_reaches_the_app_as_physical_input`).
        route_turn(
            router,
            &self.deadman,
            input::intake_physical(event, (size.w, size.h)),
            view,
            surface,
            &mut |delivery| {
                // Physical input reaches the realm's seat over the same
                // outbox an agent's chokepoint-admitted actuation uses; the
                // origin tag bound at intake rides the wire unchanged (B2).
                let Some(realm) = realm.as_ref() else {
                    trace!(origin = ?delivery.origin(), "routed input dropped: no realm attached");
                    return;
                };
                let Some(server) = realm.server.as_ref() else {
                    return;
                };
                let mut send = |frame: &[u8]| realm.outbox.send(frame);
                match server.deliver_seat_event(&delivery, &mut send) {
                    // Journal the delivery with its origin through the same
                    // funnel the agent path uses (issue #83). This is the
                    // *only* site that produces `origin="physical"` at runtime
                    // — a human's input reaching the app is the half of the
                    // physical-vs-emulated audit that never crosses a
                    // chokepoint — so sharing `record_seat_delivery` keeps it
                    // from silently diverging from the tested agent copy (and
                    // inherits the motion-flood guard the physical path needs
                    // most).
                    Ok(sent) => {
                        if sent {
                            crate::input::record_seat_delivery(&mut kernel.recorder, &delivery);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, "seat delivery to the realm failed");
                    }
                }
            },
        );
        // Backstop 2 of 3 for the elapse check (`crate::deadman`): the
        // switch is already being asked about this turn's events, so ask it
        // about the clock too.
        self.deadman_tick();
    }

    /// The host window lost or gained keyboard focus.
    ///
    /// On loss the dead-man hold is forgotten. This is the one cause of a
    /// lost key release that the core can actually see: the release will be
    /// delivered to whatever window took focus, and neither backend reports
    /// it (Smithay 0.7.0 filters `is_synthetic` key events, and winit's
    /// Wayland `wl_keyboard.leave` emits no key events at all — both verified
    /// in the pinned sources). Without this, an ordinary alt-tab with Esc
    /// down either revokes the whole session a second later with no gesture
    /// behind it, or — after such a fire — leaves the switch in a state only
    /// a release can exit, silently dead with no indicator to say so.
    ///
    /// [`DeadManSwitch::forget_hold`] carries the argument for cancelling
    /// rather than firing, and for why an agent cannot reach this path.
    ///
    /// The tick afterwards is not decoration: it lets the outstanding timer's
    /// callback see `deadline() == None` on its next wakeup and drop itself,
    /// and it repaints nothing, so a disarmed indicator disappears at the
    /// host's ordinary frame cadence.
    fn handle_focus(&mut self, focused: bool) {
        if focused {
            return;
        }
        debug!("host window lost keyboard focus; forgetting any dead-man hold in progress");
        self.deadman.borrow_mut().forget_hold();
        self.deadman_tick();
    }

    /// Complete the dead-man chord if its hold has elapsed, apply whatever
    /// that produced, and keep the timer armed while a hold is in progress.
    ///
    /// Called from three places on purpose ([`crate::deadman`], design
    /// tension 2): the timer, every input dispatch turn, and every frame.
    /// [`DeadManSwitch::fire_if_due`] is idempotent and level-triggered, so
    /// calling it more often only shortens latency -- and a timer that never
    /// fires costs at most one frame rather than the whole switch.
    ///
    /// The body is [`deadman_tick`], a free function over [`DeadManHost`],
    /// so the whole time-driven path can be driven by a test with a real
    /// `EventLoop` and no display. That split is not cosmetic: with the logic
    /// inline here, deleting every time-driven path left the entire workspace
    /// suite green, which for a dead-man switch is the one regression nothing
    /// may be allowed to hide.
    fn deadman_tick(&mut self) {
        let handle = self.loop_handle.clone();
        deadman_tick(self, &handle, Instant::now());
        // Deliberately no `request_redraw` here. The redraw chain is already
        // self-sustaining (`schedule_next_frame`), so the indicator animates
        // and disappears at the host's frame cadence for free — and adding a
        // request on a path the frame loop itself calls would chain a redraw
        // per frame *outside* `FRAME_BUDGET`, turning a held key into a
        // busy-spin.
    }

    /// Draw one frame. Rendering failure is fatal to the skeleton: log it,
    /// record it for [`run`] to propagate (non-zero exit), and stop the
    /// loop rather than spinning on a broken GL context.
    fn redraw(&mut self) {
        // Backstop 3 of 3 for the elapse check (`crate::deadman`), and the
        // one that survives a lost timer: at the host's frame cadence this
        // bounds the switch's latency to ~16.7 ms with no timer at all.
        // Guarded on an armed hold so an idle window pays nothing per frame.
        //
        // **Its exact reach, because overstating it would be worse than not
        // having it.** Placed before `try_redraw` so the frame that discovers
        // a zero-sized (minimized) window still counts — but `try_redraw`
        // returns early on that path *without* reaching `schedule_next_frame`,
        // which is the only thing that chains the next redraw. So a minimized
        // window gets this one last tick and then no more, and while it stays
        // minimized the switch is covered by the calloop timer alone. That is
        // acceptable (the timer is the primary path; this is the backstop),
        // and it is written down because a maintainer who weakened the timer
        // on the strength of a three-way redundancy that does not hold there
        // would leave the off-switch with nothing.
        //
        // In practice a minimized window is also an unfocused one, and
        // `handle_focus` has already forgotten the hold by then.
        if self.deadman.borrow().deadline().is_some() {
            self.deadman_tick();
        }
        match self.try_redraw() {
            // The composite landed, so the realm's owed frame callbacks are
            // due now. This is the other half of `Presenter::redraw`'s
            // `Scheduled` answer: the runtime deliberately did not emit them
            // at request time, because on this backend a requested redraw is
            // not a drawn one, and telling a paced shim otherwise would let it
            // run unthrottled.
            Ok(session::Presentation::Completed) => session::emit_presented(&mut self.runtime),
            // A zero-sized (minimized) window draws nothing. The callbacks
            // stay owed — which is the point of distinguishing the two: this
            // path returns `Ok` and must still not tell the shim a frame was
            // presented. `schedule_next_frame` is not reached either, so the
            // next composite comes from the resize, and it discharges them.
            Ok(session::Presentation::Scheduled) => {}
            Err(err) => {
                error!("render failed, shutting down: {err}");
                self.fatal = Some(err);
                self.loop_signal.stop();
            }
        }
    }

    /// Draw one frame, reporting whether a composite actually happened.
    ///
    /// The return is load-bearing rather than informational: [`Self::redraw`]
    /// discharges the realm's owed `frame_done` on
    /// [`session::Presentation::Completed`] only, so a path that returns `Ok`
    /// without drawing must say so. Today that is the minimized window.
    fn try_redraw(&mut self) -> Result<session::Presentation, Box<dyn Error>> {
        let frame_start = Instant::now();
        let size = self.view.backend.window_size();
        if size.w <= 0 || size.h <= 0 {
            // Zero-sized (e.g. minimized) window: skip, resize will redraw.
            // Nothing was composited, so the frame callbacks stay owed.
            return Ok(session::Presentation::Scheduled);
        }

        // Re-upload when the window size, the scene content, or the consent
        // surface changed: the same shared composition both backends present
        // (P1.3.3), plus the prompt (P1.7.1), uploaded here as a full-window
        // texture. Keying on both generations is what makes a prompt appear
        // and disappear at the host's very next frame instead of whenever the
        // scene happens to change next.
        // The hold indicator is sampled once per frame, from the same clock
        // reading nothing else in this frame contends for, and folded into
        // the cache key — so the bar really animates instead of appearing
        // whenever the scene next happens to change (the trap
        // `the_texture_key_changes_on_every_visible_transition` was written
        // for, now with a third input).
        let hold = self.deadman.borrow().hold_progress(Instant::now());
        let key = TextureKey::current(size, &self.view.scene, &self.view.consent, hold);
        if self.view.texture.as_ref().map(|v| v.key) != Some(key) {
            let pixels = window_pixels(&self.view.scene, &mut self.view.consent, hold, size);
            let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
            let texture = self.view.backend.renderer().import_memory(
                &pixels,
                Fourcc::Abgr8888,
                buffer_size,
                false,
            )?;
            self.view.texture = Some(SceneTexture { texture, key });
        }

        let full_window = Rectangle::from_size(size);
        {
            // Field-level borrows: `bind` holds `self.view.backend` mutably while
            // the view texture is read from `self.view.texture`.
            let view = self.view.texture.as_ref().expect("view composed above");
            let (renderer, mut framebuffer) = self.view.backend.bind()?;
            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
            frame.clear(CLEAR_COLOR, &[full_window])?;
            // Qualified call: GlesFrame has an inherent method of the same
            // name (extra custom-shader arguments) that would shadow the
            // renderer-agnostic trait method.
            Frame::render_texture_from_to(
                &mut frame,
                &view.texture,
                Rectangle::from_size(Size::<f64, Buffer>::from((size.w as f64, size.h as f64))),
                full_window,
                &[full_window],
                &[],
                Transform::Normal,
                1.0,
            )?;
            // The EGL swapchain synchronizes the frame; nothing to await
            // here (same posture as Smithay's own winit example).
            let _sync_point = frame.finish()?;
        }
        // Full-frame submit; damage tracking becomes worthwhile once client
        // damage arrives over the shim protocol (P1.3.4).
        self.view.backend.submit(None)?;
        trace!(?size, "frame submitted");
        self.schedule_next_frame(frame_start)?;
        Ok(session::Presentation::Completed)
    }

    /// Chain the next redraw. If the swap blocked for at least
    /// [`FRAME_BUDGET`] (effective vsync), request it immediately;
    /// otherwise arm a one-shot timer for the budget's remainder so the
    /// render loop stays capped near 60 Hz instead of busy-spinning on
    /// hosts without swap throttling.
    fn schedule_next_frame(&mut self, frame_start: Instant) -> Result<(), Box<dyn Error>> {
        let elapsed = frame_start.elapsed();
        if elapsed >= FRAME_BUDGET {
            self.view.backend.window().request_redraw();
        } else {
            let timer = Timer::from_duration(FRAME_BUDGET - elapsed);
            self.loop_handle
                .insert_source(timer, |_deadline, _, state| {
                    state.view.backend.window().request_redraw();
                    TimeoutAction::Drop
                })
                .map_err(|err| err.error)?;
        }
        Ok(())
    }
}

impl session::RuntimeHost for NestedState {
    type Hook = ConsentGate<DeadManHook<input::NoopHook>>;
    type View = NestedView;

    fn split(&mut self) -> (&mut Runtime<Self::Hook>, &mut NestedView) {
        (&mut self.runtime, &mut self.view)
    }

    fn loop_handle(&self) -> LoopHandle<'static, Self> {
        self.loop_handle.clone()
    }

    fn stop(&mut self, fatal: Option<Box<dyn Error>>) {
        self.fatal = fatal;
        self.loop_signal.stop();
    }

    /// Drive interactive consent for this dispatch round (issue #90): the
    /// nested backend is the only one that can, because it is the only place a
    /// physical input device and a human-visible display both exist.
    ///
    /// The `Rc` is cloned first so the `RefMut` it yields borrows nothing of
    /// `self` — that leaves `&mut self.runtime` and `&mut self.view.consent` as
    /// two disjoint field borrows for [`session::service_consent_round`], which
    /// raises the prompt, drains the grab's decisions and lowers stale cards.
    /// A `true` return means the visible output changed (a card went up or came
    /// down), so the frame is marked dirty and a redraw is requested — the host
    /// compositor draws it on its next frame, the same path every other visible
    /// transition takes here.
    fn service_consent(&mut self, now: Instant) {
        let grab = Rc::clone(&self.grab);
        let mut grab = grab.borrow_mut();
        if session::service_consent_round(&mut grab, &mut self.runtime, &mut self.view.consent, now)
        {
            self.runtime.dirty = true;
            self.view.backend.window().request_redraw();
        }
    }
}

impl session::Presenter for NestedView {
    fn scene(&mut self) -> &mut Scene {
        &mut self.scene
    }

    fn view_size(&self) -> (u32, u32) {
        let size = self.backend.window_size();
        (size.w.max(0) as u32, size.h.max(0) as u32)
    }

    /// Composites nothing, and deliberately so: this backend does not own its
    /// frame clock — so it answers [`session::Presentation::Scheduled`] and
    /// the realm's `frame_done` stays owed.
    ///
    /// The host compositor owns the clock. A composite here would present a
    /// frame the host never asked for, outside `FRAME_BUDGET`, and would race
    /// the redraw chain `schedule_next_frame` maintains. The runtime's
    /// once-per-round dirty flag instead becomes a `request_redraw` in
    /// [`session::Presenter::request_present`], and the host drives the
    /// actual composite through `WinitEvent::Redraw` as it always has.
    ///
    /// **The debt this creates is discharged in [`NestedState::redraw`]**,
    /// which calls [`session::emit_presented`] after `try_redraw` has actually
    /// drawn. Returning `Completed` here instead would be the silent pacing
    /// bug [`session::Presentation`] documents: the shim would be handed a
    /// frame callback per dispatch round with no composite between them, and a
    /// client that paces on `frame_done` would stop pacing.
    fn redraw(&mut self) -> Result<session::Presentation, Box<dyn Error>> {
        Ok(session::Presentation::Scheduled)
    }

    /// The bare realm view, composed on the CPU from the retained scene —
    /// the **same bytes headless retains for capture** (P1.3.8, issue #116).
    ///
    /// This backend renders through EGL/GLES straight into the host's window
    /// and keeps no readable image of the *presented* frame — there is no
    /// `ExportMem` bind, no `copy_framebuffer` of the window. It does not need
    /// one. The capture the chokepoint serves is the **realm view**, and that
    /// is exactly what [`Scene::compose`] produces: tightly packed RGBA8888,
    /// rows top-down, every pixel opaque — the one composition implementation
    /// both backends present (P1.3.3). Headless retains that composition in a
    /// pixman image and reads it back; its own tests pin the readback as a
    /// **byte-for-byte identity** of `Scene::compose`
    /// (`committed_shm_buffer_reaches_retained_framebuffer_and_capture`,
    /// `retained_framebuffer_is_the_capture_source`). So composing the scene
    /// here yields the identical bytes headless would for the same scene,
    /// which is the P1.3.8 requirement — nested and headless captures cannot
    /// drift.
    ///
    /// **Why compose rather than read back the GL framebuffer.** Two reasons,
    /// both decisive:
    ///
    /// - *The overlay must never reach a capture.* The GL framebuffer holds
    ///   the *human-visible* output — the realm view **plus** the consent
    ///   prompt (P1.7.1) and the dead-man hold indicator (P1.7.3), applied in
    ///   [`window_pixels`]. Reading it back would put the one thing
    ///   `docs/protocol/05-vitrin_consent.md` forbids outright into every
    ///   `vitrin_view.frame_ready`. Composing the bare scene instead is
    ///   overlay-free *structurally*: [`Scene::compose`] is upstream of the
    ///   output-stage fork ([`super::human_visible_from_view`]) where the
    ///   overlay joins, so no card and no indicator pixel can be present here
    ///   — the same by-construction argument the headless capture rests on.
    /// - *Cost.* A per-frame GL readback would add a full FBO map to the
    ///   nested compositing path, already a known perf hot-spot. A CPU
    ///   `Scene::compose` is cheap and, reached only through
    ///   [`session::post_dispatch`] on a **dirty** dispatch round (never the
    ///   agent-request path, never per host frame), it composes once per
    ///   latched batch of commits — not once per presented frame.
    ///
    /// **When this refreshes: per dirty round, not per agent request.**
    /// [`session::post_dispatch`] repopulates `view_cache` from this method on
    /// every dirty round *whether or not an agent is observing* — the same
    /// unconditional refresh headless takes, and deliberately so. The refresh
    /// stays on the compositing path and off the agent-request path because a
    /// request-path refresh would make an agent's capture trigger a composite
    /// and make goldens depend on request timing (`session::post_dispatch`
    /// documents exactly this). The cost of that choice on the nested hot-path
    /// is one extra CPU `Scene::compose` per *latched batch of commits* — a
    /// static or idle scene is not dirty and pays nothing, and it is a compose,
    /// never the forbidden per-frame GL readback. Gating this on a live observe
    /// grant so a nested session with no agent pays nothing was considered and
    /// rejected: it would couple the compositing path to grant state and make
    /// the capture cache's freshness — and the goldens that pin it — depend on
    /// whether a grant happened to exist, the very timing dependence the
    /// request-path refresh is kept off the table to avoid.
    ///
    /// `None` for a degenerate (zero-sized, e.g. minimized) window: there is
    /// no realm view to compose, so the capture meets the chokepoint's
    /// `no_surface` refusal — the honest answer, exactly as before this
    /// backend could serve captures at all.
    fn view_rgba(&mut self) -> Option<Vec<u8>> {
        // The window size is the size the realm view composes at (P1.3.3), the
        // same one `view_size` and `try_redraw` read. `capture_pixels` composes
        // the bare scene — never `window_pixels` — so the overlay and the hold
        // indicator cannot reach a capture.
        capture_pixels(&self.scene, self.backend.window_size())
    }

    fn request_present(&mut self) {
        self.backend.window().request_redraw();
    }

    /// The scene, and `None` for the retained half.
    ///
    /// Nested composites straight into the host compositor's surface and
    /// keeps no *retained* image of its own to scrub — [`Self::view_rgba`]
    /// composes the capture fresh from the scene on demand rather than reading
    /// a held framebuffer. That is exactly why there is nothing to scrub here
    /// and no stale-frame hazard to defend against: the death funnel takes the
    /// dead realm's surface out of the scene ([`Scene::clear_surface`]), so the
    /// very next `view_rgba` composes the empty-scene background — the dead
    /// realm's pixels are gone by construction, with `view_is_live` gating the
    /// capture shut on top of that. Headless needs the retained-image scrub
    /// because it reads back a *held* framebuffer that would otherwise keep the
    /// last painted frame; nested, composing on demand from the live scene, has
    /// no such held image and so needs no scrub.
    fn teardown_view(
        &mut self,
    ) -> (
        &mut Scene,
        Option<&mut dyn crate::lifecycle::RetainedOutput>,
    ) {
        (&mut self.scene, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::tests::prompt_fixture;
    use crate::scene::{tests::client_pixels, SurfaceContent};

    fn size_of(w: i32, h: i32) -> Size<i32, Physical> {
        (w, h).into()
    }

    /// Nested mode must actually put the prompt in the host window.
    ///
    /// This is the acceptance criterion's nested half, held as far as a
    /// display-free runner can hold it. GL presentation needs an EGL context,
    /// so what is pinned here is the pixels [`NestedState::try_redraw`]
    /// uploads: with a prompt up they are the shared human-visible
    /// composition, card rows and all; with none up they are the bare realm
    /// view. A regression that dropped the overlay from the upload — which
    /// previously passed the entire suite — fails here.
    #[test]
    fn the_nested_window_uploads_the_consent_overlay() {
        const W: i32 = 800;
        const H: i32 = 600;
        let size = size_of(W, H);

        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(300, 200), 300, 200).expect("content"));
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());

        // No prompt: the window is the realm view plus the trusted band along
        // the top (issue #85). Below the band it is the realm view byte for
        // byte; the band itself is the session colour, present on the
        // human-visible upload and never in the capture (Scene::compose).
        let plain = window_pixels(&scene, &mut consent, None, size);
        let composed = scene.compose(W as u32, H as u32);
        let band_bytes =
            crate::consent::TRUST_BAND_HEIGHT as usize * W as usize * crate::scene::BYTES_PER_PIXEL;
        assert_eq!(
            plain[band_bytes..],
            composed[band_bytes..],
            "below the band, the host window is the realm view unchanged"
        );
        assert_eq!(
            plain[..crate::scene::BYTES_PER_PIXEL],
            TrustedIndicator::for_test().color(),
            "the trusted band is on the human-visible upload"
        );
        assert_ne!(
            composed[..crate::scene::BYTES_PER_PIXEL],
            TrustedIndicator::for_test().color(),
            "...and never in the capture"
        );

        // Prompt up: the window shows the shared human-visible composition.
        consent.show_for_test(prompt_fixture());
        let with_prompt = window_pixels(&scene, &mut consent, None, size);
        assert_ne!(
            with_prompt, plain,
            "the prompt must change what is uploaded"
        );
        let mut expected = ConsentSurface::new(TrustedIndicator::for_test());
        expected.show_for_test(prompt_fixture());
        assert_eq!(
            with_prompt,
            super::super::compose_human_visible(&scene, &mut expected, W as u32, H as u32),
            "nested must upload the same composition headless retains, so the \
             two backends cannot drift in what a human sees"
        );

        // ...and the card is really painted where the hit test will look.
        let card = crate::consent::render::rasterize(&prompt_fixture());
        let (cx, cy) = consent
            .card_origin(W as u32, H as u32)
            .expect("a prompt is up");
        assert!(cx >= 0 && cy >= 0, "the card fits in an {W}x{H} window");
        for row in 0..card.height {
            let d = ((cy as u32 + row) as usize * W as usize + cx as usize)
                * crate::scene::BYTES_PER_PIXEL;
            let s = row as usize * card.width as usize * crate::scene::BYTES_PER_PIXEL;
            let run = card.width as usize * crate::scene::BYTES_PER_PIXEL;
            assert_eq!(
                &with_prompt[d..d + run],
                &card.rgba[s..s + run],
                "card row {row} must appear verbatim in the uploaded pixels"
            );
        }
    }

    /// The nested backend's **capture** path serves the bare realm view — the
    /// same bytes headless retains — and never the presented window (P1.3.8,
    /// issue #116).
    ///
    /// GL presentation needs a display, but the decision [`NestedView::view_rgba`]
    /// encodes is *which pixels to serve*, and that is [`capture_pixels`], a pure
    /// function of the scene. This pins it: a regression that served the
    /// human-visible composition instead — leaking the consent overlay (P1.7.1)
    /// or the dead-man hold indicator (P1.7.3) into every capture — fails here,
    /// exactly as [`the_nested_window_uploads_the_consent_overlay`] guards the
    /// other, human-visible half of the same fork.
    #[test]
    fn the_nested_capture_is_the_bare_realm_view_never_the_overlay() {
        const W: i32 = 800;
        const H: i32 = 600;
        let size = size_of(W, H);

        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(300, 200), 300, 200).expect("content"));

        // The capture is exactly `Scene::compose`: the one composition both
        // backends present, no renderer of its own. The headless backend's
        // own tests pin its retained readback as byte-for-byte
        // `Scene::compose`, so this equality is transitively "nested capture
        // == headless capture for the same scene" — the cross-backend proof
        // is made concrete against the real pixman readback in
        // `backend::headless`'s `nested_and_headless_captures_are_byte_identical`.
        let capture = capture_pixels(&scene, size).expect("a nonzero view has pixels");
        assert_eq!(
            capture,
            scene.compose(W as u32, H as u32),
            "the nested capture must be the bare Scene::compose realm view"
        );

        // A prompt up changes the human-visible window but not the capture: the
        // overlay is excluded by construction (it joins only downstream of
        // `Scene::compose`, in `human_visible_from_view`).
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        consent.show_for_test(prompt_fixture());
        let human_visible =
            super::super::compose_human_visible(&scene, &mut consent, W as u32, H as u32);
        assert_ne!(
            capture, human_visible,
            "the human-visible window carries the prompt; the capture must not"
        );
        // Pixel-level: no run of the consent card's bytes survives anywhere in
        // the capture — catches a partial leak a whole-buffer compare could not.
        let card = crate::consent::render::rasterize(&prompt_fixture());
        let card_row = &card.rgba[..card.width as usize * crate::scene::BYTES_PER_PIXEL];
        assert!(
            !capture.windows(card_row.len()).any(|w| w == card_row),
            "a row of consent-prompt pixels reached a nested capture"
        );

        // The dead-man hold indicator is likewise absent: the presented window
        // mid-hold differs from the capture, which carries neither the trusted
        // band nor the indicator — it is the bare view, unchanged.
        let mut idle = ConsentSurface::new(TrustedIndicator::for_test());
        let with_hold = window_pixels(&scene, &mut idle, Some(0.5), size);
        assert_ne!(
            capture, with_hold,
            "the dead-man hold indicator must never reach the capture"
        );

        // A degenerate (minimized) window has no realm view to serve, so the
        // capture meets the chokepoint's `no_surface` refusal.
        assert!(capture_pixels(&scene, size_of(0, 0)).is_none());
        assert!(capture_pixels(&scene, size_of(W, 0)).is_none());
    }

    /// The texture cache must re-upload on every transition a human would see.
    ///
    /// The consent generation is the one that is easy to leave out and
    /// impossible to notice: without it a prompt appears (or a decided prompt
    /// lingers) only when the scene next happens to change, which for a static
    /// realm is never.
    #[test]
    fn the_texture_key_changes_on_every_visible_transition() {
        let size = size_of(800, 600);
        let mut scene = Scene::new();
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());

        let base = TextureKey::current(size, &scene, &consent, None);
        assert_eq!(
            base,
            TextureKey::current(size, &scene, &consent, None),
            "an unchanged output must not force a re-upload"
        );

        // A prompt going up, and coming back down, both re-upload.
        consent.show_for_test(prompt_fixture());
        let shown = TextureKey::current(size, &scene, &consent, None);
        assert_ne!(base, shown, "a prompt appearing must re-upload");
        consent.dismiss_for_test();
        let dismissed = TextureKey::current(size, &scene, &consent, None);
        assert_ne!(shown, dismissed, "a prompt going away must re-upload");

        // The queue advancing to a different petition re-uploads too, so the
        // window cannot keep showing a decided petition's card.
        consent.show_for_test(prompt_fixture());
        let first = TextureKey::current(size, &scene, &consent, None);
        let mut next = prompt_fixture();
        next.principal =
            crate::identity::PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        consent.show_for_test(next);
        assert_ne!(
            first,
            TextureKey::current(size, &scene, &consent, None),
            "a different petition must re-upload"
        );

        // And the two pre-existing inputs still matter.
        let held = TextureKey::current(size, &scene, &consent, None);
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        assert_ne!(
            held,
            TextureKey::current(size, &scene, &consent, None),
            "a scene commit must re-upload"
        );
        assert_ne!(
            TextureKey::current(size, &scene, &consent, None),
            TextureKey::current(size_of(640, 480), &scene, &consent, None),
            "a resize must re-upload"
        );
    }

    /// The dead-man hold indicator must actually reach the host window, and
    /// must re-upload as it fills.
    ///
    /// Same trap as the consent generation, one step further out: an
    /// indicator left out of the cache key would appear only when the scene
    /// or a prompt next happened to change — which, during a hold on a static
    /// realm, is never. The human would then hold the off-switch and see
    /// nothing at all, which is exactly the "cannot tell it is working"
    /// failure the indicator exists to prevent.
    #[test]
    fn the_hold_indicator_reaches_the_window_and_animates() {
        const W: i32 = 320;
        const H: i32 = 200;
        let size = size_of(W, H);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());

        // No hold: the window is the ordinary human-visible composition,
        // byte for byte. The indicator costs an idle session nothing.
        let idle = window_pixels(&scene, &mut consent, None, size);
        assert_eq!(
            idle,
            super::super::compose_human_visible(&scene, &mut consent, W as u32, H as u32)
        );

        // Mid-hold: the top edge changes, and nothing below the bar does.
        let holding = window_pixels(&scene, &mut consent, Some(0.5), size);
        assert_ne!(holding, idle, "a hold in progress must be visible");
        let below = (8 * W * 4) as usize;
        assert_eq!(
            &holding[below..],
            &idle[below..],
            "the indicator must not disturb the realm view underneath it"
        );

        // It is drawn above a consent card, so a prompt cannot hide the fact
        // that the human is mid-gesture on the off-switch.
        consent.show_for_test(prompt_fixture());
        let prompt_only = window_pixels(&scene, &mut consent, None, size);
        let prompt_and_hold = window_pixels(&scene, &mut consent, Some(0.9), size);
        assert_ne!(prompt_and_hold, prompt_only);

        // And every visible step of the fill re-uploads.
        let mut keys: Vec<TextureKey> = Vec::new();
        for step in 0..=10 {
            keys.push(TextureKey::current(
                size,
                &scene,
                &consent,
                Some(f64::from(step) / 10.0),
            ));
        }
        keys.dedup();
        assert!(
            keys.len() > 5,
            "the fill must re-upload as it grows, got {} distinct keys",
            keys.len()
        );
        assert_ne!(
            TextureKey::current(size, &scene, &consent, Some(1.0)),
            TextureKey::current(size, &scene, &consent, None),
            "the indicator disappearing must re-upload too"
        );
    }

    // ------------------------------------------------------------------
    // The dead-man wiring: the timer, its backstops, and the replay drain
    // ------------------------------------------------------------------
    //
    // These exist because a review mutated every one of these paths --
    // `arm_deadman_timer` early-returning, both backstops removed, the
    // replay drain removed, and the router's hook given a *different*
    // switch than the backend holds -- and the entire workspace suite
    // stayed green for all five. For a dead-man switch that is the one
    // class of regression nothing may hide: the failure is a human holding
    // the panic button in a live session and nothing happening.
    //
    // `NestedState` owns a GL context and a host window, so it cannot be
    // built on a display-free runner. The wiring is therefore reached
    // through [`DeadManHost`], which `NestedState` implements and this
    // fixture implements identically -- so these drive the *same* code
    // production does, not a re-typed copy of it.

    /// A minimal [`DeadManHost`]: what `NestedState` contributes to the
    /// dead-man wiring, with nothing that needs a display.
    struct TestHost {
        switch: Rc<RefCell<DeadManSwitch>>,
        timer_armed: bool,
        triggers: Vec<Trigger>,
    }

    impl TestHost {
        fn new(hold_ms: u64) -> Self {
            let config = DeadManConfig::default()
                .with_hold_ms(hold_ms)
                .expect("within range");
            Self {
                switch: Rc::new(RefCell::new(DeadManSwitch::new(config))),
                timer_armed: false,
                triggers: Vec::new(),
            }
        }
    }

    impl DeadManHost for TestHost {
        fn switch(&self) -> &Rc<RefCell<DeadManSwitch>> {
            &self.switch
        }
        fn timer_armed(&mut self) -> &mut bool {
            &mut self.timer_armed
        }
        fn on_trigger(&mut self, trigger: Trigger) {
            self.triggers.push(trigger);
        }
    }

    #[test]
    fn the_backends_timer_completes_a_held_chord_with_no_further_input() {
        // Design tension 2 through the machinery the nested backend really
        // runs. Smithay's winit backend filters key repeats, so a held key
        // produces exactly one press and then silence: if `arm_deadman_timer`
        // inserted no source -- or `deadman_tick` never called it -- this
        // loop would sit idle until the timeout and the chord would never
        // complete. Nothing here advances the switch by hand.
        // `EventLoop::try_new` opens epoll/eventfd descriptors, and
        // `capture`'s `fd_count_returns_to_baseline` asserts an exact
        // process-wide fd count. Take the same quiesce lock so the two never
        // run concurrently -- otherwise this test intermittently fails that
        // one, from a different module, for no reason a reader could find.
        let _fd = crate::capture::tests::fd_lock();
        let mut event_loop: EventLoop<'static, TestHost> = EventLoop::try_new().expect("loop");
        let handle = event_loop.handle();
        // A short hold so the test is quick; the mechanism is duration-blind.
        let mut host = TestHost::new(250);

        let pressed_at = Instant::now();
        host.switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), pressed_at);

        // The input turn's tick: this is what arms the timer in production.
        deadman_tick(&mut host, &handle, pressed_at);
        assert!(
            host.timer_armed,
            "the input turn did not arm a timer for an armed hold"
        );
        assert!(
            host.triggers.is_empty(),
            "the chord completed before its hold elapsed"
        );

        // Now run the loop with NO input source at all. Only the timer can
        // make anything happen.
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.triggers.is_empty() && Instant::now() < deadline {
            event_loop
                .dispatch(Some(Duration::from_millis(20)), &mut host)
                .expect("dispatch");
        }

        assert_eq!(
            host.triggers.len(),
            1,
            "the backend's timer never completed the chord: the off-switch is a no-op in \
             nested mode"
        );
        assert!(host.triggers[0].held >= Duration::from_millis(250));
        // The hold is over, so the source dropped itself and the bookkeeping
        // says so -- otherwise the next hold would never arm a timer at all.
        assert!(
            !host.timer_armed,
            "the timer did not clear its armed flag when the hold ended"
        );
        assert_eq!(host.switch.borrow().deadline(), None);
    }

    #[test]
    fn an_early_timer_wakeup_reschedules_instead_of_dropping() {
        // calloop is free to fire a timer marginally early. A callback that
        // dropped on a not-yet-due check would leave the switch with no timer
        // at all until the human pressed something else -- which, for a held
        // key that produces no further events, means never.
        // `EventLoop::try_new` opens epoll/eventfd descriptors, and
        // `capture`'s `fd_count_returns_to_baseline` asserts an exact
        // process-wide fd count. Take the same quiesce lock so the two never
        // run concurrently -- otherwise this test intermittently fails that
        // one, from a different module, for no reason a reader could find.
        let _fd = crate::capture::tests::fd_lock();
        let mut event_loop: EventLoop<'static, TestHost> = EventLoop::try_new().expect("loop");
        let handle = event_loop.handle();
        let mut host = TestHost::new(300);

        // Arm the timer against a hold that started 250 ms in the FUTURE, so
        // every wakeup for the next half-second is "early".
        let future = Instant::now() + Duration::from_millis(250);
        host.switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), future);
        arm_deadman_timer(&mut host, &handle);
        assert!(host.timer_armed);

        // Dispatch across the early window. The timer must survive it.
        let stop = Instant::now() + Duration::from_millis(200);
        while Instant::now() < stop {
            event_loop
                .dispatch(Some(Duration::from_millis(20)), &mut host)
                .expect("dispatch");
        }
        assert!(
            host.triggers.is_empty(),
            "the chord fired before its hold elapsed"
        );
        assert!(
            host.timer_armed,
            "an early wakeup dropped the timer instead of rescheduling it"
        );

        // And it still fires when the hold genuinely elapses.
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.triggers.is_empty() && Instant::now() < deadline {
            event_loop
                .dispatch(Some(Duration::from_millis(20)), &mut host)
                .expect("dispatch");
        }
        assert_eq!(host.triggers.len(), 1, "the rescheduled timer never fired");
    }

    #[test]
    fn arming_twice_inserts_one_timer_source() {
        // The `deadman_timer_armed` bookkeeping. `deadman_tick` runs on every
        // input turn and every frame, so without it a held key would insert a
        // fresh calloop source per event -- an agent flooding input would
        // grow the loop without bound.
        // `EventLoop::try_new` opens epoll/eventfd descriptors, and
        // `capture`'s `fd_count_returns_to_baseline` asserts an exact
        // process-wide fd count. Take the same quiesce lock so the two never
        // run concurrently -- otherwise this test intermittently fails that
        // one, from a different module, for no reason a reader could find.
        let _fd = crate::capture::tests::fd_lock();
        let event_loop: EventLoop<'static, TestHost> = EventLoop::try_new().expect("loop");
        let handle = event_loop.handle();
        let mut host = TestHost::new(1000);
        host.switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), Instant::now());

        arm_deadman_timer(&mut host, &handle);
        assert!(host.timer_armed);
        for _ in 0..50 {
            arm_deadman_timer(&mut host, &handle);
        }
        // One source: the second and later calls returned on the flag. If
        // they had not, the loop would now hold 51 timers for one hold.
        assert!(host.timer_armed);
    }

    #[test]
    fn a_disarmed_hold_arms_no_timer() {
        // What `handle_focus` relies on: after `forget_hold` there is no
        // deadline, so nothing is armed and nothing can fire.
        // `EventLoop::try_new` opens epoll/eventfd descriptors, and
        // `capture`'s `fd_count_returns_to_baseline` asserts an exact
        // process-wide fd count. Take the same quiesce lock so the two never
        // run concurrently -- otherwise this test intermittently fails that
        // one, from a different module, for no reason a reader could find.
        let _fd = crate::capture::tests::fd_lock();
        let event_loop: EventLoop<'static, TestHost> = EventLoop::try_new().expect("loop");
        let handle = event_loop.handle();
        let mut host = TestHost::new(250);
        let t0 = Instant::now();
        host.switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), t0);
        host.switch.borrow_mut().forget_hold();

        deadman_tick(&mut host, &handle, t0 + Duration::from_secs(5));
        assert!(!host.timer_armed, "a forgotten hold armed a timer anyway");
        assert!(
            host.triggers.is_empty(),
            "a forgotten hold completed the chord"
        );
    }

    #[test]
    fn a_tap_is_replayed_to_the_app_by_the_routing_turn() {
        // The drain, which is what gives the confined app its Escape key
        // back. Removing it is invisible in every other test and costs
        // Firefox the Escape key permanently -- every tap swallowed, none
        // ever replayed.
        let deadman = Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default())));
        let now = Rc::new(Cell::new(Instant::now()));
        let mut router = input::InputRouter::new(DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            input::NoopHook,
        ));
        let view = (640u32, 480u32);
        let surface = Some(view);
        let mut delivered: Vec<input::SeatDeliveryKind> = Vec::new();

        // The press alone reaches nobody: it is withheld pending the
        // tap-or-chord question.
        route_turn(
            &mut router,
            &deadman,
            [crate::input::tests::chord_press()],
            view,
            surface,
            &mut |d| delivered.push(d.kind().clone()),
        );
        assert!(delivered.is_empty(), "a withheld press reached the app");

        // The release classifies it as a tap, and the SAME turn drains and
        // re-routes the pair: press first, then release.
        now.set(now.get() + Duration::from_millis(80));
        deadman
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_release(), now.get());
        route_turn(
            &mut router,
            &deadman,
            [crate::input::tests::chord_release()],
            view,
            surface,
            &mut |d| delivered.push(d.kind().clone()),
        );

        assert_eq!(
            delivered.len(),
            2,
            "the tap was not replayed to the app: {delivered:?}"
        );
        use vitrin_protocol::generated::vitrin_shim_seat::KeyState;
        assert!(
            matches!(
                delivered[0],
                input::SeatDeliveryKind::Key {
                    keysym: 0xff1b,
                    state: KeyState::Pressed,
                }
            ),
            "the replay must start with the withheld press: {delivered:?}"
        );
        assert!(
            matches!(
                delivered[1],
                input::SeatDeliveryKind::Key {
                    keysym: 0xff1b,
                    state: KeyState::Released,
                }
            ),
            "a press with no release latches Escape in the app: {delivered:?}"
        );
    }

    #[test]
    fn the_routers_hook_and_the_backends_handle_are_one_switch() {
        // The wiring itself. `run` builds the router's `DeadManHook` and the
        // backend's own handle from one `Rc::clone`; a refactor that handed
        // the hook a *different* switch would leave the chord watched by a
        // switch nothing ever ticks, and the timer armed on a switch nothing
        // ever presses. Both halves look fine in isolation.
        //
        // Asserted by observing through the router and reading through the
        // handle, which is only possible if they are the same cell.
        let deadman = Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default())));
        let now = Rc::new(Cell::new(Instant::now()));
        let mut router = input::InputRouter::new(DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            input::NoopHook,
        ));
        let view = (640u32, 480u32);

        assert_eq!(deadman.borrow().deadline(), None);
        let _ = router.route(crate::input::tests::chord_press(), view, Some(view));
        assert_eq!(
            deadman.borrow().deadline(),
            Some(now.get() + crate::deadman::DEFAULT_HOLD),
            "the router's hook and the backend's handle are not the same switch"
        );
    }
}
