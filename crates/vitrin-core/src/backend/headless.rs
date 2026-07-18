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

use crate::scene::Scene;
use crate::test_pattern;

/// Composite an **empty scene** (the deterministic test-pattern background)
/// once and read it back as tightly packed RGBA8888 (rows top-down) — the
/// same byte layout [`test_pattern::render`] produces. This is the
/// compositing seam the golden test drives directly, with no event loop:
/// create a software renderer, allocate an offscreen memory framebuffer,
/// blit the composed view into it 1:1, then export the composited pixels.
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

    composite(&mut renderer, &mut framebuffer, size, &Scene::new())?;
    readback(&mut renderer, &mut framebuffer, size)
}

/// Read a composited framebuffer back as tightly packed RGBA8888, rows
/// top-down: `copy_framebuffer` copies (SRC) the bound image into a fresh
/// mapping, which `map_texture` then exposes as bytes. For a 32-bpp format
/// the stride is `width * 4`, so the slice is tightly packed and needs no
/// de-striding.
///
/// Shared by [`render_once`] and [`HeadlessState::latest_frame_rgba`] (the
/// capture service's pixel source, P1.3.6), so readback has one definition
/// and one byte layout.
fn readback(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
    let target = renderer.bind(framebuffer)?;
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
/// **Refresh-driving decision (settled in P1.3.2): render once, then idle.**
/// Until the shim-facing protocol server (P1.3.4) feeds the scene real
/// client commits there is no damage source, so the composed view is static.
/// A periodic timer (the nested backend's [`FRAME_BUDGET`](super::winit)
/// posture) would re-render byte-identical pixels forever and burn CI CPU
/// for nothing; a damage-driven loop degenerates to exactly this single
/// initial frame, because nothing ever reports damage. So we composite once
/// into the retained framebuffer and let the event loop block on signals.
/// [`HeadlessState::redraw`] is the re-entrant entry point P1.3.4 calls
/// again on client damage (a scene commit), without reworking this control
/// flow.
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

/// Per-run state of the headless backend: the software renderer, the realm's
/// [`Scene`], and the virtual output's framebuffer image, retained so it can
/// be captured.
struct HeadlessState {
    renderer: PixmanRenderer,
    /// The realm's scene (P1.3.3): the single-maximized client surface, or
    /// the deterministic background when none is committed. The shim-facing
    /// protocol server (P1.3.4) commits into it and calls
    /// [`redraw`](Self::redraw); the realm object (P1.5.1) hangs off it.
    scene: Scene,
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
            scene: Scene::new(),
            framebuffer,
            size,
            loop_signal,
        })
    }

    /// (Re)composite the realm view into the retained framebuffer. The
    /// single redraw entry point: called once from [`run`] today, and again
    /// on client damage (a scene commit) once P1.3.4 feeds the scene.
    fn redraw(&mut self) -> Result<(), Box<dyn Error>> {
        composite(
            &mut self.renderer,
            &mut self.framebuffer,
            self.size,
            &self.scene,
        )
    }

    /// The capture service's pixel source (P1.3.6): the **latest completed
    /// frame**, read back from the retained framebuffer as tightly packed
    /// RGBA8888 — a pure read that never triggers a composite. This is the
    /// capture-timing decision (see [`crate::capture`]'s module docs):
    /// capture observes what the compositor last finished, exactly the
    /// IDL's "most recently composited content as of when the server
    /// processes this request", keeping goldens deterministic and keeping
    /// render cost off the agent-request path.
    ///
    /// Compiled in every build so the path stays type-checked; called from
    /// the golden test today and wired to protocol dispatch when the
    /// enforcement chokepoint lands (P1.4.4, M1.1 integration).
    #[cfg_attr(not(test), allow(dead_code))]
    fn latest_frame_rgba(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        readback(&mut self.renderer, &mut self.framebuffer, self.size)
    }
}

/// Blit the composed realm view ([`Scene::compose`]) into `framebuffer` at
/// `size`, 1:1 and upright.
///
/// Shared by [`render_once`] (which then reads the framebuffer back) and
/// [`HeadlessState::redraw`] (which fills the retained framebuffer in place),
/// so the compositing step has one definition and one behavior. Composition
/// itself — layout, letterbox, background — happens in [`Scene::compose`],
/// the single implementation both backends present (P1.3.3); this function
/// only moves the composed bytes into the retained framebuffer.
fn composite(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    size: Size<i32, Physical>,
    scene: &Scene,
) -> Result<(), Box<dyn Error>> {
    if size.w <= 0 || size.h <= 0 {
        return Ok(());
    }
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();

    // Upload the composed view exactly as the nested backend does: tightly
    // packed RGBA8888 imported as DRM `ABGR8888` (bytes R,G,B,A on
    // little-endian).
    let pixels = scene.compose(size.w as u32, size.h as u32);
    let texture = renderer.import_memory(&pixels, Fourcc::Abgr8888, buffer_size, false)?;

    let full = Rectangle::from_size(size);
    let mut target = renderer.bind(framebuffer)?;
    // `Transform::Normal`, NOT `Flipped180`: a pixman image is a top-down CPU
    // buffer, so there is no GL bottom-up convention to undo. The composed
    // view is fully opaque (Scene::compose forces alpha) and covers the whole
    // output, so blitting it 1:1 (full texture -> full framebuffer, no
    // scaling) leaves the framebuffer a byte-exact identity of the composed
    // view — no separate clear is needed, and the P1.3.2/P1.3.3 goldens
    // assert exactly that.
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
    use super::{render_once, HeadlessState};
    use crate::test_pattern;
    use calloop::EventLoop;
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

    /// The capture service's actual pixel source: after `redraw`, the
    /// *retained* framebuffer reads back as the exact synthetic pattern —
    /// the latest-completed-frame seam the P1.3.6 capture path consumes,
    /// exercised on real [`HeadlessState`], not just the `render_once`
    /// shortcut. Reading again without recompositing yields the same
    /// bytes: capture is a pure read of completed output.
    ///
    /// Takes the fd-quiescence lock: the calloop `EventLoop` (needed only
    /// to mint a `LoopSignal`) opens fds, which must not race the capture
    /// module's `/proc/self/fd` baseline assertions.
    #[test]
    fn retained_framebuffer_is_the_capture_source() {
        let _fd = crate::capture::tests::fd_lock();
        let (w, h) = (640, 400);
        let size: Size<i32, Physical> = (w as i32, h as i32).into();
        let event_loop: EventLoop<'static, HeadlessState> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state =
            HeadlessState::new(size, event_loop.get_signal()).expect("headless state under pixman");
        state
            .redraw()
            .expect("composite into the retained framebuffer");

        let golden = test_pattern::render(w, h);
        assert_eq!(
            state.latest_frame_rgba().expect("readback"),
            golden,
            "retained framebuffer must hold the composited pattern"
        );
        assert_eq!(
            state.latest_frame_rgba().expect("second readback"),
            golden,
            "capture is a pure read: no recomposite, identical bytes"
        );
    }

    /// The P1.3.3 acceptance chain, end to end on the shared path: a test
    /// client's shm buffer (real memfd bytes) is committed to the scene,
    /// composed into the retained framebuffer, and served by the capture
    /// service — and the retained bytes equal [`Scene::compose`]'s output
    /// exactly. That last equality is the sharing proof: presentation is a
    /// byte-exact identity of the one shared composition implementation,
    /// and the nested backend uploads the same `Scene::compose` output as
    /// its window texture (same seam, same bytes; GL presentation itself
    /// needs a display, so CI proves the shared-seam half).
    #[test]
    fn committed_shm_buffer_reaches_retained_framebuffer_and_capture() {
        use std::fs::File;
        use std::io::Write;
        use std::os::unix::fs::FileExt;

        use rustix::fs::MemfdFlags;
        use vitrin_ipc::Connection;
        use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format};

        use crate::capture::{
            AutoApprove, CaptureIds, CaptureOutcome, CaptureService, RealmViewFrame,
        };
        use crate::scene::{tests::client_pixels, SurfaceContent, LETTERBOX_RGBA};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 320; // view (virtual output)
        const VH: u32 = 200;
        const SW: u32 = 160; // committed client buffer (smaller: letterboxed)
        const SH: u32 = 120;

        // The client buffer travels through a real shm-style memfd: written
        // by the "client", read back by the "server" — the byte path the
        // P1.3.4 shm copy-in will take.
        let pixels = client_pixels(SW, SH);
        let memfd = rustix::fs::memfd_create("vitrin-test-client-buffer", MemfdFlags::CLOEXEC)
            .expect("memfd_create");
        let mut file = File::from(memfd);
        file.write_all(&pixels).expect("write client buffer");
        let mut from_fd = vec![0u8; pixels.len()];
        file.read_exact_at(&mut from_fd, 0)
            .expect("read client buffer");
        drop(file);

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessState> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state =
            HeadlessState::new(size, event_loop.get_signal()).expect("headless state under pixman");
        state
            .scene
            .commit(SurfaceContent::from_rgba(from_fd, SW, SH).expect("well-formed content"));
        state.redraw().expect("composite the committed surface");

        // Sharing proof: the retained framebuffer (capture's pixel source)
        // is byte-for-byte the shared composition's output.
        let retained = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            retained,
            state.scene.compose(VW, VH),
            "retained framebuffer must be the shared Scene::compose output"
        );

        // The client's shm bytes appear in the view, centered 1:1, matte
        // around them (the letterbox decision).
        let (ox, oy) = (((VW - SW) / 2) as usize, ((VH - SH) / 2) as usize);
        for row in 0..SH as usize {
            let dst = ((oy + row) * VW as usize + ox) * test_pattern::BYTES_PER_PIXEL;
            let src = row * SW as usize * test_pattern::BYTES_PER_PIXEL;
            assert_eq!(
                &retained[dst..dst + SW as usize * test_pattern::BYTES_PER_PIXEL],
                &pixels[src..src + SW as usize * test_pattern::BYTES_PER_PIXEL],
                "client row {row} must appear unscaled in the view"
            );
        }
        assert_eq!(retained[..4], LETTERBOX_RGBA, "matte at the view corner");

        // The same buffer appears in a served capture: the capture service
        // reads the same retained frame and delivers its xrgb8888 form.
        let (mut server, mut client) = Connection::pair().expect("socketpair");
        let mut service = CaptureService::new(AutoApprove);
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba: &retained,
                    width: VW,
                    height: VH,
                },
                CaptureIds {
                    view_id: 7,
                    grant_id: 5,
                },
                &mut server,
            )
            .expect("serve capture");
        assert_eq!(outcome, CaptureOutcome::Delivered);
        let msg = client
            .recv_message()
            .expect("client receive")
            .expect("a frame must be waiting");
        let (_, frame) = FrameReady::decode(&msg.bytes, msg.fd).expect("frame_ready decodes");
        assert_eq!(frame.format, Format::Xrgb8888);
        assert_eq!((frame.width, frame.height), (VW, VH));
        let served_file = File::from(frame.fd);
        let mut served = vec![0u8; (frame.stride * frame.height) as usize];
        served_file
            .read_exact_at(&mut served, 0)
            .expect("read served frame");
        // Independent re-implementation of the wire swizzle (RGBA ->
        // little-endian xrgb8888, X pinned 0xFF): the served bytes are the
        // composed view, converted.
        let expected: Vec<u8> = retained
            .chunks_exact(4)
            .flat_map(|px| [px[2], px[1], px[0], 0xff])
            .collect();
        assert_eq!(served, expected, "capture must serve the composed view");
    }

    /// The P1.3.4 acceptance criteria, end to end over the real calloop
    /// wiring: a mock shim (its own thread of control, speaking the real
    /// wire protocol over a socketpair — P1.5.2 later forks real processes)
    /// drives an animated surface through a [`vitrin_ipc::ConnectionSource`]
    /// into the retained framebuffer.
    ///
    /// - **Animated end-to-end**: every presented frame reaches the
    ///   retained framebuffer (capture's pixel source) byte-exactly.
    /// - **No tearing**: each readback equals the deterministic generator
    ///   output for exactly one frame index — every pixel of frame N
    ///   differs from frame N+1, so a torn mix would equal neither.
    /// - **Pacing**: the shim renders frame N+1 only after frame N's
    ///   `frame_done` (which the wiring sends when the composite completes —
    ///   the headless output cadence), so frames rendered == presentations,
    ///   with monotonic presentation times.
    ///
    /// Shim death (EOF at the end of the animation) drops the surface from
    /// the scene — never a stale frame — through the same wiring.
    #[test]
    fn mock_shim_animates_the_retained_framebuffer_over_the_event_loop() {
        use std::time::Instant;

        use vitrin_ipc::{Connection, ConnectionEvent, ConnectionSource};
        use vitrin_mock_shim::frame_rgba;
        use vitrin_protocol::generated::vitrin_shim_surface::BufferStatus;

        use crate::shim::{ShimConfig, ShimServer};

        let _fd = crate::capture::tests::fd_lock();
        const W: u32 = 96;
        const H: u32 = 64;
        const FRAMES: u32 = 5;

        let (mut core_conn, shim_conn) = Connection::pair().expect("socketpair");

        // The mock shim: blocking client transport, paced by frame_done.
        let shim_thread = std::thread::spawn(move || {
            let mut shim = vitrin_mock_shim::MockShim::start(shim_conn)?;
            shim.run_paced_animation(FRAMES)
        });

        struct LoopState {
            headless: HeadlessState,
            server: Option<ShimServer>,
            start: Instant,
            /// Readback of the retained framebuffer after each presentation.
            presented: Vec<Vec<u8>>,
        }

        let size: Size<i32, Physical> = (W as i32, H as i32).into();
        let mut event_loop: EventLoop<LoopState> = EventLoop::try_new().expect("event loop");
        let mut state = LoopState {
            headless: HeadlessState::new(size, event_loop.get_signal())
                .expect("headless state under pixman"),
            server: Some(ShimServer::new(ShimConfig {
                realm: "realm-0".into(),
                width: W,
                height: H,
            })),
            start: Instant::now(),
            presented: Vec::new(),
        };

        // `configure` precedes the processing of any shim request (sent on
        // the still-blocking fd; ConnectionSource::new flips it after).
        state
            .server
            .as_ref()
            .expect("server present")
            .send_configure(&mut |frame| core_conn.send_message(frame, None))
            .expect("send configure");

        let source = ConnectionSource::new(core_conn).expect("connection source");
        event_loop
            .handle()
            .insert_source(source, |event, conn, state: &mut LoopState| match event {
                ConnectionEvent::Message(msg) => {
                    let Some(server) = state.server.as_mut() else {
                        return;
                    };
                    let mut send = |frame: &[u8]| vitrin_ipc::reply(conn, frame, None);
                    match server.handle_message(msg, &mut state.headless.scene, &mut send) {
                        Ok(false) => {}
                        Ok(true) => {
                            // Presentation, headless: the composite
                            // completing IS the output cadence ("or,
                            // headless, after it would have been").
                            //
                            // TEST-ONLY SHAPE — do not copy into the
                            // runtime loop (P1.5.2). Compositing
                            // synchronously per commit is what this pacing
                            // test needs (one readback per presented
                            // frame), but at runtime it would let a
                            // hostile shim buy a full-output composite per
                            // 12-byte repaint commit. The runtime wiring
                            // must coalesce: mark the scene dirty here and
                            // schedule at most one redraw + `presented`
                            // per loop iteration or output-cadence tick
                            // (see the "Wiring" section of `crate::shim`'s
                            // module docs; `wants_presentation`/`presented`
                            // already batch all owed frame_dones).
                            state.headless.redraw().expect("redraw on commit");
                            let time_ms = state.start.elapsed().as_millis() as u32;
                            server.presented(time_ms, &mut send).expect("frame_done");
                            state
                                .presented
                                .push(state.headless.latest_frame_rgba().expect("readback"));
                        }
                        // A compliant shim never faults; a violation here
                        // is a test failure. (The hostile paths are pinned
                        // by the unit tests in `crate::shim`.)
                        Err(fault) => panic!("compliant mock shim faulted: {fault}"),
                    }
                }
                ConnectionEvent::Disconnected => {
                    // Shim death: drop the surface — never a stale frame —
                    // and stop the loop.
                    if let Some(server) = state.server.take() {
                        server.connection_closed(&mut state.headless.scene);
                    }
                    state.headless.loop_signal.stop();
                }
                ConnectionEvent::Fault(reason) => panic!("transport fault: {reason}"),
            })
            .expect("insert connection source");

        event_loop
            .run(None, &mut state, |_| {})
            .expect("event loop");

        let stats = shim_thread
            .join()
            .expect("shim thread")
            .expect("mock shim animation");
        // Pacing: one frame per presentation, presentation times monotonic,
        // every buffer returned (released) in attach order.
        assert_eq!(stats.frames_rendered, FRAMES);
        assert_eq!(stats.frame_done_times.len(), FRAMES as usize);
        assert!(
            stats.frame_done_times.windows(2).all(|w| w[0] <= w[1]),
            "presentation times must be monotonic"
        );
        assert_eq!(
            stats.buffer_dones,
            (1..=FRAMES)
                .map(|id| (id, BufferStatus::Released))
                .collect::<Vec<_>>()
        );
        // Animated end-to-end, tear-free: each presented readback of the
        // retained framebuffer is byte-exact frame k.
        assert_eq!(state.presented.len(), FRAMES as usize);
        for (k, frame) in state.presented.iter().enumerate() {
            assert_eq!(
                frame,
                &frame_rgba(k as u32, W, H),
                "presented frame {k} must be the exact generator output"
            );
        }
        // After shim death the scene composes the deterministic background.
        assert!(state.server.is_none(), "server forgotten on disconnect");
        assert_eq!(
            state.headless.scene.compose(W, H),
            test_pattern::render(W, H),
            "shim death must drop the surface from the scene"
        );
    }
}
