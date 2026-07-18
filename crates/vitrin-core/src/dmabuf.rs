//! The dmabuf import path (P1.3.5, issue #22): zero-copy shim→core frames
//! on real GPUs — the M1.5 optimization over the universal shm copy-in
//! (plan D3; PRD Doc 2 §3.4/§4.4 "dmabuf fd passed to core → core
//! composites... zero extra copies").
//!
//! This module owns every GPU/EGL mechanic of the path; the shim protocol
//! server ([`crate::shim`]) stays the **single policy site**: it delegates a
//! `kind=dmabuf` commit through the [`DmabufImporter`] seam and maps the
//! typed outcome onto the wire's dispositions (`buffer_done` statuses or the
//! `invalid_buffer` log-and-close), exactly where it already resolves the
//! shm path. No second authority or misbehavior funnel is introduced.
//!
//! The shm path remains the universal fallback: CI runs entirely on it (a
//! headless embedder passes no importer and every dmabuf commit resolves as
//! the designed `buffer_done(import_failed)` fallback event — never a silent
//! black frame), and MVP success does not depend on zero-copy working (plan
//! risk R3).
//!
//! # The D3 allowlist, enforced before any driver call
//!
//! Version 0 imports exactly xrgb8888/argb8888 with the **linear modifier
//! implied** — the IDL deliberately has no modifier argument. Support is
//! decided by [`DmabufImporter::supports`], a pure lookup against the
//! renderer's format table (for GLES: the `EGL_EXT_image_dma_buf_import`
//! format/modifier set Smithay queries **once** at display init and caches).
//! An allowlist miss is an immediate `format_unsupported` fallback event; no
//! driver entry point runs and no syscall touches the fd.
//!
//! # Hostile-fd posture (the issue-#21 precedent, extended)
//!
//! The shim is outside the TCB, so an alleged-dmabuf fd is hostile until
//! proven otherwise, and the single-threaded core loop must never make a
//! blockable syscall on an unvalidated fd. `fstat`, `lseek(SEEK_END)` and
//! `ioctl` are all forwarded to the backing filesystem for FUSE-backed
//! files, so none of them is safe here — and some EGL stacks `lseek` the fd
//! during import to learn its size, so the fd cannot simply be handed to
//! the driver either. The one probe that is safe is the same *class* as
//! shm's `F_GET_SEALS`: reading `/proc/self/fdinfo/<fd>` ([`probe_dmabuf_fd`]).
//! procfs renders fdinfo entirely in-kernel — the generic fields come from
//! the in-memory `struct file`, and the optional `show_fdinfo` hook is
//! implemented only by in-kernel drivers (FUSE has none), so no
//! shim-controlled filesystem code can run or block. A genuine dmabuf's
//! fdinfo carries the dmabuf-specific `exp_name:` and `size:` keys
//! (`dma_buf_show_fdinfo`, documented in `Documentation/filesystems/proc.rst`),
//! which yields both the authenticity check and the buffer's actual size —
//! no syscall on the fd at all. The probe runs inside the GLES importer,
//! immediately before the first driver call, because validation belongs to
//! the path that touches the fd: a core with no importer never touches it
//! (and so cannot observe a lie — it just answers the designed fallback).
//!
//! Probe dispositions: a readable fdinfo without the dmabuf keys, or a
//! `size:` below the declared geometry's footprint, is a lie about the fd —
//! [`ImportDenied::FdLie`], which the shim server maps to the IDL's
//! `invalid_buffer` log-and-close ("geometry inconsistent with the fd's
//! actual size"), the same razor the shm arm applies. An *unreadable*
//! fdinfo (no procfs) means the core cannot validate: it must not touch the
//! fd, but an honest shim is indistinguishable from a hostile one, so the
//! disposition is the recoverable `import_failed` fallback, not a kill.
//!
//! # Import-failure detection (plan risk R3: driver reality)
//!
//! Drivers disagree about *when* a doomed import fails: some reject at
//! `eglCreateImageKHR` (`EGL_BAD_MATCH` and friends), others accept the
//! EGLImage and only fail at `glEGLImageTargetTexture2DOES`, and some
//! surface nothing until the texture is first *sampled*. A failure at that
//! last moment is the black-frame trap: `buffer_done` would already have
//! said `released`, and there is no un-saying it. So this module collapses
//! every failure time into **one detection point, before `buffer_done` is
//! decided**: [`GlesDmabufImporter::import`] performs the EGL import *and*
//! a synchronous 1×1 probe composite that samples the imported texture into
//! an offscreen target and reads four bytes back (the wlroots/Mutter
//! test-render pattern). EGL-time rejection and first-render failure both
//! land in [`ImportError`] (stage-tagged for the log) and both resolve as
//! the same `buffer_done(import_failed)` fallback event. Residual risk,
//! documented: a GPU fault that only a robustness extension could report
//! (e.g. a device reset while sampling a wedged buffer) is out of scope for
//! version 0.
//!
//! # Release semantics on the zero-copy path (fd lifetime)
//!
//! Under shm copy-in, `buffer_done(released)` is prompt: the copy is done,
//! the buffer is the shim's again. Under zero-copy there is no copy to be
//! done *with* — the core samples the client's buffer at every composite
//! until something replaces it. The honest reading of the IDL's "status
//! released means the core (and GPU) are done with the buffer ... at
//! GPU-done under dmabuf passthrough" is therefore: **the core holds a
//! successfully imported buffer until the next successful commit replaces
//! it** (or the surface dies with the connection, where no event exists to
//! send). A zero-copy shim consequently needs at least two buffers in
//! flight — exactly the discipline every Wayland client already practices
//! against a compositor that holds the presented buffer.
//!
//! GPU-done is proven, not assumed: GL commands in one context execute in
//! order, and both replacement paths end in a synchronous readback — the
//! probe composite of the *replacing* import, or [`DmabufImporter::clear`]'s
//! explicit sync when CPU (shm) content takes over — so by the time the
//! deferred `released` is sent, every command that could still have sampled
//! the old buffer has provably completed. The dmabuf *fd* itself lives
//! inside the retained import and closes when that import is dropped — "as
//! it emits this event", within the IDL's fd-ownership contract.
//!
//! One ordering consequence is deliberate: `buffer_done` events stay in
//! attach order along the release chain (buffer N's `released` always
//! precedes buffer N+1's), but a *failure* disposition is answered promptly
//! even while an earlier buffer is still retained — the previously
//! committed content stays on screen, so its `released` cannot precede the
//! newcomer's failure. A strict total attach-order reading would deadlock
//! the shim's fallback loop (it may wait for the failure verdict before
//! rendering the frame whose commit would free the retained buffer), so
//! prompt-failure is the only coherent reading of the IDL's two timing
//! regimes. Flagged for the protocol track to sharpen in prose.
//!
//! # Zero-copy instrumentation
//!
//! [`CopyMeter`] counts every core-side CPU copy of client pixels — the
//! shm `copy_in` is the only such site, and it records exactly one copy per
//! shm commit. The CI tests assert the inversion (one copy per shm commit);
//! the env-gated real-GPU test asserts the zero (dmabuf commits leave the
//! meter untouched while the composited output provably shows the client's
//! pixels). The 4-byte probe readback and any test-side full-frame readback
//! are not client-pixel copies and are deliberately outside the meter.
//!
//! # Runtime capability, not compile-time
//!
//! Everything here builds in every configuration — the GLES types come with
//! the workspace's existing `backend_winit`/`renderer_gl` Smithay features —
//! but a GPU is only *used* when the embedder passes a live importer.
//! Headless/CI embedders pass `None` and never touch EGL. Nothing at
//! runtime constructs an importer yet: like the shim server itself, this
//! module ships the machinery, and the realm/backend wiring (P1.5.2) will
//! hand the nested backend's `GlesRenderer` to a [`GlesDmabufImporter`]
//! when it wires shim connections at all. The env-gated tests below play
//! that embedder today. Only the *tests* need a buffer allocator (GBM),
//! which is why the `gpu-tests` cargo feature exists; see `Cargo.toml`.

use std::fmt;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
use smithay::backend::allocator::{Format as DrmFormat, Fourcc, Modifier};
use smithay::backend::renderer::gles::{GlesError, GlesRenderbuffer, GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Frame, ImportDma, Offscreen, Renderer, Texture};
use smithay::utils::{Physical, Rectangle, Size, Transform};
use vitrin_protocol::generated::vitrin_view::Format;

use crate::scene::{layout, BYTES_PER_PIXEL, LETTERBOX_RGBA};

/// Counter of core-side CPU copies of client pixels — the zero-copy
/// instrumentation. Bumped by the shim server at its one copy site (the shm
/// `copy_in`); never bumped on the dmabuf path, which is the whole claim.
/// Per-[`ShimServer`](crate::shim::ShimServer), so tests assert exact deltas
/// with no global state.
#[derive(Debug, Default)]
pub(crate) struct CopyMeter {
    copies: u64,
    pixel_bytes: u64,
}

impl CopyMeter {
    /// Record one client-pixel copy of `bytes` bytes.
    pub fn record(&mut self, bytes: usize) {
        self.copies += 1;
        self.pixel_bytes += bytes as u64;
    }

    /// Number of client-pixel copies performed so far.
    pub fn copies(&self) -> u64 {
        self.copies
    }

    /// Total client-pixel bytes copied so far.
    pub fn pixel_bytes(&self) -> u64 {
        self.pixel_bytes
    }
}

/// The wire-validated geometry of one `kind=dmabuf` attach, as the shim
/// server hands it to an importer: single-plane, offset 0, linear modifier
/// implied (the version-0 IDL carries no modifier or offset argument).
/// Dimensions have already passed the shim server's zero/stride checks and
/// its renderer dimension limit
/// ([`MAX_SURFACE_DIM`](crate::shim::MAX_SURFACE_DIM), so they always fit
/// `i32`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DmabufSpec {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// Which failure time a doomed import died at — kept apart so the core log
/// records driver reality (risk R3's per-driver notes feed on this), even
/// though both stages resolve identically on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportStage {
    /// Rejected by the EGL/GL import entry points (`eglCreateImageKHR` /
    /// `glEGLImageTargetTexture2DOES`).
    Egl,
    /// Import call accepted, but the synchronous probe composite — the
    /// first render — failed.
    FirstRender,
}

impl fmt::Display for ImportStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportStage::Egl => write!(f, "egl-import"),
            ImportStage::FirstRender => write!(f, "first-render"),
        }
    }
}

/// A recoverable import failure: the buffer was not used, previously
/// committed content stays on screen, and the shim is directed to fall back
/// to shm (`buffer_done(import_failed)`).
#[derive(Debug)]
pub(crate) struct ImportError {
    pub stage: ImportStage,
    pub detail: String,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dmabuf import failed at {}: {}", self.stage, self.detail)
    }
}

/// Why an importer refused to latch a buffer. The shim server maps the two
/// variants onto the two existing misbehavior funnels — recoverable
/// fallback event versus `invalid_buffer` log-and-close — and nothing else;
/// this type exists so the mapping stays at that one policy site.
#[derive(Debug)]
pub(crate) enum ImportDenied {
    /// The fd is a lie: not a dmabuf at all, or its actual size cannot hold
    /// the declared geometry. The connection-fatal condition (the IDL's
    /// `invalid_buffer` razor) — a correct shim can always avoid it.
    FdLie(String),
    /// A genuine import failure (either stage): the recoverable
    /// `import_failed` fallback.
    Refusal(ImportError),
}

/// The importer seam between the shim protocol server and a GPU renderer.
///
/// Exactly one importer serves a shim connection for its lifetime (it holds
/// the retained zero-copy buffer between commits); the embedder passes the
/// same one to every `handle_message`/`connection_closed` call, or `None`
/// forever on GPU-less paths.
pub(crate) trait DmabufImporter {
    /// The D3 allowlist check: is `format` (with the implied linear
    /// modifier) importable? MUST be a pure table lookup — never a driver
    /// call, never a syscall on any fd — because the shim server consults
    /// it before anything touches the attach's fd.
    fn supports(&self, format: Format) -> bool;

    /// Validate the fd, import it, and prove it renderable; on success the
    /// import becomes the retained current GPU content, replacing (and
    /// synchronously retiring, per the module docs' release semantics) any
    /// previous one. Consumes the fd in every outcome.
    fn import(&mut self, spec: &DmabufSpec, fd: OwnedFd) -> Result<(), ImportDenied>;

    /// Drop the retained GPU content because CPU (shm) content replaced it
    /// or the surface is gone, syncing the GPU first so the underlying
    /// client buffer is provably no longer being sampled. Idempotent.
    fn clear(&mut self);
}

/// Why [`probe_dmabuf_fd`] could not vouch for an fd.
#[derive(Debug)]
pub(crate) enum FdProbeError {
    /// fdinfo was readable and lacks the dmabuf keys: the fd is not a
    /// dmabuf. (Maps to the `invalid_buffer` kill.)
    NotADmabuf,
    /// fdinfo itself was unreadable (procfs absent/restricted): validation
    /// is impossible, the fd must not be touched. (Maps to the recoverable
    /// `import_failed` fallback.)
    Unavailable(io::Error),
}

/// The non-blocking dmabuf authenticity + size probe (module docs): read
/// `/proc/self/fdinfo/<fd>` and require the dmabuf-specific `exp_name:` and
/// `size:` keys. Returns the buffer's actual size in bytes. Performs **no
/// syscall on the fd itself** — the one operation guaranteed safe against a
/// hostile fd whose own filesystem could block the core loop.
pub(crate) fn probe_dmabuf_fd(fd: BorrowedFd<'_>) -> Result<u64, FdProbeError> {
    let path = format!("/proc/self/fdinfo/{}", fd.as_raw_fd());
    let text = std::fs::read_to_string(path).map_err(FdProbeError::Unavailable)?;
    parse_dmabuf_fdinfo(&text).ok_or(FdProbeError::NotADmabuf)
}

/// Parse a procfs fdinfo blob: `Some(size)` iff both dmabuf keys are
/// present (`exp_name:` proves the fd kind; `size:` is the buffer's byte
/// size). Split out for direct unit testing against captured fixtures.
fn parse_dmabuf_fdinfo(text: &str) -> Option<u64> {
    let mut size = None;
    let mut exp_name = false;
    for line in text.lines() {
        if line.strip_prefix("exp_name:").is_some() {
            exp_name = true;
        } else if let Some(rest) = line.strip_prefix("size:") {
            size = rest.trim().parse::<u64>().ok();
        }
    }
    if exp_name {
        size
    } else {
        None
    }
}

/// The version-0 wire format → DRM fourcc mapping, restated defensively:
/// the wire enum's values *are* fourcc codes, but the importer must never
/// forward a value it does not positively recognize to a driver, so a
/// format appended to the enum in a later version fails closed here
/// (`None` → `format_unsupported`) until this path deliberately adopts it.
fn wire_fourcc(format: Format) -> Option<Fourcc> {
    match format {
        Format::Xrgb8888 => Some(Fourcc::Xrgb8888),
        Format::Argb8888 => Some(Fourcc::Argb8888),
    }
}

/// A successfully imported, currently retained zero-copy surface: the GLES
/// texture sampling the client's dmabuf, plus its pixel size for placement.
/// Owning this keeps the underlying `Dmabuf` (and its fd) alive via the
/// texture's EGLImage.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
pub(crate) struct GpuContent {
    texture: GlesTexture,
    width: u32,
    height: u32,
}

/// The real importer: wraps the embedder's `GlesRenderer` (the nested
/// backend's, once P1.5.2 wires shim connections at runtime; the env-gated
/// tests' today) and the embedder-owned retained-content slot. Constructed
/// fresh per dispatch — it borrows, it does not own.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
pub(crate) struct GlesDmabufImporter<'a> {
    pub renderer: &'a mut GlesRenderer,
    pub content: &'a mut Option<GpuContent>,
}

#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
impl DmabufImporter for GlesDmabufImporter<'_> {
    fn supports(&self, format: Format) -> bool {
        // Pure table lookup: Smithay caches the EGL dmabuf format/modifier
        // set at display init; no driver call happens here. Linear only —
        // the version-0 IDL implies it and nothing else is negotiable.
        wire_fourcc(format).is_some_and(|code| {
            self.renderer.has_dmabuf_format(DrmFormat {
                code,
                modifier: Modifier::Linear,
            })
        })
    }

    fn import(&mut self, spec: &DmabufSpec, fd: OwnedFd) -> Result<(), ImportDenied> {
        // 1. Authenticity + size, via the only safe probe (module docs) —
        //    before the driver can lseek/ioctl a hostile fd.
        let actual = match probe_dmabuf_fd(fd.as_fd()) {
            Ok(size) => size,
            Err(FdProbeError::NotADmabuf) => {
                return Err(ImportDenied::FdLie(
                    "kind=dmabuf fd is not a dmabuf (fdinfo probe: no exporter key)".into(),
                ));
            }
            Err(FdProbeError::Unavailable(e)) => {
                return Err(ImportDenied::Refusal(ImportError {
                    stage: ImportStage::Egl,
                    detail: format!(
                        "cannot validate the fd (fdinfo unreadable: {e}); \
                                     refusing to hand an unvalidated fd to the driver"
                    ),
                }));
            }
        };
        // Minimal footprint of the sampled region in 128-bit math (the
        // scene/shim overflow precedent): the last row needs only
        // `width * 4` bytes, so this never rejects an honestly allocated
        // linear buffer, while a shorter fd is the invalid_buffer lie.
        // (`height >= 1` was validated at attach; saturating keeps even a
        // violated invariant panic-free in the TCB.)
        let min = u128::from(spec.stride) * u128::from(spec.height).saturating_sub(1)
            + u128::from(spec.width) * BYTES_PER_PIXEL as u128;
        if u128::from(actual) < min {
            return Err(ImportDenied::FdLie(format!(
                "dmabuf size {actual} below the declared geometry's footprint {min}"
            )));
        }

        // 2. The import proper. The shim server enforced the allowlist
        //    (`supports`) before this call; the fourcc lookup cannot fail
        //    here short of an embedder bug, and fails closed if it does.
        let code = wire_fourcc(spec.format).ok_or_else(|| {
            ImportDenied::Refusal(ImportError {
                stage: ImportStage::Egl,
                detail: format!(
                    "format {:?} is outside the version-0 allowlist",
                    spec.format
                ),
            })
        })?;
        let mut builder = Dmabuf::builder(
            (spec.width as i32, spec.height as i32),
            code,
            Modifier::Linear,
            DmabufFlags::empty(), // top-down, like every wire buffer; no Y_INVERT
        );
        builder.add_plane(fd, 0, 0, spec.stride);
        // `build` fails only with zero planes — unreachable after
        // `add_plane` — but the TCB does not panic on a library's word:
        // fail closed as a recoverable refusal instead.
        let dmabuf = builder.build().ok_or_else(|| {
            ImportDenied::Refusal(ImportError {
                stage: ImportStage::Egl,
                detail: "dmabuf builder rejected the single plane".into(),
            })
        })?;
        let texture = self
            .renderer
            .import_dmabuf(&dmabuf, None)
            .map_err(|e| refusal(ImportStage::Egl, e))?;

        // 3. Prove it renderable *now* (both failure times collapse here,
        //    before buffer_done is decided) and, as a side effect, sync the
        //    GL queue so the previously retained buffer is GPU-done.
        probe_render(self.renderer, &texture).map_err(|e| refusal(ImportStage::FirstRender, e))?;

        *self.content = Some(GpuContent {
            texture,
            width: spec.width,
            height: spec.height,
        });
        Ok(())
    }

    fn clear(&mut self) {
        if self.content.take().is_some() {
            // Drain the GL queue before the caller declares the buffer
            // released (module docs: GPU-done is proven, not assumed). A
            // sync failure only risks an over-early release signal to an
            // already-misbehaving driver; the content itself is gone either
            // way, so log and continue.
            if let Err(e) = force_sync(self.renderer) {
                tracing::warn!("GL sync on dmabuf retire failed: {e}");
            }
        }
    }
}

#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
fn refusal(stage: ImportStage, err: GlesError) -> ImportDenied {
    ImportDenied::Refusal(ImportError {
        stage,
        detail: err.to_string(),
    })
}

/// Sample the imported texture into a 1×1 offscreen target and read the
/// texel back — the synchronous probe composite (module docs). The readback
/// forces completion of every queued GL command (in-order execution), which
/// is also the sync point the deferred-release semantics lean on. The four
/// bytes read are discarded: this is validation, not a pixel copy of the
/// frame.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
fn probe_render(renderer: &mut GlesRenderer, texture: &GlesTexture) -> Result<(), GlesError> {
    let size: Size<i32, Physical> = (1, 1).into();
    let mut target: GlesRenderbuffer =
        Offscreen::<GlesRenderbuffer>::create_buffer(renderer, Fourcc::Abgr8888, (1, 1).into())?;
    let mut fb = renderer.bind(&mut target)?;
    {
        let mut frame = renderer.render(&mut fb, size, Transform::Normal)?;
        let dst = Rectangle::from_size(size);
        // Qualified call: GlesFrame has an inherent method of the same name
        // (the winit backend's precedent).
        Frame::render_texture_from_to(
            &mut frame,
            texture,
            Rectangle::from_size(texture.size().to_f64()),
            dst,
            &[dst],
            &[],
            Transform::Normal,
            1.0,
        )?;
        let _sync = frame.finish()?;
    }
    let mapping =
        renderer.copy_framebuffer(&fb, Rectangle::from_size((1, 1).into()), Fourcc::Abgr8888)?;
    let _texel = renderer.map_texture(&mapping)?;
    Ok(())
}

/// Drain the GL command queue: a 1×1 offscreen clear + readback. In-order
/// execution means every previously queued command (including the last
/// composite that sampled a retiring buffer) has completed when this
/// returns.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
fn force_sync(renderer: &mut GlesRenderer) -> Result<(), GlesError> {
    let size: Size<i32, Physical> = (1, 1).into();
    let mut target: GlesRenderbuffer =
        Offscreen::<GlesRenderbuffer>::create_buffer(renderer, Fourcc::Abgr8888, (1, 1).into())?;
    let mut fb = renderer.bind(&mut target)?;
    {
        let mut frame = renderer.render(&mut fb, size, Transform::Normal)?;
        frame.clear(letterbox_color(), &[Rectangle::from_size(size)])?;
        let _sync = frame.finish()?;
    }
    let mapping =
        renderer.copy_framebuffer(&fb, Rectangle::from_size((1, 1).into()), Fourcc::Abgr8888)?;
    let _texel = renderer.map_texture(&mapping)?;
    Ok(())
}

/// Present retained zero-copy content into a bound framebuffer at the view
/// size: clear to the letterbox matte, then draw the texture at the
/// [`layout::place`] position, 1:1 and unscaled — the same single placement
/// policy the CPU compositor uses, so the two paths cannot drift. The GPU
/// analogue of blitting `Scene::compose` output; used by the env-gated
/// tests today and by the nested backend once P1.5.2 wires shim
/// connections at runtime.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
pub(crate) fn render_content(
    renderer: &mut GlesRenderer,
    framebuffer: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    view: Size<i32, Physical>,
    content: &GpuContent,
) -> Result<(), GlesError> {
    let full = Rectangle::from_size(view);
    let mut frame = renderer.render(framebuffer, view, Transform::Normal)?;
    frame.clear(letterbox_color(), &[full])?;
    let placement = layout::place(
        (view.w.max(0) as u32, view.h.max(0) as u32),
        (content.width, content.height),
    );
    let dst = Rectangle::new(
        (placement.x as i32, placement.y as i32).into(),
        (content.width as i32, content.height as i32).into(),
    );
    Frame::render_texture_from_to(
        &mut frame,
        &content.texture,
        Rectangle::from_size(content.texture.size().to_f64()),
        dst,
        // Damage is DST-LOCAL in this call (Smithay 0.7 constrains each
        // rect into `dst.size`, then translates by `dst.loc` — its own
        // damage tracker likewise subtracts the element location before
        // drawing): full-dst damage draws the whole placed rectangle and
        // the rasterizer clips it to the view, so a larger-than-view
        // surface center-crops. The view rectangle would be wrong here —
        // under a negative placement it shifts left/up and leaves
        // right/bottom strips of matte where client pixels belong.
        &[Rectangle::from_size(dst.size)],
        &[],
        Transform::Normal,
        1.0,
    )?;
    let _sync = frame.finish()?;
    Ok(())
}

/// [`crate::scene::LETTERBOX_RGBA`] as the renderer's float clear color —
/// derived, not restated, so the matte can never fork between the CPU and
/// GPU paths.
#[cfg_attr(not(all(test, feature = "gpu-tests")), allow(dead_code))]
fn letterbox_color() -> smithay::backend::renderer::Color32F {
    smithay::backend::renderer::Color32F::new(
        LETTERBOX_RGBA[0] as f32 / 255.0,
        LETTERBOX_RGBA[1] as f32 / 255.0,
        LETTERBOX_RGBA[2] as f32 / 255.0,
        LETTERBOX_RGBA[3] as f32 / 255.0,
    )
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::{AsFd, OwnedFd};

    use rustix::fs::MemfdFlags;

    use super::*;

    #[test]
    fn fdinfo_parser_requires_both_dmabuf_keys() {
        // A genuine dmabuf fdinfo (kernel dma_buf_show_fdinfo shape).
        let dmabuf = "pos:\t0\nflags:\t02000002\nmnt_id:\t31\nino:\t1052\n\
                      size:\t4096\ncount:\t2\nexp_name:\tsystem-heap\n";
        assert_eq!(parse_dmabuf_fdinfo(dmabuf), Some(4096));
        // A memfd's fdinfo: generic keys plus seals — no dmabuf keys.
        let memfd = "pos:\t0\nflags:\t02\nmnt_id:\t15\nino:\t99\nseals:\t0\n";
        assert_eq!(parse_dmabuf_fdinfo(memfd), None);
        // A hypothetical driver printing size but no exporter must not pass.
        let size_only = "pos:\t0\nsize:\t4096\n";
        assert_eq!(parse_dmabuf_fdinfo(size_only), None);
        // exp_name without a parseable size must not pass either: the size
        // check downstream would have nothing to check.
        let name_only = "pos:\t0\nexp_name:\tfoo\n";
        assert_eq!(parse_dmabuf_fdinfo(name_only), None);
    }

    #[test]
    fn probe_rejects_non_dmabuf_fds() {
        // The CI-reachable arm: a memfd and a character device both have
        // readable fdinfo without dmabuf keys — NotADmabuf, never a blocked
        // syscall. (The accepting arm needs a real dmabuf: the env-gated
        // GPU test covers it.)
        let memfd: OwnedFd =
            rustix::fs::memfd_create("vitrin-probe-test", MemfdFlags::CLOEXEC).expect("memfd");
        assert!(matches!(
            probe_dmabuf_fd(memfd.as_fd()),
            Err(FdProbeError::NotADmabuf)
        ));
        let devnull = OwnedFd::from(File::open("/dev/null").expect("open /dev/null"));
        assert!(matches!(
            probe_dmabuf_fd(devnull.as_fd()),
            Err(FdProbeError::NotADmabuf)
        ));
    }

    #[test]
    fn copy_meter_counts_copies_and_bytes() {
        let mut meter = CopyMeter::default();
        assert_eq!((meter.copies(), meter.pixel_bytes()), (0, 0));
        meter.record(64 * 48 * 4);
        meter.record(16);
        assert_eq!((meter.copies(), meter.pixel_bytes()), (2, 64 * 48 * 4 + 16));
    }

    #[test]
    fn wire_formats_map_to_their_fourcc() {
        // The wire enum's values are DRM fourccs; the defensive map must
        // agree with them exactly, for every wire-expressible format.
        for format in Format::ALL {
            let fourcc = wire_fourcc(*format).expect("version-1 formats are allowlisted");
            assert_eq!(fourcc as u32, *format as u32);
        }
    }
}

/// Real-GPU tests: compiled only under `--features gpu-tests`, `#[ignore]`d
/// by default, and env-gated at runtime. Run on a machine with a DRM render
/// node and EGL:
///
/// ```text
/// VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf
/// ```
#[cfg(all(test, feature = "gpu-tests"))]
mod gpu_tests {
    use std::fs::File;
    use std::os::fd::AsFd;
    use std::path::PathBuf;

    use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
    use vitrin_ipc::Connection;
    use vitrin_mock_shim::{frame_rgba, frame_xrgb8888, MockShim, SurfaceEvent};
    use vitrin_protocol::generated::vitrin_shim_surface::BufferStatus;

    use super::*;
    use crate::scene::Scene;
    use crate::shim::{ShimConfig, ShimServer};

    const W: u32 = 96;
    const H: u32 = 64;

    /// Skip (returning `None`) unless the env gate is set — belt to the
    /// `#[ignore]` braces, so `--ignored` runs on a GPU-less box degrade to
    /// a loud skip instead of a failure.
    fn env_gate() -> Option<()> {
        if std::env::var_os("VITRIN_GPU_TESTS").is_none() {
            eprintln!("skipping: set VITRIN_GPU_TESTS=1 to run real-GPU dmabuf tests");
            return None;
        }
        Some(())
    }

    /// A GLES renderer plus a GBM device on that renderer's *own* DRM
    /// render node — the first EGL device where the whole pipeline
    /// (renderer init, GBM open, linear allocation, CPU write) actually
    /// works. Multi-GPU reality (risk R3's territory): e.g. a proprietary
    /// NVIDIA device can enumerate first yet not support `gbm_bo_write`,
    /// while the Mesa iGPU next to it supports everything — so the harness
    /// probes each candidate end to end instead of trusting enumeration
    /// order, and buffers are always allocated on the device that imports
    /// them.
    fn gpu_harness() -> Option<(GlesRenderer, gbm::Device<File>, PathBuf)> {
        for device in EGLDevice::enumerate().ok()? {
            let Ok(path) = device.render_device_path() else {
                continue;
            };
            // SAFETY: `EGLDevice` came from EGL's own enumeration, so the
            // native display handle it wraps is valid for the platform-
            // device EGL platform — the constructor's entire contract.
            let Ok(display) = (unsafe { EGLDisplay::new(device) }) else {
                continue;
            };
            let Ok(context) = EGLContext::new(&display) else {
                continue;
            };
            // SAFETY: the context was just created, is current on no other
            // thread, and this thread runs no other GL user — the
            // `GlesRenderer::new` contract.
            let Ok(renderer) = (unsafe { GlesRenderer::new(context) }) else {
                continue;
            };
            let Ok(file) = File::options().read(true).write(true).open(&path) else {
                continue;
            };
            let Ok(gbm) = gbm::Device::new(file) else {
                continue;
            };
            // Prove the allocation path before committing to this device.
            if gbm_frame(&gbm, 0).is_err() {
                eprintln!(
                    "candidate {} cannot allocate+write linear GBM buffers, trying next",
                    path.display()
                );
                continue;
            }
            return Some((renderer, gbm, path));
        }
        None
    }

    /// Allocate a linear XRGB8888 GBM buffer of the given size, fill it
    /// with frame `n`'s deterministic pixels, and export (fd, stride). The
    /// test-side app/shim role: this is exactly what P1.6.2's v1 forwarding
    /// passes through untouched.
    fn gbm_frame_sized(
        gbm: &gbm::Device<File>,
        n: u32,
        width: u32,
        height: u32,
    ) -> Result<(std::os::fd::OwnedFd, u32), Box<dyn std::error::Error>> {
        use gbm::BufferObjectFlags;
        let mut bo = gbm.create_buffer_object::<()>(
            width,
            height,
            gbm::Format::Xrgb8888,
            BufferObjectFlags::RENDERING | BufferObjectFlags::LINEAR,
        )?;
        // Fill via gbm_bo_map (map_mut) rather than gbm_bo_write: Mesa
        // implements the latter only for BOs allocated with USE_WRITE
        // (cursor-style dumb buffers), while mapping works for any linear
        // BO. Rows land at the *map* stride the driver reports.
        let tight = frame_xrgb8888(n, width, height);
        let row = width as usize * BYTES_PER_PIXEL;
        bo.map_mut(0, 0, width, height, |map| {
            let stride = map.stride() as usize;
            let buffer = map.buffer_mut();
            for (i, src) in tight.chunks_exact(row).enumerate() {
                buffer[i * stride..i * stride + row].copy_from_slice(src);
            }
        })?;
        let stride = bo.stride();
        let fd = bo.fd()?;
        Ok((fd, stride))
    }

    /// [`gbm_frame_sized`] at the harness view size — the steady-state
    /// single-maximized shape the end-to-end tests drive.
    fn gbm_frame(
        gbm: &gbm::Device<File>,
        n: u32,
    ) -> Result<(std::os::fd::OwnedFd, u32), Box<dyn std::error::Error>> {
        gbm_frame_sized(gbm, n, W, H)
    }

    /// Drive `n` core-side dispatches, with the GLES importer plugged in.
    fn process_n(
        server: &mut ShimServer,
        scene: &mut Scene,
        core: &mut Connection,
        renderer: &mut GlesRenderer,
        content: &mut Option<GpuContent>,
        n: usize,
    ) {
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            let mut importer = GlesDmabufImporter { renderer, content };
            server
                .handle_message(msg, scene, Some(&mut importer), &mut |frame| {
                    core.send_message(frame, None)
                })
                .expect("compliant shim must not fault");
        }
    }

    /// Composite the retained GPU content at the view size and read it back
    /// (test apparatus, not the core frame path) as tightly packed RGBA.
    fn composite_and_readback(renderer: &mut GlesRenderer, content: &GpuContent) -> Vec<u8> {
        let size: Size<i32, Physical> = (W as i32, H as i32).into();
        let mut target: GlesRenderbuffer = Offscreen::<GlesRenderbuffer>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            (W as i32, H as i32).into(),
        )
        .expect("offscreen target");
        let mut fb = renderer.bind(&mut target).expect("bind");
        render_content(renderer, &mut fb, size, content).expect("render retained content");
        let mapping = renderer
            .copy_framebuffer(
                &fb,
                Rectangle::from_size((W as i32, H as i32).into()),
                Fourcc::Abgr8888,
            )
            .expect("copy framebuffer");
        renderer.map_texture(&mapping).expect("map").to_vec()
    }

    /// The M1.5 acceptance: on a real GPU, shim→core frames are zero-copy —
    /// end to end over the real wire (mock shim, socketpair, `ShimServer`),
    /// with the copy meter as the instrumented proof — and the deferred
    /// release semantics hold, and the shm fallback still works afterwards.
    #[test]
    #[ignore = "requires a real GPU (EGL + DRM render node); VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf"]
    fn real_gpu_dmabuf_frames_are_zero_copy_end_to_end() {
        let Some(()) = env_gate() else { return };
        let _fd = crate::capture::tests::fd_lock();
        let Some((mut renderer, gbm, node)) = gpu_harness() else {
            panic!("VITRIN_GPU_TESTS=1 but no EGL device with a working GBM pipeline was found");
        };
        eprintln!("running on {}", node.display());
        let mut probe_slot: Option<GpuContent> = None;
        let supported = GlesDmabufImporter {
            renderer: &mut renderer,
            content: &mut probe_slot,
        }
        .supports(Format::Xrgb8888);
        if !supported {
            // Linear-import support is a per-GPU reality (risk R3); a
            // driver without it exercises the fallback path, not this test.
            eprintln!("skipping: renderer does not import XRGB8888+LINEAR dmabufs");
            return;
        }

        let (mut core, shim_conn) = Connection::pair().expect("socketpair");
        let mut server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: W,
            height: H,
        });
        server
            .send_configure(&mut |frame| core.send_message(frame, None))
            .expect("configure");
        let mut mock = MockShim::start(shim_conn).expect("bring-up");
        let mut scene = Scene::new();
        let mut content: Option<GpuContent> = None;

        process_n(
            &mut server,
            &mut scene,
            &mut core,
            &mut renderer,
            &mut content,
            1,
        ); // create_surface

        // Three zero-copy frames. Frame n's buffer is released only when
        // frame n+1's import replaces it (deferred release), and no
        // core-side pixel copy ever happens.
        let mut cookies = Vec::new();
        for n in 0..3u32 {
            let (fd, stride) = gbm_frame(&gbm, n).expect("gbm buffer");
            let cookie = mock
                .attach_dmabuf(fd, Format::Xrgb8888, W, H, stride)
                .expect("attach dmabuf");
            cookies.push(cookie);
            mock.commit().expect("commit");
            process_n(
                &mut server,
                &mut scene,
                &mut core,
                &mut renderer,
                &mut content,
                3,
            );

            // Deferred release: the PREVIOUS cookie's released arrives with
            // this commit; the current one is retained, unanswered.
            if n > 0 {
                assert_eq!(
                    mock.next_surface_event().expect("event"),
                    SurfaceEvent::BufferDone {
                        buffer_id: cookies[(n - 1) as usize],
                        status: BufferStatus::Released,
                    },
                    "frame {} must be released exactly when frame {n} replaces it",
                    n - 1
                );
            }

            // Present and pace.
            assert!(server.wants_presentation());
            server
                .presented(n, &mut |frame| core.send_message(frame, None))
                .expect("presented");
            assert_eq!(
                mock.next_surface_event().expect("event"),
                SurfaceEvent::FrameDone { time_ms: n }
            );

            // The composited view IS the client's pixels — sampled from the
            // client's own buffer, byte-exact against the generator.
            let composed =
                composite_and_readback(&mut renderer, content.as_ref().expect("content retained"));
            assert_eq!(
                composed,
                frame_rgba(n, W, H),
                "composited frame {n} must be the exact generator output"
            );
            // The instrumented zero-copy proof: no core-side CPU copy of
            // client pixels happened, for any frame so far.
            assert_eq!(
                server.copy_meter().copies(),
                0,
                "dmabuf path must not copy client pixels core-side"
            );
        }

        // Fall back to shm mid-stream: the retained dmabuf is retired and
        // released FIRST (attach order), then the shm attach completes with
        // its own prompt release, and the copy meter records exactly one
        // copy. The zero-copy and copy-in regimes interleave cleanly.
        mock.attach_frame(7).expect("shm attach");
        mock.commit().expect("commit");
        process_n(
            &mut server,
            &mut scene,
            &mut core,
            &mut renderer,
            &mut content,
            3,
        );
        assert_eq!(
            mock.next_surface_event().expect("event"),
            SurfaceEvent::BufferDone {
                buffer_id: cookies[2],
                status: BufferStatus::Released,
            },
            "the retained dmabuf must be released when shm content replaces it"
        );
        assert!(content.is_none(), "GPU content retired on shm commit");
        let shm_release = mock.next_surface_event().expect("event");
        assert!(
            matches!(
                shm_release,
                SurfaceEvent::BufferDone {
                    status: BufferStatus::Released,
                    ..
                }
            ),
            "shm buffer released promptly, got {shm_release:?}"
        );
        assert_eq!(scene.compose(W, H), frame_rgba(7, W, H));
        assert_eq!(
            (
                server.copy_meter().copies(),
                server.copy_meter().pixel_bytes()
            ),
            (1, u64::from(W) * u64::from(H) * 4),
            "the shm fallback copies exactly once"
        );
        server
            .presented(99, &mut |frame| core.send_message(frame, None))
            .expect("presented");
    }

    /// The real fdinfo probe accepts a genuine dmabuf (the arm CI cannot
    /// reach) and rejects a memfd posing as one at the same commit site —
    /// on real hardware, with the real importer plugged in.
    #[test]
    #[ignore = "requires a real GPU (EGL + DRM render node); VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf"]
    fn real_gpu_probe_accepts_dmabuf_and_kills_memfd_lie() {
        let Some(()) = env_gate() else { return };
        let _fd = crate::capture::tests::fd_lock();
        let Some((mut renderer, gbm, node)) = gpu_harness() else {
            panic!("VITRIN_GPU_TESTS=1 but no EGL device with a working GBM pipeline was found");
        };
        eprintln!("running on {}", node.display());

        // Accepting arm: a genuine dmabuf passes the probe with its true
        // size.
        let (fd, stride) = gbm_frame(&gbm, 0).expect("gbm buffer");
        let size = probe_dmabuf_fd(fd.as_fd()).expect("genuine dmabuf must pass the probe");
        assert!(
            u64::from(stride) * u64::from(H) <= size + u64::from(stride),
            "fdinfo size {size} must cover the allocated geometry"
        );
        drop(fd);

        // Killing arm: a memfd attached as kind=dmabuf dies as
        // invalid_buffer at commit — with the importer present, the lie is
        // observed and the connection is killed, never a driver call on the
        // hostile fd.
        let (mut core, shim_conn) = Connection::pair().expect("socketpair");
        let mut server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: W,
            height: H,
        });
        server
            .send_configure(&mut |frame| core.send_message(frame, None))
            .expect("configure");
        let mut mock = MockShim::start(shim_conn).expect("bring-up");
        let mut scene = Scene::new();
        let mut content: Option<GpuContent> = None;
        process_n(
            &mut server,
            &mut scene,
            &mut core,
            &mut renderer,
            &mut content,
            1,
        );

        let fake = vitrin_mock_shim::memfd_with_bytes(&frame_xrgb8888(0, W, H)).expect("memfd");
        mock.attach_dmabuf(fake, Format::Xrgb8888, W, H, W * 4)
            .expect("attach");
        mock.commit().expect("commit");
        // attach + damage dispatch fine; the commit must fault the
        // connection with the invalid_buffer condition.
        for _ in 0..2 {
            let msg = core.recv_message().expect("recv").expect("msg");
            let mut importer = GlesDmabufImporter {
                renderer: &mut renderer,
                content: &mut content,
            };
            server
                .handle_message(msg, &mut scene, Some(&mut importer), &mut |frame| {
                    core.send_message(frame, None)
                })
                .expect("attach and damage are legal");
        }
        let msg = core.recv_message().expect("recv").expect("msg");
        let mut importer = GlesDmabufImporter {
            renderer: &mut renderer,
            content: &mut content,
        };
        let fault = server
            .handle_message(msg, &mut scene, Some(&mut importer), &mut |frame| {
                core.send_message(frame, None)
            })
            .expect_err("a memfd posing as a dmabuf must kill the connection");
        assert!(
            fault.to_string().contains("invalid_buffer"),
            "expected invalid_buffer, got: {fault}"
        );
    }

    /// The center-crop acceptance for [`render_content`]: content larger
    /// than the view (legal mid-resize; [`layout::place`] goes negative)
    /// must fill the **whole** view with the client's central pixels, 1:1 —
    /// exactly what `Scene::compose` does on the CPU path. Pins the
    /// dst-local damage contract of `Frame::render_texture_from_to`:
    /// passing view-space damage instead shifts the drawn region by the
    /// negative placement and leaves right/bottom strips of letterbox
    /// matte where client pixels belong.
    #[test]
    #[ignore = "requires a real GPU (EGL + DRM render node); VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf"]
    fn real_gpu_oversized_dmabuf_center_crops_the_full_view() {
        let Some(()) = env_gate() else { return };
        let _fd = crate::capture::tests::fd_lock();
        let Some((mut renderer, gbm, node)) = gpu_harness() else {
            panic!("VITRIN_GPU_TESTS=1 but no EGL device with a working GBM pipeline was found");
        };
        eprintln!("running on {}", node.display());

        // Larger than the view on both axes, asymmetrically, so both
        // center-crop offsets are negative and different: placement is
        // ((W - SW) / 2, (H - SH) / 2) = (-32, -18) for the 96x64 view.
        const SW: u32 = W + 64;
        const SH: u32 = H + 36;
        const N: u32 = 5;

        let mut content: Option<GpuContent> = None;
        let mut importer = GlesDmabufImporter {
            renderer: &mut renderer,
            content: &mut content,
        };
        if !importer.supports(Format::Xrgb8888) {
            eprintln!("skipping: renderer does not import XRGB8888+LINEAR dmabufs");
            return;
        }
        let (fd, stride) = gbm_frame_sized(&gbm, N, SW, SH).expect("gbm buffer");
        importer
            .import(
                &DmabufSpec {
                    format: Format::Xrgb8888,
                    width: SW,
                    height: SH,
                    stride,
                },
                fd,
            )
            .expect("an oversized linear buffer must import");

        let composed =
            composite_and_readback(&mut renderer, content.as_ref().expect("content retained"));
        // Expected: the buffer's central W x H window, row-extracted from
        // the same deterministic generator the buffer was filled from.
        let full = frame_rgba(N, SW, SH);
        let (cx, cy) = (((SW - W) / 2) as usize, ((SH - H) / 2) as usize);
        let row = W as usize * BYTES_PER_PIXEL;
        let mut expected = Vec::with_capacity(row * H as usize);
        for y in 0..H as usize {
            let off = ((cy + y) * SW as usize + cx) * BYTES_PER_PIXEL;
            expected.extend_from_slice(&full[off..off + row]);
        }
        assert_eq!(
            composed, expected,
            "the view must be the client's central {W}x{H} window, 1:1 — no matte strips"
        );
    }
}
