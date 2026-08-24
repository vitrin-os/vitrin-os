// SPDX-License-Identifier: MPL-2.0
//! Nested backend: the core runs as a client of the host compositor
//! (GNOME, Hyprland, …), presenting exactly one host window — the gamescope
//! nested-session pattern (PRD Doc 2 §4/§17). Rendering stays deliberately
//! trivial per plan risk R1: one window, one full-window texture blit of the
//! composed human-visible output ([`super::compose_human_visible`] — the
//! realm view of [`Scene::compose`], the same bytes the headless backend
//! retains for capture, P1.3.3, with the consent overlay on top, P1.7.1).
//!
//! There are **two** presentation paths into that one window, and both are
//! human-visible output: the CPU texture blit above, and the zero-copy
//! dmabuf present ([`crate::dmabuf::present_human_visible`], taken when a
//! GPU import is retained and no overlay needs the window this frame). Both
//! draw with [`WINDOW_TRANSFORM`] and both carry the trusted band, because a
//! path that presented without it would put a frame made entirely of
//! client-owned pixels on the human's display — see [`NestedState::try_redraw`]
//! and `no_presentation_path_can_drop_the_trusted_band`.
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
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop::{
    EventSource, Interest, LoopHandle, LoopSignal, Mode, Poll, PostAction, Readiness, Token,
};
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::context::{GlAttributes, PixelFormatRequirements};
use smithay::backend::egl::display::EGLDisplay;
use smithay::backend::egl::{native, EGLContext, EGLSurface};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis as HostAxis, AxisRelativeDirection, AxisSource,
    ButtonState as HostButtonState, Device, DeviceCapability, Event as HostEvent, InputBackend,
    InputEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, UnusedEvent,
};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, Frame, ImportMem, Renderer, RendererSuper};
use smithay::backend::SwapBuffersError;
use smithay::reexports::winit::application::ApplicationHandler;
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::event::{
    ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent,
};
use smithay::reexports::winit::event_loop::{ActiveEventLoop, EventLoop as HostEventLoop};
use smithay::reexports::winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use smithay::reexports::winit::platform::scancode::PhysicalKeyExtScancode;
use smithay::reexports::winit::platform::wayland::WindowAttributesExtWayland;
use smithay::reexports::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use smithay::reexports::winit::window::{Window as WinitWindow, WindowAttributes, WindowId};
use smithay::utils::{Buffer, Clock, Monotonic, Physical, Rectangle, Size, Transform};
use vitrin_protocol::generated::vitrin_shim_seat::KeyState as SeatKeyState;
use wayland_egl as wegl;

use tracing::{debug, error, info, trace};

use crate::consent::grab::{ConsentGate, ConsentGrab};
use crate::consent::{ConsentSurface, TrustedIndicator};
use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch, Trigger};
use crate::dmabuf::{present_human_visible, DmabufImporter, GlesDmabufImporter, RealmGpuContent};
use crate::grants::RealmId;
use crate::input;
use crate::recorder::Recorder;
use crate::scene::{RealmScenes, Scene};
use crate::session::{self, Runtime, RuntimeSeed};

/// Initial logical window size; matches the planned headless default
/// (`--headless --size 1280x800`, P1.3.2) so nested and headless views of
/// the same content agree by default.
const INITIAL_SIZE: (f64, f64) = (1280.0, 800.0);

/// The nested window's Wayland `app_id`.
///
/// Part of the operator-facing contract, not an implementation detail:
/// `docs/demo/RECORDING.md` tells a recording operator to match a
/// float-and-size window rule on this exact string, because on a tiling
/// compositor [`INITIAL_SIZE`] is only a request. Changing it breaks those
/// recipes, so change both together.
pub(crate) const NESTED_APP_ID: &str = "vitrind";

/// Background behind the composed view; only visible if the blit fails.
/// Deliberately near [`crate::scene::LETTERBOX_RGBA`] so nothing here reads
/// as client content.
const CLEAR_COLOR: Color32F = Color32F::new(0.06, 0.06, 0.08, 1.0);

/// The output transform every draw into the host window's EGL surface takes.
///
/// GL's window framebuffer has its origin at the bottom-left, while every
/// composition in this core is top-down (`Scene::compose`'s contract), so the
/// whole projection is flipped vertically. **One const, read by both
/// presentation paths** — the CPU texture blit in [`NestedState::try_redraw`]
/// and the zero-copy dmabuf present through
/// [`crate::dmabuf::present_human_visible`] — because a path that derived its
/// own would present that path's frames upside down while the other stayed
/// correct, which is exactly the regression this const replaces: the dmabuf
/// branch inherited `Transform::Normal` from the offscreen renderbuffer
/// harness `render_content` was written against, and kept it when it started
/// drawing into the window surface instead.
///
/// `pub(crate)` since first light so [`super::drm`]'s
/// `the_scanout_and_the_window_are_opposite_kinds_of_target` can assert
/// against it: the bare-metal backend's scanout constant took *this* value,
/// which mirrored the human's whole display vertically, and the durable fix is
/// a test that names the relationship between the two kinds of target rather
/// than a second magic literal. See [`crate::dmabuf::MEMORY_TARGET_TRANSFORM`].
pub(crate) const WINDOW_TRANSFORM: Transform = Transform::Flipped180;

/// Fallback frame budget (~60 Hz), scoped to whatever redraw chain is
/// already running (P1.3.9, issue #117: the chain itself starts only from a
/// real state change or an animating dead-man hold — this const paces it,
/// it does not keep it alive). Vsync'd blocking swaps are the primary
/// pacing mechanism, but Smithay's `vsync: true` only filters EGL config
/// selection — it never calls `eglSwapInterval` — so on hosts whose EGL
/// stack returns from swap immediately (Mesa software EGL under Xvfb,
/// GPU-less VMs on llvmpipe, X servers without vblank) a running chain
/// would otherwise spin unthrottled for as long as it lasts. Frames that
/// complete faster than this budget defer the next redraw to a timer
/// instead.
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
#[derive(Debug, Clone, PartialEq, Eq)]
struct TextureKey {
    size: Size<i32, Physical>,
    /// The realm the output was bound to at upload time (WS-E.1.3, issue
    /// #209), or `None` when nothing was bound.
    ///
    /// In the key because a bind changes every pixel of the window, and
    /// nothing else here would notice: [`Self::scene_generation`] is the
    /// *bound* scene's counter, and two realms' counters are independent —
    /// realm B's generation can equal realm A's by coincidence, so a bind
    /// between them would compare equal and leave the previous realm's
    /// texture on the human's screen until something unrelated changed.
    ///
    /// Deliberately **not** the realm's `layout_generation`: that counter is
    /// D-018(5)'s and issue #209 lands it unread on purpose, so the texture
    /// cache does not become its first consumer by accident. The identity of
    /// the bound realm is the fact this key needs, and it is the one it
    /// carries. (This is what costs [`TextureKey`] its `Copy`: a `RealmId`
    /// owns a `String`.)
    realm: Option<RealmId>,
    /// [`Scene::generation`] of the **bound** realm's scene at upload time.
    scene_generation: u64,
    /// [`ConsentSurface::generation`] at upload time. A separate counter
    /// rather than a merged one because the two changes have different
    /// consumers — see [`crate::consent`]'s redraw section.
    consent_generation: u64,
    /// [`crate::lock::LockSurface::generation`] at upload time (WS-E.2.2,
    /// issue #214).
    ///
    /// Omitting this shipped a real defect, found by the first human run of
    /// `shim/docs/nested-lock-screen.md` and not by any test: the lock is an
    /// input to [`super::compose_human_visible`], so raising it changes every
    /// pixel, but with no counter here the composed cover was presented only
    /// when some *unrelated* keyed input next moved. Against an idle client
    /// that is never — so the second `ctrl+alt+delete` of a session consumed
    /// the human's input behind a screen still showing the unlocked app.
    ///
    /// A separate counter rather than merging into
    /// [`Self::consent_generation`] for that field's own stated reason: the
    /// two changes have different consumers, and a lock raised over a live
    /// prompt must re-upload for both.
    lock_generation: u64,
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
    /// The agent cursor's hotspot in **integer view pixels**, or `None` when
    /// no sprite is shown (D-019).
    ///
    /// Integer for exactly the reason [`Self::hold_bucket`] is bucketed: the
    /// router's position is an `f64`, and an `f64` in a `PartialEq` cache key
    /// is a bug waiting for a rounding change. Quantizing through
    /// [`crate::cursor::hotspot`] — the same function the sprite's geometry
    /// is derived from — also makes the key exact rather than conservative:
    /// two positions that would draw byte-identical sprites compare equal and
    /// cost no re-upload, and two that would not, do not.
    agent_cursor: Option<(i32, i32)>,
    /// Whether the human's attention window is open (WS-E.1.7, issue #232),
    /// and so whether `window_pixels` drew the marker beside the trusted band.
    ///
    /// In the key for the same reason every other field here is: it is a
    /// visible element that changes independently of all of them. Without it
    /// the cache short-circuits the one call that would draw the marker *or
    /// erase it*, so pressing the key changed nothing on screen in the only
    /// backend where a human can press it — decision 8's "the human sees
    /// something" silently not holding. A bool rather than a deadline, because
    /// what the pixels depend on is open-or-not, and putting an `Instant` in a
    /// `PartialEq` cache key would re-upload every frame for a whole second.
    attention: bool,
    /// [`crate::status::StatusStrip::generation`] at upload time (WS-E.2.3).
    ///
    /// In the key for exactly the reason `attention` is: without it the cache
    /// short-circuits the one call that would redraw the strip, so on the CPU
    /// path a session's clock would advance only when the app happened to
    /// repaint — which for a static app is never. A generation rather than the
    /// snapshot itself, because the snapshot is what the generation counts.
    status_generation: u64,
    /// [`super::blank::BlankSurface::generation`] at upload time (WS-E.4.3,
    /// issue #223).
    ///
    /// In the key because the invariant this struct carries is that **every
    /// input to [`super::compose_human_visible`] appears here**, and the blank
    /// cover became one. It is a constant on this backend (`--blank-idle` is
    /// refused with `--nested`), and that is precisely why it would have been
    /// left out: `lock_generation` was missing for the whole of WS-E.2.2 for the
    /// same reason -- an input that is inert *today* on the backend you are
    /// looking at.
    blank_generation: u64,
}

/// How many distinct fill levels the hold indicator has. Twenty steps across
/// a ~1 s hold is finer than a human can perceive as stepping, and caps the
/// texture re-uploads one hold can cost.
const HOLD_STEPS: f64 = 20.0;

impl TextureKey {
    /// The key describing what a texture composed *right now* would contain.
    ///
    /// The `scene, consent, lock, status` run of parameters is deliberately
    /// in [`super::compose_human_visible`]'s order, because the invariant
    /// binding the two is that **every input to that function appears here**.
    /// `lock` was missing for the whole of WS-E.2.2 and the divergence was
    /// invisible precisely because the argument lists did not line up to be
    /// compared. Keep them parallel, and when composition grows an input, add
    /// it here in the same position and extend
    /// [`the_texture_key_changes_on_every_visible_transition`].
    #[allow(clippy::too_many_arguments)]
    fn current(
        size: Size<i32, Physical>,
        realm: Option<&RealmId>,
        scene: &Scene,
        consent: &ConsentSurface,
        lock: &crate::lock::LockSurface,
        blank: &super::blank::BlankSurface,
        hold: Option<f64>,
        agent_cursor: Option<(f64, f64)>,
        attention: bool,
        status: &crate::status::StatusStrip,
    ) -> Self {
        Self {
            size,
            realm: realm.cloned(),
            scene_generation: scene.generation(),
            consent_generation: consent.generation(),
            lock_generation: lock.generation(),
            blank_generation: blank.generation(),
            hold_bucket: hold.map(|p| (p.clamp(0.0, 1.0) * HOLD_STEPS) as u8),
            agent_cursor: agent_cursor.and_then(|(x, y)| crate::cursor::hotspot(x, y)),
            attention,
            status_generation: status.generation(),
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
/// plus the consent prompt, if one is up — then the agent cursor sprite
/// (D-019) where an agent is pointing, and, above everything, the dead-man
/// hold indicator (P1.7.3) while the human is mid-gesture.
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
///
/// **The agent cursor (D-019) is applied here for the same reason and with
/// the same argument**: a code-drawn, origin-derived overlay on the
/// human-visible side of the output stage, so it can no more reach a capture
/// than the card or the bar can — which is what turns the IDL's ordering
/// invariant 4 from a vacuous rule into an exercised one. It is *not* here
/// because headless cannot draw it: headless can, and does when the operator
/// passes `--agent-cursor`. It is here because the sprite's position is
/// nested-mode display policy, and because headless's human-visible
/// framebuffer is measured byte-for-byte by the trusted-band witness
/// (issue #139) and by `tests/integration/test_real_trust_band.py`, which
/// assert it tracks the realm view outside the band — so a sprite on by
/// default there would turn a mock-free milestone gate red for a cosmetic
/// reason. See [`super::headless::run`]'s `agent_cursor` argument.
///
/// Draw order is deliberate: the trusted band goes on inside
/// [`super::compose_human_visible`], the cursor after it but clipped below it
/// ([`crate::cursor`]), and the hold indicator last of all — so nothing an
/// agent positions can cover the strip the human reads the session colour
/// from, and nothing at all can hide a hold in progress.
/// **`human_cursor` is `None` on this backend and `Some` only on bare metal**
/// (WS-E.3.2, issue #218). The host desktop draws the human's pointer over
/// this window, so a sprite here would be a second cursor for one mouse; the
/// DRM backend has no host desktop and must draw it itself, which is the
/// change that made the IDL's "no human cursor is composited at all" sentence
/// nested-conditional. The parameter is threaded through this one function
/// rather than composited by each backend afterwards so that both CPU paths
/// draw the same overlays in the same order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window_pixels(
    scene: &Scene,
    consent: &mut ConsentSurface,
    lock: &mut crate::lock::LockSurface,
    blank: &super::blank::BlankSurface,
    status: &mut crate::status::StatusStrip,
    notice: Option<&mut crate::notice::CoreNotice>,
    hold: Option<f64>,
    agent_cursor: Option<(f64, f64)>,
    human_cursor: Option<(f64, f64)>,
    attention: bool,
    geom: crate::view::ViewGeometry,
) -> Vec<u8> {
    let (w, h) = geom.output();
    let mut pixels =
        super::compose_human_visible(scene, consent, lock, blank, status, geom, attention);
    // **After the lock cover, and before both cursors** (WS-E.3.5). After the
    // cover because the cover is opaque and the VT escape works *while
    // locked* (D-031(2)), so a notice drawn under it would be invisible in the
    // state a trapped human is most likely to be in. Before the sprites
    // because neither an agent's pointer nor the human's own may sit on top of
    // a core alarm; and before the hold indicator, which stays last of all so
    // nothing can hide a gesture on the off-switch in progress.
    //
    // `None` on nested and headless, and that is a fact about who owns the
    // display rather than a policy difference: neither can switch a VT, so
    // neither can fail to.
    if let Some(notice) = notice {
        notice.composite_over(&mut pixels, w, h);
    }
    if let Some((x, y)) = agent_cursor {
        // Deliberately drawn even over a raised lock screen (WS-E.2.2). It
        // looks wrong for a moment and is the honest choice: a lock does not
        // suspend an agent's grants (D-025), so an agent really is pointing
        // somewhere, and D-019's whole warrant is that a human can see when one
        // is acting. Hiding the sprite behind the lock would make the surface
        // that says "agents keep working" the one surface on which they
        // invisibly do.
        crate::cursor::composite_agent_cursor(&mut pixels, w, h, x, y);
    }
    // After the agent's, so the human's own pointer is never the sprite that
    // disappears under an overlapping one, and before the hold indicator for
    // the reason that one is last.
    if let Some((x, y)) = human_cursor {
        crate::cursor::composite_human_cursor(&mut pixels, w, h, x, y);
    }
    if let Some(progress) = hold {
        // Last, so neither a consent card nor an agent's cursor can hide the
        // fact that the human is mid-gesture on the off-switch.
        crate::deadman::composite_hold_indicator(&mut pixels, w, h, progress);
    }
    pixels
}

/// Upload the status strip's raster as a GPU texture, re-using the cached one
/// when nothing that would change it has changed (WS-E.2.3, issue #215).
///
/// The zero-copy path presents a *client* texture with no CPU composite, so the
/// strip has to reach it as a texture of its own — forcing the CPU path for a
/// clock would cost the whole zero-copy win, which at 2560x1600@240 is not a
/// trade available to make. The key is `(generation, width)`: the snapshot
/// changes once a minute (`HH:MM`, no seconds), so at steady state this uploads
/// **one 2560x20 RGBA texture — 200 KiB — per minute**, i.e. ~3.4 KiB/s, plus
/// one textured quad per presented frame.
///
/// The bytes come from [`crate::status::StatusStrip::raster`], the same cache
/// [`window_pixels`] blits from on the CPU path, so the two paths cannot draw
/// different strips.
///
/// A failed upload is a `None` and a warning, never an error: a frame without
/// the strip is a cosmetic loss, and unlike the trusted band the strip asserts
/// nothing about authenticity — so taking the session down, or refusing to
/// present, would trade a real property for a cosmetic one.
pub(crate) fn upload_status_texture<'t>(
    renderer: &mut GlesRenderer,
    strip: &mut crate::status::StatusStrip,
    cache: &'t mut Option<(u64, u32, GlesTexture)>,
    width: u32,
) -> Option<&'t GlesTexture> {
    let height = strip.height();
    if height == 0 || width == 0 {
        // Dropped rather than kept: a session that never turns the strip back
        // on must not hold a GPU texture for it.
        *cache = None;
        return None;
    }
    let generation = strip.generation();
    let fresh = matches!(cache.as_ref(), Some((cached, cached_w, _))
        if *cached == generation && *cached_w == width);
    if !fresh {
        let (bytes, raster_w, raster_h) = strip.raster(width)?;
        let buffer_size: Size<i32, Buffer> = (raster_w as i32, raster_h as i32).into();
        match renderer.import_memory(bytes, Fourcc::Abgr8888, buffer_size, false) {
            Ok(texture) => *cache = Some((generation, width, texture)),
            Err(err) => {
                tracing::warn!(
                    %err,
                    "the status strip could not be uploaded; this frame draws without it"
                );
                *cache = None;
                return None;
            }
        }
    }
    cache.as_ref().map(|(_, _, texture)| texture)
}

/// **Whose retained texture this frame presents zero-copy** — the *bound*
/// realm's, or nothing at all (WS-E.1.3, issue #209).
///
/// `None` means [`NestedState::try_redraw`] takes the CPU compose path over
/// [`RealmScenes::bound`] instead, which is what it did before any dmabuf was
/// ever imported. There are exactly three ways to get it:
///
/// - **No realm is bound.** There is no picture to be showing, so there is no
///   texture to show.
/// - **The bound realm has imported nothing** (or its import was cleared by a
///   shm commit or by its death). A *hidden* realm's retained content is not a
///   candidate here and that is the whole point of this function: the CPU path
///   was keyed on the bound realm from the start and this one was not, so with
///   `--dmabuf` a hidden realm's commit took over the human's window and
///   falsified the published "only the bound realm is on screen" claim on the
///   one path a human actually looks at.
/// - **An overlay needs the window.** A consent card (P1.7.1), a dead-man
///   hold indicator (P1.7.3) or a **lock screen** (WS-E.2.2) is up, and none of
///   them can be composited onto a GPU texture on this path — see
///   [`NestedState::try_redraw`], which carries that seam's full argument. The
///   lock is the one whose omission would be a *security* regression rather
///   than a cosmetic one: presenting the client's texture zero-copy while the
///   lock consumed every key would put the app on screen behind the lock.
///
/// A free function rather than a method on either argument because it is a
/// judgement *between* them, and split out for the reason [`window_pixels`] is:
/// presenting needs an EGL context and a host window, so CI cannot drive
/// `try_redraw` end to end (D-019(4) records headless as the only backend CI
/// can run). This is the part of that frame path a display-free test can hold,
/// and `a_hidden_realms_dmabuf_import_is_never_what_the_window_presents` holds
/// it.
/// **Which overlays need the CPU compositing path this frame**, and therefore
/// forbid the zero-copy dmabuf branch.
///
/// Three causes, and the third is a *security* one rather than a cosmetic one:
///
/// - a **dead-man hold** is animating (P1.7.3);
/// - a **consent card** is up (P1.7.1);
/// - a **lock screen** is up (WS-E.2.2, issue #214). The zero-copy branch
///   presents the confined client's own texture with no core composite at all,
///   so a locked session that took it would put the app on screen, full-window,
///   with the lock nowhere on it — while the gate consumed every key. That is a
///   second human-visible path exactly as issue #85's trusted band found, closed
///   here for the same reason and in the same place.
///
/// A free function rather than three `||`s inside [`NestedState::try_redraw`],
/// for the reason [`window_pixels`] and [`zero_copy_source`] are split out:
/// presenting needs an EGL context and a host window, so CI cannot drive
/// `try_redraw` at all (D-019(4)), and a decision written inline there is a
/// decision nothing can pin. `every_overlay_forces_the_cpu_path` pins it.
pub(crate) fn overlay_needs_the_window(
    hold: Option<f64>,
    consent: &ConsentSurface,
    lock: &crate::lock::LockSurface,
    blank: &super::blank::BlankSurface,
    notice: Option<&crate::notice::CoreNotice>,
) -> bool {
    hold.is_some()
        || consent.prompt().is_some()
        || lock.is_raised()
        // A fifth cause, and the same *security* shape as the lock (WS-E.4.3,
        // issue #223): the zero-copy branch presents the confined client's own
        // texture with no core composite at all, so a session that raised the
        // idle cover and then took that branch would put the app on screen,
        // full-window, on the frame it is about to power the panel down on --
        // and leave that frame in the scanout buffer for whatever the unblank
        // reveals. Nested and headless hold a surface that is never raised, so
        // this term is a constant `false` there.
        || blank.is_covering()
        // A fourth cause, and the only one that is *conditional on the
        // backend* (WS-E.3.5): the notice surface exists to tell a human on
        // bare metal that their VT escape did not work, and nested passes
        // `None` because it has no VT to escape from. A raised notice is
        // CPU-composited like the cover and the card, so the zero-copy branch
        // would present the client's texture with the one message the human
        // needs nowhere on it -- which is the original defect with an extra
        // step.
        || notice.is_some_and(crate::notice::CoreNotice::is_up)
}

pub(crate) fn zero_copy_source<'a, C>(
    scenes: &RealmScenes,
    content: &'a RealmGpuContent<C>,
    overlay_up: bool,
) -> Option<&'a C> {
    if overlay_up {
        return None;
    }
    content.of(scenes.focused()?)
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
pub(crate) fn capture_pixels(scene: &Scene, geom: crate::view::ViewGeometry) -> Option<Vec<u8>> {
    let (w, h) = geom.output();
    if w == 0 || h == 0 {
        return None;
    }
    // The bare realm view only — NOT `window_pixels`. Byte-for-byte what the
    // headless backend retains and reads back for the same scene, **inset
    // included** (issue #304): an agent's capture shows the app where the
    // human sees it, with the reserved rows as matte, rather than 28 rows
    // higher than the picture on the screen.
    Some(scene.compose(geom))
}

// ============================================================================
// Own winit glue (issue #118)
// ============================================================================
//
// Smithay 0.7.0's `backend::winit` module (`init_from_attributes_with_gl_attr`
// / `WinitEventLoop`) is a closed box: its `ApplicationHandler` and the
// `winit::event_loop::EventLoop` it pumps are both private, and the one
// keyboard event it hands out (`WinitKeyboardInputEvent`) carries only the
// evdev scancode — winit's interpreted `logical_key` (the layout-*dependent*
// character a keypress produces) is read and then dropped before the core
// ever sees it. There is no callback, no accessor, and no second
// `ApplicationHandler` winit will let us pump the same event loop with (it
// panics on a second `EventLoop` per thread) — so the only way to reach
// `logical_key` is for this backend to own the winit event loop itself.
//
// What follows is a from-scratch `WinitGraphicsBackend`/`WinitEventLoop`
// pair, built from the same *public* `smithay::backend::egl` primitives
// Smithay's own module calls (verified against its source, pinned at
// `=0.7.0`): one EGL display/context/surface, bound to the raw window handle
// winit hands back, exactly as upstream does it. The only real difference is
// the `ApplicationHandler`: ours additionally captures `KeyEvent::logical_key`
// out of `WindowEvent::KeyboardInput` and resolves it with
// [`input::host_keysym`], then routes keyboard through
// [`NestedState::handle_key`] → [`input::physical_key`] directly, bypassing
// [`input::intake_physical`]'s scancode-only `Keyboard` arm (which stays, for
// the generic `InputBackend`s the unit tests drive). Every other input class
// (pointer motion/button/axis) still flows through `intake_physical`, now
// instantiated over [`NestedInput`] instead of Smithay's private
// `WinitInput` — its event-struct fields are `pub(crate)` to smithay and so
// cannot be reused from here; [`NestedInput`]'s pointer/axis event structs
// and button-code table are copied 1:1 from Smithay's own
// `backend::winit::input` (verified line-for-line against the pinned
// source), only the marker type differs. Touch, gestures and tablet events
// are `UnusedEvent` here, so this backend produces none — `intake_physical`
// resolves gestures since WS-E.4.2 but can never see one from winit, and
// still resolves neither touch nor tablet. Nothing observable regresses by
// not modeling them.

/// Marker used to define the [`InputBackend`] types for this backend's own
/// winit event pump — the crate-local counterpart of Smithay's private
/// `WinitInput` (see the module section above for why it cannot be reused
/// directly).
#[derive(Debug)]
pub(crate) struct NestedInput;

/// Virtual input device winit-sourced events are attributed to. Mirrors
/// Smithay's `WinitVirtualDevice` field-for-field; there is exactly one
/// physical input path in nested mode, so one device suffices.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct NestedVirtualDevice;

impl Device for NestedVirtualDevice {
    fn id(&self) -> String {
        String::from("vitrin-nested-winit")
    }

    fn name(&self) -> String {
        String::from("vitrin nested winit virtual input")
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer
        )
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

/// Position relative to the window, each coordinate in `[0, 1]` — the same
/// normalization Smithay's private `RelativePosition` performs, needed here
/// only because that type is not exported.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NestedRelativePosition {
    x: f64,
    y: f64,
}

/// Absolute pointer motion, wrapping [`PointerMotionAbsoluteEvent`]. Built in
/// [`NestedWinitEventsApp::window_event`] from `WindowEvent::CursorMoved`,
/// identically to how Smithay's own handler builds `WinitMouseMovedEvent`.
#[derive(Debug, Clone)]
pub(crate) struct NestedMotionEvent {
    time: u64,
    position: NestedRelativePosition,
    global_x: f64,
    global_y: f64,
}

impl HostEvent<NestedInput> for NestedMotionEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> NestedVirtualDevice {
        NestedVirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<NestedInput> for NestedMotionEvent {}
impl AbsolutePositionEvent<NestedInput> for NestedMotionEvent {
    fn x(&self) -> f64 {
        self.global_x
    }

    fn y(&self) -> f64 {
        self.global_y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        f64::max(self.position.x * width as f64, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        f64::max(self.position.y * height as f64, 0.0)
    }
}

/// A wheel/touchpad scroll, wrapping [`PointerAxisEvent`]. Built from
/// `WindowEvent::MouseWheel`; the amount/v120 conversion is
/// `intake_physical`'s job, not this event's — it only reports the raw
/// winit delta, exactly as Smithay's `WinitMouseWheelEvent` does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NestedAxisEvent {
    time: u64,
    delta: MouseScrollDelta,
}

impl HostEvent<NestedInput> for NestedAxisEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> NestedVirtualDevice {
        NestedVirtualDevice
    }
}

impl PointerAxisEvent<NestedInput> for NestedAxisEvent {
    fn source(&self) -> AxisSource {
        match self.delta {
            MouseScrollDelta::LineDelta(_, _) => AxisSource::Wheel,
            MouseScrollDelta::PixelDelta(_) => AxisSource::Continuous,
        }
    }

    fn amount(&self, axis: HostAxis) -> Option<f64> {
        match (axis, self.delta) {
            (HostAxis::Horizontal, MouseScrollDelta::PixelDelta(delta)) => Some(-delta.x),
            (HostAxis::Vertical, MouseScrollDelta::PixelDelta(delta)) => Some(-delta.y),
            (_, MouseScrollDelta::LineDelta(_, _)) => None,
        }
    }

    fn amount_v120(&self, axis: HostAxis) -> Option<f64> {
        match (axis, self.delta) {
            (HostAxis::Horizontal, MouseScrollDelta::LineDelta(x, _)) => Some(-x as f64 * 120.0),
            (HostAxis::Vertical, MouseScrollDelta::LineDelta(_, y)) => Some(-y as f64 * 120.0),
            (_, MouseScrollDelta::PixelDelta(_)) => None,
        }
    }

    fn relative_direction(&self, _axis: HostAxis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

/// A pointer button press/release, wrapping [`PointerButtonEvent`]. Built
/// from `WindowEvent::MouseInput`; `button_code` reuses the exact
/// libinput/evdev mapping Smithay's own `WinitMouseInputEvent` uses
/// ([`mouse_button_code`], copied because the table itself, and the
/// X11-vs-Wayland `Other(u8)` distinction it encodes, are not exported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NestedButtonEvent {
    time: u64,
    button: WinitMouseButton,
    state: ElementState,
    is_x11: bool,
}

impl HostEvent<NestedInput> for NestedButtonEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> NestedVirtualDevice {
        NestedVirtualDevice
    }
}

impl PointerButtonEvent<NestedInput> for NestedButtonEvent {
    fn button_code(&self) -> u32 {
        mouse_button_code(self.button, self.is_x11)
    }

    fn state(&self) -> HostButtonState {
        match self.state {
            ElementState::Pressed => HostButtonState::Pressed,
            ElementState::Released => HostButtonState::Released,
        }
    }
}

/// libinput/evdev button code for a winit mouse button — Smithay's own
/// `WinitMouseInputEvent::button_code`, copied verbatim (that method, and
/// the `xorg_mouse_to_libinput` helper it calls for `Other`, are
/// `pub(crate)` to smithay and so unreachable from here). `is_x11` mirrors
/// the same distinction [`init_nested_winit`] derives from the window's raw
/// handle: X11 numbers extra buttons per the historical `xf86-input-libinput`
/// table; Wayland already numbers them as libinput does.
fn mouse_button_code(button: WinitMouseButton, is_x11: bool) -> u32 {
    match button {
        WinitMouseButton::Left => 0x110,
        WinitMouseButton::Right => 0x111,
        WinitMouseButton::Middle => 0x112,
        WinitMouseButton::Forward => 0x115,
        WinitMouseButton::Back => 0x116,
        WinitMouseButton::Other(b) => {
            if is_x11 {
                xorg_mouse_to_libinput(b as u32)
            } else {
                b as u32
            }
        }
    }
}

/// Converts an X11 mouse button number to the libinput/evdev numbering.
/// Taken from the same source Smithay's own (unreachable) copy cites:
/// <https://sources.debian.org/src/xserver-xorg-input-libinput/1.1.0-1/src/xf86libinput.c/?hl=1508#L236-L252>
fn xorg_mouse_to_libinput(xorg: u32) -> u32 {
    match xorg {
        0 => 0,
        1 => 0x110,            // BTN_LEFT
        2 => 0x112,            // BTN_MIDDLE
        3 => 0x111,            // BTN_RIGHT
        _ => xorg - 8 + 0x113, // BTN_SIZE
    }
}

impl InputBackend for NestedInput {
    type Device = NestedVirtualDevice;
    type KeyboardKeyEvent = UnusedEvent;
    type PointerAxisEvent = NestedAxisEvent;
    type PointerButtonEvent = NestedButtonEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = NestedMotionEvent;

    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;

    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SwitchToggleEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;
}

/// This backend's own `WinitGraphicsBackend<GlesRenderer>` — the EGL/GLES
/// context and window pair, built directly rather than through Smithay's
/// `init_from_attributes_with_gl_attr` so [`init_nested_winit`] can hand back
/// an event pump that also owns the raw `winit::event_loop::EventLoop` (see
/// the module section above). Field-for-field and method-for-method
/// identical to Smithay's type, hardcoded to [`GlesRenderer`] rather than
/// generic — the only renderer this backend ever binds.
pub(crate) struct NestedWinitBackend {
    renderer: GlesRenderer,
    // Unused after construction but must outlive `egl_surface`.
    _display: EGLDisplay,
    egl_surface: EGLSurface,
    window: Arc<WinitWindow>,
    damage_tracking: bool,
    bind_size: Option<Size<i32, Physical>>,
}

impl NestedWinitBackend {
    /// Window size of the underlying window, in physical pixels.
    pub(crate) fn window_size(&self) -> Size<i32, Physical> {
        let (w, h): (i32, i32) = self.window.inner_size().into();
        (w, h).into()
    }

    /// Reference to the underlying window.
    pub(crate) fn window(&self) -> &WinitWindow {
        &self.window
    }

    /// Access the underlying renderer.
    pub(crate) fn renderer(&mut self) -> &mut GlesRenderer {
        &mut self.renderer
    }

    /// Bind the underlying window to the underlying renderer, resizing the
    /// EGL surface first if the window size changed since the last bind —
    /// identical ordering to Smithay's own `bind` (see its doc comment for
    /// why: resizing after `make_current` latches the back buffer on some
    /// drivers).
    pub(crate) fn bind(
        &mut self,
    ) -> Result<
        (
            &mut GlesRenderer,
            <GlesRenderer as RendererSuper>::Framebuffer<'_>,
        ),
        SwapBuffersError,
    > {
        let window_size = self.window_size();
        if Some(window_size) != self.bind_size {
            self.egl_surface.resize(window_size.w, window_size.h, 0, 0);
        }
        self.bind_size = Some(window_size);

        let fb = self.renderer.bind(&mut self.egl_surface)?;
        Ok((&mut self.renderer, fb))
    }

    /// Submit the back buffer to the window by swapping.
    pub(crate) fn submit(
        &mut self,
        damage: Option<&[Rectangle<i32, Physical>]>,
    ) -> Result<(), SwapBuffersError> {
        let mut damage = match damage {
            Some(damage) if self.damage_tracking && !damage.is_empty() => {
                let bind_size = self
                    .bind_size
                    .expect("submitting without ever binding the renderer.");
                Some(
                    damage
                        .iter()
                        .map(|rect| {
                            Rectangle::new(
                                (rect.loc.x, bind_size.h - rect.loc.y - rect.size.h).into(),
                                rect.size,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        };

        self.window.pre_present_notify();
        self.egl_surface.swap_buffers(damage.as_deref_mut())?;
        Ok(())
    }
}

/// One event from this backend's own winit event pump. The pointer/resize/
/// focus/close/redraw variants mirror Smithay's `WinitEvent` exactly; `Key`
/// is the addition issue #118 exists for — the one event Smithay's own type
/// cannot carry `logical_key` on.
pub(crate) enum NestedWinitEvent {
    /// The window has been resized.
    Resized,
    /// The focus state of the window changed.
    Focus(bool),
    /// A non-keyboard input event: routed through [`input::intake_physical`]
    /// exactly as before, now instantiated over [`NestedInput`].
    Input(InputEvent<NestedInput>),
    /// One key press or release that passed [`admits_key_event`] — never an
    /// autorepeat, never a synthetic press, but *including* the synthetic
    /// releases winit emits on X11 focus loss — with both the evdev scancode
    /// (for the layout-invariant table) and winit's resolved host keysym,
    /// when there is one (issue #118's payload).
    Key {
        evdev: u32,
        host_keysym: Option<u32>,
        state: SeatKeyState,
    },
    /// The user requested to close the window.
    CloseRequested,
    /// A redraw was requested.
    Redraw,
}

/// Per-window state [`NestedWinitEventsApp`] needs across events: the key
/// counter that filters autorepeat (Smithay's own handler keeps the same
/// counter, for the same reason — winit's `event.repeat` alone does not
/// distinguish "still held from before this run" at startup), the clock
/// events are timestamped from, and whether this is an X11 or Wayland host
/// (the button-code table's one platform difference).
struct NestedWinitEventsInner {
    window: Arc<WinitWindow>,
    clock: Clock<Monotonic>,
    key_counter: u32,
    is_x11: bool,
}

/// This backend's own `WinitEventLoop` — a `calloop::EventSource` that pumps
/// a winit `EventLoop<()>` this backend owns outright (see the module
/// section above for why owning it, rather than using Smithay's, is the
/// point). Structurally identical to Smithay's type: a `Generic` wrapper
/// registers the winit loop's own wakeup fd with calloop, and
/// `process_events`/`before_sleep` pump it with a bounded
/// `ApplicationHandler` that turns `WindowEvent`s into [`NestedWinitEvent`]s.
pub(crate) struct NestedWinitEvents {
    inner: NestedWinitEventsInner,
    fake_token: Option<Token>,
    pending_events: Vec<NestedWinitEvent>,
    event_loop: Generic<HostEventLoop<()>>,
}

impl NestedWinitEvents {
    fn dispatch_new_events<F>(&mut self, callback: F) -> PumpStatus
    where
        F: FnMut(NestedWinitEvent),
    {
        // SAFETY: mirrors Smithay's own `dispatch_new_events` — the wrapped
        // `EventLoop` is never dropped by us while this reference is live.
        let event_loop = unsafe { self.event_loop.get_mut() };
        event_loop.pump_app_events(
            Some(Duration::ZERO),
            &mut NestedWinitEventsApp {
                inner: &mut self.inner,
                callback,
            },
        )
    }
}

impl EventSource for NestedWinitEvents {
    type Event = NestedWinitEvent;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        let mut pending_events = std::mem::take(&mut self.pending_events);
        let callback = |event| pending_events.push(event);
        // Drain winit's own event loop before going to sleep, so a wakeup
        // from another thread is not missed — same ordering as Smithay's
        // own `before_sleep`.
        self.dispatch_new_events(callback);
        self.pending_events = pending_events;
        if self.pending_events.is_empty() {
            Ok(None)
        } else {
            Ok(Some((Readiness::EMPTY, self.fake_token.unwrap())))
        }
    }

    fn process_events<F>(
        &mut self,
        _readiness: Readiness,
        _token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut callback = |event| callback(event, &mut ());
        for event in self.pending_events.drain(..) {
            callback(event);
        }
        Ok(match self.dispatch_new_events(callback) {
            PumpStatus::Continue => PostAction::Continue,
            PumpStatus::Exit(_) => PostAction::Remove,
        })
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.fake_token = Some(token_factory.token());
        self.event_loop.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.event_loop.register(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.event_loop.unregister(poll)
    }
}

/// The [`ApplicationHandler`] that actually pumps [`NestedWinitEvents`]'s
/// winit loop. Mirrors Smithay's own `WinitEventLoopApp` variant for
/// variant, with the one addition issue #118 exists for: `KeyboardInput`
/// resolves `event.logical_key` through [`input::host_keysym`] instead of
/// discarding it, and reaches the core as [`NestedWinitEvent::Key`] rather
/// than an `InputEvent::Keyboard` — the dedicated path
/// [`NestedState::handle_key`] feeds straight to [`input::physical_key`].
struct NestedWinitEventsApp<'a, F: FnMut(NestedWinitEvent)> {
    inner: &'a mut NestedWinitEventsInner,
    callback: F,
}

impl<F: FnMut(NestedWinitEvent)> NestedWinitEventsApp<'_, F> {
    fn timestamp(&self) -> u64 {
        self.inner.clock.now().as_micros()
    }
}

impl<F: FnMut(NestedWinitEvent)> ApplicationHandler for NestedWinitEventsApp<'_, F> {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(_size) => {
                trace!("host window resized");
                (self.callback)(NestedWinitEvent::Resized);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                (self.callback)(NestedWinitEvent::Resized);
            }
            WindowEvent::RedrawRequested => {
                (self.callback)(NestedWinitEvent::Redraw);
            }
            WindowEvent::CloseRequested => {
                (self.callback)(NestedWinitEvent::CloseRequested);
            }
            WindowEvent::Focused(focused) => {
                (self.callback)(NestedWinitEvent::Focus(focused));
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if admits_key_event(is_synthetic, event.repeat, event.state) => {
                match event.state {
                    ElementState::Pressed => self.inner.key_counter += 1,
                    ElementState::Released => {
                        self.inner.key_counter = self.inner.key_counter.saturating_sub(1);
                    }
                }
                let evdev = event.physical_key.to_scancode().unwrap_or(0);
                // The #118 payload: winit's interpreted key, resolved to an
                // X keysym when it names a character. `None` for named/dead
                // keys — `physical_key` falls back to the layout-invariant
                // table for those (Escape, Enter, arrows, …), so dead-man
                // and every other layout-invariant path is unaffected.
                let host_keysym = input::host_keysym(&event.logical_key);
                (self.callback)(NestedWinitEvent::Key {
                    evdev,
                    host_keysym,
                    state: match event.state {
                        ElementState::Pressed => SeatKeyState::Pressed,
                        ElementState::Released => SeatKeyState::Released,
                    },
                });
            }
            WindowEvent::CursorMoved { position, .. } => {
                let size = self.inner.window.inner_size();
                let x = position.x / size.width as f64;
                let y = position.y / size.height as f64;
                // The timestamp is read into a local first: `(self.callback)`
                // takes `&mut self.callback` for the whole call, and
                // `self.timestamp()` borrows all of `self` — evaluating it
                // as part of the call's argument list would overlap the two
                // borrows.
                let time = self.timestamp();
                (self.callback)(NestedWinitEvent::Input(InputEvent::PointerMotionAbsolute {
                    event: NestedMotionEvent {
                        time,
                        position: NestedRelativePosition { x, y },
                        global_x: position.x,
                        global_y: position.y,
                    },
                }));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let time = self.timestamp();
                (self.callback)(NestedWinitEvent::Input(InputEvent::PointerAxis {
                    event: NestedAxisEvent { time, delta },
                }));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let time = self.timestamp();
                let is_x11 = self.inner.is_x11;
                (self.callback)(NestedWinitEvent::Input(InputEvent::PointerButton {
                    event: NestedButtonEvent {
                        time,
                        button,
                        state,
                        is_x11,
                    },
                }));
            }
            // Every other class -- touch, gestures, IME, file drag, tablet,
            // window chrome -- is unhandled: `intake_physical` has never
            // resolved touch/gestures (v0's seat vocabulary is pointer +
            // keyboard, `crate::input`'s module doc), so dropping them here
            // regresses nothing observable.
            _ => {}
        }
    }
}

/// Whether one winit keyboard event reaches intake.
///
/// Split out of [`NestedWinitEventsApp::window_event`] so the filter is
/// pinned by a test that needs no host window — the whole event handler is
/// otherwise unreachable from CI, which is how the release half of this
/// decision shipped inverted and stayed green.
///
/// - **Autorepeats never do.** A held key produces no new physical fact and
///   the dead-man switch owns its own clock (module docs), so repeats are
///   pure noise on the wire.
/// - **Synthetic *presses* never do.** Those are winit's report of keys
///   already down when focus *arrived*: the core did not see that press
///   begin, the human may well have pressed it in another window entirely,
///   and admitting it would hand the confined app — and the dead-man
///   watcher — a press with no gesture behind it.
/// - **Synthetic *releases* do, and this is the #124 regression it fixes.**
///   Those are exactly the events winit's X11 backend emits for keys that
///   were down when the window lost focus, and they are the only notice the
///   core ever gets that such a key came up. Filtering them left the key
///   latched down in the confined app indefinitely — a stuck modifier the
///   human cannot clear, because the release they eventually perform is
///   delivered to whatever window took focus. Smithay's own handler drops
///   them too, which is why the earlier comment here cited it; matching a
///   compositor that also owns the keyboard focus it lost is not the same
///   situation as a nested core that does not.
///
/// A release admitted this way is still *paired*: the router delivers a
/// release only if it delivered its press
/// ([`input::InputRouter::route_physical`]),
/// so a synthetic release for a key the app never saw pressed goes nowhere.
/// The Wayland half of the same hazard — `wl_keyboard.leave` emits no key
/// events at all — is covered by [`NestedState::handle_focus`], which pays
/// the app whatever releases the router still shows outstanding.
fn admits_key_event(is_synthetic: bool, repeat: bool, state: ElementState) -> bool {
    if repeat {
        return false;
    }
    !is_synthetic || matches!(state, ElementState::Released)
}

/// Build this backend's own [`NestedWinitBackend`]/[`NestedWinitEvents`]
/// pair — the direct replacement for Smithay's
/// `init_from_attributes_with_gl_attr` (see the module section above for
/// why). EGL setup below is copied step for step from Smithay's own
/// `backend::winit::init_from_attributes_with_gl_attr` (verified against the
/// pinned `=0.7.0` source): same `EGLDisplay`/`EGLContext` construction, same
/// 10-bit-then-8-bit pixel format fallback, same Wayland/X11 native-surface
/// branch. Only the window/event-loop creation and the `ApplicationHandler`
/// wired to it differ.
fn init_nested_winit(
    attributes: WindowAttributes,
    gl_attributes: GlAttributes,
) -> Result<(NestedWinitBackend, NestedWinitEvents), Box<dyn Error>> {
    let event_loop: HostEventLoop<()> = HostEventLoop::builder().build()?;

    #[allow(deprecated)]
    let window = Arc::new(event_loop.create_window(attributes)?);

    let (display, context, surface, is_x11) = {
        let display = unsafe { EGLDisplay::new(window.clone())? };

        let context = EGLContext::new_with_config(
            &display,
            gl_attributes,
            PixelFormatRequirements::_10_bit(),
        )
        .or_else(|_| {
            EGLContext::new_with_config(&display, gl_attributes, PixelFormatRequirements::_8_bit())
        })?;

        let (surface, is_x11) = match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Wayland(handle)) => {
                debug!("nested winit backend: Wayland");
                let size = window.inner_size();
                let surface = unsafe {
                    wegl::WlEglSurface::new_from_raw(
                        handle.surface.as_ptr() as *mut _,
                        size.width as i32,
                        size.height as i32,
                    )
                }
                .map_err(|err| Box::new(err) as Box<dyn Error>)?;
                unsafe {
                    (
                        EGLSurface::new(
                            &display,
                            context
                                .pixel_format()
                                .expect("configured context has a pixel format"),
                            context.config_id(),
                            surface,
                        )?,
                        false,
                    )
                }
            }
            Ok(RawWindowHandle::Xlib(handle)) => {
                debug!("nested winit backend: X11");
                unsafe {
                    (
                        EGLSurface::new(
                            &display,
                            context
                                .pixel_format()
                                .expect("configured context has a pixel format"),
                            context.config_id(),
                            native::XlibWindow(handle.window),
                        )?,
                        true,
                    )
                }
            }
            _ => {
                return Err("nested backend requires a Wayland or X11 host compositor".into());
            }
        };

        let _ = context.unbind();
        (display, context, surface, is_x11)
    };

    let renderer = unsafe { GlesRenderer::new(context)? };
    let damage_tracking = display.supports_damage();

    event_loop.set_control_flow(smithay::reexports::winit::event_loop::ControlFlow::Poll);
    // Not threaded through `NestedWinitEvent::Resized`, unlike Smithay's own
    // `WinitEvent::Resized`: every consumer in this crate re-reads the
    // window size on demand (`NestedWinitBackend::window_size`) rather than
    // taking it off the event, so there is nothing here that needs it.
    let event_loop = Generic::new(event_loop, Interest::READ, Mode::Level);

    Ok((
        NestedWinitBackend {
            window: window.clone(),
            _display: display,
            egl_surface: surface,
            damage_tracking,
            bind_size: None,
            renderer,
        },
        NestedWinitEvents {
            inner: NestedWinitEventsInner {
                window,
                clock: Clock::<Monotonic>::new(),
                key_counter: 0,
                is_x11,
            },
            fake_token: None,
            pending_events: Vec::new(),
            event_loop,
        },
    ))
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
    backend: NestedWinitBackend,
    /// **Every realm's scene, and the one bound to the host window**
    /// (P1.3.3; WS-E.1.3, issue #209) — the same composition implementation
    /// the headless backend retains for capture; this backend presents the
    /// *bound* realm's output 1:1 in the host window. The shim-facing
    /// protocol server (P1.3.4) commits into the committing realm's own
    /// scene and requests a redraw.
    scenes: RealmScenes,
    /// The consent surface (P1.7.1): the prompt composited above the realm
    /// view in the host window. Driven now (issue #90): once per dispatch
    /// round [`session::RuntimeHost::service_consent`] calls
    /// [`session::service_consent_round`], which raises the front pending
    /// petition's card here and lowers it again when the petition is decided
    /// or leaves the table. It is empty only while no petition is pending.
    consent: ConsentSurface,
    /// The lock surface (WS-E.2.2, issue #214): the opaque cover and card
    /// composited above everything but the trusted band while the session is
    /// locked. Driven by [`session::service_lock_round`] on the same
    /// once-per-dispatch cadence [`Self::consent`] is, from the very same
    /// [`crate::lock::LockScreen`] the router's outermost gate judges against —
    /// so "the pixels are up" and "physical input is being consumed" are one
    /// state rather than two that can drift.
    lock: crate::lock::LockSurface,
    /// The idle blank's cover (WS-E.4.3, issue #223), **permanently lowered on
    /// this backend**.
    ///
    /// `--blank-idle` is refused with `--nested` at startup
    /// ([`crate::backend::blank::BLANK_NEEDS_THE_OUTPUT`]), because a nested
    /// `vitrind` painting its own window black would be asserting something
    /// about a screen the host compositor owns -- the same reason
    /// [`session::PromptVisibility`] refuses to read winit's `Occluded`. The
    /// surface is still held and still composited through, on
    /// [`Self::lock`]'s terms: a backend that simply had no cover would be a
    /// backend the shared output stage has to have a branch for, and the branch
    /// is where the next presentation path forgets.
    blank: super::blank::BlankSurface,
    /// This session's trusted indicator (issue #85), the same value
    /// [`Self::consent`] frames its prompts and paints its band in.
    ///
    /// Held here as well because the two presentation paths need it in two
    /// different shapes: the CPU path paints the band through the consent
    /// surface's own canvas, while the zero-copy path
    /// ([`crate::dmabuf::present_human_visible`]) has no CPU canvas and hands
    /// the colour straight to the renderer. Both are seeded from the one
    /// `RuntimeSeed::indicator` minted before the listener accepted anyone,
    /// on the same line of `run_inner`, so they cannot be two secrets;
    /// `no_presentation_path_can_drop_the_trusted_band` re-checks that
    /// against what the consent surface actually paints rather than trusting
    /// the construction site.
    indicator: TrustedIndicator,
    texture: Option<SceneTexture>,
    /// **Every realm's** retained zero-copy GPU content: one slot per realm,
    /// filled by a `kind=dmabuf` commit and emptied by a replacement or by
    /// that realm's death (P1.3.5, issue #117; keyed by realm since WS-E.1.3,
    /// issue #209). Lives here rather than in [`Scene`] because it is GPU
    /// state bound to this backend's own [`GlesRenderer`] — [`Scene`] stays
    /// renderer-free and shared with the headless backend, which has no GPU
    /// to hold this on. Every slot empty on every path that has not imported
    /// a dmabuf: the ordinary shm/CPU-compose path, unchanged.
    ///
    /// Keyed for the same reason [`Self::scenes`] is, and it is not a
    /// symmetry argument: while this was one slot for the session, the
    /// zero-copy branch of [`NestedState::try_redraw`] presented whichever
    /// realm imported last, so a **hidden** realm's commit took the human's
    /// window. [`RealmGpuContent`] carries the whole argument.
    dmabuf_content: RealmGpuContent,
    /// The agent-owned pointer position this window draws the cursor sprite
    /// at (D-019), pushed here once per dispatch round by
    /// [`session::Presenter::set_agent_cursor`] from
    /// [`crate::input::InputRouter::agent_pointer`].
    ///
    /// A copy rather than a borrow of the router because the composite happens
    /// on the *host's* frame clock, long after the dispatch round that moved
    /// the pointer has ended: `redraw` here only schedules, and `try_redraw`
    /// runs from `WinitEvent::Redraw`. Both presentation paths read it — the
    /// CPU upload through [`window_pixels`], the zero-copy path through
    /// [`crate::dmabuf::present_human_visible`] — because a path that drew no
    /// sprite would silently be the one issue #85 was about, one property
    /// milder.
    agent_cursor: Option<(f64, f64)>,
    /// Whether the human's **attention window** is open right now (WS-E.1.7),
    /// pushed here once per dispatch round by
    /// [`session::Presenter::set_attention`].
    ///
    /// A copy rather than a borrow of the signal, for exactly the reason
    /// [`Self::agent_cursor`] is one: the composite happens on the host's frame
    /// clock, after the dispatch round that gated the press has ended. Read by
    /// the CPU path through [`window_pixels`]; the zero-copy dmabuf path draws
    /// no marker, which is a **cosmetic** gap rather than the band's kind — the
    /// marker asserts nothing about authenticity and carries no session secret,
    /// so a frame missing it is a frame the human has to check by other means
    /// rather than a frame that can lie to them.
    attention: bool,
    /// The status strip (WS-E.2.3, issue #215): the snapshot, its cached CPU
    /// raster, and the generation both presentation paths key on.
    ///
    /// Read by the CPU path through [`window_pixels`] and — unlike the
    /// attention marker above — by the **zero-copy path too**, as a texture
    /// ([`Self::status_texture`]). That is not symmetry for its own sake: this
    /// backend is the one that can actually reach 2560x1600@240, the strip is
    /// on every frame rather than transiently, and a status bar that vanished
    /// whenever the app took the fast path would be a status bar nobody could
    /// rely on.
    status: crate::status::StatusStrip,
    /// The strip's GPU texture and the `(generation, width)` it was uploaded
    /// for.
    ///
    /// Re-uploaded only when that key changes, which is once a minute at
    /// steady state (`HH:MM`, no seconds) plus once per resize. The bytes come
    /// from [`crate::status::StatusStrip::raster`] — the *same* cache the CPU
    /// blit uses — so the two paths cannot draw different strips.
    status_texture: Option<(u64, u32, GlesTexture)>,
    /// The dead-man switch, shared with [`NestedState::deadman`] and the
    /// router's hook (WS-E.4.2, issue #222).
    ///
    /// Unlike `backend::drm`'s copy this one is **not** read by the composite —
    /// `try_redraw` samples the hold from `NestedState`, which has the whole
    /// state in hand. It is here for [`session::Presenter::output_gates`],
    /// which is a `&NestedView` method called from the runtime's dispatch round
    /// where only the view is reachable, and which must answer whether a
    /// dead-man hold is in progress.
    deadman: Rc<RefCell<DeadManSwitch>>,
}

/// Per-run state of the nested backend: its presentation half, the session
/// runtime, and the dead-man / consent wiring the host window feeds.
/// **The nested backend's whole hook stack, named once.**
///
/// Spelled as a type alias rather than inline at the construction site because
/// the ORDER is the decision (WS-E.1.7 decision 11, extended by WS-E.2.2), and
/// a decision written in two places is a decision that drifts. Outermost first:
///
/// 1. [`crate::lock::LockGate`] — consumes *all* physical input while the
///    session is locked, so nothing below it can be answered, chorded or typed
///    at by somebody who is not there. Outermost because a lock that something
///    else could preempt is not a lock; its `observe` tap is unconditional **by
///    construction** ([`crate::input::ConsumingGate`]), so being outermost costs
///    the dead-man switch nothing.
/// 2. [`ConsentGate`] — a raised prompt owns physical input.
/// 3. [`DeadManHook`] — the human's off-switch; detection in `observe`, which
///    every hook above forwards unconditionally.
/// 4. [`crate::clipboard::ClipboardHook`] — outside the attention hook because
///    that hook eats both Supers, and a modifier matcher inside it would have a
///    permanently wrong `super` bit (WS-E.2.1).
/// 5. [`crate::screenshot::ScreenshotHook`] — WS-E.2.4. Below the lock, so a
///    locked session cannot be photographed by whoever is standing at it;
///    below the clipboard, its peer, because the clipboard moves an app's
///    bytes across a realm boundary and this only reads; above the attention
///    hook, for the clipboard's own reason (a modifier matcher inside it would
///    never see a Super press).
/// 6. [`crate::backlight::BacklightHook`] — D-041. Consumes the two
///    `XF86MonBrightness*` keys when a session passed `--backlight`, and
///    delivers them untouched when it did not. Its position is unobservable
///    and says so: the two keysyms it consumes are not modifiers and are in no
///    chord's vocabulary, so no matcher above or below it can have its state
///    changed by what this takes. Below the lock for the reason the screenshot
///    hook is — the lock's rule is that somebody who is not the human cannot
///    operate the machine, and "operate" must not acquire an exception list.
/// 7. [`crate::attention::AttentionHook`] — innermost, therefore suppressible
///    by everything above, which is the correct posture for a mechanism whose
///    worst failure is a focus change that does not happen.
///
/// `the_lock_gate_is_the_outermost_hook` pins position 1 against this alias,
/// because [`crate::lock::gate`] tracks chord modifiers in its `judge` and that
/// is sound *only* while nothing above it can eat an event.
pub(crate) type NestedHook = crate::lock::LockGate<
    ConsentGate<
        DeadManHook<
            crate::clipboard::ClipboardHook<
                crate::screenshot::ScreenshotHook<
                    crate::backlight::BacklightHook<
                        crate::attention::AttentionHook<input::NoopHook>,
                    >,
                >,
            >,
        >,
    >,
>;

pub(crate) struct NestedState {
    view: NestedView,
    /// The session runtime ([`crate::session`]): the core socket's listener,
    /// every accepted principal connection, the capability kernel, and the
    /// realm's shim session. It carries the router below rather than owning a
    /// second one, so an agent's chokepoint-admitted actuations and a human's
    /// physical input share one implicit-grab and pointer state — which is
    /// what makes the preemption hook mean anything.
    runtime: Runtime<NestedHook>,
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
    /// embedder that drives `route` advances this cell first, and
    /// `session::route_physical_turn` hands the same sample to the router's
    /// presence record (`input::InputRouter::observe_at`). The grab reads it
    /// for its
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
    /// The lock screen's state (WS-E.2.2, issue #214), shared with the
    /// router's **outermost** gate: whether the session is locked, the
    /// passphrase being typed at it, and the facts the recorder is owed.
    ///
    /// Shared rather than mirrored, for the reason [`Self::grab`] is: the
    /// alternative is a window in which the pixels say "locked" and the gate
    /// does not, which is a lock screen an app can type through.
    lock: Rc<RefCell<crate::lock::LockScreen>>,
    /// The dead-man chord's name, carried here for one purpose: the lock card
    /// tells the human what *does* revoke an agent's authority, and an
    /// instruction naming the wrong key would be worse than none.
    deadman_chord: &'static str,
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
pub fn run(
    dead_man: DeadManConfig,
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
    lock: crate::lock::LockConfig,
    status: crate::status::StatusConfig,
    seed: RuntimeSeed,
) -> (Recorder, Result<(), Box<dyn Error>>) {
    // The seed is consumed the moment the state is built; until then it is
    // still ours, and either way the recorder must come back so `run_session`
    // can write the footer it owes. Threading it through two slots keeps `?`
    // usable for every startup step below instead of turning each into a
    // four-line `match`.
    let mut seed = Some(seed);
    let mut recovered = None;
    let result = run_inner(
        dead_man,
        attention,
        clipboard,
        screenshot,
        lock,
        status,
        &mut seed,
        &mut recovered,
    );
    let recorder = recovered
        .or_else(|| seed.take().map(|seed| seed.recorder))
        .expect("the seed is either still unconsumed or its recorder was recovered");
    (recorder, result)
}

// Eight parameters, one over clippy's default, and each is one of this
// session's five core-owned gestures or one of the two slots `run` threads the
// seed and the recorder back through. Bundling the chords into a struct would
// hide the one thing this signature is good for: every physical gesture this
// build owns is named here, once.
#[allow(clippy::too_many_arguments)]
fn run_inner(
    dead_man: DeadManConfig,
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
    lock: crate::lock::LockConfig,
    status: crate::status::StatusConfig,
    seed: &mut Option<RuntimeSeed>,
    recovered: &mut Option<Recorder>,
) -> Result<(), Box<dyn Error>> {
    // **The passphrase file, before anything else exists.** Read here rather
    // than at parse time because `Action` must stay `PartialEq` and printable,
    // and read *first* because a session that came up with an unreadable,
    // malformed or world-readable passphrase file would be a session whose lock
    // cannot be answered -- the fail-open configuration trap `realm.rs` and
    // `deadman.rs` both refuse. `?` here is a non-zero exit with the reason
    // named, before the listener accepts anyone.
    let verifier = match lock.passphrase.as_deref() {
        Some(path) => Some(crate::lock::passphrase::PassphraseFile::load(path)?),
        None => None,
    };
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

    // vsync on (Smithay's default is off): while a redraw chain is running
    // (a dirty frame, or an animating dead-man hold), the blocking swap
    // paces it to the host's refresh rate. Because drivers are free to
    // ignore the default swap interval, [`FRAME_BUDGET`] additionally caps
    // the chain when the swap does not block (see its doc comment). Outside
    // a running chain the window idles at 0 fps (P1.3.9, issue #117) —
    // vsync paces frames that happen, it does not manufacture them.
    let (backend, winit_source) = init_nested_winit(
        WinitWindow::default_attributes()
            .with_inner_size(LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1))
            .with_title("vitrind (nested)")
            // The Wayland app_id, and it is not cosmetic. `with_inner_size`
            // above is a *request*: a tiling compositor (Hyprland, Sway,
            // river) ignores it and hands the window whatever its tile is,
            // which silently invalidates any absolute coordinate the demo or
            // a recording recipe pins to INITIAL_SIZE. The operator's fix is
            // a float-and-size rule, and every compositor matches those on
            // app_id -- so without this there is nothing to match and the
            // rule cannot be written at all. Stable by contract: recipes in
            // docs/demo/ name this string.
            .with_name(NESTED_APP_ID, "nested"),
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: true,
        },
    )?;
    info!(size = ?backend.window_size(), "nested backend initialized");

    loop_handle.insert_source(winit_source, |event, _, state| match event {
        NestedWinitEvent::Redraw => state.redraw(),
        NestedWinitEvent::Resized => {
            let size = state.view.backend.window_size();
            debug!(?size, "host window resized");
            // Every realm composes at the output's size, bound or not, so a
            // resize moves every realm's geometry — and every realm in the
            // FULLSCREEN arrangement is re-configured at the new size,
            // because that is what `set_fullscreen` normatively promises
            // ("and again whenever the output resizes while the realm is in
            // it"). Both halves live in `session::apply_output_resize` so
            // this backend cannot do one without the other; see that
            // function for why the pairing is the whole point.
            session::apply_output_resize(state, (size.w.max(0) as u32, size.h.max(0) as u32));
            // Drop the uploaded view; the next redraw recomposes it 1:1 at
            // the new size (kept pixel-exact for the P1.3.2/P1.3.6 goldens).
            state.view.texture = None;
            state.view.backend.window().request_redraw();
        }
        NestedWinitEvent::CloseRequested => {
            info!("host window close requested");
            state.loop_signal.stop();
        }
        // P1.3.7 input intake: nested-mode host events ARE the human
        // principal's input, origin-tagged `physical` at this single point
        // of entry (B2) — see `crate::input`.
        NestedWinitEvent::Input(event) => state.handle_input(&event),
        // #118: the one event class Smithay's own `WinitEvent` cannot
        // carry `logical_key` on. Routed straight to `input::physical_key`,
        // bypassing `intake_physical`'s scancode-only `Keyboard` arm — see
        // `NestedState::handle_key`.
        NestedWinitEvent::Key {
            evdev,
            host_keysym,
            state: key_state,
        } => state.handle_key(evdev, host_keysym, key_state),
        // Not ignorable: a key held when focus leaves produces no release
        // event on either backend, so the dead-man switch must be told the
        // hold is no longer verifiable (see `handle_focus`).
        NestedWinitEvent::Focus(focused) => state.handle_focus(focused),
    })?;

    let grab = Rc::new(RefCell::new(ConsentGrab::new()));
    let now = Rc::new(Cell::new(Instant::now()));
    let deadman = Rc::new(RefCell::new(DeadManSwitch::new(dead_man)));
    info!(
        chord = dead_man.chord.name(),
        hold_ms = dead_man.hold.as_millis(),
        "dead-man switch armed: holding this key revokes every grant in the session"
    );
    // The physical-presence map the chokepoint judges `preempted` against.
    // Handed to the router, which feeds it at its own tap and hands the same
    // handle on to `Runtime::new` for the kernel — one map, and no wiring step
    // here to forget (`input::InputRouter`'s "Presence is not a stackable
    // hook"). On the same `now` cell the grab and the watcher read, so all
    // three see the dispatch turn's single sampled instant.
    // The attention signal (WS-E.1.7). Minted here and handed to the hook
    // alone: `Runtime::new` takes it back **out of the router**
    // (`InputRouter::attention`), exactly as it does the presence map, so the
    // kernel's signal and the hook's are one object by construction and there
    // is no wiring step here to forget.
    info!(
        chord = attention.name(),
        window_ms = crate::attention::ATTENTION_WINDOW.as_millis(),
        "attention key armed: tapping it lifts `preempted` for one layout use by a holder \
         of layout authority. It delegates nothing -- every authority exercised afterwards \
         came from a grant a human approved on a consent card"
    );
    // The clipboard chords (WS-E.2.1), named in full at startup for the reason
    // the attention chord is: both are keys the core takes away from every app
    // in every realm, and an operator has to be able to see which.
    let clipboard_signal = Rc::new(RefCell::new(
        crate::clipboard::ClipboardSignal::new(clipboard)
            .expect("the clipboard chord pair was validated at startup"),
    ));
    {
        let signal = clipboard_signal.borrow();
        let spellings: Vec<String> = signal.chords().map(|c| c.spelling()).collect();
        info!(
            key = signal.trigger(),
            chords = spellings.join(", "),
            max_bytes = crate::clipboard::MAX_CLIPBOARD_BYTES,
            mime = crate::clipboard::CLIPBOARD_MIME,
            "cross-realm clipboard armed: the first chord asks the focused realm for its \
             selection, the second offers the core's slot to the focused realm. Both are \
             CONSUMED in every realm, so an app that binds them loses them"
        );
    }
    // The screenshot chord (WS-E.2.4), named in full at startup for the reason
    // the two above are -- and with the one thing an operator most needs to
    // have been told BEFORE they press it, which is what the file will not
    // contain.
    let screenshot_signal = Rc::new(RefCell::new(
        crate::screenshot::ScreenshotSignal::new(screenshot)
            .expect("the screenshot chord was validated at startup"),
    ));
    info!(
        chord = screenshot_signal.borrow().spelling(),
        "screenshot key armed: it is CONSUMED in every realm. What it writes is the REALM \
         VIEW -- no trusted band, no consent prompt, no lock screen, no status strip, no \
         agent cursor -- because the band's colour is this session's secret and the \
         confined app can read any file this core writes. With no --screenshot-dir the \
         chord is still consumed and writes nothing"
    );
    // The lock screen (WS-E.2.2), named in full at startup for the reason the
    // attention and clipboard chords are: it is a key the core takes away from
    // every app in every realm, and it is the one whose consequence a human
    // most needs to have been told about before it happens to them.
    // The session's activity clock. Minted here, handed to the lock's gate and
    // to nothing else on this backend: `--blank-idle` is refused with
    // `--nested` (`crate::backend::blank::BLANK_NEEDS_THE_OUTPUT`), so nothing
    // here ticks the blank and `NestedView::blank` stays down for the whole
    // session. The clock is still shared-shaped rather than owned, so the two
    // backends construct the lock identically.
    let activity = Rc::new(RefCell::new(super::blank::SessionActivity::new(
        None,
        Instant::now(),
    )));
    // **No `with_seat_change_policy` here, deliberately** (issue #246): this
    // backend has no seat to lose. A host compositor's focus loss is not a seat
    // loss -- the host keeps the session, the human can still see the window,
    // and no session event ever reaches this core -- so `set_seat_absent` is
    // never called on this screen and a policy would be a claim the code does
    // not implement. `--lock-on-seat-change` is refused with `--nested`
    // (`crate::lock::SEAT_POLICY_NEEDS_A_SEAT`), so there is nothing to pass.
    let lock_screen = Rc::new(RefCell::new(crate::lock::LockScreen::new(
        lock.chord,
        lock.idle,
        verifier,
        Rc::clone(&activity),
    )));
    {
        let screen = lock_screen.borrow();
        info!(
            chord = screen.chord_spelling(),
            idle_s = lock.idle.map(|d| d.as_secs()),
            passphrase = matches!(
                screen.unlock_method(),
                crate::lock::UnlockMethod::Passphrase
            ),
            "lock screen armed: the chord takes every physical event away from every realm \
             until a human answers. It does NOT suspend an agent's grants -- an observe \
             holder keeps capturing across a lock (see docs/book/src/limits.md); the \
             instrument that stops everything is still the dead-man chord, which fires while \
             locked"
        );
    }
    let router = input::InputRouter::new(
        Rc::new(RefCell::new(input::PhysicalPresenceMap::new())),
        Rc::clone(&now),
        // `NestedHook`, built. The order is that alias's doc comment, which is
        // the single place this decision is written down: the lock outermost,
        // then the consent grab, the dead-man watcher, the clipboard chords,
        // the screenshot key and the attention key.
        crate::lock::lock_gate(
            Rc::clone(&lock_screen),
            Rc::clone(&now),
            ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                DeadManHook::new(
                    Rc::clone(&deadman),
                    Rc::clone(&now),
                    crate::clipboard::ClipboardHook::new(
                        Rc::clone(&clipboard_signal),
                        crate::screenshot::ScreenshotHook::new(
                            Rc::clone(&screenshot_signal),
                            crate::backlight::BacklightHook::new(
                                Rc::new(RefCell::new(crate::session::backlight_signal(
                                    seed.as_ref(),
                                ))),
                                crate::attention::AttentionHook::new(
                                    Rc::new(RefCell::new(crate::attention::AttentionSignal::new(
                                        attention,
                                    ))),
                                    Rc::clone(&now),
                                    input::NoopHook,
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        ),
    );
    // The session's trusted indicator, minted in `run_session` before the
    // listener accepted anyone (issue #85). Read by `Copy` before the seed is
    // consumed into the runtime below.
    let indicator: TrustedIndicator = seed
        .as_ref()
        .expect("the seed is present until the state is built")
        .indicator;
    // Read before `backend` moves into the state: the scene set is seeded
    // with the window's size so the very first `Resized` that reports the
    // same size is correctly a no-op for `layout_generation`.
    let backend_size = backend.window_size();
    let mut state = NestedState {
        view: NestedView {
            backend,
            // The host window's current size; the `Resized` handler feeds
            // every later change through `RealmScenes::set_view_size`.
            scenes: RealmScenes::new((backend_size.w.max(0) as u32, backend_size.h.max(0) as u32)),
            consent: ConsentSurface::new(indicator),
            lock: crate::lock::LockSurface::new(indicator),
            blank: super::blank::BlankSurface::new(),
            indicator,
            texture: None,
            dmabuf_content: RealmGpuContent::new(),
            // No agent has moved a pointer yet; the first emulated motion
            // establishes it (`InputRouter::agent_pointer`).
            agent_cursor: None,
            attention: false,
            status: crate::status::StatusStrip::new(status),
            status_texture: None,
            // An `Rc` clone of the same switch `NestedState` holds and the
            // router's hook watches, on `backend::drm`'s precedent: the view
            // needs to *read* whether a hold is in progress, because a hold is
            // one of the four overlays that deactivates a pointer constraint
            // (WS-E.4.2). A second switch would answer for a hold nobody is
            // making.
            deadman: Rc::clone(&deadman),
        },
        runtime: Runtime::new(
            seed.take().expect("the seed is consumed exactly once"),
            router,
        ),
        grab,
        now,
        deadman,
        lock: lock_screen,
        deadman_chord: dead_man.chord.name(),
        deadman_timer_armed: false,
        loop_signal: event_loop.get_signal(),
        loop_handle: event_loop.handle(),
        fatal: None,
    };

    // Kick off exactly the first frame. Past this point the redraw chain no
    // longer self-sustains (P1.3.9, issue #117): `schedule_next_frame` only
    // continues it while a dead-man hold is animating, so once this first
    // composite lands the window idles at 0 fps until a real state change —
    // a shim commit, a consent transition, physical input, a resize, or an
    // armed hold — asks for the next one via `request_redraw`.
    state.view.backend.window().request_redraw();

    // Every runtime source goes in before the loop starts, and before
    // anything spawns a realm: a shim whose socketpair is not being serviced
    // wedges permanently (see `session::install`).
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
    /// Ask the host compositor for a frame. The nested backend forwards to
    /// its window; a test host counts the requests, which is what makes
    /// "how often does an armed hold repaint" an assertable number rather
    /// than something only a live compositor could show.
    fn request_redraw(&mut self);
}

/// Where a [`deadman_tick`] call is coming from, which decides whether that
/// tick may also start the hold indicator's redraw chain.
///
/// The distinction is load-bearing rather than descriptive: a hold has to
/// animate, so *something* must keep asking for frames, but there are two
/// candidates and only one of them is budgeted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickSource {
    /// Outside the redraw chain — an input dispatch turn, a focus change.
    /// Nothing else is going to ask for a frame here, so an armed hold must
    /// kick the chain or the indicator never appears on a window idling at
    /// 0 fps (P1.3.9 stopped the chain self-sustaining). Bounded by the host
    /// event rate, and `request_redraw` is idempotent per host frame, so a
    /// burst of events still costs at most one frame each.
    OffChain,
    /// From inside [`NestedState::redraw`], where a chain is by definition
    /// already running and [`NestedState::schedule_next_frame`] paces its
    /// continuation at [`FRAME_BUDGET`].
    ///
    /// **Requesting a redraw here is the busy-spin**, and it shipped: a
    /// request issued while handling `RedrawRequested` makes winit emit the
    /// next one as soon as the loop turns, so the hold repainted as fast as
    /// the loop could go, outside the budget, for as long as the human held
    /// the key — the un-budgeted spin P1.3.9 had just removed. Coalescing
    /// does not help, because there is nothing to coalesce *with*: the frame
    /// this tick runs inside has not been drawn yet, so the request is
    /// always the only one outstanding.
    InChain,
}

/// Complete the chord if due, dispose of any trigger, keep the timer armed,
/// and — from [`TickSource::OffChain`] only — make sure an armed hold has a
/// redraw chain to animate in. Idempotent and level-triggered; safe to call
/// from anywhere, given the right `source`.
pub(crate) fn deadman_tick<D: DeadManHost + 'static>(
    host: &mut D,
    handle: &LoopHandle<'static, D>,
    now: Instant,
    source: TickSource,
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
    // Read the deadline out into a local first: the `Ref` must be dropped
    // before `request_redraw` takes `&mut host`.
    let armed = host.switch().borrow().deadline().is_some();
    if armed && source == TickSource::OffChain {
        host.request_redraw();
    }
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
///
/// Every event here is **physical**, so it is addressed by the human's
/// attention ([`input::InputRouter::route_physical`]) — the replay included,
/// since a withheld chord press is a physical event this core deferred rather
/// than a new one it invented.
///
/// `switch` is an `Option` because one backend genuinely has no dead-man
/// watcher: a `physical-input-injector` headless build stacks `NoopHook`
/// (`crate::backend::headless`), so nothing can arm a hold and there is no
/// replay to drain. Passing `None` there is what keeps the injector on **this**
/// path rather than on a second, weaker one of its own.
pub(crate) fn route_turn<H: input::PreemptionHook>(
    router: &mut input::InputRouter<H>,
    switch: Option<&RefCell<DeadManSwitch>>,
    inputs: impl IntoIterator<Item = input::SeatInput>,
    view: crate::view::ViewGeometry,
    surface: Option<(u32, u32)>,
    deliver: &mut dyn FnMut(input::SeatDelivery),
) {
    for input in inputs {
        if let Some(delivery) = router.route_physical(input, view, surface) {
            deliver(delivery);
        }
    }
    // A separate statement from the loop so the switch's borrow ends before
    // routing re-enters the hook that holds it.
    let replay = switch
        .map(|switch| switch.borrow_mut().take_replay())
        .unwrap_or_default();
    for replayed in replay {
        if let Some(delivery) = router.route_physical(replayed, view, surface) {
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

    fn request_redraw(&mut self) {
        self.view.backend.window().request_redraw();
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
    ///
    /// Pointer/axis/button only — keyboard is [`Self::handle_key`]'s (issue
    /// #118: [`input::intake_physical`]'s `Keyboard` arm is scancode-only,
    /// so it stays the generic-`InputBackend` path the unit tests drive,
    /// and this backend's own keyboard events bypass it entirely).
    fn handle_input(&mut self, event: &smithay::backend::input::InputEvent<NestedInput>) {
        let size = self.view.backend.window_size();
        let inputs = input::intake_physical(event, (size.w, size.h));
        self.route_physical_inputs(inputs, size);
    }

    /// The #118 payload: one keyboard event admitted by [`admits_key_event`]
    /// from this backend's own winit event pump, with winit's resolved
    /// `logical_key` already folded into `host_keysym` (see
    /// [`NestedWinitEventsApp::window_event`] and [`input::host_keysym`]).
    ///
    /// Calls [`input::physical_key`] directly — the same function
    /// [`input::intake_physical`]'s `Keyboard` arm calls with `None`, so a
    /// scancode with no host keysym (a modifier chord, or a generic
    /// `InputBackend` in a test) resolves identically either way, through
    /// the shared layout-invariant table ([`input::invariant_keysym`]) that
    /// keeps Esc/Enter/arrows working for dead-man regardless of which path
    /// reached it.
    fn handle_key(&mut self, evdev: u32, host_keysym: Option<u32>, state: SeatKeyState) {
        let size = self.view.backend.window_size();
        let inputs = input::physical_key(evdev, host_keysym, state);
        self.route_physical_inputs(inputs, size);
    }

    /// Shared tail of [`Self::handle_input`] and [`Self::handle_key`]: prime
    /// the consent grab's view and the dispatch turn's clock, route the
    /// intake's `SeatInput`s (plus whatever the dead-man watcher's replay
    /// owes), and tick the watcher. One turn per host event either way, so a
    /// key and a pointer event judge the grab and the watcher against
    /// exactly the same discipline the rest of the core follows.
    fn route_physical_inputs(
        &mut self,
        inputs: impl IntoIterator<Item = input::SeatInput>,
        size: Size<i32, Physical>,
    ) {
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
        // The whole of the turn — bind the router's generation, map through
        // the target realm's geometry, route, drain the dead-man watcher's
        // replay, deliver — lives in `session::route_physical_turn`. It is
        // there rather than inline here because CI has no display (D-019(4)),
        // so anything inside a `NestedState` method is unreachable by every
        // test in this workspace: that is how D-018(2)'s fifth ordering rule
        // came to have production wiring a reviewer could revert while the
        // suite stayed green.
        //
        // Disjoint field borrows: `self.view.scenes` is read while
        // `self.runtime` is borrowed mutably, so they are split here rather
        // than reached through `&mut self` twice.
        // The router maps into SURFACE coordinates, so it takes the geometry
        // rather than the bare window size: the app's first row is below the
        // rows the core reserves (issue #304), and a router that mapped against
        // the output would put a click 28 rows too high in the app. The grab
        // above keeps the bare size on purpose — the consent card is drawn in
        // output coordinates, not the app's.
        let geom = session::Presenter::view_geometry(&self.view);
        let scenes = &self.view.scenes;
        session::route_physical_turn(
            &mut self.runtime,
            scenes,
            Some(&self.deadman),
            inputs,
            geom,
            // The very sample taken above, not a second `Instant::now()`: the
            // grab, the watcher and the presence tap all judge this turn
            // against one instant.
            self.now.get(),
        );
        // Backstop 2 of 3 for the elapse check (`crate::deadman`): the
        // switch is already being asked about this turn's events, so ask it
        // about the clock too. `OffChain` — a host input event is not a
        // frame, so an armed hold needs this to start animating.
        self.deadman_tick(TickSource::OffChain);
    }

    /// The host window lost or gained keyboard focus.
    ///
    /// Focus loss is the one moment the core knows a physical key release
    /// will be delivered *somewhere else*, and it has two victims. Both are
    /// handled here, because a fix to either alone leaves a stuck key.
    ///
    /// **The dead-man hold** is forgotten. Without this, an ordinary alt-tab
    /// with Esc down either revokes the whole session a second later with no
    /// gesture behind it, or — after such a fire — leaves the switch in a
    /// state only a release can exit, silently dead with no indicator to say
    /// so. [`DeadManSwitch::forget_hold`] carries the argument for cancelling
    /// rather than firing, and for why an agent cannot reach this path.
    ///
    /// The tick afterwards is not decoration: it lets the outstanding timer's
    /// callback see `deadline() == None` on its next wakeup and drop itself,
    /// and it repaints nothing, so a disarmed indicator disappears at the
    /// host's ordinary frame cadence.
    ///
    /// **The confined app's keyboard state** is settled: every key whose
    /// press the router delivered *for the human* gets its release, through
    /// the same funnel an ordinary release takes. On Wayland this is the
    /// *only* notice the app will ever get — winit's `wl_keyboard.leave`
    /// handler emits `ModifiersChanged` and `Focused(false)` and no key
    /// events at all (verified in the pinned source) — so without it a key
    /// held across an alt-tab stays latched down in the app indefinitely. On
    /// X11 winit does emit synthetic releases, which [`admits_key_event`] now
    /// lets through; whichever arrives first takes the key out of the
    /// router's pairing table and the other finds nothing to release, so the
    /// two cannot double up.
    ///
    /// Keys an *agent* is holding are not touched, which is why the router
    /// method is [`input::InputRouter::release_physical_keys`] and not a blanket
    /// drain: both origins share a realm's seat state (`session::route_seat`),
    /// the host window's keyboard focus is not part of an agent's actuation
    /// path, and a release synthesised for an agent's key would reach the shim
    /// and the flight recorder tagged as the human's. That method's docs carry
    /// the full argument and the one bounded imprecision.
    ///
    /// **Keys only, and buttons deliberately not** — the asymmetry with
    /// [`input::InputRouter::bind_to`]'s drain, which pays both. This event is
    /// the host telling the core its window lost *keyboard* focus; winit
    /// reports pointer state separately (a `CursorLeft`, and on a real drag the
    /// host keeps sending motion), so synthesising a button release here would
    /// end a drag the human is still making. A realm switch is the other case:
    /// there the human's next release is addressed to a different realm
    /// whatever the host does, so both tables have to be paid.
    ///
    /// **Nor the gesture drain** ([`input::InputRouter::end_physical_gesture`],
    /// WS-E.4.2): a gesture is pointer-side, so it follows the buttons'
    /// exclusion and not the keys' inclusion. It costs nothing here for a
    /// second, sharper reason — smithay's winit backend surfaces no gesture
    /// events at all, so a nested session can never have one in flight.
    ///
    /// Ordering: the hold is forgotten *before* the drain, so a chord in
    /// progress is already cancelled when the releases go out. The chord's
    /// press was consumed by the gate and never delivered, so it is not in
    /// the router's table and the app is owed nothing for it.
    fn handle_focus(&mut self, focused: bool) {
        if focused {
            return;
        }
        debug!("host window lost keyboard focus; forgetting any dead-man hold in progress");
        self.deadman.borrow_mut().forget_hold();
        self.deadman_tick(TickSource::OffChain);

        // Disjoint field borrows, the same split `route_physical_inputs`
        // takes: the router hands back the deliveries while the sink reaches
        // the realm's shim session. The releases a focus change owes an app
        // must reach the app that was *told* about the presses, which is the
        // realm the human's input is bound to — the router names its own
        // debtor (`bound_realm`), and `session::deliver_physical` resolves the
        // same binding from the scene registry, so this site has no binding to
        // get wrong.
        let scenes = &self.view.scenes;
        let session::Runtime {
            router,
            realms,
            kernel,
            ..
        } = &mut self.runtime;
        let Some(bound) = router.bound_realm().cloned() else {
            return;
        };
        for delivery in router.release_physical_keys(&bound) {
            debug!("releasing a key held across focus loss so it cannot latch in the app");
            session::deliver_physical(realms, scenes, &mut kernel.recorder, delivery);
        }
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
    ///
    /// `source` says whether this tick is allowed to start a redraw chain:
    /// vsync pacing (P1.3.9, issue #117) stopped `schedule_next_frame` from
    /// self-sustaining once idle, so a hold armed while the window idles at
    /// 0 fps needs one kick to become visible — but only from *outside* the
    /// chain. See [`TickSource`], which carries the whole argument and the
    /// spin that omitting it caused.
    fn deadman_tick(&mut self, source: TickSource) {
        let handle = self.loop_handle.clone();
        deadman_tick(self, &handle, Instant::now(), source);
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
        //
        // `InChain`: this runs inside the host's own `RedrawRequested`
        // handling, and `schedule_next_frame` below already continues the
        // chain at `FRAME_BUDGET` while the hold animates. A second request
        // from here would be the un-budgeted spin — see [`TickSource`].
        if self.deadman.borrow().deadline().is_some() {
            self.deadman_tick(TickSource::InChain);
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

        // The hold indicator is sampled once per frame, from the same clock
        // reading nothing else in this frame contends for; both this and
        // the overlay check below feed the same instant so a hold that
        // completes mid-frame is judged consistently within it.
        let hold = self.deadman.borrow().hold_progress(Instant::now());
        // `None` for the notice surface: nested has no VT to escape from, so
        // it can never fail to (WS-E.3.5).
        let overlay_up = overlay_needs_the_window(
            hold,
            &self.view.consent,
            &self.view.lock,
            &self.view.blank,
            None,
        );

        // Zero-copy dmabuf presentation (P1.3.5, issue #117): **the bound
        // realm** has a retained GPU import and neither overlay needs the
        // window this frame, so the client's own texture goes straight to the
        // framebuffer via
        // [`present_human_visible`] — no CPU composite, no [`ImportMem`]
        // upload of any kind, and no core-side copy of a client pixel. This
        // is the runtime home of the zero-memcpy claim [`crate::dmabuf`]'s
        // [`crate::dmabuf::CopyMeter`] and its env-gated real-GPU test
        // (`VITRIN_GPU_TESTS=1`) pin end to end; reaching it here is what
        // makes that proof describe this backend's actual frame path rather
        // than only a test harness driving the importer directly.
        //
        // **It is still human-visible output, so it still carries the
        // trusted band** (issue #85). The band is inside
        // [`crate::dmabuf::human_visible_frame`]'s draw list rather than
        // applied by a caller, so this branch cannot present without it: as
        // first merged it did `bind → blit → submit` and skipped
        // [`super::human_visible_from_view`] entirely, which left every
        // dmabuf frame made *only* of pixels the confined client owns — free
        // to rasterize a counterfeit band with nothing genuine above it. The
        // band is a solid fill of the same [`TrustedIndicator`] colour the
        // CPU path paints, drawn last, so it costs one draw call and takes
        // nothing away from the zero-copy claim (which is about client-pixel
        // *copies*, not about the core never drawing).
        //
        // **Known MVP seam, not a silent bug**: the moment either overlay
        // needs the window, this falls through to the CPU path below, which
        // composes from [`Scene`] — and the dmabuf arm of `shim.rs`'s
        // `apply_buffer` deliberately never commits into `Scene` (module
        // docs: it stays renderer-free and GPU-content-free). So a frame
        // drawn while an overlay is up shows whatever `Scene` last held on
        // the CPU side — the deterministic background, or a stale pre-
        // dmabuf commit — not the live GPU pixels, for as long as the
        // overlay stays up. Reconciling that would mean a GPU-composited
        // overlay path or a readback-to-CPU bridge, either of which is the
        // rendering-pipeline overhaul this backend's module docs already
        // disclaim for what is a performance optimization, not a
        // correctness requirement (plan risk R3; shm stays the universal,
        // always-correct fallback, decision D3). Overlays are transient — a
        // petition gets decided, a hold releases or fires — so the window
        // self-heals back to live GPU content the instant `overlay_up` next
        // reads `false`.
        //
        // **Whose texture, decided by [`zero_copy_source`] and nowhere else**
        // (WS-E.1.3). Before that this branch asked only "is there a retained
        // import", of a slot every realm shared, so a hidden realm's dmabuf
        // commit became the picture in the human's window — the CPU path had
        // been keyed on the bound realm and this one had not.
        if zero_copy_source(&self.view.scenes, &self.view.dmabuf_content, overlay_up).is_some() {
            {
                // Scoped: `bind` holds `self.view.backend` mutably (a
                // disjoint field from `dmabuf_content`, `scenes` and
                // `indicator`) for the duration of the composite, and both
                // borrows must end here — `submit` and `schedule_next_frame`
                // right after need the whole `self.view`/`self` back.
                let indicator = self.view.indicator;
                // Copied out before the mutable borrow, same as the
                // indicator. The sprite rides in the draw list rather than
                // being applied by this caller, so the zero-copy path cannot
                // quietly present without it (`dmabuf::human_visible_frame`).
                let agent_cursor = self.view.agent_cursor;
                // The strip's texture, uploaded (or re-used) BEFORE `bind`
                // takes the backend: `renderer()` and `bind()` both want
                // `self.view.backend`, and the returned borrow is of
                // `status_texture` alone, so the two never overlap.
                // The session's geometry, read once for this frame and shared
                // by the strip's upload width and the draw list's placement
                // (issue #304) -- so the rows the strip is drawn into and the
                // rows the client is kept out of are one number.
                let geom = session::Presenter::view_geometry(&self.view);
                let status_width = geom.output().0;
                let status = upload_status_texture(
                    self.view.backend.renderer(),
                    &mut self.view.status,
                    &mut self.view.status_texture,
                    status_width,
                );
                let (renderer, mut framebuffer) = self.view.backend.bind()?;
                // Resolved a second time rather than held across `bind`: the
                // selection is one map lookup, and re-asking the same function
                // is what keeps the branch condition and the presented texture
                // from ever being two different decisions.
                let content =
                    zero_copy_source(&self.view.scenes, &self.view.dmabuf_content, overlay_up)
                        .expect("checked above");
                // The sync point is dropped here and used on bare metal:
                // the EGL swapchain synchronizes this frame, while a scanout
                // buffer needs the fence handed to `queue_buffer`.
                let _sync = present_human_visible(
                    renderer,
                    &mut framebuffer,
                    geom,
                    // The window surface, not the offscreen harness — see
                    // `WINDOW_TRANSFORM`, which the CPU blit below reads too.
                    WINDOW_TRANSFORM,
                    content,
                    indicator,
                    agent_cursor,
                    // No human cursor on this backend -- see `window_pixels`.
                    None,
                    status,
                )?;
            }
            self.view.backend.submit(None)?;
            trace!(?size, "dmabuf frame presented with zero core-side copies");
            self.schedule_next_frame(frame_start, hold)?;
            return Ok(session::Presentation::Completed);
        }

        // Re-upload when the window size, the scene content, or the consent
        // surface changed: the same shared composition both backends present
        // (P1.3.3), plus the prompt (P1.7.1), uploaded here as a full-window
        // texture. Keying on both generations is what makes a prompt appear
        // and disappear at the host's very next frame instead of whenever the
        // scene happens to change next. The hold bucket folds into the same
        // key — so the bar really animates instead of appearing whenever the
        // scene next happens to change (the trap
        // `the_texture_key_changes_on_every_visible_transition` was written
        // for).
        // The agent cursor folds into the same key, for the same reason the
        // hold bucket does: without it a sprite would move only when the
        // scene or the consent surface next happened to change, which for an
        // agent hovering over a static app is never.
        let agent_cursor = self.view.agent_cursor;
        // **The bound realm's** scene: this is the window, and the window
        // shows one realm (WS-E.1.3). A hidden realm's commit still marks the
        // frame dirty and still costs this backend a redraw — that is
        // decision 2's pacing bill, paid so the hidden realm's own capture
        // never goes stale — but it changes no pixel here, and the texture
        // key says so by keying on the bound scene's generation alone.
        let key = TextureKey::current(
            size,
            self.view.scenes.focused(),
            self.view.scenes.bound(),
            &self.view.consent,
            &self.view.lock,
            &self.view.blank,
            hold,
            agent_cursor,
            self.view.attention,
            &self.view.status,
        );
        // The scene's own pending damage (P1.3.9, issue #117), drained here
        // at most once per redraw whenever the scene changed — regardless of
        // which branch below ends up using it. `Scene::take_damage_view`'s
        // bookkeeping is one-shot per call and must never straddle two
        // redraws (see its docs), and this is the only site downstream of a
        // shim commit that ever calls it.
        let scene_dirty = self.view.texture.as_ref().map(|v| v.key.scene_generation)
            != Some(key.scene_generation);
        let scene_damage = if scene_dirty {
            self.view
                .scenes
                .take_bound_damage(session::Presenter::view_geometry(&self.view))
        } else {
            None
        };
        // Read before the mutable field borrows below, exactly as `indicator`
        // and `agent_cursor` are: `window_pixels` takes `&mut` to three of this
        // struct's fields, so the geometry cannot be asked for inside the call.
        let geom = session::Presenter::view_geometry(&self.view);
        if self.view.texture.as_ref().map(|v| &v.key) != Some(&key) {
            let pixels = window_pixels(
                self.view.scenes.bound(),
                &mut self.view.consent,
                &mut self.view.lock,
                &self.view.blank,
                &mut self.view.status,
                // No notice surface here either, and for the same reason:
                // only the backend that can switch a VT can fail to.
                None,
                hold,
                agent_cursor,
                // No human cursor here, ever: the host desktop is already
                // drawing the human's pointer over this window, so a sprite
                // would be a second cursor for one mouse. Only the bare-metal
                // backend passes `Some` (WS-E.3.2, issue #218).
                None,
                self.view.attention,
                geom,
            );
            let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();

            // Damage-limited upload: if the *only* thing that changed since
            // the last upload is the scene's own content, within a bounded
            // rectangle, and the existing texture is already the right size,
            // `update_memory` re-uploads just that rectangle instead of the
            // whole window — the shim already tracks and forwards real
            // damage (`shim/src/upstream.c`) and `Scene` already turned it
            // into view-space coordinates; this is the seam that turns it
            // into fewer bytes crossing the GPU bus. Any other cause — the
            // first frame, a resize, unbounded scene damage (a fresh/resized
            // surface, or a shim that named none), or a consent / hold /
            // agent-cursor transition riding along with this scene change —
            // takes the full [`ImportMem::import_memory`] path unchanged from
            // before this.
            //
            // **The agent-cursor term is load-bearing, not symmetry.** The
            // sprite is drawn wherever the agent is pointing, which in the
            // general case is outside the scene's damage rectangle — so a
            // re-upload bounded to that rectangle would leave the *previous*
            // sprite standing in the texture and put the new one nowhere: the
            // cursor would smear a trail across the window instead of moving.
            // The same failure the consent and hold terms exist to stop, one
            // overlay along.
            let bounded_scene_only_damage = match (&self.view.texture, scene_damage) {
                (Some(prev), Some(rect))
                    if prev.key.size == key.size
                        && prev.key.realm == key.realm
                        && prev.key.consent_generation == key.consent_generation
                        && prev.key.hold_bucket == key.hold_bucket
                        && prev.key.agent_cursor == key.agent_cursor =>
                {
                    Some(rect)
                }
                _ => None,
            };
            match bounded_scene_only_damage {
                Some(rect) if rect.width > 0 && rect.height > 0 => {
                    let region: Rectangle<i32, Buffer> =
                        Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into());
                    let prev = self.view.texture.as_ref().expect("checked above");
                    self.view
                        .backend
                        .renderer()
                        .update_memory(&prev.texture, &pixels, region)?;
                    self.view.texture.as_mut().expect("checked above").key = key.clone();
                    trace!(?rect, "damage-limited texture upload");
                }
                // Degenerate (all-zero) damage: the accumulated change
                // clipped away entirely outside the view (e.g. a center-
                // cropped margin), so nothing visible changed and no upload
                // — partial or full — is owed. The key still advances so the
                // next frame's comparison starts from the true current
                // state rather than re-deriving the same "nothing to do"
                // answer.
                Some(_) => {
                    self.view.texture.as_mut().expect("checked above").key = key.clone();
                }
                None => {
                    let texture = self.view.backend.renderer().import_memory(
                        &pixels,
                        Fourcc::Abgr8888,
                        buffer_size,
                        false,
                    )?;
                    self.view.texture = Some(SceneTexture { texture, key });
                }
            }
        }

        let full_window = Rectangle::from_size(size);
        {
            // Field-level borrows: `bind` holds `self.view.backend` mutably while
            // the view texture is read from `self.view.texture`.
            let view = self.view.texture.as_ref().expect("view composed above");
            let (renderer, mut framebuffer) = self.view.backend.bind()?;
            let mut frame = renderer.render(&mut framebuffer, size, WINDOW_TRANSFORM)?;
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
        // Full-window blit regardless of how much of the *upload* above was
        // damage-limited: the window texture itself is always whole, so the
        // GPU-side blit that presents it stays one full-window draw call —
        // only the CPU→GPU upload feeding that texture is bounded.
        self.view.backend.submit(None)?;
        trace!(?size, "frame submitted");
        self.schedule_next_frame(frame_start, hold)?;
        Ok(session::Presentation::Completed)
    }

    /// Chain the next redraw only while there is already-known work that
    /// needs one: an in-progress dead-man hold, whose indicator must keep
    /// animating every frame until it fires or is released (P1.3.9, issue
    /// #117). Otherwise the chain stops here — the loop idles at 0 fps
    /// instead of the unconditional self-sustaining redraw this backend used
    /// to run at all times, dirty or not. The next redraw then comes from an
    /// actual state change requesting one: a shim commit through
    /// [`session::Presenter::request_present`], a consent transition
    /// ([`NestedState::service_consent`]), physical input
    /// ([`NestedState::handle_input`]), or a resize — never from this method
    /// chaining itself.
    ///
    /// A hold's *first* frame is kicked separately, from
    /// [`NestedState::deadman_tick`] the moment the chord arms — this method
    /// only ever continues a chain already running, matching
    /// [`arm_deadman_timer`]'s "if none outstanding" discipline for the same
    /// reason: idempotent per hold, not per frame. **This is the only thing
    /// that paces an animating hold**, which is why the frame-cadence
    /// backstop inside [`NestedState::redraw`] must not also request one
    /// ([`TickSource::InChain`]).
    ///
    /// The decision itself is [`next_frame`], a pure function, so CI can pin
    /// the cadence without a host window; this method is its executor.
    fn schedule_next_frame(
        &mut self,
        frame_start: Instant,
        hold: Option<f64>,
    ) -> Result<(), Box<dyn Error>> {
        match next_frame(hold, frame_start.elapsed()) {
            NextFrame::Idle => {}
            NextFrame::Now => self.view.backend.window().request_redraw(),
            NextFrame::After(remaining) => {
                let timer = Timer::from_duration(remaining);
                self.loop_handle
                    .insert_source(timer, |_deadline, _, state| {
                        state.view.backend.window().request_redraw();
                        TimeoutAction::Drop
                    })
                    .map_err(|err| err.error)?;
            }
        }
        Ok(())
    }
}

/// What the redraw chain owes after one frame.
///
/// Split from [`NestedState::schedule_next_frame`] because presenting needs
/// a host window but the *cadence* is arithmetic, and an under- or
/// over-paced hold indicator is a safety-relevant defect either way: too
/// slow and the human cannot see how far the off-switch gesture has got, too
/// fast and the compositor burns a core on a window nothing is changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextFrame {
    /// Nothing is animating: the chain stops and the window idles at 0 fps
    /// until a real state change asks for a frame.
    Idle,
    /// The frame already cost at least [`FRAME_BUDGET`] — a real
    /// vsync-blocking swap did the pacing — so request the next immediately.
    Now,
    /// The frame completed early: defer the next request by this much. Hosts
    /// whose EGL swap returns immediately (Mesa software EGL under Xvfb,
    /// llvmpipe VMs, X servers without vblank) would otherwise spin the
    /// chain as fast as the loop turns.
    After(Duration),
}

/// The pacing decision: continue the chain only while a hold animates, and
/// never faster than [`FRAME_BUDGET`].
pub(crate) fn next_frame(hold: Option<f64>, elapsed: Duration) -> NextFrame {
    if hold.is_none() {
        return NextFrame::Idle;
    }
    match FRAME_BUDGET.checked_sub(elapsed) {
        Some(remaining) if !remaining.is_zero() => NextFrame::After(remaining),
        _ => NextFrame::Now,
    }
}

impl session::RuntimeHost for NestedState {
    type Hook = NestedHook;
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
        if session::service_consent_round(
            &mut grab,
            &mut self.runtime,
            &mut self.view.consent,
            now,
            // **Always reachable, on purpose** (D-030(4)). The gate this passes
            // is a bare-metal fact — "the seat took our devices away" — and
            // nested has no honest analogue. The nearest candidate is winit's
            // `Occluded`, which is not delivered on every platform, is not the
            // same fact (a minimized window is still answerable the moment the
            // human raises it), and would make a *host* compositor's bookkeeping
            // decide whether this core journals a prompt as shown. `limits.md`
            // already publishes that the nested lock covers a window rather than
            // a session; this is the same boundary, and it is scoped here rather
            // than half-implemented.
            session::PromptVisibility::Reachable,
        ) {
            self.runtime.dirty = true;
            self.view.backend.window().request_redraw();
        }
    }

    /// Drive the lock screen for this dispatch round (WS-E.2.2, issue #214).
    ///
    /// Same shape as [`Self::service_consent`] above and for the same borrow
    /// reason: the `Rc` is cloned first so the `RefMut` borrows nothing of
    /// `self`, leaving `&mut self.runtime` and `&mut self.view.lock` as two
    /// disjoint field borrows. A `true` return means visible output changed, so
    /// the frame is dirtied and a redraw is requested — the same path every
    /// other visible transition here takes.
    ///
    /// The nested backend is the only one that overrides the trait's no-op, and
    /// for a stronger reason than consent's: it is the only backend with a
    /// physical input device, and a lock a session cannot dismiss is a wedge.
    /// `main` refuses every `--lock-*` flag with `--headless` so that
    /// asymmetry is a startup error rather than a discovery.
    fn service_lock(&mut self, now: Instant) {
        let screen = Rc::clone(&self.lock);
        let mut screen = screen.borrow_mut();
        if session::service_lock_round(
            &mut screen,
            &mut self.view.lock,
            &mut self.runtime,
            &self.grab,
            self.deadman_chord,
            now,
        ) {
            self.runtime.dirty = true;
            self.view.backend.window().request_redraw();
        }
    }
}

impl session::Presenter for NestedView {
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
        let size = self.backend.window_size();
        (size.w.max(0) as u32, size.h.max(0) as u32)
    }

    fn status_height(&self) -> u32 {
        self.status.height()
    }

    /// The host window is the output here, so its size is already the new one
    /// by the time `Resized` is dispatched — [`Self::view_size`] reads it
    /// back from the backend. What this propagates is the scene registry's
    /// copy, which is what bumps every realm's `layout_generation`.
    fn set_view_size(&mut self, size: (u32, u32)) {
        self.scenes.set_view_size(size);
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
    fn view_rgba(&mut self, realm: &RealmId) -> Option<Vec<u8>> {
        // The window size is the size *every* realm's view composes at
        // (P1.3.3; decision 3 keeps one output size for all of them), the same
        // one `view_size` and `try_redraw` read. `capture_pixels` composes the
        // bare scene — never `window_pixels` — so the overlay and the hold
        // indicator cannot reach a capture.
        //
        // **This realm's scene, bound or not** (WS-E.1.3): a hidden realm's
        // `observe` grant serves that realm's own composition, and the bound
        // realm has no privileged path here — nested composes on the CPU for
        // both, which is why this backend needed no second implementation to
        // stop leaking siblings' pixels.
        capture_pixels(
            self.scenes.scene(realm)?,
            session::Presenter::view_geometry(self),
        )
    }

    fn request_present(&mut self) {
        self.backend.window().request_redraw();
    }

    /// Take the router's agent-owned position for the sprite this window
    /// draws (D-019), reporting whether the drawn result would differ.
    ///
    /// The comparison is on the **quantized hotspot**, not the raw `f64`:
    /// two positions inside the same view pixel draw byte-identical sprites,
    /// so reporting a change for them would request a host frame that
    /// composited nothing new — the anti-amplification posture the whole
    /// dirty/`request_present` split exists for. It is the same quantization
    /// [`TextureKey::agent_cursor`] keys on, so this method and the texture
    /// cache cannot disagree about what counts as a move.
    ///
    /// This backend never opts out of the sprite: it is the mode a human is
    /// watching, and the reason D-019 exists at all — an operator was told to
    /// "watch the cursor move" at a window that drew none. That is a statement
    /// about *this method*, not about every frame: since WS-E.1.3
    /// [`session::post_dispatch`] offers `None` unless the router's realm is
    /// the realm the output is bound to, so an agent acting in a hidden realm
    /// reaches here with nothing to draw. See
    /// [`session::Presenter::set_agent_cursor`] for that gate and its cost.
    fn set_agent_cursor(&mut self, pos: Option<(f64, f64)>) -> bool {
        let quantize =
            |pos: Option<(f64, f64)>| pos.and_then(|(x, y)| crate::cursor::hotspot(x, y));
        if quantize(self.agent_cursor) == quantize(pos) {
            return false;
        }
        self.agent_cursor = pos;
        true
    }

    /// The attention marker (WS-E.1.7): a plain `bool`, because unlike the
    /// cursor there is no position to quantise -- the window is open or it is
    /// not.
    fn set_attention(&mut self, open: bool) -> bool {
        if self.attention == open {
            return false;
        }
        self.attention = open;
        true
    }

    /// The status strip (WS-E.2.3). Sampled against **the realm the output is
    /// bound to**, not the router's: the strip names the realm in the picture
    /// the human is looking at, and naming a hidden one would be a caption for
    /// a different image. Answers `false` and touches nothing when `--status`
    /// is off.
    fn refresh_status(&mut self, now: std::time::SystemTime, mono: Instant) -> bool {
        self.status.refresh(now, mono, self.scenes.focused())
    }

    /// The pointer-constraint gates (WS-E.4.2, issue #222).
    ///
    /// Overridden here even though **this backend has no sprite to hide** —
    /// [`window_pixels`] is handed `None` for `human_cursor` in nested mode, so
    /// the safety property is vacuous. What is not vacuous is the app-facing
    /// half: an app locked in a nested realm must still be told `inactive` when
    /// a consent card goes up over it, and the freeze in
    /// `crate::input::route_into` must still lift. Answering the default here
    /// would have left a nested lock in force behind a consent prompt.
    ///
    /// `active` is unconditionally `true`: there is no seat to pause, so this
    /// backend's output never becomes unpresentable in the sense the bare-metal
    /// one's does.
    fn output_gates(&self) -> crate::input::OutputGates {
        let hold = self.deadman.borrow().hold_progress(Instant::now());
        crate::input::OutputGates {
            // `None` for the notice: nested has no VT to escape from, which is
            // the same argument `try_redraw` already makes at its own call.
            overlay_up: overlay_needs_the_window(
                hold,
                &self.consent,
                &self.lock,
                &self.blank,
                None,
            ),
            active: true,
            // `false` unconditionally: `--blank-idle` is refused on this
            // backend, so `self.blank` is never raised and there is no panel
            // here to power down in the first place -- the host compositor owns
            // the display this window sits on.
            dark: false,
        }
    }

    /// The scene, `None` for the retained half, and the dmabuf importer
    /// bound to this backend's live `GlesRenderer` (P1.3.5, issue #117).
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
    ///
    /// The importer, unlike the retained half, is very much needed here: a
    /// GPU renderer exists ([`Self::backend`]'s `GlesRenderer`), and the
    /// death funnel must drop the dying realm's retained GPU content through
    /// it ([`DmabufImporter::clear`]) exactly as a live dispatch's replacing
    /// commit would — the same GPU-done sync, the same single disposal
    /// path, never a second one for teardown alone.
    ///
    /// **It is lent `realm`'s own slot** (WS-E.1.3): the importer is built
    /// over [`RealmGpuContent::slot_mut`], so a death drops exactly the dying
    /// realm's texture. Before that there was one slot for the session, so any
    /// realm's death cleared whichever realm's content happened to be resident
    /// — the same session-wide reach the scene half had, and it outlived the
    /// scene's fix because the two are different fields.
    ///
    /// The concrete [`GlesDmabufImporter`] lives as this call's own local —
    /// never boxed and handed back by value (see
    /// [`session::Presenter::teardown_view`]'s docs for why) — and `f` is
    /// where the whole teardown funnel actually runs, so the local's borrow
    /// of `self.backend`/`self.dmabuf_content` need only last for that one
    /// call.
    fn teardown_view<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(
            &'v mut Scene,
            Option<&'v mut dyn crate::lifecycle::RetainedOutput>,
            Option<&'v mut dyn DmabufImporter>,
        ) -> R,
    ) -> R {
        let mut importer = GlesDmabufImporter {
            renderer: self.backend.renderer(),
            content: self.dmabuf_content.slot_mut(realm),
        };
        f(self.scenes.scene_mut(realm), None, Some(&mut importer))
    }

    /// The scene and a [`GlesDmabufImporter`] wrapping this backend's live
    /// `GlesRenderer` and its retained content slot (P1.3.5, issue #117):
    /// the seam that lets a `kind=dmabuf` shim commit resolve as a real
    /// zero-copy import instead of the designed headless fallback.
    ///
    /// Constructed fresh per dispatch, as the trait's docs require: the
    /// importer borrows `self.backend`'s renderer and **this realm's**
    /// [`RealmGpuContent`] slot for exactly this call and is handed to `f` as
    /// a bare trait reference, never boxed (see
    /// [`session::Presenter::scene_and_importer`]'s docs for why that
    /// distinction is load-bearing, not stylistic).
    ///
    /// **Both halves name the same realm** (WS-E.1.3) — the one whose shim
    /// connection is being dispatched. A `kind=dmabuf` commit therefore lands
    /// in that realm's own slot, and [`zero_copy_source`] decides separately
    /// whether the output is showing that realm; while this was one slot for
    /// the session, importing *was* presenting.
    fn scene_and_importer<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(&'v mut Scene, Option<&'v mut dyn DmabufImporter>) -> R,
    ) -> R {
        let mut importer = GlesDmabufImporter {
            renderer: self.backend.renderer(),
            content: self.dmabuf_content.slot_mut(realm),
        };
        f(self.scenes.scene_mut(realm), Some(&mut importer))
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

    /// **`LockGate` is the outermost hook, and this is a compile-time
    /// assertion of it** (WS-E.2.2, issue #214).
    ///
    /// Not decoration. [`crate::lock::gate`] tracks its chord's modifier state
    /// inside `judge` rather than in an `observe` it deliberately does not
    /// have, and that is sound *only* while nothing above it can consume an
    /// event — at any inner position a consumed Shift release would leave the
    /// matcher believing Shift is held, and the lock chord would start firing
    /// on gestures the human did not make (or stop firing on ones they did).
    ///
    /// A type-level check rather than a behavioural one because the property is
    /// about the *stack's shape*: `assert_lock_outermost` accepts only a
    /// `PhantomData<LockGate<_>>`, so this stops compiling the moment
    /// [`NestedHook`] is reordered. A behavioural test would go red later, in
    /// some unrelated chord case, with a much worse error message.
    #[test]
    fn the_lock_gate_is_the_outermost_hook() {
        use std::marker::PhantomData;
        fn assert_lock_outermost<H: input::PreemptionHook>(
            _: PhantomData<crate::lock::LockGate<H>>,
        ) {
        }
        assert_lock_outermost(PhantomData::<NestedHook>);
    }

    /// **The nested backend never hands `window_pixels` a human cursor**
    /// (WS-E.4.2, issue #222) — so the pointer-constraint change's safety
    /// property is *structurally* vacuous here, rather than incidentally so.
    ///
    /// This matters more than its size suggests. The core hides its own cursor
    /// sprite while a pointer constraint is in force, and a missed un-hide on
    /// **bare metal** is a human with no visible pointer on a display server
    /// they cannot exit. On this backend the host desktop draws the human's
    /// pointer over the window, so no sprite is composited at all and nothing
    /// can be hidden — which is why every mode CI can run is immune to the
    /// worst failure this change could cause, and why that immunity has to be
    /// **pinned**: it is exactly the asymmetry that lets a defect ship green,
    /// and a nested backend that started drawing a sprite would silently
    /// acquire the hazard with no test noticing.
    ///
    /// Held as a shape, because presenting needs an EGL context and a host
    /// window that CI does not have — the same instrument `window_pixels` and
    /// `zero_copy_source` were split out for.
    #[test]
    fn the_nested_backend_never_hands_window_pixels_a_human_cursor() {
        let source = include_str!("winit.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("this file ends with its own test module")
            .0;
        // The CALL, not the definition: `let pixels = window_pixels(` is the
        // one upload site, inside `try_redraw`.
        let after = source
            .split("let pixels = window_pixels(")
            .nth(1)
            .expect("this backend uploads through window_pixels");
        let args = after
            .split_once(");")
            .expect("the call's arguments end at its closing paren")
            .0;
        assert!(
            args.contains("None"),
            "the nested upload must pass `None` for the human cursor: the host desktop draws \
             it, so a sprite here would be a second cursor for one mouse -- and it is what \
             makes this backend structurally unable to strand a human's pointer under a \
             pointer constraint. Args were: {args}"
        );
        // ...and the zero-copy branch, which draws through a different
        // function and so could regress independently.
        let after = source
            .split("let _sync = present_human_visible(")
            .nth(1)
            .expect("the zero-copy branch presents through present_human_visible");
        let args = after
            .split_once(")?")
            .expect("the call's arguments end at its closing paren")
            .0;
        assert!(
            args.contains("None"),
            "the zero-copy branch must pass `None` for the human cursor too: a sprite drawn on \
             one path and not the other would appear and disappear as the branch engaged. Args \
             were: {args}"
        );
    }

    /// **Every overlay forces the CPU path**, the lock included (WS-E.2.2).
    ///
    /// The lock's row is the one that matters: without it a locked session
    /// takes the zero-copy branch and presents the confined app's own texture
    /// full-window, with the lock screen nowhere on it, while the gate is
    /// consuming every key. That is not a cosmetic regression — it is the app
    /// on display behind a lock — and before this function existed the
    /// condition lived inline in `try_redraw`, which no test on a display-free
    /// runner can reach.
    #[test]
    fn every_overlay_forces_the_cpu_path() {
        let indicator = TrustedIndicator::for_test();
        let idle_consent = ConsentSurface::new(indicator);
        let idle_lock = no_lock();

        // The control FIRST: with nothing up, the zero-copy branch is allowed.
        // Without this the assertions below would pass against a function
        // that always answered `true`.
        let unblanked = crate::backend::blank::BlankSurface::for_test();
        assert!(!overlay_needs_the_window(
            None,
            &idle_consent,
            &idle_lock,
            &unblanked,
            None
        ));

        // A hold.
        assert!(overlay_needs_the_window(
            Some(0.5),
            &idle_consent,
            &idle_lock,
            &unblanked,
            None
        ));
        // A consent card.
        let mut prompt = ConsentSurface::new(indicator);
        prompt.show_for_test(prompt_fixture());
        assert!(overlay_needs_the_window(
            None, &prompt, &idle_lock, &unblanked, None
        ));
        // A lock screen.
        let mut locked = no_lock();
        locked.raise(crate::lock::tests::lock_fixture());
        assert!(
            overlay_needs_the_window(None, &idle_consent, &locked, &unblanked, None),
            "a raised lock MUST force the CPU path: the zero-copy branch presents the \
             client's own texture, so a locked session that took it would show the app \
             full-window with the lock nowhere on it"
        );
        // ...and lowering it gives the branch back, so the test is measuring
        // the lock's state and not the surface's existence.
        locked.lower();
        assert!(!overlay_needs_the_window(
            None,
            &idle_consent,
            &locked,
            &unblanked,
            None
        ));

        // **A raised core notice**, the fourth cause and the bare-metal-only
        // one (WS-E.3.5). It is CPU-composited, so a session that took the
        // zero-copy branch while one was up would present the client's texture
        // with the one message a trapped human needs nowhere on it -- the
        // original defect with an extra step. `None` first, so this measures
        // the notice's state rather than the parameter's presence.
        let mut notice = crate::notice::CoreNotice::new();
        assert!(!overlay_needs_the_window(
            None,
            &idle_consent,
            &locked,
            &unblanked,
            Some(&notice)
        ));
        notice.raise(
            crate::notice::NoticeContent::VtSwitchFailed {
                target: 2,
                own: Some(3),
            },
            std::time::Instant::now(),
        );
        assert!(
            overlay_needs_the_window(None, &idle_consent, &locked, &unblanked, Some(&notice)),
            "a raised notice MUST force the CPU path, or the message telling the human their \
             session cannot leave this terminal is composited into a frame that is never drawn"
        );

        // **A raised idle cover**, the fifth cause and the second *security*
        // one (WS-E.4.3, issue #223). The zero-copy branch presents the
        // confined client's own texture with no core composite at all, so a
        // session that raised the cover and then took it would put the app on
        // screen full-window on the very frame it is about to power the panel
        // down on -- and leave that frame in the scanout buffer for the next
        // unblank to reveal. The control is `unblanked` above, used by every
        // assertion in this test, so this measures the cover's state.
        let mut covering = crate::backend::blank::BlankSurface::for_test();
        covering.set_covering(true);
        let idle_notice = crate::notice::CoreNotice::new();
        assert!(
            overlay_needs_the_window(None, &idle_consent, &locked, &covering, Some(&idle_notice)),
            "a raised blank cover MUST force the CPU path, or the frame the panel goes dark on \
             is made entirely of pixels the confined client owns"
        );
    }

    /// A lock surface with nothing raised, for the many composite tests that
    /// are about the *other* overlays.
    ///
    /// A fresh one per call rather than a shared fixture: [`LockSurface`]
    /// carries a generation counter and a raster cache, and a shared instance
    /// would let one test's raise change what the next test measures.
    /// A status strip that is off: `--status` is opt-in, so this is what every
    /// composite in this suite runs with unless it is testing the strip.
    fn no_status() -> crate::status::StatusStrip {
        crate::status::StatusStrip::new(crate::status::StatusConfig::off())
    }

    fn no_lock() -> crate::lock::LockSurface {
        crate::lock::LockSurface::new(crate::consent::TrustedIndicator::for_test())
    }

    /// A blank cover with nothing raised -- what every composite in this suite
    /// runs with, since `--blank-idle` is refused on this backend outright.
    fn no_blank() -> crate::backend::blank::BlankSurface {
        crate::backend::blank::BlankSurface::for_test()
    }

    /// **A raised notice really reaches the human-visible upload, and it is
    /// drawn OVER a raised lock cover** (WS-E.3.5, D-031(3)).
    ///
    /// Both halves are load-bearing and neither is worth much alone. Deleting
    /// the composite call from [`window_pixels`] left every other test in this
    /// workspace green while the one surface a trapped human can read stopped
    /// being drawn — so the positive is pinned. And the VT escape works
    /// **while locked** by decision, so the state a human is most likely to be
    /// trapped in is precisely the one where an opaque cover is on top of
    /// everything composed before it; a notice under that cover would be a
    /// message nobody can read, which is the defect it exists to close.
    #[test]
    fn a_raised_notice_reaches_the_upload_and_sits_over_the_lock_cover() {
        const W: i32 = 800;
        const H: i32 = 600;
        let size = size_of(W, H);
        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(400, 300), 400, 300).expect("content"));
        let last_row = |px: &[u8]| {
            let off = (H as usize - 1) * W as usize * crate::scene::BYTES_PER_PIXEL;
            px[off..off + crate::scene::BYTES_PER_PIXEL].to_vec()
        };

        let mut notice = crate::notice::CoreNotice::new();
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        let without = window_pixels(
            &scene,
            &mut consent,
            &mut raised_lock(),
            &no_blank(),
            &mut no_status(),
            Some(&mut notice),
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );

        notice.raise(
            crate::notice::NoticeContent::VtSwitchFailed {
                target: 2,
                own: Some(3),
            },
            Instant::now(),
        );
        let with = window_pixels(
            &scene,
            &mut consent,
            &mut raised_lock(),
            &no_blank(),
            &mut no_status(),
            Some(&mut notice),
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(
            with, without,
            "a raised notice must change what is uploaded, or the human is told nothing"
        );
        assert_ne!(
            last_row(&with),
            last_row(&without),
            "the notice is anchored to the bottom edge and must be ON the frame, not merely \
             rasterized somewhere"
        );
        // ...and the trusted band is still exactly the session colour, so
        // nothing the notice draws reached rows [0, TRUST_BAND_HEIGHT).
        assert_eq!(
            with[..crate::scene::BYTES_PER_PIXEL],
            TrustedIndicator::for_test().color(),
            "the notice must not touch the band"
        );
    }

    /// A lock surface with the cover **up**, by the chord, passphrase-backed.
    ///
    /// Exists because until WS-E.2.2's fix there was no way to write one: the
    /// texture key took no lock, so "the screen is locked" was a state this
    /// suite's key assertions could not express, and the omission read as
    /// coverage. Any new input to [`super::compose_human_visible`] wants the
    /// matching pair — an `off` fixture and an `on` one — for the same reason.
    fn raised_lock() -> crate::lock::LockSurface {
        let mut lock = no_lock();
        lock.raise(crate::lock::LockContent {
            cause: crate::lock::LockCause::Chord,
            realms: vec![RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)],
            unlock: crate::lock::UnlockMethod::Passphrase,
            deadman_chord: "esc",
        });
        lock
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
    /// **The nested hook stack is `ConsentGate` outside `DeadManHook` outside
    /// `ClipboardHook` outside `ScreenshotHook` outside `AttentionHook`**
    /// (WS-E.1.7 decision 11, extended by WS-E.2.1 and WS-E.2.4), pinned as a
    /// fact about the type rather than as a comment beside the constructor.
    ///
    /// The order is the decision: a consent prompt and a dead-man chord press
    /// must each short-circuit *before* the attention key, whose worst failure
    /// is a focus change that does not happen and which must never be able to
    /// delay the human's off-switch or open a window while the human is
    /// answering a security question. Reordering the three is a one-line edit
    /// that changes no behaviour any other test in this crate observes — the
    /// two chord vocabularies are disjoint, so neither hook sees the other's
    /// key — which is exactly why the order needs a tripwire of its own.
    ///
    /// Read off `type_name`, because nested generic arguments appear in
    /// left-to-right stacking order there and nowhere else this cheaply.
    #[test]
    fn the_nested_stack_is_consent_then_dead_man_then_clipboard_then_screenshot_then_attention() {
        let name = std::any::type_name::<<NestedState as session::RuntimeHost>::Hook>();
        let at = |needle: &str| {
            name.find(needle)
                .unwrap_or_else(|| panic!("the nested stack must carry {needle}: {name}"))
        };
        let (consent, dead, clipboard, screenshot, attention) = (
            at("ConsentGate"),
            at("DeadManHook"),
            at("ClipboardHook"),
            at("ScreenshotHook"),
            at("AttentionHook"),
        );
        assert!(
            consent < dead && dead < clipboard && clipboard < screenshot && screenshot < attention,
            "the nested hook stack must be \
             ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<AttentionHook<..>>>>>: a \
             prompt and the human's off-switch each short-circuit before the clipboard \
             chords, the screenshot key and the attention key, never after; both chord \
             matchers must sit ABOVE the attention hook, which eats both Supers and would \
             otherwise leave their `super` modifier bit permanently wrong; and the \
             screenshot key must sit BELOW the lock, so a locked session cannot be \
             photographed by whoever is standing at it. Got {name}"
        );
    }

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
        let plain = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        let composed = scene.compose((W as u32, H as u32).into());
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
        //
        // With no hold and no agent cursor, both of which are nested-side
        // overlays this backend applies *after* the shared step. The equality
        // below is exactly that case and no wider: a hold in progress, or an
        // agent pointing at the window, makes nested's output differ from what
        // headless retains by design (the hold indicator has no meaning on a
        // backend with no physical input device; the agent cursor is opt-in
        // there — see `window_pixels` and D-019). What must never drift is the
        // shared composition underneath, which is what this pins.
        consent.show_for_test(prompt_fixture());
        let with_prompt = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(
            with_prompt, plain,
            "the prompt must change what is uploaded"
        );
        let mut expected = ConsentSurface::new(TrustedIndicator::for_test());
        expected.show_for_test(prompt_fixture());
        assert_eq!(
            with_prompt,
            super::super::compose_human_visible(
                &scene,
                &mut expected,
                &mut no_lock(),
                &no_blank(),
                &mut no_status(),
                (W as u32, H as u32).into(),
                false
            ),
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

    /// **No presentation path on this backend emits a frame without the
    /// trusted band** (issue #85).
    ///
    /// The nested backend has two, and only one of them went through the
    /// output stage. P1.3.5's zero-copy branch did `bind → render → submit`
    /// and never reached [`super::human_visible_from_view`] — the only
    /// non-test caller of `ConsentSurface::composite_trust_band` — so every
    /// dmabuf-presented frame consisted entirely of pixels the confined
    /// client owns. A client that maximized its surface could then rasterize
    /// a counterfeit band into the top of its own buffer with nothing genuine
    /// above it, and the human's one unforgeable reference for judging a
    /// consent prompt would be the app's own drawing. The whole suite passed.
    ///
    /// The sibling of [`the_nested_window_uploads_the_consent_overlay`]:
    /// that one holds the CPU path's half, this one holds both halves
    /// together and — crucially — pins that they paint the *same* colour, so
    /// a GPU band derived from anywhere but this session's indicator (a
    /// second forgery surface, not a rounding nit) fails here.
    #[test]
    fn no_presentation_path_can_drop_the_trusted_band() {
        const W: i32 = 800;
        const H: i32 = 600;
        let size = size_of(W, H);
        let indicator = TrustedIndicator::for_test();

        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(300, 200), 300, 200).expect("content"));

        // Path 1, the CPU texture upload. The band's *bottom* row is read
        // rather than its top, because the dead-man hold indicator is
        // deliberately composited above everything (`composite_hold_indicator`
        // — a human mid-gesture on the off-switch must see that, whatever
        // else is on screen) and its bar is shorter than the band, so the
        // band survives underneath it. Row 0 would only test the no-hold
        // case.
        let band_row = crate::consent::TRUST_BAND_HEIGHT - 1;
        let band_px = |buf: &[u8]| {
            let off = band_row as usize * W as usize * crate::scene::BYTES_PER_PIXEL;
            buf[off..off + crate::scene::BYTES_PER_PIXEL].to_vec()
        };
        let mut consent = ConsentSurface::new(indicator);
        for hold in [None, Some(0.0), Some(0.5), Some(1.0)] {
            assert_eq!(
                band_px(&window_pixels(
                    &scene,
                    &mut consent,
                    &mut no_lock(),
                    &no_blank(),
                    &mut no_status(),
                    None,
                    hold,
                    None,
                    None,
                    false,
                    (size.w.max(0) as u32, size.h.max(0) as u32).into()
                )),
                indicator.color(),
                "the CPU path dropped the trusted band (hold={hold:?})"
            );
        }
        consent.show_for_test(prompt_fixture());
        assert_eq!(
            band_px(&window_pixels(
                &scene,
                &mut consent,
                &mut no_lock(),
                &no_blank(),
                &mut no_status(),
                None,
                None,
                None,
                None,
                false,
                (size.w.max(0) as u32, size.h.max(0) as u32).into()
            )),
            indicator.color(),
            "a raised prompt must not cover the band it is checked against"
        );
        // An agent's cursor aimed straight at the band cannot cover it
        // either, and that one is the sharper case: the position is the
        // agent's own choice, so an unclipped sprite at row 0 would be a
        // forgery surface rather than an overlap (`crate::cursor`).
        let mut idle = ConsentSurface::new(indicator);
        for aim in [(0.0, 0.0), (W as f64 / 2.0, 0.0), (W as f64 / 2.0, 3.0)] {
            assert_eq!(
                band_px(&window_pixels(
                    &scene,
                    &mut idle,
                    &mut no_lock(),
                    &no_blank(),
                    &mut no_status(),
                    None,
                    None,
                    Some(aim),
                    None,
                    false,
                    (size.w.max(0) as u32, size.h.max(0) as u32).into()
                )),
                indicator.color(),
                "an agent's cursor at {aim:?} painted into the trusted band"
            );
        }

        // Path 2, the zero-copy dmabuf present: the band is the last thing
        // the frame draws, at the same rectangle, in the same colour. The
        // rectangle is checked against the view's own numbers rather than
        // against `trust_band_rect(size)` — asserting a function's output
        // equals that same function's output is vacuous, and this test used
        // to pass with the band collapsed to zero size.
        let draws = crate::dmabuf::human_visible_frame(
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
            (300, 200),
            indicator,
            None,
            None,
        );
        let last = crate::dmabuf::HUMAN_VISIBLE_DRAWS - 1;
        let crate::dmabuf::Draw::TrustBand(band, band_rgba) = draws[last] else {
            panic!(
                "the zero-copy path presented a frame made only of client pixels: {:?}",
                draws[last]
            )
        };
        assert_eq!(
            (band.loc.x, band.loc.y, band.size.w, band.size.h),
            (0, 0, W, crate::consent::TRUST_BAND_HEIGHT as i32),
            "the zero-copy band must cover the full width of the view's top strip, at the \
             CPU path's height -- a narrower or shorter one leaves client-owned pixels \
             where the human reads the session colour"
        );

        // The two paths' colour is one secret, not two. `NestedView` holds
        // the indicator beside the consent surface it also seeded, so this
        // re-derives what the consent surface actually paints rather than
        // trusting that construction site: a band and a frame in different
        // colours would teach the human to distrust genuine prompts.
        let mut probe = vec![0u8; W as usize * crate::consent::TRUST_BAND_HEIGHT as usize * 4];
        ConsentSurface::new(indicator).composite_trust_band(
            &mut probe,
            W as u32,
            crate::consent::TRUST_BAND_HEIGHT,
        );
        assert_eq!(
            probe[..crate::scene::BYTES_PER_PIXEL],
            band_rgba,
            "the GPU band's colour must be the very colour the consent surface paints"
        );
    }

    /// **Neither presentation path on this backend drops the agent cursor,
    /// and they draw the same one** (D-019).
    ///
    /// Issue #85's bug, one property milder: the nested backend has two
    /// human-visible paths, the zero-copy dmabuf branch bypasses the CPU
    /// output stage entirely, and the first cut of the trusted band was
    /// painted on only one of them while the whole suite stayed green. A
    /// cursor that existed on the CPU path alone would vanish the moment a
    /// client committed a dmabuf — the operator would be told again to watch a
    /// cursor that is not there, for a reason no test named.
    ///
    /// GL presentation needs a display, so what is pinned is the decision each
    /// path makes: the pixels [`window_pixels`] composes, and the draw list
    /// [`crate::dmabuf::present_human_visible`] executes. The two are held
    /// against each other rather than each against its own constants — a
    /// crosshair of a different size or colour on the GPU path would fail
    /// here.
    #[test]
    fn no_presentation_path_can_drop_the_agent_cursor() {
        use crate::cursor::{AGENT_CURSOR_CORE, AGENT_CURSOR_HALO};

        const W: i32 = 400;
        const H: i32 = 300;
        let size = size_of(W, H);
        let indicator = TrustedIndicator::for_test();
        let at = (200.0_f64, 150.0_f64);

        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(400, 300), 400, 300).expect("content"));

        // Path 1, the CPU texture upload: the sprite's colours really appear,
        // and only when a cursor is shown.
        let mut consent = ConsentSurface::new(indicator);
        let without = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        let with = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            Some(at),
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(with, without, "the CPU path dropped the agent cursor");
        let count = |px: &[u8], rgba: [u8; 4]| {
            px.chunks_exact(crate::scene::BYTES_PER_PIXEL)
                .filter(|pixel| *pixel == rgba)
                .count()
        };
        assert_eq!(count(&without, AGENT_CURSOR_CORE), 0);
        assert!(count(&with, AGENT_CURSOR_CORE) > 0);
        assert!(count(&with, AGENT_CURSOR_HALO) > 0);

        // Path 2, the zero-copy dmabuf present: the same rectangles, in the
        // same order, in the same colours — derived from one geometry function
        // (`crate::cursor::agent_cursor_rects`), which is why this can be an
        // equality rather than a family of hand-written expectations.
        let draws = crate::dmabuf::human_visible_frame(
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
            (400, 300),
            indicator,
            Some(at),
            None,
        );
        let gpu: Vec<_> = draws
            .iter()
            .filter_map(|draw| match draw {
                crate::dmabuf::Draw::AgentCursor(rect, rgba) => {
                    Some((rect.loc.x, rect.loc.y, rect.size.w, rect.size.h, *rgba))
                }
                _ => None,
            })
            .collect();
        let expected: Vec<_> = crate::cursor::agent_cursor_rects(W as u32, H as u32, at.0, at.1)
            .expect("a 400x300 view has room for a sprite")
            .iter()
            .map(|r| (r.x, r.y, r.w as i32, r.h as i32, r.rgba))
            .collect();
        assert_eq!(
            gpu, expected,
            "the zero-copy path draws a different agent cursor than the CPU path"
        );
        // ...and a frame with no cursor really has none, so the equality above
        // is not passing on an unconditional draw.
        assert!(
            crate::dmabuf::human_visible_frame(
                (size.w.max(0) as u32, size.h.max(0) as u32).into(),
                (400, 300),
                indicator,
                None,
                None
            )
            .iter()
            .all(|draw| !matches!(draw, crate::dmabuf::Draw::AgentCursor(..))),
            "the zero-copy path drew a cursor for a frame that has none"
        );
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
        let capture = capture_pixels(&scene, (size.w.max(0) as u32, size.h.max(0) as u32).into())
            .expect("a nonzero view has pixels");
        assert_eq!(
            capture,
            scene.compose((W as u32, H as u32).into()),
            "the nested capture must be the bare Scene::compose realm view"
        );

        // A prompt up changes the human-visible window but not the capture: the
        // overlay is excluded by construction (it joins only downstream of
        // `Scene::compose`, in `human_visible_from_view`).
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        consent.show_for_test(prompt_fixture());
        let human_visible = super::super::compose_human_visible(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            (W as u32, H as u32).into(),
            false,
        );
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
        let with_hold = window_pixels(
            &scene,
            &mut idle,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            Some(0.5),
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(
            capture, with_hold,
            "the dead-man hold indicator must never reach the capture"
        );

        // And so is the agent cursor (D-019, IDL ordering invariant 4): the
        // window carries it, the capture does not, and not one pixel of either
        // sprite colour appears in the composed realm view.
        let with_cursor = window_pixels(
            &scene,
            &mut idle,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            Some((400.0, 300.0)),
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(
            capture, with_cursor,
            "the agent cursor must never reach the capture"
        );
        for colour in [
            crate::cursor::AGENT_CURSOR_CORE,
            crate::cursor::AGENT_CURSOR_HALO,
        ] {
            assert!(
                !capture
                    .chunks_exact(crate::scene::BYTES_PER_PIXEL)
                    .any(|px| px == colour),
                "an agent-cursor pixel ({colour:?}) reached a nested capture"
            );
            assert!(
                with_cursor
                    .chunks_exact(crate::scene::BYTES_PER_PIXEL)
                    .any(|px| px == colour),
                "...and the sprite really was drawn on the human-visible side"
            );
        }

        // A degenerate (minimized) window has no realm view to serve, so the
        // capture meets the chokepoint's `no_surface` refusal.
        assert!(capture_pixels(
            &scene,
            (size_of(0, 0).w.max(0) as u32, size_of(0, 0).h.max(0) as u32).into()
        )
        .is_none());
        assert!(capture_pixels(
            &scene,
            (size_of(W, 0).w.max(0) as u32, size_of(W, 0).h.max(0) as u32).into()
        )
        .is_none());
    }

    /// **The nested window shows the bound realm, and the texture key knows
    /// when that changes** (WS-E.1.3, issue #209) — the whole of what CI can
    /// check about this backend's realm keying.
    ///
    /// **CI cannot exercise the nested backend at all**: GitHub runners have
    /// no display, and D-019(4) records headless as the only backend CI can
    /// run. So this pins the two decisions `try_redraw` makes — *which pixels
    /// to upload* ([`window_pixels`] over [`RealmScenes::bound`]) and *when to
    /// re-upload them* ([`TextureKey`]) — and nothing about the GL submit.
    /// "A human sees the right realm in the host window" is a **manual
    /// runbook step** (`shim/docs/nested-multi-realm.md`), written as one, and
    /// is not a CI criterion.
    ///
    /// The realm in the key is what this is really about. Two realms' scenes
    /// have independent `generation` counters, so a bind between two realms
    /// whose counters happen to match would compare equal on every other field
    /// — leaving the *previous* realm's texture on the human's screen until
    /// something unrelated changed. The fixture below builds exactly that
    /// coincidence rather than hoping for it.
    #[test]
    fn the_window_shows_the_bound_realm_and_a_bind_re_uploads() {
        const W: i32 = 320;
        const H: i32 = 200;
        let size = size_of(W, H);
        let (a, b) = (RealmId::new("realm-0"), RealmId::new("realm-b"));
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());
        let mut scenes = RealmScenes::new((W as u32, H as u32));

        // Two realms, different content, and — deliberately — the **same**
        // scene generation: one commit each.
        scenes
            .scene_mut(&a)
            .commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        scenes
            .scene_mut(&b)
            .commit(SurfaceContent::from_rgba(client_pixels(80, 60), 80, 60).expect("content"));
        assert_eq!(
            scenes.scene(&a).unwrap().generation(),
            scenes.scene(&b).unwrap().generation(),
            "the fixture must produce the counter coincidence this test is about"
        );

        // Bound to A: the window is A's composition, byte for byte.
        scenes.bind(&a);
        let window_a = window_pixels(
            scenes.bound(),
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_eq!(
            window_a,
            super::super::compose_human_visible(
                scenes.scene(&a).unwrap(),
                &mut ConsentSurface::new(TrustedIndicator::for_test()),
                &mut no_lock(),
                &no_blank(),
                &mut no_status(),
                (W as u32, H as u32).into(),
                false
            ),
            "the host window must be the bound realm's own composition"
        );
        let key_a = TextureKey::current(
            size,
            scenes.focused(),
            scenes.bound(),
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );

        // Bound to B: different pixels, and a different key even though every
        // other field is identical.
        scenes.bind(&b);
        let window_b = window_pixels(
            scenes.bound(),
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert!(
            window_a != window_b,
            "binding the output to another realm must change what the window shows"
        );
        let key_b = TextureKey::current(
            size,
            scenes.focused(),
            scenes.bound(),
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_eq!(
            key_a.scene_generation, key_b.scene_generation,
            "fixture check: the scene counters really do coincide, so the key below \
             cannot be distinguishing on them"
        );
        assert_ne!(
            key_a, key_b,
            "a bind must re-upload: without the realm in the key, two realms whose scene \
             generations coincide leave the previous realm's texture on the screen"
        );

        // And a capture of either realm is that realm's own view, whichever
        // is bound — this backend composes both on the CPU, so the bound one
        // has no privileged path.
        assert_eq!(
            capture_pixels(
                scenes.scene(&a).unwrap(),
                (size.w.max(0) as u32, size.h.max(0) as u32).into()
            )
            .expect("a nonzero view"),
            scenes
                .scene(&a)
                .unwrap()
                .compose((W as u32, H as u32).into())
        );
        assert!(
            capture_pixels(
                scenes.scene(&a).unwrap(),
                (size.w.max(0) as u32, size.h.max(0) as u32).into()
            ) != capture_pixels(
                scenes.scene(&b).unwrap(),
                (size.w.max(0) as u32, size.h.max(0) as u32).into()
            ),
            "two realms' captures must differ while both are live"
        );
    }

    /// **A hidden realm's dmabuf import is never what the window presents**
    /// (WS-E.1.3, issue #209) — the zero-copy half of the same claim
    /// [`the_window_shows_the_bound_realm_and_a_bind_re_uploads`] makes about
    /// the CPU path.
    ///
    /// The defect this pins was live and was **only** on the `--dmabuf` path:
    /// the retained GPU content was one slot for the whole session, written by
    /// whichever realm imported last and presented without ever consulting
    /// [`RealmScenes::focused`]. So a hidden realm's commit took over the
    /// human-visible window while the CPU path — keyed on the bound realm from
    /// the start — went on claiming the opposite. That falsifies
    /// `docs/book/src/limits.md`'s "only the realm the output is bound to is
    /// on screen" on the one path a human actually looks at.
    ///
    /// **Display-free, and here is exactly what that costs.** Presenting needs
    /// an EGL context and a host window; D-019(4) records headless as the only
    /// backend CI can run, and a [`GpuContent`] cannot even be *minted* without
    /// a live GL context. So this drives the real [`RealmGpuContent`] and the
    /// real [`zero_copy_source`] — the one function `try_redraw`'s branch and
    /// its `present_human_visible` argument both go through — with a stand-in
    /// content type. What is left uncovered is the GL submit itself, and that
    /// is a manual runbook step (`shim/docs/nested-multi-realm.md`).
    ///
    /// Both directions are asserted, because only the pair is evidence: a
    /// `zero_copy_source` that answered `None` forever would satisfy the
    /// hidden-realm assertion and silently delete the zero-copy path.
    ///
    /// [`GpuContent`]: crate::dmabuf::GpuContent
    #[test]
    fn a_hidden_realms_dmabuf_import_is_never_what_the_window_presents() {
        let (a, b) = (RealmId::new("realm-0"), RealmId::new("realm-b"));
        let mut scenes = RealmScenes::new((320, 200));
        let mut content: RealmGpuContent<&'static str> = RealmGpuContent::new();

        // The output shows A. The *hidden* realm B imports a dmabuf — the
        // exact sequence that used to put B's texture in the window.
        scenes.bind(&a);
        *content.slot_mut(&b) = Some("B's texture");
        assert!(
            zero_copy_source(&scenes, &content, false).is_none(),
            "a HIDDEN realm's dmabuf import must not become the frame the window presents; \
             with one retained slot for the session it did, and the CPU path's bound-realm \
             keying said otherwise"
        );

        // ...and the bound realm's own import *is* presented, so the assertion
        // above is not passing because nothing is ever selected.
        *content.slot_mut(&a) = Some("A's texture");
        assert_eq!(
            zero_copy_source(&scenes, &content, false),
            Some(&"A's texture"),
            "the bound realm's retained import is what the zero-copy path presents"
        );

        // Rebinding moves the selection with the output, both ways.
        scenes.bind(&b);
        assert_eq!(
            zero_copy_source(&scenes, &content, false),
            Some(&"B's texture")
        );
        scenes.bind(&a);
        assert_eq!(
            zero_copy_source(&scenes, &content, false),
            Some(&"A's texture")
        );

        // An overlay takes the window: the CPU compose path, whatever is
        // retained (`try_redraw`'s documented MVP seam).
        assert!(
            zero_copy_source(&scenes, &content, true).is_none(),
            "a consent card or a hold indicator forces the CPU path"
        );

        // A death clears exactly the dying realm's slot — the teardown funnel
        // is handed `slot_mut(realm)`, so it cannot reach a sibling's texture
        // the way one shared slot could.
        *content.slot_mut(&a) = None;
        assert!(
            zero_copy_source(&scenes, &content, false).is_none(),
            "the bound realm's cleared slot leaves nothing to present zero-copy"
        );
        assert_eq!(
            content.of(&b),
            Some(&"B's texture"),
            "one realm's teardown must leave a sibling's retained content alone"
        );

        // Nothing bound: there is no picture to be showing.
        let vacant = RealmScenes::new((320, 200));
        assert!(zero_copy_source(&vacant, &content, false).is_none());
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
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        let mut scene = Scene::new();
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());

        let base = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_eq!(
            base,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
            "an unchanged output must not force a re-upload"
        );

        // A prompt going up, and coming back down, both re-upload.
        consent.show_for_test(prompt_fixture());
        let shown = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_ne!(base, shown, "a prompt appearing must re-upload");
        consent.dismiss_for_test();
        let dismissed = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_ne!(shown, dismissed, "a prompt going away must re-upload");

        // The queue advancing to a different petition re-uploads too, so the
        // window cannot keep showing a decided petition's card.
        consent.show_for_test(prompt_fixture());
        let first = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        let mut next = prompt_fixture();
        next.principal =
            crate::identity::PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        consent.show_for_test(next);
        assert_ne!(
            first,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
            "a different petition must re-upload"
        );

        // **The blank cover** (WS-E.4.3, issue #223), the newest input to the
        // composition and therefore the newest way to be left out of this key.
        // It is inert on this backend -- `--blank-idle` is refused with
        // `--nested` -- which is exactly the property `lock_generation` had when
        // it was missing for the whole of WS-E.2.2: an input nobody notices
        // because the backend they are looking at never moves it.
        let mut blank = crate::backend::blank::BlankSurface::for_test();
        let unblanked = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &blank,
            None,
            None,
            false,
            &no_status(),
        );
        blank.set_covering(true);
        assert_ne!(
            unblanked,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &blank,
                None,
                None,
                false,
                &no_status()
            ),
            "the idle cover going up must re-upload: a cache that short-circuited it would \
             leave the confined app's own pixels in the window on the frame the panel is \
             about to go dark on"
        );

        // And the two pre-existing inputs still matter.
        let held = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        assert_ne!(
            held,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
            "a scene commit must re-upload"
        );

        // The lock, which this test omitted for the whole of WS-E.2.2 and
        // which the first human run of `shim/docs/nested-lock-screen.md`
        // caught: raising the cover changes every pixel, so a key that
        // compares equal across it presents the *unlocked* app to a human
        // whose input is already being consumed by the gate. Asserted in both
        // directions because `lower` is the direction that returns the human
        // to their session, and a stale cover there is a session that looks
        // locked forever.
        let unlocked = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        let locked = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &raised_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_ne!(unlocked, locked, "the lock screen going up must re-upload");

        // And a *second* raise in the same session, which is the exact shape
        // that failed on hardware: lock, unlock, lock again, with nothing else
        // moving. The counter must not return to a value already presented.
        let mut cycled = raised_lock();
        cycled.lower();
        let after_unlock = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &cycled,
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_ne!(
            locked, after_unlock,
            "the lock screen coming down must re-upload"
        );
        cycled.raise(crate::lock::LockContent {
            cause: crate::lock::LockCause::Chord,
            realms: vec![RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)],
            unlock: crate::lock::UnlockMethod::Passphrase,
            deadman_chord: "esc",
        });
        let relocked = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &cycled,
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        assert_ne!(
            after_unlock, relocked,
            "a SECOND lock over an idle client must re-upload -- the hardware bug"
        );
        assert_ne!(
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
            TextureKey::current(
                size_of(640, 480),
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
            "a resize must re-upload"
        );

        // The agent cursor is the newest input (D-019) and the same trap: a
        // sprite left out of the key would move only when something else
        // happened to change, which for an agent hovering over a static app
        // is never — the operator watches a frozen crosshair.
        let no_cursor = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        let at_100 = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            Some((100.0, 100.0)),
            false,
            &no_status(),
        );
        assert_ne!(
            no_cursor, at_100,
            "an agent cursor appearing must re-upload"
        );
        assert_ne!(
            at_100,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                Some((140.0, 100.0)),
                false,
                &no_status()
            ),
            "an agent cursor MOVING must re-upload"
        );
        assert_eq!(
            at_100,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                Some((100.4, 99.8)),
                false,
                &no_status()
            ),
            "a sub-pixel move draws the same sprite and must not re-upload"
        );
        assert_eq!(
            no_cursor,
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                Some((f64::NAN, 0.0)),
                false,
                &no_status()
            ),
            "a position that is not a number draws no sprite, so it is no transition"
        );

        // The attention marker (WS-E.1.7, issue #232) is the newest visible
        // element and fell into exactly the trap this test exists for: it was
        // drawn by `window_pixels` and absent from the key, so the cache
        // short-circuited the only call that would paint it. Pressing the key
        // changed nothing on the human's screen -- in the one backend where a
        // human can press it -- and every test stayed green, because nothing
        // else in the key had changed.
        //
        // Both directions are asserted. Erasing the marker when the window
        // lapses matters as much as drawing it: a marker that stayed up would
        // tell the human a defence is lifted when it is not, which is worse
        // than never drawing one.
        let closed = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            false,
            &no_status(),
        );
        let open = TextureKey::current(
            size,
            Some(&realm),
            &scene,
            &consent,
            &no_lock(),
            &no_blank(),
            None,
            None,
            true,
            &no_status(),
        );
        assert_ne!(
            closed, open,
            "the attention window OPENING must re-upload: the marker is drawn by \
             `window_pixels` and nothing else in the key changes when the human \
             presses the chord"
        );
        assert_ne!(
            open, closed,
            "...and CLOSING must re-upload too, or the marker outlives the window \
             and tells the human a lifted defence is still lifted"
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
        let realm = RealmId::new(crate::realm::WELL_KNOWN_REALM_ID);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 48), 64, 48).expect("content"));
        let mut consent = ConsentSurface::new(TrustedIndicator::for_test());

        // No hold: the window is the ordinary human-visible composition,
        // byte for byte. The indicator costs an idle session nothing.
        let idle = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_eq!(
            idle,
            super::super::compose_human_visible(
                &scene,
                &mut consent,
                &mut no_lock(),
                &no_blank(),
                &mut no_status(),
                (W as u32, H as u32).into(),
                false
            )
        );

        // Mid-hold: the top edge changes, and nothing below the bar does.
        let holding = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            Some(0.5),
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
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
        let prompt_only = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            None,
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        let prompt_and_hold = window_pixels(
            &scene,
            &mut consent,
            &mut no_lock(),
            &no_blank(),
            &mut no_status(),
            None,
            Some(0.9),
            None,
            None,
            false,
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
        );
        assert_ne!(prompt_and_hold, prompt_only);

        // And every visible step of the fill re-uploads.
        let mut keys: Vec<TextureKey> = Vec::new();
        for step in 0..=10 {
            keys.push(TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                Some(f64::from(step) / 10.0),
                None,
                false,
                &no_status(),
            ));
        }
        keys.dedup();
        assert!(
            keys.len() > 5,
            "the fill must re-upload as it grows, got {} distinct keys",
            keys.len()
        );
        assert_ne!(
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                Some(1.0),
                None,
                false,
                &no_status()
            ),
            TextureKey::current(
                size,
                Some(&realm),
                &scene,
                &consent,
                &no_lock(),
                &no_blank(),
                None,
                None,
                false,
                &no_status()
            ),
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
        /// How many frames this host has been asked for. The production
        /// implementation forwards to the host window; counting is what
        /// turns "how often does an armed hold repaint" into an assertion
        /// (see `an_armed_hold_kicks_the_chain_once_and_never_from_inside_it`).
        redraw_requests: usize,
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
                redraw_requests: 0,
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
        fn request_redraw(&mut self) {
            self.redraw_requests += 1;
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

        // The input turn's tick: this is what arms the timer in production,
        // and it is `OffChain` there for the same reason it is here.
        deadman_tick(&mut host, &handle, pressed_at, TickSource::OffChain);
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

        deadman_tick(
            &mut host,
            &handle,
            t0 + Duration::from_secs(5),
            TickSource::OffChain,
        );
        assert!(!host.timer_armed, "a forgotten hold armed a timer anyway");
        assert!(
            host.triggers.is_empty(),
            "a forgotten hold completed the chord"
        );
        assert_eq!(
            host.redraw_requests, 0,
            "a forgotten hold has no indicator to animate, so it must not wake the \
             compositor either"
        );
    }

    /// **An armed hold wakes the compositor from outside the redraw chain,
    /// and never from inside it.**
    ///
    /// Both halves are regressions that shipped, in opposite directions, one
    /// release apart:
    ///
    /// - Without the `OffChain` kick, pressing the chord while the window
    ///   idles at 0 fps leaves the indicator invisible until some unrelated
    ///   redraw comes along — the human holds the panic button and sees
    ///   nothing.
    /// - With the kick unconditional, the frame-cadence backstop inside
    ///   [`NestedState::redraw`] requested a fresh frame on *every* frame,
    ///   from inside the host's own `RedrawRequested` handling. That is not
    ///   coalesced with anything (the frame it runs in has not been drawn
    ///   yet), so the hold repainted as fast as the loop could turn, outside
    ///   [`FRAME_BUDGET`] — the un-budgeted spin P1.3.9 had just removed.
    ///
    /// The switch's own firing must be unaffected by either, which is what
    /// the trigger assertions below are for: under-ticking a dead-man switch
    /// is as bad as over-drawing for it.
    #[test]
    fn an_armed_hold_kicks_the_chain_once_and_never_from_inside_it() {
        let _fd = crate::capture::tests::fd_lock();
        let event_loop: EventLoop<'static, TestHost> = EventLoop::try_new().expect("loop");
        let handle = event_loop.handle();
        let mut host = TestHost::new(250);

        let t0 = Instant::now();
        host.switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), t0);

        // The arming input turn: nothing else will ask for a frame, so this
        // one must.
        deadman_tick(&mut host, &handle, t0, TickSource::OffChain);
        assert_eq!(
            host.redraw_requests, 1,
            "an armed hold must wake an idle window, or the indicator never appears"
        );

        // Ten frames' worth of the in-chain backstop. `schedule_next_frame`
        // is what paces the chain; these ticks must add nothing to it.
        for frame in 0..10 {
            deadman_tick(
                &mut host,
                &handle,
                t0 + Duration::from_millis(frame * 16),
                TickSource::InChain,
            );
        }
        assert_eq!(
            host.redraw_requests, 1,
            "the frame-cadence backstop requested a redraw from inside the redraw chain: \
             the hold now repaints as fast as the loop turns, not at FRAME_BUDGET"
        );

        // The tick still does its real job on both sources: the hold has not
        // elapsed, the timer is armed, and the chord completes on time.
        assert!(host.timer_armed);
        assert!(host.triggers.is_empty(), "the hold has not elapsed yet");
        deadman_tick(
            &mut host,
            &handle,
            t0 + Duration::from_millis(300),
            TickSource::InChain,
        );
        assert_eq!(
            host.triggers.len(),
            1,
            "an in-chain tick must still complete an elapsed chord -- it is backstop 3 of 3"
        );
    }

    /// The chain's cadence while a hold animates: [`FRAME_BUDGET`], never
    /// loop speed, and it stops the moment the hold does.
    ///
    /// [`next_frame`] is the whole decision [`NestedState::schedule_next_frame`]
    /// executes, split out because presenting needs a host window but pacing
    /// is arithmetic — and because a hold indicator that under-paints is a
    /// safety defect, not a cosmetic one.
    #[test]
    fn a_hold_repaints_at_the_frame_budget_not_at_loop_speed() {
        // No hold: the chain stops and the window idles at 0 fps, whatever
        // the frame cost.
        assert_eq!(next_frame(None, Duration::ZERO), NextFrame::Idle);
        assert_eq!(next_frame(None, Duration::from_secs(1)), NextFrame::Idle);

        // A hold, on a host whose swap does not block: the whole budget is
        // still owed, so the next frame waits.
        assert_eq!(
            next_frame(Some(0.0), Duration::ZERO),
            NextFrame::After(FRAME_BUDGET),
            "an instant frame must defer the next one by the full budget"
        );
        assert_eq!(
            next_frame(Some(0.5), FRAME_BUDGET / 4),
            NextFrame::After(FRAME_BUDGET - FRAME_BUDGET / 4)
        );

        // A hold, on a host with real vsync throttling: the swap already
        // spent the budget, so the next request goes out immediately.
        assert_eq!(next_frame(Some(0.5), FRAME_BUDGET), NextFrame::Now);
        assert_eq!(next_frame(Some(1.0), FRAME_BUDGET * 3), NextFrame::Now);
    }

    /// The keyboard filter that decides what reaches intake at all.
    ///
    /// The release half of this shipped inverted (#124) and nothing caught
    /// it: winit's X11 backend reports keys that were down at focus-out as
    /// **synthetic releases**, and dropping them left the key latched down in
    /// the confined app with no event that could ever clear it. Synthetic
    /// *presses* — keys already down when focus arrived — must still be
    /// dropped: the core never saw that press begin.
    #[test]
    fn a_synthetic_release_reaches_intake_but_a_synthetic_press_does_not() {
        // Real events, both directions.
        assert!(admits_key_event(false, false, ElementState::Pressed));
        assert!(admits_key_event(false, false, ElementState::Released));

        // The focus-out releases: the only notice the core gets that a key
        // held across an alt-tab came up.
        assert!(
            admits_key_event(true, false, ElementState::Released),
            "a synthetic release is the focus-out release; dropping it latches the key"
        );

        // The focus-in presses: a gesture the core never saw begin, and one
        // the dead-man watcher must not be handed.
        assert!(
            !admits_key_event(true, false, ElementState::Pressed),
            "a synthetic press is a key the core never saw go down"
        );

        // Autorepeat is noise on every path: the switch owns its own clock.
        for synthetic in [false, true] {
            for state in [ElementState::Pressed, ElementState::Released] {
                assert!(
                    !admits_key_event(synthetic, true, state),
                    "autorepeat must never reach intake ({synthetic}, {state:?})"
                );
            }
        }
    }

    #[test]
    fn a_tap_is_replayed_to_the_app_by_the_routing_turn() {
        // The drain, which is what gives the confined app its Escape key
        // back. Removing it is invisible in every other test and costs
        // Firefox the Escape key permanently -- every tap swallowed, none
        // ever replayed.
        let deadman = Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default())));
        let now = Rc::new(Cell::new(Instant::now()));
        let mut router = input::InputRouter::detached(DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            input::NoopHook,
        ));
        // The human's attention has to be somewhere for a physical delivery
        // to have a destination (WS-E.1.6).
        assert!(router.bind_to(&crate::input::tests::test_realm()).is_none());
        let view: crate::view::ViewGeometry = (640u32, 480u32).into();
        let surface = Some(view.usable());
        let mut delivered: Vec<input::SeatDeliveryKind> = Vec::new();

        // The press alone reaches nobody: it is withheld pending the
        // tap-or-chord question.
        route_turn(
            &mut router,
            Some(&deadman),
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
            Some(&deadman),
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
        let mut router = input::InputRouter::detached(DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            input::NoopHook,
        ));
        let view: crate::view::ViewGeometry = (640u32, 480u32).into();

        assert_eq!(deadman.borrow().deadline(), None);
        let _ = router.route_physical(
            crate::input::tests::chord_press(),
            view,
            Some(view.usable()),
        );
        assert_eq!(
            deadman.borrow().deadline(),
            Some(now.get() + crate::deadman::DEFAULT_HOLD),
            "the router's hook and the backend's handle are not the same switch"
        );
    }
}
