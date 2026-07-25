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
//!
//! # Two retained images, because two audiences see different pixels (P1.7.1)
//!
//! [`HeadlessView`] retains **both** sides of the output-stage fork
//! ([`super::human_visible_from_view`]):
//!
//! - [`view_framebuffer`] — the composed **realm view**. This is the capture
//!   service's pixel source ([`HeadlessView::latest_frame_rgba`]) and it is
//!   overlay-free *structurally*: nothing composites a consent prompt into
//!   it, here or anywhere.
//! - [`output_framebuffer`] — the virtual display's **human-visible output**:
//!   the same realm view plus the consent overlay. Headless has no display,
//!   so this image is what stands in for one, and it is what the P1.7.1
//!   golden reads back.
//!
//! One `Scene::compose` call fills both every redraw, so they can differ only
//! by the overlay — never by drifting composition. A single shared image
//! would be the obvious simplification and is exactly wrong: the capture path
//! reads the retained image directly, so folding the two together would put
//! the consent prompt into `vitrin_view.frame_ready` and hand agents the
//! prompt-watching ability `docs/protocol/05-vitrin_consent.md` forbids.
//!
//! # The out-of-process consent injector (issue #138, `consent-injector`)
//!
//! A plain headless build hosts no consent prompt at all: it inherits
//! [`session::RuntimeHost::service_consent`]'s no-op, and `main` refuses
//! `--headless --consent=interactive` at startup because nothing here could
//! ever answer a prompt. That refusal is *correct*, and it is also why the
//! M1.4 consent property had no mock-free gate for so long: the only backend
//! that can raise a prompt is the one no CI runner can run.
//!
//! Under the `consent-injector` cargo feature — never a deployment build,
//! same posture as `dead-man-injector` — **and only when the invocation also
//! carries `--consent-injector-fd N`**, this backend gains exactly what the
//! refusal says it lacks, and nothing else:
//!
//! - **A display a harness can see the card in.** Not the whole frame:
//!   [`HeadlessView::consent_occlusion_window`] exports the consent card's
//!   own footprint, and nothing else, as a sealed memfd. The trust band, the
//!   trusted ring and the scrim are never even read back, so the session's
//!   indicator secret (issue #85) reaches no descriptor and no file in this
//!   build any more than in a shipping one. [`HeadlessView::latest_output_rgba`]
//!   stays `#[cfg(test)]`: **no build that ships, and no instrumented build
//!   either, can obtain a whole human-visible frame.**
//! - **A way for a human to answer.** An inherited `AF_UNIX`/`SOCK_STREAM`
//!   socketpair ([`crate::consent::injector`]) on which the peer says
//!   `decide <token> <button>`; the core deposits one
//!   [`Decision`](crate::consent::grab::Decision) into the round's
//!   [`ConsentGrab`](crate::consent::grab::ConsentGrab). That is the *only*
//!   thing it does: the decision is then drained, validated and applied by
//!   [`session::service_consent_round`] and
//!   `PetitionRegistry::resolve_human` — the same two calls a real click on
//!   the nested backend reaches. There is no second decision path and no
//!   second authority-checking site; there is one funnel, fed by a socket
//!   instead of a mouse.
//!
//! With no flag there is no grab, no channel and no consent round: "the
//! injector is absent" is true at runtime, not merely at the parser.
//!
//! What this still cannot prove, and only nested mode with a human at a mouse
//! can, is that a *physical click on the button rectangle* produces the
//! decision — the hit test, the guard interval, the press-arms/release-commits
//! ladder and the "an agent may not answer its own prompt" origin check. Those
//! live in [`crate::consent::grab`]'s own tests and in `shim/docs/firefox.md`'s
//! §9 nested recipe. The router here still stacks [`NoopHook`]: no input of any
//! origin reaches this grab's `judge`, because there is no intake to tag one
//! physical.
//!
//! [`view_framebuffer`]: HeadlessView::view_framebuffer
//! [`output_framebuffer`]: HeadlessView::output_framebuffer

use std::error::Error;

#[cfg(feature = "consent-injector")]
use std::os::fd::AsFd;

use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopHandle, LoopSignal};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Frame, ImportMem, Offscreen, Renderer};
use smithay::reexports::pixman::Image;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};
use tracing::info;

use crate::consent::{ConsentSurface, TrustedIndicator};
use crate::deadman::DeadManConfig;
use crate::input::{InputRouter, NoopHook};
use crate::recorder::Recorder;
use crate::scene::Scene;
use crate::session::{self, Runtime, RuntimeSeed};
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

    let pixels = Scene::new().compose(size.w as u32, size.h as u32);
    composite(&mut renderer, &mut framebuffer, size, &pixels)?;
    readback(&mut renderer, &mut framebuffer, size)
}

/// Read a composited framebuffer back as tightly packed RGBA8888, rows
/// top-down: `copy_framebuffer` copies (SRC) the bound image into a fresh
/// mapping, which `map_texture` then exposes as bytes. For a 32-bpp format
/// the stride is `width * 4`, so the slice is tightly packed and needs no
/// de-striding.
///
/// Shared by [`render_once`] and [`HeadlessView::latest_frame_rgba`] (the
/// capture service's pixel source, P1.3.6), so readback has one definition
/// and one byte layout.
fn readback(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
    readback_region(renderer, framebuffer, Rectangle::from_size(buffer_size))
}

/// [`readback`] restricted to `region` — the same copy, the same layout, over
/// a sub-rectangle instead of the whole image.
///
/// Smithay's `PixmanRenderer::copy_framebuffer` allocates a `region.size`
/// image and composites `Src` from `region.loc`, so bytes outside `region`
/// are **never read into a buffer at all**. That is not a detail here: it is
/// what lets the `consent-injector` build's occlusion export
/// ([`HeadlessView::consent_occlusion_window`], issue #138) claim the
/// session's trusted indicator was never *read*, rather than the weaker
/// "was redacted after reading" — the difference between a property and a
/// redactor that could have a bug in it.
///
/// The tight-packing assertion is re-derived from the region rather than the
/// image, so the invariant travels with the caller's rectangle.
fn readback_region(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    region: Rectangle<i32, Buffer>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let target = renderer.bind(framebuffer)?;
    let mapping = renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888)?;
    let pixels = renderer.map_texture(&mapping)?;

    let expected = region.size.w as usize * region.size.h as usize * test_pattern::BYTES_PER_PIXEL;
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
/// [`HeadlessView::redraw`] is the re-entrant entry point P1.3.4 calls
/// again on client damage (a scene commit), without reworking this control
/// flow.
///
/// # What the M1.1 runtime wiring changed about "then idle"
///
/// The idle is no longer the whole session. The loop now also carries the
/// core socket's listener, every accepted principal connection, the realm's
/// shim socketpair and the expiry sweep ([`crate::session`]), so a headless
/// core wakes on protocol traffic and recomposites on latched commits — but
/// only ever **once per dispatch round**, in [`session::post_dispatch`],
/// never once per commit. The single initial composite below is still what
/// makes the retained images readable before the first agent connects.
///
/// The recorder travels through here rather than staying in `run_session`
/// because calloop fixes one state type per loop and the whole kernel — the
/// recorder with it — has to live in that state. It is handed straight back,
/// so the run's footer is still written by the code that opened the log.
///
/// `dead_man` is the session's configured chord/hold — accepted here so both
/// backends' `run` have the same signature, and read past that only by a
/// `dead-man-injector` build (issue #109): a plain build has no physical
/// input device and, structurally, no way to *fire* the switch, so it never
/// looks at this beyond naming it in the parameter list. See the
/// `dead-man-injector` block in `run_inner` and [`crate::deadman`]'s module
/// docs ("the test injector proves the consequence half").
///
/// # Why the two backends' `run` signatures now differ
///
/// `consent_injector_fd` exists only on a `consent-injector` build (issue
/// #138) and only here. `--consent-injector-fd` is refused at *parse* time
/// with `--nested`, so the nested backend can never receive one: nested has a
/// real human at a real mouse, and the `service_consent` override the channel
/// feeds lives on this backend. Threading it as a cfg'd parameter rather than
/// through [`RuntimeSeed`] is deliberate — the seed is kernel state, not a
/// test channel.
pub fn run(
    size: (u32, u32),
    dead_man: DeadManConfig,
    #[cfg(feature = "consent-injector")] consent_injector_fd: Option<std::os::fd::RawFd>,
    seed: RuntimeSeed,
) -> (Recorder, Result<(), Box<dyn Error>>) {
    // The seed is consumed the moment the state is built; until then it is
    // still ours, and either way the recorder must come back so `run_session`
    // can write the footer it owes. Threading it through two slots keeps `?`
    // usable for every startup step below instead of turning each into a
    // four-line `match` (the nested backend does exactly the same).
    let mut seed = Some(seed);
    let mut recovered = None;
    let result = run_inner(
        size,
        dead_man,
        #[cfg(feature = "consent-injector")]
        consent_injector_fd,
        &mut seed,
        &mut recovered,
    );
    let recorder = recovered
        .or_else(|| seed.take().map(|seed| seed.recorder))
        .expect("the seed is either still unconsumed or its recorder was recovered");
    (recorder, result)
}

fn run_inner(
    size: (u32, u32),
    #[cfg_attr(not(feature = "dead-man-injector"), allow(unused_variables))]
    dead_man: DeadManConfig,
    #[cfg(feature = "consent-injector")] consent_injector_fd: Option<std::os::fd::RawFd>,
    seed: &mut Option<RuntimeSeed>,
    recovered: &mut Option<Recorder>,
) -> Result<(), Box<dyn Error>> {
    let (width, height) = size;
    let mut event_loop: EventLoop<'static, HeadlessState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Signal sources first (same posture as the nested backend): the process
    // is single-threaded here, so both masks are installed before anything
    // else runs. `signalfd` only ever sees a signal that is blocked on
    // **every** thread, so installing either of these after a backend has
    // spawned a thread would silently deliver the signal the old way instead
    // — and for SIGCHLD that means a realm's exit is never noticed.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state: &mut HeadlessState| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.view.loop_signal.stop();
    })?;
    // SIGCHLD: the realm's shim exiting. A hint only — it says some child
    // changed state, never which — so the handler asks `waitpid`.
    loop_handle.insert_source(
        crate::lifecycle::child_signal_source()?,
        |_event, _, state: &mut HeadlessState| session::reap_realm(state),
    )?;

    // The dead-man test injector (issue #109), compiled in only for a
    // `dead-man-injector` build and never for a deployment one (same posture
    // as `crate::petitions`' `scripted-consent`). Headless has no physical
    // input device to hold the configured chord on
    // (`crate::deadman`'s module docs), so CI's stand-in for a completed
    // hold is a signal instead of a keypress -- delivered here through the
    // exact same `Runtime::apply_dead_man` entry point the nested backend's
    // `DeadManHost::on_trigger` calls for a real one, so what this proves
    // about revocation is exactly as strong as the nested path's. Blocked
    // process-wide by `main::block_loop_signals` before this runs, under the
    // same feature gate, so this `Signals::new` only creates the descriptor
    // that reads what is already blocked.
    #[cfg(feature = "dead-man-injector")]
    loop_handle.insert_source(
        Signals::new(&[Signal::SIGUSR1])?,
        move |_event, _, state: &mut HeadlessState| {
            let trigger = crate::deadman::Trigger {
                chord: dead_man.chord.name(),
                held: dead_man.hold,
            };
            tracing::warn!(
                chord = trigger.chord,
                held_ms = trigger.held.as_millis(),
                "dead-man-injector: SIGUSR1 received, synthesizing a completed chord \
                 (issue #109; this build path never ships)"
            );
            state
                .runtime
                .apply_dead_man(&trigger, std::time::Instant::now());
        },
    )?;

    let physical_size: Size<i32, Physical> = (width as i32, height as i32).into();
    info!(
        width,
        height,
        "headless backend starting: virtual output, software pixman renderer (no EGL/DRM/GPU)"
    );

    // The session's trusted indicator, minted in `run_session` before the
    // listener accepted anyone (issue #85). Read by `Copy` before the seed is
    // consumed below.
    let indicator = seed
        .as_ref()
        .expect("the seed is present until the state is built")
        .indicator;
    // The out-of-process consent injector's channel (issue #138), adopted
    // from the descriptor `--consent-injector-fd` named. Validated as an open
    // `AF_UNIX`/`SOCK_STREAM` socket at or above fd 3 before anything is read
    // from it, and a failure is a **startup error** rather than a warning: a
    // session started with a hook that is not there would look instrumented
    // from the outside and behave as a plain one, which is the worst of both.
    #[cfg(feature = "consent-injector")]
    let injector = match consent_injector_fd {
        Some(number) => Some(
            crate::consent::injector::Injector::adopt(number).map_err(Box::<dyn Error>::from)?,
        ),
        None => None,
    };
    let view = HeadlessView::new(physical_size, event_loop.get_signal(), indicator)?;
    let mut state = HeadlessState {
        view,
        // Headless has no physical input device — structurally, not by a
        // runtime check — so its router stacks no preemption hook: there is
        // no chord to hold and no prompt for a human to click.
        runtime: Runtime::new(
            seed.take().expect("the seed is consumed exactly once"),
            InputRouter::new(NoopHook),
        ),
        loop_handle: event_loop.handle(),
        fatal: None,
        // An idle grab: nothing is armed until `service_consent` raises the
        // first pending petition, and nothing feeds it but the channel above
        // (module docs — the router stacks `NoopHook`, so no input of any
        // origin can reach `ConsentGrab::judge` here). With no channel,
        // `service_consent` returns on its first line and this grab is never
        // touched at all.
        #[cfg(feature = "consent-injector")]
        grab: crate::consent::grab::ConsentGrab::new(),
        #[cfg(feature = "consent-injector")]
        injector,
    };

    // Readiness for the injector channel. The `Injector` itself lives in the
    // state (it is written from `service_consent`, which the state owns), so
    // the source carries a *duplicate* descriptor used for nothing but
    // `poll`: `Generic` needs an owned `AsFd`, and the alternative — an
    // `Rc<RefCell<_>>` around a test hook — would put a cell in the
    // compositor's dispatch path for the sake of a build that never ships.
    #[cfg(feature = "consent-injector")]
    if let Some(injector) = state.injector.as_ref() {
        let poll_fd = rustix::io::fcntl_dupfd_cloexec(injector.as_fd(), 3)?;
        loop_handle.insert_source(
            calloop::generic::Generic::new(poll_fd, calloop::Interest::READ, calloop::Mode::Level),
            |_readiness, _fd, state: &mut HeadlessState| {
                // EOF or a protocol violation removes the source; the core
                // keeps running and every pending petition times out, which
                // is the fail-closed direction.
                Ok(if state.service_injector() {
                    calloop::PostAction::Continue
                } else {
                    calloop::PostAction::Remove
                })
            },
        )?;
    }

    // Composite once so the retained images are ready before we start idling
    // (see this function's doc comment for why exactly once).
    if let Err(err) = state.view.redraw() {
        *recovered = Some(state.runtime.into_recorder());
        return Err(err);
    }
    info!(
        "virtual-output framebuffers composited and retained in memory: realm view \
         (capture-ready) + human-visible output (consent overlay)"
    );

    if let Err(err) = session::install(&loop_handle, &mut state.runtime) {
        *recovered = Some(state.runtime.into_recorder());
        return Err(err);
    }

    // The realm, only now. `install` has put the listener and the sweeps on
    // the loop, so the loop is ready to service the shim's socketpair the
    // moment the child has one — and that ordering is the whole of trap T1:
    // the shim blocks on `configure` and then on every reply, with no timeout
    // anywhere on its side, so spawning before the reader exists wedges it
    // permanently rather than slowly. `event_loop.run` is the very next
    // statement.
    if let Err(err) = session::start_realm(&mut state) {
        tracing::error!("fatal: cannot start the session's realm: {err}");
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
    // The shutdown ladder, here and not in `run_session`: it blocks
    // deliberately (hang up, wait, SIGTERM, wait, SIGKILL) so it must run
    // after the loop has stopped and never inside a dispatch, and it must run
    // before the recorder is handed back so the realm's `realm_died` /
    // `realm_exited` entries land in the run they belong to. The event loop
    // is still alive in this scope, which is what lets rung 0 retire the shim
    // connection's registration.
    session::shutdown_realm(&mut state);

    // Recovered before the early return below, so a fatal run still returns
    // the recorder that must write the footer naming it.
    *recovered = Some(state.runtime.into_recorder());
    outcome?;
    info!("event loop stopped, shutting down");
    Ok(())
}

/// The headless backend's calloop state: the presentation half and the
/// runtime half, as two disjoint fields.
///
/// The disjointness is not incidental — see [`session::RuntimeHost`]. Merging
/// these two into one flat struct would make [`session::RuntimeHost::split`]
/// unimplementable and force a `RefCell` into the compositor's dispatch path.
pub(crate) struct HeadlessState {
    view: HeadlessView,
    runtime: Runtime<NoopHook>,
    loop_handle: LoopHandle<'static, HeadlessState>,
    /// Set when a composite failure stops the loop, so [`run`] propagates it
    /// as an error (and `main` as a non-zero exit) instead of masking a
    /// mid-run fatal as a clean shutdown.
    fatal: Option<Box<dyn Error>>,
    /// The `consent-injector` build's input grab (issue #138, module docs),
    /// present only when `--consent-injector-fd` wired a channel.
    ///
    /// A *fourth* disjoint field, deliberately not folded into `view`: it is
    /// borrowed at the same time as `runtime` and `view.output.consent` by
    /// [`session::service_consent_round`], which is exactly the borrow shape
    /// the nested backend reaches for an `Rc<RefCell<_>>` to satisfy. Here it
    /// needs no `Rc` at all, because no input hook shares it — headless
    /// stacks [`NoopHook`], so this grab has exactly one feeder, the channel
    /// below.
    #[cfg(feature = "consent-injector")]
    grab: crate::consent::grab::ConsentGrab,
    /// The adopted injector channel, or `None` when the run named no
    /// `--consent-injector-fd` (in which case none of the consent machinery
    /// above runs at all) or when the peer has gone away.
    #[cfg(feature = "consent-injector")]
    injector: Option<crate::consent::injector::Injector>,
}

/// The `consent-injector` build's channel service (issue #138): read the
/// peer's requests and hand at most one decision to the grab, exactly where a
/// click deposits one.
#[cfg(feature = "consent-injector")]
impl HeadlessState {
    /// Drain everything the peer has written. Returns `false` once the
    /// channel is finished, so the caller removes the calloop source.
    ///
    /// **Fail-closed on every ambiguity.** A malformed line queues nothing
    /// (`decided-ack malformed`) rather than synthesising a `Deny`: a denial
    /// nobody took would be a lie in the flight recorder, whereas letting the
    /// petition time out is equally refusing and true. An unknown or spent
    /// token queues nothing. A button the raised prompt does not offer queues
    /// nothing. A decision arriving with no card up queues nothing.
    ///
    /// Nothing here confers anything: [`ConsentGrab::queue_decision`] is an
    /// enqueue, and [`session::service_consent_round`] — running at the end of
    /// this same dispatch, through `post_dispatch` — is what applies it, via
    /// `PetitionRegistry::resolve_human`. Three further latches stand behind
    /// the token: `ArmedPrompt::decided`, `ConsentGrab::lower`'s
    /// decision-dropping, and the registry's `NotPending`.
    ///
    /// [`ConsentGrab::queue_decision`]: crate::consent::grab::ConsentGrab::queue_decision
    fn service_injector(&mut self) -> bool {
        use crate::consent::injector::{DecideAck, Request};

        let Some(injector) = self.injector.as_mut() else {
            return false;
        };
        let Ok(requests) = injector.poll_requests() else {
            self.injector = None;
            return false;
        };
        for request in requests {
            match request {
                None => {
                    let injector = self.injector.as_mut().expect("still present in this loop");
                    tracing::warn!(
                        "consent-injector: unparseable line; nothing queued (issue #138)"
                    );
                    injector.send_line(
                        &format!("decided-ack {}", DecideAck::Malformed.word()),
                        None,
                    );
                }
                Some(Request::Describe) => self.answer_describe(),
                Some(Request::Decide { token, choice }) => self.answer_decide(token, choice),
            }
        }
        true
    }

    /// Answer `describe`: recomposite, then report the consent surface's
    /// geometry and — when a card is up — export its footprint.
    ///
    /// The recomposite comes first so the exported window is synchronously
    /// consistent with the core's current prompt state. That removes an
    /// entire class of harness race (no "poll a file until a header flips")
    /// at the cost of one extra composite per describe, in a build that never
    /// ships.
    fn answer_describe(&mut self) {
        let (view_w, view_h) = {
            let size = self.view.output.size;
            (size.w.max(0) as u32, size.h.max(0) as u32)
        };
        if let Err(err) = self.view.redraw() {
            tracing::error!("consent-injector: describe could not recomposite: {err}");
        }
        let card = self.view.output.consent.card_rect(view_w, view_h);
        let window = self.view.consent_occlusion_window();
        let token = self
            .injector
            .as_ref()
            .and_then(|inj| inj.live_token())
            .map(|(_, token)| token.to_hex())
            .unwrap_or_else(|| "-".into());
        let (state, cx, cy, cw, ch) = match card {
            Some((x, y, w, h)) => ("shown", x, y, w, h),
            None => ("none", 0, 0, 0, 0),
        };
        let (wx, wy, ww, wh, bytes) = match &window {
            Some((rect, pixels)) => (
                rect.loc.x,
                rect.loc.y,
                rect.size.w.max(0) as u32,
                rect.size.h.max(0) as u32,
                pixels.len(),
            ),
            None => (0, 0, 0, 0, 0),
        };
        let line = format!(
            "prompt {state} {token} {cx} {cy} {cw} {ch} {view_w} {view_h} {band} {wx} {wy} \
             {ww} {wh} {bytes}",
            band = crate::consent::TRUST_BAND_HEIGHT,
        );
        // Seal the pixels only once, and only if there are any: exactly one
        // way pixels leave this process (`capture::sealed_frame_memfd`).
        let sealed = window.as_ref().and_then(|(_, pixels)| {
            crate::capture::sealed_frame_memfd(pixels)
                .inspect_err(|err| {
                    tracing::error!("consent-injector: cannot seal the occlusion window: {err}")
                })
                .ok()
        });
        let Some(injector) = self.injector.as_mut() else {
            return;
        };
        match &sealed {
            Some(fd) => injector.send_line(&line, Some(fd.as_fd())),
            None => injector.send_line(&line, None),
        }
    }

    /// Answer `decide <token> <button>`: validate, then enqueue at most one
    /// decision. See [`Self::service_injector`] for the fail-closed table.
    fn answer_decide(
        &mut self,
        token: crate::consent::injector::PromptToken,
        choice: crate::consent::Choice,
    ) {
        use crate::consent::grab::Decision;
        use crate::consent::injector::DecideAck;

        let armed = self.injector.as_ref().and_then(|inj| inj.armed_petition());
        let live = self.injector.as_ref().and_then(|inj| inj.live_token());
        let ack = match (armed, live) {
            // No card is up at all: there is nothing to press.
            (None, _) => DecideAck::NoPrompt,
            // A card is up but this token does not name it -- wrong, stale,
            // or already spent by an earlier accepted decision. Distinguished
            // from `no-prompt` on purpose: answering "no card is up" while one
            // still is would be a false statement about the screen.
            (Some(_), None) => DecideAck::UnknownToken,
            (Some(_), Some((_, live_token))) if live_token != token => DecideAck::UnknownToken,
            (Some(petition), Some(_)) => {
                // The armed petition is the authority on what is on screen;
                // the channel's token is only a name for it. A disagreement
                // is a race with `retire_stale`, and it fails closed.
                if self.grab.armed_petition() != Some(petition) {
                    DecideAck::UnknownToken
                } else if !self
                    .view
                    .output
                    .consent
                    .prompt()
                    .map(|prompt| prompt.choices().contains(&choice))
                    .unwrap_or(false)
                {
                    // A button the card does not draw is not a button a human
                    // could have pressed, so it is not one this channel may
                    // synthesise either.
                    DecideAck::NoSuchButton
                } else {
                    let peer = self.injector.as_ref().and_then(|inj| inj.peer_pid());
                    tracing::warn!(
                        %petition,
                        choice = choice.label(),
                        peer_pid = ?peer,
                        peer_uid = rustix::process::geteuid().as_raw(),
                        "consent-injector: synthesizing a human decision on the raised prompt \
                         (issue #138; this build path never ships)"
                    );
                    self.grab.queue_decision(Decision { petition, choice });
                    if let Some(inj) = self.injector.as_mut() {
                        // Spend the token: a replay is `unknown-token`, and a
                        // decision can never land on a petition that advanced
                        // underneath it.
                        inj.spend_token();
                    }
                    DecideAck::Queued
                }
            }
        };
        if let Some(injector) = self.injector.as_mut() {
            injector.send_line(&format!("decided-ack {}", ack.word()), None);
        }
    }
}

impl session::RuntimeHost for HeadlessState {
    type Hook = NoopHook;
    type View = HeadlessView;

    fn split(&mut self) -> (&mut Runtime<NoopHook>, &mut HeadlessView) {
        (&mut self.runtime, &mut self.view)
    }

    fn loop_handle(&self) -> LoopHandle<'static, Self> {
        self.loop_handle.clone()
    }

    fn stop(&mut self, fatal: Option<Box<dyn Error>>) {
        self.fatal = fatal;
        self.view.loop_signal.stop();
    }

    /// Drive interactive consent for this dispatch round on a
    /// `consent-injector` build with a wired channel (issue #138) — the
    /// *same* orchestration the nested backend runs, called with this
    /// backend's own disjoint fields instead of an `Rc<RefCell<_>>`.
    ///
    /// A plain build has no override here and inherits the trait's no-op,
    /// which is why `main` may keep refusing `--headless
    /// --consent=interactive` there: with this method absent, a petition
    /// really could only pend until it timed out. A feature build with **no**
    /// `--consent-injector-fd` returns on the first line, so the refusal is
    /// equally correct there, and equally for the same reason.
    ///
    /// `set_view` every round rather than once at startup, matching
    /// [`ConsentGrab::set_view`]'s contract that the grab's view is the same
    /// one the router hit-tests in. Nothing hit-tests here (the channel names
    /// its petition outright), but a grab whose view is a lie is exactly the
    /// state that makes a *later* wiring of real input land its clicks
    /// somewhere unintended, and the fix costs two integer copies.
    ///
    /// The `raised`/`lowered` edges are emitted from the armed-petition
    /// transition this call produces, **after** `service_consent_round`
    /// returns — so a `raised` line the harness can read is a guarantee, in
    /// the core's own ordering, that `ConsentGrab::raise` has already run
    /// `mark_prompt_shown` and `consent_held` is therefore already in force
    /// for that principal. A harness that blocks on the line and then acts
    /// needs no sleep and can observe no half-raised state.
    ///
    /// [`ConsentGrab::set_view`]: crate::consent::grab::ConsentGrab::set_view
    #[cfg(feature = "consent-injector")]
    fn service_consent(&mut self, now: std::time::Instant) {
        if self.injector.is_none() {
            return;
        }
        let size = self.view.output.size;
        self.grab
            .set_view((size.w.max(0) as u32, size.h.max(0) as u32));
        let before = self.grab.armed_petition();
        // Three disjoint field borrows, which is the whole reason `grab` is a
        // field of the state rather than of the view: `service_consent_round`
        // needs the registry and the recorder (inside `runtime`) live at the
        // same time as the surface it draws on.
        if session::service_consent_round(
            &mut self.grab,
            &mut self.runtime,
            &mut self.view.output.consent,
            now,
        ) {
            // A card went up or came down: the human-visible framebuffer
            // changed, so the round must recomposite. Headless owns its frame
            // clock, so marking dirty is the whole of it -- `post_dispatch`
            // redraws immediately after this returns.
            self.runtime.dirty = true;
        }
        let after = self.grab.armed_petition();
        if before != after {
            if let Some(injector) = self.injector.as_mut() {
                if let Some(gone) = before {
                    injector.note_lowered(gone);
                }
                if let Some(up) = after {
                    injector.note_raised(up);
                }
            }
        }
    }
}

impl session::Presenter for HeadlessView {
    fn scene(&mut self) -> &mut Scene {
        &mut self.scene
    }

    fn view_size(&self) -> (u32, u32) {
        let size = self.output.size;
        (size.w.max(0) as u32, size.h.max(0) as u32)
    }

    /// Always [`Presentation::Completed`]: this backend composites
    /// synchronously into its retained framebuffer, so the composite finishing
    /// *is* the output cadence and any owed `frame_done` is due on return.
    fn redraw(&mut self) -> Result<session::Presentation, Box<dyn Error>> {
        HeadlessView::redraw(self)?;
        Ok(session::Presentation::Completed)
    }

    /// The retained realm view, read back tightly packed.
    ///
    /// A readback failure yields `None` rather than an error: this is called
    /// on the redraw path, where the alternative is tearing down a session
    /// over a transient mapping failure, and `None` degrades to the
    /// chokepoint's `no_surface` refusal — the same answer a capture gets
    /// before any surface exists. Never the *output* image: that one carries
    /// the consent overlay, which no capture may ever contain.
    fn view_rgba(&mut self) -> Option<Vec<u8>> {
        self.latest_frame_rgba().ok()
    }

    /// All three, lent to `f`: the scene and retained image from the two
    /// fields the struct was split into for exactly this call (see
    /// [`HeadlessView::output`]); no importer, since this backend has no GPU
    /// renderer at all — [`session::Presenter::scene_and_importer`]'s
    /// default already answers `None` for the same reason, restated here
    /// because `teardown_view` carries no default.
    fn teardown_view<R>(
        &mut self,
        f: impl for<'v> FnOnce(
            &'v mut Scene,
            Option<&'v mut dyn crate::lifecycle::RetainedOutput>,
            Option<&'v mut dyn crate::dmabuf::DmabufImporter>,
        ) -> R,
    ) -> R {
        f(&mut self.scene, Some(&mut self.output), None)
    }
}

/// Per-run state of the headless backend: the software renderer, the realm's
/// [`Scene`], the consent surface, and the two retained images the module
/// docs describe.
pub(crate) struct HeadlessView {
    /// The realm's scene (P1.3.3): the single-maximized client surface, or
    /// the deterministic background when none is committed. The shim-facing
    /// protocol server (P1.3.4) commits into it and calls
    /// [`redraw`](Self::redraw); the realm object (P1.5.1) hangs off it.
    scene: Scene,
    /// The renderer and the two retained images, in their own struct.
    ///
    /// **The split is load-bearing, not tidiness.** A realm's death borrows
    /// the scene and the retained output *at the same time*:
    /// `RealmTeardown` holds `scene: &mut Scene` and
    /// `retained: Option<&mut dyn RetainedOutput>` together, because
    /// `RealmLifecycle::die` clears the surface and scrubs the framebuffer
    /// inside one latched transition. With `scrub_retained_frame`
    /// implemented on the whole view, those would be two `&mut` borrows of
    /// one struct and the teardown funnel would be unbuildable — which is
    /// exactly the pressure that would push someone toward a second,
    /// unlatched scrub path outside the funnel.
    ///
    /// The split is honest as well as convenient: the scrub already
    /// deliberately does not touch the scene (see
    /// [`RetainedOutput::scrub_retained_frame`]'s implementation below), so
    /// these really are disjoint concerns — the scene is what a frame is
    /// composed *from*, and this is where composed frames are *kept*.
    ///
    /// [`RetainedOutput::scrub_retained_frame`]: crate::lifecycle::RetainedOutput::scrub_retained_frame
    output: HeadlessOutput,
    loop_signal: LoopSignal,
}

/// The headless backend's renderer and its two retained images — everything
/// a composed frame lands in, and nothing a frame is composed from.
pub(crate) struct HeadlessOutput {
    renderer: PixmanRenderer,
    /// The consent surface (P1.7.1): the prompt composited above the realm
    /// view on the human-visible side of the output stage only. Always
    /// present, empty until P1.7.2 puts a petition up.
    consent: ConsentSurface,
    /// The composed **realm view**, retained across the process lifetime
    /// (PRD Doc 2 §9) so an internal capture reads composited pixels, not a
    /// freshly cleared buffer. Overlay-free by construction — this is what
    /// [`HeadlessView::latest_frame_rgba`] serves to agents.
    view_framebuffer: Image<'static, 'static>,
    /// The virtual display's **human-visible output**: the realm view with
    /// the consent overlay on top. Never read by the capture path.
    output_framebuffer: Image<'static, 'static>,
    size: Size<i32, Physical>,
}

impl HeadlessView {
    fn new(
        size: Size<i32, Physical>,
        loop_signal: LoopSignal,
        indicator: TrustedIndicator,
    ) -> Result<Self, Box<dyn Error>> {
        let mut renderer = PixmanRenderer::new()?;
        // `.max(0)` is defensive only: `run` always passes a positive size
        // (the CLI parser rejects zero/negative), but a degenerate size must
        // never wrap to a huge allocation on the `i32 -> usize` cast.
        let buffer_size: Size<i32, Buffer> = (size.w.max(0), size.h.max(0)).into();
        let view_framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        let output_framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        Ok(Self {
            scene: Scene::new(),
            output: HeadlessOutput {
                renderer,
                consent: ConsentSurface::new(indicator),
                view_framebuffer,
                output_framebuffer,
                size,
            },
            loop_signal,
        })
    }

    /// (Re)composite both retained images. The single redraw entry point:
    /// called once from [`run`] today, and again on client damage (a scene
    /// commit) once P1.3.4 feeds the scene — or on a consent-surface change
    /// (P1.7.2), which is why [`ConsentSurface::generation`] exists.
    ///
    /// One [`Scene::compose`] call feeds both images, so the realm view an
    /// agent may capture and the output a human sees are the same
    /// composition, differing only by the overlay (module docs).
    ///
    /// # Why two full composites per frame are accepted
    ///
    /// Both retained images are rewritten every redraw, even with no prompt up
    /// — when the two are byte-identical and the second blit writes pixels the
    /// first just wrote. That cost is deliberate. The alternative is tracking
    /// when `output_framebuffer` is a stale alias of `view_framebuffer`, which
    /// reintroduces exactly the class of cache-invalidation bug this backend
    /// exists to be free of: the invariant `output_framebuffer ==
    /// human_visible_from_view(view, consent)` is what the P1.7.1 golden and
    /// [`Self::scrub_retained_frame`] both rest on, and it is worth more here
    /// than the CPU. Headless is the deterministic backend CI runs, not the
    /// one a human watches; the nested backend, which *is* watched, already
    /// uploads once per (size, scene, consent) change rather than once per
    /// frame.
    fn redraw(&mut self) -> Result<(), Box<dyn Error>> {
        self.output.present(&self.scene)
    }

    /// The capture service's pixel source (P1.3.6): the **latest completed
    /// frame** of the *realm view*, read back from
    /// [`Self::view_framebuffer`] as tightly packed RGBA8888 — a pure read
    /// that never triggers a composite. This is the capture-timing decision
    /// (see [`crate::capture`]'s module docs): capture observes what the
    /// compositor last finished, exactly the IDL's "most recently composited
    /// content as of when the server processes this request", keeping
    /// goldens deterministic and keeping render cost off the agent-request
    /// path.
    ///
    /// Reads the realm view, **not** the human-visible output, so a consent
    /// prompt that is on screen is absent from every capture
    /// (`docs/protocol/05-vitrin_consent.md`;
    /// `a_capture_taken_while_a_prompt_is_up_contains_no_overlay` pins it).
    ///
    /// Reached at runtime through [`session::Presenter::view_rgba`], which
    /// the runtime calls on the **redraw** path to refresh the cache the
    /// chokepoint's capture reads -- never on the agent-request path, which
    /// is what keeps the "pure read" claim above true of the wired code and
    /// not only of this function.
    fn latest_frame_rgba(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        let out = &mut self.output;
        readback(&mut out.renderer, &mut out.view_framebuffer, out.size)
    }

    /// The virtual display's latest human-visible output — realm view plus
    /// consent overlay — read back from [`Self::output_framebuffer`].
    ///
    /// Headless mode has no display, so this is the only thing that can
    /// stand in for "what a human would see", which is what makes the P1.7.1
    /// prompt golden-testable on a GPU-less CI runner at all. It has no wire
    /// path and never will: no protocol message delivers human-visible
    /// output.
    ///
    /// **`#[cfg(test)]`, not merely `allow(dead_code)`.** This and
    /// [`Self::latest_frame_rgba`] have the same signature and differ only by
    /// name, and `capture::RealmViewFrame.rgba` is a plain `&[u8]` that either
    /// one satisfies. The runtime wiring picked the realm view, as it must,
    /// and "the output" is the more
    /// natural-sounding name for a getter that would deliver the consent
    /// overlay to every `vitrin_view.frame_ready` — the one thing
    /// `docs/protocol/05-vitrin_consent.md` forbids, with no test to catch it
    /// because no test yet exercises the wired path. Compiling this out of
    /// non-test builds makes that mistake fail to *build* rather than merely
    /// be discouraged by a doc comment, which is the posture the rest of this
    /// overlay's design takes.
    #[cfg(test)]
    fn latest_output_rgba(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        let out = &mut self.output;
        readback(&mut out.renderer, &mut out.output_framebuffer, out.size)
    }

    /// The **consent card's own footprint** of the human-visible output, and
    /// nothing else (`consent-injector`, issue #138).
    ///
    /// This is the whole of what an instrumented build can see of the human's
    /// screen, and the restriction is the point. The exported rectangle is
    /// byte-identical to `render::rasterize(prompt)` — see
    /// [`ConsentSurface::card_rect`]'s docs for the ordering argument and the
    /// in-process test that checks it — so the session's trusted indicator
    /// (issue #85) is not merely redacted, it is **never read**:
    /// [`readback_region`] hands smithay a sub-rectangle, and
    /// `copy_framebuffer` composites only from that rectangle's origin.
    ///
    /// # Not parameterised by any caller-supplied rectangle
    ///
    /// The peer on the injector channel cannot ask for a region. It gets the
    /// card or it gets nothing, and which one is decided here, from the
    /// core's own geometry.
    ///
    /// # Four fail-closed guards, all pure geometry, before any readback
    ///
    /// A prompt is up; the rect is non-empty; the rect is wholly inside the
    /// view; and the rect is wholly at `y >= TRUST_BAND_HEIGHT`. Any failure
    /// exports nothing and logs at `error`, and the gate then fails on a
    /// zero-byte window. The last guard is the machine-checkable statement of
    /// "the band was never read", and it is *necessary* rather than
    /// belt-and-braces: the card's height is content-derived
    /// (`render::rasterize` sums row heights, and
    /// `card_height_tracks_its_content` proves a longer principal makes a
    /// taller card), so a tall card in a short view really can clip into the
    /// band.
    ///
    /// [`ConsentSurface::card_rect`]: crate::consent::ConsentSurface::card_rect
    #[cfg(feature = "consent-injector")]
    fn consent_occlusion_window(&mut self) -> Option<(Rectangle<i32, Buffer>, Vec<u8>)> {
        let size = self.output.size;
        let (view_w, view_h) = (size.w.max(0) as u32, size.h.max(0) as u32);
        let (x, y, w, h) = self.output.consent.card_rect(view_w, view_h)?;
        let band = crate::consent::TRUST_BAND_HEIGHT as i32;
        if w == 0 || h == 0 {
            tracing::error!("consent-injector: the card has no area; exporting nothing");
            return None;
        }
        if x < 0 || y < 0 || x + w as i32 > view_w as i32 || y + h as i32 > view_h as i32 {
            tracing::error!(
                x,
                y,
                w,
                h,
                view_w,
                view_h,
                "consent-injector: the card does not fit the view; exporting nothing"
            );
            return None;
        }
        if y < band {
            tracing::error!(
                y,
                band,
                "consent-injector: the card reaches into the trusted band; exporting nothing \
                 (the band is painted in this session's secret and must never be read back)"
            );
            return None;
        }
        let region: Rectangle<i32, Buffer> =
            Rectangle::new((x, y).into(), (w as i32, h as i32).into());
        let out = &mut self.output;
        match readback_region(&mut out.renderer, &mut out.output_framebuffer, region) {
            Ok(pixels) => Some((region, pixels)),
            Err(err) => {
                tracing::error!("consent-injector: occlusion-window readback failed: {err}");
                None
            }
        }
    }
}

impl HeadlessOutput {
    /// (Re)composite both retained images from `scene`.
    ///
    /// The single composition path: [`HeadlessView::redraw`] calls it with
    /// the live scene and [`Self::scrub_retained_frame`] calls it with an
    /// empty one, so "a scrubbed frame is what an unpainted realm looks
    /// like" is a property of the code rather than of two implementations
    /// that agree today.
    ///
    /// One [`Scene::compose`] call feeds both images, so the realm view an
    /// agent may capture and the output a human sees are the same
    /// composition, differing only by the overlay (module docs).
    fn present(&mut self, scene: &Scene) -> Result<(), Box<dyn Error>> {
        let (w, h) = (self.size.w.max(0) as u32, self.size.h.max(0) as u32);
        let view = scene.compose(w, h);
        composite(
            &mut self.renderer,
            &mut self.view_framebuffer,
            self.size,
            &view,
        )?;
        // The human-visible half, through the shared overlay step both
        // backends call. `view` is moved in rather than recomposed, so "the
        // two images differ only by the overlay" is a property of the code
        // and not of a comment.
        let output = super::human_visible_from_view(view, &mut self.consent, w, h);
        composite(
            &mut self.renderer,
            &mut self.output_framebuffer,
            self.size,
            &output,
        )
    }
}

impl crate::lifecycle::RetainedOutput for HeadlessOutput {
    /// Composite an **empty** [`Scene`] into the retained framebuffer, so
    /// the last frame the dead realm painted is gone from the one buffer
    /// [`Self::latest_frame_rgba`] reads back.
    ///
    /// An empty scene rather than a memset: [`Scene::compose`] on an empty
    /// scene *is* the deterministic background, so this reuses the single
    /// composition implementation instead of inventing a second idea of
    /// what an unpainted realm looks like. It deliberately does not touch
    /// `self.scene` -- the death funnel has already cleared that through
    /// `ShimServer::connection_closed`, and this type must not become a
    /// second place a realm's death is decided.
    ///
    /// This keeps [`Self::latest_frame_rgba`] a pure read: the recomposite
    /// happens on the death path, never on the capture path, so capture
    /// still observes "what the compositor last finished" and the goldens
    /// stay deterministic.
    ///
    /// **Both** retained images are scrubbed. The human-visible one matters
    /// less (no agent reads it) but leaving a dead realm's last frame on a
    /// virtual display would be the same staleness bug with a different
    /// audience, and a scrub that covered only one image would be a trap for
    /// whoever next reaches for the other.
    /// # A realm dying must not take a live prompt off the screen
    ///
    /// The human-visible image is scrubbed to
    /// `human_visible_from_view(empty_scene, consent)` — **not** to the empty
    /// scene — so a prompt that is up survives the death of a realm.
    ///
    /// Scrubbing it to the bare empty scene (as this first did) is
    /// unrecoverable rather than merely wrong: this backend recomposites only
    /// on a scene commit, which a dead realm never sends again, and a scrub
    /// does not bump [`ConsentSurface::generation`], so nothing downstream
    /// ever learns the display needs repainting. The prompt would be gone from
    /// the screen while the core still believed it was up — and under P1.7.2,
    /// where "on screen", "input grabbed" and "`consent_held` holds" become
    /// one moment, that is a session wedged behind an invisible dialog until
    /// the consent timeout fires.
    ///
    /// The petition being prompted need not belong to the realm that died;
    /// deciding that is [`crate::petitions`]' job, and this type must not
    /// become a second place a realm's death is decided (above). The scrub
    /// removes the dead realm's *pixels*, which is all it is for.
    fn scrub_retained_frame(&mut self) -> Result<(), Box<dyn Error>> {
        self.present(&Scene::new())
    }
}

/// Blit already-composed `pixels` into `framebuffer` at `size`, 1:1 and
/// upright.
///
/// Takes bytes rather than a [`Scene`] because it has two callers with two
/// different sources: [`HeadlessView::redraw`] fills the realm-view image
/// from [`Scene::compose`] and the output image from that same buffer with
/// the consent overlay applied. Passing a scene instead would mean composing
/// twice, and two composes are two chances for the capture path and the
/// human's display to disagree about the realm view.
///
/// Composition itself — layout, letterbox, background — happens in
/// [`Scene::compose`], the single implementation both backends present
/// (P1.3.3); this function only moves composed bytes into a retained image.
fn composite(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Image<'static, 'static>,
    size: Size<i32, Physical>,
    pixels: &[u8],
) -> Result<(), Box<dyn Error>> {
    if size.w <= 0 || size.h <= 0 {
        return Ok(());
    }
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();

    // Upload the composed view exactly as the nested backend does: tightly
    // packed RGBA8888 imported as DRM `ABGR8888` (bytes R,G,B,A on
    // little-endian).
    let texture = renderer.import_memory(pixels, Fourcc::Abgr8888, buffer_size, false)?;

    let full = Rectangle::from_size(size);
    let mut target = renderer.bind(framebuffer)?;
    // `Transform::Normal`, NOT `Flipped180`: a pixman image is a top-down CPU
    // buffer, so there is no GL bottom-up convention to undo. The composed
    // pixels are fully opaque (`Scene::compose` forces alpha, and the consent
    // overlay preserves it) and cover the whole output, so blitting them 1:1
    // (full texture -> full framebuffer, no scaling) leaves the framebuffer a
    // byte-exact identity of what was composed — no separate clear is needed,
    // and the P1.3.2/P1.3.3/P1.7.1 goldens assert exactly that.
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
    use super::{render_once, HeadlessView};
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

    // --- click-target's on-screen signature ------------------------------
    //
    // `shim/tests/click_target.c` paints, before any click lands, a solid
    // black field with exactly ONE centred green square of `TARGET_SIZE`
    // edge. Both colours are chosen there with channels that are multiples
    // of 0x11 and travel the shm path unscaled, so they arrive byte-exact
    // in the RGBA readback -- no tolerance, no image codec, no decoding
    // (plan risk R7: nothing but raw byte comparison in this crate).
    //
    // This shape is the discriminator the real-app occlusion proof needs.
    // "The view differs from the empty-scene test pattern" is NOT: the C
    // shim's own first commit -- an app-less realm view -- already differs
    // from it, so a wait on mere difference passes before any client has
    // attached. The test pattern is also, specifically, not excluded by a
    // green-pixel count alone: it contains a full-height green colour bar
    // and a green corner marker. What only click-target produces is the
    // conjunction below: black corners AND a single green region whose
    // bounding box is exactly the square's edge.

    /// click-target's `TARGET_SIZE`.
    const CLICK_TARGET_EDGE: u32 = 160;
    /// click-target's `BACKGROUND` (`0x000000`), as RGBA.
    const CLICK_TARGET_BG: [u8; 4] = [0x00, 0x00, 0x00, 0xff];
    /// click-target's `TARGET` (`0x00ff00`), as RGBA.
    const CLICK_TARGET_FG: [u8; 4] = [0x00, 0xff, 0x00, 0xff];

    /// What a candidate realm view looks like, measured against
    /// click-target's rendering. Kept as a struct rather than a bare
    /// `bool` so a timeout can say *how far off* the closest frame was --
    /// "0 green pixels, corners not black" and "25600 green pixels in a
    /// 160x160 box but corners not black" are very different bugs.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct ClickTargetSignature {
        /// All four corners are click-target's black field.
        corners_are_field: bool,
        /// Pixels exactly equal to click-target's green.
        target_pixels: u32,
        /// Bounding box of those pixels, `(w, h)`; `(0, 0)` if there are none.
        target_box: (u32, u32),
    }

    impl ClickTargetSignature {
        fn of(view: &[u8], w: u32, h: u32) -> Self {
            let at = |x: u32, y: u32| -> [u8; 4] {
                let o = (y as usize * w as usize + x as usize) * test_pattern::BYTES_PER_PIXEL;
                view[o..o + test_pattern::BYTES_PER_PIXEL]
                    .try_into()
                    .unwrap()
            };
            let corners_are_field = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
                .into_iter()
                .all(|(x, y)| at(x, y) == CLICK_TARGET_BG);
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
            let mut target_pixels = 0u32;
            for y in 0..h {
                for x in 0..w {
                    if at(x, y) == CLICK_TARGET_FG {
                        target_pixels += 1;
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            let target_box = if target_pixels == 0 {
                (0, 0)
            } else {
                (x1 - x0 + 1, y1 - y0 + 1)
            };
            Self {
                corners_are_field,
                target_pixels,
                target_box,
            }
        }

        /// True only for a frame click-target itself could have drawn.
        ///
        /// The square must be solid (every pixel of a `TARGET_SIZE`-edge
        /// box) and nothing outside it may be that green, which the
        /// count-equals-area check enforces jointly with the bounding box:
        /// a stray green pixel elsewhere grows the box, and a hole in the
        /// square lowers the count.
        fn is_click_target(self) -> bool {
            self.corners_are_field
                && self.target_box == (CLICK_TARGET_EDGE, CLICK_TARGET_EDGE)
                && self.target_pixels == CLICK_TARGET_EDGE * CLICK_TARGET_EDGE
        }

        /// The more app-like of two observations, for the timeout message.
        fn better_of(self, other: Self) -> Self {
            if (other.corners_are_field, other.target_pixels)
                > (self.corners_are_field, self.target_pixels)
            {
                other
            } else {
                self
            }
        }
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
    /// exercised on real [`HeadlessView`], not just the `render_once`
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
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");
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
    fn scrubbing_the_retained_frame_removes_the_dead_realms_pixels() {
        // The other half of `lifecycle`'s scrub property, on real pixels:
        // `lifecycle` proves the death funnel drives this seam, and this
        // proves the seam really erases what the shim painted.
        //
        // Without it, `latest_frame_rgba` -- documented as "the capture
        // service's actual pixel source" and deliberately a pure read that
        // never recomposites -- keeps returning the dead realm's last frame
        // byte for byte for the rest of the session, with nothing between
        // those bytes and an agent but the embedder remembering to gate on
        // `view_is_live`.
        use crate::lifecycle::RetainedOutput;
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 96;
        const VH: u32 = 64;
        const SW: u32 = 48;
        const SH: u32 = 32;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");

        let pixels = client_pixels(SW, SH);
        state.scene.commit(
            SurfaceContent::from_rgba(pixels.clone(), SW, SH).expect("well-formed content"),
        );
        state.redraw().expect("composite the committed surface");
        let painted = state.latest_frame_rgba().expect("readback");
        let background = test_pattern::render(VW, VH);
        assert_ne!(
            painted, background,
            "the realm must really have painted, or the scrub below proves nothing"
        );

        // The realm dies: the scene loses its surface (what the death
        // funnel does through `ShimServer::connection_closed`)...
        state.scene.clear_surface();
        assert_eq!(
            state.latest_frame_rgba().expect("readback"),
            painted,
            "clearing the scene alone must NOT change the retained frame -- that is exactly \
             the staleness the scrub exists for, and capture reads the retained frame"
        );

        // ...and the funnel scrubs the retained frame.
        state.output.scrub_retained_frame().expect("scrub");
        let scrubbed = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            scrubbed, background,
            "a scrubbed retained frame is the empty-scene background, byte for byte"
        );
        assert!(
            !scrubbed
                .windows(pixels.len().min(scrubbed.len()))
                .any(|w| w == &pixels[..w.len()]),
            "no run of the dead realm's committed pixels may survive in the framebuffer"
        );

        // Still a pure read: the scrub happened on the death path, so
        // capture does not recomposite and stays deterministic.
        assert_eq!(
            state.latest_frame_rgba().expect("second readback"),
            scrubbed,
            "capture must remain a pure read after a scrub"
        );
    }

    #[test]
    fn committed_shm_buffer_reaches_retained_framebuffer_and_capture() {
        use std::fs::File;
        use std::io::Write;
        use std::os::unix::fs::FileExt;

        use rustix::fs::MemfdFlags;
        use vitrin_ipc::Connection;
        use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format};

        use crate::capture::{render_frame, RealmViewFrame};
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
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");
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

        // The same buffer appears in a served capture: the capture
        // mechanics (which the enforcement chokepoint invokes for every
        // admitted capture_frame) read the same retained frame and deliver
        // its xrgb8888 form.
        let (mut server, mut client) = Connection::pair().expect("socketpair");
        let (rendered, _digest) = render_frame(&RealmViewFrame {
            rgba: &retained,
            width: VW,
            height: VH,
        })
        .expect("render the retained view");
        server
            .send_message(
                &rendered.encode(7),
                Some(std::os::fd::AsFd::as_fd(&rendered.fd)),
            )
            .expect("ship the frame");
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

    /// The P1.3.8 acceptance, cross-backend: the nested backend serves the
    /// **same bare-scene bytes** the headless backend does for an identical
    /// scene (issue #116).
    ///
    /// Both are [`Scene::compose`]. Headless retains it in a pixman image and
    /// reads it back; the nested backend composes it on the CPU on demand
    /// ([`crate::backend::winit::capture_pixels`]). This drives *both* real
    /// capture paths against one shared scene and asserts they are
    /// byte-identical — no winit window, no GL, no display, because pixman is
    /// CPU and `capture_pixels` needs no renderer at all. It is the concrete
    /// form of the transitive equality winit's
    /// `the_nested_capture_is_the_bare_realm_view_never_the_overlay` states,
    /// and it goes all the way through [`crate::capture::render_frame`] so the
    /// *delivered* wire bytes are pinned equal too, not merely the readback.
    #[test]
    fn nested_and_headless_captures_are_byte_identical() {
        use std::os::fd::OwnedFd;
        use std::os::unix::fs::FileExt;

        use crate::backend::winit::capture_pixels;
        use crate::capture::{render_frame, RealmViewFrame};
        use crate::consent::tests::prompt_fixture;
        use crate::consent::{ConsentSurface, TrustedIndicator};
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 320;
        const VH: u32 = 200;
        const SW: u32 = 160;
        const SH: u32 = 120;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state =
            HeadlessView::new(size, event_loop.get_signal(), TrustedIndicator::for_test())
                .expect("headless state under pixman");
        state
            .scene
            .commit(SurfaceContent::from_rgba(client_pixels(SW, SH), SW, SH).expect("content"));
        state.redraw().expect("composite the committed surface");

        // Headless capture: the retained pixman framebuffer, read back.
        let headless_capture = state.latest_frame_rgba().expect("headless readback");
        // Nested capture: the bare scene composed on the CPU, from the SAME
        // scene object that fed the headless readback above.
        let nested_capture = capture_pixels(&state.scene, size).expect("a nonzero view");

        // The P1.3.8 requirement: same scene, same pixels, byte for byte.
        assert_eq!(
            nested_capture, headless_capture,
            "nested and headless captures must be byte-identical for the same scene"
        );
        // ...and both are exactly the one shared composition.
        assert_eq!(nested_capture, state.scene.compose(VW, VH));

        // Overlay excluded on the nested side too: a prompt on the
        // human-visible composition changes it, but the nested capture is
        // unmoved — `capture_pixels` takes no `ConsentSurface`, so it *cannot*
        // carry a card.
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        consent.show_for_test(prompt_fixture());
        let human_visible = super::super::compose_human_visible(&state.scene, &mut consent, VW, VH);
        assert_ne!(
            nested_capture, human_visible,
            "the nested capture must exclude the consent overlay"
        );

        // The delivered artifact matches too: rendering each capture through
        // the real capture mechanics yields identical wire frames (format,
        // shape, and every byte).
        let render = |rgba: &[u8]| {
            render_frame(&RealmViewFrame {
                rgba,
                width: VW,
                height: VH,
            })
            .expect("render capture")
            .0
        };
        let headless_frame = render(&headless_capture);
        let nested_frame = render(&nested_capture);
        assert_eq!(
            (
                nested_frame.format,
                nested_frame.width,
                nested_frame.height,
                nested_frame.stride
            ),
            (
                headless_frame.format,
                headless_frame.width,
                headless_frame.height,
                headless_frame.stride
            ),
            "the served frame_ready contract must match across backends"
        );
        let read = |fd: OwnedFd, len: usize| {
            let file = std::fs::File::from(fd);
            let mut buf = vec![0u8; len];
            file.read_exact_at(&mut buf, 0).expect("read served frame");
            buf
        };
        let len = (nested_frame.stride * nested_frame.height) as usize;
        assert_eq!(
            read(nested_frame.fd, len),
            read(headless_frame.fd, len),
            "the served capture bytes must be identical across backends"
        );
    }

    /// The P1.7.1 acceptance criteria, both halves, on real backend pixels.
    ///
    /// The overlay must appear in the human-visible output **and** must be
    /// absent from what an agent captures. Those are not two views of one
    /// fact: they are two retained images, and this test reads both after a
    /// single redraw with a prompt up.
    ///
    /// The agent-side assertion goes all the way through
    /// [`crate::capture::render_frame`] rather than stopping at the readback,
    /// because that is the buffer that would actually be sealed into a memfd
    /// and handed over `SCM_RIGHTS`. Anything weaker would prove the retained
    /// image is clean while leaving the delivered artifact untested.
    #[test]
    fn a_prompt_reaches_human_visible_output_but_never_a_capture() {
        use crate::capture::{render_frame, RealmViewFrame};
        use crate::consent::tests::prompt_fixture;
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
        // Wide enough that the 560px card fits with realm view around it, so
        // "the overlay changed the output" is not an artifact of cropping.
        const VW: u32 = 800;
        const VH: u32 = 600;
        const SW: u32 = 400;
        const SH: u32 = 300;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");

        // A realm that has painted, so the test distinguishes "the overlay is
        // absent" from "nothing was ever drawn".
        state.scene.commit(
            SurfaceContent::from_rgba(client_pixels(SW, SH), SW, SH).expect("well-formed content"),
        );
        state.redraw().expect("composite the committed surface");
        let clean_view = state.latest_frame_rgba().expect("readback");
        let clean_output = state.latest_output_rgba().expect("readback");
        // With no prompt up the two retained images differ ONLY by the trusted
        // band (issue #85): it is on the human-visible output and never the
        // capture. Below the band, the two are the identical realm view.
        let band_bytes = crate::consent::TRUST_BAND_HEIGHT as usize
            * VW as usize
            * test_pattern::BYTES_PER_PIXEL;
        assert_eq!(
            clean_view[band_bytes..],
            clean_output[band_bytes..],
            "below the band, no-prompt output is the realm view unchanged"
        );
        let band_color = crate::consent::TrustedIndicator::for_test().color();
        assert_eq!(
            clean_output[..test_pattern::BYTES_PER_PIXEL],
            band_color,
            "the trusted band is on the human-visible output"
        );
        assert_ne!(
            clean_view[..test_pattern::BYTES_PER_PIXEL],
            band_color,
            "the trusted band is NEVER on the capture"
        );

        // The prompt goes up. This is the moment P1.7.2 will pair with
        // `mark_prompt_shown` + `vitrin_consent.state(shown)`.
        state.output.consent.show_for_test(prompt_fixture());
        state.redraw().expect("recomposite with the prompt up");

        // --- Human-visible side: the prompt is really on the display. ---
        let output = state.latest_output_rgba().expect("readback");
        assert_ne!(output, clean_output, "the prompt must change the output");
        let card = crate::consent::render::rasterize(&prompt_fixture());
        let (cx, cy) = state
            .output
            .consent
            .card_origin(VW, VH)
            .expect("a prompt is up, so the card has an origin");
        assert!(cx >= 0 && cy >= 0, "the card fits in an {VW}x{VH} view");
        for row in 0..card.height {
            let d = ((cy as u32 + row) as usize * VW as usize + cx as usize)
                * test_pattern::BYTES_PER_PIXEL;
            let s = row as usize * card.width as usize * test_pattern::BYTES_PER_PIXEL;
            let run = card.width as usize * test_pattern::BYTES_PER_PIXEL;
            assert_eq!(
                &output[d..d + run],
                &card.rgba[s..s + run],
                "card row {row} must appear verbatim in the human-visible output"
            );
        }
        // ...and the realm view around it is darkened, not erased. Checked at
        // the bottom-left — below the band (whose colour is identical prompt or
        // not) and outside the centered card, so this isolates the scrim.
        let bl = (VH as usize - 1) * VW as usize * test_pattern::BYTES_PER_PIXEL;
        assert_ne!(
            output[bl..bl + 4],
            clean_output[bl..bl + 4],
            "the scrim must apply"
        );
        assert!(output[bl] > 0 || output[bl + 1] > 0 || output[bl + 2] > 0);

        // --- Agent side: the capture is the realm view, unchanged. ---
        let view = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            view, clean_view,
            "a prompt being up must not move a single pixel of the realm view"
        );
        assert_eq!(
            view,
            state.scene.compose(VW, VH),
            "the capture source must be exactly Scene::compose -- the overlay \
             composites at the output stage, above this"
        );
        assert_ne!(view, output, "...and the two really do differ now");

        // The delivered artifact, not just the retained image: what a
        // `capture_frame` would seal into a memfd carries no overlay pixel.
        let (frame, _digest) = render_frame(&RealmViewFrame {
            rgba: &view,
            width: VW,
            height: VH,
        })
        .expect("render the retained view");
        let served = {
            use std::os::unix::fs::FileExt;
            let file = std::fs::File::from(frame.fd);
            let mut buf = vec![0u8; (frame.stride * frame.height) as usize];
            file.read_exact_at(&mut buf, 0).expect("read served frame");
            buf
        };
        let swizzle = |rgba: &[u8]| -> Vec<u8> {
            rgba.chunks_exact(4)
                .flat_map(|px| [px[2], px[1], px[0], 0xff])
                .collect()
        };
        assert_eq!(
            served,
            swizzle(&clean_view),
            "the served capture must be the overlay-free realm view"
        );
        assert_ne!(
            served,
            swizzle(&output),
            "the served capture must NOT be the human-visible output"
        );
        // Pixel-level: no run of the card's bytes survives anywhere in the
        // delivered frame. Catches a partial leak an equality check on the
        // whole buffer could not (a wrongly-offset blit, say).
        let card_row = &card.rgba[..card.width as usize * test_pattern::BYTES_PER_PIXEL];
        let card_row_wire = swizzle(card_row);
        assert!(
            !served
                .windows(card_row_wire.len())
                .any(|w| w == card_row_wire),
            "a row of consent-prompt pixels reached a capture"
        );

        // Taking the prompt down restores the human-visible output exactly.
        state.output.consent.dismiss_for_test();
        state.redraw().expect("recomposite with the prompt down");
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            clean_output,
            "a dismissed prompt must leave no pixels behind"
        );
    }

    /// The P1.7.1 occlusion proof (issue #109, M1.4 exit gate), against a
    /// REAL app: the consent overlay really occludes a real app's content on
    /// the human-visible side, and is really absent from the agent-visible
    /// capture -- proven against the real C shim (E6, P1.6.2) and a real
    /// `click-target` client, not the in-process mock shim every other test
    /// in this module drives.
    /// `a_prompt_reaches_human_visible_output_but_never_a_capture` above pins
    /// the identical property against a synthetic committed surface; this is
    /// its real-app counterpart, in the same spirit -- and following the
    /// exact same opt-in discipline -- as `shim.rs`'s
    /// `c_shim_conforms_to_the_real_core` cross-track check.
    ///
    /// **Opt-in, but never silently so under CI**, identically to that check:
    /// unset `VITRIN_C_SHIM_BIN` skips outside CI, and FAILS inside it unless
    /// `VITRIN_C_SHIM_CONFORMANCE_SKIP` declares the gap (the `rust` job,
    /// which has no C toolchain). The `conformance` job's
    /// `cargo test -p vitrin-core c_shim` filter picks this test up by name.
    /// Run it locally exactly as that test's doc says, with `click-target`
    /// (co-built with the shim, `shim/meson.build`) beside the built shim:
    ///
    /// ```text
    /// meson setup shim/build shim && meson compile -C shim/build
    /// VITRIN_C_SHIM_BIN=$PWD/shim/build/vitrin-shim cargo test -p vitrin-core c_shim
    /// ```
    #[test]
    fn c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture() {
        use crate::consent::tests::prompt_fixture;
        use crate::shim::{ShimConfig, ShimServer};

        let Some(shim_bin) = std::env::var_os("VITRIN_C_SHIM_BIN") else {
            assert!(
                std::env::var_os("CI").is_none()
                    || std::env::var_os("VITRIN_C_SHIM_CONFORMANCE_SKIP").is_some(),
                "VITRIN_C_SHIM_BIN is unset in CI, so the C shim was never built and this \
                 real-app occlusion check proved nothing. Build the shim and point the variable \
                 at it (see the `conformance` job in .github/workflows/ci.yml), or set \
                 VITRIN_C_SHIM_CONFORMANCE_SKIP=1 in a job that cannot build C."
            );
            eprintln!("skipping: set VITRIN_C_SHIM_BIN to the built shim/build/vitrin-shim");
            return;
        };
        let shim_bin = std::path::PathBuf::from(shim_bin);
        assert!(
            shim_bin.is_file(),
            "VITRIN_C_SHIM_BIN does not name a file: {}",
            shim_bin.display()
        );

        // `click-target` is co-built beside the shim unconditionally (a bare
        // wl_shm client, no optional dep -- `shim/meson.build`), resolved the
        // same way the Python real-app ladder resolves it
        // (`tests/integration/test_real_actuation.py`'s `_resolve_sibling`),
        // with the same override escape hatch for a nonstandard layout.
        let app_bin = std::env::var_os("VITRIN_CLICK_TARGET_APP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                shim_bin
                    .parent()
                    .expect("VITRIN_C_SHIM_BIN has a parent directory")
                    .join("click-target")
            });
        assert!(
            app_bin.is_file(),
            "no click-target beside the C shim ({}), and VITRIN_C_SHIM_BIN is set. It is \
             co-built with the shim (shim/meson.build); rebuild it, or set \
             VITRIN_CLICK_TARGET_APP.",
            shim_bin
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );

        let _fd = crate::capture::tests::fd_lock();
        let base = crate::spawn::tests::scratch();
        const VW: u32 = 640;
        const VH: u32 = 480;

        let paths = crate::spawn::SpawnPaths::under(&base, &shim_bin);
        let env_allow: Vec<String> = [
            "WLR_BACKENDS",
            "WLR_RENDERER",
            "WLR_RENDERER_ALLOW_SOFTWARE",
            "WLR_LIBINPUT_NO_DEVICES",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (mut recorder, _log) =
            crate::recorder::tests::scratch_recorder("c-shim-consent-occlusion");
        let realm = crate::realm::tests::realm_with_spawn(
            "realm-0",
            &app_bin,
            &["--run-ms".to_string(), "40000".to_string()],
            &env_allow,
        );
        let mut spawned =
            crate::spawn::spawn_realm_with_env(&realm, &paths, &mut recorder, |name| match name {
                "WLR_BACKENDS" => Some("headless".into()),
                "WLR_RENDERER" => Some("pixman".into()),
                "WLR_RENDERER_ALLOW_SOFTWARE" => Some("1".into()),
                "WLR_LIBINPUT_NO_DEVICES" => Some("1".into()),
                _ => None,
            })
            .expect("the C shim must spawn");

        let mut server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: VW,
            height: VH,
        });
        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");
        server
            .send_configure(&mut |frame| spawned.connection_mut().send_message(frame, None))
            .expect("configure is the core's first message");

        // A watchdog: the receive below is blocking, so a wedged real
        // shim/app must fail this test rather than hang the suite -- the
        // same posture `shim.rs`'s cross-track check takes. Its window is
        // deliberately WIDER than the content deadline the loop below
        // enforces: a shim that keeps talking but never paints the app is
        // the interesting failure, and it should be reported by that
        // loop's own diagnostic rather than as an anonymous SIGKILL.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog = {
            let done = std::sync::Arc::clone(&done);
            let pid = spawned.pid();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + 2 * crate::spawn::tests::DEADLINE;
                while std::time::Instant::now() < deadline {
                    if done.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            })
        };

        // Drive the real shim until click-target's REAL content lands --
        // not merely "a commit happened", and not merely "the view stopped
        // being the empty-scene test pattern" (the shim's own first commit
        // satisfies that before any client has attached, which would run
        // this whole occlusion proof against an app-less realm view). The
        // wait below blocks on a signature only click-target's rendering
        // produces, so the human-visible/capture split further down is
        // checked against real app pixels or not at all.
        let deadline = std::time::Instant::now() + crate::spawn::tests::DEADLINE;
        let mut commits = 0u32;
        let mut best = ClickTargetSignature::default();
        let clean_view = loop {
            let Some(msg) = spawned
                .connection_mut()
                .recv_message()
                .expect("the C shim's framing must be readable")
            else {
                panic!(
                    "the C shim closed the connection after {commits} commit(s) without \
                     click-target's content ever reaching the realm view (best seen: \
                     {best:?}); expected a solid {CLICK_TARGET_BG:?} field with one \
                     centred {CLICK_TARGET_EDGE}x{CLICK_TARGET_EDGE} {CLICK_TARGET_FG:?} \
                     square"
                );
            };
            let conn = spawned.connection_mut();
            let committed = server
                .handle_message(msg, &mut state.scene, None, &mut |frame| {
                    conn.send_message(frame, None)
                })
                .expect("the C shim must not violate the shim protocol");
            if committed {
                commits += 1;
                state.redraw().expect("composite the shim's commit");
                let view = state.latest_frame_rgba().expect("readback");
                let seen = ClickTargetSignature::of(&view, VW, VH);
                if seen.is_click_target() {
                    break view;
                }
                best = best.better_of(seen);
            }
            // A bounded wait that FAILS, never one that proceeds: an
            // app-less realm view must not be allowed to stand in for the
            // real app's pixels just because the shim kept talking. (The
            // blocking `recv_message` above cannot check a clock while it
            // waits; the watchdog thread covers that half by killing a
            // wedged shim, which surfaces as the panic above.)
            assert!(
                std::time::Instant::now() < deadline,
                "click-target's content never reached the realm view within {:?} \
                 ({commits} commit(s) composed; best seen: {best:?}). Expected a solid \
                 {CLICK_TARGET_BG:?} field with one centred \
                 {CLICK_TARGET_EDGE}x{CLICK_TARGET_EDGE} {CLICK_TARGET_FG:?} square \
                 (shim/tests/click_target.c). Proceeding would run the consent-occlusion \
                 proof against a realm view with no app in it.",
                crate::spawn::tests::DEADLINE
            );
        };
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        watchdog.join().expect("watchdog thread");
        let clean_output = state.latest_output_rgba().expect("readback");

        // The prompt goes up, over the real app's real content.
        state.output.consent.show_for_test(prompt_fixture());
        state.redraw().expect("recomposite with the prompt up");

        // --- Human-visible side: the prompt is really on the display, over
        // the real app. ---
        let output = state.latest_output_rgba().expect("readback");
        assert_ne!(
            output, clean_output,
            "a prompt over a real app's content must change the human-visible output"
        );
        let card = crate::consent::render::rasterize(&prompt_fixture());
        let (cx, cy) = state
            .output
            .consent
            .card_origin(VW, VH)
            .expect("a prompt is up, so the card has an origin");
        assert!(cx >= 0 && cy >= 0, "the card fits in a {VW}x{VH} view");
        for row in 0..card.height {
            let d = ((cy as u32 + row) as usize * VW as usize + cx as usize)
                * test_pattern::BYTES_PER_PIXEL;
            let s = row as usize * card.width as usize * test_pattern::BYTES_PER_PIXEL;
            let run = card.width as usize * test_pattern::BYTES_PER_PIXEL;
            assert_eq!(
                &output[d..d + run],
                &card.rgba[s..s + run],
                "card row {row} must appear verbatim in the human-visible output, over the \
                 real app"
            );
        }

        // --- Agent side: the capture is the real app's content, unmoved. ---
        let view = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            view, clean_view,
            "a prompt being up must not move a single pixel of the real app's captured content"
        );
        assert_ne!(view, output, "...and the two really do differ now");
        // Pixel-level: no run of the card's bytes survives anywhere in the
        // agent-visible capture -- catches a partial leak an equality check
        // on the whole buffer could not.
        let card_row = &card.rgba[..card.width as usize * test_pattern::BYTES_PER_PIXEL];
        assert!(
            !view.windows(card_row.len()).any(|w| w == card_row),
            "a row of consent-prompt pixels reached the capture of a real app"
        );

        // Orderly teardown of the real shim/app: SIGTERM (not SIGKILL) so
        // the shim reaps click-target itself (P1.5.2), exactly as
        // `c_shim_conforms_to_the_real_core` tears down.
        if let Some(pid) = rustix::process::Pid::from_raw(spawned.pid() as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        let _ = spawned.child_mut().wait();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The sharing proof for the output stage: the retained human-visible
    /// image is byte-for-byte [`super::compose_human_visible`]'s output, which
    /// is the same function the nested backend uploads as its window texture.
    /// GL presentation itself needs a display, so — exactly as P1.3.3 does for
    /// the realm view — CI proves the shared-seam half, and nested mode
    /// cannot drift from headless in what a human sees.
    #[test]
    fn human_visible_output_is_the_shared_compose_for_both_backends() {
        use crate::consent::tests::prompt_fixture;
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 700;
        const VH: u32 = 560;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");
        state
            .scene
            .commit(SurfaceContent::from_rgba(client_pixels(200, 150), 200, 150).expect("content"));

        for prompt_up in [false, true] {
            if prompt_up {
                state.output.consent.show_for_test(prompt_fixture());
            }
            state.redraw().expect("redraw");
            let mut expected =
                crate::consent::ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
            if prompt_up {
                expected.show_for_test(prompt_fixture());
            }
            assert_eq!(
                state.latest_output_rgba().expect("readback"),
                super::super::compose_human_visible(&state.scene, &mut expected, VW, VH),
                "retained output must be the shared compose (prompt_up = {prompt_up})"
            );
        }
    }

    /// A realm dying scrubs its pixels — and must not take a live consent
    /// prompt off the screen with them.
    ///
    /// The scrub exists because this backend recomposites only on a scene
    /// commit, which a dead realm never sends again. That is exactly why
    /// scrubbing the human-visible image to the bare empty scene would be
    /// unrecoverable: nothing would ever redraw the prompt, and the scrub does
    /// not bump [`ConsentSurface::generation`], so nothing downstream would
    /// learn that it needs to. The core would still believe the prompt was up.
    ///
    /// So this pins both halves after a scrub: the dead realm's pixels are
    /// gone from the capture source, and the human-visible image is still the
    /// shared composition *with the prompt on it*.
    #[test]
    fn a_scrub_clears_the_dead_realms_pixels_but_keeps_a_live_prompt_on_screen() {
        use crate::consent::tests::prompt_fixture;
        use crate::lifecycle::RetainedOutput;
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 800;
        const VH: u32 = 600;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
        )
        .expect("headless state under pixman");

        let painted = client_pixels(300, 200);
        state.scene.commit(
            SurfaceContent::from_rgba(painted.clone(), 300, 200).expect("well-formed content"),
        );
        state.output.consent.show_for_test(prompt_fixture());
        state
            .redraw()
            .expect("redraw with a prompt over a live realm");

        // The realm dies: the scene loses its surface (the death funnel), and
        // the funnel scrubs the retained images.
        state.scene.clear_surface();
        state.output.scrub_retained_frame().expect("scrub");

        // The capture source is the empty-scene background, byte for byte...
        let view = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            view,
            test_pattern::render(VW, VH),
            "a scrubbed realm view is the empty-scene background"
        );
        assert!(
            !view
                .windows(painted.len().min(view.len()))
                .any(|w| w == &painted[..w.len()]),
            "no run of the dead realm's pixels may survive in the capture source"
        );

        // ...and the human can still see and answer the prompt.
        assert!(
            state.output.consent.prompt().is_some(),
            "the scrub must not silently take the prompt down"
        );
        let mut expected =
            crate::consent::ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        expected.show_for_test(prompt_fixture());
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            super::super::compose_human_visible(&crate::scene::Scene::new(), &mut expected, VW, VH),
            "after a scrub the human-visible image must still be the shared \
             composition with the prompt on it"
        );
        // Belt and braces: the card really is painted, so the assertion above
        // cannot be satisfied by both sides having lost the overlay.
        let card = crate::consent::render::rasterize(&prompt_fixture());
        let (cx, cy) = state
            .output
            .consent
            .card_origin(VW, VH)
            .expect("a prompt is up");
        let output = state.latest_output_rgba().expect("readback");
        let row = (cy as u32 + card.height / 2) as usize;
        let d = (row * VW as usize + cx as usize) * test_pattern::BYTES_PER_PIXEL;
        let s = (card.height / 2) as usize * card.width as usize * test_pattern::BYTES_PER_PIXEL;
        let run = card.width as usize * test_pattern::BYTES_PER_PIXEL;
        assert_eq!(
            &output[d..d + run],
            &card.rgba[s..s + run],
            "a card row must appear verbatim in the post-scrub output"
        );
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
            headless: HeadlessView,
            server: Option<ShimServer>,
            /// The realm's input router: no intake feeds it headless (no
            /// physical source exists, structurally), but the teardown
            /// funnel (`connection_closed`) resets it alongside the scene
            /// — the embedder shape P1.5.2 inherits.
            router: crate::input::InputRouter<crate::input::NoopHook>,
            start: Instant,
            /// Readback of the retained framebuffer after each presentation.
            presented: Vec<Vec<u8>>,
        }

        let size: Size<i32, Physical> = (W as i32, H as i32).into();
        let mut event_loop: EventLoop<LoopState> = EventLoop::try_new().expect("event loop");
        let mut state = LoopState {
            headless: HeadlessView::new(
                size,
                event_loop.get_signal(),
                crate::consent::TrustedIndicator::for_test(),
            )
            .expect("headless state under pixman"),
            server: Some(ShimServer::new(ShimConfig {
                realm: "realm-0".into(),
                width: W,
                height: H,
            })),
            router: crate::input::InputRouter::new(crate::input::NoopHook),
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
                    // No dmabuf importer on the headless path (P1.3.5):
                    // pixman has no GPU import, so every dmabuf commit
                    // resolves as the designed shm-fallback event.
                    match server.handle_message(msg, &mut state.headless.scene, None, &mut send) {
                        Ok(false) => {}
                        Ok(true) => {
                            // Presentation, headless: the composite
                            // completing IS the output cadence ("or,
                            // headless, after it would have been").
                            //
                            // TEST-ONLY SHAPE — do not copy into the
                            // runtime loop. The runtime coalescer now
                            // exists and is the thing to copy instead:
                            // `session::dispatch_shim` marks the scene
                            // dirty here and `session::post_dispatch` does
                            // the one redraw + `presented` per dispatch
                            // round. Compositing
                            // synchronously per commit is what this pacing
                            // test needs (one readback per presented
                            // frame), but at runtime it would let a
                            // hostile shim buy a full-output composite per
                            // 12-byte repaint commit. The runtime wiring
                            // does coalesce (see the "Wiring" section of
                            // `crate::shim`'s module docs;
                            // `wants_presentation`/`presented` batch all
                            // owed frame_dones).
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
                    // reset the realm's seat state, and stop the loop.
                    if let Some(server) = state.server.take() {
                        server.connection_closed(
                            &mut state.headless.scene,
                            None,
                            &mut state.router,
                        );
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
