// SPDX-License-Identifier: MPL-2.0
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
//! - **A reading of the trusted band that carries no colour** (issue #139).
//!   `band` reports [`super::band_witness::BandReport`]: how many composites
//!   happened, how many of them moved a byte of the band's rows (zero), how
//!   many moved the client's own rows just below it, whether the
//!   human-visible frame still tracks the realm view outside the band, and a
//!   digest of *realm-view* pixels the agent may capture anyway. Every field
//!   is a constant function of the run, independent of the indicator's value
//!   — which is a stricter rule than "no pixels leave", and
//!   [`super::band_witness`] explains why the weaker rule is not enough.
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
//! # The physical-input injector (issue #212, `physical-input-injector`)
//!
//! Headless has **no input device**, structurally: `crate::input`'s
//! `SeatInput::physical` is private to that module and its only producer is
//! the nested backend's intake, so a plain build here cannot mint a
//! physical-origin event at all. That is why the router below stacks
//! [`NoopHook`] — there is no chord to hold and no prompt for a human to
//! click.
//!
//! WS-E.1.6's claim is precisely about where physical input goes versus where
//! an agent's actuation goes, so it has no mock-free gate without one. Under
//! the `physical-input-injector` cargo feature — never a deployment build,
//! same posture as the two above — **and only when the invocation also
//! carries `--physical-input-fd N`**, this backend adopts a second inherited
//! socketpair ([`crate::input::injector`]) on which a harness says `motion`,
//! `button`, `scroll` and `key`. Each becomes a host event handed to
//! `crate::input::intake_physical` (or `physical_key`), the same entry points
//! the nested backend's winit handler calls, and is then routed by the same
//! [`session::route_physical_turn`]. There is no second, weaker path.
//!
//! Two things it deliberately does **not** do. It does not stack a hook: the
//! router here still carries [`NoopHook`], so an injected event passes no
//! consent grab and no dead-man watcher, because inventing a hook stack no
//! backend actually runs would prove something about a configuration nobody
//! ships. And it does not pretend to be a human — the injected input is
//! physical-*tagged*, which is the whole point, and it is also why a build
//! carrying this feature has a runtime guarantee where a shipping build has a
//! compile-time one (`crate::input::injector`, and the feature's Cargo block).
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
use crate::grants::RealmId;
use crate::input::{InputRouter, NoopHook, PhysicalPresenceMap};

/// **This backend's hook stack**, and the one thing about it that is not
/// [`NoopHook`] (WS-E.1.7, issue #232).
///
/// A plain headless build stacks nothing: there is no chord to hold, no prompt
/// for a human to click, and — structurally — no physical input at all, since
/// `SeatInput::physical` is private to `crate::input` and this backend calls no
/// intake. A `physical-input-injector` build **does** have physical input, so
/// it stacks [`crate::attention::AttentionHook`] and nothing else: the
/// attention key is the one core-owned chord whose consequence *and* whose
/// trigger are both reachable from the injector channel, which is what gives
/// `tests/integration/test_attention.py` a mock-free gate.
///
/// It deliberately does **not** stack the consent grab or the dead-man watcher
/// here. Those two are nested-mode policies with their own injectors
/// (`consent-injector`, `dead-man-injector`), and inventing a hook stack no
/// backend actually runs would prove something about a configuration nobody
/// ships. What it costs is stated rather than implied: an attention press on
/// *this* backend meets no consent gate above it, so the "a prompt consumes the
/// chord" half of decision 6 is a nested-mode and unit-test property, exactly
/// as `preempted`'s detection half is.
///
/// Since WS-E.2.1 (issue #213) the injector build stacks
/// [`crate::clipboard::ClipboardHook`] **above** the attention hook as well, on
/// the same grounds: the clipboard chords' consequence *and* their trigger are
/// both reachable from the injector channel, which is what gives
/// `tests/integration/test_real_clipboard.py` a mock-free gate. The order
/// matches the nested backend's for the reason stated there — a modifier
/// matcher inside the attention hook would never see a Super press.
///
/// WS-E.2.4 (issue #216) adds [`crate::screenshot::ScreenshotHook`] between the
/// two, on identical grounds and in the nested backend's order: the screenshot
/// chord's trigger *and* its whole effect are reachable from the injector
/// channel, which is what gives `tests/integration/test_screenshot.py` a gate
/// that presses a real key rather than asserting the effect half alone. What is
/// **not** stacked here is still the consent grab, the dead-man watcher and the
/// lock — so "a lock suppresses the screenshot key" stays a nested-mode and
/// unit-test property, exactly as the attention key's equivalent does.
#[cfg(feature = "physical-input-injector")]
type HeadlessHook = crate::clipboard::ClipboardHook<
    crate::screenshot::ScreenshotHook<crate::attention::AttentionHook<NoopHook>>,
>;
#[cfg(not(feature = "physical-input-injector"))]
type HeadlessHook = NoopHook;

/// Build [`HeadlessHook`] on **the router's own clock cell**. Two bodies
/// rather than one with a `cfg` inside, so the plain build's router provably
/// carries no cell and no signal beyond what it already had.
///
/// The cell is a parameter rather than something this mints, and that is the
/// bug it exists to prevent: `route_physical_turn` advances the router's cell
/// through [`InputRouter::observe_at`], so a hook holding a *second* cell would
/// time every attention press at process start and the window would be
/// permanently expired by the time the chokepoint asked.
#[cfg(feature = "physical-input-injector")]
fn headless_hook(
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
    now: &std::rc::Rc<std::cell::Cell<std::time::Instant>>,
) -> HeadlessHook {
    crate::clipboard::ClipboardHook::new(
        std::rc::Rc::new(std::cell::RefCell::new(
            crate::clipboard::ClipboardSignal::new(clipboard)
                .expect("the clipboard chord pair was validated at startup"),
        )),
        crate::screenshot::ScreenshotHook::new(
            std::rc::Rc::new(std::cell::RefCell::new(
                crate::screenshot::ScreenshotSignal::new(screenshot)
                    .expect("the screenshot chord was validated at startup"),
            )),
            crate::attention::AttentionHook::new(
                std::rc::Rc::new(std::cell::RefCell::new(
                    crate::attention::AttentionSignal::new(attention),
                )),
                std::rc::Rc::clone(now),
                NoopHook,
            ),
        ),
    )
}

#[cfg(not(feature = "physical-input-injector"))]
fn headless_hook(_now: &std::rc::Rc<std::cell::Cell<std::time::Instant>>) -> HeadlessHook {
    NoopHook
}
use crate::recorder::Recorder;
use crate::scene::{RealmScenes, Scene};
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
///
/// # `agent_cursor`: why this backend's sprite is opt-in and nested's is not
///
/// `--agent-cursor` (D-019). The nested backend composites the agent cursor
/// sprite with no opt-in: a human is watching that window, and drawing nothing
/// there is the defect this whole change fixes. (Neither backend draws it for
/// a realm the output is not bound to -- `session::post_dispatch` withholds
/// the position, WS-E.1.3 -- which is a limit on *both* and not what this
/// argument is about.) Here it is **off unless asked for**,
/// and the reason is a real gate rather than caution:
/// [`super::band_witness`] measures this backend's human-visible framebuffer
/// against the realm view byte for byte, and
/// `tests/integration/test_real_trust_band.py` asserts `tracks_view == 1`
/// *after* a real `grant.pointer.click()` — whose move would put a sprite in
/// the human-visible buffer and nowhere else. A sprite on by default here
/// turns a mock-free milestone gate red for a cosmetic reason.
///
/// The flag exists rather than the feature being simply absent because
/// headless is the only backend CI can run, so it is the only place the
/// capture-exclusion property can be *proved* on real composited pixels
/// (`the_agent_cursor_reaches_human_visible_output_but_never_a_capture`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    size: (u32, u32),
    dead_man: DeadManConfig,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    attention: crate::attention::AttentionChord,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    clipboard: crate::chord::Trigger,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    screenshot: crate::chord::ModChord,
    agent_cursor: bool,
    status: crate::status::StatusConfig,
    #[cfg(feature = "consent-injector")] consent_injector_fd: Option<std::os::fd::RawFd>,
    #[cfg(feature = "physical-input-injector")] physical_input_fd: Option<std::os::fd::RawFd>,
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
        attention,
        clipboard,
        screenshot,
        agent_cursor,
        status,
        #[cfg(feature = "consent-injector")]
        consent_injector_fd,
        #[cfg(feature = "physical-input-injector")]
        physical_input_fd,
        &mut seed,
        &mut recovered,
    );
    let recorder = recovered
        .or_else(|| seed.take().map(|seed| seed.recorder))
        .expect("the seed is either still unconsumed or its recorder was recovered");
    (recorder, result)
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    size: (u32, u32),
    #[cfg_attr(not(feature = "dead-man-injector"), allow(unused_variables))]
    dead_man: DeadManConfig,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    attention: crate::attention::AttentionChord,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    clipboard: crate::chord::Trigger,
    #[cfg_attr(not(feature = "physical-input-injector"), allow(unused_variables))]
    screenshot: crate::chord::ModChord,
    agent_cursor: bool,
    status: crate::status::StatusConfig,
    #[cfg(feature = "consent-injector")] consent_injector_fd: Option<std::os::fd::RawFd>,
    #[cfg(feature = "physical-input-injector")] physical_input_fd: Option<std::os::fd::RawFd>,
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
    // The physical-input channel (issue #212), adopted on the same terms and
    // for the same reason a failure is a **startup error**: a session started
    // with a hook that is not there would look instrumented from the outside
    // and behave as a plain one.
    #[cfg(feature = "physical-input-injector")]
    let physical_input = match physical_input_fd {
        Some(number) => {
            Some(crate::input::injector::Injector::adopt(number).map_err(Box::<dyn Error>::from)?)
        }
        None => None,
    };
    if agent_cursor {
        info!(
            "--agent-cursor: the agent cursor sprite will be composited into this run's \
             human-visible output (never a capture). Off by default on this backend because \
             its human-visible framebuffer is measured against the realm view (issue #139)."
        );
    }
    let view = HeadlessView::new(
        physical_size,
        event_loop.get_signal(),
        indicator,
        agent_cursor,
        status,
    )?;
    let now_cell = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
    let mut state = HeadlessState {
        view,
        // Headless stacks **no policy hook** — there is no chord to hold and
        // no prompt for a human to click — but it does carry the presence tap,
        // because `InputRouter` carries one unconditionally: the chokepoint's
        // `preempted` judgement reads the map the router writes, and an
        // optional tap is what made `preempted` unreachable in every shipped
        // build up to issue #212's review. It is fed only in a
        // `physical-input-injector` build, which is what lets
        // `tests/integration/test_input_switch.py` prove the per-realm
        // narrowing mock-free instead of by unit test alone.
        runtime: Runtime::new(
            seed.take().expect("the seed is consumed exactly once"),
            InputRouter::new(
                std::rc::Rc::new(std::cell::RefCell::new(PhysicalPresenceMap::new())),
                // The turn's instant, shared with whatever the stack carries:
                // `route_physical_turn` is the one writer
                // (`InputRouter::observe_at`), and a hook holding a second
                // cell would judge every press against process start.
                std::rc::Rc::clone(&now_cell),
                headless_hook(
                    #[cfg(feature = "physical-input-injector")]
                    attention,
                    #[cfg(feature = "physical-input-injector")]
                    clipboard,
                    #[cfg(feature = "physical-input-injector")]
                    screenshot,
                    &now_cell,
                ),
            ),
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
        #[cfg(feature = "physical-input-injector")]
        physical_input,
        #[cfg(feature = "physical-input-injector")]
        attention_chord: attention,
        #[cfg(feature = "physical-input-injector")]
        clipboard_trigger: clipboard,
        #[cfg(feature = "physical-input-injector")]
        screenshot_chord: screenshot,
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

    // Readiness for the physical-input channel, the same shape as the consent
    // one above and for the same borrow reason: the `Injector` lives in the
    // state, so the source carries a duplicate descriptor used for nothing but
    // `poll`.
    #[cfg(feature = "physical-input-injector")]
    if let Some(injector) = state.physical_input.as_ref() {
        let poll_fd = rustix::io::fcntl_dupfd_cloexec(injector.as_fd(), 3)?;
        loop_handle.insert_source(
            calloop::generic::Generic::new(poll_fd, calloop::Interest::READ, calloop::Mode::Level),
            |_readiness, _fd, state: &mut HeadlessState| {
                // EOF or a protocol violation removes the source; the core
                // keeps running with no way to inject, which is the
                // fail-closed direction.
                Ok(if state.service_physical_input() {
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

    // The status strip's tick (WS-E.2.3, issue #215), armed only for a session
    // that asked for one: the strip's clock has to move on a session where
    // nothing else is happening, and the loop otherwise has no reason to wake.
    if status.enabled {
        if let Err(err) = session::arm_status_tick(&loop_handle) {
            *recovered = Some(state.runtime.into_recorder());
            return Err(err);
        }
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
    runtime: Runtime<HeadlessHook>,
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
    /// The adopted physical-input channel (issue #212, module docs), or
    /// `None` when the run named no `--physical-input-fd`.
    ///
    /// A fifth disjoint field for the same reason the grab is a fourth: it is
    /// borrowed while `runtime` and `view.scenes` are, by the turn that routes
    /// what it produced.
    #[cfg(feature = "physical-input-injector")]
    physical_input: Option<crate::input::injector::Injector>,
    /// This run's configured attention chord (WS-E.1.7), so the channel's
    /// `attention` line presses the key the operator actually chose rather
    /// than a scancode the harness guessed. The same value the hook above was
    /// built with, by construction: both come from `run_inner`'s one argument.
    #[cfg(feature = "physical-input-injector")]
    attention_chord: crate::attention::AttentionChord,
    /// This run's configured clipboard trigger key (WS-E.2.1), so the channel's
    /// `clipboard` line chords the key the operator actually chose rather than
    /// a scancode the harness guessed.
    #[cfg(feature = "physical-input-injector")]
    clipboard_trigger: crate::chord::Trigger,
    /// This run's configured screenshot chord (WS-E.2.4), so the channel's
    /// `screenshot` line presses the gesture the operator actually chose.
    #[cfg(feature = "physical-input-injector")]
    screenshot_chord: crate::chord::ModChord,
}

/// The `physical-input-injector` build's channel service (issue #212): read
/// the peer's requests, turn each into physical-tagged intake through the
/// production entry point, and route the turn.
#[cfg(feature = "physical-input-injector")]
impl HeadlessState {
    /// Drain everything the peer has written. Returns `false` once the
    /// channel is finished, so the caller removes the calloop source.
    ///
    /// Every accepted request is routed through
    /// [`session::route_physical_turn`] — the same function the nested
    /// backend's input handler tails into, which binds the human's attention
    /// to the realm the output shows, pays the realm being left whatever it is
    /// owed, maps through that realm's geometry and delivers. `switch` is
    /// `None`: this backend stacks no dead-man watcher, so there is no replay
    /// to drain (see [`crate::backend::winit::route_turn`]).
    ///
    /// The reply is the number of `SeatInput`s the intake produced, never the
    /// number delivered: whether an event reaches an app is the router's and
    /// the shim's business, and a channel that reported delivery would be the
    /// core agreeing with itself about the very thing a gate here measures.
    fn service_physical_input(&mut self) -> bool {
        let Some(mut injector) = self.physical_input.take() else {
            return false;
        };
        let batch = injector.poll_requests();
        let alive = batch.is_ok();
        // One clock sample for the whole batch, before any of it is routed —
        // the same discipline `NestedState::handle_input` follows, and what
        // makes the presence tap's `note` and the chokepoint's `owns_target`
        // read one timeline. `route_physical_turn` pushes it into the tap.
        let now = std::time::Instant::now();
        let view = session::Presenter::view_size(&self.view);
        for parsed in batch.unwrap_or_default() {
            let Some(request) = parsed else {
                injector.reject("unknown request");
                continue;
            };
            let inputs = crate::input::injector::intake(
                request,
                (view.0 as i32, view.1 as i32),
                self.attention_chord,
                self.clipboard_trigger,
                self.screenshot_chord,
            );
            let produced = inputs.len();
            session::route_physical_turn(
                &mut self.runtime,
                &self.view.scenes,
                None,
                inputs,
                view,
                now,
            );
            injector.ack(produced);
        }
        if alive {
            self.physical_input = Some(injector);
        }
        alive
    }
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
                Some(Request::Band) => self.answer_band(),
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

    /// Answer `band`: recomposite, then report the trusted-band witness
    /// (issue #139).
    ///
    /// The recomposite comes first for the same reason [`Self::answer_describe`]
    /// does one — the reply then describes the frame that is on the virtual
    /// display *now*, so a harness needs no poll loop and can cross-check
    /// [`BandReport::probe_fnv`] against the `--capture-dump` of the same
    /// instant. It costs one extra composite per request, in a build that never
    /// ships.
    ///
    /// **No descriptor, no pixels, and nothing that moves with the session
    /// secret.** The reply is [`BandReport`]'s ten ASCII fields and nothing
    /// else; the argument that this is a stricter rule than "no pixels", and
    /// the near-miss that motivates it, are in [`super::band_witness`]. There
    /// is deliberately no fail-closed table here because there is nothing to
    /// fail closed *about*: the request names no resource, takes no argument,
    /// and confers nothing.
    ///
    /// [`BandReport`]: super::band_witness::BandReport
    /// [`BandReport::probe_fnv`]: super::band_witness::BandReport::probe_fnv
    fn answer_band(&mut self) {
        if let Err(err) = self.view.redraw() {
            tracing::error!("consent-injector: band could not recomposite: {err}");
        }
        let report = self.view.output.band_witness.report();
        if let Some(injector) = self.injector.as_mut() {
            injector.send_line(&format!("band {report}"), None);
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
                    let (peer_pid, peer_uid) = self
                        .injector
                        .as_ref()
                        .map(|inj| inj.peer_cred())
                        .unwrap_or((None, 0));
                    tracing::warn!(
                        %petition,
                        choice = choice.label(),
                        peer_pid = ?peer_pid,
                        peer_uid,
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
    type Hook = HeadlessHook;
    type View = HeadlessView;

    fn split(&mut self) -> (&mut Runtime<HeadlessHook>, &mut HeadlessView) {
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
            // This build has no panel and no seat at all, so there is nothing
            // that could take a screen away mid-run: the injector *is* the
            // human here, and it is reachable for as long as the process runs.
            // `--headless` with `--consent=interactive` is refused at startup
            // for the un-instrumented case (`HEADLESS_INTERACTIVE_REFUSAL`).
            session::PromptVisibility::Reachable,
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
    fn scene_mut(&mut self, realm: &RealmId) -> &mut Scene {
        self.scenes.scene_mut(realm)
    }

    fn scene(&self, realm: &RealmId) -> Option<&Scene> {
        self.scenes.scene(realm)
    }

    fn focused(&self) -> Option<&RealmId> {
        self.scenes.focused()
    }

    fn bind_output(&mut self, realm: &RealmId) {
        self.scenes.bind(realm);
    }

    fn unbind_output(&mut self) {
        self.scenes.unbind();
    }

    fn view_size(&self) -> (u32, u32) {
        let size = self.output.size;
        (size.w.max(0) as u32, size.h.max(0) as u32)
    }

    /// Never called in production: this backend's virtual output is fixed at
    /// construction and has no resize event to raise one. Implemented rather
    /// than made to panic because "the output never resizes" is a property of
    /// *this* backend, not of the trait, and a backend that grew a resizable
    /// virtual output should get the same propagation the nested one does.
    fn set_view_size(&mut self, size: (u32, u32)) {
        self.scenes.set_view_size(size);
    }

    /// Always [`Presentation::Completed`]: this backend composites
    /// synchronously into its retained framebuffer, so the composite finishing
    /// *is* the output cadence and any owed `frame_done` is due on return.
    fn redraw(&mut self) -> Result<session::Presentation, Box<dyn Error>> {
        HeadlessView::redraw(self)?;
        Ok(session::Presentation::Completed)
    }

    /// **This realm's** latest completed view, tightly packed.
    ///
    /// Two sources, one composition (WS-E.1.3):
    ///
    /// - **The bound realm** reads back the retained view framebuffer — the
    ///   exact bytes this backend last composited for the output. Unchanged
    ///   from before this issue, which is what keeps the P1.3.2/P1.3.6
    ///   goldens and the M1.3 fidelity gate byte-exact.
    /// - **A hidden realm** composes its own scene at the same view size.
    ///   That is not a second implementation: this backend's own tests pin
    ///   the retained readback as a byte-for-byte identity of
    ///   [`Scene::compose`] (`retained_framebuffer_is_the_capture_source`),
    ///   so the two paths are the same function reached two ways — the
    ///   identical argument that lets the nested backend serve captures at
    ///   all (P1.3.8).
    ///
    /// A readback failure yields `None` rather than an error: this is called
    /// on the redraw path, where the alternative is tearing down a session
    /// over a transient mapping failure, and `None` degrades to the
    /// chokepoint's `no_surface` refusal — the same answer a capture gets
    /// before any surface exists. Never the *output* image: that one carries
    /// the consent overlay, which no capture may ever contain.
    fn view_rgba(&mut self, realm: &RealmId) -> Option<Vec<u8>> {
        if self.scenes.focused() == Some(realm) {
            return self.latest_frame_rgba().ok();
        }
        let (w, h) = (
            self.output.size.w.max(0) as u32,
            self.output.size.h.max(0) as u32,
        );
        if w == 0 || h == 0 {
            return None;
        }
        Some(self.scenes.scene(realm)?.compose(w, h))
    }

    /// Take the router's agent-owned position for the sprite (D-019) — but
    /// only on a run that asked for one.
    ///
    /// With `--agent-cursor` absent this answers `false` and stores nothing,
    /// so the composite is byte-for-byte what it was before this existed and
    /// the dispatch round pays nothing. That is what keeps
    /// `tests/integration/test_real_trust_band.py` green **unchanged**: its
    /// `tracks_view == 1` assertion is taken after a real
    /// `grant.pointer.click()`, which sends a move first, so a sprite drawn by
    /// default here would fail a mock-free milestone gate.
    ///
    /// With the flag present it behaves exactly as the nested backend's does,
    /// which is the point: the opt-in exists so a test can enable the sprite
    /// on the one backend CI can run and prove it reaches human-visible output
    /// and never a capture.
    fn set_agent_cursor(&mut self, pos: Option<(f64, f64)>) -> bool {
        if !self.output.draw_agent_cursor {
            return false;
        }
        let quantize =
            |pos: Option<(f64, f64)>| pos.and_then(|(x, y)| crate::cursor::hotspot(x, y));
        if quantize(self.output.agent_cursor) == quantize(pos) {
            return false;
        }
        self.output.agent_cursor = pos;
        true
    }

    /// The attention marker, on this backend too (WS-E.1.7). Unlike the agent
    /// cursor this needs no flag: it is drawn in
    /// [`super::human_visible_from_view`], the *shared* output-stage fork both
    /// backends reach, so gating it here would make the two backends present
    /// different human-visible output for the same session state -- which is
    /// the drift that step exists to prevent. A plain headless build never
    /// sees a `true` anyway: nothing can press a physical chord there.
    fn set_attention(&mut self, open: bool) -> bool {
        if self.output.attention == open {
            return false;
        }
        self.output.attention = open;
        true
    }

    /// The status strip (WS-E.2.3), on this backend too and for the reason the
    /// attention marker needs no flag here: it is composited in
    /// [`super::human_visible_from_view`], the *shared* output-stage fork, so a
    /// backend that withheld it would make the two backends present different
    /// human-visible output for the same session state. What is gated is
    /// `--status` itself, in the [`crate::status::StatusStrip`] both backends
    /// hold; a headless session without the flag samples nothing.
    ///
    /// The realm handed to the sampler is **the one bound to the output**, not
    /// the router's: the strip names the realm in the picture the human is
    /// looking at, and naming a hidden one would be a caption for a different
    /// image.
    fn refresh_status(&mut self, now: std::time::SystemTime, mono: std::time::Instant) -> bool {
        self.output.status.refresh(now, mono, self.scenes.focused())
    }

    /// All three, lent to `f`: the scene and retained image from the two
    /// fields the struct was split into for exactly this call (see
    /// [`HeadlessView::output`]); no importer, since this backend has no GPU
    /// renderer at all — [`session::Presenter::scene_and_importer`]'s
    /// default already answers `None` for the same reason, restated here
    /// because `teardown_view` carries no default.
    fn teardown_view<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(
            &'v mut Scene,
            Option<&'v mut dyn crate::lifecycle::RetainedOutput>,
            Option<&'v mut dyn crate::dmabuf::DmabufImporter>,
        ) -> R,
    ) -> R {
        f(self.scenes.scene_mut(realm), Some(&mut self.output), None)
    }
}

/// Per-run state of the headless backend: the software renderer, the realm's
/// [`Scene`], the consent surface, and the two retained images the module
/// docs describe.
pub(crate) struct HeadlessView {
    /// **Every realm's scene, and the one bound to the output** (P1.3.3;
    /// WS-E.1.3, issue #209). Each scene is a single-maximized client
    /// surface, or the deterministic background when that realm has
    /// committed nothing. The shim-facing protocol server (P1.3.4) commits
    /// into the committing realm's own scene and calls
    /// [`redraw`](Self::redraw), which composites the **bound** one; the
    /// realm object (P1.5.1) hangs off it.
    scenes: RealmScenes,
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
    /// The lock surface (WS-E.2.2, issue #214). **Always present and
    /// permanently empty on this backend**, and that is the honest shape rather
    /// than a `cfg`: [`super::human_visible_from_view`] is the one
    /// overlay-application step both backends reach, so the parameter has to be
    /// *something*, and a backend-shaped `Option` would make "both backends
    /// composite the same way" true only by inspection.
    ///
    /// Nothing raises it here. A headless session has no physical input device
    /// ([`crate::input`]: `SeatInput::physical` is private and this backend
    /// calls no intake), so a lock it raised could never be dismissed — which is
    /// why `main` refuses every `--lock-*` flag with `--headless` at startup
    /// rather than arming a wedge. Its *compositing* is still exercised here, by
    /// this module's own tests, which is what gives "the lock reaches
    /// human-visible output and never a capture" a home on the one backend CI
    /// can run (D-019(4)).
    lock: crate::lock::LockSurface,
    /// The idle blank's cover (WS-E.4.3, issue #223), **permanently lowered on
    /// this backend**.
    ///
    /// `--blank-idle` is refused with `--headless`
    /// ([`super::blank::BLANK_NEEDS_THE_OUTPUT`]) for a reason this backend
    /// makes obvious: there is no display to power down, and -- decisively --
    /// no lock gate in this backend's hook stack, so nothing would ever write
    /// the activity clock a blank is postponed and woken by. A headless session
    /// that accepted the flag would go dark after the timeout and never come
    /// back.
    ///
    /// The surface is still held and still composited through, on
    /// [`Self::lock`]'s terms: the shared output stage takes the cover as a
    /// required parameter precisely so a backend cannot answer "I have none".
    blank: super::blank::BlankSurface,
    /// The composed **realm view**, retained across the process lifetime
    /// (PRD Doc 2 §9) so an internal capture reads composited pixels, not a
    /// freshly cleared buffer. Overlay-free by construction — this is what
    /// [`HeadlessView::latest_frame_rgba`] serves to agents.
    view_framebuffer: Image<'static, 'static>,
    /// The virtual display's **human-visible output**: the realm view with
    /// the consent overlay on top. Never read by the capture path.
    output_framebuffer: Image<'static, 'static>,
    /// Whether the human's **attention window** is open right now (WS-E.1.7),
    /// offered once per dispatch round by `Presenter::set_attention` and read
    /// by [`Self::present`], which passes it to the shared output-stage fork.
    ///
    /// Presentation state about the *human*, not about a realm's content, so
    /// [`Self::scrub_retained_frame`] leaves it alone: a realm dying must not
    /// silently change what the human is being told about their own window.
    attention: bool,
    size: Size<i32, Physical>,
    /// The trusted-band witness (issue #139), on a `consent-injector` build
    /// only. Reads the two buffers [`Self::present`] already holds and keeps
    /// counters; it exports no pixel and nothing that moves with the session
    /// secret (see [`super::band_witness`]).
    #[cfg(feature = "consent-injector")]
    band_witness: super::band_witness::BandWitness,
    /// Whether this run composites the agent cursor sprite into its
    /// human-visible output (`--agent-cursor`, D-019). **`false` by default,
    /// and that default is not laziness** — see [`run`]'s `agent_cursor`
    /// argument for the whole argument. In short: this backend's
    /// human-visible framebuffer is *measured*, byte for byte against the
    /// realm view, by [`super::band_witness`] and by
    /// `tests/integration/test_real_trust_band.py`, and that gate clicks a
    /// real pointer. A sprite on by default here would turn a mock-free
    /// milestone gate red for a cosmetic reason.
    draw_agent_cursor: bool,
    /// The agent-owned pointer position the sprite is drawn at, pushed by
    /// [`session::Presenter::set_agent_cursor`]. Always `None` unless
    /// [`Self::draw_agent_cursor`] is set: with the flag off nothing is
    /// stored, so "the flag is absent" is true of the composite and not only
    /// of the parser.
    agent_cursor: Option<(f64, f64)>,
    /// The status strip (WS-E.2.3, issue #215). Always present and composited
    /// through the shared output-stage fork, exactly like the lock surface; a
    /// session without `--status` holds one that is off, samples nothing and
    /// draws nothing.
    ///
    /// **Off by default here for the same measured reason `draw_agent_cursor`
    /// is**: this backend's human-visible framebuffer is compared byte for byte
    /// against the realm view by [`super::band_witness`] and by
    /// `tests/integration/test_real_trust_band.py`, and a clock ticking in it
    /// by default would make that gate a function of wall-clock time.
    status: crate::status::StatusStrip,
}

impl HeadlessView {
    fn new(
        size: Size<i32, Physical>,
        loop_signal: LoopSignal,
        indicator: TrustedIndicator,
        agent_cursor: bool,
        status: crate::status::StatusConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let mut renderer = PixmanRenderer::new()?;
        // `.max(0)` is defensive only: `run` always passes a positive size
        // (the CLI parser rejects zero/negative), but a degenerate size must
        // never wrap to a huge allocation on the `i32 -> usize` cast.
        let buffer_size: Size<i32, Buffer> = (size.w.max(0), size.h.max(0)).into();
        let view_framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        let output_framebuffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        Ok(Self {
            // The virtual output never resizes, so this size is the one every
            // realm's view is ever composed at and `set_view_size` has nothing
            // further to say on this backend.
            scenes: RealmScenes::new((size.w.max(0) as u32, size.h.max(0) as u32)),
            output: HeadlessOutput {
                renderer,
                consent: ConsentSurface::new(indicator),
                lock: crate::lock::LockSurface::new(indicator),
                blank: super::blank::BlankSurface::new(),
                view_framebuffer,
                output_framebuffer,
                size,
                #[cfg(feature = "consent-injector")]
                band_witness: super::band_witness::BandWitness::new(),
                draw_agent_cursor: agent_cursor,
                agent_cursor: None,
                attention: false,
                status: crate::status::StatusStrip::new(status),
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
        // **The bound realm's** scene, and never a hidden one's: this is the
        // output, and the output shows one realm (WS-E.1.3 decision 3). With
        // nothing bound it is a permanently empty scene, which composes the
        // documented deterministic background.
        self.output
            .present(self.scenes.bound(), self.scenes.focused())
    }

    /// The single realm the pre-WS-E.1.3 tests in this module drive:
    /// `realm-0`, bound to the output and minted on first use.
    ///
    /// A named test helper rather than a `scene` field so those tests keep
    /// reading as "the realm commits, the output shows it" while the
    /// multi-realm tests below have to name their realms explicitly. Binding
    /// here is what makes it the *output's* realm, which is what every one of
    /// them then asserts about.
    #[cfg(test)]
    fn scene_for_test(&mut self) -> &mut Scene {
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        self.scenes.bind(&realm);
        self.scenes.scene_mut(&realm)
    }

    /// The scene the output composites — the read-only half of
    /// [`Self::scene_for_test`].
    #[cfg(test)]
    fn bound_scene(&self) -> &Scene {
        self.scenes.bound()
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
    fn present(
        &mut self,
        scene: &Scene,
        // The realm `scene` belongs to, for the trusted-band witness's report
        // (issue #139 + WS-E.1.3): every pixel-derived field it exports is a
        // statement about one realm's view, and it must say which. Unused
        // outside a `consent-injector` build, where the witness does not
        // exist — the underscore is the honest spelling of that, not a
        // dropped argument.
        #[cfg_attr(not(feature = "consent-injector"), allow(unused_variables))] realm: Option<
            &RealmId,
        >,
    ) -> Result<(), Box<dyn Error>> {
        let (w, h) = (self.size.w.max(0) as u32, self.size.h.max(0) as u32);
        let view = scene.compose(w, h);
        composite(
            &mut self.renderer,
            &mut self.view_framebuffer,
            self.size,
            &view,
        )?;
        // The realm view is moved into the overlay step below, and the witness
        // needs both sides of that step; a `consent-injector` build keeps a
        // copy, a shipping build does not exist to keep one.
        #[cfg(feature = "consent-injector")]
        let witnessed_view = view.clone();
        // The human-visible half, through the shared overlay step both
        // backends call. `view` is moved in rather than recomposed, so "the
        // two images differ only by the overlay" is a property of the code
        // and not of a comment.
        let mut output = super::human_visible_from_view(
            view,
            &mut self.consent,
            &mut self.lock,
            &self.blank,
            &mut self.status,
            w,
            h,
            self.attention,
        );
        // The agent cursor sprite (D-019), opt-in here and only here: the
        // nested backend always draws it, this one draws it when the run
        // passed `--agent-cursor` (see [`Self::draw_agent_cursor`]). Applied
        // to the human-visible buffer only, downstream of the composite the
        // capture reads — the same fork the consent overlay obeys — so no
        // `vitrin_view.frame_ready` can carry it.
        //
        // **Before the witness, deliberately.** The witness reports on the
        // bytes that are about to be presented; measuring it before this draw
        // and presenting after would let the core report a human-visible
        // frame it did not present, which is the exact class of dishonesty
        // issue #139's counters exist to remove. With the flag on, the
        // witness's `tracks_view` reads 0 and that is the truth about the
        // frame.
        if let Some((x, y)) = self.agent_cursor {
            crate::cursor::composite_agent_cursor(&mut output, w, h, x, y);
        }
        // Issue #139: measured here, on the bytes that are about to be
        // presented, rather than on a second composition — a witness that
        // composed its own frame would agree with this one today and could
        // drift from it tomorrow, which is the whole failure mode it exists to
        // catch. Before the blit, because the blit consumes nothing but is the
        // step that could fail, and a frame the witness saw but the display
        // did not is the safer of the two asymmetries: it can only over-report
        // composites, never miss one that reached the screen.
        #[cfg(feature = "consent-injector")]
        self.band_witness
            .observe(&witnessed_view, &output, w, h, self.status.height(), realm);
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
        // No realm: this composites an *empty* scene, which belongs to no
        // realm at all. Naming the dying realm here would tell the witness
        // that realm's view is what the output now tracks, which is the
        // opposite of what a scrub means.
        self.present(&Scene::new(), None)
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

    /// A lock surface with nothing raised, for composite assertions about the
    /// *other* overlays. A fresh one per call: [`crate::lock::LockSurface`]
    /// carries a generation counter and a raster cache, so a shared instance
    /// would let one caller's raise change what the next one measures.
    /// A status strip that is off: `--status` is opt-in, so this is what every
    /// composite in this suite runs with.
    fn no_status() -> crate::status::StatusStrip {
        crate::status::StatusStrip::new(crate::status::StatusConfig::off())
    }

    fn no_lock() -> crate::lock::LockSurface {
        crate::lock::LockSurface::new(crate::consent::TrustedIndicator::for_test())
    }

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
            false,
            crate::status::StatusConfig::off(),
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        let pixels = client_pixels(SW, SH);
        state.scene_for_test().commit(
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
        state.scene_for_test().clear_surface();
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state
            .scene_for_test()
            .commit(SurfaceContent::from_rgba(from_fd, SW, SH).expect("well-formed content"));
        state.redraw().expect("composite the committed surface");

        // Sharing proof: the retained framebuffer (capture's pixel source)
        // is byte-for-byte the shared composition's output.
        let retained = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            retained,
            state.bound_scene().compose(VW, VH),
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

    /// **A hidden realm's capture is its own view, and the bound realm's is
    /// still the retained readback** (WS-E.1.3, issue #209).
    ///
    /// This backend serves two sources: the bound realm reads back the
    /// retained pixman framebuffer — unchanged, which is what keeps the
    /// P1.3.2/P1.3.6 goldens and the M1.3 fidelity gate byte-exact — and a
    /// hidden realm composes its own scene at the same view size. The test
    /// asserts both, *and* that the two sources agree for the bound realm, so
    /// "two paths, one composition" is checked rather than argued.
    #[test]
    fn a_hidden_realms_capture_is_its_own_view_never_the_outputs() {
        use crate::grants::RealmId;
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::session::Presenter;

        const VW: u32 = 96;
        const VH: u32 = 64;
        let event_loop: EventLoop<'static, ()> = EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            (VW as i32, VH as i32).into(),
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        let (bound, hidden) = (RealmId::new("realm-0"), RealmId::new("realm-b"));
        // Different fixtures: the bound realm fills the view, the hidden one
        // is letterboxed, so the two compositions cannot coincide.
        state
            .scene_mut(&bound)
            .commit(SurfaceContent::from_rgba(client_pixels(VW, VH), VW, VH).expect("content"));
        state
            .scene_mut(&hidden)
            .commit(SurfaceContent::from_rgba(client_pixels(32, 24), 32, 24).expect("content"));
        state.bind_output(&bound);
        state.redraw().expect("composite the bound realm");

        let got_bound = state.view_rgba(&bound).expect("the bound realm has a view");
        let got_hidden = state
            .view_rgba(&hidden)
            .expect("a hidden realm still has a view");
        let want_bound = state
            .scenes
            .scene(&bound)
            .expect("bound scene")
            .compose(VW, VH);
        let want_hidden = state
            .scenes
            .scene(&hidden)
            .expect("hidden scene")
            .compose(VW, VH);
        // Diagnosed rather than dumped: two view-sized buffers in an
        // `assert_eq!` message is a wall of bytes nobody reads, and the fact
        // that matters is *whose* pixels came back.
        let whose = |got: &[u8]| match got {
            g if g == want_bound => "the BOUND realm's",
            g if g == want_hidden => "the HIDDEN realm's",
            _ => "neither realm's",
        };

        // The bound realm's answer is the retained framebuffer, and it is
        // also exactly `Scene::compose` — the two sources agree.
        assert!(
            got_bound == state.latest_frame_rgba().expect("readback"),
            "the bound realm's capture must be the retained readback"
        );
        assert_eq!(
            whose(&got_bound),
            "the BOUND realm's",
            "...which is byte-for-byte the one shared composition"
        );

        // The hidden realm's answer is its own scene, not the output's.
        assert_eq!(
            whose(&got_hidden),
            "the HIDDEN realm's",
            "a hidden realm's capture returned {} pixels: this backend must compose that \
             realm's own scene, never hand back the output's retained frame",
            whose(&got_hidden)
        );
        assert!(
            want_bound != want_hidden,
            "the two realms' compositions must differ, or this test proves nothing"
        );

        // A realm nothing has touched has no view at all, which the
        // chokepoint turns into `no_surface`.
        assert!(state.view_rgba(&RealmId::new("realm-z")).is_none());
    }

    /// The P1.3.8 acceptance, cross-backend: the nested backend serves the
    /// **same bare-scene bytes** the headless backend does for an identical
    /// scene (issue #116).
    ///
    /// Both are [`Scene::compose`]. Headless retains it in a pixman image and
    /// reads it back; the nested backend composes it on the CPU on demand
    /// ([`crate::backend::winit::capture_pixels`]). This drives *both* real
    /// capture paths against the same one scene and asserts they are
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
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            TrustedIndicator::for_test(),
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state
            .scene_for_test()
            .commit(SurfaceContent::from_rgba(client_pixels(SW, SH), SW, SH).expect("content"));
        state.redraw().expect("composite the committed surface");

        // Headless capture: the retained pixman framebuffer, read back.
        let headless_capture = state.latest_frame_rgba().expect("headless readback");
        // Nested capture: the bare scene composed on the CPU, from the SAME
        // scene object that fed the headless readback above.
        let nested_capture = capture_pixels(state.bound_scene(), size).expect("a nonzero view");

        // The P1.3.8 requirement: same scene, same pixels, byte for byte.
        assert_eq!(
            nested_capture, headless_capture,
            "nested and headless captures must be byte-identical for the same scene"
        );
        // ...and both are exactly the one shared composition.
        assert_eq!(nested_capture, state.bound_scene().compose(VW, VH));

        // Overlay excluded on the nested side too: a prompt on the
        // human-visible composition changes it, but the nested capture is
        // unmoved — `capture_pixels` takes no `ConsentSurface`, so it *cannot*
        // carry a card.
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        consent.show_for_test(prompt_fixture());
        let human_visible = super::super::compose_human_visible(
            state.bound_scene(),
            &mut consent,
            &mut no_lock(),
            &super::super::blank::BlankSurface::for_test(),
            &mut no_status(),
            VW,
            VH,
            false,
        );
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        // A realm that has painted, so the test distinguishes "the overlay is
        // absent" from "nothing was ever drawn".
        state.scene_for_test().commit(
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
            state.bound_scene().compose(VW, VH),
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

    /// **The lock screen reaches the human-visible framebuffer and is
    /// byte-absent from a capture of the same instant** (WS-E.2.2, issue #214,
    /// acceptance criterion 2).
    ///
    /// The shape of
    /// [`a_prompt_reaches_human_visible_output_but_never_a_capture`] above,
    /// deliberately, because it is the same structural claim about the same
    /// fork — and the claim is *load-bearing in the opposite direction* here.
    /// For the consent card, "an agent cannot watch it" is the guarantee. For
    /// the lock, the guarantee is the one nobody expects: an agent holding
    /// `observe` keeps receiving the realm view **across a lock**, unchanged,
    /// because the lock composites downstream of `Scene::compose`. That is the
    /// owner's decision (D-025), and this is the test that says it is really
    /// what the code does rather than what a doc comment claims.
    ///
    /// The negative half is the one that would pass vacuously, so the run of
    /// bytes it searches for is asserted to be **present in the output** first.
    #[test]
    fn the_lock_screen_reaches_human_visible_output_but_never_a_capture() {
        use crate::capture::{render_frame, RealmViewFrame};
        use crate::lock::tests::lock_fixture;
        use crate::scene::{tests::client_pixels, SurfaceContent};

        let _fd = crate::capture::tests::fd_lock();
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        // A realm that has painted, so "the lock is absent from the capture" is
        // distinguishable from "nothing was ever drawn".
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(SW, SH), SW, SH).expect("well-formed content"),
        );
        state.redraw().expect("composite the committed surface");
        let clean_view = state.latest_frame_rgba().expect("readback");
        let clean_output = state.latest_output_rgba().expect("readback");

        // The lock goes up.
        state.output.lock.raise(lock_fixture());
        state.redraw().expect("recomposite with the lock up");

        // --- Human-visible side: the cover and the card are really there. ---
        let output = state.latest_output_rgba().expect("readback");
        assert_ne!(output, clean_output, "the lock must change the output");
        let card = crate::lock::render::rasterize(&lock_fixture());
        let (cx, cy) = state
            .output
            .lock
            .card_origin(VW, VH)
            .expect("a lock is up, so the card has an origin");
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
        // The cover is OPAQUE, not a scrim: the bottom-left pixel is the lock's
        // own colour and carries nothing of the client's. Below the trusted band
        // and outside the centered card, so it isolates the cover.
        let bl = (VH as usize - 1) * VW as usize * test_pattern::BYTES_PER_PIXEL;
        assert_eq!(
            &output[bl..bl + 4],
            &crate::lock::render::COVER_RGBA[..],
            "the lock cover must replace the realm view, not darken it"
        );

        // --- Agent side: the capture is the realm view, unchanged. ---
        let view = state.latest_frame_rgba().expect("readback");
        assert!(
            view == clean_view,
            "a lock being up must not move a single pixel of the realm view: an observe \
             grant keeps capturing across a lock (D-025), published in \
             docs/book/src/limits.md rather than quietly fixed here"
        );
        assert!(
            view == state.bound_scene().compose(VW, VH),
            "the capture source must be exactly Scene::compose -- the lock composites at the \
             output stage, above this"
        );
        assert_ne!(view, output, "...and the two really do differ now");

        // The delivered artifact, not just the retained image.
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
        assert!(
            served == swizzle(&clean_view),
            "the served capture must be the lock-free realm view"
        );
        // Byte-absence, pixel level. Negative claims are the ones that pass
        // vacuously, so the run being searched for is asserted PRESENT in the
        // output first -- which makes "absent from the capture" a statement
        // about a pattern that demonstrably exists.
        let card_row = &card.rgba[..card.width as usize * test_pattern::BYTES_PER_PIXEL];
        let card_row_wire = swizzle(card_row);
        assert!(
            swizzle(&output)
                .windows(card_row_wire.len())
                .any(|w| w == card_row_wire),
            "the searched-for run must exist in the human-visible output, or the absence \
             assertion below proves nothing"
        );
        assert!(
            !served
                .windows(card_row_wire.len())
                .any(|w| w == card_row_wire),
            "a row of lock-screen pixels reached a capture"
        );

        // Taking the lock down restores the human-visible output exactly.
        state.output.lock.lower();
        state.redraw().expect("recomposite with the lock down");
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            clean_output,
            "a lowered lock must leave no pixels behind"
        );
    }

    /// **The agent cursor reaches human-visible output and never a capture**
    /// (D-019) — the sibling of
    /// [`a_prompt_reaches_human_visible_output_but_never_a_capture`], on real
    /// composited pixels of the one backend CI can run.
    ///
    /// This is the test that makes the IDL's ordering invariant 4 — *no agent
    /// principal's cursor is composited into another principal's captured
    /// frame* — a rule with something to be true of. Before this change the
    /// invariant held **vacuously**: the core drew no cursor at all, so no
    /// arrangement of the code could have violated it and nothing checked.
    /// Now a sprite really is composited, and the exclusion is a property of
    /// where it is composited: at the output stage, downstream of the
    /// `Scene::compose` a capture reads. This asserts both halves of that
    /// against the two retained images, and takes the agent-side half all the
    /// way through [`crate::capture::render_frame`] — the buffer that would
    /// actually be sealed into a memfd — because anything weaker proves the
    /// retained image is clean while leaving the delivered artifact untested.
    ///
    /// The sprite is enabled here through the same `--agent-cursor` switch an
    /// operator passes, not through a test-only back door: the flag exists so
    /// this property can be *proved* on headless, which is why the default
    /// being off (see [`run`]) does not leave it unproven.
    #[test]
    fn the_agent_cursor_reaches_human_visible_output_but_never_a_capture() {
        use crate::capture::{render_frame, RealmViewFrame};
        use crate::cursor::{AGENT_CURSOR_CORE, AGENT_CURSOR_HALO};
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::session::Presenter;

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 400;
        const VH: u32 = 300;
        const SW: u32 = 400;
        const SH: u32 = 300;
        // Well inside the view and well below the trusted band, so the sprite
        // is drawn whole and lands on client-owned pixels — which is what
        // makes "it is on the output and not in the capture" a statement about
        // the same pixels rather than about two different regions.
        const CX: f64 = 200.0;
        const CY: f64 = 150.0;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            // `--agent-cursor`, the operator's own switch.
            true,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        // A realm that has painted, so "the sprite is absent from the
        // capture" is distinguishable from "nothing was ever drawn".
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(SW, SH), SW, SH).expect("well-formed content"),
        );
        state.redraw().expect("composite the committed surface");
        let clean_view = state.latest_frame_rgba().expect("readback");
        let clean_output = state.latest_output_rgba().expect("readback");
        let count = |px: &[u8], rgba: [u8; 4]| {
            px.chunks_exact(test_pattern::BYTES_PER_PIXEL)
                .filter(|pixel| *pixel == rgba)
                .count()
        };
        // The client's test pattern happens to contain neither sprite colour,
        // so a nonzero count later is the sprite and only the sprite. Asserted
        // rather than assumed: if the pattern ever changed to include one, this
        // test would silently stop measuring anything.
        assert_eq!(count(&clean_output, AGENT_CURSOR_CORE), 0);
        assert_eq!(count(&clean_output, AGENT_CURSOR_HALO), 0);

        // The agent moves its pointer. Through `set_agent_cursor`, which is
        // the seam `session::post_dispatch` drives once per dispatch round —
        // not by writing the field, so the return value's "did this change"
        // contract is exercised too.
        assert!(
            state.set_agent_cursor(Some((CX, CY))),
            "a first agent position must dirty the frame"
        );
        assert!(
            !state.set_agent_cursor(Some((CX + 0.25, CY))),
            "a sub-pixel move draws the same sprite and must not dirty the frame"
        );
        state.redraw().expect("recomposite with the cursor up");

        // --- Human-visible side: the sprite is really on the display. ---
        let output = state.latest_output_rgba().expect("readback");
        assert_ne!(output, clean_output, "the cursor must change the output");
        let core_px = count(&output, AGENT_CURSOR_CORE);
        assert!(
            core_px > 0,
            "the agent cursor is not on the human-visible output at all"
        );
        assert!(count(&output, AGENT_CURSOR_HALO) > 0, "no halo was drawn");
        // At the hotspot, specifically — not merely somewhere.
        let at = |px: &[u8], x: u32, y: u32| {
            let off = (y as usize * VW as usize + x as usize) * test_pattern::BYTES_PER_PIXEL;
            px[off..off + test_pattern::BYTES_PER_PIXEL].to_vec()
        };
        assert_eq!(
            at(&output, CX as u32, CY as u32),
            AGENT_CURSOR_CORE.to_vec(),
            "the sprite is not where the agent's pointer is"
        );
        // The trusted band is untouched, and so is everything outside the
        // sprite's own footprint: the cursor is an overlay, not a repaint.
        let band_bytes = crate::consent::TRUST_BAND_HEIGHT as usize
            * VW as usize
            * test_pattern::BYTES_PER_PIXEL;
        assert_eq!(
            output[..band_bytes],
            clean_output[..band_bytes],
            "the agent cursor reached the trusted band's rows"
        );
        let changed = output
            .chunks_exact(test_pattern::BYTES_PER_PIXEL)
            .zip(clean_output.chunks_exact(test_pattern::BYTES_PER_PIXEL))
            .filter(|(now, before)| now != before)
            .count();
        assert_eq!(
            changed,
            core_px + count(&output, AGENT_CURSOR_HALO),
            "the cursor changed pixels that are not part of the sprite"
        );

        // --- Agent side: the capture is the realm view, unchanged. ---
        let view = state.latest_frame_rgba().expect("readback");
        assert_eq!(
            view, clean_view,
            "an agent cursor being drawn must not move a single pixel of the realm view"
        );
        assert_eq!(
            view,
            state.bound_scene().compose(VW, VH),
            "the capture source must be exactly Scene::compose -- the sprite composites \
             at the output stage, above this"
        );
        assert_eq!(count(&view, AGENT_CURSOR_CORE), 0);
        assert_eq!(count(&view, AGENT_CURSOR_HALO), 0);
        assert_ne!(view, output, "...and the two really do differ now");

        // The delivered artifact, not just the retained image: what a
        // `capture_frame` would seal into a memfd carries no sprite pixel. The
        // wire frame is XRGB, so the sprite's colours are compared swizzled —
        // otherwise "absent" would be true of bytes nobody was looking for.
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
        let swizzle = |rgba: [u8; 4]| [rgba[2], rgba[1], rgba[0], 0xff];
        for colour in [AGENT_CURSOR_CORE, AGENT_CURSOR_HALO] {
            let wire = swizzle(colour);
            assert!(
                !served.chunks_exact(4).any(|px| px == wire),
                "an agent-cursor pixel ({colour:?}) reached a served capture: \
                 `vitrin_view.frame_ready` is delivering human-visible output"
            );
        }

        // The pointer going away takes the sprite with it, leaving nothing
        // behind — the realm-teardown case (`InputRouter::reset`).
        assert!(state.set_agent_cursor(None));
        state.redraw().expect("recomposite with no cursor");
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            clean_output,
            "a cleared agent pointer must leave no sprite pixels behind"
        );
    }

    /// **The status strip reaches human-visible output and never a capture**
    /// (WS-E.2.3, issue #215).
    ///
    /// Written positive-first on purpose. "Absent from the capture" is a
    /// negative claim and negatives pass vacuously — a capture that was empty
    /// for an unrelated reason would satisfy it while proving nothing. So the
    /// **pattern is located in the human-visible output first**, by ground
    /// colour and by exact row range, and only then looked for in the realm
    /// view and in the bytes a `capture_frame` would actually seal into a
    /// memfd.
    ///
    /// What is at stake is not cosmetic: the clock is a timing oracle and the
    /// battery level is a session fact an agent has no other route to. Both are
    /// low-bandwidth; neither is nothing.
    #[test]
    fn the_status_strip_reaches_human_visible_output_but_never_a_capture() {
        use crate::capture::{render_frame, RealmViewFrame};
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::session::Presenter;
        use crate::status::{StatusConfig, DEFAULT_HEIGHT, STRIP_TOP};

        let _fd = crate::capture::tests::fd_lock();
        const VW: u32 = 400;
        const VH: u32 = 300;
        const BPP: usize = test_pattern::BYTES_PER_PIXEL;

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            false,
            // `--status`, the operator's own switch.
            StatusConfig {
                enabled: true,
                ..StatusConfig::default()
            },
        )
        .expect("headless state under pixman");

        // A realm that has painted, so "the strip is absent from the capture"
        // is distinguishable from "nothing was ever drawn".
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(VW, VH), VW, VH).expect("well-formed content"),
        );
        state.redraw().expect("composite the committed surface");
        let clean_view = state.latest_frame_rgba().expect("readback");
        let clean_output = state.latest_output_rgba().expect("readback");

        // Nothing has been sampled yet, so no strip is drawn: the flag alone
        // paints nothing, which is what makes the difference below the strip's
        // and not the flag's.
        let band_bytes = crate::consent::TRUST_BAND_HEIGHT as usize * VW as usize * BPP;
        assert_eq!(
            clean_output[band_bytes..],
            clean_view[band_bytes..],
            "before the first sample the output differs from the view only by the band"
        );

        // One sample: this is the seam `session::post_dispatch` drives.
        assert!(
            state.refresh_status(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_786_244_643),
                std::time::Instant::now(),
            ),
            "a first sample must dirty the frame"
        );
        state.redraw().expect("recomposite with the strip up");
        let output = state.latest_output_rgba().expect("readback");
        let view = state.latest_frame_rgba().expect("readback");

        // --- Human-visible side: the strip is REALLY on the display. ---
        let ground = |px: &[u8]| {
            px.chunks_exact(BPP)
                .filter(|pixel| *pixel == crate::status::render::tests::GROUND)
                .count()
        };
        let strip_px = ground(&output);
        assert!(
            strip_px > 0,
            "the status strip is not on the human-visible output at all"
        );
        // And in its own rows, exactly: every row of `[STRIP_TOP, STRIP_TOP +
        // DEFAULT_HEIGHT)` differs from the clean composite, and no row outside
        // that range does.
        let row =
            |px: &[u8], y: u32| px[y as usize * VW as usize * BPP..][..VW as usize * BPP].to_vec();
        for y in 0..VH {
            let inside = (STRIP_TOP..STRIP_TOP + DEFAULT_HEIGHT).contains(&y);
            assert_eq!(
                row(&output, y) != row(&clean_output, y),
                inside,
                "row {y} changed={} but inside-the-strip={inside}",
                row(&output, y) != row(&clean_output, y)
            );
        }

        // --- Agent side: the capture is the realm view, unchanged. ---
        assert_eq!(
            view, clean_view,
            "a status strip being drawn must not move a single pixel of the realm view"
        );
        assert_eq!(
            view,
            state.bound_scene().compose(VW, VH),
            "the capture source must be exactly Scene::compose -- the strip composites at the \
             output stage, above this"
        );
        assert_eq!(
            ground(&view),
            0,
            "a strip pixel reached the realm view an agent may capture"
        );
        assert_ne!(view, output, "...and the two really do differ now");

        // The delivered artifact, not just the retained image. The wire frame
        // is XRGB, so the ground colour is compared swizzled -- otherwise
        // "absent" would be true of bytes nobody was looking for.
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
        let g = crate::status::render::tests::GROUND;
        let wire = [g[2], g[1], g[0], 0xff];
        assert!(
            !served.chunks_exact(4).any(|px| px == wire),
            "a status-strip pixel reached a served capture: `vitrin_view.frame_ready` is \
             delivering human-visible output, and with it a clock an agent must not have"
        );
    }

    /// **The session's trusted-indicator secret reaches human-visible output
    /// and never a screenshot file** (WS-E.2.4, issue #216).
    ///
    /// The sharpest question in the issue, asserted on real composited pixels
    /// and on the real file on disk. `TrustedIndicator::for_test()` is
    /// `0xFF00AA`, a colour nothing else in this composite paints, so a search
    /// that finds the triple found the band or the card's ring and nothing
    /// else.
    ///
    /// **Positive-first, for `the_status_strip_...`'s reason**: "absent from
    /// the file" is a negative claim and negatives pass vacuously — a file that
    /// was empty, or a session with no indicator, would satisfy it while
    /// proving nothing. So the colour is located in the human-visible output
    /// *first*, twice: in the always-present band, and then again in the
    /// **ring around a raised consent card**, which is the case that kills the
    /// "just crop the band's rows out" alternative — the ring is in the middle
    /// of the output, not at the top.
    ///
    /// Why this matters rather than being tidy: the confined realm runs as this
    /// core's uid, so a file the core writes is a file the app can read. A
    /// screenshot carrying the band would hand a forger the one colour that
    /// tells a genuine prompt from a painted replica, permanently, on the first
    /// press of the key.
    #[test]
    fn the_trusted_indicator_reaches_human_visible_output_but_never_a_screenshot_file() {
        use crate::consent::tests::prompt_fixture;
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::screenshot::{capture_to_file, ScreenshotDir, ScreenshotGesture};

        const VW: u32 = 800;
        const VH: u32 = 600;
        const BPP: usize = test_pattern::BYTES_PER_PIXEL;
        let secret = crate::consent::TrustedIndicator::for_test().color();
        // The RGB triple as it lands in an 8-bit truecolor PNG's pixel bytes.
        let triple = [secret[0], secret[1], secret[2]];

        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(VW, VH), VW, VH).expect("well-formed content"),
        );
        // A prompt up, so the ring is composited too: the band alone would let
        // a crop pass this test, and a crop is exactly the design that was
        // rejected.
        state.output.consent.show_for_test(prompt_fixture());
        state.redraw().expect("composite with a prompt up");

        let output = state.latest_output_rgba().expect("readback");
        let view = state.latest_frame_rgba().expect("readback");

        // --- Positive control, twice. ---
        let secret_px = |px: &[u8]| px.chunks_exact(BPP).filter(|p| *p == secret).count();
        let band_rows = crate::consent::TRUST_BAND_HEIGHT as usize;
        let band_bytes = band_rows * VW as usize * BPP;
        assert!(
            secret_px(&output[..band_bytes]) > 0,
            "the trusted band is not on the human-visible output at all"
        );
        assert!(
            secret_px(&output[band_bytes..]) > 0,
            "the card's trusted ring is not on the human-visible output below the band -- \
             without it this test would pass for a design that merely cropped the band"
        );
        assert_eq!(
            secret_px(&view),
            0,
            "the secret reached the realm view, which is what a screenshot is taken from"
        );

        // --- The artifact on disk, not the buffer. ---
        let dir = std::env::temp_dir().join(format!(
            "vitrin-indicator-screenshot-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut target = ScreenshotDir::open(&dir).expect("a clean, private temp dir");
        let (name, _digest) =
            capture_to_file(&mut target, ScreenshotGesture::Full, &view, VW, VH).expect("written");
        let png = std::fs::read(dir.join(&name)).expect("read the screenshot back");

        // The encoder writes pixels literally (filter 0, stored DEFLATE
        // blocks), so the triple would be findable in the file's bytes if it
        // were in the picture -- proved by the same search over an encoding of
        // the human-visible buffer, which DOES contain it.
        let contains = |haystack: &[u8]| haystack.windows(3).any(|w| w == triple);
        let rgb_of = |rgba: &[u8]| -> Vec<u8> {
            rgba.chunks_exact(BPP)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        };
        assert!(
            contains(&vitrin_png::encode_rgb(VW, VH, &rgb_of(&output))),
            "the search itself is sound: encoding the HUMAN-VISIBLE buffer does put the \
             secret in the file's bytes"
        );
        assert!(
            !contains(&png),
            "the session's trusted-indicator secret reached a file on disk, which any \
             same-uid app can read: the forger now knows the colour"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Without `--status`, the headless composite is unchanged.** The default
    /// this backend must keep, for the reason `--agent-cursor`'s default exists:
    /// `tests/integration/test_real_trust_band.py` compares the human-visible
    /// frame against the realm view byte for byte, and a ticking clock in that
    /// comparison would make a mock-free milestone gate a function of the time
    /// of day.
    #[test]
    fn without_the_flag_the_status_strip_changes_no_headless_pixel() {
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::session::Presenter;

        const VW: u32 = 200;
        const VH: u32 = 150;
        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(VW, VH), VW, VH).expect("well-formed content"),
        );
        state.redraw().expect("composite");
        let before = state.latest_output_rgba().expect("readback");

        assert!(
            !state.refresh_status(std::time::SystemTime::now(), std::time::Instant::now()),
            "a strip that is off must never dirty a frame"
        );
        state.redraw().expect("recomposite");
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            before,
            "`--status` off must leave the human-visible composite byte-identical"
        );
    }

    /// **Without `--agent-cursor`, the headless composite is unchanged** — the
    /// default this backend must keep, because
    /// `tests/integration/test_real_trust_band.py` asserts the human-visible
    /// frame tracks the realm view *after* a real `grant.pointer.click()`, and
    /// that gate is mock-free milestone evidence.
    #[test]
    fn without_the_flag_an_agent_pointer_changes_no_headless_pixel() {
        use crate::scene::{tests::client_pixels, SurfaceContent};
        use crate::session::Presenter;

        const VW: u32 = 200;
        const VH: u32 = 150;
        let size: Size<i32, Physical> = (VW as i32, VH as i32).into();
        let event_loop: EventLoop<'static, HeadlessView> =
            EventLoop::try_new().expect("calloop event loop");
        let mut state = HeadlessView::new(
            size,
            event_loop.get_signal(),
            crate::consent::TrustedIndicator::for_test(),
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(client_pixels(VW, VH), VW, VH).expect("well-formed content"),
        );
        state.redraw().expect("composite");
        let before = state.latest_output_rgba().expect("readback");

        // The router really does hand a position over; the backend really does
        // decline it. `false` means `post_dispatch` marks nothing dirty, so a
        // hovering agent costs a plain headless run no composite at all.
        assert!(!state.set_agent_cursor(Some((100.0, 75.0))));
        state.redraw().expect("recomposite");
        assert_eq!(
            state.latest_output_rgba().expect("readback"),
            before,
            "a plain headless run must composite the same bytes it did before the \
             agent cursor existed"
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

        // The same probe as `crate::shim::tests::c_shim_conforms_to_the_real_core`,
        // called rather than copied (#288): the assert's behaviour is
        // preserved by `Require::UnderCiUnlessDeclared`, the declared skip
        // prints a marker line the census can itemise instead of an
        // `eprintln!` that `cargo test` swallowed on the passing path, and
        // the answer arrives as an opaque `Verdict` this test cannot inspect
        // -- so the guard cannot be inverted around the body below.
        vitrin_skip::skip_unless!(vitrin_skip::C_SHIM, crate::shim::tests::c_shim_built());
        let shim_bin = crate::shim::tests::c_shim_bin();

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
        let mut spawned = crate::spawn::spawn_realm_with_env(
            &realm,
            &paths,
            &mut recorder,
            crate::spawn::SpawnOrigin::Startup,
            |name| match name {
                "WLR_BACKENDS" => Some("headless".into()),
                "WLR_RENDERER" => Some("pixman".into()),
                "WLR_RENDERER_ALLOW_SOFTWARE" => Some("1".into()),
                "WLR_LIBINPUT_NO_DEVICES" => Some("1".into()),
                _ => None,
            },
        )
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
            false,
            crate::status::StatusConfig::off(),
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
                .handle_message(msg, state.scene_for_test(), None, &mut |frame| {
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");
        state
            .scene_for_test()
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
                super::super::compose_human_visible(
                    state.bound_scene(),
                    &mut expected,
                    &mut no_lock(),
                    &super::super::blank::BlankSurface::for_test(),
                    &mut no_status(),
                    VW,
                    VH,
                    false
                ),
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
            false,
            crate::status::StatusConfig::off(),
        )
        .expect("headless state under pixman");

        let painted = client_pixels(300, 200);
        state.scene_for_test().commit(
            SurfaceContent::from_rgba(painted.clone(), 300, 200).expect("well-formed content"),
        );
        state.output.consent.show_for_test(prompt_fixture());
        state
            .redraw()
            .expect("redraw with a prompt over a live realm");

        // The realm dies: the scene loses its surface (the death funnel), and
        // the funnel scrubs the retained images.
        state.scene_for_test().clear_surface();
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
            super::super::compose_human_visible(
                &crate::scene::Scene::new(),
                &mut expected,
                &mut no_lock(),
                &super::super::blank::BlankSurface::for_test(),
                &mut no_status(),
                VW,
                VH,
                false
            ),
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
                false,
                crate::status::StatusConfig::off(),
            )
            .expect("headless state under pixman"),
            server: Some(ShimServer::new(ShimConfig {
                realm: "realm-0".into(),
                width: W,
                height: H,
            })),
            router: crate::input::InputRouter::detached(crate::input::NoopHook),
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
                    match server.handle_message(
                        msg,
                        state.headless.scene_for_test(),
                        None,
                        &mut send,
                    ) {
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
                            &crate::grants::RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
                            state.headless.scene_for_test(),
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
            state.headless.bound_scene().compose(W, H),
            test_pattern::render(W, H),
            "shim death must drop the surface from the scene"
        );
    }
}
