//! Nested backend: the core runs as a client of the host compositor
//! (GNOME, Hyprland, …), presenting exactly one host window — the gamescope
//! nested-session pattern (PRD Doc 2 §4/§17). Rendering stays deliberately
//! trivial per plan risk R1: one window, one full-window texture blit of the
//! composed human-visible output ([`super::compose_human_visible`] — the
//! realm view of [`Scene::compose`], the same bytes the headless backend
//! retains for capture, P1.3.3, with the consent overlay on top, P1.7.1).
//!
//! The host window IS the human's display here, so it presents the
//! human-visible side of the output stage and there is no second window,
//! no second surface, and no compositing anywhere else (decision D4). The
//! consent overlay reaching this texture is therefore exactly as far as it
//! goes: no capture path reads a GL texture (see [`super`] and
//! [`crate::consent`] for the fork).
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
use crate::consent::ConsentSurface;
use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch};
use crate::input;
use crate::scene::Scene;

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

/// Per-run state of the nested backend: the winit window + GLES renderer
/// pair, the realm's [`Scene`], and the currently uploaded view texture.
struct NestedState {
    backend: WinitGraphicsBackend<GlesRenderer>,
    /// The realm's scene (P1.3.3) — the same composition implementation the
    /// headless backend retains for capture; this backend presents its
    /// output 1:1 in the host window. The shim-facing protocol server
    /// (P1.3.4) commits into it and requests a redraw.
    scene: Scene,
    /// The consent surface (P1.7.1): the prompt composited above the realm
    /// view in the host window. Always present; empty until something
    /// raises a petition through [`ConsentGrab::raise`], which needs the
    /// petition registry the M1.1 listener wiring constructs.
    consent: ConsentSurface,
    /// The input router (P1.3.7): host input tagged `physical` at intake
    /// flows through it toward the realm's shim seat. Its preemption hook is
    /// P1.7.2's consent grab wrapping P1.7.3's dead-man watcher — the
    /// stacking the hook point was designed for, with no restructuring of
    /// the router, and the exact shape [`crate::input`]'s docs predicted
    /// (`ConsentGate<NoopHook>` became `ConsentGate<DeadManHook>`).
    ///
    /// Order matters and is deliberate: the watcher is **innermost**, so its
    /// detection half rides `observe`, which `ConsentGate` forwards
    /// unconditionally even for events it swallows whole. The off-switch
    /// therefore keeps working while a consent prompt owns the screen.
    router: input::InputRouter<ConsentGate<DeadManHook<input::NoopHook>>>,
    /// The consent input grab (P1.7.2), shared with the router's gate:
    /// while a prompt is up it owns physical input, and a click on one of
    /// the card's buttons becomes a petition decision here.
    ///
    /// Live but idle. Nothing raises a prompt at runtime yet — that needs
    /// the petition registry the M1.1 listener wiring constructs (issue
    /// #77) — so this grab consumes nothing and produces no decisions in a
    /// running `vitrind`. It is carried anyway, rather than attached later,
    /// because the alternative is a window in which a prompt could be shown
    /// with no grab behind it.
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
    /// Live and really running — the chord is watched, the timer is armed,
    /// and a completed hold fires. What a completed hold cannot yet *do* is
    /// revoke anything: nothing constructs a grant table or a petition
    /// registry until the M1.1 listener wiring (issue #77), so
    /// [`NestedState::deadman_tick`] logs the trigger and drops it. See
    /// [`crate::deadman`]'s "what is mechanism-only" section, including why
    /// the flight recorder is not threaded in here to close the gap early.
    deadman: Rc<RefCell<DeadManSwitch>>,
    /// Whether a one-shot timer is outstanding for the armed hold, so an
    /// arming keypress does not insert a fresh calloop source per event.
    deadman_timer_armed: bool,
    view: Option<SceneTexture>,
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
pub fn run(dead_man: DeadManConfig) -> Result<(), Box<dyn Error>> {
    let mut event_loop: EventLoop<'static, NestedState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Signal source first (single-threaded process, so the mask is set
    // before anything else runs): SIGINT/SIGTERM stop the loop cleanly.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.loop_signal.stop();
    })?;

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
            state.view = None;
            state.backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => {
            info!("host window close requested");
            state.loop_signal.stop();
        }
        // P1.3.7 input intake: nested-mode host events ARE the human
        // principal's input, origin-tagged `physical` at this single point
        // of entry (B2) — see `crate::input`.
        WinitEvent::Input(event) => state.handle_input(&event),
        WinitEvent::Focus(_) => {}
    })?;

    let grab = Rc::new(RefCell::new(ConsentGrab::new()));
    let now = Rc::new(Cell::new(Instant::now()));
    let deadman = Rc::new(RefCell::new(DeadManSwitch::new(dead_man)));
    info!(
        chord = dead_man.chord.name(),
        hold_ms = dead_man.hold.as_millis(),
        "dead-man switch armed: holding this key revokes every grant in the session"
    );
    let mut state = NestedState {
        backend,
        scene: Scene::new(),
        consent: ConsentSurface::new(),
        router: input::InputRouter::new(ConsentGate::new(
            Rc::clone(&grab),
            Rc::clone(&now),
            DeadManHook::new(Rc::clone(&deadman), Rc::clone(&now), input::NoopHook),
        )),
        grab,
        now,
        deadman,
        deadman_timer_armed: false,
        view: None,
        loop_signal: event_loop.get_signal(),
        loop_handle: event_loop.handle(),
        fatal: None,
    };

    // Kick off the redraw cycle; each completed frame requests the next,
    // so presentation is paced by the host compositor (60 Hz on a 60 Hz
    // host — winit redraws on Wayland are frame-callback driven), with
    // FRAME_BUDGET as the floor when the host doesn't throttle swaps.
    state.backend.window().request_redraw();

    event_loop.run(None, &mut state, |_| {})?;
    if let Some(err) = state.fatal.take() {
        return Err(err);
    }
    info!("event loop stopped, shutting down");
    Ok(())
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
        let size = self.backend.window_size();
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
        for tagged in input::intake_physical(event, (size.w, size.h)) {
            self.route_one(tagged, view);
        }
        // A tap on the chord key is handed back here to be routed as if it
        // had arrived normally: the dead-man gate withholds the chord's
        // press until it knows whether the human is tapping or holding, and
        // a tap owes the app the press *and* its release, in that order
        // (see `crate::deadman`). Draining after the loop, never before it,
        // is what keeps this a drain rather than a second policy hook.
        //
        // Re-routing rather than injecting means every downstream policy
        // still applies -- a prompt that went up between the press and the
        // release consumes the replayed press exactly as it would a fresh
        // one, with no carve-out anywhere.
        // The drain is a separate statement from the loop so the switch's
        // borrow ends before routing re-enters the hook that holds it.
        let replay = self.deadman.borrow_mut().take_replay();
        for replayed in replay {
            self.route_one(replayed, view);
        }
        // Backstop 2 of 3 for the elapse check (`crate::deadman`): the
        // switch is already being asked about this turn's events, so ask it
        // about the clock too.
        self.deadman_tick();
    }

    /// Route one tagged event and dispose of the delivery.
    fn route_one(&mut self, tagged: input::SeatInput, view: (u32, u32)) {
        if let Some(delivery) = self.router.route(tagged, view, self.scene.surface_size()) {
            // P1.5.2 hands this to ShimServer::deliver_seat_event on
            // the realm's live connection.
            trace!(
                origin = ?delivery.origin(),
                "routed input dropped: no shim connection yet (P1.5.2)"
            );
        }
    }

    /// Complete the dead-man chord if its hold has elapsed, apply whatever
    /// that produced, and keep the timer armed while a hold is in progress.
    ///
    /// Called from three places on purpose ([`crate::deadman`], design
    /// tension 2): the timer below, every input dispatch turn, and every
    /// frame. [`DeadManSwitch::fire_if_due`] is idempotent and
    /// level-triggered, so calling it more often only shortens latency --
    /// and a timer that never fires costs at most one frame rather than the
    /// whole switch.
    fn deadman_tick(&mut self) {
        let trigger = {
            let mut switch = self.deadman.borrow_mut();
            switch.fire_if_due(Instant::now());
            switch.take_trigger()
        };
        if let Some(trigger) = trigger {
            // Honest gap, stated where it happens: `crate::deadman::apply`
            // wants the session's grant table and petition registry, and
            // nothing constructs either until issue #77. The switch really
            // completed -- it is `warn!`-logged by `fire_if_due` itself --
            // there is simply no authority in this process to revoke.
            error!(
                chord = trigger.chord,
                held_ms = trigger.held.as_millis(),
                "dead-man chord completed, but this build has no grant table to revoke: the \
                 M1.1 listener wiring (issue #77) is what constructs one. NOTHING WAS REVOKED."
            );
        }
        self.arm_deadman_timer();
        // Deliberately no `request_redraw` here. The redraw chain is already
        // self-sustaining (`schedule_next_frame`), so the indicator animates
        // and disappears at the host's frame cadence for free — and adding a
        // request on a path the frame loop itself calls would chain a redraw
        // per frame *outside* `FRAME_BUDGET`, turning a held key into a
        // busy-spin.
    }

    /// Arm a one-shot timer at the hold's deadline, if one is armed and no
    /// timer is outstanding.
    ///
    /// The callback reschedules itself rather than dropping when the hold is
    /// still pending, which closes the gap an early or coarse wakeup would
    /// otherwise leave: calloop is free to fire a timer marginally early,
    /// and a `Drop` on a not-yet-due check would leave the switch with no
    /// timer at all until the human pressed something else.
    fn arm_deadman_timer(&mut self) {
        if self.deadman_timer_armed {
            return;
        }
        let Some(deadline) = self.deadman.borrow().deadline() else {
            return;
        };
        let armed = self.loop_handle.insert_source(
            Timer::from_deadline(deadline),
            |_deadline, _, state: &mut NestedState| {
                let trigger = {
                    let mut switch = state.deadman.borrow_mut();
                    switch.fire_if_due(Instant::now());
                    switch.take_trigger()
                };
                if let Some(trigger) = trigger {
                    error!(
                        chord = trigger.chord,
                        held_ms = trigger.held.as_millis(),
                        "dead-man chord completed, but this build has no grant table to revoke: \
                         the M1.1 listener wiring (issue #77) is what constructs one. NOTHING \
                         WAS REVOKED."
                    );
                }
                match state.deadman.borrow().deadline() {
                    // Still held and not yet due: this wakeup was early.
                    Some(next) => TimeoutAction::ToInstant(next),
                    None => {
                        state.deadman_timer_armed = false;
                        TimeoutAction::Drop
                    }
                }
            },
        );
        match armed {
            Ok(_token) => self.deadman_timer_armed = true,
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

    /// Draw one frame. Rendering failure is fatal to the skeleton: log it,
    /// record it for [`run`] to propagate (non-zero exit), and stop the
    /// loop rather than spinning on a broken GL context.
    fn redraw(&mut self) {
        // Backstop 3 of 3 for the elapse check (`crate::deadman`), and the
        // one that survives a lost timer: at the host's frame cadence this
        // bounds the switch's latency to ~16.7 ms with no timer at all.
        // Placed before `try_redraw`, which returns early for a zero-sized
        // (minimized) window, so a minimized window does not silently drop
        // the backstop with it. Guarded on an armed hold so an idle window
        // pays nothing per frame.
        if self.deadman.borrow().deadline().is_some() {
            self.deadman_tick();
        }
        if let Err(err) = self.try_redraw() {
            error!("render failed, shutting down: {err}");
            self.fatal = Some(err);
            self.loop_signal.stop();
        }
    }

    fn try_redraw(&mut self) -> Result<(), Box<dyn Error>> {
        let frame_start = Instant::now();
        let size = self.backend.window_size();
        if size.w <= 0 || size.h <= 0 {
            // Zero-sized (e.g. minimized) window: skip, resize will redraw.
            return Ok(());
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
        let key = TextureKey::current(size, &self.scene, &self.consent, hold);
        if self.view.as_ref().map(|v| v.key) != Some(key) {
            let pixels = window_pixels(&self.scene, &mut self.consent, hold, size);
            let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
            let texture = self.backend.renderer().import_memory(
                &pixels,
                Fourcc::Abgr8888,
                buffer_size,
                false,
            )?;
            self.view = Some(SceneTexture { texture, key });
        }

        let full_window = Rectangle::from_size(size);
        {
            // Field-level borrows: `bind` holds `self.backend` mutably while
            // the view texture is read from `self.view`.
            let view = self.view.as_ref().expect("view composed above");
            let (renderer, mut framebuffer) = self.backend.bind()?;
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
        self.backend.submit(None)?;
        trace!(?size, "frame submitted");
        self.schedule_next_frame(frame_start)?;
        Ok(())
    }

    /// Chain the next redraw. If the swap blocked for at least
    /// [`FRAME_BUDGET`] (effective vsync), request it immediately;
    /// otherwise arm a one-shot timer for the budget's remainder so the
    /// render loop stays capped near 60 Hz instead of busy-spinning on
    /// hosts without swap throttling.
    fn schedule_next_frame(&mut self, frame_start: Instant) -> Result<(), Box<dyn Error>> {
        let elapsed = frame_start.elapsed();
        if elapsed >= FRAME_BUDGET {
            self.backend.window().request_redraw();
        } else {
            let timer = Timer::from_duration(FRAME_BUDGET - elapsed);
            self.loop_handle
                .insert_source(timer, |_deadline, _, state| {
                    state.backend.window().request_redraw();
                    TimeoutAction::Drop
                })
                .map_err(|err| err.error)?;
        }
        Ok(())
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
        let mut consent = ConsentSurface::new();

        // No prompt: the window shows the realm view, byte for byte.
        let plain = window_pixels(&scene, &mut consent, None, size);
        assert_eq!(
            plain,
            scene.compose(W as u32, H as u32),
            "with no prompt up the host window is the realm view unchanged"
        );

        // Prompt up: the window shows the shared human-visible composition.
        consent.show_for_test(prompt_fixture());
        let with_prompt = window_pixels(&scene, &mut consent, None, size);
        assert_ne!(
            with_prompt, plain,
            "the prompt must change what is uploaded"
        );
        let mut expected = ConsentSurface::new();
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
        let mut consent = ConsentSurface::new();

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
        let mut consent = ConsentSurface::new();

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
}
