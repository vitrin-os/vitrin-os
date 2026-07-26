# SPDX-License-Identifier: Apache-2.0
"""Issue #138 (M1.4 exit gate, the CONSENT half): a real consent prompt, raised
by the shipped `vitrind` over a REAL app, really occludes the human-visible
output, is really absent from the agent's capture, really holds that
principal's actuations while it is up, and is really resolved by a decision
that funnels through the human path -- no mock on any seam, no in-process
runtime.

`test_real_deadman.py` (P1.7.4) closed #109's hold-Esc half. This closes the
half `tests/integration/README.md` documented as open: the occlusion evidence
that existed was `crates/vitrin-core/src/backend/headless.rs`'s
`c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture`,
which is genuinely mock-free on the app seam but builds a `HeadlessView` and a
`ShimServer` in-process -- exactly what plan §5 D12 disqualifies as milestone
evidence. Everything below drives `target/debug/vitrind` over a real socket.

# Why this is a socket, not a mouse click (the test-gated injector)

Headless has no display for a human to look at and no pointer for a human to
click with, which is why a plain build REFUSES `--headless
--consent=interactive` at startup -- correctly, since a petition there could
only pend until it timed out. A `consent-injector` build (issue #138,
`crates/vitrin-core/Cargo.toml`) supplies what the refusal says is missing,
and only when the invocation ALSO carries `--consent-injector-fd N`:

- an inherited `AF_UNIX`/`SOCK_STREAM` socketpair on which the harness reads
  `raised <petition> <token>` / `lowered <petition>` edges, asks `describe`,
  and says `decide <token> <button>`;
- `describe` returns the consent card's OWN FOOTPRINT of the human-visible
  framebuffer as a sealed memfd -- not a whole frame.

It is **not a second decision path**. The injected decision is deposited into
the round's `ConsentGrab` by `ConsentGrab::queue_decision` and then drained,
validated and applied by the same `session::service_consent_round` and the
same `PetitionRegistry::resolve_human` a real click on the nested backend
reaches -- which is why proof 4 below demands `issuer == "human_consent"`. A
decision that had taken `scripted-consent`'s in-process `resolve_scripted`
shortcut would journal `scripted_consent` there and fail.

# What this gate proves, end to end, against `click-target`

1. **A real petition raises a real prompt over a real app's scene**, and the
   decision the channel names is the rung the grant gets.
2. **The prompt occludes the human-visible output and never the capture
   path.** Three independently transported artifacts of one instant agree:
   the sealed memfd (human-visible, card footprint), the core-internal
   `--capture-dump` (realm view), and the agent's own `observe()` frame.
   Stated as the metric it really is: the sealed memfd is first shown to *be*
   a raster of vitrind's consent card, on exactly the rectangle the core
   named -- accent ring on all four edges, exact perimeter count, opaque
   body, buttons, antialiased text (`_assert_is_a_real_card_raster`) -- and
   only then shown to carry zero pixels of the app's target colour, while the
   realm view carries that target at those same coordinates. The order
   matters: without the positive control the second half is an absence over
   bytes of unproven provenance, and an empty or synthetic export satisfies
   an absence perfectly. That gap was real and is the first entry in the
   watched-failing list below.
3. **Mid-prompt actuations never land, demonstrably BECAUSE a prompt was
   pending** -- five independent facts, listed on the test method.
4. **The decision resolves through the real state machine and the flight
   recorder journals it**, with `issuer: human_consent`.

# What this gate CANNOT prove, and where the rest lives

- **Unspoofability (issue #85) is out of scope here, permanently.** The gate
  never learns this session's trusted indicator colour, by design: that
  secret is never written to any descriptor or file, and the exported window
  is exactly the card's footprint -- which
  `crates/vitrin-core/src/consent/mod.rs`'s `card_rect` and its
  `the_card_footprint_carries_no_indicator_pixel` test prove is
  indicator-free, because the trusted ring is stroked strictly OUTSIDE it and
  the opaque card is blitted last. So this gate proves **occlusion**, not
  that the card is framed in a colour a confined app cannot forge. That half
  stays component-level evidence: `consent/mod.rs`'s band and frame tests,
  `headless.rs`'s `a_prompt_reaches_human_visible_output_but_never_a_capture`
  and the real-app `c_shim_consent_prompt_occludes_...`, and, for a human,
  `shim/docs/firefox.md` §9's nested recipe.
- **The physical click is not proven here.** The hit test, the 500 ms
  `GUARD_INTERVAL`, the press-arms/release-commits ladder, and the origin
  check that stops an agent answering its own prompt are proven only by
  `crates/vitrin-core/src/consent/grab.rs`'s own tests (which drive the
  private `judge_parts` with real events, including
  `an_agent_cannot_answer_the_prompt_it_petitioned_for`) and by
  `shim/docs/firefox.md` §9 with a human at a mouse. The injector bypasses
  `judge` completely, and the headless router still stacks `NoopHook`, so no
  input of any origin can reach this grab.
- **It says nothing about the human-visible frame OUTSIDE the card
  footprint** -- not the scrim, not the ring -- and with `click-target` it
  structurally cannot, which is worth stating precisely rather than as a
  caveat. The export is clamped to the card's footprint (C1, above), and the
  card is `blit_opaque`-ed last, so those bytes do not depend on the frame
  underneath them at all. Measured on this gate's own artifacts: the realm
  view outside the card footprint is 93 840 px of a single colour, black --
  `click-target` paints black everywhere except one centred 160x160 square
  that the 560x381 centred card wholly covers. So a regression that erased
  the realm view from the human-visible output entirely (`view.fill(0)`
  before `composite_over`) changes **no pixel** of what a human sees here:
  the exported window is byte-identical, sha256 and all, which was confirmed
  by running it. No assertion this gate could add would see it. That defect
  is real and it *is* caught -- by
  `backend::headless`'s `a_prompt_reaches_human_visible_output_but_never_a_capture`
  (a full-bleed test pattern, bottom-left asserted scrimmed-not-erased) and
  `backend::winit`'s `the_nested_window_uploads_the_consent_overlay`, both of
  which go red on it. Component evidence, named as such.
- It cannot show the card is legible or names the right principal;
  `consent/render.rs`'s golden and sourcing tests hold that.

# Watched failing (plan §5 D12 item 4)

Mock-freeness proves what a test is *wired to*, never what it
*discriminates*. Each proof was broken and confirmed red before this was
cited:

- **P1**: comment out `HeadlessState::service_consent`, or skip
  `mark_prompt_shown` in `ConsentGrab::raise` -> no `raised` edge, no
  `consent_transition{shown}` -> fails by name into the deadline.
- **P2, occlusion half**: make `backend::human_visible_from_view` skip
  `composite_over` -> the exported window shows click-target's green ->
  `green(A) == 0` fails.
- **P2, the export's provenance**: replace the `readback_region` call in
  `consent_occlusion_window` with `Some((region, vec![0u8; w*h*4]))`, so the
  human-visible framebuffer is never read at all -> the positive control
  fails on the very first edge pixel, naming the cause ("these bytes are not
  a readback of the card at the rectangle the core reported"). Before that
  control existed this sabotage kept the gate GREEN and printed its success
  line verbatim; it is the reason `_assert_is_a_real_card_raster` is here.
- **P2, capture half**: point `Presenter::view_rgba` at
  `latest_output_rgba` -- folding the two retained images, the exact mistake
  that accessor is `#[cfg(test)]` to prevent -> the byte-equality against the
  settled control fails and `C` diverges from `B`.
- **P2, geometry**: make `consent_occlusion_window` read the full frame ->
  `card_w == 560`, the centred `card_x`, and `card_y >= band_h` all fail.
- **P3**: (a) return `Gate::Deliver` unconditionally from the chokepoint's
  `consent_held` step -> facts 1, 3 and 4 fail independently; (b) suppress
  the `use_decision` refusal record -> fact 4 alone fails; (c) make
  `consent_held` also refuse captures -> fact 2 fails; (d) delete the
  post-denial click -> `_await_flip` fails and says in as many words that the
  earlier `ConsentHeld` now proves nothing.
- **P4**: route the injected decision through `resolve_scripted` ->
  `issuer` reads `scripted_consent` -> fails.

# Skip-or-fail policy (matches the real-app ladder)

- `VITRIN_SKIP_REAL_APP=1` -> skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim or `click-target` is missing ->
  fail (both co-built by the shim's Meson build).
- A `vitrind` built or invoked without the injector -> fail, naming the
  feature and the rebuild, never a silent skip: a real run was requested.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import tempfile
import time
import unittest

from harness import (
    ConsentInjector,
    CoreFailed,
    IntegrationTest,
    children_of,
    comm_of,
    descendant_named,
    dominant_colour,
    golden_cmp,
    locate_colour,
    packed_xrgb,
    require_binaries,
)

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

#: Same realm shape as the actuation and dead-man gates, so a `click-target`
#: centroid found here means exactly what it means there.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: `shim/tests/click_target.c`'s `TARGET_SIZE`: the green square is 160x160 and
#: centred, so its true area is 25 600 px. Named so the threshold class below
#: can pin the bar against it rather than against a number in prose.
TARGET_SIZE = 160
TARGET_AREA = TARGET_SIZE * TARGET_SIZE

#: How much green must be on screen before the agent trusts it located the
#: target. Same bar the actuation and dead-man gates use for the same square.
MIN_TARGET_PIXELS = 5000

#: `crates/vitrin-core/src/consent/render.rs`'s `CARD_WIDTH`. The card's
#: *height* is content-derived (`render::rasterize` sums row heights, and
#: `card_height_tracks_its_content` proves a longer principal makes a taller
#: card), so this gate reads every other dimension from the core at runtime
#: and hard-codes only the one constant that really is one.
CARD_WIDTH = 560

#: The card raster's own palette, from `consent/render.rs`. Every one of these
#: is pinned against that file by
#: `test_the_card_raster_constants_match_the_renderer`, the same way
#: `CARD_WIDTH` is, so a renderer that restyled the card fails there loudly
#: instead of quietly turning the positive control below into a check on
#: colours nothing paints any more.
#:
#: They exist because "no pixel of the app's green is inside the card's
#: footprint" is an ABSENCE, and an absence is equally true of an empty
#: buffer, a synthetic one, or a rectangle of some other part of the screen.
#: The gate has no independent view of the human-visible framebuffer -- it
#: must not have one, that is C1 (issue #85) -- so the provenance of those
#: bytes has to come from their own content: they have to BE a raster of
#: vitrind's consent card, at exactly the rectangle the core named.
#:
#: `BORDER`/`ACCENT` carry most of that weight, because the accent ring is
#: positional: `render::rasterize` strokes it `BORDER` px wide along the edges
#: of the card image, so finding it exactly on the exported rectangle's four
#: edges -- and nowhere else, an exact pixel count -- says the readback landed
#: on the card's footprint and not one pixel off it.
CARD_BG = (0x14, 0x16, 0x1C, 0xFF)
ACCENT = (0x4D, 0x9D, 0xE0, 0xFF)
BUTTON_BG = (0x22, 0x27, 0x31, 0xFF)
BUTTON_BORDER = (0x5C, 0x66, 0x78, 0xFF)
BORDER = 2

#: How many distinct colours a genuinely rasterized card carries. The real
#: export measures 824: `render::text` antialiases every glyph, so a card with
#: text on it has hundreds of blend values between `CARD_BG` and the label
#: colours. A flat forgery -- fill, ring, buttons -- has a handful. Set far
#: below the measured value and far above any flat construction.
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
    the real-app ladder, e.g. `test_real_deadman.py::_require_shim`)."""
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


# -- raw-RGBA colour analysis ------------------------------------------------
#
# The occlusion window and `--capture-dump` are both tightly packed RGBA8888,
# rows top-down. `harness.locate_colour` is written against the *wire* frame
# shape (BGRX), so these two helpers do the same 4-bit quantisation for raw
# RGBA rather than repacking twice.


def _quantised(rgb: tuple[int, int, int]) -> tuple[int, int, int]:
    return (rgb[0] & 0xF0, rgb[1] & 0xF0, rgb[2] & 0xF0)


def _count_rgba(buf: bytes, hex6: str) -> int:
    """How many pixels of a raw-RGBA buffer quantise to `hex6`."""
    want = _quantised((int(hex6[0:2], 16), int(hex6[2:4], 16), int(hex6[4:6], 16)))
    hits = 0
    for off in range(0, len(buf), 4):
        if (buf[off] & 0xF0, buf[off + 1] & 0xF0, buf[off + 2] & 0xF0) == want:
            hits += 1
    return hits


def _exact_rgba(buf: bytes, want: tuple[int, int, int, int]) -> int:
    """How many pixels of a raw-RGBA buffer are EXACTLY `want`.

    Unquantised, unlike `_count_rgba`. The app's colours come off a real GPU-less
    renderer and through a shim, so those get the 4-bit tolerance; the card's
    do not -- `render::rasterize` writes `CARD_BG`/`ACCENT` into a CPU buffer
    with `copy_from_slice` and `readback_region` reads that buffer back, so an
    exact count is a check on identity, not on similarity.
    """
    return sum(
        1
        for off in range(0, len(buf), 4)
        if (buf[off], buf[off + 1], buf[off + 2], buf[off + 3]) == want
    )


def _px(buf: bytes, width: int, x: int, y: int) -> tuple[int, int, int, int]:
    """One pixel of a raw-RGBA buffer, as a 4-tuple."""
    off = (y * width + x) * 4
    return (buf[off], buf[off + 1], buf[off + 2], buf[off + 3])


def _crop_rgba(buf: bytes, size: tuple[int, int], rect: tuple[int, int, int, int]) -> bytes:
    """The `rect` sub-image of a raw-RGBA frame, as raw RGBA."""
    width, _height = size
    x, y, w, h = rect
    out = bytearray()
    for row in range(h):
        start = ((y + row) * width + x) * 4
        out += buf[start : start + w * 4]
    return bytes(out)


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


class RealConsentPrompt(IntegrationTest):
    """The M1.4 consent exit gate: a real prompt over a real app, answered
    from outside the process, occluding the human's view and never the
    agent's, and holding that principal's actuations while it is up."""

    #: The colours `click-target` paints (channels multiples of 0x11, so the
    #: quantised histogram reads them back exactly). Green target on a black
    #: field; a landed click repaints the whole surface red, permanently.
    TARGET = "00ff00"
    HIT = "ff0000"

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
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-consent-"))
        self.addCleanup(shutil.rmtree, self.work, True)
        self.dump = str(self.work / "internal.rgba")
        self.core_log = self.work / "core.log"

    # -- the core, under the one policy this gate is about -----------------

    def real_core(self):
        """A real chain under `--consent=interactive`, with the channel wired.

        Deliberately NOT the suite's `auto-approve` default: under
        auto-approve no prompt is ever drawn and no decision is ever taken, so
        every assertion below would pass over a session that never exercised
        the consent surface at all. That is the vacuity this module exists to
        remove, so the policy is named here rather than inherited.
        """
        try:
            return self.core(
                consent="interactive",
                size=REALM_SIZE,
                shim=str(self.shim_bin),
                command=self.app_bin,
                args=["--run-ms", "90000"],
                env_allow=tuple(WLR_ENV),
                extra_env=WLR_ENV,
                log_file=str(self.core_log),
                capture_dump=self.dump,
                consent_injector=True,
            )
        except CoreFailed as exc:
            self.fail(f"{_WRONG_BUILD}\n\nThe core's own words:\n{exc}")

    def _assert_instrumented(self, core) -> ConsentInjector:
        """C2: a RUNNING instrumented core is identifiable by how it was
        invoked, not only by how it was built.

        Four independent tells, all inspectable from outside the process. The
        first is the load-bearing one -- the flag NAMES A RESOURCE, so it
        cannot be a stray boolean left in a wrapper script, and a `ps` on the
        box answers the question.
        """
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
            f"the injector descriptor must be an unnamed socket, not {target!r} -- a channel "
            "with a filesystem name would be connectable by the confined app, which runs as "
            "this core's own uid",
        )
        injector = core.injector
        assert isinstance(injector, ConsentInjector)
        try:
            banner = injector.await_banner()
        except Exception as exc:  # noqa: BLE001 -- the diagnostic is the point
            self.fail(f"{_WRONG_BUILD}\n\nThe channel said: {exc}")
        self.assertEqual(banner, "vitrin-consent-injector 1")
        log = self.core_log.read_text(errors="replace")
        self.assertIn(
            "CONSENT INJECTOR IS WIRED",
            log,
            "an instrumented session must carry a standing warning in its own log; a run whose "
            "log did not distinguish itself could be read as one a human answered",
        )
        return injector

    def _spine(self, core) -> None:
        """Wait out `vitrind -> vitrin-shim -> click-target`, matching the rest
        of the real-app ladder."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            if core.proc.poll() is not None:
                self.fail(
                    f"the core exited {core.proc.returncode} instead of serving.\n"
                    f"{_WRONG_BUILD}\n{core.output()}"
                )
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r}",
        )
        app_pid = descendant_named(core.pid, "click-target", timeout=15.0)
        self.assertIsNotNone(app_pid, "the C shim never fork/exec'd click-target")

    def _settle(self) -> bytes:
        """Block until the real app's frame is on screen AND unchanging.

        Returns the settled realm-view dump, which is this gate's control for
        the byte-equality assertion. Taking it before the app has stopped
        painting is exactly the P1.9.8 gate-integrity failure: a still-moving
        app can forge the later "nothing changed" evidence.
        """
        deadline = time.monotonic() + 30.0
        stable = 0
        last: bytes | None = None
        while time.monotonic() < deadline:
            frame = _read_dump(self.dump, REALM_WH)
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
            "a settled control this gate's 'raising the prompt moved zero pixels of the capture "
            "path' assertion would be about a moving target."
        )

    # -- agent-side helpers -------------------------------------------------

    def _petition(self, conn, persistence=None):
        """Send a petition and return the pending grant WITHOUT waiting.

        `harness.whole_realm_grant` blocks in `await_consent()`, which is
        exactly wrong here: the interesting window is the one where the
        petition is pending and the prompt is up, and a helper that waits it
        out would skip past it.
        """
        return conn.request_grant(
            verbs=("observe", "actuate.pointer", "actuate.text"),
            persistence=persistence or vitrin_os.Persistence.WHILE_RUNNING,
        )

    def _assert_is_a_real_card_raster(
        self, window: bytes, card: tuple[int, int, int, int]
    ) -> tuple[int, int, set[bytes]]:
        """The POSITIVE control on the exported occlusion window.

        Proof 2's occlusion half is an ABSENCE -- "no pixel of the app's green
        is in this rectangle" -- and an absence is equally true of an empty
        buffer, a synthetic one, or a rectangle of some other part of the
        screen. Without this, a `consent_occlusion_window` that returned
        `vec![0u8; w*h*4]` and never touched the framebuffer at all would keep
        the gate green while printing the same success line, which is exactly
        how a gate comes to be cited for a property it never checked.

        The harness has no independent view of the human-visible framebuffer,
        and must not have one: that is C1 (issue #85). Whole-frame readback is
        `#[cfg(test)]` precisely so no running build can be asked for one. So
        the provenance of these bytes has to come from the bytes: they have to
        BE a raster of vitrind's consent card, laid out on exactly the
        rectangle the core named.

        Four independent things are checked, in rising strength:

        1. **The accent ring is on the exported rectangle's four edges.**
           `render::rasterize` strokes it `BORDER` px wide along the edges of
           the CARD image, so this is positional: a readback that landed a
           pixel off the card's footprint, or on a different region, or on
           nothing, does not have the ring on its border. This is what ties
           the delivered bytes to the reported geometry -- the geometry
           assertions above are otherwise the core's own numbers checked
           against each other.
        2. **The accent appears nowhere else**, by exact pixel count against
           the perimeter formula -- so it is a frame, not a fill.
        3. **The card body and its BUTTONS are there**: `CARD_BG` dominant,
           and both button colours present. A consent card with no buttons
           would not be one a human could answer.
        4. **The raster carries antialiased text** (`MIN_CARD_COLOURS`
           distinct colours). A flat forgery -- fill, ring, two rectangles --
           cannot reach it.

        What this does NOT prove is that some *other* correct card raster was
        not substituted for this one; under C1 no assertion available to this
        harness can, and pretending otherwise is the failure this docstring
        exists to avoid.

        Returns `(accent_px, card_bg_px, distinct_colours)` so the run's
        summary line can state what was checked instead of only the absence.
        """
        _, _, cw, ch = card
        self.assertEqual(len(window), cw * ch * 4, "the export is the whole card footprint")

        # 1. The ring, positionally: every pixel of the outermost BORDER-wide
        #    band on all four edges.
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
        # ...and the ring really is BORDER thick, so it is a frame drawn on a
        # card and not a flood fill that happens to reach the edges.
        self.assertEqual(
            _px(window, cw, BORDER, BORDER),
            CARD_BG,
            "the pixel just inside the accent ring must be the card's background",
        )

        # 2. Exactly the perimeter, nowhere else.
        ring = cw * ch - (cw - 2 * BORDER) * (ch - 2 * BORDER)
        accent_px = _exact_rgba(window, ACCENT)
        self.assertEqual(
            accent_px,
            ring,
            f"the accent colour must occupy exactly the {ring}px border ring of a "
            f"{cw}x{ch} card and nothing else",
        )

        # 3. The card body, and the buttons a human is meant to press.
        body = _exact_rgba(window, CARD_BG)
        self.assertGreater(
            body,
            cw * ch // 2,
            f"only {body} of {cw * ch} px are the card's background: the export is not an "
            "opaque consent card",
        )
        self.assertGreater(
            _exact_rgba(window, BUTTON_BG), 0, "the card must carry its button row"
        )
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
        return accent_px, body, colours

    def _locate_target(self, grant, timeout=20.0):
        """Observe until the green target is on screen; return `(frame, cx, cy)`."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            cx, cy, count = locate_colour(frame, self.TARGET)
            if count >= MIN_TARGET_PIXELS:
                return frame, cx, cy
            time.sleep(0.1)
        self.fail(
            f"the green target never reached the agent within {timeout:.0f}s: click-target's "
            "frame did not arrive, so there is nothing to click"
        )

    def _await_flip(self, grant, timeout=20.0) -> int:
        """Observe until the app's surface is the HIT colour, or fail loudly."""
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            colour, pct = dominant_colour(frame)
            if not seen or seen[-1] != colour:
                seen.append(colour)
            if colour == self.HIT and pct > 90:
                return pct
            time.sleep(0.1)
        self.fail(
            f"the surface never flipped to #{self.HIT} within {timeout:.0f}s after the "
            "post-denial click. THE `ConsentHeld` REFUSAL EARLIER IN THIS TEST THEREFORE PROVES "
            "NOTHING: this grant's click does not land even with no prompt up, so the refusal "
            "may have had nothing to do with consent. Dominant-colour sequence: "
            + " -> ".join("#" + c for c in seen)
        )

    # -- the gate ----------------------------------------------------------

    def test_a_real_prompt_occludes_the_human_view_holds_actuation_and_is_answered(self):
        core = self.real_core()
        injector = self._assert_instrumented(core)
        self._spine(core)
        settled = self._settle()

        # == Proof 1: a real petition raises a real prompt over a real scene ==
        #
        # Everything after this depends on the decision path actually working,
        # so a broken channel fails HERE rather than making a later assertion
        # pass vacuously.
        actor = core.connect()
        grant = self._petition(actor)
        id1, tok1 = injector.await_raised()

        # The core's own record, read mid-run (uncached).
        shown_now = [
            e
            for e in core.entries()
            if e["kind"] == "consent_transition"
            and e.get("state") == "shown"
            and e.get("petition") == id1
        ]
        self.assertTrue(
            shown_now,
            f"the flight recorder must journal {id1}'s prompt as shown; got "
            f"{[e for e in core.entries() if e['kind'] == 'consent_transition']}",
        )
        fields, _pixels = injector.describe()
        self.assertEqual(fields["state"], "shown")
        self.assertEqual(fields["token"], tok1)

        self.assertEqual(injector.decide(tok1, "allow-while-running"), "queued")
        injector.await_lowered(id1)
        grant.await_consent()
        self.assertEqual(
            grant.effective_persistence,
            vitrin_os.Persistence.WHILE_RUNNING,
            "the injected button must confer the rung it named, not a wider or narrower one",
        )
        # The wire's own view of the same event. The SDK's public
        # `consent_state` is the LATEST state, which by now is `closed`
        # (queued -> shown -> closed is the sequence `principal.rs` emits), and
        # `closed` is sent on every resolution whether or not a card was ever
        # drawn -- so it would be true of a session that showed nothing. The
        # sequence is what carries the claim, and the SDK keeps it on a
        # private list; reading it here is deliberate and is the only private
        # attribute this gate touches.
        self.assertIn(
            vitrin_os.ConsentState.SHOWN,
            grant._consent_states,  # noqa: SLF001 -- see comment above
            "the petitioner must have been told on the wire that its prompt went up; the SDK "
            f"saw {grant._consent_states}",
        )

        # The grant is live against the real app: the agent finds
        # click-target's real green square in its own captured frame (D10).
        frame, cx, cy = self._locate_target(grant)
        self.assertEqual((frame.width, frame.height), REALM_WH)

        # == Proof 2 + 3's window: a SECOND petition, on a second connection ==
        #
        # The IDL keys `consent_held` on the *principal's* pending prompt, and
        # a principal spanning several connections is one principal -- so the
        # FIRST connection's already-granted authority is what the prompt must
        # hold. Doing it on one connection would prove less: the held grant
        # and the pending petition would share a socket, and "that connection
        # is busy mid-petition" would be a competing explanation.
        watcher = core.connect()
        pending = self._petition(watcher)
        id2, tok2 = injector.await_raised()
        self.assertNotEqual(id1, id2)
        self.assertNotEqual(tok1, tok2, "each prompt is named by a fresh token")

        # -- Proof 2, at one instant -----------------------------------------
        fields, window = injector.describe()
        self.assertEqual(fields["state"], "shown")
        self.assertEqual(fields["token"], tok2)
        self.assertIsNotNone(window, "a raised prompt must export its footprint")

        # Geometry, cross-checked against what the core CANNOT choose: the
        # card's one true constant, the centring rule, and the band. The core
        # cannot hand back a conveniently-chosen rectangle and still pass.
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
        self.assertEqual(len(window), card[2] * card[3] * 4)

        # The card really covers the app's target, so "the human sees the card
        # where the app is painting" is a statement about this app's pixels.
        self.assertTrue(
            card[0] <= cx < card[0] + card[2] and card[1] <= cy < card[1] + card[3],
            f"the located target ({cx},{cy}) must fall inside the card's footprint {card}; "
            "otherwise the occlusion assertion below is about empty space",
        )

        # A0 = the POSITIVE control on those bytes, before any absence is read
        # out of them. See `_assert_is_a_real_card_raster`: "no green here" is
        # a statement about the human-visible framebuffer only if these bytes
        # ARE the human-visible framebuffer's card footprint, and the only
        # evidence for that available under C1 is the bytes' own content.
        ring_px, body_px, window_colours = self._assert_is_a_real_card_raster(window, card)

        # A = the human-visible output, card footprint. Nothing of the app's
        # target survives inside the rectangle just shown to be the card.
        green_a = _count_rgba(window, self.TARGET)
        self.assertEqual(
            green_a,
            0,
            f"the human-visible output carries {green_a} px of the app's target inside the "
            "card's own footprint: the prompt is not occluding the app",
        )

        # B = the same rectangle sliced out of the core-internal realm view.
        # The capture path still shows the target at exactly those coordinates.
        dump_up = _read_dump(self.dump, REALM_WH)
        green_b = _count_rgba(_crop_rgba(dump_up, REALM_WH, card), self.TARGET)
        self.assertGreaterEqual(
            green_b,
            MIN_TARGET_PIXELS,
            f"the realm view must still show the target under the card (saw {green_b} px); "
            "the overlay has reached the image captures are served from",
        )

        # ...and raising the prompt moved ZERO pixels of the capture path.
        if dump_up != settled:
            changed = sum(
                1
                for off in range(0, len(dump_up), 4)
                if dump_up[off : off + 3] != settled[off : off + 3]
            )
            green_now = _count_rgba(dump_up, self.TARGET)
            hit_now = _count_rgba(dump_up, self.HIT)
            self.fail(
                f"raising the consent prompt changed {changed} px of the CAPTURE path "
                f"(green {_count_rgba(settled, self.TARGET)} -> {green_now}, hit px {hit_now}). "
                "The human-visible overlay must not reach the realm view at all."
            )

        # C = the agent's own mid-prompt capture. `observe()` succeeding at
        # all is the IDL's "`consent_held` never refuses a capture".
        held_frame = grant.observe()
        green_c = locate_colour(held_frame, self.TARGET)[2]
        self.assertGreaterEqual(
            green_c,
            MIN_TARGET_PIXELS,
            f"the agent's mid-prompt capture shows only {green_c} green px: it is not a live "
            "view of the real app, so 'no card in it' would prove nothing",
        )
        # ...and it agrees with the core-internal capture through the M1.3
        # gate's own comparator, rather than a second, driftable byte compare.
        agent_path = self.work / "agent.xrgb"
        agent_path.write_bytes(packed_xrgb(held_frame))
        dump_path = self.work / "dump.rgba"
        dump_path.write_bytes(dump_up)
        cmp_result = golden_cmp(
            str(agent_path),
            str(dump_path),
            REALM_WH,
            "tol:1,0.001",
            artifacts=str(self.work / "cmp-art"),
        )
        self.assertEqual(
            cmp_result.returncode,
            0,
            "the agent's mid-prompt frame must agree with the core-internal realm-view capture: "
            "if the two disagree while a prompt is up, one of them is carrying the overlay.\n"
            f"{cmp_result.stdout.strip()}\n{cmp_result.stderr.strip()}",
        )

        # -- Proof 3: the hold, and why it is about consent ------------------
        #
        # Fact 1: the code is `ConsentHeld`, SPECIFICALLY. `Revoked`,
        # `Expired`, `RateLimited` and `NoSurface` all fail this test, which
        # is what makes it NAME consent as the reason rather than assume it.
        with self.assertRaises(errors.ConsentHeld) as held:
            grant.pointer.click(cx, cy)
        self.assertEqual(held.exception.verb, vitrin_os.Verb.ACTUATE_POINTER)
        # Fact 2 is the `observe()` above: the grant is provably still good in
        # this same window, so `consent_held` is separated from every other
        # refusal cause POSITIVELY -- a revoked, expired, rate-limited or
        # never-granted grant would have refused that capture too.
        #
        # Fact 3: the app's own observable side effect. Read the CORE-INTERNAL
        # capture, which no grant, wire frame or SDK decode stands in front of.
        dump_held = _read_dump(self.dump, REALM_WH)
        hit_px = _count_rgba(dump_held, self.HIT)
        green_held = _count_rgba(dump_held, self.TARGET)
        self.assertEqual(
            hit_px,
            0,
            f"click-target shows {hit_px} px of its hit colour after the held click: a click "
            "that reached its wl_seat repaints the whole surface red, permanently (D10)",
        )
        self.assertGreaterEqual(green_held, MIN_TARGET_PIXELS, "the target must still be green")

        # Fact 5 (the positive control) is LAST, because click-target's flip is
        # one-way and permanent: a successful control click destroys the
        # green-target evidence every assertion above rests on.
        self.assertEqual(injector.decide(tok2, "deny"), "queued")
        with self.assertRaises(errors.GrantDenied):
            pending.await_consent()
        injector.await_lowered(id2)

        grant.pointer.click(cx, cy)
        flipped = self._await_flip(grant)

        actor.close()
        watcher.close()
        core.terminate()
        entries = core.entries()

        # == Proof 4: the real state machine, and the journal =================
        run_started = [e for e in entries if e["kind"] == "run_started"]
        self.assertTrue(run_started)
        self.assertEqual(
            run_started[0].get("consent_policy"),
            "interactive+consent-injector",
            "an instrumented run must brand its own journal: the injected decision correctly "
            "journals `human_consent` because it really did traverse `resolve_human`, so "
            "without this marker the run would be indistinguishable from a human-answered one",
        )

        shown = [
            e
            for e in entries
            if e["kind"] == "consent_transition" and e.get("state") == "shown"
        ]
        self.assertGreaterEqual(
            len(shown), 2, f"both petitions must have journalled a shown prompt; got {shown}"
        )
        resolved = [e for e in entries if e["kind"] == "petition_resolved"]
        granted = [e for e in resolved if e.get("outcome") == "granted"]
        denied = [e for e in resolved if e.get("outcome") == "denied"]
        self.assertTrue(granted, f"the allowed petition must journal a grant; got {resolved}")
        self.assertTrue(denied, f"the denied petition must journal a denial; got {resolved}")
        for entry in granted:
            self.assertEqual(
                entry.get("issuer"),
                "human_consent",
                "the injected decision must be issued by the HUMAN consent path "
                "(`resolve_human`). `scripted_consent` would mean it took `scripted-consent`'s "
                "in-process shortcut and `auto_approve_policy` that no human path ran at all; "
                f"either is a second decision entry point: {entry}",
            )

        # Fact 4 of proof 3: the chokepoint's own record of the refusal,
        # ORDERED against the prompt's lifetime. A refusal recorded before the
        # card went up, or after it came down, fails -- which is what turns
        # "the code string was consent_held" into a claim about WHEN.
        kinds = [e["kind"] for e in entries]
        shown_at = [
            i
            for i, e in enumerate(entries)
            if e["kind"] == "consent_transition"
            and e.get("state") == "shown"
            and e.get("petition") == id2
        ]
        self.assertTrue(shown_at, f"{id2}'s prompt must be journalled as shown; kinds={set(kinds)}")
        resolved_at = [
            i
            for i, e in enumerate(entries)
            if e["kind"] == "petition_resolved" and e.get("outcome") == "denied"
        ]
        self.assertTrue(resolved_at, "the denied petition must be journalled")
        # Either journal shape counts: a lone refusal is a `use_decision`, and
        # a run of identical ones is additionally summarised by
        # `use_refusal_summary` (the recorder's refusal bounding). Accepting
        # both means this assertion tests the refusal, not the bounding policy.
        refusals = [
            i
            for i, e in enumerate(entries)
            if e["kind"] in ("use_decision", "use_refusal_summary")
            and e.get("refusal") == "consent_held"
        ]
        self.assertTrue(
            refusals,
            "the enforcement chokepoint must have recorded a `consent_held` refusal; "
            f"kinds seen: {sorted(set(kinds))}",
        )
        self.assertTrue(
            any(shown_at[0] < i < resolved_at[-1] for i in refusals),
            "the `consent_held` refusal must fall strictly between the prompt going up and the "
            f"petition resolving: shown at {shown_at}, resolved at {resolved_at}, refusals at "
            f"{refusals}. A refusal outside that window was not about this prompt.",
        )

        print(
            f"\n[real-consent] prompt {id1} raised over a real click-target and allowed via "
            f"resolve_human (while_running). Prompt {id2} raised: the export at {card} was "
            f"checked to BE a card raster there ({ring_px} px of accent border on all four "
            f"edges, {body_px} px of card background, {len(window_colours)} distinct colours) "
            f"and carried {green_a} px of the app's target, the realm view carried "
            f"{green_b} px at those same coordinates and was byte-identical to the settled "
            f"control, the agent's mid-prompt observe() still showed {green_c} px and agreed "
            f"with the core-internal capture. pointer.click refused ConsentHeld and the app "
            f"stayed green ({green_held} px, 0 hit px). After deny, the identical click landed "
            f"and the surface flipped to #{self.HIT} at {flipped}%"
        )


class ConsentGateThresholdsStayDiscriminating(unittest.TestCase):
    """Binary-free: pin this gate's one computed threshold against the real
    feature it scores, so it cannot be relaxed back into vacuity.

    Follows `test_demo.py`'s `HeadlessGateThresholdsStayDiscriminating`. A
    `MIN_TARGET_PIXELS` of 0 would make "the target is on screen" true of a
    black frame; one above the target's true area would make it unsatisfiable
    and turn every real-app assertion into a timeout with no explanation.
    """

    def test_min_target_pixels_sits_strictly_inside_the_targets_real_area(self):
        self.assertEqual(TARGET_AREA, 25_600, "click_target.c's TARGET_SIZE is 160")
        self.assertGreater(MIN_TARGET_PIXELS, 0, "zero green px must not count as 'located'")
        self.assertLess(
            MIN_TARGET_PIXELS,
            TARGET_AREA,
            "the bar must be reachable by the square the app actually paints",
        )

    def test_the_settle_loop_cannot_accept_a_moving_app(self):
        self.assertGreaterEqual(
            SETTLE_READS,
            2,
            "a single read is not a settle: the control capture would be taken while the app "
            "was still painting, which is the P1.9.8 gate-integrity failure exactly",
        )
        self.assertGreater(SETTLE_INTERVAL, 0.0)

    def test_the_card_width_constant_matches_the_renderer(self):
        source = (
            pathlib.Path(__file__).resolve().parents[2]
            / "crates/vitrin-core/src/consent/render.rs"
        ).read_text()
        self.assertIn(
            f"pub(crate) const CARD_WIDTH: u32 = {CARD_WIDTH};",
            source,
            "this gate hard-codes the card's WIDTH (the one dimension that really is a "
            "constant; the height is content-derived). If the renderer's constant moved, the "
            "geometry cross-check would be asserting the wrong number.",
        )

    def test_the_card_raster_constants_match_the_renderer(self):
        """Every colour the positive control looks for is still one the card
        is painted in.

        `_assert_is_a_real_card_raster` is the whole provenance argument for
        the exported occlusion window, and it is written in terms of exact
        RGBA values. A restyled card would leave every one of those searches
        finding nothing -- which fails loudly, so this is not a soundness
        hole, but it would fail with "no accent border" rather than "the
        renderer's palette moved". This says which.
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
        ):
            self.assertIn(
                rgba(name, value),
                source,
                f"consent::render::{name} moved; the positive control on the exported "
                "occlusion window is searching for a colour the card no longer carries",
            )
        self.assertIn(
            f"const BORDER: u32 = {BORDER};",
            source,
            "the accent ring's thickness moved; the exact perimeter count and the "
            "just-inside-the-ring check are both computed from it",
        )

    def test_the_card_colour_thresholds_stay_discriminating(self):
        """The positive control's two numeric bars are reachable and not free.

        `MIN_CARD_COLOURS` of 1 would be true of a solid fill; above the ~824
        a real card measures it would be unsatisfiable and every run would
        fail with a message about text antialiasing rather than about consent.
        """
        self.assertGreater(MIN_CARD_COLOURS, 8, "a flat forgery has a handful of colours")
        self.assertLess(
            MIN_CARD_COLOURS,
            824,
            "the real export measures 824 distinct colours; the bar must sit under it",
        )
        self.assertGreaterEqual(BORDER, 1, "a zero-thickness ring would make the ring check free")


if __name__ == "__main__":
    unittest.main()
