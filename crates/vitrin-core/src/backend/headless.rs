//! Headless backend: a fixed-size virtual output composited entirely in
//! software — no display, no DRM/KMS, no EGL, no GPU (PRD Doc 2 §9, "headless
//! mode = one virtual framebuffer per realm"). This is the path CI runs: the
//! plan makes pixman + shm mandatory on the GPU-less runners and keeps nested
//! mode off the CI critical path (01-phase-1-mvp.md §6 D3 / §7 R1).
//!
//! Where the nested backend ([`super::winit`]) is EGL/GLES-bound by
//! construction, this backend renders with Smithay's [`PixmanRenderer`], a CPU
//! rasterizer that writes into an ordinary memory image. That image is
//! *retained* for the life of the process so an internal capture can read it
//! back byte-for-byte — the P1.3.2 golden here, and the protocol-facing
//! capture service in P1.3.6.
//!
//! Two differences from the GL path are load-bearing and easy to get wrong:
//!
//! - **No vertical flip.** A GL framebuffer is bottom-up, so the nested
//!   backend renders with [`Transform::Flipped180`]. A pixman image is a
//!   top-down CPU buffer, so we render with [`Transform::Normal`]; the pattern
//!   lands upright with no compensating flip, which the golden's corner
//!   markers assert.
//! - **Tightly packed readback.** For a 32-bpp format pixman's stride equals
//!   `width * 4`, so the mapped framebuffer slice is already exactly
//!   `width * height * 4` bytes — the tightly packed RGBA the capture wants,
//!   with no per-row padding to strip.

use std::error::Error;

use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopSignal};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Frame, ImportMem, Offscreen, Renderer};
use smithay::reexports::pixman::Image;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};
use tracing::info;

use crate::test_pattern;

/// Composite the virtual output once and read it back as tightly packed
/// RGBA8888 (rows top-down) — the same byte layout [`test_pattern::render`]
/// produces. This is the compositing seam the golden test drives directly,
/// with no event loop: create a software renderer, allocate an offscreen
/// memory framebuffer, blit the test pattern into it 1:1, then export the
/// composited pixels.
///
/// A zero or negative size has no pixels; it yields an empty buffer rather
/// than driving pixman with a degenerate image.
///
/// Compiled in every build so the readback path stays type-checked, but only
/// *called* from the golden test until the protocol capture service (P1.3.6)
/// wires it to the wire — hence the non-test dead-code allowance.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn render_once(size: Size<i32, Physical>) -> Result<Vec<u8>, Box<dyn Error>> {
    if size.w <= 0 || size.h <= 0 {
        return Ok(Vec::new());
    }

    let mut renderer = PixmanRenderer::new()?;
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
    let mut framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;

    composite(&mut renderer, &mut framebuffer, size)?;

    // Read the composited framebuffer back out of pixman: `copy_framebuffer`
    // copies (SRC) the bound image into a fresh mapping, which `map_texture`
    // then exposes as bytes. For a 32-bpp format the stride is `width * 4`, so
    // the slice is tightly packed and needs no de-striding.
    let target = renderer.bind(&mut framebuffer)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    let pixels = renderer.map_texture(&mapping)?;

    let expected = size.w as usize * size.h as usize * test_pattern::BYTES_PER_PIXEL;
    assert_eq!(
        pixels.len(),
        expected,
        "pixman 32-bpp readback must be tightly packed (stride == width * 4)"
    );
    Ok(pixels.to_vec())
}

/// Run the headless compositor: composite the virtual output once, then idle
/// until SIGINT/SIGTERM.
///
/// **Refresh-driving decision (the one this task settles): render once, then
/// idle.** P1.3.2 has no client surfaces and therefore no damage source, so
/// the composited pattern is static. A periodic timer (the nested backend's
/// [`FRAME_BUDGET`](super::winit) posture) would re-render byte-identical
/// pixels forever and burn CI CPU for nothing; a damage-driven loop
/// degenerates to exactly this single initial frame, because nothing ever
/// reports damage. So we composite once into the retained framebuffer and let
/// the event loop block on signals. [`HeadlessState::redraw`] is factored as a
/// re-entrant entry point so P1.3.3 can call it again when real client damage
/// arrives, without reworking this control flow.
pub fn run(size: (u32, u32)) -> Result<(), Box<dyn Error>> {
    let (width, height) = size;
    let mut event_loop: EventLoop<'static, HeadlessState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Signal source first (same posture as the nested backend): the process is
    // single-threaded, so the signal mask is installed before anything else
    // runs. SIGINT/SIGTERM stop the loop cleanly.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state: &mut HeadlessState| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.loop_signal.stop();
    })?;

    let physical_size: Size<i32, Physical> = (width as i32, height as i32).into();
    info!(
        width,
        height,
        "headless backend starting: virtual output, software pixman renderer (no EGL/DRM/GPU)"
    );

    let mut state = HeadlessState::new(physical_size, event_loop.get_signal())?;
    // Composite once so the framebuffer is capture-ready before we start
    // idling (see this function's doc comment for why exactly once).
    state.redraw()?;
    info!("virtual-output framebuffer composited and retained in memory (capture-ready)");

    event_loop.run(None, &mut state, |_| {})?;
    info!("event loop stopped, shutting down");
    Ok(())
}

/// Per-run state of the headless backend: the software renderer and the
/// virtual output's framebuffer image, retained so it can be captured.
///
/// Scene composition of real client surfaces (P1.3.3) hangs off this struct
/// exactly as it does off the nested backend's state; [`redraw`](Self::redraw)
/// is where a real frame will be assembled from client surfaces.
struct HeadlessState {
    renderer: PixmanRenderer,
    /// The virtual output's framebuffer. Retained across the process lifetime
    /// (PRD Doc 2 §9) so an internal capture reads composited pixels, not a
    /// freshly cleared buffer.
    framebuffer: Image<'static, 'static>,
    size: Size<i32, Physical>,
    loop_signal: LoopSignal,
}

impl HeadlessState {
    fn new(size: Size<i32, Physical>, loop_signal: LoopSignal) -> Result<Self, Box<dyn Error>> {
        let mut renderer = PixmanRenderer::new()?;
        // `.max(0)` is defensive only: `run` always passes a positive size
        // (the CLI parser rejects zero/negative), but a degenerate size must
        // never wrap to a huge allocation on the `i32 -> usize` cast.
        let buffer_size: Size<i32, Buffer> = (size.w.max(0), size.h.max(0)).into();
        let framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        Ok(Self {
            renderer,
            framebuffer,
            size,
            loop_signal,
        })
    }

    /// (Re)composite the virtual output into the retained framebuffer. The
    /// single redraw entry point: called once from [`run`] today, and again
    /// from client damage in P1.3.3.
    fn redraw(&mut self) -> Result<(), Box<dyn Error>> {
        composite(&mut self.renderer, &mut self.framebuffer, self.size)
    }
}

/// Blit the test pattern into `framebuffer` at `size`, 1:1 and upright.
///
/// Shared by [`render_once`] (which then reads the framebuffer back) and
/// [`HeadlessState::redraw`] (which fills the retained framebuffer in place),
/// so the compositing step has one definition and one behavior.
fn composite(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    size: Size<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if size.w <= 0 || size.h <= 0 {
        return Ok(());
    }
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();

    // Upload the pattern exactly as the nested backend does: tightly packed
    // RGBA8888 imported as DRM `ABGR8888` (bytes R,G,B,A on little-endian).
    let pixels = test_pattern::render(size.w as u32, size.h as u32);
    let texture = renderer.import_memory(&pixels, Fourcc::Abgr8888, buffer_size, false)?;

    let full = Rectangle::from_size(size);
    let mut target = renderer.bind(framebuffer)?;
    // `Transform::Normal`, NOT `Flipped180`: a pixman image is a top-down CPU
    // buffer, so there is no GL bottom-up convention to undo. The pattern is
    // fully opaque and covers the whole output, so blitting it 1:1 (full
    // texture -> full framebuffer, no scaling) leaves the framebuffer a
    // byte-exact identity of the pattern — no separate clear is needed, and
    // the P1.3.2 golden asserts exactly that.
    let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
    frame.render_texture_from_to(
        &texture,
        Rectangle::from_size(Size::<f64, Buffer>::from((size.w as f64, size.h as f64))),
        full,
        &[full],
        &[],
        Transform::Normal,
        1.0,
    )?;
    // Pixman renders synchronously on the CPU, so the returned sync point is
    // already signaled; there is nothing to await.
    let _sync = frame.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_once;
    use crate::test_pattern;
    use smithay::utils::{Physical, Size};

    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 800;

    /// Composite the 1280x800 virtual output and return the captured RGBA8888.
    fn capture() -> Vec<u8> {
        let size: Size<i32, Physical> = (WIDTH as i32, HEIGHT as i32).into();
        render_once(size).expect("headless render_once must succeed under pixman")
    }

    /// The RGBA quadruple at `(x, y)` in a tightly packed WIDTH*HEIGHT buffer.
    fn pixel(buf: &[u8], x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * WIDTH as usize + x as usize) * test_pattern::BYTES_PER_PIXEL;
        buf[offset..offset + test_pattern::BYTES_PER_PIXEL]
            .try_into()
            .unwrap()
    }

    /// The exact-match golden (P1.3.2 acceptance). The golden is the
    /// deterministic *synthetic pattern* itself — [`test_pattern::render`],
    /// generated in-process, not a committed image file — so the pixman
    /// compositor must be a byte-exact identity for the (opaque) pattern: what
    /// we import is what we read back. This is the core guarantee that the
    /// software path moves no pixels of its own. Serializing a capture to PNG
    /// is the SDK's job (P1.8.2); the core keeps no image codec, even in tests
    /// (plan risk R7).
    #[test]
    fn capture_is_identity_of_test_pattern() {
        assert_eq!(
            capture(),
            test_pattern::render(WIDTH, HEIGHT),
            "pixman compositing altered the pattern"
        );
    }

    /// The capture is upright: the four distinct corner markers land in their
    /// named corners. Checked against the capture directly (not the pattern),
    /// so it catches an orientation bug the identity test alone could not.
    #[test]
    fn capture_is_upright() {
        let cap = capture();
        assert_eq!(pixel(&cap, 0, 0), test_pattern::MARKER_TOP_LEFT, "top-left");
        assert_eq!(
            pixel(&cap, WIDTH - 1, 0),
            test_pattern::MARKER_TOP_RIGHT,
            "top-right"
        );
        assert_eq!(
            pixel(&cap, 0, HEIGHT - 1),
            test_pattern::MARKER_BOTTOM_LEFT,
            "bottom-left"
        );
        assert_eq!(
            pixel(&cap, WIDTH - 1, HEIGHT - 1),
            test_pattern::MARKER_BOTTOM_RIGHT,
            "bottom-right"
        );
    }
}
