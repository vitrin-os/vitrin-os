// SPDX-License-Identifier: MPL-2.0
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
//! the old buffer has provably completed. The dmabuf *fd* itself is
//! consumed and closed by the successful import (EGL dups/references what
//! it needs at `eglCreateImageKHR`, per `EGL_EXT_image_dma_buf_import`) —
//! "before ... this event", within the IDL's fd-ownership contract. What a
//! retained import keeps alive is the *kernel buffer*, via the texture's
//! EGLImage: no fd of a retained buffer stays in the core's fd table, so
//! retained imports never count against fd-exhaustion budgets (the
//! issue-#21 EMFILE accounting) — only *staged* attaches (fd received,
//! commit pending) hold fds, and those are capped in [`crate::shim`].
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
//! Headless/CI embedders pass `None` and never touch EGL.
//!
//! The **nested** backend does construct one at runtime (issue #132): both
//! `Presenter::scene_and_importer` and `Presenter::teardown_view` build a
//! [`GlesDmabufImporter`] over its live `GlesRenderer`, so a `kind=dmabuf`
//! commit under `--nested` resolves as a real import and the death funnel
//! disposes of retained content through the same seam. The **headless**
//! backend has no GPU renderer and inherits the trait's `None` — every
//! dmabuf commit there is still the designed `import_failed` shm fallback.
//! Only the *tests* need a buffer allocator (GBM), which is why the
//! `gpu-tests` cargo feature exists; see `Cargo.toml`.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
use smithay::backend::allocator::{Format as DrmFormat, Fourcc, Modifier};
use smithay::backend::renderer::gles::{GlesError, GlesRenderbuffer, GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, ExportMem, Frame, ImportDma, Offscreen, Renderer, Texture};
use smithay::utils::{Physical, Rectangle, Size, Transform};
use vitrin_protocol::generated::vitrin_view::Format;

use crate::consent::{TrustedIndicator, TRUST_BAND_HEIGHT};
use crate::grants::RealmId;
use crate::scene::{BYTES_PER_PIXEL, LETTERBOX_RGBA};
use crate::view::ViewGeometry;

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
/// One importer serves one shim connection -- that is, one realm. It is
/// constructed fresh per dispatch (see [`GlesDmabufImporter`]), but always
/// over the **same slot**: the embedder builds every
/// `handle_message`/`connection_closed` importer for a realm over that realm's
/// own [`RealmGpuContent`] entry, so the retained zero-copy buffer persists
/// between that realm's commits and no other realm can reach it. `None`
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
/// Owning this keeps the *kernel buffer* alive via the texture's EGLImage;
/// the dmabuf fd itself was already closed when [`GlesDmabufImporter::import`]
/// returned success (EGL holds its own reference on the buffer — no fd
/// lives here, and none stays open in the core's fd table).
pub(crate) struct GpuContent {
    texture: GlesTexture,
    width: u32,
    height: u32,
}

/// **One retained zero-copy slot per realm** (WS-E.1.3, issue #209) — the
/// GPU-side twin of [`crate::scene::RealmScenes`], and for the same
/// confidentiality reason.
///
/// # Why this is not one slot
///
/// It was one, and that was the defect. A single `Option<GpuContent>` for the
/// whole session is written by *whichever* realm's shim last imported a
/// dmabuf, so with `--dmabuf` a **hidden** realm's commit took over the
/// human-visible window: the frame path presented the one retained texture without
/// ever asking which realm the output was bound to. That falsifies the
/// published claim that only the bound realm is on screen, on the one path a
/// human actually looks at, and it is the same class of bug the scene split
/// closed — the last committer owning the only thing there is to present.
///
/// It also made teardown session-wide. [`crate::session::Presenter::teardown_view`]
/// names the dying realm and hands out *its* scene, but the importer it lends
/// held the one slot, so any realm's death cleared whichever realm's retained
/// content happened to be resident.
///
/// # The shape
///
/// A slot is reached only by naming a realm ([`Self::slot_mut`], [`Self::of`]).
/// There is deliberately **no** "the content" accessor: the frame path asks
/// `backend::winit`'s `zero_copy_source`, which takes the bound realm and can
/// therefore be read — and tested — as the selection it is.
///
/// Generic in `C` for one reason, stated plainly because an unused type
/// parameter is otherwise a smell: a [`GpuContent`] holds a [`GlesTexture`]
/// and cannot be minted without a live GL context, and D-019(4) records
/// headless as the only backend CI can run. The stand-in lets the display-free
/// tests drive the *real* store and the *real* selection function rather than
/// a paraphrase of them. Nothing outside `#[cfg(test)]` ever instantiates it
/// at anything but the default.
pub(crate) struct RealmGpuContent<C = GpuContent> {
    /// `Option<C>` rather than a bare `C` because
    /// [`GlesDmabufImporter::content`] borrows the slot itself: an import
    /// fills it, [`DmabufImporter::clear`] empties it, and the importer must
    /// be able to do both through one `&mut`. An empty slot left behind by a
    /// realm's death is kept rather than removed — dropping a [`GpuContent`]
    /// outside [`DmabufImporter::clear`] would skip the GPU sync that clear
    /// performs, and the map is bounded by [`crate::realm::MAX_REALMS`]
    /// `Option`s either way.
    slots: BTreeMap<RealmId, Option<C>>,
}

impl<C> RealmGpuContent<C> {
    /// No realm, nothing retained.
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
        }
    }

    /// **This realm's** retained-content slot, minted empty on first use —
    /// the one mutable accessor, and it always names a realm.
    ///
    /// What [`GlesDmabufImporter`] is built over, on both of the nested
    /// backend's paths: the shim dispatch that imports
    /// (`Presenter::scene_and_importer`, keyed by the realm whose connection
    /// is being serviced) and the death funnel that clears
    /// (`Presenter::teardown_view`, keyed by the dying realm). Neither can
    /// reach a sibling's texture.
    pub fn slot_mut(&mut self, realm: &RealmId) -> &mut Option<C> {
        self.slots.entry(realm.clone()).or_default()
    }

    /// This realm's retained content, if it has imported one and nothing has
    /// cleared it since.
    pub fn of(&self, realm: &RealmId) -> Option<&C> {
        self.slots.get(realm).and_then(|slot| slot.as_ref())
    }
}

/// The real importer: wraps the embedder's `GlesRenderer` (the nested
/// backend's at runtime since issue #132; the env-gated tests' under
/// `VITRIN_GPU_TESTS=1`) and the embedder-owned retained-content slot.
/// Constructed fresh per dispatch — it borrows, it does not own.
pub(crate) struct GlesDmabufImporter<'a> {
    pub renderer: &'a mut GlesRenderer,
    pub content: &'a mut Option<GpuContent>,
}

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

/// One primitive of a human-visible GPU frame, in draw order.
///
/// The list [`human_visible_frame`] returns is *the* description of what the
/// zero-copy path puts on the human's display, and it is a value rather than
/// a straight-line sequence of GL calls for exactly the reason
/// `backend::winit`'s `window_pixels` is its own function: presenting
/// needs an EGL context and a host window, so CI cannot drive the executor —
/// but it can drive the decision, and the decision is the one that carries
/// the trusted-indicator invariant (issue #85). A frame with no
/// [`Draw::TrustBand`] is a forgeable frame, and
/// `every_zero_copy_frame_ends_with_the_trusted_band` fails on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Draw {
    /// Nothing. The cursor slots of a frame with no agent cursor to draw.
    ///
    /// A fixed-length draw list with empty slots rather than a `Vec` whose
    /// length varies: the list's length is what makes "the trusted band is
    /// the last draw" a shape a display-free test can assert, and the
    /// zero-copy path allocates nothing per frame by design.
    Nothing,
    /// Clear the whole view to the letterbox matte.
    Matte,
    /// Blit the client's imported texture into this destination rectangle,
    /// 1:1 and unscaled ([`ViewGeometry::place`]).
    Content(Rectangle<i32, Physical>),
    /// Fill this rectangle with a piece of the agent cursor sprite
    /// ([`crate::cursor`]). Human-visible only, like everything else here,
    /// and already clipped below the trusted band by the geometry function.
    AgentCursor(Rectangle<i32, Physical>, [u8; 4]),
    /// Fill this rectangle with a piece of the **human's own** pointer
    /// sprite (WS-E.3.2, [`crate::cursor::human_cursor_rects`]), on exactly
    /// the terms above.
    ///
    /// A distinct variant rather than a second [`Draw::AgentCursor`] because
    /// the two mean different things to whoever audits a frame: an agent's
    /// crosshair is a safety signal saying *something other than the human is
    /// pointing here*, and the human's is the pointing device itself. Only a
    /// backend that owns the physical display emits it — nested leaves these
    /// slots empty, because the host desktop draws the human's pointer over
    /// the core's window already.
    HumanCursor(Rectangle<i32, Physical>, [u8; 4]),
    /// Blit the status strip's CPU raster into this rectangle
    /// (WS-E.2.3, [`crate::status`]).
    ///
    /// **The rectangle only.** The texture is supplied by the executor
    /// ([`present_human_visible`]) rather than carried here, so this enum stays
    /// `Copy` and [`human_visible_frame`] stays a pure function of numbers —
    /// which is what lets a display-free test assert the strip's geometry
    /// against the view's own numbers. The pixels the executor uploads come
    /// from [`crate::status::StatusStrip::raster`], the same cache the CPU path
    /// blits, so the two paths cannot draw different strips.
    ///
    /// Present only when `--status` is on. This is the one slot of the draw
    /// list that a *configuration* can empty, and the difference from the band
    /// is the point: the band is not optional and no arm omits it, whereas a
    /// session with no status strip has no strip to draw.
    StatusStrip(Rectangle<i32, Physical>),
    /// Fill this rectangle with the session's trusted-indicator colour: the
    /// reserved strip along the top edge the human reads this session's
    /// secret colour from.
    TrustBand(Rectangle<i32, Physical>, [u8; 4]),
}

/// The trusted band's rectangle on a `view`-sized human-visible frame —
/// the full width, [`TRUST_BAND_HEIGHT`] tall, clamped to a shorter view.
///
/// Derived from the same constant
/// [`ConsentSurface::composite_trust_band`](crate::consent::ConsentSurface::composite_trust_band)
/// fills on the CPU path, never restated, so the two paths cannot paint
/// bands of different heights;
/// `the_gpu_band_covers_exactly_what_the_cpu_band_paints` pins the equality
/// against the real CPU compositor, and
/// `every_zero_copy_frame_ends_with_the_trusted_band` pins the rectangle
/// itself against the view's numbers (never against this function's own
/// output, which would make the check vacuous).
pub(crate) fn trust_band_rect(view: Size<i32, Physical>) -> Rectangle<i32, Physical> {
    let h = (TRUST_BAND_HEIGHT as i32).min(view.h.max(0));
    Rectangle::new((0, 0).into(), (view.w.max(0), h).into())
}

/// The status strip's rectangle on a `view`-sized human-visible frame — the
/// full width, `strip_h` tall, starting immediately below the band.
///
/// `strip_h` is [`crate::status::StatusStrip::height`], which is `0` when
/// `--status` is off; an empty rectangle is the honest answer then, and
/// [`human_visible_frame`] leaves the slot at [`Draw::Nothing`] rather than
/// submitting a degenerate draw.
///
/// Derived from [`crate::status::STRIP_TOP`] — itself derived from
/// [`TRUST_BAND_HEIGHT`] — and never restated, so the CPU blit and this
/// rectangle cannot disagree about where the strip is. That is the same
/// discipline [`trust_band_rect`] documents, applied to the surface that sits
/// directly under the band; `the_gpu_strip_covers_exactly_what_the_cpu_strip_paints`
/// pins the equality against the real CPU compositor.
pub(crate) fn status_strip_rect(
    view: Size<i32, Physical>,
    strip_h: u32,
) -> Rectangle<i32, Physical> {
    let top = (crate::status::STRIP_TOP as i32).min(view.h.max(0));
    let h = (strip_h as i32).min(view.h.max(0) - top);
    Rectangle::new((0, top).into(), (view.w.max(0), h.max(0)).into())
}

/// How many draws one human-visible GPU frame is: the letterbox matte, the
/// client's content, the status strip, the agent cursor's slots, the human
/// cursor's slots, and the trusted band.
pub(crate) const HUMAN_VISIBLE_DRAWS: usize =
    4 + crate::cursor::AGENT_CURSOR_RECTS + crate::cursor::HUMAN_CURSOR_RECTS;

/// Index of the first agent-cursor slot. The three fixed draws (matte,
/// content, strip) come first.
const AGENT_CURSOR_SLOT: usize = 3;
/// Index of the first human-cursor slot: immediately after the agent's, so
/// the human's pointer is drawn *over* an agent's crosshair when the two
/// overlap. Deliberate — the human's own pointer must never be the thing
/// that disappears — and harmless to D-019, whose signal is "a crosshair is
/// somewhere on this screen", not "no pixel of it is covered".
const HUMAN_CURSOR_SLOT: usize = AGENT_CURSOR_SLOT + crate::cursor::AGENT_CURSOR_RECTS;

/// Everything one human-visible GPU frame draws, in order: the letterbox
/// matte, the client's content at its [`ViewGeometry::place`] position, the agent
/// cursor sprite if one is shown, and the trusted band last.
///
/// **The band is not optional and there is no arm of this function that
/// omits it.** That is the whole point of returning a fixed-size array from
/// one pure function instead of writing the GL calls inline: the zero-copy
/// path's frame is composed entirely of pixels the confined client owns, so
/// a frame that reached the display without the band would let the client
/// rasterize a counterfeit band into the top of its own buffer with nothing
/// genuine above it — the forgery the indicator exists to make impossible
/// (see [`crate::consent::TrustedIndicator`]).
///
/// The band goes on top of everything else for the same reason the CPU path
/// composites it after the client's surface *and* after the consent overlay:
/// client content in that strip is always overdrawn by the genuine colour.
/// The cursor slots sit *before* it, and their geometry is clipped below the
/// band anyway ([`crate::cursor::agent_cursor_rects`]) — belt and braces, on
/// a sprite whose position an agent chooses.
///
/// `agent_cursor` is the agent-owned pointer position for this frame, or
/// `None` when no sprite is shown; its rectangles are derived from
/// [`crate::cursor::agent_cursor_rects`] and never restated here, so the CPU
/// and GPU paths cannot draw different crosshairs — the drift
/// [`trust_band_rect`] exists to prevent for the band.
///
/// `geom` is this output's [`ViewGeometry`] (issue #304): its size, and the
/// rows the core reserves above the client. **It subsumes the `status_h: u32`
/// this function used to take beside the view size** — that parameter was
/// partial awareness of exactly this problem, the GPU path knowing the strip's
/// height because it had to draw it, and leaving it alongside a `ViewGeometry`
/// would be the second carrier for one number that #304 exists to remove. The
/// strip's height is [`ViewGeometry::strip_height`], `0` when the session has
/// no strip; it rides in the draw list for the same reason the band and the
/// cursor do — so a presentation path gets it by construction — but unlike the
/// band it is legitimately absent, and
/// `every_zero_copy_frame_ends_with_the_trusted_band` checks the band is still
/// the *last* draw either way.
///
/// The content rectangle comes from [`ViewGeometry::place`], the same call the
/// CPU compositor and the input router make, so the zero-copy path reserves the
/// same rows they do — the "one path reserving rows the others do not" failure
/// that made a half-done inset worse than none.
pub(crate) fn human_visible_frame(
    geom: ViewGeometry,
    content: (u32, u32),
    indicator: TrustedIndicator,
    agent_cursor: Option<(f64, f64)>,
    human_cursor: Option<(f64, f64)>,
) -> [Draw; HUMAN_VISIBLE_DRAWS] {
    let (vw, vh) = geom.output();
    let view: Size<i32, Physical> = (vw as i32, vh as i32).into();
    let status_h = geom.strip_height();
    let placement = geom.place(content);
    let dst = Rectangle::new(
        (placement.x as i32, placement.y as i32).into(),
        (content.0 as i32, content.1 as i32).into(),
    );
    let mut draws = [Draw::Nothing; HUMAN_VISIBLE_DRAWS];
    draws[0] = Draw::Matte;
    draws[1] = Draw::Content(dst);
    // **Strip BEFORE the cursor**, matching the CPU path, where
    // `compose_human_visible` draws the strip and `composite_agent_cursor` runs
    // after it. The two paths disagreed: on the GPU the strip sat in the slot
    // after the cursor rects and covered the crosshair. The cursor wins because
    // it is the signal that an AGENT IS POINTING somewhere -- a safety cue --
    // while the strip is informational, and two backends that disagree about
    // which of those a human sees is worse than either answer.
    if status_h > 0 {
        let rect = status_strip_rect(view, status_h);
        if rect.size.w > 0 && rect.size.h > 0 {
            draws[2] = Draw::StatusStrip(rect);
        }
    }
    if let Some(rects) = agent_cursor.and_then(|(x, y)| {
        crate::cursor::agent_cursor_rects(view.w.max(0) as u32, view.h.max(0) as u32, x, y)
    }) {
        for (slot, rect) in draws[AGENT_CURSOR_SLOT..HUMAN_CURSOR_SLOT]
            .iter_mut()
            .zip(rects)
        {
            *slot = Draw::AgentCursor(
                Rectangle::new(
                    (rect.x, rect.y).into(),
                    (rect.w as i32, rect.h as i32).into(),
                ),
                rect.rgba,
            );
        }
    }
    // ...then the human's own pointer, in its own slots (WS-E.3.2). `None`
    // for every backend but the bare-metal one, which is the only place the
    // core owns the physical display and therefore the only place a human
    // pointer exists that nobody else is drawing.
    if let Some(rects) = human_cursor.and_then(|(x, y)| {
        crate::cursor::human_cursor_rects(view.w.max(0) as u32, view.h.max(0) as u32, x, y)
    }) {
        for (slot, rect) in draws[HUMAN_CURSOR_SLOT..HUMAN_VISIBLE_DRAWS - 1]
            .iter_mut()
            .zip(rects)
        {
            *slot = Draw::HumanCursor(
                Rectangle::new(
                    (rect.x, rect.y).into(),
                    (rect.w as i32, rect.h as i32).into(),
                ),
                rect.rgba,
            );
        }
    }
    draws[HUMAN_VISIBLE_DRAWS - 1] = Draw::TrustBand(trust_band_rect(view), indicator.color());
    draws
}

/// The output transform for any **memory-backed** render target.
///
/// Two kinds of target reach [`present_human_visible`] and they take opposite
/// transforms, so the kinds are named here rather than the values guessed at
/// each call site. A memory-backed target — an offscreen renderbuffer read
/// back with `copy_framebuffer`, or the GBM buffer `GbmBufferedSurface` hands
/// to a CRTC — is addressed from its **first row of storage**, and smithay's
/// renderer already post-multiplies GL's own y-flip, so `Normal` puts the
/// logical top row there. The other kind is an EGL **window surface**, whose
/// default framebuffer origin is bottom-left; that one takes
/// [`crate::backend::winit::WINDOW_TRANSFORM`].
///
/// This constant exists because the bare-metal backend took the *window's*
/// value for a scanout buffer and presented the human's whole display
/// vertically mirrored — found by running it on a panel, and by nothing else.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the production readers are `backend::drm`'s SCANOUT_TRANSFORM, which spells \
                  the literal so a mutation of it is a real one, and the GPU harness. It is \
                  named here so the two kinds of target are stated once rather than guessed \
                  at each call site"
    )
)]
pub(crate) const MEMORY_TARGET_TRANSFORM: Transform = Transform::Normal;

/// Present retained zero-copy content into a bound framebuffer at the view
/// size, as **human-visible** output: [`human_visible_frame`]'s draw list,
/// executed against `renderer`.
///
/// This is the GPU analogue of [`crate::backend::human_visible_from_view`],
/// and the one entry point any backend has for putting retained
/// [`GpuContent`] on a display. There is deliberately no content-only
/// variant to reach for: the trusted band rides in the draw list itself, so
/// a future third presentation path gets it by construction rather than by
/// remembering to ask for it (the P1.3.5 zero-copy branch shipped without
/// it, and nothing failed).
///
/// **Human-visible only, never a capture.** An agent's frames come from
/// [`Scene::compose`](crate::scene::Scene::compose) on the CPU on both
/// backends (see [`crate::backend::winit::capture_pixels`]), which is
/// upstream of every overlay — so the band painted here can no more reach
/// `vitrin_view.frame_ready` than the consent card can. Nothing may serve
/// the output of this function to an agent.
///
/// `transform` is the **output** transform of the target being drawn into,
/// and it is a parameter because the targets fall into two kinds that
/// disagree. The winit EGL **window surface** has its GL origin at the
/// bottom-left and needs `backend::winit`'s `WINDOW_TRANSFORM`
/// (`Flipped180`); every **memory-backed** target — an offscreen renderbuffer
/// read back with `copy_framebuffer`, and the GBM buffer the bare-metal
/// backend hands to a CRTC — is addressed from its first row of storage and
/// needs [`MEMORY_TARGET_TRANSFORM`]. It was hardcoded to `Normal` when this
/// function only ever ran against the offscreen harness; presenting into the
/// window with that constant renders the frame upside down, and passing the
/// *window's* constant for a scanout buffer renders it upside down the other
/// way — which is the defect first light found (`backend::drm`'s
/// `SCANOUT_TRANSFORM`).
///
/// `geom` is this output's [`ViewGeometry`]: its size, and the rows the core
/// reserves above the client (issue #304). The strip's height rides in it, so
/// this function is not handed one number twice.
///
/// `status` is the status strip's already-uploaded texture, or `None` for a
/// session with no strip. It is a *texture* rather than a
/// forced fall-back to the CPU compositor because this path exists precisely
/// because 2560x1600@240 cannot be CPU-composited: a re-upload of one
/// 2560x20 RGBA texture is 200 KiB, and the snapshot changes once a minute
/// (`HH:MM`, no seconds — [`crate::status::sample::ClockReading`]), so the
/// amortised cost is 3.4 KiB/s of bus traffic and one extra textured quad per
/// frame. Forcing the CPU path for a clock would cost the whole zero-copy win.
/// The upload itself is the caller's ([`crate::backend::winit`]), keyed on the
/// strip's generation, so this function stays a pure executor of a draw list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn present_human_visible(
    renderer: &mut GlesRenderer,
    framebuffer: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    geom: ViewGeometry,
    transform: Transform,
    content: &GpuContent,
    indicator: TrustedIndicator,
    agent_cursor: Option<(f64, f64)>,
    human_cursor: Option<(f64, f64)>,
    status: Option<&GlesTexture>,
) -> Result<SyncPoint, GlesError> {
    let (vw, vh) = geom.output();
    let view: Size<i32, Physical> = (vw as i32, vh as i32).into();
    let mut frame = renderer.render(framebuffer, view, transform)?;
    for draw in human_visible_frame(
        geom,
        (content.width, content.height),
        indicator,
        agent_cursor,
        human_cursor,
    ) {
        match draw {
            // An empty cursor slot. Nothing to submit, and deliberately not
            // an error: a frame with no agent cursor has four of them.
            Draw::Nothing => {}
            Draw::Matte => frame.clear(letterbox_color(), &[Rectangle::from_size(view)])?,
            // Qualified calls: `GlesFrame` has inherent methods of both
            // names (extra custom-shader arguments on one, no blend
            // handling on the other) that would shadow the
            // renderer-agnostic trait methods.
            Draw::Content(dst) => Frame::render_texture_from_to(
                &mut frame,
                &content.texture,
                Rectangle::from_size(content.texture.size().to_f64()),
                dst,
                // Damage is DST-LOCAL in this call (Smithay 0.7 constrains
                // each rect into `dst.size`, then translates by `dst.loc` —
                // its own damage tracker likewise subtracts the element
                // location before drawing): full-dst damage draws the whole
                // placed rectangle and the rasterizer clips it to the view,
                // so a larger-than-view surface center-crops. The view
                // rectangle would be wrong here — under a negative placement
                // it shifts left/up and leaves right/bottom strips of matte
                // where client pixels belong.
                &[Rectangle::from_size(dst.size)],
                &[],
                Transform::Normal,
                1.0,
            )?,
            // The band clip in `agent_cursor_rects` can empty a rectangle
            // (a sprite aimed at row 0), and a partially off-view sprite
            // arrives with off-canvas coordinates the rasterizer clips —
            // but a zero-extent draw is skipped here rather than submitted,
            // so no GL call is made with a degenerate rectangle.
            Draw::AgentCursor(rect, rgba) | Draw::HumanCursor(rect, rgba) => {
                if rect.size.w > 0 && rect.size.h > 0 {
                    Frame::draw_solid(
                        &mut frame,
                        rect,
                        // Dst-local, exactly as above.
                        &[Rectangle::from_size(rect.size)],
                        color32f(rgba),
                    )?;
                }
            }
            // The strip's texture is 1:1 with its rectangle by construction —
            // it was rasterized at this view's width and the configured height
            // — so this is a blit, never a scale. A slot with no texture behind
            // it is skipped rather than drawn black: `human_visible_frame`
            // only emits this draw when `status_h > 0`, which only happens
            // when the caller passed a texture, so the `else` is unreachable
            // defence rather than a mode.
            Draw::StatusStrip(rect) => {
                if let Some(texture) = status {
                    Frame::render_texture_from_to(
                        &mut frame,
                        texture,
                        Rectangle::from_size(texture.size().to_f64()),
                        rect,
                        // Dst-local, exactly as the content blit above.
                        &[Rectangle::from_size(rect.size)],
                        &[],
                        Transform::Normal,
                        1.0,
                    )?;
                }
            }
            Draw::TrustBand(rect, rgba) => Frame::draw_solid(
                &mut frame,
                rect,
                // Dst-local, exactly as above.
                &[Rectangle::from_size(rect.size)],
                color32f(rgba),
            )?,
        }
    }
    // **Handed back rather than dropped** (WS-E.3.2). The nested backend's
    // EGL swapchain synchronises its own frame and ignores this, but a
    // scanout buffer does not: `GbmBufferedSurface::queue_buffer` wants the
    // fence so the display controller does not read a buffer the GPU has not
    // finished writing. Returning it keeps that decision at the call site
    // that knows which target it is drawing into.
    frame.finish()
}

/// An opaque RGBA8888 core colour as the renderer's float colour. One
/// conversion for every colour this module puts on a GPU frame, so a colour
/// the CPU compositor and the GPU path both paint cannot fork between them
/// in the conversion — which for the trusted indicator would be a new
/// forgery surface, not a rounding nit.
fn color32f(rgba: [u8; 4]) -> smithay::backend::renderer::Color32F {
    smithay::backend::renderer::Color32F::new(
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    )
}

/// [`crate::scene::LETTERBOX_RGBA`] as the renderer's float clear color —
/// derived, not restated, so the matte can never fork between the CPU and
/// GPU paths.
fn letterbox_color() -> smithay::backend::renderer::Color32F {
    color32f(LETTERBOX_RGBA)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::{AsFd, OwnedFd};

    use rustix::fs::MemfdFlags;

    use super::*;

    /// **The two kinds of render target take opposite transforms**, and the
    /// difference is the whole of the defect first light found.
    ///
    /// [`MEMORY_TARGET_TRANSFORM`] is what an offscreen renderbuffer and a
    /// scanout buffer take; `backend::winit::WINDOW_TRANSFORM` is what an EGL
    /// window surface takes. A change that made them equal would mean somebody
    /// had decided the distinction does not exist — which is exactly the
    /// belief that put the human's whole display upside down.
    #[test]
    fn a_memory_target_and_a_window_surface_are_opposite_kinds() {
        assert_eq!(MEMORY_TARGET_TRANSFORM, Transform::Normal);
        assert_ne!(
            MEMORY_TARGET_TRANSFORM,
            crate::backend::winit::WINDOW_TRANSFORM,
            "a memory-backed target is addressed from its first row of storage and a window \
             surface is not; if these two ever agree, one of them is presenting mirrored"
        );
    }

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
        //
        // Takes the fd-quiescence lock like every other test in this crate
        // that opens a descriptor: it was the last one that did not, and
        // the two descriptors it holds live across its assertions were
        // landing inside other tests' `/proc/self/fd` measurements (issue
        // #74's intermittent `fd_count_returns_to_baseline`).
        let _fd = crate::capture::tests::fd_lock();
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

    /// **No zero-copy frame reaches a display without the trusted band.**
    ///
    /// The regression this exists for shipped: P1.3.5's zero-copy branch did
    /// `bind → blit → submit` and never went through the CPU output stage
    /// that paints the band, so every dmabuf-presented frame was made
    /// entirely of pixels the confined client owns — the client could
    /// rasterize a counterfeit band into the top of its own buffer with
    /// nothing genuine above it, which is precisely the forgery the trusted
    /// indicator exists to make impossible (issue #85). Nothing in the suite
    /// noticed.
    ///
    /// Presenting needs a GPU, so what is pinned here is the decision
    /// [`present_human_visible`] executes: the draw list. It is the same
    /// split `backend::winit`'s `window_pixels` was given, for the same
    /// reason.
    ///
    /// The band's rectangle is asserted against numbers derived from the
    /// *inputs*, never against [`trust_band_rect`]'s own return value.
    /// Comparing the last draw to `Draw::TrustBand(trust_band_rect(size), _)`
    /// re-runs the function under test to compute the expectation, so a
    /// `trust_band_rect` returning a zero-sized rectangle passed it — this
    /// test, and its `backend::winit` sibling, were both vacuous on geometry
    /// when they first shipped, which is exactly the class of gate this
    /// change exists to stop merging.
    #[test]
    fn every_zero_copy_frame_ends_with_the_trusted_band() {
        let indicator = TrustedIndicator::from_rgb(0x11, 0x22, 0x33);
        // Exact fit, letterboxed, center-cropped on one axis, center-cropped
        // on both, and a view shorter than the band itself.
        for (view, content) in [
            ((800, 600), (800, 600)),
            ((800, 600), (320, 240)),
            ((800, 600), (1024, 240)),
            ((800, 600), (1024, 900)),
            ((800, 4), (800, 4)),
        ] {
            let size: Size<i32, Physical> = (view.0, view.1).into();
            // Both cursor postures: a frame with no cursor at all, and one
            // with a sprite parked wherever an agent liked. Neither may cost
            // the band its slot. The **human** sprite (WS-E.3.2) is fed the
            // same positions in the same loop, because a bare-metal frame
            // carries both and the band's slot must survive either.
            for cursor in [None, Some((10.0, 10.0)), Some((0.0, 0.0))] {
                let draws = human_visible_frame(
                    (size.w.max(0) as u32, size.h.max(0) as u32).into(),
                    content,
                    indicator,
                    cursor,
                    cursor,
                );
                assert_eq!(draws[0], Draw::Matte, "{view:?}/{content:?}");
                assert!(
                    matches!(draws[1], Draw::Content(_)),
                    "{view:?}/{content:?}: the client's texture must be drawn"
                );
                // Last, so neither the client's own content nor the agent's
                // cursor can sit over the one strip the human reads the
                // session colour from.
                let last = *draws.last().expect("the draw list is never empty");
                let Draw::TrustBand(rect, rgba) = last else {
                    panic!(
                        "{view:?}/{content:?}: every human-visible GPU frame must end with the \
                         trusted band, got {last:?}"
                    )
                };
                assert!(
                    !draws[..HUMAN_VISIBLE_DRAWS - 1]
                        .iter()
                        .any(|draw| matches!(draw, Draw::TrustBand(..))),
                    "{view:?}/{content:?}: the band is drawn once, and last"
                );
                assert_eq!(
                    rgba,
                    indicator.color(),
                    "{view:?}/{content:?}: the band must carry this session's colour"
                );
                assert_eq!(
                    (rect.loc.x, rect.loc.y),
                    (0, 0),
                    "{view:?}/{content:?}: the band hugs the top-left corner"
                );
                assert_eq!(
                    rect.size.w, view.0,
                    "{view:?}/{content:?}: a band narrower than the view leaves a strip of \
                     client-owned pixels where the human reads the session colour"
                );
                assert_eq!(
                    rect.size.h,
                    (TRUST_BAND_HEIGHT as i32).min(view.1),
                    "{view:?}/{content:?}: the band is the CPU path's height, clamped only by \
                     a view shorter than the band itself"
                );
                assert!(
                    rect.size.h > 0,
                    "{view:?}/{content:?}: a zero-height band is no band at all"
                );
                // No cursor slot may overlap the band's rows, at any position
                // an agent can ask for -- nor at any position the human's own
                // pointer can reach, which is the same clip through the same
                // geometry function.
                for draw in draws {
                    if let Draw::AgentCursor(cursor, _) | Draw::HumanCursor(cursor, _) = draw {
                        assert!(
                            cursor.size.h == 0 || cursor.loc.y >= rect.size.h,
                            "{view:?}/{content:?}: a cursor reached the trusted \
                             band: {cursor:?} against {rect:?}"
                        );
                    }
                }
            }
        }
    }

    /// The unused-arm check the fixed-length draw list makes possible: a
    /// frame with no agent cursor carries exactly four empty slots, and one
    /// with a cursor carries four filled ones. Written because the alternative
    /// — a `Vec` whose length varies — makes "the band is the last draw"
    /// unassertable without also knowing how many cursor rectangles happened
    /// to be produced.
    #[test]
    fn the_cursor_slots_are_filled_only_when_an_agent_cursor_is_shown() {
        let indicator = TrustedIndicator::from_rgb(0x11, 0x22, 0x33);
        let size: Size<i32, Physical> = (800, 600).into();
        let count = |cursor| {
            human_visible_frame(
                (size.w.max(0) as u32, size.h.max(0) as u32).into(),
                (400, 300),
                indicator,
                cursor,
                None,
            )
            .iter()
            .filter(|draw| matches!(draw, Draw::AgentCursor(..)))
            .count()
        };
        assert_eq!(count(None), 0);
        assert_eq!(
            count(Some((100.0, 100.0))),
            crate::cursor::AGENT_CURSOR_RECTS
        );
        // A position that is not a number draws no sprite rather than a
        // sprite at an arbitrary place.
        assert_eq!(count(Some((f64::NAN, 100.0))), 0);
    }

    /// **The human's sprite has slots of its own, and filling them displaces
    /// neither the agent's nor the band's** (WS-E.3.2, issue #218).
    ///
    /// The sibling of the test above, and it exists because the failure it
    /// guards is silent: slot arithmetic that overlapped the two cursors
    /// would make a bare-metal frame drop one sprite whenever both were on
    /// screen, and every existing assertion here would still pass.
    #[test]
    fn the_human_cursor_has_its_own_slots_and_takes_nobody_elses() {
        let indicator = TrustedIndicator::from_rgb(0x11, 0x22, 0x33);
        let size: Size<i32, Physical> = (800, 600).into();
        let at = Some((100.0, 100.0));

        // Human only: the agent's slots stay empty.
        let human_only = human_visible_frame(
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
            (400, 300),
            indicator,
            None,
            at,
        );
        assert_eq!(
            human_only
                .iter()
                .filter(|d| matches!(d, Draw::HumanCursor(..)))
                .count(),
            crate::cursor::HUMAN_CURSOR_RECTS
        );
        assert!(!human_only
            .iter()
            .any(|d| matches!(d, Draw::AgentCursor(..))));

        // Both at once -- the bare-metal frame -- and every slot of both is
        // filled, with the band still last.
        let both = human_visible_frame(
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
            (400, 300),
            indicator,
            at,
            at,
        );
        assert_eq!(
            both.iter()
                .filter(|d| matches!(d, Draw::AgentCursor(..)))
                .count(),
            crate::cursor::AGENT_CURSOR_RECTS
        );
        assert_eq!(
            both.iter()
                .filter(|d| matches!(d, Draw::HumanCursor(..)))
                .count(),
            crate::cursor::HUMAN_CURSOR_RECTS
        );
        assert!(matches!(both[HUMAN_VISIBLE_DRAWS - 1], Draw::TrustBand(..)));
        // The human's slots come after the agent's, so an overlapping human
        // pointer is drawn on top of an agent's crosshair rather than under
        // it.
        let first_human = both
            .iter()
            .position(|d| matches!(d, Draw::HumanCursor(..)))
            .expect("filled above");
        let last_agent = both
            .iter()
            .rposition(|d| matches!(d, Draw::AgentCursor(..)))
            .expect("filled above");
        assert!(last_agent < first_human);
    }

    /// **The GPU strip and the CPU strip are the same strip** (WS-E.2.3,
    /// issue #215).
    ///
    /// The band's own test's discipline, applied to the surface directly below
    /// it: assert against what the CPU compositor **actually paints** — the
    /// footprint [`crate::status::StatusStrip::composite_over`] changes on a
    /// known buffer — rather than against the constants both paths happen to
    /// read. Comparing the rectangle to [`status_strip_rect`]'s own return value
    /// would be vacuous: a version of that function returning a zero-sized
    /// rectangle would pass.
    #[test]
    fn the_gpu_strip_covers_exactly_what_the_cpu_strip_paints() {
        use crate::status::{StatusConfig, StatusStrip};

        const W: u32 = 64;
        const H: u32 = 64;

        let mut strip = StatusStrip::new(StatusConfig {
            enabled: true,
            ..StatusConfig::default()
        });
        strip.refresh(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_786_244_643),
            std::time::Instant::now(),
            None,
        );

        // A view of a colour the strip is not, so every changed pixel is the
        // strip and only the strip.
        let mut view = vec![0u8; W as usize * H as usize * BYTES_PER_PIXEL];
        for px in view.chunks_exact_mut(BYTES_PER_PIXEL) {
            px.copy_from_slice(&[0x00, 0x80, 0x00, 0xff]);
        }
        let before = view.clone();
        strip.composite_over(&mut view, W, H);

        let rect = status_strip_rect((W as i32, H as i32).into(), strip.height());
        let mut painted_any = false;
        for y in 0..H {
            for x in 0..W {
                let off = (y as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
                let painted =
                    view[off..off + BYTES_PER_PIXEL] != before[off..off + BYTES_PER_PIXEL];
                painted_any |= painted;
                let inside = (x as i32) < rect.loc.x + rect.size.w
                    && (x as i32) >= rect.loc.x
                    && (y as i32) >= rect.loc.y
                    && (y as i32) < rect.loc.y + rect.size.h;
                assert_eq!(
                    painted, inside,
                    "({x},{y}): the GPU strip rectangle {rect:?} must be exactly the footprint \
                     the CPU strip paints"
                );
            }
        }
        assert!(
            painted_any,
            "the CPU strip painted nothing, so the equality above compared two empty sets"
        );
        // Pinned against the VIEW's numbers and the band's constant, never
        // against this function's own output.
        assert_eq!(
            rect.loc,
            (0, crate::consent::TRUST_BAND_HEIGHT as i32).into()
        );
        assert_eq!(rect.size.w, W as i32);
        assert_eq!(rect.size.h, crate::status::DEFAULT_HEIGHT as i32);
        // It cannot overlap the band, whatever the height is asked for.
        assert!(rect.loc.y >= trust_band_rect((W as i32, H as i32).into()).size.h);
    }

    /// A view too short to hold the whole strip clips it rather than reaching
    /// past the buffer, and a view shorter than the band leaves no strip at
    /// all.
    #[test]
    fn a_short_view_clips_the_strip_and_never_overruns_it() {
        for h in 0..40i32 {
            let rect = status_strip_rect((32, h).into(), crate::status::DEFAULT_HEIGHT);
            assert!(rect.loc.y >= 0 && rect.size.h >= 0, "h={h}: {rect:?}");
            assert!(
                rect.loc.y + rect.size.h <= h.max(0),
                "h={h}: the strip {rect:?} runs past the view"
            );
        }
    }

    /// **The band is still the last draw, with or without a strip**, and the
    /// strip slot is empty exactly when `--status` is off.
    #[test]
    fn the_strip_joins_the_draw_list_without_displacing_the_band() {
        let indicator = TrustedIndicator::from_rgb(0x7F, 0x10, 0xC0);
        let size: Size<i32, Physical> = (640, 480).into();

        let off = human_visible_frame(
            (size.w.max(0) as u32, size.h.max(0) as u32).into(),
            (400, 300),
            indicator,
            None,
            None,
        );
        assert!(
            matches!(off[HUMAN_VISIBLE_DRAWS - 1], Draw::TrustBand(..)),
            "the band must be the last draw"
        );
        assert!(
            !off.iter().any(|d| matches!(d, Draw::StatusStrip(_))),
            "a session with no strip must emit no strip draw"
        );

        let on = human_visible_frame(
            ViewGeometry::new((size.w.max(0) as u32, size.h.max(0) as u32), 20),
            (400, 300),
            indicator,
            None,
            None,
        );
        assert_eq!(
            off[HUMAN_VISIBLE_DRAWS - 1],
            on[HUMAN_VISIBLE_DRAWS - 1],
            "the strip must not change the band draw"
        );
        assert!(
            matches!(on[HUMAN_VISIBLE_DRAWS - 1], Draw::TrustBand(..)),
            "the band must still be last with a strip up"
        );
        // The strip sits at slot 2 -- after the matte and the client content,
        // BEFORE the cursor rects. It moved there when the GPU path was
        // corrected to match the CPU one, where `composite_agent_cursor` runs
        // after the strip: the two backends had disagreed about whether the
        // crosshair or the clock is on top, and the crosshair wins because it
        // is the signal that an agent is pointing.
        assert_eq!(
            on[2],
            Draw::StatusStrip(Rectangle::new(
                (0, crate::consent::TRUST_BAND_HEIGHT as i32).into(),
                (640, 20).into()
            )),
            "the strip is drawn after the content and before the cursor"
        );
        // ...and it is drawn AFTER the client content, so a client cannot
        // cover it.
        let strip_at = on
            .iter()
            .position(|d| matches!(d, Draw::StatusStrip(_)))
            .expect("a strip draw");
        let content_at = on
            .iter()
            .position(|d| matches!(d, Draw::Content(_)))
            .expect("a content draw");
        assert!(strip_at > content_at);
    }

    /// The GPU band and the CPU band are the same band.
    ///
    /// The colour and the geometry are both anti-forgery properties, so a
    /// GPU path that derived either of them independently would be a second
    /// indicator the human has no reason to trust. This asserts against what
    /// the CPU compositor *actually paints* — the footprint
    /// `ConsentSurface::composite_trust_band` changes on a known buffer —
    /// rather than against the constants both happen to read.
    #[test]
    fn the_gpu_band_covers_exactly_what_the_cpu_band_paints() {
        use crate::consent::ConsentSurface;

        const W: u32 = 64;
        const H: u32 = 40;
        let indicator = TrustedIndicator::from_rgb(0x7F, 0x10, 0xC0);

        // A view of a colour the band is not, so every changed pixel is the
        // band and only the band.
        let mut view = vec![0u8; W as usize * H as usize * BYTES_PER_PIXEL];
        for px in view.chunks_exact_mut(BYTES_PER_PIXEL) {
            px.copy_from_slice(&[0x00, 0x80, 0x00, 0xff]);
        }
        let before = view.clone();
        ConsentSurface::new(indicator).composite_trust_band(&mut view, W, H);

        let rect = trust_band_rect((W as i32, H as i32).into());
        for y in 0..H {
            for x in 0..W {
                let off = (y as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
                let px = &view[off..off + BYTES_PER_PIXEL];
                let painted = px != &before[off..off + BYTES_PER_PIXEL];
                let inside = (x as i32) < rect.size.w && (y as i32) < rect.size.h;
                assert_eq!(
                    painted, inside,
                    "({x},{y}): the GPU band rectangle {rect:?} must be exactly the \
                     footprint the CPU band paints"
                );
                if inside {
                    assert_eq!(
                        px,
                        indicator.color(),
                        "({x},{y}): the CPU band's colour is the value the GPU path \
                         hands to the renderer"
                    );
                }
            }
        }
        assert_eq!(rect.loc.x, 0, "the band hugs the top-left corner");
        assert_eq!(rect.loc.y, 0);
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

    /// Whether the env gate is set — belt to the `#[ignore]` braces, so
    /// `--ignored` runs on a GPU-less box degrade to a loud skip instead of
    /// a failure.
    ///
    /// A [`vitrin_skip::Verdict`] rather than the `Option<()>` this was
    /// (#288). The old shape was `let Some(()) = env_gate() else { return }`
    /// — a bare `return` in a test body, in a module NO CI job compiles, so
    /// nothing anywhere would have noticed it becoming unconditional. The
    /// verdict is opaque: it cannot be matched, so that shape no longer
    /// type-checks, here or in a copy of it somebody writes next year.
    fn gpu_tests_requested() -> vitrin_skip::Verdict {
        vitrin_skip::Verdict::capable_if(
            std::env::var_os("VITRIN_GPU_TESTS").is_some(),
            "set VITRIN_GPU_TESTS=1 to run the real-GPU dmabuf tests",
        )
    }

    /// Whether this renderer imports the format the acceptance measures.
    ///
    /// Linear-import support is a per-GPU reality (plan risk R3); a driver
    /// without it exercises the fallback path, not this test.
    fn imports_linear_xrgb(supported: bool) -> vitrin_skip::Verdict {
        vitrin_skip::Verdict::capable_if(
            supported,
            "this renderer does not import XRGB8888+LINEAR dmabufs",
        )
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

    /// The indicator the GPU harness presents with. Fixed and vivid, and —
    /// like every other pixel assertion in this crate — nothing the
    /// deterministic frame generator can produce, so a band read back in
    /// these bytes is the band and not client content.
    fn harness_indicator() -> TrustedIndicator {
        TrustedIndicator::from_rgb(0xFF, 0x00, 0xAA)
    }

    /// Present the retained GPU content at the view size through the **real**
    /// presentation entry point and read the result back as tightly packed
    /// RGBA. The readback is test apparatus; [`present_human_visible`] is
    /// not — it is the same function the nested backend's zero-copy branch
    /// calls, which is why these expectations include the trusted band.
    ///
    /// [`MEMORY_TARGET_TRANSFORM`] because an offscreen renderbuffer read back
    /// with `copy_framebuffer` is already top-down; the window surface the
    /// nested backend binds is not, and passes `WINDOW_TRANSFORM` instead.
    /// Read from the shared constant rather than spelled `Transform::Normal`
    /// here, so this harness and the bare-metal scanout path cannot come to
    /// disagree about which kind of target they are.
    /// **It presents at a real [`ViewGeometry`], not a bare size** (issue
    /// #304): `--status` off, so the reservation is the trusted band's rows
    /// and nothing else — the geometry every default session has. That is why
    /// the expectations below place the client's pixels below the reserved
    /// rows rather than at `y = 0`.
    fn composite_and_readback(renderer: &mut GlesRenderer, content: &GpuContent) -> Vec<u8> {
        let geom = crate::view::ViewGeometry::new((W, H), 0);
        let mut target: GlesRenderbuffer = Offscreen::<GlesRenderbuffer>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            (W as i32, H as i32).into(),
        )
        .expect("offscreen target");
        let mut fb = renderer.bind(&mut target).expect("bind");
        // The sync point is dropped: this harness reads the result back with
        // `copy_framebuffer` on the same context, which orders itself. A
        // scanout buffer does not, which is why the caller is handed the
        // fence rather than the function swallowing it.
        let _sync = present_human_visible(
            renderer,
            &mut fb,
            geom,
            MEMORY_TARGET_TRANSFORM,
            content,
            harness_indicator(),
            // No agent cursor: these expectations are about the client's own
            // pixels and the band above them. The sprite's own two-path
            // equality is pinned without a GPU, in `backend::winit`'s
            // `no_presentation_path_can_drop_the_agent_cursor`.
            None,
            // ...and no human cursor: this harness stands in for the nested
            // backend, which never draws one (the host desktop does).
            None,
            // ...and no status strip, for the same reason: `--status` is
            // opt-in, these expectations are about the client's pixels, and
            // the strip's own two-path geometry is pinned without a GPU by
            // `the_gpu_strip_covers_exactly_what_the_cpu_strip_paints`.
            None,
        )
        .expect("present retained content");
        let mapping = renderer
            .copy_framebuffer(
                &fb,
                Rectangle::from_size((W as i32, H as i32).into()),
                Fourcc::Abgr8888,
            )
            .expect("copy framebuffer");
        renderer.map_texture(&mapping).expect("map").to_vec()
    }

    /// Overwrite the top [`TRUST_BAND_HEIGHT`] rows of a `W x H` expectation
    /// with the harness indicator's colour.
    ///
    /// Every human-visible GPU frame ends with the trusted band (issue #85),
    /// so an expectation built from client pixels alone describes a frame
    /// the core must never present. This is the one place the GPU goldens
    /// account for it.
    fn with_trust_band(mut expected: Vec<u8>) -> Vec<u8> {
        let rows = (TRUST_BAND_HEIGHT as usize).min(H as usize);
        let band = &harness_indicator().color();
        for px in expected[..rows * W as usize * BYTES_PER_PIXEL].chunks_exact_mut(BYTES_PER_PIXEL)
        {
            px.copy_from_slice(band);
        }
        expected
    }

    /// The M1.5 acceptance: on a real GPU, shim→core frames are zero-copy —
    /// end to end over the real wire (mock shim, socketpair, `ShimServer`),
    /// with the copy meter as the instrumented proof — and the deferred
    /// release semantics hold, and the shm fallback still works afterwards.
    #[test]
    #[ignore = "requires a real GPU (EGL + DRM render node); VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf"]
    fn real_gpu_dmabuf_frames_are_zero_copy_end_to_end() {
        vitrin_skip::skip_unless!(vitrin_skip::GPU, gpu_tests_requested());
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
        vitrin_skip::skip_unless!(vitrin_skip::GPU, imports_linear_xrgb(supported));

        let (mut core, shim_conn) = Connection::pair().expect("socketpair");
        let mut server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            geom: (W, H).into(),
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
            // client's own buffer, byte-exact against the generator —
            // everywhere the trusted band does not overdraw them (issue #85:
            // the band is on *every* human-visible frame, and this is the
            // human-visible presentation function).
            //
            // **Placed below the reserved rows since issue #304.** This mock
            // shim commits the view's full `W x H` while the app is
            // configured for the usable `W x (H - 8)`, so `ViewGeometry::place`
            // centres it four rows LOWER than it used to: view rows `[0, 4)`
            // are matte, view rows `[4, H)` carry generator rows `[0, H - 4)`,
            // and the band then overdraws view rows `[0, 8)`. The offset is
            // spelled out rather than read from `ViewGeometry::place`, on the
            // same reasoning as `shim.rs`'s fixtures: an expectation that
            // asked the production placement would agree with a broken one.
            const CONTENT_TOP: usize = 4;
            let row = W as usize * BYTES_PER_PIXEL;
            let generated = frame_rgba(n, W, H);
            let mut expected = LETTERBOX_RGBA.repeat(CONTENT_TOP * W as usize);
            expected.extend_from_slice(&generated[..(H as usize - CONTENT_TOP) * row]);
            let composed =
                composite_and_readback(&mut renderer, content.as_ref().expect("content retained"));
            assert_eq!(
                composed,
                with_trust_band(expected),
                "composited frame {n} must be the exact generator output, placed below the \
                 rows the core reserves, under the trusted band"
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
        // **The shm fallback attaches at the CONFIGURED size, not the
        // output's** (issue #304), and both numbers below follow from that.
        // `MockShim::attach_frame` sizes its memfd from the `configure` it
        // received, and since #304 that carries `ViewGeometry::usable()` --
        // the output minus the rows the core reserves. So this frame is
        // `W x uh`, it lands *whole* at `reserved_top` with no crop, and the
        // realm view is the reserved rows of matte followed by all of it.
        // (The dmabuf frames above differ on purpose: `gbm_frame` allocates
        // the full `W x H` regardless of the configure, so those centre-crop
        // by `CONTENT_TOP` instead.)
        //
        // Both assertions previously read `frame_rgba(7, W, H)` and
        // `W * H * 4` -- the pre-inset shape, in which a client filled the
        // view. Nothing caught it: this test is `#[ignore]`d behind a real
        // GPU, so neither CI nor any review round ever executed these lines.
        let geom = crate::view::ViewGeometry::new((W, H), 0);
        let (uw, uh) = geom.usable();
        let mut expected_shm = LETTERBOX_RGBA.repeat(geom.reserved_top() as usize * W as usize);
        expected_shm.extend_from_slice(&frame_rgba(7, uw, uh));
        assert_eq!(
            scene.compose(geom),
            expected_shm,
            "the shm fallback must compose the configured-size frame below the rows the \
             core reserves, not a full-view frame at the origin"
        );
        assert_eq!(
            (
                server.copy_meter().copies(),
                server.copy_meter().pixel_bytes()
            ),
            (1, u64::from(uw) * u64::from(uh) * 4),
            "the shm fallback copies exactly once, and copies the frame the app was \
             configured for"
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
        vitrin_skip::skip_unless!(vitrin_skip::GPU, gpu_tests_requested());
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
            geom: (W, H).into(),
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
    /// than the view (legal mid-resize; [`ViewGeometry::place`] goes negative)
    /// must fill the **whole** view with the client's central pixels, 1:1 —
    /// exactly what `Scene::compose` does on the CPU path. Pins the
    /// dst-local damage contract of `Frame::render_texture_from_to`:
    /// passing view-space damage instead shifts the drawn region by the
    /// negative placement and leaves right/bottom strips of letterbox
    /// matte where client pixels belong.
    #[test]
    #[ignore = "requires a real GPU (EGL + DRM render node); VITRIN_GPU_TESTS=1 cargo test -p vitrin-core --features gpu-tests -- --ignored dmabuf"]
    fn real_gpu_oversized_dmabuf_center_crops_the_full_view() {
        vitrin_skip::skip_unless!(vitrin_skip::GPU, gpu_tests_requested());
        let _fd = crate::capture::tests::fd_lock();
        let Some((mut renderer, gbm, node)) = gpu_harness() else {
            panic!("VITRIN_GPU_TESTS=1 but no EGL device with a working GBM pipeline was found");
        };
        eprintln!("running on {}", node.display());

        // Larger than the view on both axes, asymmetrically, so both
        // center-crop offsets are negative and different: placement is
        // (-32, -14) for the 96x64 view. `x` is `(W - SW) / 2` as it always
        // was; `y` is that same centring inside the USABLE `96x56` rectangle
        // translated down by the 8 reserved rows — `(56 - SH) / 2 + 8 = -14`,
        // where it was `(H - SH) / 2 = -18` before issue #304 inset the view.
        const SW: u32 = W + 64;
        const SH: u32 = H + 36;
        const N: u32 = 5;

        let mut content: Option<GpuContent> = None;
        let mut importer = GlesDmabufImporter {
            renderer: &mut renderer,
            content: &mut content,
        };
        vitrin_skip::skip_unless!(
            vitrin_skip::GPU,
            imports_linear_xrgb(importer.supports(Format::Xrgb8888))
        );
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
        // Expected: the W x H window the placement selects, row-extracted
        // from the same deterministic generator the buffer was filled from.
        // `cx` is the horizontal centre as before; `cy` is **14, not 18** —
        // the inset moved the placement four rows down, so the view samples
        // the buffer four rows EARLIER (issue #304). Spelled literally rather
        // than read from `ViewGeometry::place`, so a broken placement cannot
        // agree with this expectation.
        let full = frame_rgba(N, SW, SH);
        let (cx, cy) = (((SW - W) / 2) as usize, 14usize);
        let row = W as usize * BYTES_PER_PIXEL;
        let mut expected = Vec::with_capacity(row * H as usize);
        for y in 0..H as usize {
            let off = ((cy + y) * SW as usize + cx) * BYTES_PER_PIXEL;
            expected.extend_from_slice(&full[off..off + row]);
        }
        assert_eq!(
            composed,
            with_trust_band(expected),
            "the view must be the client's central {W}x{H} window, 1:1 — no matte strips — \
             under the trusted band"
        );
    }
}
