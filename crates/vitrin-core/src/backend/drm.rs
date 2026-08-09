// SPDX-License-Identifier: MPL-2.0
//! Bare-metal backend: the core owns the display controller outright — DRM/KMS
//! mode setting, a GBM swapchain scanned out by the panel, libinput for the
//! human's real keyboard and mouse, and libseat for the seat that hands over
//! all three (WS-E.3.2, issue #218).
//!
//! This is the third [`session::Presenter`]/[`session::RuntimeHost`] pair, and
//! the first that is *the human's whole display* rather than a window inside
//! somebody else's. Everything downstream of the output stage is unchanged and
//! deliberately so: it composites through the same
//! [`super::human_visible_from_view`] the nested backend does, serves captures
//! from the same [`Scene::compose`], and mints physical input through the same
//! private constructor in [`crate::input`]. What is new here is what a bare
//! machine forces and a nested window does not.
//!
//! # Four things bare metal forces that nesting did not
//!
//! **1. The frame clock is the panel's.** A composite is not a presentation:
//! the flip lands at vblank, and only then is a realm's `frame_done` due. So
//! [`session::Presenter::redraw`] answers [`session::Presentation::Scheduled`]
//! — the nested posture, for the reason `session::Presentation` states — and
//! [`session::emit_presented`] is called from the vblank handler
//! ([`DrmState::on_vblank`]). Answering `Completed` at composite time would
//! hand a `frame_done`-paced shim a permit per dispatch round with no
//! presentation between them, and it would stop throttling.
//!
//! **2. There is no host desktop to draw the human's pointer**, so the core
//! draws it: [`crate::cursor::composite_human_cursor`] on the CPU path and
//! [`crate::dmabuf::human_visible_frame`]'s human-cursor slots on the
//! zero-copy one. It is applied on the human-visible side of the output stage,
//! exactly where the agent sprite, the consent card and the trusted band go,
//! so no capture can contain it. `protocol/vitrin-v0.xml` used to say no human
//! cursor is composited at all; that sentence is now nested-conditional, which
//! is the paired IDL + `docs/protocol/06-vitrin_view.md` edit this backend
//! forced.
//!
//! **3. libinput gives a scancode and a relative delta, and neither is what
//! the wire carries.** The keyboard arm resolves through the core's own xkb
//! keymap (WS-E.3.1, D-028, [`crate::input::keymap_key`]) rather than
//! [`crate::input::intake_physical`]'s scancode-only arm, which maps no letter
//! and no digit; the pointer arm accumulates `PointerMotion`'s relative delta
//! into the absolute view position `SeatInputKind::Motion` is defined in
//! ([`accumulate_pointer`]), because `intake_physical` drops relative motion
//! and an ordinary mouse emits nothing else. Everything else — buttons, axes,
//! absolute motion from a touchscreen — goes through `intake_physical`
//! unchanged, which is where the origin tag and the v120 conversion live.
//!
//! **4. Nothing here may be fatal to the session.** The runtime stops the loop
//! for exactly one condition, a `redraw` that returns `Err`
//! ([`session::post_dispatch`] hands it to `RuntimeHost::stop`). A page flip
//! that returns `EBUSY`, a modeset race, or a frame composed while the VT is
//! switched away are transient facts about a display, not reasons to end a
//! session that is holding an agent's grants and a human's apps. So the
//! presentation path logs and keeps the frame owed, and `redraw` is infallible
//! in practice — the same posture `session::install`'s `AcceptError` arm takes.
//!
//! # There is exactly one presentation path pair, and both carry the band
//!
//! Two branches, like nested: the CPU composite (a full-view texture uploaded
//! and blitted) and the zero-copy dmabuf present taken when the bound realm
//! has a retained GPU import and no overlay needs the compositor
//! ([`super::winit::zero_copy_source`], [`super::winit::overlay_needs_the_window`],
//! reused rather than restated so the two backends cannot disagree about when
//! a client texture may go straight to the display). Both end in
//! [`crate::dmabuf::human_visible_frame`]'s draw list or in
//! [`super::winit::window_pixels`], and both therefore carry the trusted band:
//! `backend/mod.rs`'s module docs put the band *inside* the draw list
//! precisely so a third presentation path added later cannot drop it, and this
//! is that third path.
//!
//! **No hardware cursor plane.** A KMS cursor plane is composited by the
//! display controller, outside any draw list this core owns — a fourth
//! presentation path with no band in it and no way to put one there. Both
//! sprites are drawn into the frame instead, at the cost of a full composite
//! per pointer move.
//!
//! # One output, and a startup refusal rather than a guess
//!
//! [`pick_sole_connected`] refuses to start on zero or more than one connected
//! connector, and the reason is in the [`session::Presenter`] contract rather
//! than in how much content exists to show. See that function.
//!
//! # What no green check in this repository proves
//!
//! That this backend lights a panel, presents a frame, or delivers a key. CI
//! runners have no DRM device and no seat, and the acceptance evidence is
//! WS-E.3.4's runbook executed on real hardware. Everything here that *can* be
//! held without a device is held as a free function with its own test —
//! [`pick_sole_connected`], [`accumulate_pointer`] — for the reason
//! `backend::winit`'s `window_pixels` and `next_frame` were split out: a
//! decision written inline in a method no test can reach is a decision a
//! reviewer can revert while the suite stays green.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopHandle, LoopSignal};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, GbmBufferedSurface};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    AbsolutePositionEvent, InputEvent, KeyState as HostKeyState, KeyboardKeyEvent,
    PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, Frame, ImportDma, ImportMem, Renderer, Texture};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::reexports::drm::control::{
    connector, crtc, Device as ControlDevice, Mode as DrmMode, ModeTypeFlags,
};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{Buffer, DeviceFd, Physical, Rectangle, Size, Transform};
use tracing::{debug, error, info, trace, warn};
use vitrin_protocol::generated::vitrin_shim_seat::KeyState as SeatKeyState;

use super::winit::{
    overlay_needs_the_window, upload_status_texture, window_pixels, zero_copy_source, DeadManHost,
    TickSource,
};
use crate::consent::grab::{ConsentGate, ConsentGrab};
use crate::consent::{ConsentSurface, TrustedIndicator};
use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch, Trigger};
use crate::dmabuf::{present_human_visible, DmabufImporter, GlesDmabufImporter, RealmGpuContent};
use crate::grants::RealmId;
use crate::input::{self, keymap::CoreKeymap};
use crate::recorder::Recorder;
use crate::scene::{RealmScenes, Scene};
use crate::session::{self, Presenter, Runtime, RuntimeSeed};

/// The output transform every draw into a **scanout** buffer takes.
///
/// The same value the nested backend's window surface uses, and for the same
/// reason: GL's framebuffer origin is bottom-left while every composition in
/// this core is top-down (`Scene::compose`'s contract), and a DRM scanout
/// reads the buffer top-down from offset zero — so the projection is flipped
/// exactly as it is for the host window. (The offscreen renderbuffer in
/// `dmabuf`'s GPU harness needs `Normal` because `copy_framebuffer` reads it
/// back the other way up; that is a readback, not a display.)
///
/// **Unverified on hardware, and it cannot be verified here.** It is a
/// reasoned default, not a measurement: nothing in this workspace can put a
/// frame on a panel. A frame presented upside down is the symptom, and
/// WS-E.3.4's bring-up runbook owns the check.
const SCANOUT_TRANSFORM: Transform = Transform::Flipped180;

/// Background behind the composed view, visible only if the blit fails.
/// The nested backend's constant, restated here rather than shared because it
/// is `CLEAR_COLOR` there and private; both are deliberately near
/// [`crate::scene::LETTERBOX_RGBA`] so nothing here reads as client content.
const CLEAR_COLOR: Color32F = Color32F::new(0.06, 0.06, 0.08, 1.0);

/// The scanout formats offered to [`GbmBufferedSurface`], in preference
/// order: opaque first, since the panel composites nothing under this buffer
/// and an alpha channel it will ignore only costs bandwidth.
const SCANOUT_FORMATS: &[Fourcc] = &[Fourcc::Xrgb8888, Fourcc::Argb8888];

/// The hook stack this backend's router carries.
///
/// **The nested backend's alias, reused rather than respelled.** The order —
/// lock, consent grab, dead-man watcher, clipboard, screenshot, attention — is
/// a decision, and `backend::winit`'s `NestedHook` is where it is written
/// down, complete with the argument for why the lock gate must be outermost. A
/// second spelling here would be a second place that decision could drift, and
/// the drift would be silent: `Runtime::new` takes the presence map, the
/// attention signal and both chord queues *out of the router*, so a stack
/// missing one hands the kernel a signal nothing ever writes — the mechanism
/// dies and every test stays green.
pub(crate) type DrmHook = super::winit::NestedHook;

// ============================================================================
// Output selection: the one refusal this backend makes at startup
// ============================================================================

/// Why a bare-metal session refused to start.
///
/// Both variants are startup-fatal and both name what the operator can act on,
/// on the precedent of `main`'s refusal table and `realm.rs`'s loader: a
/// session that comes up in a state its own contract cannot describe is the
/// fail-open shape this codebase refuses wherever configuration is read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutputRefusal {
    /// Nothing is plugged in.
    NoConnectedConnector,
    /// More than one panel is plugged in, named in enumeration order.
    SeveralConnectedConnectors(Vec<String>),
}

impl fmt::Display for OutputRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConnectedConnector => write!(
                f,
                "no connected display: this backend IS the human's display, and a session with \
                 nowhere to draw could raise a consent card nobody can read and a lock nobody \
                 can answer. That is `--headless --consent=interactive`'s refusal one step \
                 further out. Plug a display in, or run `--headless`"
            ),
            Self::SeveralConnectedConnectors(names) => write!(
                f,
                "{} connected displays ({}), and this core models exactly one. The limit is in \
                 the `Presenter` contract, not in how much there is to show: `view_size` is one \
                 size for the whole session and every realm's shim is configured with it, the \
                 output has a single `bound` realm that the human's seat target and the agent \
                 cursor's coordinate space both resolve through, and `apply_output_resize` \
                 bumps every realm's layout generation from one size. Two panels would mean \
                 lighting whichever connector enumerated first, binding the output to one \
                 realm, and leaving a powered display dark with no message and no verb that \
                 could ever move the output to it. Unplug all but one",
                names.len(),
                names.join(", ")
            ),
        }
    }
}

impl Error for OutputRefusal {}

/// Choose the session's one output from the connectors reported connected, or
/// refuse.
///
/// # Why one, re-derived
///
/// Issue #218's own justification for this refusal — "`Scene` holds at most
/// one client surface and `MAX_REALMS = 1`, so a second output has nothing to
/// display" — is **stale in both halves**: `MAX_REALMS` is 16 (`realm.rs`,
/// raised by WS-E.1.2) and every realm has had its own `Scene` since WS-E.1.3
/// (`scene::RealmScenes`). A session really can hold sixteen live realms with
/// fifteen of them unbound, each with content that could in principle drive a
/// second panel.
///
/// The refusal holds anyway, on a different and stronger basis: **the
/// singularity is in the contract, not in the content.** Six places, each of
/// which has exactly one answer and would need two:
///
/// 1. [`session::Presenter::view_size`] is one size for the whole session, and
///    `start_realm_in` reads it **once** before the first realm forks and
///    configures every shim with it. Two connectors with different preferred
///    modes have no single answer.
/// 2. `RealmScenes::bound` is a single `Option<RealmId>`, and
///    `focused`/`bind_output`/`unbind_output` are its only accessors.
/// 3. `physical_seat_target` follows that one binding — D-018(2)'s fifth
///    ordering rule is that the human watches and types into the same realm.
///    Two lit panels and one keyboard means the rule has two answers and the
///    core picks one silently.
/// 4. `post_dispatch` draws the agent cursor in **output** coordinates
///    resolved from `view.focused()`; a second output has no coordinate space
///    in that model.
/// 5. `apply_output_resize` bumps every realm's `layout_generation` from one
///    size (D-018(5)).
/// 6. `refresh_status` captions the strip with `scenes.focused()` — one
///    caption.
///
/// And the fail-open consequence is *worse* now than the stale argument
/// claimed, not milder: with sixteen realms an operator has plausibly
/// configured content for both panels, so coming up anyway would light one and
/// leave the other dark with nothing in the protocol able to move the output
/// to it.
///
/// # The gap this does not close, stated rather than left undecided
///
/// This is a **startup** refusal. A connector hot-plugged *after* startup is
/// not covered: this backend enumerates once and inserts no udev monitor, so a
/// second panel plugged in mid-session is neither lit nor complained about,
/// and unplugging the only panel leaves the session compositing into a surface
/// nobody sees. That is a deliberate omission with a stated shape rather than
/// an oversight — handling it means answering what a session *does* when its
/// only display disappears.
///
/// **D-030 does not answer it, and this sentence used to imply WS-E.3.3
/// would.** A seat pause and an unplug look alike from here and are not the
/// same fact: a paused session still has its panel and is told when it gets it
/// back, so the answer is "hold everything, and resume" — while an unplugged
/// session's panel is gone with no event promising its return, and holding a
/// consent card and a lock for a screen that no longer exists is a different
/// decision with a different failure mode. It stays unowned rather than being
/// absorbed by D-030, and it is a numbered item in WS-E.3.4's runbook.
///
/// Takes names rather than `connector::Info` so the decision is testable with
/// no device — the shape `backend::winit::next_frame` established.
pub(crate) fn pick_sole_connected(connected: &[String]) -> Result<usize, OutputRefusal> {
    match connected.len() {
        0 => Err(OutputRefusal::NoConnectedConnector),
        1 => Ok(0),
        _ => Err(OutputRefusal::SeveralConnectedConnectors(
            connected.to_vec(),
        )),
    }
}

/// A connector's name in the spelling operators see (`eDP-1`, `DP-3`).
fn connector_name(info: &connector::Info) -> String {
    format!("{}-{}", info.interface().as_str(), info.interface_id())
}

/// The mode this connector will be driven at: its preferred one, or its first
/// if the kernel marks none preferred.
///
/// A missing preferred mode is not a refusal — some panels and most virtual
/// connectors report none — but it is logged, because a session running at a
/// mode nobody chose is a fact the operator should be able to find afterwards.
fn preferred_mode(info: &connector::Info) -> Option<DrmMode> {
    if let Some(mode) = info
        .modes()
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
    {
        return Some(*mode);
    }
    let first = info.modes().first().copied();
    if first.is_some() {
        warn!(
            connector = connector_name(info),
            "this display reports no PREFERRED mode; taking its first"
        );
    }
    first
}

// ============================================================================
// The presentation half
// ============================================================================

/// The scanout half: everything that exists only because this process holds
/// DRM master.
///
/// Split out of [`DrmView`] rather than flattened into it so the composite can
/// take `&mut self.output.renderer` and `&mut self.output.surface` while the
/// scene registry, the consent surface and the status strip beside them are
/// borrowed too — the same field-disjointness discipline `RuntimeHost::split`
/// exists for, one level down.
struct DrmOutput {
    /// The GBM device, held for its lifetime rather than its API: the EGL
    /// display and the allocator are both derived from it and neither keeps it
    /// alive.
    _gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlesRenderer,
    surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    /// The CRTC's active mode, in physical pixels. This is
    /// [`session::Presenter::view_size`]'s answer and it is final before the
    /// first realm forks — `start_realm_in` reads the size **once** and
    /// configures every shim with it, so a backend still holding a placeholder
    /// at that moment misconfigures the whole session silently.
    mode: Size<i32, Physical>,
    /// A page flip is in flight. A second `drmModePageFlip` before the first
    /// completes returns `EBUSY`, so this is what makes the flip request
    /// idempotent between vblanks.
    flip_pending: bool,
    /// A presentation is **owed**: something changed and no flip carrying it
    /// has been queued yet.
    ///
    /// Set by [`session::Presenter::request_present`], cleared only when a
    /// flip is actually queued. The debt surviving a round is the point — a
    /// present asked for while a flip is in flight, or while the VT is
    /// switched away, is serviced at the next vblank or the next activation
    /// rather than lost.
    wanted: bool,
    /// Whether this session currently holds the devices. `false` between
    /// `PauseSession` and `ActivateSession` (a VT switch), when queueing a
    /// flip would fail and the buffers are no longer ours.
    active: bool,
    /// The CPU path's uploaded frame, re-used across composites when the
    /// output size has not changed.
    ///
    /// Deliberately **not** the nested backend's `TextureKey` cache: that one
    /// decides *whether to re-upload*, which matters when the host asks for a
    /// frame at its own cadence whether anything changed or not. Here a
    /// composite happens only because something asked for one, so the upload
    /// is owed by definition and the only thing worth keeping is the texture
    /// allocation.
    texture: Option<GlesTexture>,
    /// The status strip's uploaded raster, keyed as the nested backend keys
    /// it (`(generation, width)`) through the shared
    /// [`upload_status_texture`].
    status_texture: Option<(u64, u32, GlesTexture)>,
}

/// The bare-metal [`session::Presenter`].
pub(crate) struct DrmView {
    output: DrmOutput,
    /// **Every realm's scene, and the one the output composites** — the same
    /// registry both other backends hold, so nothing about per-realm capture
    /// isolation (WS-E.1.3) is re-decided here.
    scenes: RealmScenes,
    consent: ConsentSurface,
    lock: crate::lock::LockSurface,
    indicator: TrustedIndicator,
    status: crate::status::StatusStrip,
    dmabuf_content: RealmGpuContent,
    agent_cursor: Option<(f64, f64)>,
    /// **The human's own pointer**, in view coordinates, accumulated from
    /// libinput's relative deltas by [`accumulate_pointer`].
    ///
    /// One value serving two purposes on purpose: it is both the position
    /// `SeatInputKind::Motion` carries to the realm's shim and the position
    /// the cursor sprite is drawn at, so what the human sees and where the app
    /// is told the pointer is cannot drift.
    human_cursor: (f64, f64),
    attention: bool,
    /// The dead-man switch, shared with [`DrmState::deadman`] and with the
    /// router's hook.
    ///
    /// The presentation half needs to *read* the hold's progress, unlike on
    /// the nested backend, because there the composite runs inside
    /// `NestedState::try_redraw` — a method with the whole state in hand —
    /// while here it is driven by the panel's own clock through
    /// [`session::Presenter::request_present`] and [`DrmState::on_vblank`],
    /// where only the view is reachable. An `Rc` clone of the same switch, not
    /// a copy of its state: a second switch would animate an indicator for a
    /// hold the router is not watching.
    deadman: Rc<RefCell<DeadManSwitch>>,
}

impl DrmView {
    fn view_size(&self) -> (u32, u32) {
        (
            self.output.mode.w.max(0) as u32,
            self.output.mode.h.max(0) as u32,
        )
    }

    /// Queue a page flip carrying the current state, if one is owed and the
    /// hardware can take it.
    ///
    /// Idempotent and level-triggered, like every other "service one turn"
    /// entry point in this core: safe to call from the vblank handler, from a
    /// dispatch round's `redraw`, and from an input turn that moved the
    /// pointer.
    ///
    /// **A failure here is never fatal.** The frame stays owed and the next
    /// occasion retries; see this module's docs for why a transient flip error
    /// must not reach `redraw`'s `Result`.
    fn service_present(&mut self) {
        if !should_queue_flip(
            self.output.wanted,
            self.output.flip_pending,
            self.output.active,
        ) {
            return;
        }
        match self.compose_and_queue() {
            Ok(()) => {
                self.output.wanted = false;
                self.output.flip_pending = true;
            }
            Err(err) => {
                // Logged and kept owed rather than returned: a hiccup on the
                // display controller must not end a session holding a human's
                // apps and an agent's grants.
                warn!(%err, "could not queue this frame; it stays owed and the next vblank retries");
            }
        }
    }

    /// Compose one frame and hand it to the swapchain. The two branches are
    /// the nested backend's two, with the same condition deciding between
    /// them.
    fn compose_and_queue(&mut self) -> Result<(), Box<dyn Error>> {
        let size = self.output.mode;
        if size.w <= 0 || size.h <= 0 {
            return Ok(());
        }
        // Sampled once for the whole frame, from one clock reading, exactly as
        // the nested backend samples it: a hold that completes mid-frame is
        // judged consistently within it.
        let hold = self.deadman.borrow().hold_progress(Instant::now());
        let overlay_up = overlay_needs_the_window(hold, &self.consent, &self.lock);
        let agent_cursor = self.agent_cursor;
        // Always `Some` on this backend, and that is the whole of decision 6:
        // there is no host desktop drawing this, so a `None` here is a session
        // with no visible pointer.
        let human_cursor = Some(self.human_cursor);

        if zero_copy_source(&self.scenes, &self.dmabuf_content, overlay_up).is_some() {
            let indicator = self.indicator;
            // Uploaded before the swapchain buffer is acquired: this borrows
            // `status_texture` for the rest of the frame, and the acquire
            // below borrows `surface`, which is a different field.
            let status = upload_status_texture(
                &mut self.output.renderer,
                &mut self.status,
                &mut self.output.status_texture,
                size.w.max(0) as u32,
            );
            let (mut dmabuf, _age) = self.output.surface.next_buffer()?;
            let mut framebuffer = self.output.renderer.bind(&mut dmabuf)?;
            // Resolved a second time rather than held across the acquire, the
            // nested backend's discipline: re-asking the same function is what
            // keeps the branch condition and the presented texture from being
            // two different decisions.
            let content = zero_copy_source(&self.scenes, &self.dmabuf_content, overlay_up)
                .expect("checked above");
            let sync = present_human_visible(
                &mut self.output.renderer,
                &mut framebuffer,
                size,
                SCANOUT_TRANSFORM,
                content,
                indicator,
                agent_cursor,
                human_cursor,
                status,
            )?;
            drop(framebuffer);
            // **The fence, unlike nested.** There is no EGL swapchain here to
            // order the write against the read: the display controller will
            // scan this buffer out, so it is handed the sync point the GL
            // commands finished on.
            self.output.surface.queue_buffer(Some(sync), None, ())?;
            trace!(
                ?size,
                "dmabuf frame queued for scanout with zero core-side copies"
            );
            return Ok(());
        }

        // The CPU path: the same composition the nested backend uploads, from
        // the same function, so the two backends cannot present different
        // human-visible output for the same session state.
        let pixels = window_pixels(
            self.scenes.bound(),
            &mut self.consent,
            &mut self.lock,
            &mut self.status,
            hold,
            agent_cursor,
            human_cursor,
            self.attention,
            size,
        );
        let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
        let full = Rectangle::from_size(size);
        let reusable = self.output.texture.as_ref().is_some_and(|texture| {
            texture.width() == size.w as u32 && texture.height() == size.h as u32
        });
        if reusable {
            let texture = self.output.texture.as_ref().expect("checked above");
            self.output.renderer.update_memory(
                texture,
                &pixels,
                Rectangle::from_size(buffer_size),
            )?;
        } else {
            self.output.texture = Some(self.output.renderer.import_memory(
                &pixels,
                Fourcc::Abgr8888,
                buffer_size,
                false,
            )?);
        }

        let (mut dmabuf, _age) = self.output.surface.next_buffer()?;
        let mut framebuffer = self.output.renderer.bind(&mut dmabuf)?;
        let sync = {
            let texture = self.output.texture.as_ref().expect("uploaded above");
            let mut frame =
                self.output
                    .renderer
                    .render(&mut framebuffer, size, SCANOUT_TRANSFORM)?;
            frame.clear(CLEAR_COLOR, &[full])?;
            // Qualified call: `GlesFrame` has an inherent method of the same
            // name (extra custom-shader arguments) that would shadow the
            // renderer-agnostic trait method.
            Frame::render_texture_from_to(
                &mut frame,
                texture,
                Rectangle::from_size(Size::<f64, Buffer>::from((size.w as f64, size.h as f64))),
                full,
                &[full],
                &[],
                Transform::Normal,
                1.0,
            )?;
            frame.finish()?
        };
        drop(framebuffer);
        self.output.surface.queue_buffer(Some(sync), None, ())?;
        trace!(?size, "frame queued for scanout");
        Ok(())
    }
}

impl Presenter for DrmView {
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

    /// The CRTC's active mode. Final from construction: mode setting happens
    /// while the backend is being built, never lazily on the first flip, so
    /// this already answers the real size when `start_realm_in` reads it once
    /// before the first fork.
    fn view_size(&self) -> (u32, u32) {
        DrmView::view_size(self)
    }

    /// Propagation, not a resize. Nothing in this backend changes mode after
    /// startup today, so in production this is unreached — exactly as the
    /// headless backend documents for its fixed virtual output. It is
    /// implemented as propagation rather than a panic because the way a mode
    /// change would arrive is through [`session::apply_output_resize`], which
    /// also re-configures every fullscreen realm; a handler that called
    /// `scenes.set_view_size` directly would propagate the size without the
    /// re-configure and silently falsify `set_fullscreen`'s promise.
    fn set_view_size(&mut self, size: (u32, u32)) {
        self.scenes.set_view_size(size);
    }

    /// Composites nothing here, and answers [`session::Presentation::Scheduled`].
    ///
    /// A composite queues a page flip; the *presentation* is the flip landing
    /// at vblank, which is where [`session::emit_presented`] is called from
    /// ([`DrmState::on_vblank`]). Answering `Completed` at queue time would be
    /// the silent pacing bug `session::Presentation` documents.
    ///
    /// It still asks for the frame — `request_present` then `service_present`
    /// — because `post_dispatch` reaches here on a dirty round without
    /// necessarily having called `request_present` this round, and a dirty
    /// round that queued nothing would leave the change on screen only
    /// whenever something else happened to ask.
    ///
    /// **Infallible on purpose.** `Err` here is the one condition the runtime
    /// stops the loop for; a display controller's transient refusal is not
    /// that. See this module's docs.
    fn redraw(&mut self) -> Result<session::Presentation, Box<dyn Error>> {
        self.output.wanted = true;
        self.service_present();
        Ok(session::Presentation::Scheduled)
    }

    /// The bare realm view, composed on the CPU from the retained scene — the
    /// nested backend's answer, byte for byte, through the same
    /// [`super::winit::capture_pixels`].
    ///
    /// **Never a scanout readback**, and this is the sharpest instance of that
    /// rule in the codebase: the scanout buffer is the human's display, so it
    /// holds the trusted band, the status strip, the consent card, the lock
    /// cover, the dead-man indicator and now both cursor sprites. Reading it
    /// back would put every one of them into every `vitrin_view.frame_ready`,
    /// which `docs/protocol/05-vitrin_consent.md` forbids outright — and it
    /// would put this session's *secret* band colour into a frame an agent
    /// receives, which is the forgery the indicator exists to prevent.
    /// Composing the scene instead is overlay-free structurally:
    /// [`Scene::compose`] is upstream of the output-stage fork.
    ///
    /// One path for the bound realm and the hidden ones alike, as nested has:
    /// the only thing this backend could read back is the forbidden source, so
    /// there is no bound-realm special case to be tempted by (headless has one
    /// because it retains a readable image that is *not* human-visible
    /// output).
    fn view_rgba(&mut self, realm: &RealmId) -> Option<Vec<u8>> {
        super::winit::capture_pixels(self.scenes.scene(realm)?, self.output.mode)
    }

    /// Mark a presentation owed and try to queue it now.
    ///
    /// The default is a no-op — headless's posture, where a completed
    /// composite *is* the output cadence — and inheriting it here would mean
    /// this backend composites nothing, ever, with every test still green:
    /// `redraw` returns `Scheduled`, so the only thing that ever turns a
    /// change into a frame is this method and the vblank handler.
    fn request_present(&mut self) {
        self.output.wanted = true;
        self.service_present();
    }

    /// The agent sprite, on the nested backend's terms: compared on the
    /// **quantized** hotspot, so two positions inside one view pixel do not
    /// cost a frame that composites nothing new.
    ///
    /// No `--agent-cursor` opt-in here. That flag exists on the headless
    /// backend alone because its human-visible framebuffer is measured
    /// byte-for-byte by the trusted-band witness and by
    /// `tests/integration/test_real_trust_band.py`; this backend is a human's
    /// actual display, which is the situation D-019 was written for.
    fn set_agent_cursor(&mut self, pos: Option<(f64, f64)>) -> bool {
        let quantize =
            |pos: Option<(f64, f64)>| pos.and_then(|(x, y)| crate::cursor::hotspot(x, y));
        if quantize(self.agent_cursor) == quantize(pos) {
            return false;
        }
        self.agent_cursor = pos;
        true
    }

    /// The attention marker (WS-E.1.7). Identical to both shipped backends,
    /// and it must be: the marker is drawn inside the shared
    /// [`super::human_visible_from_view`], so withholding it here would make
    /// two backends present different human-visible output for the same
    /// session state.
    fn set_attention(&mut self, open: bool) -> bool {
        if self.attention == open {
            return false;
        }
        self.attention = open;
        true
    }

    /// The status strip, sampled against the realm the **output** is bound to
    /// rather than the router's — the strip captions the picture the human is
    /// looking at.
    fn refresh_status(&mut self, now: std::time::SystemTime, mono: Instant) -> bool {
        self.status.refresh(now, mono, self.scenes.focused())
    }

    /// The dying realm's scene and a dmabuf importer over **its own** retained
    /// slot; `None` for the retained-framebuffer half.
    ///
    /// `None` is the nested backend's answer and the argument carries over
    /// unchanged: what this backend retains is a *scanout* buffer, which is
    /// not a readable image of a realm view and is never served to anyone, and
    /// [`Self::view_rgba`] composes each capture fresh from the live scene. The
    /// death funnel takes the dead realm's surface out of that scene, so its
    /// pixels are gone by construction with nothing to scrub.
    ///
    /// The importer half is very much needed: a live `GlesRenderer` exists, so
    /// a dying realm's retained GPU content must be dropped through the same
    /// disposal path a replacing commit would take.
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
            renderer: &mut self.output.renderer,
            content: self.dmabuf_content.slot_mut(realm),
        };
        f(self.scenes.scene_mut(realm), None, Some(&mut importer))
    }

    /// This realm's scene and an importer over this realm's slot, both lent
    /// for one shim dispatch.
    ///
    /// **Must override the default**, which is the headless posture: no GPU
    /// renderer, so every `kind=dmabuf` commit resolves as the designed
    /// `import_failed` shm fallback. A GPU exists here, and a backend that
    /// inherited the default would answer `import_failed` to a client whose
    /// buffer it could perfectly well have scanned out.
    ///
    /// The concrete importer is this call's own local and is lent out as a
    /// bare `&mut dyn DmabufImporter`, never boxed — see
    /// [`session::Presenter::teardown_view`]'s docs for the dropck argument.
    fn scene_and_importer<R>(
        &mut self,
        realm: &RealmId,
        f: impl for<'v> FnOnce(&'v mut Scene, Option<&'v mut dyn DmabufImporter>) -> R,
    ) -> R {
        let mut importer = GlesDmabufImporter {
            renderer: &mut self.output.renderer,
            content: self.dmabuf_content.slot_mut(realm),
        };
        f(self.scenes.scene_mut(realm), Some(&mut importer))
    }
}

// ============================================================================
// The runtime half
// ============================================================================

/// The bare-metal calloop state: the presentation half and the runtime half as
/// two disjoint fields, with the shared `Rc`s beside them.
///
/// The disjointness is not incidental — see [`session::RuntimeHost::split`],
/// which is two field borrows here and nothing cleverer. Everything the
/// consent round and the lock round borrow *together* with the runtime lives
/// as its own field rather than inside `view`, and the `Rc<RefCell<_>>`s are
/// cloned before being borrowed so the `RefMut` holds nothing of `self`.
pub(crate) struct DrmState {
    view: DrmView,
    runtime: Runtime<DrmHook>,
    /// The consent input grab, shared with the router's gate.
    grab: Rc<RefCell<ConsentGrab>>,
    /// The dispatch turn's instant, shared with every hook that judges
    /// against it, so one turn is judged against one sample.
    now: Rc<Cell<Instant>>,
    deadman: Rc<RefCell<DeadManSwitch>>,
    lock: Rc<RefCell<crate::lock::LockScreen>>,
    /// The session's compiled keymap (WS-E.3.1, D-028), or `None` for a
    /// session started with no `--keymap`.
    ///
    /// `None` is a real configuration and a degraded one: without a keymap the
    /// only keys this backend can resolve are
    /// [`crate::input::invariant_keysym`]'s layout-invariant table — Escape,
    /// Enter, arrows, the modifiers, the function keys — so every core chord
    /// still works and **no letter, digit or punctuation reaches the app at
    /// all**. `main` says so loudly at startup and refuses
    /// `--lock-passphrase-file` in that configuration, because a passphrase
    /// nobody can type is a lock nobody can answer.
    keymap: Option<CoreKeymap>,
    /// The seat handle, held for its lifetime and **not** for its verbs.
    ///
    /// `LibSeatSession` owns the seat every device this session opened belongs
    /// to, so dropping it would close them underneath a live compositor. That
    /// is the whole reason it is a field.
    ///
    /// It also carries `Session::change_vt`, and D-030(1) is the decision that
    /// this core never calls it: it does not inhibit a switch, does not
    /// initiate one, and does not switch *back*. A display server that can move
    /// the human between VTs is one that can trap them on its own, which
    /// contradicts the posture the dead-man switch is built on — the human
    /// always has an off-switch, and on bare metal `Ctrl-Alt-F<n>` is it. The
    /// band is scoped to this screen instead (`crate::consent::indicator`).
    #[allow(
        dead_code,
        reason = "held for the seat's lifetime, not for its verbs: dropping it would close \
                  the devices this session opened. D-030(1) refuses `change_vt` outright"
    )]
    seat: LibSeatSession,
    /// The DRM device, held so the seat's pause/activate can reach it.
    ///
    /// Not an optimisation and not a convenience: a session that only flips a
    /// local `active` flag on `PauseSession` never drops DRM master and never
    /// re-acquires it, so after one VT round-trip every `queue_buffer` commits
    /// against cached atomic state the kernel has since discarded. The flip is
    /// rejected, `service_present` keeps the frame owed, no vblank arrives
    /// because no flip landed, and nothing retries — **a permanently dark
    /// panel on a live session**. [`DrmDevice::pause`] and
    /// [`DrmDevice::activate`] are the named handlers for that, and they are
    /// unreachable unless the device is held here.
    device: DrmDevice,
    /// The libinput context, held for [`Libinput::suspend`] /
    /// [`Libinput::resume`] across a VT switch.
    ///
    /// A clone rather than the original: `Libinput` is refcounted
    /// (`libinput_ref`), and [`LibinputInputBackend`] takes the context by
    /// value while exposing only `context(&self) -> &Libinput`, so a `&mut`
    /// for suspend/resume is reachable no other way. Without it the input
    /// nodes stay open across a switch away — the seat revokes the fds
    /// underneath, and libinput is left holding devices it can no longer read.
    libinput: Libinput,
    deadman_chord: &'static str,
    deadman_timer_armed: bool,
    loop_signal: LoopSignal,
    loop_handle: LoopHandle<'static, DrmState>,
    /// Set when a fatal stops the loop, so [`run`] propagates it as an error
    /// rather than masking a mid-run fatal as a clean shutdown. Nothing in the
    /// presentation path writes it — see this module's docs.
    fatal: Option<Box<dyn Error>>,
}

impl session::RuntimeHost for DrmState {
    type Hook = DrmHook;
    type View = DrmView;

    /// Two field borrows and nothing else. Anything cleverer would mean the
    /// state struct had grown an aliasing problem, which belongs fixed in the
    /// struct rather than hidden here.
    fn split(&mut self) -> (&mut Runtime<Self::Hook>, &mut DrmView) {
        (&mut self.runtime, &mut self.view)
    }

    fn loop_handle(&self) -> LoopHandle<'static, Self> {
        self.loop_handle.clone()
    }

    fn stop(&mut self, fatal: Option<Box<dyn Error>>) {
        self.fatal = fatal;
        self.loop_signal.stop();
    }

    /// Drive interactive consent for this round. **Must override the trait's
    /// no-op**, which is headless's posture ("no display to draw a consent
    /// card on and no physical input device to answer it") — this backend is
    /// the display and the keyboard both.
    ///
    /// The `Rc` is cloned first so the `RefMut` borrows nothing of `self`,
    /// leaving `&mut self.runtime` and `&mut self.view.consent` as the two
    /// disjoint field borrows [`session::service_consent_round`] needs.
    ///
    /// **This is the one backend that can answer
    /// [`session::PromptVisibility`] with anything but `Reachable`** (D-030(4)),
    /// and it answers it off `DrmOutput::active` — the *same* field
    /// [`should_queue_flip`] reads, deliberately, so this core has exactly one
    /// notion of "is this screen ours" and a second flag cannot disagree with
    /// it. While the seat holds the devices, nothing composited here reaches a
    /// panel, so a card raised now would be journalled `shown` to a human who
    /// could not see it.
    fn service_consent(&mut self, now: Instant) {
        let visibility = if self.view.output.active {
            session::PromptVisibility::Reachable
        } else {
            session::PromptVisibility::ScreenNotOurs
        };
        let grab = Rc::clone(&self.grab);
        let mut grab = grab.borrow_mut();
        if session::service_consent_round(
            &mut grab,
            &mut self.runtime,
            &mut self.view.consent,
            now,
            visibility,
        ) {
            self.runtime.dirty = true;
            self.view.request_present();
        }
    }

    /// Drive the lock screen for this round, on the same terms and for a
    /// sharper reason: a backend with a physical keyboard is the only one that
    /// can dismiss a lock it raised.
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
            self.view.request_present();
        }
    }
}

impl DeadManHost for DrmState {
    fn switch(&self) -> &Rc<RefCell<DeadManSwitch>> {
        &self.deadman
    }

    fn timer_armed(&mut self) -> &mut bool {
        &mut self.deadman_timer_armed
    }

    fn on_trigger(&mut self, trigger: Trigger) {
        self.runtime.apply_dead_man(&trigger, Instant::now());
    }

    /// "Ask for a frame" on a backend that owns its own frame clock: mark one
    /// owed and queue it if the hardware can take it now.
    ///
    /// The hold indicator's animation is continued from
    /// [`DrmState::on_vblank`] rather than from a frame-budget timer, because
    /// the real vblank is both the correct cadence and a cadence nothing can
    /// spin faster than.
    fn request_redraw(&mut self) {
        self.view.request_present();
    }
}

impl DrmState {
    /// Complete the dead-man chord if due and keep its timer armed —
    /// [`super::winit::deadman_tick`], the one shared implementation, driven
    /// from this backend too.
    fn deadman_tick(&mut self, source: TickSource) {
        let handle = self.loop_handle.clone();
        super::winit::deadman_tick(self, &handle, Instant::now(), source);
    }

    /// A page flip completed: the frame is now on the panel.
    ///
    /// This is the **only** place a realm's owed `frame_done` is discharged on
    /// this backend, which is what makes `Presentation::Scheduled` honest.
    fn on_vblank(&mut self) {
        if let Err(err) = self.view.output.surface.frame_submitted() {
            // The swapchain will run out of buffers if this keeps failing, and
            // the symptom is a session that stops presenting — logged loudly,
            // and still not fatal, for this module's stated reason.
            warn!(%err, "the swapchain refused the completed frame");
        }
        self.view.output.flip_pending = false;
        session::emit_presented(&mut self.runtime);

        // Backstop for the dead-man elapse check, at the panel's cadence, and
        // the continuation of the hold indicator's animation: an armed hold
        // must keep repainting or the human cannot see how far their gesture
        // has got. `InChain` -- this IS the frame clock, so the tick must not
        // also request one (`TickSource`'s whole argument).
        let animating = self.deadman.borrow().deadline().is_some();
        if animating {
            self.deadman_tick(TickSource::InChain);
            self.view.output.wanted = true;
        }
        self.view.service_present();
    }

    /// One turn of physical input from libinput.
    ///
    /// Three arms, and only the third is this backend's own novelty:
    ///
    /// * **Keyboard** resolves through the session's [`CoreKeymap`]
    ///   ([`crate::input::keymap_key`]), not through
    ///   [`crate::input::intake_physical`]'s `Keyboard` arm. That arm resolves
    ///   the layout-*invariant* table only, which maps no letter and no digit
    ///   (`invariant_keysym`'s own test asserts `KEY_A -> None`), so routing
    ///   libinput through it would give a session that cannot type. With no
    ///   keymap configured this falls back to exactly that table, which is the
    ///   degraded configuration `Self::keymap` documents.
    /// * **Pointer motion** is *relative* on libinput, and
    ///   `SeatInputKind::Motion` is absolute view coordinates —
    ///   `intake_physical` drops relative motion outright (its `_ =>
    ///   Vec::new()` arm). So the delta is accumulated here through
    ///   [`accumulate_pointer`] and minted absolute
    ///   ([`crate::input::physical_motion`]). Absolute motion (a touchscreen)
    ///   is resolved through the same [`AbsolutePositionEvent`] methods
    ///   `intake_physical` would use, so both classes land on one stored
    ///   position.
    /// * **Everything else** — buttons, axes, and the classes v0's seat
    ///   vocabulary does not carry — goes through `intake_physical` unchanged,
    ///   which is where the v120 conversion and the drop policy live.
    fn handle_libinput(&mut self, event: &InputEvent<LibinputInputBackend>) {
        let view = self.view.view_size();
        let inputs = match event {
            InputEvent::PointerMotion { event } => {
                let moved = accumulate_pointer(
                    self.view.human_cursor,
                    (event.delta_x(), event.delta_y()),
                    view,
                );
                self.set_human_cursor(moved);
                input::physical_motion(moved.0, moved.1)
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let at = (
                    event.x_transformed(view.0 as i32),
                    event.y_transformed(view.1 as i32),
                );
                let at = accumulate_pointer(at, (0.0, 0.0), view);
                self.set_human_cursor(at);
                input::physical_motion(at.0, at.1)
            }
            InputEvent::Keyboard { event } => {
                let state = match event.state() {
                    HostKeyState::Pressed => SeatKeyState::Pressed,
                    HostKeyState::Released => SeatKeyState::Released,
                };
                let evdev = event
                    .key_code()
                    .raw()
                    .saturating_sub(input::XKB_KEYCODE_OFFSET);
                match self.keymap.as_mut() {
                    Some(keymap) => input::keymap_key(keymap, evdev, state),
                    None => input::physical_key(evdev, None, state),
                }
            }
            InputEvent::DeviceAdded { device } => {
                debug!(device = ?device, "libinput device added");
                Vec::new()
            }
            InputEvent::DeviceRemoved { device } => {
                debug!(device = ?device, "libinput device removed");
                Vec::new()
            }
            other => input::intake_physical(other, (view.0 as i32, view.1 as i32)),
        };
        self.route_physical_inputs(inputs, view);
    }

    /// Record the human's pointer position and, if the drawn sprite would
    /// differ, ask for a frame.
    ///
    /// **Deliberately not `runtime.dirty`.** The dirty flag drives
    /// `post_dispatch`'s capture-cache refresh, which composes *every* live
    /// realm; the human's cursor is human-visible only and changes no capture,
    /// so marking a pointer move dirty would pay up to `MAX_REALMS` full-view
    /// CPU composites for a sprite an agent can never see. Compared on the
    /// quantized hotspot for the reason `set_agent_cursor` is: two positions
    /// inside one view pixel draw the same sprite.
    fn set_human_cursor(&mut self, pos: (f64, f64)) {
        let before = crate::cursor::hotspot(self.view.human_cursor.0, self.view.human_cursor.1);
        self.view.human_cursor = pos;
        if before != crate::cursor::hotspot(pos.0, pos.1) {
            self.view.request_present();
        }
    }

    /// The shared tail of every input turn: prime the grab's view and the
    /// turn's clock, route, drain the dead-man replay, tick the watcher.
    ///
    /// The routing itself is [`session::route_physical_turn`] rather than
    /// anything written here, and that is not tidiness: CI has no display
    /// (D-019(4)), so anything inside a backend method is unreachable by every
    /// test in this workspace — which is how D-018(2)'s fifth ordering rule
    /// once came to have production wiring a reviewer could revert with the
    /// suite staying green.
    fn route_physical_inputs(&mut self, inputs: Vec<input::SeatInput>, view: (u32, u32)) {
        // Fed from the same local the route uses, on the line before it: a
        // drifted hit test can turn a click aimed at Deny into an Allow.
        self.grab.borrow_mut().set_view(view);
        // One clock sample for the whole turn, so the grab, the dead-man
        // watcher and the presence tap judge it against one instant.
        self.now.set(Instant::now());
        let scenes = &self.view.scenes;
        session::route_physical_turn(
            &mut self.runtime,
            scenes,
            Some(&self.deadman),
            inputs,
            view,
            self.now.get(),
        );
        // Backstop 2 of 3 for the elapse check: a physical event is not a
        // frame, so an armed hold needs this to start animating.
        self.deadman_tick(TickSource::OffChain);
    }

    /// The seat took the devices away (a VT switch), or gave them back.
    ///
    /// **The policy this implements is D-030** (WS-E.3.3, issue #219), and the
    /// decisions that are *not* taken here are named there rather than left as
    /// a `warn!` pointing at an unwritten entry:
    ///
    /// * **The core does not prevent the switch.** It cannot — the kernel owns
    ///   `VT_SETMODE` — and a display server that traps the human on its own VT
    ///   contradicts the dead-man switch's whole posture. The answer is scope,
    ///   not prevention: the trusted band asserts only about this screen and
    ///   this process, and `docs/book/src/limits.md` says so in the human's own
    ///   words.
    /// * **A switch away does not raise the lock**, and `LockCause` gains no
    ///   third variant. A lock raised on a seat event would claim a protection
    ///   this core does not have (the other VT is not covered, and D-025 keeps
    ///   every grant live regardless) while charging the human a passphrase for
    ///   using the escape hatch the point above deliberately leaves open. The
    ///   idle timer already covers the case that matters, identically whichever
    ///   way the human left — see D-030(2).
    /// * **The session colour is not re-minted.** Nothing on either arm touches
    ///   [`TrustedIndicator`]: rotation would destroy the stability the whole
    ///   check rests on. D-030(3).
    ///
    /// What the two arms do:
    ///
    /// * **The gesture in progress is cancelled and every press the human is
    ///   holding is paid out** — keys and buttons both — through
    ///   [`session::suspend_physical_seat`], or a hold fires with no gesture
    ///   behind it and a key stays latched down in the confined app
    ///   indefinitely. An *agent's* presses are untouched: the seat leaving is
    ///   not part of an agent's actuation path.
    /// * **No consent prompt is raised while the screen is not ours**, gated in
    ///   [`Self::service_consent`] off the same `active` flag this sets, so the
    ///   flight recorder never says a human was shown a card that reached no
    ///   panel. A petition that arrives during the pause stays pending and the
    ///   ordinary sweep resolves it `timed_out`, which is refusal.
    /// * **A prompt that spanned the pause gets its guard back** on the way in,
    ///   through [`session::resume_physical_seat`] — the same treatment an
    ///   unlock gives a card that was behind the lock cover, and for the same
    ///   reason.
    /// * **The output stops queueing flips** while paused; a frame owed at the
    ///   moment of the switch stays owed and is presented on activation.
    /// * **DRM master is dropped and re-acquired, and the CRTC state is
    ///   reset.** [`DrmDevice::pause`] / [`DrmDevice::activate`] and
    ///   [`DrmSurface::reset_state`] are the devices' own named handlers for
    ///   exactly this, and skipping them is not a missing feature but a defect
    ///   in its purest form: the kernel discards this session's atomic state
    ///   while another VT holds master, so the first post-resume commit is
    ///   rejected, no vblank follows a flip that never landed, and the retry
    ///   chain has no link left. The panel stays dark for the rest of the
    ///   session with nothing but a `warn!` to say why.
    /// * **libinput is suspended and resumed.** The seat revokes the input
    ///   fds underneath a paused session; a context left running holds devices
    ///   it can no longer read.
    fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::PauseSession => {
                info!("the seat paused this session (VT switch away); the panel is not ours");
                self.view.output.active = false;
                // Drop DRM master and stop reading the input nodes, in that
                // order. Both are the devices' OWN named handlers, and a pause
                // that only set the flag above would leave this session
                // holding master over a panel another VT is now driving.
                self.device.pause();
                self.libinput.suspend();
                // Stop the idle clock (D-030(7), Taha's call on 2026-08-09).
                // Physical input is suspended for the whole absence, so
                // `last_activity` cannot advance; without this a session with
                // `--lock-idle` locks itself *during* the switch-away, at an
                // instant no human could observe, and the human returns to a
                // passphrase prompt nobody asked for. A lock already up stays
                // up -- this suppresses the raise, never a lower.
                self.lock.borrow_mut().set_seat_absent(true, self.now.get());
                // The `Rc` is cloned so the `RefCell` borrow inside holds
                // nothing of `self`, leaving `runtime` and `view.scenes` as two
                // disjoint field borrows.
                let deadman = Rc::clone(&self.deadman);
                let released =
                    session::suspend_physical_seat(&mut self.runtime, &self.view.scenes, &deadman);
                // After the drain rather than before it: `forget_hold` inside
                // the call above has already cancelled the gesture, and this
                // is what takes the hold indicator off the (owed) frame.
                self.deadman_tick(TickSource::OffChain);
                info!(
                    released,
                    "session paused: master dropped, input suspended, dead-man hold forgotten, \
                     held presses paid out, presentation suspended, and no consent prompt will \
                     be raised until the seat returns (D-030)"
                );
            }
            SessionEvent::ActivateSession => {
                info!("the seat activated this session; reclaiming the panel");
                // Input first, then master, then the CRTC's own state. Input
                // first because `resume` re-opens the nodes through the seat
                // and a failure there must not leave us lit but deaf; master
                // before `reset_state` because resetting state without it is
                // a permission error, not a no-op.
                if self.libinput.resume().is_err() {
                    // Not fatal and not silent: the panel is usable, the
                    // keyboard is not, and a human staring at a live screen
                    // that ignores them deserves the reason in the log.
                    error!(
                        "libinput could not be resumed after the VT switch back; this session \
                         has the panel but no physical input. Switch away and back to retry"
                    );
                }
                if let Err(err) = self.device.activate(true) {
                    error!(%err, "could not re-acquire DRM master after the VT switch back");
                }
                // The kernel discarded this session's atomic state while
                // another VT held master, so the surface's cached view of the
                // CRTC is stale. Committing against it fails the flip, and a
                // failed flip yields no vblank, so nothing would ever retry.
                if let Err(err) = self.view.output.surface.surface().reset_state() {
                    error!(%err, "could not reset the CRTC state after the VT switch back");
                }
                // The idle countdown restarts from the moment the human comes
                // back, rather than resuming a count frozen mid-way: returning
                // to your own screen is the one thing an idle timer is asking
                // about. Before `active = true`, so no `tick` between the two
                // can see a running clock against a stale `last_activity`.
                self.lock
                    .borrow_mut()
                    .set_seat_absent(false, self.now.get());
                self.view.output.active = true;
                // The flip we had in flight died with DRM master, and its
                // completion event will never arrive: without clearing this
                // the backend would wait for a vblank that cannot come and
                // present nothing again, ever.
                self.view.output.flip_pending = false;
                self.view.output.surface.reset_buffers();
                self.view.request_present();
                // Last, once the screen really is ours again: a card that was
                // up across the pause spent its guard on somebody else's VT,
                // and a press armed on Allow before the switch survives it.
                // Only the guard moves; the petition's deadline does not.
                // `Instant::now()` rather than `self.now`: that cell is the
                // *input turn's* sample and no input turn has run since before
                // the pause, so reusing it would restart the guard to a moment
                // that has already expired -- which is the bug this call exists
                // to close, with an extra step.
                session::resume_physical_seat(&self.grab, Instant::now());
            }
        }
    }
}

/// Whether this instant is one at which a page flip may be queued.
///
/// The whole coalescing rule, as a pure function, because all three ways of
/// getting it wrong are silent:
///
/// * queueing while a flip is in flight returns `EBUSY` from the kernel and
///   drops the frame;
/// * queueing while the session is paused (a VT switch) fails against devices
///   that are not ours;
/// * **clearing the debt in either case** loses the frame permanently — the
///   change is on nobody's screen and nothing will ask again. So this answers
///   "not now", never "not ever": `wanted` is cleared by the caller only after
///   a flip is actually queued.
pub(crate) fn should_queue_flip(wanted: bool, flip_pending: bool, active: bool) -> bool {
    wanted && !flip_pending && active
}

/// Fold a pointer delta into an absolute view position, clamped to the output.
///
/// The whole of this backend's pointer model, and a free function so it can be
/// held by a test with no device: libinput reports *relative* motion, the wire
/// carries *absolute* view coordinates, and the difference is a position
/// somebody has to own. Getting the clamp wrong is not cosmetic — an
/// unclamped position walks off the output and takes the human's only pointer
/// with it, and a position that wraps would deliver clicks somewhere the human
/// is not looking.
///
/// * The result is always inside `[0, w-1] x [0, h-1]`, so it names a real
///   pixel of the view the router maps against.
/// * A non-finite delta (a driver reporting NaN) leaves the position where it
///   was rather than destroying it — `hotspot` would refuse to draw a NaN
///   anyway, and a pointer that vanishes on one bad event is worse than a
///   pointer that does not move on it.
/// * A degenerate view pins the position at the origin.
pub(crate) fn accumulate_pointer(
    pos: (f64, f64),
    delta: (f64, f64),
    view: (u32, u32),
) -> (f64, f64) {
    let axis = |current: f64, delta: f64, extent: u32| {
        let max = f64::from(extent.saturating_sub(1));
        if max <= 0.0 {
            return 0.0;
        }
        let next = current + delta;
        if !next.is_finite() {
            return current.clamp(0.0, max);
        }
        next.clamp(0.0, max)
    };
    (axis(pos.0, delta.0, view.0), axis(pos.1, delta.1, view.1))
}

// ============================================================================
// Startup
// ============================================================================

/// Run the bare-metal compositor loop until SIGINT/SIGTERM.
///
/// The recorder travels through here and comes back for the reason both other
/// backends' `run` do: calloop fixes one state type per loop, the whole kernel
/// lives in that state, and the run's footer must still be written by the code
/// that opened the log. Threading the seed and the recovered recorder through
/// two `Option` slots is what keeps `?` usable for every startup step in
/// [`run_inner`].
#[allow(clippy::too_many_arguments)]
pub fn run(
    dead_man: DeadManConfig,
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
    lock: crate::lock::LockConfig,
    status: crate::status::StatusConfig,
    keymap: Option<PathBuf>,
    seed: RuntimeSeed,
) -> (Recorder, Result<(), Box<dyn Error>>) {
    let mut seed = Some(seed);
    let mut recovered = None;
    let result = run_inner(
        dead_man,
        attention,
        clipboard,
        screenshot,
        lock,
        status,
        keymap.as_deref(),
        &mut seed,
        &mut recovered,
    );
    let recorder = recovered
        .or_else(|| seed.take().map(|seed| seed.recorder))
        .expect("the seed is either still unconsumed or its recorder was recovered");
    (recorder, result)
}

// Nine parameters: this session's five core-owned gestures, the keymap that
// makes the keyboard able to type them, and the two slots `run` threads the
// seed and the recorder back through. Bundling them would hide the one thing
// this signature is good for -- every startup input is named here, once.
#[allow(clippy::too_many_arguments)]
fn run_inner(
    dead_man: DeadManConfig,
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
    lock: crate::lock::LockConfig,
    status: crate::status::StatusConfig,
    keymap: Option<&Path>,
    seed: &mut Option<RuntimeSeed>,
    recovered: &mut Option<Recorder>,
) -> Result<(), Box<dyn Error>> {
    // **The two files this session cannot come up without, first.**
    //
    // The passphrase, for the nested backend's stated reason: a session that
    // came up with an unreadable, malformed or world-readable passphrase file
    // would be a session whose lock cannot be answered.
    //
    // The keymap on the same line and for a reason of the same shape
    // (D-028(2)): `CoreKeymap::from_path` audits the file and every directory
    // above it under `realm.toml`'s rule -- whoever can write the keymap
    // chooses what the trusted core believes the human typed -- and refuses a
    // keymap that does not deliver every core chord trigger, so a layout that
    // would disarm the dead-man switch cannot be loaded at all. Every
    // `KeymapError` is startup-fatal; `?` here is a non-zero exit with the
    // reason named, before the listener accepts anyone.
    let verifier = match lock.passphrase.as_deref() {
        Some(path) => Some(crate::lock::passphrase::PassphraseFile::load(path)?),
        None => None,
    };
    let keymap = match keymap {
        Some(path) => {
            let compiled = CoreKeymap::from_path(path)?;
            info!(path = %path.display(), "keymap compiled: this session can type");
            Some(compiled)
        }
        None => {
            warn!(
                "no --keymap: this session resolves ONLY the layout-invariant scancode table \
                 (Escape, Enter, Tab, Space, the arrows, the function keys, the modifiers). \
                 Every core chord still fires and NO letter, digit or punctuation reaches any \
                 app. Pass --keymap PATH with a compiled xkb keymap to type"
            );
            None
        }
    };

    let mut event_loop: EventLoop<'static, DrmState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // **Signal sources first, and this backend is the one the rule was written
    // for.** The process is still single-threaded here; below this line
    // libseat dispatches its own, libudev runs a monitor thread and the
    // EGL/GBM driver stack spawns whatever it likes. `signalfd` only sees a
    // signal blocked on **every** thread, so installing either of these
    // afterwards misses those -- and for SIGCHLD that means a realm's exit is
    // never noticed, with nothing logged to say so.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    loop_handle.insert_source(signals, |event, _, state: &mut DrmState| {
        info!(signal = ?event.signal(), "shutdown signal received");
        state.loop_signal.stop();
    })?;
    loop_handle.insert_source(
        crate::lifecycle::child_signal_source()?,
        |_event, _, state: &mut DrmState| session::reap_realm(state),
    )?;

    // The seat: this is what lets a non-root process open the card and the
    // input nodes, and what tells it when they are taken away again.
    let (mut seat, seat_notifier) = LibSeatSession::new()?;
    let seat_name = seat.seat();
    info!(seat = %seat_name, "seat acquired");

    // The card node, opened *through* the seat rather than directly, so the
    // descriptor is one logind can revoke on a VT switch.
    let card = smithay::backend::udev::primary_gpu(&seat_name)?.ok_or_else(|| {
        Box::<dyn Error>::from(
            "no primary GPU on this seat: udev reports no DRM device the session may drive",
        )
    })?;
    let card_fd = seat.open(
        &card,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let device_fd = DrmDeviceFd::new(DeviceFd::from(card_fd));
    // `true`: connectors this session is not driving are disabled, so a stale
    // mode left by whatever ran before does not stay lit beside ours.
    let (mut device, drm_notifier) = DrmDevice::new(device_fd.clone(), true)?;
    info!(card = %card.display(), "DRM device opened");

    let (connector_info, mode, crtc) = select_output(&device)?;
    let drm_surface = device.create_surface(crtc, mode, &[connector_info.handle()])?;

    // GBM, EGL and the renderer, in that order: the allocator and the EGL
    // display are both derived from the one GBM device.
    let gbm = GbmDevice::new(device_fd)?;
    // SAFETY: the GBM device outlives the display -- it is moved into
    // `DrmOutput` below and dropped with it -- which is the whole of
    // `EGLDisplay::new`'s contract.
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    // SAFETY: the context is current on this thread and is not shared; the
    // whole core is single-threaded by construction.
    let renderer = unsafe { GlesRenderer::new(egl_context)? };
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let surface = GbmBufferedSurface::new(
        drm_surface,
        allocator,
        SCANOUT_FORMATS,
        renderer.dmabuf_formats(),
    )?;

    let (width, height) = mode.size();
    let mode_size: Size<i32, Physical> = (i32::from(width), i32::from(height)).into();
    info!(
        connector = connector_name(&connector_info),
        width,
        height,
        refresh_hz = mode.vrefresh(),
        "mode set: this session owns the panel"
    );

    // libinput, on the same seat and through the same session interface, so
    // the input nodes are revoked and restored with the card.
    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(seat.clone()));
    libinput.udev_assign_seat(&seat_name).map_err(|()| {
        Box::<dyn Error>::from(format!("cannot assign libinput to seat {seat_name}"))
    })?;
    // Cloned before the backend takes the context by value: `Libinput` is
    // refcounted, and `LibinputInputBackend` exposes only `context(&self)`,
    // so this is the only route to the `&mut` that suspend/resume need.
    let libinput_handle = libinput.clone();
    let libinput_backend = LibinputInputBackend::new(libinput);

    let grab = Rc::new(RefCell::new(ConsentGrab::new()));
    let now = Rc::new(Cell::new(Instant::now()));
    let deadman = Rc::new(RefCell::new(DeadManSwitch::new(dead_man)));
    info!(
        chord = dead_man.chord.name(),
        hold_ms = dead_man.hold.as_millis(),
        "dead-man switch armed: holding this key revokes every grant in the session"
    );
    info!(
        chord = attention.name(),
        window_ms = crate::attention::ATTENTION_WINDOW.as_millis(),
        "attention key armed: tapping it lifts `preempted` for one layout use by a holder \
         of layout authority. It delegates nothing -- every authority exercised afterwards \
         came from a grant a human approved on a consent card"
    );
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
    let screenshot_signal = Rc::new(RefCell::new(
        crate::screenshot::ScreenshotSignal::new(screenshot)
            .expect("the screenshot chord was validated at startup"),
    ));
    info!(
        chord = screenshot_signal.borrow().spelling(),
        "screenshot key armed: it is CONSUMED in every realm. What it writes is the REALM \
         VIEW -- no trusted band, no consent prompt, no lock screen, no status strip, no \
         agent cursor. With no --screenshot-dir the chord is still consumed and writes nothing"
    );
    let lock_screen = Rc::new(RefCell::new(crate::lock::LockScreen::new(
        lock.chord,
        lock.idle,
        verifier,
        Instant::now(),
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

    // The hook stack, in `DrmHook`'s order -- which is `NestedHook`'s order,
    // because it is the same alias and the same decision.
    let router = input::InputRouter::new(
        Rc::new(RefCell::new(input::PhysicalPresenceMap::new())),
        Rc::clone(&now),
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
    );

    let indicator: TrustedIndicator = seed
        .as_ref()
        .expect("the seed is present until the state is built")
        .indicator;
    let view_size = (mode_size.w.max(0) as u32, mode_size.h.max(0) as u32);
    let mut state = DrmState {
        view: DrmView {
            output: DrmOutput {
                _gbm: gbm,
                renderer,
                surface,
                mode: mode_size,
                flip_pending: false,
                // Owed from the first instant: the panel is showing whatever
                // the previous occupant left on it until this session puts a
                // frame there, and `start_realm` below is what will make the
                // frame worth having.
                wanted: true,
                active: true,
                texture: None,
                status_texture: None,
            },
            scenes: RealmScenes::new(view_size),
            consent: ConsentSurface::new(indicator),
            lock: crate::lock::LockSurface::new(indicator),
            indicator,
            status: crate::status::StatusStrip::new(status),
            dmabuf_content: RealmGpuContent::new(),
            agent_cursor: None,
            // The middle of the output: a pointer that started in a corner
            // would leave a human hunting for it on a session that has not
            // moved the mouse yet.
            human_cursor: (f64::from(view_size.0) / 2.0, f64::from(view_size.1) / 2.0),
            attention: false,
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
        keymap,
        seat,
        device,
        libinput: libinput_handle,
        deadman_chord: dead_man.chord.name(),
        deadman_timer_armed: false,
        loop_signal: event_loop.get_signal(),
        loop_handle: event_loop.handle(),
        fatal: None,
    };

    // The three device sources, all on the one loop, because the whole runtime
    // assumes a single-threaded dispatch discipline.
    loop_handle.insert_source(drm_notifier, |event, _metadata, state: &mut DrmState| {
        match event {
            DrmEvent::VBlank(_crtc) => state.on_vblank(),
            // Not fatal: the kernel refused one event read, and the next
            // vblank is still coming. A session ended over this would be an
            // outage caused by a hiccup.
            DrmEvent::Error(err) => warn!(%err, "DRM event error"),
        }
    })?;
    loop_handle.insert_source(libinput_backend, |event, _, state: &mut DrmState| {
        state.handle_libinput(&event);
    })?;
    loop_handle.insert_source(seat_notifier, |event, _, state: &mut DrmState| {
        state.handle_session_event(event);
    })?;

    // **One composite before the loop**, the headless backend's posture and
    // for a sharper reason here: without it the panel keeps showing whatever
    // the previous occupant of this VT left on it until something happens to
    // dirty a frame, and `post_dispatch` returns on its dirty gate before ever
    // reaching `redraw`. The human would be looking at another program's
    // pixels on a display this core has taken over -- with no trusted band on
    // them. This puts vitrin's own output, band included, on screen at
    // startup. It is queued rather than presented: the flip lands at the next
    // vblank like every other.
    state.view.service_present();

    // Every runtime source goes in before the loop starts and before anything
    // spawns a realm: a shim whose socketpair is not being serviced wedges
    // permanently (`session::install`).
    if let Err(err) = session::install(&loop_handle, &mut state.runtime) {
        *recovered = Some(state.runtime.into_recorder());
        return Err(err);
    }

    // The status strip's tick, only for a session that asked for one: the
    // strip's clock has to move on a session where nothing else is happening,
    // and the loop otherwise has no reason to wake.
    if status.enabled {
        if let Err(err) = session::arm_status_tick(&loop_handle) {
            *recovered = Some(state.runtime.into_recorder());
            return Err(err);
        }
    }

    // The realm, only now: `install` has put the listener and the sweeps on
    // the loop, so it is ready to service the shim's socketpair, and
    // `event_loop.run` is the very next statement. Spawning before the reader
    // exists wedges the shim permanently rather than slowly -- it blocks on
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

    *recovered = Some(state.runtime.into_recorder());
    outcome?;
    info!("event loop stopped, shutting down");
    Ok(())
}

/// Enumerate the device's connectors, apply [`pick_sole_connected`], and
/// resolve the chosen one to a mode and a CRTC.
fn select_output(
    device: &DrmDevice,
) -> Result<(connector::Info, DrmMode, crtc::Handle), Box<dyn Error>> {
    let resources = device.resource_handles()?;
    let mut connected: Vec<connector::Info> = Vec::new();
    for handle in resources.connectors() {
        // `false`: do not force a probe. A probe here would block startup on
        // every port of a multi-head machine, and the kernel's cached state is
        // what every other compositor enumerates from.
        let info = device.get_connector(*handle, false)?;
        if info.state() == connector::State::Connected {
            connected.push(info);
        }
    }
    let names: Vec<String> = connected.iter().map(connector_name).collect();
    let chosen = pick_sole_connected(&names)?;
    let info = connected.swap_remove(chosen);

    let mode = preferred_mode(&info).ok_or_else(|| {
        Box::<dyn Error>::from(format!(
            "{} is connected but reports no usable mode; this core cannot invent one",
            connector_name(&info)
        ))
    })?;

    // The encoder the kernel already has on this connector, else any encoder
    // it lists; then any CRTC that encoder can drive.
    let crtc = info
        .current_encoder()
        .into_iter()
        .chain(info.encoders().iter().copied())
        .filter_map(|encoder| device.get_encoder(encoder).ok())
        .find_map(|encoder| {
            resources
                .filter_crtcs(encoder.possible_crtcs())
                .first()
                .copied()
        })
        .ok_or_else(|| {
            Box::<dyn Error>::from(format!(
                "{} has no CRTC this session can drive",
                connector_name(&info)
            ))
        })?;
    Ok((info, mode, crtc))
}

/// **[`session::RuntimeHost::split`] really hands out two borrows that live at
/// the same time**, checked by the compiler rather than by a runtime
/// assertion.
///
/// This function is never called. It does not need to be: it is type-checked
/// in every build, and it stops compiling the moment `split` becomes anything
/// other than two disjoint field borrows — a `RefCell`, a clone, a re-borrow
/// through `&mut self` twice. That is exactly the failure
/// `session::RuntimeHost`'s docs describe ("if it ever needs to be cleverer
/// than that, the state struct has grown an aliasing problem"), and a runtime
/// test could not catch it because such a `split` would simply not build.
///
/// It lives outside `mod tests` so it is checked in release builds too: a
/// `#[cfg(test)]` compile-time assertion is one that a `cargo build` can rot
/// past.
#[allow(
    dead_code,
    reason = "a compile-time assertion about `split`; calling it would prove nothing more"
)]
fn assert_split_borrows_are_disjoint(state: &mut DrmState) {
    let (runtime, view) = session::RuntimeHost::split(state);
    // Both halves held live across each other's use. The `ServerCtx`
    // construction this trait exists for does exactly this, which is why an
    // implementation that returned overlapping borrows would fail there
    // instead — a long way from the backend that caused it.
    let focused = view.focused().cloned();
    runtime.dirty = true;
    if let Some(realm) = focused.as_ref() {
        let _ = view.scene(realm);
    }
    let _ = runtime.dirty;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This module's own source **above `mod tests`**, for the two properties
    /// below that are about *where a call appears* rather than about a value.
    /// The same instrument `crate::input`'s
    /// `no_source_file_resolves_a_keymap_by_name` uses, and for the same
    /// reason: CI has no DRM device, so the only thing a test here can hold
    /// about the frame path is its shape.
    ///
    /// Truncated at the test module deliberately — otherwise every assertion
    /// below would find its own text and pass or fail on itself.
    fn source() -> &'static str {
        include_str!("drm.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("this file ends with its own test module")
            .0
    }

    /// **`redraw` answers `Scheduled`, and the vblank handler is the only
    /// place a realm's `frame_done` is discharged.**
    ///
    /// One test because it is one decision, and the halves are worthless
    /// apart: answering `Scheduled` without ever calling
    /// [`session::emit_presented`] stalls every `frame_done`-paced shim
    /// forever, and calling `emit_presented` from `redraw` (or answering
    /// `Completed`) hands a paced shim a permit per dispatch round with no
    /// presentation between them, so it stops throttling and spins as fast as
    /// the loop will dispatch it. `session::Presentation`'s docs call that out
    /// as silently wrong rather than loudly wrong, which is why it is pinned.
    ///
    /// Checked against the source because the alternative — driving `redraw`
    /// — needs a `GlesRenderer`, a GBM device and DRM master, none of which
    /// exist on a CI runner or may ever exist inside `cargo test`.
    #[test]
    fn redraw_only_schedules_and_the_vblank_handler_alone_reports_presentation() {
        let redraw = source()
            .split("fn redraw(&mut self)")
            .nth(1)
            .expect("this backend implements Presenter::redraw");
        let body = redraw
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;
        assert!(
            body.contains("Presentation::Scheduled"),
            "redraw must answer Scheduled: a composite here queues a page flip, and the flip \
             lands at vblank"
        );
        assert!(
            !body.contains("Presentation::Completed"),
            "redraw answered Completed without having presented; a frame_done-paced shim would \
             stop throttling"
        );
        assert!(
            !body.contains("emit_presented"),
            "frame_done was discharged at queue time rather than at vblank"
        );

        // Exactly one call site, and it is inside `on_vblank`.
        assert_eq!(
            source().matches("session::emit_presented(").count(),
            1,
            "the owed frame callbacks must be discharged from exactly one place"
        );
        let vblank = source()
            .split("fn on_vblank(&mut self)")
            .nth(1)
            .expect("the vblank handler exists");
        assert!(
            vblank
                .split_once("\n    }\n")
                .expect("the method body ends at its closing brace")
                .0
                .contains("session::emit_presented("),
            "the one `emit_presented` call must be the vblank handler's"
        );
    }

    /// **The capture path never reads the scanout buffer back.**
    ///
    /// The sharpest instance of P1.7.1's fork in this crate: the buffer this
    /// backend scans out is the human's whole display, so it holds the trusted
    /// band — this session's *secret* colour — as well as the consent card,
    /// the lock cover, the status strip and both cursor sprites. A capture
    /// served from it would hand an agent the one thing that tells a real
    /// prompt from a forged one.
    ///
    /// Two halves: the served bytes really are `Scene::compose` (the property
    /// `backend::winit`'s
    /// `the_nested_capture_is_the_bare_realm_view_never_the_overlay` pins for
    /// the other CPU-composing backend, mirrored here through the very same
    /// [`super::winit::capture_pixels`]), and no readback machinery appears in
    /// this file at all.
    #[test]
    fn the_bare_metal_capture_is_the_bare_scene_and_never_a_scanout_readback() {
        use crate::scene::{tests::client_pixels, SurfaceContent};

        const W: i32 = 800;
        const H: i32 = 600;
        let size: Size<i32, Physical> = (W, H).into();
        let mut scene = Scene::new();
        scene
            .commit(SurfaceContent::from_rgba(client_pixels(300, 200), 300, 200).expect("content"));

        // What `view_rgba` serves, from the function it serves it through.
        let capture =
            super::super::winit::capture_pixels(&scene, size).expect("a nonzero view has pixels");
        assert_eq!(
            capture,
            scene.compose(W as u32, H as u32),
            "the bare-metal capture must be the bare Scene::compose realm view"
        );
        // A degenerate mode has no realm view to serve, so the capture meets
        // the chokepoint's `no_surface` refusal rather than serving black.
        assert!(super::super::winit::capture_pixels(&scene, (0, 0).into()).is_none());

        // ...and there is no readback here to be tempted by. `copy_framebuffer`
        // / `ExportMem` are how a GLES target is read back into CPU memory;
        // neither may ever appear in this backend.
        // Call syntax, not the bare word: the module docs *name*
        // `copy_framebuffer` while explaining why this backend does not use
        // it, and a test that failed on its own explanation would teach the
        // next reader to delete the explanation.
        for forbidden in ["copy_framebuffer(", "map_texture(", "ExportMem"] {
            assert!(
                !source().contains(forbidden),
                "`{forbidden}` in the bare-metal backend: the only image it holds is the \
                 scanout buffer, and reading that back would put the trusted band into every \
                 vitrin_view.frame_ready"
            );
        }
        // The capture must not reach for the OUTPUT's scene either -- the
        // deliberately differently-named `RealmScenes::bound` accessor, whose
        // whole purpose is to make that mistake legible (WS-E.1.3 decision 1).
        let view_rgba = source()
            .split("fn view_rgba(&mut self")
            .nth(1)
            .expect("this backend implements Presenter::view_rgba");
        let body = view_rgba
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;
        assert!(
            body.contains("scenes.scene(realm)") && !body.contains("bound()"),
            "a capture must serve the scene the GRANT names, never the one the output shows"
        );
        // ...and it must serve that scene BARE. The check above catches
        // serving the wrong scene; it does not catch serving the right one
        // with something added, which is the direction that leaks. Every
        // entry point below draws a human-visible element -- the trusted
        // band, the consent card, the lock cover, the status strip, either
        // cursor -- and any of them appearing in this body would put a
        // trusted pixel into `vitrin_view.frame_ready`.
        //
        // Found by review, not by this test: an overlay composited onto every
        // capture here left the whole suite green, because the assertion above
        // was satisfied the entire time.
        for forbidden in [
            "compose_human_visible",
            "human_visible_frame",
            "window_pixels",
            "crosshair_rects",
            "Draw::",
            "TrustBand",
            "human_cursor",
            "agent_cursor",
            "status",
            "consent",
            "lock",
        ] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` inside `view_rgba`: an agent's capture is the BARE realm view, \
                 and every one of these draws something only the human may see"
            );
        }
    }

    /// **Neither bare-metal branch can present unbanded pixels.**
    ///
    /// [`super::compose_human_visible`]'s module docs say a third presentation
    /// path added by someone who never read that paragraph must not be able to
    /// drop the trusted band. They say it in prose. This is the part that
    /// holds.
    ///
    /// Both branches of [`DrmView::compose_and_queue`] reach the band through
    /// exactly one call each: the zero-copy branch through
    /// `present_human_visible`, whose final draw slot is unconditionally
    /// `Draw::TrustBand`, and the CPU branch through `window_pixels` ->
    /// `compose_human_visible`. Swapping either for a bare `Scene::compose` is
    /// the precise defect `backend/mod.rs` warns about — a presentation path
    /// made entirely of pixels the confined client owns.
    ///
    /// **This test exists because that mutation was performed and passed.**
    /// Replacing the CPU branch's `window_pixels` with a raw scene compose
    /// left all 853 tests green; the nested backend's equivalent guard,
    /// `no_presentation_path_can_drop_the_trusted_band`, is scoped to the
    /// nested backend by its own first sentence and never reads this file.
    #[test]
    fn no_bare_metal_branch_can_present_unbanded_pixels() {
        let after = source()
            .split("fn compose_and_queue(&mut self")
            .nth(1)
            .expect("this backend composes through compose_and_queue");
        let body = after
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;

        for required in ["present_human_visible(", "window_pixels("] {
            assert!(
                body.contains(required),
                "`compose_and_queue` no longer routes a branch through `{required}`: the trusted \
                 band, the consent card and the lock cover all live behind those two calls, and \
                 a branch that skips them presents the confined client's own pixels as the \
                 human's whole screen"
            );
        }
        // The inverse, because the two assertions above stay satisfied if a
        // THIRD branch is added beside them. Nothing in this method may
        // compose a scene directly.
        for forbidden in [".compose(", "Scene::compose", "capture_pixels("] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` inside `compose_and_queue`: that is the realm view, which is what \
                 an agent may see and NOT what the human's panel shows -- the human's screen is \
                 the composited output, band included"
            );
        }
    }

    /// **The bare-metal backend really does ask for the human's cursor.**
    ///
    /// `protocol/vitrin-v0.xml` used to state flatly that no human cursor is
    /// composited at all, "in nested operation the host desktop draws it
    /// outside the realm view entirely". On bare metal there is no host
    /// desktop, so that sentence was rewritten to be nested-conditional — a
    /// paired IDL + prose edit made **for this backend and no other** (D-029
    /// decision 2).
    ///
    /// Nothing pinned the other half of that bargain. Deleting the human
    /// cursor from both draw calls left every test green, which would leave
    /// the protocol describing a behaviour its only implementation had
    /// dropped, and the human on a bare-metal session with no pointer at all.
    /// Both branches are checked: a sprite drawn on one path and not the other
    /// appears and disappears as the zero-copy branch engages.
    #[test]
    fn the_bare_metal_backend_composites_the_humans_cursor() {
        let after = source()
            .split("fn compose_and_queue(&mut self")
            .nth(1)
            .expect("this backend composes through compose_and_queue");
        let body = after
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;

        let zero_copy = body
            .split_once("present_human_visible(")
            .expect("the zero-copy branch draws through present_human_visible")
            .1;
        let zero_copy_args = zero_copy
            .split_once(")?")
            .expect("the call's arguments end at its closing paren")
            .0;
        assert!(
            zero_copy_args.contains("human_cursor"),
            "the zero-copy branch stopped passing the human cursor: on bare metal the core is \
             the only thing that can draw a pointer, and the IDL was amended to say so"
        );

        let cpu = body
            .split_once("window_pixels(")
            .expect("the CPU branch draws through window_pixels")
            .1;
        let cpu_args = cpu
            .split_once(");")
            .expect("the call's arguments end at its closing paren")
            .0;
        assert!(
            cpu_args.contains("human_cursor"),
            "the CPU branch stopped passing the human cursor, so the pointer would vanish \
             whenever the zero-copy path is unavailable -- an overlay up, or an unimportable \
             client buffer"
        );
    }

    /// **The seat handler carries D-030's policy, on both arms** (WS-E.3.3).
    ///
    /// `handle_session_event` is a `&mut DrmState` method and `DrmState` cannot
    /// be constructed without a real `DrmDevice`, `LibSeatSession`, `GbmDevice`
    /// and `GlesRenderer`, so no test in this workspace can call it. The
    /// *behaviour* therefore lives in two free functions with their own
    /// behavioural tests one crate module over
    /// (`session::tests::a_paused_seat_pays_out_held_presses_and_forgets_the_dead_man_hold`,
    /// `consent::grab::tests::a_prompt_that_spanned_a_seat_pause_gets_a_fresh_guard_and_loses_its_armed_press`),
    /// and what is left for this file is the **wiring** — which arm calls
    /// which, and what neither arm may do. Deleting either call compiles
    /// cleanly and takes nothing else red, which is precisely why it is pinned
    /// here.
    ///
    /// The three negatives are decisions, not hygiene:
    ///
    /// * **Neither arm may touch the trusted indicator** (D-030(3)). Rotating
    ///   the session colour on resume reads as hygiene and destroys the
    ///   stability the whole check rests on. The crate-wide guard is
    ///   `consent::indicator`'s `the_session_colour_is_minted_once_and_nothing_re_mints_it`;
    ///   this is the same rule at the one site most likely to break it.
    /// * **Neither arm may raise the lock** (D-030(2)). A lock raised on a seat
    ///   event would claim a protection this core does not have while charging
    ///   the human a passphrase for using the escape hatch D-030(1) leaves open
    ///   on purpose.
    /// * **Neither arm may change the VT** (D-030(1)). `LibSeatSession` carries
    ///   `change_vt`; a display server that can move the human between VTs is
    ///   one that can trap them on its own.
    #[test]
    fn the_seat_handler_drains_on_pause_restores_the_guard_on_resume_and_rotates_nothing() {
        let after = source()
            .split("fn handle_session_event(&mut self, event: SessionEvent)")
            .nth(1)
            .expect("this backend handles the seat's pause/activate events");
        let body = after
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;
        let (pause, activate) = body
            .split_once("SessionEvent::ActivateSession")
            .expect("the handler has both arms");

        assert!(
            pause.contains("session::suspend_physical_seat("),
            "the pause arm stopped draining the seat: a dead-man hold survives with no gesture \
             behind it, a key stays latched down in the confined app, and a held button wedges \
             its implicit pointer grab -- for the whole time the human is on another VT"
        );
        assert!(
            activate.contains("session::resume_physical_seat("),
            "the activate arm stopped restarting the consent guard: a card raised before the \
             switch spent its guard on somebody else's VT, and the first press on return \
             commits it"
        );
        // The gate that keeps `Shown` honest, read off the SAME field
        // `should_queue_flip` reads -- one notion of "is this screen ours",
        // never two that can disagree.
        let consent = source()
            .split("fn service_consent(&mut self, now: Instant)")
            .nth(1)
            .expect("this backend overrides the consent round");
        let consent_body = consent
            .split_once("\n    }\n")
            .expect("the method body ends at its closing brace")
            .0;
        assert!(
            consent_body.contains("self.view.output.active")
                && consent_body.contains("PromptVisibility::ScreenNotOurs"),
            "the consent round stopped asking whether this screen is ours: a petition arriving \
             during a VT switch would be journalled `consent_transition{{shown}}` and told to \
             the petitioner as `shown`, for a card that reaches no panel"
        );

        for (forbidden, why) in [
            (
                "TrustedIndicator",
                "rotating the session colour on a seat event destroys the stability the \
                 human's whole check rests on (D-030(3))",
            ),
            (
                "LockCause",
                "a VT switch does not raise the lock: it would claim a protection this core \
                 does not have, and charge the human a passphrase for the escape hatch \
                 D-030(1) deliberately leaves open",
            ),
            (
                "change_vt",
                "this core never moves the human between VTs; one that can is one that can \
                 trap them on its own (D-030(1))",
            ),
        ] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` in the seat handler: {why}"
            );
        }
    }

    /// **A frame that cannot be queued now is owed, not lost.**
    ///
    /// Every one of these three is a silent failure if the gate gets it wrong:
    /// a session that composites nothing, or one that drops the frame carrying
    /// a consent card, with every test in this workspace still green.
    #[test]
    fn a_frame_that_cannot_be_flipped_now_stays_owed() {
        // The ordinary case.
        assert!(should_queue_flip(true, false, true));
        // Nothing asked for a frame: an idle session queues nothing at all,
        // which is the whole reason `redraw` and `request_present` set the
        // flag rather than compositing unconditionally.
        assert!(!should_queue_flip(false, false, true));
        // A flip is already in flight -- a second `drmModePageFlip` before the
        // first completes returns EBUSY -- and a paused session's devices are
        // not ours. In both cases the answer is "not now": the caller leaves
        // `wanted` set, and the vblank handler or the activation retries.
        assert!(!should_queue_flip(true, true, true));
        assert!(!should_queue_flip(true, false, false));
        assert!(!should_queue_flip(true, true, false));
    }

    /// **The one-output refusal, both directions.**
    ///
    /// The decision this pins is the *contract* argument in
    /// [`pick_sole_connected`]'s docs, not the stale content argument issue
    /// #218 gave for it: coming up on two panels would light one and leave a
    /// powered display dark with no verb able to move the output to it.
    #[test]
    fn a_session_starts_on_exactly_one_connected_display() {
        assert_eq!(pick_sole_connected(&[String::from("eDP-1")]), Ok(0));

        assert_eq!(
            pick_sole_connected(&[]),
            Err(OutputRefusal::NoConnectedConnector)
        );
        let two = vec![String::from("eDP-1"), String::from("DP-3")];
        assert_eq!(
            pick_sole_connected(&two),
            Err(OutputRefusal::SeveralConnectedConnectors(two.clone()))
        );

        // The message names what the operator can act on -- both displays --
        // rather than only that there were too many.
        let refusal = pick_sole_connected(&two).expect_err("two displays are refused");
        let message = refusal.to_string();
        assert!(
            message.contains("eDP-1") && message.contains("DP-3"),
            "{message}"
        );
        // ...and the zero case names the alternative that does work.
        assert!(
            pick_sole_connected(&[])
                .expect_err("no display is refused")
                .to_string()
                .contains("--headless"),
            "the empty-connector refusal must name the mode that does not need a display"
        );
    }

    /// **The pointer stays on the output, always.**
    ///
    /// libinput reports relative motion, so the accumulated position is the
    /// only thing that keeps a human's pointer inside the view the router maps
    /// against and inside the frame the sprite is drawn into. A drifted or
    /// wrapped position would deliver clicks somewhere the human is not
    /// looking.
    #[test]
    fn the_human_pointer_accumulates_and_never_leaves_the_output() {
        let view = (1920, 1080);
        // Ordinary motion accumulates.
        assert_eq!(
            accumulate_pointer((100.0, 100.0), (10.5, -20.0), view),
            (110.5, 80.0)
        );
        // Both edges clamp, and to a real pixel of the view rather than to
        // its exclusive bound.
        assert_eq!(
            accumulate_pointer((0.0, 0.0), (-50.0, -50.0), view),
            (0.0, 0.0)
        );
        assert_eq!(
            accumulate_pointer((1900.0, 1070.0), (1e6, 1e6), view),
            (1919.0, 1079.0)
        );
        // A driver reporting a non-finite delta leaves the pointer where it
        // was; it does not destroy it.
        let kept = accumulate_pointer((640.0, 480.0), (f64::NAN, 3.0), view);
        assert_eq!(kept.0, 640.0);
        assert_eq!(kept.1, 483.0);
        assert!(
            accumulate_pointer((640.0, 480.0), (f64::INFINITY, 0.0), view)
                .0
                .is_finite()
        );
        // A degenerate output has one legal position.
        assert_eq!(
            accumulate_pointer((5.0, 5.0), (1.0, 1.0), (0, 0)),
            (0.0, 0.0)
        );
        assert_eq!(
            accumulate_pointer((5.0, 5.0), (1.0, 1.0), (1, 1)),
            (0.0, 0.0)
        );
    }
}
