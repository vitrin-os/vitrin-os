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

use crate::consent::ConsentSurface;
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
}

impl TextureKey {
    /// The key describing what a texture composed *right now* would contain.
    fn current(size: Size<i32, Physical>, scene: &Scene, consent: &ConsentSurface) -> Self {
        Self {
            size,
            scene_generation: scene.generation(),
            consent_generation: consent.generation(),
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
/// plus the consent prompt, if one is up.
///
/// Split out of [`NestedState::try_redraw`] so it can be tested without a
/// display. Presenting those pixels needs an EGL/GLES context and a host
/// window, so CI cannot drive `try_redraw` end to end; what it *can* pin is
/// the two decisions that function makes — which pixels to upload (here) and
/// when to re-upload them ([`TextureKey`]) — leaving only the GL submit
/// itself uncovered. Before this split nothing constrained nested-mode
/// presentation at all: deleting the overlay from the upload left the whole
/// suite green.
fn window_pixels(
    scene: &Scene,
    consent: &mut ConsentSurface,
    size: Size<i32, Physical>,
) -> Vec<u8> {
    super::compose_human_visible(scene, consent, size.w.max(0) as u32, size.h.max(0) as u32)
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
    /// view in the host window. Always present, empty until P1.7.2 puts a
    /// petition up.
    consent: ConsentSurface,
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
        consent: ConsentSurface::new(),
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

        // Re-upload when the window size, the scene content, or the consent
        // surface changed: the same shared composition both backends present
        // (P1.3.3), plus the prompt (P1.7.1), uploaded here as a full-window
        // texture. Keying on both generations is what makes a prompt appear
        // and disappear at the host's very next frame instead of whenever the
        // scene happens to change next.
        let key = TextureKey::current(size, &self.scene, &self.consent);
        if self.view.as_ref().map(|v| v.key) != Some(key) {
            let pixels = window_pixels(&self.scene, &mut self.consent, size);
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
        let plain = window_pixels(&scene, &mut consent, size);
        assert_eq!(
            plain,
            scene.compose(W as u32, H as u32),
            "with no prompt up the host window is the realm view unchanged"
        );

        // Prompt up: the window shows the shared human-visible composition.
        consent.show(prompt_fixture());
        let with_prompt = window_pixels(&scene, &mut consent, size);
        assert_ne!(
            with_prompt, plain,
            "the prompt must change what is uploaded"
        );
        let mut expected = ConsentSurface::new();
        expected.show(prompt_fixture());
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

        let base = TextureKey::current(size, &scene, &consent);
        assert_eq!(
            base,
            TextureKey::current(size, &scene, &consent),
            "an unchanged output must not force a re-upload"
        );

        // A prompt going up, and coming back down, both re-upload.
        consent.show(prompt_fixture());
        let shown = TextureKey::current(size, &scene, &consent);
        assert_ne!(base, shown, "a prompt appearing must re-upload");
        consent.dismiss();
        let dismissed = TextureKey::current(size, &scene, &consent);
        assert_ne!(shown, dismissed, "a prompt going away must re-upload");

        // The queue advancing to a different petition re-uploads too, so the
        // window cannot keep showing a decided petition's card.
        consent.show(prompt_fixture());
        let first = TextureKey::current(size, &scene, &consent);
        let mut next = prompt_fixture();
        next.principal =
            crate::identity::PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        consent.show(next);
        assert_ne!(
            first,
            TextureKey::current(size, &scene, &consent),
            "a different petition must re-upload"
        );

        // And the two pre-existing inputs still matter.
        let held = TextureKey::current(size, &scene, &consent);
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        assert_ne!(
            held,
            TextureKey::current(size, &scene, &consent),
            "a scene commit must re-upload"
        );
        assert_ne!(
            TextureKey::current(size, &scene, &consent),
            TextureKey::current(size_of(640, 480), &scene, &consent),
            "a resize must re-upload"
        );
    }
}
