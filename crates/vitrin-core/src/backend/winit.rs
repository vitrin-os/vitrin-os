//! Nested backend: the core runs as a client of the host compositor
//! (GNOME, Hyprland, …), presenting exactly one host window — the gamescope
//! nested-session pattern (PRD Doc 2 §4/§17). Rendering stays deliberately
//! trivial per plan risk R1: one window, one full-window texture blit of the
//! composed realm view ([`Scene::compose`] — the same bytes the headless
//! backend retains for capture, P1.3.3).
//!
//! The winit backend is EGL/GLES-bound by construction, so this path always
//! renders with [`GlesRenderer`]. The pixman software path (mandatory for
//! GPU-less CI) arrives with the headless backend in P1.3.2, where no host
//! GL surface exists.

use std::error::Error;
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

/// The composed realm view ([`Scene::compose`]) uploaded as a GLES texture,
/// remembered together with the window size and scene generation it was
/// composed for, so resizes and scene commits re-upload it.
struct SceneTexture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    generation: u64,
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
    /// The input router (P1.3.7): host input tagged `physical` at intake
    /// flows through it toward the realm's shim seat. Carries the MVP
    /// no-op preemption hook, where the P1.7.2 consent grab and P1.7.3
    /// revocation watcher later attach.
    router: input::InputRouter<input::NoopHook>,
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
pub fn run() -> Result<(), Box<dyn Error>> {
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

    let mut state = NestedState {
        backend,
        scene: Scene::new(),
        router: input::InputRouter::new(input::NoopHook),
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
        for tagged in input::intake_physical(event, (size.w, size.h)) {
            if let Some(delivery) = self.router.route(tagged, view, self.scene.surface_size()) {
                // P1.5.2 hands this to ShimServer::deliver_seat_event on
                // the realm's live connection.
                trace!(
                    origin = ?delivery.origin(),
                    "routed input dropped: no shim connection yet (P1.5.2)"
                );
            }
        }
    }

    /// Draw one frame. Rendering failure is fatal to the skeleton: log it,
    /// record it for [`run`] to propagate (non-zero exit), and stop the
    /// loop rather than spinning on a broken GL context.
    fn redraw(&mut self) {
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

        // Re-upload the view when the window size or the scene content
        // changed: the same `Scene::compose` bytes the headless backend
        // retains for capture, presented here as a full-window texture (the
        // single shared composition implementation, P1.3.3).
        let generation = self.scene.generation();
        if self.view.as_ref().map(|v| (v.size, v.generation)) != Some((size, generation)) {
            let pixels = self.scene.compose(size.w as u32, size.h as u32);
            let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
            let texture = self.backend.renderer().import_memory(
                &pixels,
                Fourcc::Abgr8888,
                buffer_size,
                false,
            )?;
            self.view = Some(SceneTexture {
                texture,
                size,
                generation,
            });
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
