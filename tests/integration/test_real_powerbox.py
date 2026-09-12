# SPDX-License-Identifier: Apache-2.0
"""P2.6.6 (issue #190): a real agent designates a real file through the real
core-drawn picker, and the descriptor it receives is the row the picker put in
front of the human -- no mock on any seam, no in-process runtime.

Everything below drives `target/debug/vitrind` over a real Unix socket, with a
real `vitrin-shim` and a real `click-target` in the realm, and reads the
delivered descriptor with `fstat`/`pread`/`pwrite`. The picker is driven
through the `consent-injector` channel's four picker verbs (`list`,
`navigate`, `confirm`, `cancel`), which stand in for a human's keyboard on a
headless runner exactly as `decide` stands in for a human's mouse in
`test_real_consent.py`.

# This is a PROPERTY gate, not a milestone gate

Deliberately, and it is worth being exact about why. `CLAUDE.md`'s
definition-of-done rule binds milestones `M1.2`-`M1.5`, each of which names its
own gate; and `docs/plan/02-phase-2-semantic-epochs.md` names **P2.6.9**
(`test_real_ransomware.py`, not yet written) as M2.5's gate, not this file. So
citing this run as "M2.5 is done" would be the exact class of overclaim the
suite's README exists to prevent. What it *is*: the mock-free acceptance gate
for issue #190, on the terms `test_launch.py` and `test_two_realms.py` already
set for their own workstream tasks.

Two things P2.6.6's plan-cell criterion asks for that live elsewhere, stated
here rather than left to be discovered:

- **The path racer.** The cell asks for "a repo-authored racer [that] swaps a
  component of the chosen path between the human's confirm and the open". That
  window is sub-millisecond and unsynchronisable from a test process, so it is
  proved in-crate, against the private resolver, in
  `crates/vitrin-core/src/picker/resolve.rs`'s own tests -- where the swap can
  be sequenced rather than raced. What *this* gate covers is the half a racer
  cannot reach: over a real socket, through a real picker, the delivered
  descriptor's `(st_dev, st_ino)` is the inode of the row the channel said was
  selected, and the resolver's defining flag is exercised by a real symlink row
  that is refused rather than followed (`RESOLVE_NO_SYMLINKS` -> `ELOOP` ->
  `unresolvable`).
- **The `openat2` floor.** "Where openat2 is unavailable the picker REFUSES to
  designate" is a claim about kernels below 5.6. Nothing here can produce one;
  `picker/resolve.rs::probe` and its tests own it.

# What this gate proves, end to end

1. **A real agent designates a real file.** An SDK `request_file` against the
   shipped `vitrind` raises a real core-drawn picker; `navigate`/`confirm`
   move and commit the selection; a descriptor arrives over `SCM_RIGHTS` --
   asserted as a descriptor, by counting this process's own `/proc/self/fd`
   across the ask and `fstat`-ing what appeared.
2. **The descriptor names the file the picker displayed.** The `list` reply
   names the selected row's raw filename BYTES; the delivered fd's
   `(st_dev, st_ino)` equals the pair recorded for that name **before the core
   was started**, and differs from every other row's. No path is re-resolved
   at assertion time -- re-resolving a name after the fact is the race the
   picker exists to close, so a gate that did it would prove nothing.
3. **Read (and write) through it.** The descriptor is authority, so the bytes
   are actually moved: `pread` returns the file's contents, and a
   `request_file(write=True)` designation is written through with `pwrite` and
   read back. The write half is what makes `mode` falsifiable -- an assertion
   that every designation is `read` passes on a core that ignores the ask.
4. **A refusal is a refusal.** A cancelled picker answers
   `vitrin_powerbox.refused(cancelled)` -- the code, specifically -- and **no
   descriptor**: no new fd appears in this process at all. A symlink row
   answers `refused(unresolvable)` on the same terms.
5. **The picker never reaches a capture.** With the picker up, an `observe()`
   frame is **byte-identical** to a settled control taken before it went up,
   the core-internal `--capture-dump` realm view is byte-identical to its own
   settled control, the two agree through the M1.3 gate's comparator
   (`vitrin-golden-cmp`), and the card's two most distinctive colours (the
   accent ring and the selected-row fill) appear **zero** times in either --
   having been counted, at that same instant, in the human-visible export.
   That capture is taken on a **second connection of the same principal**,
   because `request_file` blocks across human time and therefore runs on a
   thread that owns the asking connection's socket for the duration; two
   readers on one framed stream deadlock (measured). It is the same split
   `test_real_consent.py` uses for its own mid-prompt window.
6. **Provenance before absence.** Every absence above is read out of bytes
   whose provenance was established FIRST: the exported occlusion window is
   shown to BE a raster of vitrind's picker card, at exactly the rectangle the
   core named (`_assert_is_a_real_picker_raster`) -- accent ring on all four
   edges with an exact perimeter count, opaque body, buttons, antialiased text,
   and the picker's own selected-row highlight at the slot the `list` reply
   independently reported, moving by exactly `PANEL_ROW_H` when the cursor
   moves by one. This is #138's lesson restated: an absence over bytes of
   unproven origin is satisfied just as well by an empty buffer, and an export
   that never read the framebuffer once kept that gate green while printing its
   success line.
7. **The journal records each designation**, with the resolved
   `(st_dev, st_ino)` on the two that produced a descriptor and neither field
   on the two that did not, plus the mode, the grant row, the principal and the
   realm.

# The realm's half ends at the shim in THIS gate, and that is disclosed

The core sends the same descriptor twice: once to the asking agent, once to the
realm's shim as `vitrin_shim_session.designation`. The shim's half is real here
-- a real `vitrin-shim` decodes the event and its own log says so, which this
gate asserts, because that is the only witness available on the far side of
that wire. The per-realm `designation.sock` exists as of P2.6.7 (issue #191):
the shim binds it before the app is forked and relays each designation to
whichever app holds a connection on it. The realm's app in this gate is
`click-target`, which never connects, so the disposition the shim logs for
every designation here is `CLOSED, not relayed: no app is connected` -- the
app's choice, not the shim's -- and the app never receives it. The assertion
below greps for the shim's decode -- designation id, kind, mode and name
length -- and not for the disposition, so it stays a witness of the wire and
not of which app this gate happens to spawn. The app-side proof that a
powerbox-aware app receives over `designation.sock` under a real core is
P2.6.9's (`test_real_ransomware.py`); the component-level proof
against `shim/tests/mock_core.c` is `shim/tests/acceptance/designation_relay.sh`.

# What this gate does NOT prove

- **That a human at a real keyboard can drive the picker.** The injector's
  four verbs land in the same `ConsentGrab` queue a physical key lands in and
  are drained by `session::service_picker_round`, the production path -- but
  no key is pressed, headless has no input device, and the router still stacks
  `NoopHook`. Same shape as `test_real_consent.py`'s injected click and
  `test_real_deadman.py`'s SIGUSR1. The keyboard half is
  `crates/vitrin-core/src/picker/keys.rs` and `consent/grab.rs`'s own tests.
- **That the card is legible, or names the right principal.** The channel
  reports geometry and pixels, never text; `consent/render.rs`'s golden
  (`crates/vitrin-core/tests/golden/picker_card.txt`) and its sourcing tests
  hold that.
- **Unspoofability (issue #85).** This gate never learns the session's
  trusted-indicator colour, exactly as the consent gate does not: the exported
  rectangle is the card's own footprint, and the trusted ring is stroked
  strictly outside it.
- **Anything about `request_dir`.** A directory designation is a different
  chrome and a different terminal shape; it is unexercised here and no
  assertion below should be read as covering it.

# Watched failing

Mock-freeness proves what a test is wired to, never what it discriminates.
Each of these was applied to the tree, built, and confirmed RED before this
file was offered as evidence (2026-09-10, Arch, kernel 7.2.4-arch1-2):

- **Proof 5** -- point `HeadlessState::latest_frame_rgba` at
  `output_framebuffer` instead of `view_framebuffer`, so the realm-view
  readback carries the human-visible overlay -> "raising the picker changed
  346896 px of the CAPTURE path (green 25600 -> 0)".
- **Proof 6** -- make `consent_occlusion_window` return `vec![0u8; w*h*4]`
  instead of the `readback_region` result, so the framebuffer is never read ->
  the provenance control fails on the very first edge pixel, `(0,0,0,0) !=
  (77,157,224,255)`. Without it, that sabotage is invisible: this is the exact
  shape that kept #138's consent gate green while printing its success line.
- **Proof 6's discriminating half** -- make `ConsentSurface::show_picker`
  refuse to replace a picker already up, so the card goes stale -> "the drawn
  highlight must move by exactly one slot ...: 0 != 22".
- **Proof 2** -- make `PickerSession::chosen` settle from `entries.first()`
  instead of the cursor, so the fd is the first row's while the channel reports
  the second -> "the delivered descriptor is not the inode of the row the
  picker displayed (b'beta.txt'): (37, 302202) != (37, 302203)".
- **Proof 4** -- make the injector's `answer_cancel` press zero Tabs, so a
  cancel commits the listing selection -> "a cancelled picker must answer
  `refused`, not DesignatedEvent(...)".

Three more are permanent rather than one-off. `PowerboxGateStaysDiscriminating`
feeds the provenance checker an empty buffer, a flat forgery and a card whose
highlight is the wrong shape on every invocation, and must reject all three
against a fourth it accepts. `PowerboxWithoutAPickerRoot` is the positive
control for the whole file: the same core, the same grant, the same ask, with
the `[[picker]]` table removed, is refused before a card exists. And every
"this colour appears zero times in the capture" assertion is paired, in the
same run and at the same instant, with the same search over the human-visible
export, where it must be non-zero -- so a zero here is a measurement rather
than a search that finds nothing anywhere.

# Skip-or-fail policy (matches the real-app ladder)

- `VITRIN_SKIP_REAL_APP=1` -> skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` set but the shim or `click-target` is missing -> fail.
- A `vitrind` built or invoked without the injector -> fail, naming the
  feature and the rebuild, never a silent skip.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import stat
import tempfile
import threading
import time
import unittest

from harness import (
    ConsentInjector,
    CoreFailed,
    DEMO_IDENTITY,
    IntegrationTest,
    capture_dump_path,
    comm_of,
    descendant_named,
    exe_identity,
    file_identity,
    golden_cmp,
    locate_colour,
    packed_xrgb,
    require_binaries,
    shims_of,
)

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors, messages, protocol  # noqa: E402

#: The view has to be TALLER THAN THE PICKER CARD, which is the one sizing
#: constraint this gate has and the only one that bites silently. The card is
#: 560x570 against `test_real_consent.py`'s 640x480 view, so the core clamps the
#: export and logs `the card does not fit the view; exporting nothing` -- a run
#: that kept that size would find `bytes == 0` and no window to check the
#: provenance of, which is exactly the shape of evidence this gate refuses to
#: accept.
REALM_SIZE = "720x640"
REALM_WH = (720, 640)

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: `shim/tests/click_target.c`'s `TARGET_SIZE`: a centred 160x160 green square,
#: 25 600 px. The app is here to make the capture path carry something a leak
#: could be measured against, not to be clicked -- no actuation verb is
#: petitioned in this file.
TARGET_SIZE = 160
TARGET_AREA = TARGET_SIZE * TARGET_SIZE
MIN_TARGET_PIXELS = 5000

# -- the picker card's own geometry and palette ------------------------------
#
# Every constant here is pinned against `crates/vitrin-core/src/consent/render.rs`
# by `PowerboxGateStaysDiscriminating.test_the_picker_card_constants_match_the_renderer`,
# on `test_real_consent.py`'s precedent and for its reason: the provenance
# argument below is written in exact RGBA values and exact pixel arithmetic, so
# a restyled card must fail loudly at "the renderer's palette moved" rather
# than quietly at "no accent border".

#: `render::CARD_WIDTH`. The card's *height* is content-derived, so it is read
#: from the core at runtime and never hard-coded.
CARD_WIDTH = 560
BORDER = 2
PAD_X = 24
#: `render::CONTENT_X` / `CONTENT_W`, restated as arithmetic rather than as
#: numbers so a moved `PAD_X` moves these too.
CONTENT_X = BORDER + PAD_X
CONTENT_W = CARD_WIDTH - 2 * CONTENT_X
PANEL_THUMB_W = 4
#: `draw_panel`'s `slot_w = CONTENT_W - PANEL_THUMB_W - 4`.
SLOT_W = CONTENT_W - PANEL_THUMB_W - 4
PANEL_ROW_H = 22
PANEL_ROWS = 8

CARD_BG = (0x14, 0x16, 0x1C, 0xFF)
ACCENT = (0x4D, 0x9D, 0xE0, 0xFF)
BUTTON_BG = (0x22, 0x27, 0x31, 0xFF)
BUTTON_BORDER = (0x5C, 0x66, 0x78, 0xFF)
#: The selected panel row's fill -- the picker's own colour. A consent card
#: draws a panel too, but nothing in a consent card's panel MOVES when this
#: channel says `navigate down`, which is the property the assertions use.
PANEL_SELECTED_BG = (0x2B, 0x33, 0x42, 0xFF)

#: `draw_panel` fills the selected slot and then strokes it `BORDER` px in the
#: accent, and `Canvas::stroke_rect` strokes INSIDE the rect -- so the fill is
#: visible on exactly the rows and columns the stroke does not cover.
HL_ROWS = PANEL_ROW_H - 2 * BORDER
HL_COLS = SLOT_W - 2 * BORDER
HL_X0 = CONTENT_X + BORDER

#: The selected slot's accent stroke: the whole ring, and the two horizontal
#: edges of it alone.
#:
#: **The vertical edges are deliberately not asserted pixel-for-pixel, and the
#: reason is measured rather than assumed.** `draw_panel` draws the row's name
#: starting at `CONTENT_X` -- the slot rect's own left edge -- so the first
#: glyph's ink lands on top of the left stroke's two columns. Measured on this
#: gate's own fixture, 9 to 22 of those 72 vertical-stroke pixels are covered,
#: depending on the name (`alpha.txt` 13, `beta.txt` 22, `gamma.txt` 17,
#: `zlink.txt` 9). Nothing reaches the horizontal edges: the baseline is
#: centred in the slot, so ink stays inside rows 4..17 of 22. So the exact
#: claims below are "no accent anywhere else", "both horizontal edges
#: complete", and a two-sided bound on the total -- rather than one exact count
#: fitted to one fixture.
SLOT_RING = SLOT_W * PANEL_ROW_H - (SLOT_W - 2 * BORDER) * (PANEL_ROW_H - 2 * BORDER)
SLOT_RING_HORIZONTAL = 2 * BORDER * SLOT_W

#: How many distinct colours a genuinely rasterized card carries. The real
#: picker export measures 961 on this box: `render`'s text pass antialiases
#: every glyph, and the picker draws a listing's worth of them. A flat forgery
#: -- fill, ring, buttons, one highlight -- has a handful. Set far below the
#: measured value and far above any flat construction.
MIN_CARD_COLOURS = 64

#: How many consecutive identical `--capture-dump` reads mean "the app has
#: settled". The P1.9.8 gate-integrity lesson: a control capture taken while
#: the app is still painting lets the app forge the later evidence.
SETTLE_READS = 4
SETTLE_INTERVAL = 0.15

_WRONG_BUILD = (
    "This is what a `vitrind` built or invoked WITHOUT the consent injector does. Three "
    "distinguishable causes, all fatal here, never a silent skip:\n"
    "  * the core refuses `--headless --consent=interactive` BEFORE the flag is parsed -> the "
    "binary lacks the `consent-injector` cargo feature;\n"
    "  * the core rejects `--consent-injector-fd` as an unknown argument -> same cause, seen "
    "one line later;\n"
    "  * the core starts but never writes `vitrin-consent-injector 1` on the channel -> it did "
    "not adopt the descriptor.\n"
    "Rebuild with `cargo build --workspace --features vitrin-core/dead-man-injector,"
    "vitrin-core/consent-injector` -- tests/integration/run.sh does this automatically, and "
    "CI's warm-build step must pass the same feature list."
)


def _resolve_sibling(shim_bin: pathlib.Path, name: str, env_override: str) -> str | None:
    """A tool built beside the C shim, or an explicit override, or None."""
    explicit = os.environ.get(env_override)
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / name
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


def _require_shim(test: IntegrationTest) -> pathlib.Path:
    """The shared shim resolution + skip-or-fail preamble (matches the rest of
    the real-app ladder, e.g. `test_real_consent.py::_require_shim`)."""
    if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
        test.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")
    shim = os.environ.get("VITRIN_C_SHIM_BIN")
    if not shim:
        test.skipTest(
            "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
            "Build it (meson setup shim/build shim && meson compile -C shim/build) and point "
            "the variable at shim/build/vitrin-shim. CI sets it."
        )
    shim_bin = pathlib.Path(shim)
    if not (shim_bin.is_file() and os.access(shim_bin, os.X_OK)):
        test.fail(
            f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a "
            "real run was requested; refusing to skip a requested gate (CI misconfig)."
        )
    return shim_bin


# -- raw-RGBA analysis -------------------------------------------------------
#
# The occlusion window and `--capture-dump` are both tightly packed RGBA8888,
# rows top-down; a wire frame packed by `harness.packed_xrgb` is BGRX. Both are
# 4 bytes per pixel, which is all the exact-match helpers need.


def _quantised(rgb: tuple[int, int, int]) -> tuple[int, int, int]:
    return (rgb[0] & 0xF0, rgb[1] & 0xF0, rgb[2] & 0xF0)


def _count_rgba(buf: bytes, hex6: str) -> int:
    """How many pixels of a raw-RGBA buffer quantise to `hex6`.

    Quantised because the app's colours come off a real software renderer and
    through a real shim; the card's do not, and get `_exact_rgba` instead.
    """
    want = _quantised((int(hex6[0:2], 16), int(hex6[2:4], 16), int(hex6[4:6], 16)))
    hits = 0
    for off in range(0, len(buf), 4):
        if (buf[off] & 0xF0, buf[off + 1] & 0xF0, buf[off + 2] & 0xF0) == want:
            hits += 1
    return hits


def _exact_pixels(buf: bytes, want: tuple[int, int, int, int]) -> list[int]:
    """The **pixel indices** of every pixel exactly equal to `want`.

    `bytes.find` rather than a Python loop over 300 000 pixels: the card
    raster is scanned several times per ask and this is the difference between
    a gate that runs in seconds and one that runs in a minute. Misaligned
    matches (a 4-byte run straddling two pixels) are real and are discarded by
    the `% 4` test -- which is why this returns indices rather than a count
    taken from `bytes.count`, which cannot make that distinction.
    """
    pattern = bytes(want)
    out: list[int] = []
    at = buf.find(pattern)
    while at != -1:
        if at % 4 == 0:
            out.append(at // 4)
        at = buf.find(pattern, at + 1)
    return out


def _exact_rgba(buf: bytes, want: tuple[int, int, int, int]) -> int:
    return len(_exact_pixels(buf, want))


def _px(buf: bytes, width: int, x: int, y: int) -> tuple[int, int, int, int]:
    off = (y * width + x) * 4
    return (buf[off], buf[off + 1], buf[off + 2], buf[off + 3])


def _read_dump(path: str, size: tuple[int, int]) -> bytes:
    """Block until `--capture-dump`'s atomic temp+rename has produced a whole
    frame, then return its raw RGBA bytes."""
    width, height = size
    expected = width * height * 4
    p = pathlib.Path(path)
    deadline = time.monotonic() + 15.0
    while time.monotonic() < deadline:
        if p.is_file():
            data = p.read_bytes()
            if len(data) == expected:
                return data
        time.sleep(0.05)
    size_now = p.stat().st_size if p.is_file() else "absent"
    raise AssertionError(
        f"the core-internal capture at {path} never reached {expected} bytes (size: {size_now}); "
        "`--capture-dump` did not write the composited readback"
    )


def _fd_set() -> set[str]:
    """This process's open descriptor numbers.

    The whole of how "a descriptor arrived over `SCM_RIGHTS`" and "no
    descriptor arrived" are told apart here. The SDK transfers ownership of a
    designated fd to the caller and closes nothing on its behalf, so a
    descriptor that crossed the socket is sitting in this process and is
    visible in procfs; a refusal terminal carries no ancillary data at all and
    leaves this set unchanged.

    **The listing's own descriptor is excluded, and that is not tidiness.**
    `os.listdir("/proc/self/fd")` opens a directory descriptor to read the
    directory and that descriptor appears in its own result, always at the
    lowest free number. So a delivered fd takes the slot the previous sample's
    phantom occupied and the phantom moves up one, and the naive difference
    reports *the phantom's* number rather than the descriptor's: measured, a
    run whose `DesignatedEvent` carried `fd=6` reported "this process gained
    ['7']". The count happened to be right, but the identity in the failure
    message was of a descriptor that never crossed the socket -- and if the
    kernel ever hands the SCM_RIGHTS fd a number above the phantom's slot, the
    difference is two and the arm fails for a reason that is not the property.
    The phantom is identified exactly, by the one thing it is: a descriptor
    onto this process's own `/proc/<pid>/fd`. Nothing the picker can designate
    reads back as that, because the picker root is a temp directory.
    """
    here = f"/proc/{os.getpid()}/fd"
    live: set[str] = set()
    for name in os.listdir("/proc/self/fd"):
        try:
            target = os.readlink(f"/proc/self/fd/{name}")
        except OSError:
            # The listing's descriptor, already closed by the time this loop
            # reached its name. Not a swallow: the entry is gone, so there is
            # nothing to attribute to it.
            continue
        if target == here:
            continue
        live.add(name)
    return live


class _Ask:
    """One `request_file`, running while the harness drives the picker.

    `Connection.request_file` blocks for human time, by design and by its own
    docstring -- so a single-threaded test cannot both make the ask and answer
    it. This runs the ask on a daemon thread and leaves the SDK connection
    untouched by the main thread for the whole of its life, which is the
    handover the SDK's single-threaded design permits: one thread owns the
    socket at a time. The main thread meanwhile talks only to the injector,
    which is a different socket the harness owns.

    A hung ask fails as a named assertion here rather than as
    `IntegrationTest`'s nameless 90 s alarm.
    """

    def __init__(self, conn, grant, **kwargs: object) -> None:
        self._out: dict[str, object] = {}
        self._thread = threading.Thread(
            target=self._run, args=(conn, grant, kwargs), daemon=True
        )
        self._thread.start()

    def _run(self, conn, grant, kwargs) -> None:
        try:
            self._out["value"] = conn.request_file(grant, **kwargs)
        except BaseException as exc:  # noqa: BLE001 -- re-raised in `result`
            self._out["error"] = exc

    def result(self, test: unittest.TestCase, timeout: float = 30.0):
        self._thread.join(timeout)
        if self._thread.is_alive():
            test.fail(
                f"request_file did not return within {timeout:.0f}s of the picker being "
                "driven. The core owes exactly one terminal per admitted ask "
                "(`vitrin_powerbox.designated` or `refused`); nothing arrived."
            )
        if "error" in self._out:
            raise self._out["error"]  # type: ignore[misc]
        return self._out["value"]


class RealPowerboxDesignation(IntegrationTest):
    """Issue #190's mock-free gate: a real designation, end to end."""

    #: `click-target`'s colours. Green target on a black field. Nothing here
    #: clicks it -- no actuation verb is petitioned -- so it stays green for
    #: the whole run and is the content a capture-path leak would be measured
    #: against.
    TARGET = "00ff00"

    #: The picker root's contents, in the order `picker::session::read_entries`
    #: sorts them (raw bytes, ascending). Distinct lengths and distinct
    #: contents on purpose: a read that returned the wrong file's bytes, or a
    #: shim that decoded the wrong name length, fails rather than coincides.
    FILES = (
        (b"alpha.txt", b"ALPHA-0\n"),
        (b"beta.txt", b"BETA-CONTENT-1\n"),
        (b"gamma.txt", b"GAMMA-CONTENT-22\n"),
    )
    #: A symlink OUT of the root, sorting last but one. Refused by
    #: containment — but note `RESOLVE_BENEATH` alone refuses this, so it does
    #: not on its own witness `RESOLVE_NO_SYMLINKS`.
    LINK = b"zlink.txt"
    #: A symlink pointing INSIDE the root, at a file that really is there and
    #: really is designatable by its own name. `RESOLVE_BENEATH` is satisfied
    #: by it, so **only `RESOLVE_NO_SYMLINKS` refuses it** — which makes this
    #: the row that witnesses the resolver's defining flag.
    #:
    #: Added after the out-of-root row was measured NOT to: dropping
    #: `NO_SYMLINKS` from `picker::resolve`'s flag set and rebuilding left this
    #: gate green, because the escape still tripped `BENEATH` and reported
    #: EXDEV instead of ELOOP. Both are real containment; only one of them is
    #: the flag the assertion named.
    INNER_LINK = b"zzlink-inner.txt"

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        app = _resolve_sibling(self.shim_bin, "click-target", "VITRIN_CLICK_TARGET_APP")
        if app is None:
            self.fail(
                f"no click-target beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "rebuild the shim, or set VITRIN_CLICK_TARGET_APP."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-powerbox-"))
        self.addCleanup(shutil.rmtree, self.work, True)
        self.dump = str(self.work / "internal.rgba")
        self.dump_path = str(capture_dump_path(self.dump))
        self.core_log = self.work / "core.log"

        # The picker root, and the OUTSIDE file the symlink points at -- built
        # here, before any core exists.
        self.root = self.work / "docs"
        self.root.mkdir()
        self.outside = self.work / "outside-the-root.txt"
        self.outside.write_bytes(b"NEVER DESIGNATED\n")
        #: name bytes -> `(st_dev, st_ino)`, recorded **at creation time**.
        #: This is the whole of proof 2's ground truth, and it is taken before
        #: the core starts precisely so that no assertion below re-resolves a
        #: path: re-resolving a name after the fact is the race the picker
        #: exists to close.
        self.inode_of: dict[bytes, tuple[int, int]] = {}
        for name, body in self.FILES:
            path = self.root / os.fsdecode(name)
            path.write_bytes(body)
            st = path.stat()
            self.inode_of[name] = (st.st_dev, st.st_ino)
        os.symlink(self.outside, self.root / os.fsdecode(self.LINK))
        st = self.outside.stat()
        self.outside_inode = (st.st_dev, st.st_ino)
        # Points at `alpha.txt`, which the picker will happily designate under
        # its own name — so a refusal here cannot be about the target being
        # unreachable, only about the link being a link.
        os.symlink(
            os.fsdecode(self.FILES[0][0]), self.root / os.fsdecode(self.INNER_LINK)
        )

    # -- the core, under the one policy this gate is about ------------------

    def real_core(self):
        """A real chain under `--consent=interactive`, with both channels the
        gate needs: the injector, and a configured picker root.

        **The `[[picker]]` table is what makes this a deployment that serves
        designation at all.** `crates/vitrin-core/src/main.rs` has no `$HOME`
        fallback and states why: an admitted ask seizes the human's physical
        input until the ticket's deadline, so a deployment serving designations
        it cannot draw a card for would freeze the keyboard behind a blank
        screen. Without the table every ask here is refused `internal` before a
        picker exists -- which is `PowerboxWithoutAPickerRoot` below, the
        control that makes this line load-bearing rather than decorative.
        """
        try:
            return self.core(
                consent="interactive",
                size=REALM_SIZE,
                shim=str(self.shim_bin),
                command=self.app_bin,
                args=["--run-ms", "120000"],
                env_allow=tuple(WLR_ENV),
                extra_env=WLR_ENV,
                log_file=str(self.core_log),
                capture_dump=self.dump,
                consent_injector=True,
                picker_root=str(self.root),
            )
        except CoreFailed as exc:
            self.fail(f"{_WRONG_BUILD}\n\nThe core's own words:\n{exc}")

    def _assert_instrumented(self, core) -> ConsentInjector:
        """A RUNNING instrumented core, identifiable by how it was invoked --
        `test_real_consent.py::_assert_instrumented`, unchanged, plus the one
        line that says this session can serve a designation at all."""
        cmdline = pathlib.Path(f"/proc/{core.pid}/cmdline").read_bytes().split(b"\0")
        self.assertIn(
            b"--consent-injector-fd",
            cmdline,
            "an instrumented session must be identifiable from /proc/<pid>/cmdline alone",
        )
        fd = core.injector_fd
        self.assertIsNotNone(fd)
        target = os.readlink(f"/proc/{core.pid}/fd/{fd}")
        self.assertTrue(
            target.startswith("socket:"),
            f"the injector descriptor must be an unnamed socket, not {target!r}",
        )
        injector = core.injector
        assert isinstance(injector, ConsentInjector)
        try:
            banner = injector.await_banner()
        except Exception as exc:  # noqa: BLE001 -- the diagnostic is the point
            self.fail(f"{_WRONG_BUILD}\n\nThe channel said: {exc}")
        self.assertEqual(banner, "vitrin-consent-injector 1")
        log = self.core_log.read_text(errors="replace")
        self.assertIn("CONSENT INJECTOR IS WIRED", log)
        self.assertIn(
            f"picker root opened root={self.root}",
            log,
            "this session must have opened the configured picker root at STARTUP. The core "
            "probes `openat2` there before anyone connects, so a deployment below the 5.6 "
            "floor -- or one whose root cannot be opened -- says so here rather than at the "
            "moment a human is waiting for a card, and serves no designation at all.",
        )
        return injector

    def _spine(self, core) -> None:
        """Wait out `vitrind -> vitrin-shim -> click-target`, and prove the
        shim is the real one BY INODE (P2.6.2 renamed it inside the realm, so a
        name test stopped telling the real shim from `vitrin-mock-shim`)."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            if core.proc.poll() is not None:
                self.fail(
                    f"the core exited {core.proc.returncode} instead of serving.\n"
                    f"{_WRONG_BUILD}\n{core.output()}"
                )
            found = shims_of(core.pid)
            if found:
                shim_pid = found[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        self.assertEqual(
            exe_identity(shim_pid),
            file_identity(self.shim_bin),
            f"the realm's shim (pid {shim_pid}, comm {comm_of(shim_pid)!r}) is not the C shim "
            f"this gate named ({self.shim_bin}) -- vitrin-mock-shim must appear nowhere here",
        )
        self.assertIsNotNone(
            descendant_named(core.pid, "click-target", timeout=15.0),
            "the C shim never fork/exec'd click-target",
        )
        return shim_pid

    def _settle(self) -> bytes:
        """Block until the real app's frame is on screen AND unchanging.

        Returns the settled realm-view dump: this gate's control for the
        byte-equality assertions. Taking it before the app has stopped painting
        is the P1.9.8 gate-integrity failure -- a still-moving app can forge the
        later "nothing changed" evidence.
        """
        deadline = time.monotonic() + 30.0
        stable = 0
        last: bytes | None = None
        while time.monotonic() < deadline:
            frame = _read_dump(self.dump_path, REALM_WH)
            green = _count_rgba(frame, self.TARGET)
            if green >= MIN_TARGET_PIXELS and frame == last:
                stable += 1
                if stable >= SETTLE_READS:
                    return frame
            else:
                stable = 0
            last = frame
            time.sleep(SETTLE_INTERVAL)
        green = _count_rgba(last or b"", self.TARGET)
        self.fail(
            f"click-target's frame never settled within 30s: last read had {green} green px "
            f"(needed >= {MIN_TARGET_PIXELS}) and did not repeat {SETTLE_READS} times. Without "
            "a settled control, 'raising the picker moved zero pixels of the capture path' "
            "would be an assertion about a moving target."
        )

    # -- picker-side helpers ------------------------------------------------

    def _await_cursor(self, injector, want: int, timeout: float = 10.0) -> dict:
        """Poll `list` until the cursor reaches `want`.

        A poll, not a sleep: `navigate` enqueues a step into the same
        `ConsentGrab` queue a keystroke lands in, and
        `session::service_picker_round` applies it at the end of that dispatch
        round -- so the reply's `queued` is an acceptance, never an outcome
        (the channel deliberately never reports one). A fixed sleep here would
        be a race dressed as a constant.
        """
        deadline = time.monotonic() + timeout
        snap: dict = {}
        while time.monotonic() < deadline:
            snap = injector.picker()
            if snap["state"] == "shown" and snap["cursor"] == want:
                return snap
            time.sleep(0.03)
        self.fail(f"the picker's cursor never reached {want}; last `list` reply was {snap!r}")

    def _assert_is_a_real_picker_raster(
        self, window: bytes, card: tuple[int, int, int, int], cursor: int
    ) -> dict[str, object]:
        """The POSITIVE control on the exported occlusion window.

        Every absence this gate reads out of a capture is checked *after* this
        returns, and would be equally true of an empty buffer, a synthetic one,
        or a readback of some other part of the screen. The harness has no
        independent view of the human-visible framebuffer and must not have one
        (issue #85), so the provenance of these bytes has to come from the
        bytes: they have to BE a raster of vitrind's PICKER card, on exactly
        the rectangle the core named, showing exactly the row the core said was
        selected.

        Five checks, in rising strength:

        1. **The accent ring is on the exported rectangle's four edges**, and
           the pixel just inside it is the card's background. `draw_card`
           strokes `BORDER` px along the edges of the card image, so this is
           positional: a readback one pixel off the footprint fails it.
        2. **The accent appears exactly `perimeter + one highlight stroke`
           times.** A frame, not a fill -- and the second term is what makes
           this a PICKER's accent count rather than any card's.
        3. **The card body and its buttons are there**: `CARD_BG` over most of
           it, both button colours present.
        4. **The raster carries antialiased text** (`MIN_CARD_COLOURS`).
        5. **The selected-row highlight is exactly one slot-sized band.** Its
           rows are contiguous and number exactly `PANEL_ROW_H - 2*BORDER`;
           its columns are contiguous, number exactly `SLOT_W - 2*BORDER` and
           start at `CONTENT_X + BORDER`.

        **`cursor` is reported, not checked, and the split is deliberate.** The
        panel's own top is content-derived -- the card grows with the prompt
        text -- so there is no absolute row this function could compare a slot
        against without hard-coding a height the core is free to change. What
        ties the drawn highlight to the number that arrived over the channel is
        therefore a *difference* between two calls, and it is asserted by the
        caller: `shot1["hl_top"] - shot0["hl_top"] == PANEL_ROW_H` after a
        `navigate down`. That is the check no consent card and no forgery
        satisfies, and it is the one this gate watched fail (making
        `service_picker_round` skip its redraw turns it red, `0 != 22`) -- but
        it lives in the live arm, so the synthetic controls in
        `PowerboxGateStaysDiscriminating`, which call this function once, do
        not exercise it. Said here rather than left to be discovered, because
        a docstring claiming an assertion this function does not make is the
        same defect one level up from the ones this file is written to catch.

        What it does NOT prove is that some other correct picker raster was not
        substituted for this one; under issue #85 no assertion available here
        can, and pretending otherwise is the failure this docstring exists to
        avoid.

        Returns the measurements the run's summary line prints.
        """
        _, _, cw, ch = card
        self.assertEqual(len(window), cw * ch * 4, "the export is the whole card footprint")

        # 1. The ring, positionally.
        for row in list(range(BORDER)) + list(range(ch - BORDER, ch)):
            for col in range(cw):
                self.assertEqual(
                    _px(window, cw, col, row),
                    ACCENT,
                    f"the exported window's edge pixel ({col},{row}) is not the card's accent "
                    "border: these bytes are not a readback of the card at the rectangle the "
                    "core reported",
                )
        for col in list(range(BORDER)) + list(range(cw - BORDER, cw)):
            for row in range(ch):
                self.assertEqual(
                    _px(window, cw, col, row),
                    ACCENT,
                    f"the exported window's edge pixel ({col},{row}) is not the card's accent "
                    "border: these bytes are not a readback of the card at the rectangle the "
                    "core reported",
                )
        self.assertEqual(
            _px(window, cw, BORDER, BORDER),
            CARD_BG,
            "the pixel just inside the accent ring must be the card's background",
        )

        # 5a. The highlight, located before the accent count that depends on it.
        hl = _exact_pixels(window, PANEL_SELECTED_BG)
        self.assertTrue(
            hl,
            "the exported card carries NO selected-row highlight. `draw_panel` fills the "
            "selected slot in PANEL_SELECTED_BG, so this export is not a picker card with a "
            "row selected -- and every absence this gate reads out of a capture would be "
            "satisfied by these same bytes.",
        )
        rows = sorted({i // cw for i in hl})
        cols = sorted({i % cw for i in hl})
        self.assertEqual(
            (rows[-1] - rows[0] + 1, len(rows)),
            (HL_ROWS, HL_ROWS),
            f"the highlight spans rows {rows[0]}..{rows[-1]} ({len(rows)} of them); one filled "
            f"panel slot is exactly {HL_ROWS} contiguous rows (PANEL_ROW_H less the accent "
            "stroke drawn inside it). More than one band means more than one row is drawn "
            "selected.",
        )
        self.assertEqual(
            (cols[0], cols[-1] - cols[0] + 1, len(cols)),
            (HL_X0, HL_COLS, HL_COLS),
            f"the highlight spans columns {cols[0]}..{cols[-1]}; one filled panel slot starts "
            f"at CONTENT_X+BORDER ({HL_X0}) and is exactly {HL_COLS} contiguous columns wide",
        )

        # 2. The accent: the card's perimeter ring, plus this one slot's stroke,
        #    and NOTHING ELSE anywhere on the card.
        slot_y = rows[0] - BORDER
        ring = cw * ch - (cw - 2 * BORDER) * (ch - 2 * BORDER)
        accent = _exact_pixels(window, ACCENT)
        stray = []
        for index in accent:
            x, y = index % cw, index // cw
            on_perimeter = x < BORDER or x >= cw - BORDER or y < BORDER or y >= ch - BORDER
            in_slot = (
                CONTENT_X <= x < CONTENT_X + SLOT_W
                and slot_y <= y < slot_y + PANEL_ROW_H
            )
            if not on_perimeter and not in_slot:
                stray.append((x, y))
        self.assertEqual(
            len(stray),
            0,
            f"{len(stray)} accent pixels sit outside the card's border ring and outside the "
            f"one selected slot at y={slot_y} (first few: {stray[:8]}). The accent is a frame "
            "and one highlight stroke; anything else means a second row is drawn selected, or "
            "these bytes are not this card.",
        )
        # ...and both horizontal edges of that stroke are complete, which is
        # what makes the highlight a drawn RECTANGLE rather than a fill that
        # happens to be the right size. Compared a row at a time rather than a
        # pixel at a time: 2000 `assertEqual`s would say the same thing slowly.
        accent_row = bytes(ACCENT) * SLOT_W
        for dy in list(range(BORDER)) + list(range(PANEL_ROW_H - BORDER, PANEL_ROW_H)):
            start = ((slot_y + dy) * cw + CONTENT_X) * 4
            self.assertEqual(
                window[start : start + SLOT_W * 4],
                accent_row,
                f"row {slot_y + dy} of the selected slot's accent stroke is incomplete across "
                f"its {SLOT_W}px width",
            )
        accent_px = len(accent)
        self.assertGreaterEqual(
            accent_px,
            ring + SLOT_RING_HORIZONTAL,
            f"the accent covers {accent_px} px: less than the card's {ring}px border ring plus "
            f"the {SLOT_RING_HORIZONTAL}px of horizontal stroke just checked",
        )
        self.assertLessEqual(
            accent_px,
            ring + SLOT_RING,
            f"the accent covers {accent_px} px: more than the card's {ring}px border ring plus "
            f"one {SLOT_RING}px slot stroke, so it is not a frame and one highlight",
        )

        # 3. The card body, and the buttons a human is meant to press.
        body = _exact_rgba(window, CARD_BG)
        self.assertGreater(
            body,
            cw * ch // 2,
            f"only {body} of {cw * ch} px are the card's background: the export is not an "
            "opaque card",
        )
        self.assertGreater(_exact_rgba(window, BUTTON_BG), 0, "the card must carry its buttons")
        self.assertGreater(
            _exact_rgba(window, BUTTON_BORDER), 0, "the card's buttons must be outlined"
        )

        # 4. Real, antialiased text.
        colours = {window[off : off + 4] for off in range(0, len(window), 4)}
        self.assertGreaterEqual(
            len(colours),
            MIN_CARD_COLOURS,
            f"the exported card carries only {len(colours)} distinct colours: a rasterized "
            "card antialiases its glyphs and carries hundreds",
        )
        return {
            "accent_px": accent_px,
            "body_px": body,
            "colours": len(colours),
            "hl_top": rows[0],
            "hl_px": len(hl),
            "cursor": cursor,
        }

    def _describe_card(self, injector) -> tuple[tuple[int, int, int, int], bytes]:
        """`describe` while a picker is up: geometry cross-checked against what
        the core cannot choose, and the card's own footprint of the
        human-visible framebuffer."""
        fields, window = injector.describe()
        self.assertEqual(fields["state"], "shown", "a raised picker must be a card on screen")
        self.assertEqual(
            fields["token"],
            "-",
            "a picker is not a petition and has no decidable token: `decide` must have nothing "
            "to land on while one is up",
        )
        self.assertIsNotNone(window, "a raised picker must export its footprint")
        card = (
            int(fields["card_x"]),
            int(fields["card_y"]),
            int(fields["card_w"]),
            int(fields["card_h"]),
        )
        win = (
            int(fields["win_x"]),
            int(fields["win_y"]),
            int(fields["win_w"]),
            int(fields["win_h"]),
        )
        view_w, view_h = int(fields["view_w"]), int(fields["view_h"])
        band_h = int(fields["band_h"])
        self.assertEqual((view_w, view_h), REALM_WH)
        self.assertEqual(win, card, "the exported window must be exactly the card's footprint")
        self.assertEqual(card[2], CARD_WIDTH, "consent::render::CARD_WIDTH")
        self.assertEqual(card[0], (view_w - card[2]) // 2, "the card is centred horizontally")
        self.assertEqual(card[1], (view_h - card[3]) // 2, "the card is centred vertically")
        self.assertGreaterEqual(
            card[1],
            band_h,
            "the exported rectangle must start below the trust band: the band is painted in "
            "this session's secret indicator colour and must never be read back at all",
        )
        self.assertGreaterEqual(
            card[3],
            PANEL_ROWS * PANEL_ROW_H,
            "a picker card is at least its panel tall; a shorter one is not a picker",
        )
        return card, window

    # -- the gate ----------------------------------------------------------

    def test_a_real_agent_designates_a_real_file_through_the_real_picker(self):
        core = self.real_core()
        injector = self._assert_instrumented(core)
        self._spine(core)
        settled = self._settle()

        # **Two connections, and the split is structural rather than tidy.**
        # `request_file` blocks across human time and is therefore run on a
        # thread (`_Ask`), which takes sole ownership of that connection's
        # socket for the life of the ask -- so a mid-picker `observe()` on the
        # SAME connection would have two readers on one framed stream, each
        # consuming the other's frames. (Measured: it deadlocks and the test
        # dies on `IntegrationTest`'s 90 s alarm, which is exactly the failure
        # that alarm exists to name.) The watcher is a second connection of the
        # same principal, which is what `test_real_consent.py` uses for its own
        # mid-prompt window and is a fair reading of "the agent's own capture":
        # the IDL keys a principal's state on the principal, not the socket.
        #
        # Both petitions are consented BEFORE any ask, because `ConsentGrab`
        # admits one surface at a time: a card raised while a picker is up is
        # refused, so a petition made mid-ask would pend rather than prompt.
        actor = core.connect()
        grant = actor.request_grant(
            verbs=("designate.file",),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        petition, token = injector.await_raised()
        self.assertEqual(injector.decide(token, "allow-while-running"), "queued")
        injector.await_lowered(petition)
        grant.await_consent()
        self.assertEqual(grant.effective_persistence, vitrin_os.Persistence.WHILE_RUNNING)

        watcher = core.connect()
        eye = watcher.request_grant(
            verbs=("observe",), persistence=vitrin_os.Persistence.WHILE_RUNNING
        )
        petition2, token2 = injector.await_raised()
        self.assertNotEqual(petition, petition2)
        self.assertEqual(injector.decide(token2, "allow-while-running"), "queued")
        injector.await_lowered(petition2)
        eye.await_consent()

        # The agent's own settled control, taken with no card up. Byte-exact
        # equality against this is proof 5's sharpest form, and it is only
        # available because the app is provably not painting any more.
        control_frame = eye.observe()
        agent_settled = packed_xrgb(control_frame)
        self.assertGreaterEqual(
            locate_colour(control_frame, self.TARGET)[2],
            MIN_TARGET_PIXELS,
            "the agent's control frame must show the real app's target; a black frame would "
            "make every 'the card is not in this capture' assertion below vacuous",
        )

        names = [name for name, _ in self.FILES] + [self.LINK, self.INNER_LINK]

        # == ASK A: navigate to the second row and confirm ====================
        fds_before = _fd_set()
        ask = _Ask(actor, grant)
        snap0 = injector.await_picker(timeout=20.0)
        self.assertEqual(snap0["state"], "shown")
        self.assertEqual(snap0["focus"], "listing", "a fresh picker starts on the listing")
        self.assertEqual(snap0["cursor"], 0)
        self.assertEqual(
            snap0["visible"],
            len(names),
            "the picker must list every entry of the configured root (`.` and `..` excluded)",
        )
        self.assertEqual(
            snap0["selected"],
            names[0],
            "the first row is the first name in raw-byte order: `read_entries` sorts by the "
            "bytes, because getdents order is the filesystem's and moves between runs",
        )
        designation = snap0["designation"]
        self.assertIsNotNone(designation)

        # -- proof 6, FIRST: these bytes are a picker card at that rectangle --
        card, window0 = self._describe_card(injector)
        shot0 = self._assert_is_a_real_picker_raster(window0, card, cursor=0)

        self.assertEqual(injector.navigate("down"), "queued")
        snap1 = self._await_cursor(injector, 1)
        self.assertEqual(
            snap1["selected"],
            names[1],
            "moving the cursor one row must select the next name in the listing order",
        )
        card1, window1 = self._describe_card(injector)
        self.assertEqual(
            card1, card, "moving the selection must not resize or move the card"
        )
        shot1 = self._assert_is_a_real_picker_raster(window1, card, cursor=1)
        self.assertEqual(
            shot1["hl_top"] - shot0["hl_top"],
            PANEL_ROW_H,
            "the drawn highlight must move by exactly one slot when the channel says the "
            "cursor moved by one row. This is the assertion that ties the exported pixels to "
            "the state the core reported over a different socket: a stale raster, a cached "
            "one, or a card that is not this picker fails here.",
        )

        # -- proof 5, now that the export's provenance is established ---------
        dump_up = _read_dump(self.dump_path, REALM_WH)
        if dump_up != settled:
            changed = sum(
                1
                for off in range(0, len(dump_up), 4)
                if dump_up[off : off + 3] != settled[off : off + 3]
            )
            self.fail(
                f"raising the picker changed {changed} px of the CAPTURE path (green "
                f"{_count_rgba(settled, self.TARGET)} -> {_count_rgba(dump_up, self.TARGET)}). "
                "The human-visible card must not reach the realm view at all."
            )
        frame_up = eye.observe()
        agent_up = packed_xrgb(frame_up)
        self.assertEqual(
            agent_up,
            agent_settled,
            "the agent's own capture changed while the picker was up: the card has reached "
            "the image captures are served from",
        )
        self.assertGreaterEqual(
            _count_rgba(dump_up, self.TARGET),
            MIN_TARGET_PIXELS,
            "the realm view must still show the real app under the card",
        )
        # The searches that turn the two byte-equalities above into a
        # measurement rather than a tautology: the card's two most distinctive
        # colours, counted in the human-visible export at the same instant
        # (where they must be present) and in both capture artifacts (where
        # they must be absent). Quantised on the capture side and exact on the
        # export side, for `_count_rgba`'s stated reason -- and quantising
        # WIDENS what would count as a leak, so the zeroes are stronger for it.
        #
        # Both quantise clear of everything the realm view carries: the app
        # paints black and #00ff00, and `scene::LETTERBOX_RGBA` (the matte on
        # the reserved rows) is 0x0f0f14 -> (0x00,0x00,0x10).
        for label, colour, hex6 in (
            ("accent ring", ACCENT, "4d9de0"),
            ("selected row", PANEL_SELECTED_BG, "2b3342"),
        ):
            self.assertGreater(
                _exact_rgba(window0, colour),
                0,
                f"the card's {label} colour must be present in the human-visible export",
            )
            self.assertEqual(
                _count_rgba(dump_up, hex6),
                0,
                f"the card's {label} colour appears in the CORE-INTERNAL realm view: the "
                "picker has reached the capture path",
            )
            self.assertEqual(
                locate_colour(frame_up, hex6)[2],
                0,
                f"the card's {label} colour appears in the AGENT's own frame",
            )
        agent_path = self.work / "agent.xrgb"
        agent_path.write_bytes(agent_up)
        dump_file = self.work / "dump.rgba"
        dump_file.write_bytes(dump_up)
        cmp_result = golden_cmp(
            str(agent_path),
            str(dump_file),
            REALM_WH,
            "tol:1,0.001",
            artifacts=str(self.work / "cmp-art"),
        )
        self.assertEqual(
            cmp_result.returncode,
            0,
            "the agent's mid-picker frame must agree with the core-internal realm-view "
            "capture through the M1.3 gate's own comparator.\n"
            f"{cmp_result.stdout.strip()}\n{cmp_result.stderr.strip()}",
        )

        # -- proofs 1, 2, 3: confirm, and read what arrives -------------------
        self.assertEqual(injector.confirm(), "queued")
        designated = ask.result(self)
        self.assertIsInstance(
            designated,
            messages.DesignatedEvent,
            f"a confirmed picker must answer `designated`, not {designated!r}",
        )
        new_fds = _fd_set() - fds_before
        self.assertEqual(
            len(new_fds),
            1,
            f"exactly one descriptor must have crossed the socket; this process gained "
            f"{sorted(new_fds)}",
        )
        self.assertEqual(
            int(designation), designated.designation_id,
            "the terminal must name the designation the card on screen was raised for",
        )
        st = os.fstat(designated.fd)
        self.assertTrue(stat.S_ISREG(st.st_mode), "a file designation delivers a regular file")
        self.assertEqual(
            (st.st_dev, st.st_ino),
            self.inode_of[names[1]],
            "the delivered descriptor is not the inode of the row the picker displayed "
            f"({names[1]!r}). Compared against a pair recorded when the file was created, "
            "never against a path re-resolved now: a name re-resolved after the fact is "
            "exactly the race this descriptor exists to close.",
        )
        self.assertNotEqual(
            (st.st_dev, st.st_ino),
            self.inode_of[names[0]],
            "the delivered descriptor is the FIRST row's inode while the channel reported the "
            "second row selected: the confirm did not settle from the drawn cursor",
        )
        self.assertEqual(
            designated.name,
            os.fsdecode(names[1]),
            "`name` is display copy for the row the human chose",
        )
        self.assertEqual(designated.mode, protocol.DesignationMode.READ)
        body = dict(self.FILES)[names[1]]
        self.assertEqual(
            os.pread(designated.fd, len(body) + 16, 0),
            body,
            "reading through the delivered descriptor must return the designated file's bytes: "
            "the descriptor is the authority, so this gate spends it rather than holding it",
        )
        os.close(designated.fd)

        # == ASK B: cancel ====================================================
        fds_before_b = _fd_set()
        ask_b = _Ask(actor, grant)
        snap_b = injector.await_picker(timeout=20.0)
        self.assertEqual(snap_b["cursor"], 0, "a fresh picker starts at the top again")
        self.assertNotEqual(
            snap_b["designation"], designation, "each ask gets its own designation id"
        )
        self.assertEqual(injector.cancel(), "queued")
        refused = ask_b.result(self)
        self.assertIsInstance(
            refused,
            messages.PowerboxRefusedEvent,
            f"a cancelled picker must answer `refused`, not {refused!r}",
        )
        self.assertEqual(
            refused.code,
            int(protocol.PowerboxRefusal.CANCELLED),
            "the refusal must be `cancelled` SPECIFICALLY: `timed_out`, `busy` and "
            "`unresolvable` are all compatible with a picker nobody cancelled, so accepting "
            "any of them would stop this naming the human's choice as the reason",
        )
        self.assertEqual(
            _fd_set() - fds_before_b,
            set(),
            "a refused ask delivered a descriptor: a cancel must hand over nothing at all",
        )
        self.assertEqual(
            injector.picker()["state"], "none", "a cancelled picker must leave the screen"
        )

        # == ASK C: a read-write designation, written through =================
        fds_before_c = _fd_set()
        ask_c = _Ask(actor, grant, write=True)
        snap_c = injector.await_picker(timeout=20.0)
        self.assertEqual(snap_c["selected"], names[0])
        self.assertEqual(injector.confirm(), "queued")
        rw = ask_c.result(self)
        self.assertIsInstance(rw, messages.DesignatedEvent)
        self.assertEqual(
            rw.mode,
            protocol.DesignationMode.READ_WRITE,
            "a `request_file(write=True)` the human confirmed unchanged must answer "
            "`read_write`. Without this arm every `mode` assertion in this file reads `read` "
            "and would pass on a core that ignored the ask entirely.",
        )
        self.assertEqual((os.fstat(rw.fd).st_dev, os.fstat(rw.fd).st_ino), self.inode_of[names[0]])
        marker = b"WRITTEN-THROUGH-THE-DESIGNATION"
        self.assertEqual(
            os.pwrite(rw.fd, marker, 0),
            len(marker),
            "a read_write designation must be writable through the descriptor: the mode is a "
            "claim about the open file description, not a label on the terminal",
        )
        self.assertEqual(os.pread(rw.fd, len(marker), 0), marker)
        os.close(rw.fd)
        self.assertEqual(
            (self.root / os.fsdecode(names[0])).read_bytes()[: len(marker)],
            marker,
            "the write must have landed on the designated file itself",
        )
        self.assertEqual(len(_fd_set() - fds_before_c), 0, "the delivered fd was closed above")

        # == ASK D: a symlink row, refused rather than followed ===============
        fds_before_d = _fd_set()
        ask_d = _Ask(actor, grant)
        injector.await_picker(timeout=20.0)
        # `last` then one `up`: the inside-pointing link (ASK E) sorts after
        # this one, so the escaping link is now the second-to-last row.
        self.assertEqual(injector.navigate("last"), "queued")
        self._await_cursor(injector, len(names) - 1)
        self.assertEqual(injector.navigate("up"), "queued")
        snap_d = self._await_cursor(injector, len(names) - 2)
        self.assertEqual(
            snap_d["selected"],
            self.LINK,
            "this row must be the escaping symlink",
        )
        self.assertEqual(injector.confirm(), "queued")
        linked = ask_d.result(self)
        self.assertIsInstance(
            linked,
            messages.PowerboxRefusedEvent,
            "a symlink row must be REFUSED, not followed: `picker::resolve` walks with "
            "RESOLVE_NO_SYMLINKS|BENEATH|NO_MAGICLINKS, and a picker that quietly degraded to "
            f"`openat` would hand over {self.outside} under a guarantee it had stopped "
            "providing",
        )
        self.assertEqual(
            linked.code,
            int(protocol.PowerboxRefusal.UNRESOLVABLE),
            "the refusal must be `unresolvable`; `cancelled` would mean the human declined",
        )
        self.assertEqual(_fd_set() - fds_before_d, set(), "a refused ask hands over nothing")

        # == ASK E: a symlink INSIDE the root, which only NO_SYMLINKS refuses ==
        #
        # Ask D's link escapes the root, so `RESOLVE_BENEATH` refuses it with
        # EXDEV whether or not `NO_SYMLINKS` is in the flag set — measured, by
        # dropping the flag and watching this gate stay green. This row's link
        # points at `alpha.txt`, a file the picker designates happily under its
        # own name, so containment has nothing to object to and the only thing
        # left to refuse it is the flag the assertion is about.
        fds_before_e = _fd_set()
        ask_e = _Ask(actor, grant)
        injector.await_picker(timeout=20.0)
        self.assertEqual(injector.navigate("last"), "queued")
        snap_e = self._await_cursor(injector, len(names) - 1)
        self.assertEqual(
            snap_e["selected"],
            self.INNER_LINK,
            "the last row must be the inside-pointing symlink; it sorts last by raw bytes",
        )
        self.assertEqual(injector.confirm(), "queued")
        inner = ask_e.result(self)
        self.assertIsInstance(
            inner,
            messages.PowerboxRefusedEvent,
            "a symlink whose target is INSIDE the root must still be refused. Its target "
            f"({self.FILES[0][0]!r}) is designatable under its own name, and the link does "
            "not escape containment — so if this is followed, `RESOLVE_NO_SYMLINKS` is not "
            "in the resolver's flag set and the picker has quietly become path-based",
        )
        self.assertEqual(
            inner.code,
            int(protocol.PowerboxRefusal.UNRESOLVABLE),
            "the refusal must be `unresolvable`",
        )
        self.assertEqual(_fd_set() - fds_before_e, set(), "a refused ask hands over nothing")

        actor.close()
        watcher.close()
        core.terminate()
        entries = core.entries()

        # == proof 7: the journal ============================================
        run_started = [e for e in entries if e["kind"] == "run_started"]
        self.assertTrue(run_started)
        self.assertEqual(
            run_started[0].get("consent_policy"),
            "interactive+consent-injector",
            "an instrumented run must brand its own journal, or it could be read as one a "
            "human answered",
        )
        settled_entries = [e for e in entries if e["kind"] == "designation_settled"]
        self.assertEqual(
            len(settled_entries),
            5,
            "every admitted ask owes exactly one settled record, delivered or not -- a "
            f"designation the human made and the agent never received must not be silent: "
            f"{settled_entries}",
        )
        self.assertEqual(
            len({e["designation"] for e in settled_entries}),
            5,
            "each ask must be journalled under its own designation id",
        )
        for entry in settled_entries:
            self.assertEqual(entry["principal"], DEMO_IDENTITY)
            self.assertEqual(entry["realm"], "realm-0")
            self.assertEqual(entry["target"], "file")

        delivered = [e for e in settled_entries if e["outcome"] == "delivered"]
        self.assertEqual(len(delivered), 2, f"two asks produced a descriptor: {settled_entries}")
        by_mode = {e["mode"]: e for e in delivered}
        self.assertEqual(sorted(by_mode), ["read", "read_write"])
        self.assertEqual(
            (by_mode["read"]["st_dev"], by_mode["read"]["st_ino"]),
            self.inode_of[names[1]],
            "the journal's (st_dev, st_ino) must be the inode this test read bytes out of. It "
            "is `fstat`'d off the descriptor at the moment it was opened, so nothing else in "
            "the journal could reconstruct it -- a path would be a name, and a name re-read "
            "later is a claim about whatever it points at then",
        )
        self.assertEqual(
            (by_mode["read_write"]["st_dev"], by_mode["read_write"]["st_ino"]),
            self.inode_of[names[0]],
        )
        for entry in delivered:
            self.assertTrue(entry["delivered"])

        # `unresolvable` twice now: the escaping link (refused by containment)
        # and the inside-pointing link (refused only by NO_SYMLINKS).
        for outcome, count in (("cancelled", 1), ("unresolvable", 2)):
            rows = [e for e in settled_entries if e["outcome"] == outcome]
            self.assertEqual(
                len(rows), count, f"expected {count} {outcome} designation(s): {settled_entries}"
            )
            for row in rows:
                self.assertFalse(row["delivered"])
            self.assertNotIn(
                "st_dev",
                rows[0],
                f"a {outcome} designation opened nothing, so it must name no inode at all: a "
                "zero there would read as one",
            )
            self.assertNotIn("st_ino", rows[0])

        # The grant row, joined through the chokepoint's own record rather than
        # assumed: `use_decision` carries the WIRE id this SDK holds and the
        # internal `grant_id` the settled record names, so the two can be tied.
        owed = [
            e
            for e in entries
            if e["kind"] == "use_decision" and e.get("verb") == "designate_file"
        ]
        self.assertEqual(
            len(owed), 5, f"each ask must be admitted once at the chokepoint: {owed}"
        )
        for entry in owed:
            self.assertEqual(entry.get("decision"), "owed")
            self.assertEqual(
                entry.get("grant_wire_id"),
                grant.id,
                "the ask was admitted under a grant this agent does not hold",
            )
        grant_rows = {e["grant_id"] for e in owed}
        self.assertEqual(len(grant_rows), 1)
        for entry in settled_entries:
            self.assertEqual(
                entry["grant_id"],
                next(iter(grant_rows)),
                "the settled record must name the same grant row the chokepoint admitted the "
                "ask under",
            )

        # The far side of the second SCM_RIGHTS half: a REAL C shim decoded the
        # event. The needle is the decode -- id, kind, mode, name length -- and
        # not the disposition. Since P2.6.7 (#191) the line continues with one
        # of three dispositions, and here it is "CLOSED, not relayed: no app is
        # connected", because click-target never connects to designation.sock;
        # pinning that suffix would make this gate a witness of which app it
        # spawns rather than of the wire.
        realm_log = core.app_output("realm-0")
        want = (
            f"designation {designated.designation_id} arrived "
            f"(file, read-only, name {len(names[1])} bytes)"
        )
        self.assertIn(
            want,
            realm_log,
            f"the realm's own shim never reported decoding the designation ({want!r}). The "
            "core sends one descriptor twice -- to the agent and to the realm's shim -- and "
            "the shim's log is the only witness available on the far side of that wire.",
        )

        print(
            f"\n[real-powerbox] picker root {self.root} served {len(names)} rows to a real "
            f"agent through a real vitrin-shim. Designation {designated.designation_id}: the "
            f"export at {card} was checked to BE a picker raster there ({shot0['accent_px']} "
            f"px of accent, {shot0['body_px']} px of card background, {shot0['colours']} "
            f"distinct colours, a {shot0['hl_px']} px highlight whose top moved "
            f"{shot1['hl_top'] - shot0['hl_top']} px from cursor 0 to cursor 1), while the "
            "realm view and the agent's own frame were BYTE-IDENTICAL to their settled "
            f"controls and carried 0 px of the card's colours. The fd was inode "
            f"{self.inode_of[names[1]]} -- {names[1]!r}, the row the picker displayed -- and "
            f"read back {body!r}. A cancel answered refused(cancelled) and a symlink row "
            "refused(unresolvable), neither delivering a descriptor; a write=True designation "
            "was written through and read back."
        )


class PowerboxWithoutAPickerRoot(IntegrationTest):
    """The positive control for the whole file: no `[[picker]]` table, no
    designation.

    Without this, every assertion in `RealPowerboxDesignation` would be
    consistent with a core that serves designations from some ambient default,
    and the gate would be silent about the one line of `realm.toml` that makes
    the deployment serve the verb at all. It is also what proves
    `ConsentInjector.await_picker` can fail: here it never sees a card, because
    there is none to see.

    `crates/vitrin-core/src/main.rs` states the asymmetry this checks: a
    missing picker root is a WARNING at startup rather than an abort -- a
    session that cannot serve one verb still composites and serves every other
    -- but every ask is then refused `internal` rather than served quietly.
    """

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        app = _resolve_sibling(self.shim_bin, "click-target", "VITRIN_CLICK_TARGET_APP")
        if app is None:
            self.fail("no click-target beside the C shim; rebuild it (shim/meson.build)")
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-powerbox-none-"))
        self.addCleanup(shutil.rmtree, self.work, True)
        self.core_log = self.work / "core.log"

    def test_a_deployment_with_no_picker_root_refuses_every_ask(self):
        try:
            core = self.core(
                consent="interactive",
                size=REALM_SIZE,
                shim=str(self.shim_bin),
                command=self.app_bin,
                args=["--run-ms", "60000"],
                env_allow=tuple(WLR_ENV),
                extra_env=WLR_ENV,
                log_file=str(self.core_log),
                consent_injector=True,
                # ...and no `picker_root`. That is the whole experiment.
            )
        except CoreFailed as exc:
            self.fail(f"{_WRONG_BUILD}\n\nThe core's own words:\n{exc}")
        injector = core.injector
        assert isinstance(injector, ConsentInjector)
        injector.await_banner()
        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline and not shims_of(core.pid):
            time.sleep(0.05)
        self.assertTrue(shims_of(core.pid), "the core forked no shim")

        conn = core.connect()
        grant = conn.request_grant(
            verbs=("observe", "designate.file"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        petition, token = injector.await_raised()
        injector.decide(token, "allow-while-running")
        injector.await_lowered(petition)
        grant.await_consent()

        fds_before = _fd_set()
        # Blocking is safe here: no card is ever raised, so nothing has to
        # drive one, and the refusal is answered inside the ask's own dispatch
        # turn rather than across human time.
        with self.assertRaises(errors.GrantRefused) as refused:
            conn.request_file(grant)
        self.assertEqual(refused.exception.verb, vitrin_os.Verb.DESIGNATE_FILE)
        self.assertEqual(_fd_set() - fds_before, set(), "a refused ask hands over nothing")
        self.assertEqual(
            injector.picker()["state"],
            "none",
            "no card may be raised for an ask the chokepoint refused",
        )

        conn.close()
        core.terminate()
        entries = core.entries()
        refusals = [
            e
            for e in entries
            if e["kind"] in ("use_decision", "use_refusal_summary")
            and e.get("verb") == "designate_file"
            and e.get("refusal") == "internal"
        ]
        self.assertTrue(
            refusals,
            "the chokepoint must journal an `internal` refusal naming the verb; kinds seen: "
            f"{sorted({e['kind'] for e in entries})}",
        )
        self.assertFalse(
            [e for e in entries if e["kind"] == "designation_settled"],
            "no ticket may be minted for an ask that never reached a picker",
        )
        log = self.core_log.read_text(errors="replace")
        self.assertIn(
            "no picker root",
            log,
            "a session serving no designations must say so at startup, in its own log",
        )
        print(
            "\n[real-powerbox] control: with no [[picker]] table the same grant, the same "
            "verb and the same ask are refused `internal` at the chokepoint, no card is "
            "raised and no descriptor crosses the socket."
        )


class PowerboxGateStaysDiscriminating(unittest.TestCase):
    """Binary-free checks that this gate's constants still name real things and
    that its provenance control still rejects a forgery.

    Follows `test_real_consent.py`'s `ConsentGateThresholdsStayDiscriminating`.
    The three synthetic buffers below are the answer to "can this assertion
    fail": they are run on every invocation, so the control cannot rot into a
    check that passes on anything.
    """

    def _blank_card(self, cw: int, ch: int, cursor: int | None) -> bytes:
        """A card-shaped forgery: correct ring, correct body, correct buttons,
        and (optionally) a correct highlight at `cursor`."""
        buf = bytearray(bytes(CARD_BG) * (cw * ch))

        def fill(x0, y0, w, h, colour):
            for y in range(y0, y0 + h):
                for x in range(x0, x0 + w):
                    off = (y * cw + x) * 4
                    buf[off : off + 4] = bytes(colour)

        def stroke(x0, y0, w, h, colour, t):
            fill(x0, y0, w, t, colour)
            fill(x0, y0 + h - t, w, t, colour)
            fill(x0, y0, t, h, colour)
            fill(x0 + w - t, y0, t, h, colour)

        stroke(0, 0, cw, ch, ACCENT, BORDER)
        fill(40, ch - 60, 100, 30, BUTTON_BG)
        stroke(40, ch - 60, 100, 30, BUTTON_BORDER, 1)
        if cursor is not None:
            top = 200 + cursor * PANEL_ROW_H
            fill(CONTENT_X, top, SLOT_W, PANEL_ROW_H, PANEL_SELECTED_BG)
            stroke(CONTENT_X, top, SLOT_W, PANEL_ROW_H, ACCENT, BORDER)
        return bytes(buf)

    def _checker(self):
        """`_assert_is_a_real_picker_raster`, bound to a throwaway test case so
        its assertions raise here rather than needing a live core."""
        gate = RealPowerboxDesignation("test_a_real_agent_designates_a_real_file_through_the_real_picker")
        return gate._assert_is_a_real_picker_raster  # noqa: SLF001 -- the point

    def test_the_provenance_control_rejects_an_empty_export(self):
        """The failure that motivates this whole class: an export that never
        read the framebuffer at all. #138's consent gate stayed GREEN through
        exactly that sabotage and printed its success line verbatim."""
        cw, ch = CARD_WIDTH, 570
        with self.assertRaises(AssertionError):
            self._checker()(bytes(cw * ch * 4), (0, 0, cw, ch), 0)

    def test_the_provenance_control_rejects_a_card_with_no_selection(self):
        """A flat card-shaped forgery with no highlight is not a picker with a
        row selected, and every absence this gate reads would be equally true
        of it."""
        cw, ch = CARD_WIDTH, 570
        with self.assertRaises(AssertionError):
            self._checker()(self._blank_card(cw, ch, None), (0, 0, cw, ch), 0)

    def test_the_provenance_control_rejects_a_highlight_of_the_wrong_shape(self):
        """A highlight that is not one slot -- two rows selected, or a band the
        wrong width -- is a card the confirm cannot be settled from."""
        cw, ch = CARD_WIDTH, 570
        two = bytearray(self._blank_card(cw, ch, 0))
        # Paint a second highlighted slot: two rows drawn selected at once.
        for y in range(200 + 3 * PANEL_ROW_H + BORDER, 200 + 4 * PANEL_ROW_H - BORDER):
            for x in range(HL_X0, HL_X0 + HL_COLS):
                off = (y * cw + x) * 4
                two[off : off + 4] = bytes(PANEL_SELECTED_BG)
        with self.assertRaises(AssertionError):
            self._checker()(bytes(two), (0, 0, cw, ch), 0)

    def test_the_provenance_control_accepts_a_correctly_shaped_card(self):
        """...and the complement, without which the three refusals above would
        be satisfied by a checker that rejects everything."""
        cw, ch = CARD_WIDTH, 570
        card = self._blank_card(cw, ch, 0)
        # Antialiased text is the one property a hand-built forgery cannot
        # reach, so it is supplied here rather than left as the reason this
        # passes for the wrong cause: the shape checks are what is under test.
        buf = bytearray(card)
        for i in range(MIN_CARD_COLOURS + 8):
            off = ((60 + i) * cw + 300) * 4
            buf[off : off + 4] = bytes((i, i, i, 0xFF))
        self._checker()(bytes(buf), (0, 0, cw, ch), 0)

    def test_the_picker_card_constants_match_the_renderer(self):
        """Every constant the provenance control computes with is still the
        renderer's own.

        A restyled or re-laid-out card would leave the searches finding
        nothing, which fails loudly -- so this is not a soundness hole. It says
        WHICH thing moved, instead of failing with "no accent border".
        """
        source = (
            pathlib.Path(__file__).resolve().parents[2]
            / "crates/vitrin-core/src/consent/render.rs"
        ).read_text()

        def rgba(name: str, value: tuple[int, int, int, int]) -> str:
            body = ", ".join(f"0x{c:02x}" for c in value)
            return f"const {name}: [u8; 4] = [{body}];"

        for name, value in (
            ("CARD_BG", CARD_BG),
            ("ACCENT", ACCENT),
            ("BUTTON_BG", BUTTON_BG),
            ("BUTTON_BORDER", BUTTON_BORDER),
            ("PANEL_SELECTED_BG", PANEL_SELECTED_BG),
        ):
            self.assertIn(
                rgba(name, value),
                source,
                f"consent::render::{name} moved; the picker export's provenance control is "
                "searching for a colour the card no longer carries",
            )
        for decl in (
            f"pub(crate) const CARD_WIDTH: u32 = {CARD_WIDTH};",
            f"const BORDER: u32 = {BORDER};",
            f"const PAD_X: u32 = {PAD_X};",
            "const CONTENT_X: u32 = BORDER + PAD_X;",
            "const CONTENT_W: u32 = CARD_WIDTH - 2 * CONTENT_X;",
            f"const PANEL_THUMB_W: u32 = {PANEL_THUMB_W};",
            f"const PANEL_ROW_H: u32 = {PANEL_ROW_H};",
            f"pub(crate) const PANEL_ROWS: u16 = {PANEL_ROWS};",
            "let slot_w = CONTENT_W - PANEL_THUMB_W - 4;",
        ):
            self.assertIn(
                decl,
                source,
                f"consent::render no longer declares `{decl}`; the highlight's exact row and "
                "column arithmetic is computed from it",
            )
        self.assertIn(
            "canvas.fill_rect(rect, PANEL_SELECTED_BG);",
            source,
            "the selected panel row is no longer FILLED; the highlight this gate locates is "
            "the fill, and its extent is computed from the accent stroke drawn inside it",
        )
        self.assertIn(
            "canvas.stroke_rect(rect, ACCENT, BORDER);",
            source,
            "the selected panel row is no longer STROKED; the accent's exact pixel count and "
            "the highlight's 18-row extent both assume it is",
        )

    def test_the_view_is_taller_than_the_card_can_be(self):
        """The one sizing constraint this gate has, checked rather than
        remembered.

        A view shorter than the card makes the core clamp the export and log
        `the card does not fit the view; exporting nothing` -- and a gate whose
        provenance control has no bytes to run on is a gate that proves
        nothing. 570 px is the picker card's measured height on this tree; the
        margin is what leaves room for the card to grow a row.
        """
        self.assertGreater(
            REALM_WH[1],
            600,
            "the view must be taller than the picker card (measured 570 px) with margin",
        )
        self.assertGreater(REALM_WH[0], CARD_WIDTH)

    def test_the_thresholds_stay_reachable_and_not_free(self):
        self.assertEqual(TARGET_AREA, 25_600, "click_target.c's TARGET_SIZE is 160")
        self.assertGreater(MIN_TARGET_PIXELS, 0, "zero green px must not count as 'located'")
        self.assertLess(MIN_TARGET_PIXELS, TARGET_AREA)
        self.assertGreater(MIN_CARD_COLOURS, 8, "a flat forgery has a handful of colours")
        self.assertLess(
            MIN_CARD_COLOURS,
            961,
            "the real picker export measures 961 distinct colours; the bar must sit under it",
        )
        self.assertGreaterEqual(
            SETTLE_READS, 2, "a single read is not a settle (the P1.9.8 gate-integrity failure)"
        )
        self.assertEqual(HL_ROWS, 18)
        self.assertEqual(HL_COLS, 496)
        self.assertEqual(HL_X0, 28)


if __name__ == "__main__":
    unittest.main()
