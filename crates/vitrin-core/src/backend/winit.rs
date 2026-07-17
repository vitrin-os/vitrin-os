//! Nested backend: the core runs as a client of the host compositor
//! (GNOME, Hyprland, …), presenting exactly one host window — the gamescope
//! nested-session pattern (PRD Doc 2 §4/§17). Rendering stays deliberately
//! trivial per plan risk R1: one window, one full-window texture blit.
//!
//! The winit backend is EGL/GLES-bound by construction, so this path always
//! renders with [`GlesRenderer`]. The pixman software path (mandatory for
//! GPU-less CI) arrives with the headless backend in P1.3.2, where no host
//! GL surface exists.

use std::error::Error;

use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopSignal};
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::context::GlAttributes;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Color32F, Frame, ImportMem, Renderer};
use smithay::backend::winit::{self as winit_backend, WinitEvent, WinitGraphicsBackend};
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};
use tracing::{debug, error, info, trace};

use crate::test_pattern;

/// Initial logical window size; matches the planned headless default
/// (`--headless --size 1280x800`, P1.3.2) so nested and headless views of
/// the same content agree by default.
const INITIAL_SIZE: (f64, f64) = (1280.0, 800.0);

/// Background behind the test pattern; only visible if the blit fails.
const CLEAR_COLOR: Color32F = Color32F::new(0.06, 0.06, 0.08, 1.0);

/// The test pattern uploaded as a GLES texture, remembered together with
/// the window size it was generated for so resizes regenerate it.
struct PatternTexture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
}

/// Per-run state of the nested backend: the winit window + GLES renderer
/// pair and the current test-pattern texture.
///
/// Scene composition of real client surfaces replaces the test-pattern blit
/// in P1.3.3; this struct is where the realm view state will hang.
struct NestedState {
    backend: WinitGraphicsBackend<GlesRenderer>,
    pattern: Option<PatternTexture>,
    loop_signal: LoopSignal,
}

/// Run the nested compositor loop until the host window is closed or a
/// SIGINT/SIGTERM arrives.
pub fn run() -> Result<(), Box<dyn Error>> {
    let mut event_loop: EventLoop<'_, NestedState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Signal source first (single-threaded process, so the mask is set
    // before anything else runs): SIGINT/SIGTERM stop the loop cleanly.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.loop_signal.stop();
    })?;

    // vsync on (Smithay's default is off): each frame chains the next via
    // `request_redraw`, and the blocking swap is what paces that chain to
    // the host's refresh rate — without it the loop spins unthrottled on
    // hosts that don't coalesce redraw requests (X11). A real frame clock
    // replaces this once client surfaces need scheduling (P1.3.3).
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
            // Drop the pattern; the next redraw regenerates it 1:1 at the
            // new size (kept pixel-exact for the P1.3.2/P1.3.6 goldens).
            state.pattern = None;
            state.backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => {
            info!("host window close requested");
            state.loop_signal.stop();
        }
        // Input intake is P1.3.7: nested-mode host events become the human
        // principal's input, origin-tagged at this point of entry.
        WinitEvent::Input(_) => {}
        WinitEvent::Focus(_) => {}
    })?;

    let mut state = NestedState {
        backend,
        pattern: None,
        loop_signal: event_loop.get_signal(),
    };

    // Kick off the redraw cycle; each completed frame requests the next,
    // so presentation is paced by the host compositor (60 Hz on a 60 Hz
    // host — winit redraws on Wayland are frame-callback driven).
    state.backend.window().request_redraw();

    event_loop.run(None, &mut state, |_| {})?;
    info!("event loop stopped, shutting down");
    Ok(())
}

impl NestedState {
    /// Draw one frame. Rendering failure is fatal to the skeleton: log it
    /// and stop the loop rather than spinning on a broken GL context.
    fn redraw(&mut self) {
        if let Err(err) = self.try_redraw() {
            error!("render failed, shutting down: {err}");
            self.loop_signal.stop();
        }
    }

    fn try_redraw(&mut self) -> Result<(), Box<dyn Error>> {
        let size = self.backend.window_size();
        if size.w <= 0 || size.h <= 0 {
            // Zero-sized (e.g. minimized) window: skip, resize will redraw.
            return Ok(());
        }

        if self.pattern.as_ref().map(|p| p.size) != Some(size) {
            let pixels = test_pattern::render(size.w as u32, size.h as u32);
            let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
            let texture = self.backend.renderer().import_memory(
                &pixels,
                Fourcc::Abgr8888,
                buffer_size,
                false,
            )?;
            self.pattern = Some(PatternTexture { texture, size });
        }

        let full_window = Rectangle::from_size(size);
        {
            // Field-level borrows: `bind` holds `self.backend` mutably while
            // the pattern texture is read from `self.pattern`.
            let pattern = self.pattern.as_ref().expect("pattern generated above");
            let (renderer, mut framebuffer) = self.backend.bind()?;
            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
            frame.clear(CLEAR_COLOR, &[full_window])?;
            // Qualified call: GlesFrame has an inherent method of the same
            // name (extra custom-shader arguments) that would shadow the
            // renderer-agnostic trait method.
            Frame::render_texture_from_to(
                &mut frame,
                &pattern.texture,
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
        // Full-frame submit; damage tracking becomes worthwhile with real
        // client content (P1.3.3).
        self.backend.submit(None)?;
        trace!(?size, "frame submitted");
        self.backend.window().request_redraw();
        Ok(())
    }
}
