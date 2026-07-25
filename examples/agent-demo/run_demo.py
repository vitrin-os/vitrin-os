#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Phase 1's integrating demo agent — and the M1.5 acceptance test.

One script, two venues, exactly the same agent code path in both:

    connect (the static demo identity)
      -> request the ONE MVP grant (observe + actuate.pointer + actuate.text
         on realm-0, `while-running`)
      -> await consent (a human clicks Allow in nested; a guarded
         auto-approve resolves it headless)
      -> capture a "before" frame (headless: settle the app first, so the
         "before" is a real control capture and not a frame racing the app's
         own startup paint)
      -> locate the input target by pixels
      -> click it, type text, press Enter (the trailing "\\n")
      -> capture an "after" frame
      -> assert the page changed *because of the actuation*.

Both venues run the SAME real chain: the shipped `vitrind` execs the real
per-app Wayland shim (`vitrin-shim`, issue #103/#104), which fork/execs a real
app inside its own private, confined Wayland socket (issue #110) —
`vitrin-mock-shim` is a unit-test fixture and stands in for nothing here. The
two venues differ only in *which real app* stands behind the shim and in *how
"the page changed" is proven*, never in the agent's protocol conduct:

* **Nested** (`cargo xtask demo`): a real Firefox ESR runs in realm-0, behind
  the real shim, on the host's real display. This script serves a
  deterministic solid-colour page from a stdlib ``http.server`` bound to
  ``127.0.0.1:0`` on a daemon thread, types that local URL into Firefox's URL
  bar, and proves the navigation happened by the *dominant colour* of the
  after-frame matching the colour it served (robust to anti-aliasing via
  colour bucketing). A human answers the consent prompt. This venue needs a
  display, a browser, and a built C shim; it is correct-by-construction here
  but is exercised on a workstation, never in CI.

* **Headless** (`cargo xtask demo --headless`, and CI): a real, trivial
  Wayland client (`weston-terminal`, or `foot`) runs in realm-0, behind the
  real shim — GPU-free, so CI never depends on Firefox or a GPU (plan risk
  R6/R1). "The page changed" here has to mean *the typed text landed*, not
  merely "the app repainted", because a real app is always repainting
  something. Three things make it mean that: the app is **settled** before the
  control capture (:func:`_settle`), so its asynchronous first paint and shell
  prompt are inside the "before" frame rather than straddling it; the settled
  app is then watched, idle, for at least as long as the gate later polls
  (:func:`_idle_probe`), so an app that can forge the gate's own signature
  unaided fails the run instead of passing it; and the change that follows
  must carry the *shape* a typed line makes — enough pixels to rule out a
  one-cell repaint AND a densely inked run of them along one scanline, wide
  enough that only a row of characters could have drawn it
  (:func:`actuation_landed`).
  What additionally proves the actuation *causally reached the app* is the
  flight recorder's ``use_decision`` entries — an allowed ``move`` at the
  clicked coordinate and an allowed ``type`` whose ``chars`` equals the typed
  text's length — exactly as ``tests/integration/test_actuation.py``
  establishes.

The consent guard (``--consent=auto-approve``) is only sound because the
principal registry the launcher writes holds *nothing but* the one demo
principal (plan risk R6). The launcher — ``cargo xtask demo [--headless]`` —
owns that invariant; this script only presents the matching credential.

Pure stdlib, Python >= 3.11, zero runtime dependencies — the same posture the
SDK holds (decision D8). No Pillow, no requests: frames are written with
``Frame.to_png`` and the local page is served with ``http.server``.
"""

from __future__ import annotations

import argparse
import http.server
import os
import pathlib
import socketserver
import sys
import tempfile
import threading
import time
from collections import Counter
from dataclasses import dataclass

import vitrin_os
from vitrin_os import errors


# --- identity ---------------------------------------------------------------

#: The static demo identity, matching ``examples/principals.toml`` and the
#: registry the launcher writes. Auto-approve is only permitted when the
#: registry holds exactly this one principal (R6), so the launcher writes a
#: one-row registry and this script presents its credential.
DEMO_IDENTITY = "vitrin://local/agent/demo"

#: The pre-shared token. 64 hex chars: the core refuses tokens under 16 bytes,
#: and this must byte-for-byte match what the launcher writes into the
#: throwaway ``principals.toml`` (see ``crates/xtask``). Kept identical to the
#: integration harness's ``DEMO_TOKEN`` so the demo and the suite agree.
DEMO_TOKEN = "a" * 64


# --- the served page (nested venue) -----------------------------------------

#: The solid colour the local page paints, as ``(R, G, B)``. Chosen distinct
#: from Firefox chrome and from ``about:blank`` white so the dominant-colour
#: assertion has real signal.
SERVED_RGB = (0x33, 0x66, 0xCC)

#: A deterministic, self-contained page: a full-viewport solid fill, no
#: external resources, no scrollbars. The whole point is that its dominant
#: colour is exactly :data:`SERVED_RGB`.
_SOLID_PAGE = (
    "<!doctype html><html><head><meta charset='utf-8'>"
    "<title>vitrin demo</title>"
    "<style>html,body{{margin:0;padding:0;width:100%;height:100%;"
    "overflow:hidden;background:#{r:02x}{g:02x}{b:02x}}}</style>"
    "</head><body></body></html>"
).format(r=SERVED_RGB[0], g=SERVED_RGB[1], b=SERVED_RGB[2]).encode("utf-8")

#: The text the headless venue types into the real terminal app. Not a URL —
#: there is no browser headless — just a harmless shell line: the real app
#: renders it (and its shell's response, whatever that is), which is what the
#: pixel-change assertion needs, and its *length* reaches the flight recorder
#: (typed text is recorded by shape, never verbatim). A stable literal keeps
#: the recorder assertion deterministic.
DEFAULT_HEADLESS_INPUT = "echo vitrin-demo"


class _SolidPageHandler(http.server.BaseHTTPRequestHandler):
    """Serve :data:`_SOLID_PAGE` for every GET; stay silent otherwise."""

    def do_GET(self) -> None:  # noqa: N802 (http.server's fixed spelling)
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(_SOLID_PAGE)))
        self.end_headers()
        self.wfile.write(_SOLID_PAGE)

    def log_message(self, *_args: object) -> None:
        # A demo's stdout is its transcript; the http server's per-request
        # noise does not belong in it.
        pass


class _LocalPage:
    """A stdlib ``http.server`` on ``127.0.0.1:0``, served from a daemon thread.

    Bound to an ephemeral port and to loopback only: nothing this demo serves
    should be reachable off the machine. The thread is a daemon so a crashed
    demo never wedges the process on a live server, and :meth:`close` is
    idempotent so every teardown path can call it.
    """

    def __init__(self) -> None:
        self._server = socketserver.TCPServer(("127.0.0.1", 0), _SolidPageHandler)
        self._thread = threading.Thread(
            target=self._server.serve_forever, name="vitrin-demo-httpd", daemon=True
        )
        self._thread.start()

    @property
    def url(self) -> str:
        host, port = self._server.server_address[:2]
        return f"http://{host}:{port}/"

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=2.0)


# --- URL-bar locator --------------------------------------------------------

#: The window size Firefox ESR is pinned to in nested mode — the nested
#: backend's initial window (``vitrind``'s ``DEFAULT_HEADLESS_SIZE``, which the
#: nested window matches). Used to pick the fixed URL-bar coordinate below.
ESR_WINDOW_SIZE = (1280, 800)

#: Fixed URL-bar target for the pinned ESR at :data:`ESR_WINDOW_SIZE`: the
#: horizontal centre, in the toolbar band a few dozen pixels below the top.
#: Deliberately a documented constant, not a vision model — version 1 has no
#: semantic tree, so a pinned browser is located by geometry.
ESR_URL_BAR = (640, 72)

#: Vertical band the URL bar sits in, used by the proportional fallback when
#: the window is some size other than :data:`ESR_WINDOW_SIZE`.
ESR_TOOLBAR_Y = 72

#: Headless nominal Y: the real terminal app has no URL bar, so any in-bounds
#: point serves — what matters headless is that *some* coordinate reaches the
#: chokepoint and is recorded. Kept near the top for parity with a real
#: toolbar, and clamped into the frame below.
HEADLESS_NOMINAL_Y = 16


def locate_url_bar(frame: vitrin_os.Frame, *, headless: bool) -> tuple[int, int]:
    """Pick the pixel to click. Small and honest by construction.

    Headless: a nominal in-bounds coordinate (the real terminal app models no
    URL bar). Nested: the fixed geometry of the pinned ESR when the window is
    the pinned size, with a proportional fallback for any other size.
    """
    width, height = frame.width, frame.height
    if headless:
        return (width // 2, min(HEADLESS_NOMINAL_Y, height - 1))
    if (width, height) == ESR_WINDOW_SIZE:
        return ESR_URL_BAR
    # Fallback: centre horizontally, clamp the toolbar band into the frame.
    return (width // 2, min(ESR_TOOLBAR_Y, height - 1))


# --- pixel analysis (nested "page changed") ---------------------------------

def dominant_color(
    frame: vitrin_os.Frame, *, bucket: int = 16, step: int = 4
) -> tuple[int, int, int]:
    """The most common colour in the frame, bucketed to absorb anti-aliasing.

    Reads ``frame.raw`` directly — little-endian xrgb8888, so each pixel is
    the four bytes ``B, G, R, X`` (the SDK's ``png.xrgb8888_to_rgb`` documents
    the same layout). Channels are quantised into ``bucket``-wide bins so a
    solid fill and its anti-aliased edges land in one bucket, and pixels are
    sub-sampled every ``step`` in each axis to keep this cheap on a 1280x800
    frame without changing which colour dominates.
    """
    raw = frame.raw
    stride, width, height = frame.stride, frame.width, frame.height
    counts: Counter[tuple[int, int, int]] = Counter()
    for row in range(0, height, step):
        base = row * stride
        for px in range(0, width, step):
            off = base + px * 4
            b, g, r = raw[off], raw[off + 1], raw[off + 2]
            counts[(r // bucket, g // bucket, b // bucket)] += 1
    (rk, gk, bk), _ = counts.most_common(1)[0]
    half = bucket // 2
    return (rk * bucket + half, gk * bucket + half, bk * bucket + half)


def colors_close(a: tuple[int, int, int], b: tuple[int, int, int], *, tol: int = 24) -> bool:
    """True if every channel is within ``tol`` — the AA/compression slack."""
    return all(abs(x - y) <= tol for x, y in zip(a, b))


# --- pixel analysis (headless "page changed") -------------------------------
#
# The headless venue's whole job is to tell the agent's actuation apart from
# the app's own repaint, and a bare "some pixels changed" cannot do that: a
# real app is *always* repainting something. The threshold here used to be 24
# changed pixels in a 640x480 frame, which weston-terminal clears on its own
# — its asynchronous first paint alone rewrites the whole view, and it can
# easily land between the demo's two captures — so the named M1.5 gate passed
# whether or not the click and the typed text ever reached the app.
#
# Three things fix that, and all three are needed:
#
#   1. The app is SETTLED before the control capture (:func:`_settle`), so
#      startup paint is finished and inside the "before" frame, not straddling
#      it. That removes the race, rather than trying to out-threshold it.
#   2. The settled app is then left strictly alone for an IDLE PROBE at least
#      as long as the window the gate later polls (:func:`_idle_probe`). If
#      the app can draw an actuation-shaped diff with nobody actuating, this
#      gate cannot discriminate on this runner, and it says so instead of
#      passing.
#   3. The change that follows must carry the *shape* the typed text makes:
#      not merely enough pixels, but a densely inked chain of them along one
#      scanline that only a row of characters could have drawn.
#
# The thresholds below are derived from the input the demo types, not from
# one lucky measurement, so they do not have to be retuned per runner.


#: Minimum changed pixels for the headless "the page changed" claim.
#:
#: Justification: a filled character cell in a terminal at any plausible
#: font size is roughly 8x17 = ~140 pixels, and glyphs ink well under half
#: of that, so a blinking cursor or a single re-drawn glyph tops out around
#: one cell. 400 is ~3 cells: unreachable by incidental one-cell repaint,
#: and far below what the demo's own input produces — typing
#: ``echo vitrin-demo`` + Enter into a real weston-terminal on a 640x480
#: pixman view measured 1703 changed pixels across 49 scanlines, i.e. a 4x
#: margin. Deliberately not tuned close to that measurement: a gate that
#: fails randomly gets muted, which is how gates die.
MIN_HEADLESS_CHANGED_PIXELS = 400

#: Two changed pixels on one scanline belong to the same *run* when they lie
#: no more than this many pixels apart.
#:
#: Typed text does not ink a solid bar. Within a line of monospace glyphs the
#: ink is broken by intra-glyph whitespace and by blank cells (a space
#: character inks nothing at all), so a *strictly contiguous* run through a
#: real typed line is only a handful of pixels long — measured 9 px for
#: ``echo vitrin-demo`` in a real weston-terminal on a 640x480 pixman view. A
#: contiguous-run threshold would therefore be unreachable by the very thing
#: this gate demands. What a typed line does guarantee is that its ink never
#: skips more than about one blank cell: measured maximum gap inside that same
#: echoed line, 15 px. 24 px covers a blank cell at any plausible terminal
#: font size in a 640x480 view, while staying far below the distance between
#: independent widgets — a cursor at the left margin, a mode indicator
#: mid-line and a scrollbar at the right edge sit hundreds of pixels apart and
#: can never chain into one run.
RUN_GAP_TOLERANCE = 24

#: A run counts only if at least this fraction of the pixels it covers
#: actually changed. Without it the gap tolerance alone would accept a dotted
#: artefact — one changed pixel every 24 — as a "run". A real row of glyphs
#: inks far more than a quarter of its own width: measured 96 of 143 px (67%)
#: on the densest scanline of the real run above, a 2.7x margin.
MIN_RUN_INK_RATIO = 0.25

#: Minimum width, in pixels, of the widest *dense run* (:data:`RUN_GAP_TOLERANCE`,
#: :data:`MIN_RUN_INK_RATIO`) on any one scanline.
#:
#: This is the discriminating half — the signature only the typed text makes.
#: The demo types at least 16 characters, a terminal renders them into that
#: many adjacent monospace cells, and no monospace cell is narrower than
#: ~4 px, so the echoed line must chain >= 64 px on at least one scanline. A
#: blinking cursor, a focus ring, or a one-glyph repaint is *one* cell wide
#: (~8-20 px) and cannot reach it however many times it blinks. Measured dense
#: run for the real run above: 143 px.
#:
#: Note what changed on 2026-07-25 and why: this used to be checked against the
#: *bounding span* of a scanline's changed pixels (last minus first), which is
#: not what the paragraph above derives. Three unrelated one-cell repaints at
#: x=0, x=300 and x=600 have a 608 px bounding span and would have cleared a
#: 64 px "span" threshold while containing no drawn line at all — the exact
#: class of change this constant exists to reject. It is now checked against a
#: run, so those three cells measure 8 px and are rejected.
MIN_HEADLESS_CHANGED_RUN = 64

#: Two consecutive captures differing by fewer than this many pixels count as
#: "the app has settled" (:func:`_settle`). Sized to absorb a blinking cursor
#: (one cell, ~140 px) while staying strictly below
#: :data:`MIN_HEADLESS_CHANGED_PIXELS`.
#:
#: That ordering is necessary but *not* sufficient, and the docstring here
#: used to claim otherwise ("the settle tolerance can never swallow a change
#: the gate is supposed to demand"). It cannot: this tolerance is measured per
#: poll interval, while :data:`MIN_HEADLESS_CHANGED_PIXELS` is measured
#: cumulatively across the whole post-actuation window. An app repainting 150
#: px every 150 ms — a spinner, a clock — looks quiet to every consecutive
#: comparison here and still accumulates well past 400 px later. What actually
#: rules that out is :func:`_idle_probe`, which measures the *cumulative* diff
#: from the control capture over a window at least as long as the one the gate
#: later polls.
SETTLE_MAX_CHANGED_PIXELS = 200

#: Consecutive quiet captures :func:`_settle` needs before it calls the app
#: settled. Three at the 0.15 s poll below is ~0.45 s of continuous quiet.
SETTLE_QUIET_ROUNDS = 3

#: :func:`_settle` never accepts quiescence sooner than this many seconds in,
#: however quiet the app looks. A terminal can map its window, go quiet, and
#: only *then* draw its shell prompt; a run of quiet captures taken before
#: that prompt would produce a control frame the prompt itself later "changes"
#: — a startup repaint masquerading as the agent's typed text.
#:
#: This is now a **backstop, not the guard**. A wall-clock constant cannot
#: know when a given runner's app has painted, and this one did not: on
#: 2026-07-26, the first CI run to ever execute :func:`_idle_probe` failed
#: with 26545 changed pixels across all 480 scanlines and a full-width dense
#: run, with the agent deliberately idle — the app's own startup paint landing
#: after a control capture this floor had already blessed. The actual guard is
#: :func:`frame_has_content`: quiet rounds do not begin accumulating until the
#: app has demonstrably drawn something. The floor stays as a cheap second
#: opinion for a paint that arrives in stages.
SETTLE_FLOOR = 1.0

#: How long :func:`_settle` waits for the app to go quiet before giving up.
#: It gives up by FAILING: an unsettled app means the control capture would
#: be racing the app's own paint, which is exactly the vacuity this gate is
#: being fixed for.
SETTLE_TIMEOUT = 12.0

#: The post-actuation capture window (:func:`_capture_after_change`): one
#: initial pause plus this many polls. Named as constants because
#: :data:`IDLE_PROBE_SECONDS` is derived from them — the idle probe is only
#: meaningful if it is at least as long as the window it is vouching for.
AFTER_SETTLE = 0.4
AFTER_ATTEMPTS = 12
AFTER_POLL = 0.15

#: The longest a post-actuation diff has to accumulate before the gate gives
#: up on it: 0.4 + 12 * 0.15 = 2.2 s.
ACTUATION_POLL_BUDGET = AFTER_SETTLE + AFTER_ATTEMPTS * AFTER_POLL

#: How long :func:`_idle_probe` watches the settled app with nobody actuating.
#: Tied to :data:`ACTUATION_POLL_BUDGET`, not chosen: the probe's claim is
#: "over a window at least as long as the one the gate later polls, this app
#: drew nothing actuation-shaped by itself", and a shorter probe could not
#: make it. ``HeadlessGateThresholdsStayDiscriminating`` pins the ordering.
IDLE_PROBE_SECONDS = ACTUATION_POLL_BUDGET


@dataclass(frozen=True)
class ChangeProfile:
    """How two same-size frames differ, in the shapes the headless gate needs."""

    #: Pixels whose colour channels differ.
    count: int
    #: Width of the widest *dense run* on any single scanline: the widest
    #: stretch whose consecutive changed pixels are never more than
    #: :data:`RUN_GAP_TOLERANCE` apart AND at least :data:`MIN_RUN_INK_RATIO`
    #: of whose pixels changed. This is the gate's shape metric — the one
    #: :func:`actuation_landed` reads.
    widest_dense_run: int
    #: How many pixels inside that dense run actually changed (its ink).
    dense_run_ink: int
    #: Widest first-to-last changed-pixel distance on any single scanline: a
    #: *bounding span*, kept for diagnosis and deliberately NOT the gate's
    #: metric. Three isolated one-cell repaints at x=0, x=300 and x=600 have a
    #: 608 px bounding span and no run wider than one cell — reading the span
    #: as if it were a run is precisely the defect this field's name now makes
    #: impossible to repeat.
    widest_row_span: int
    #: Scanlines carrying at least one changed pixel.
    rows: int

    def __str__(self) -> str:
        return (
            f"{self.count} px changed, widest dense run {self.widest_dense_run} px "
            f"({self.dense_run_ink} px inked), bounding span {self.widest_row_span} px, "
            f"{self.rows} scanline(s) touched"
        )


def change_profile(before: vitrin_os.Frame, after: vitrin_os.Frame) -> ChangeProfile:
    """Compare two same-size frames pixel by pixel, row by row.

    Per scanline this measures three things: how many pixels changed, the
    first-to-last *bounding span* of the changed ones (diagnostic), and the
    widest *dense run* — a stretch whose consecutive changed pixels are never
    more than :data:`RUN_GAP_TOLERANCE` apart and at least
    :data:`MIN_RUN_INK_RATIO` of whose covered pixels changed. Only the dense
    run feeds :func:`actuation_landed`; see :data:`MIN_HEADLESS_CHANGED_RUN`
    for why the span cannot.

    Reads ``frame.raw`` directly (little-endian xrgb8888, ``B, G, R, X`` per
    pixel) and compares only the three colour bytes — the fourth, padding,
    byte carries no content and the C shim composites an opaque background
    whose padding plane is a constant regardless of the client (see
    ``tests/integration/harness.py``'s ``colour_bytes`` for the same
    reasoning). Walks rows via ``frame.stride`` so inter-row padding is never
    mistaken for image content. Raises if the two frames are not the same
    size, which would be a bug in the caller, not a "changed" frame. Inlined
    rather than imported from the harness — this is a *shipped example*,
    launched with only ``PYTHONPATH=sdk/python/src`` (see
    :func:`_capture_when_ready`'s docstring for why the demo stays
    standalone).
    """
    a, b = before.raw, after.raw
    if len(a) != len(b) or (before.width, before.height) != (after.width, after.height):
        raise ValueError(
            f"frame size mismatch: {before.width}x{before.height} ({len(a)} bytes) "
            f"vs {after.width}x{after.height} ({len(b)} bytes)"
        )
    stride, width, height = before.stride, before.width, before.height
    gap = RUN_GAP_TOLERANCE
    count = rows = widest_span = best_run = best_ink = 0

    def _offer(length: int, ink: int) -> None:
        """Keep the widest run that is inked densely enough to be a drawing."""
        nonlocal best_run, best_ink
        if ink >= length * MIN_RUN_INK_RATIO and (length, ink) > (best_run, best_ink):
            best_run, best_ink = length, ink

    for y in range(height):
        base = y * stride
        first = last = -1
        run_start = run_ink = 0
        for x in range(width):
            off = base + x * 4
            if a[off : off + 3] != b[off : off + 3]:
                count += 1
                if first < 0:
                    first, run_start, run_ink = x, x, 0
                elif x - last > gap:
                    # Too far from the previous changed pixel to be the same
                    # drawing: close the run that ended at `last`, open a new
                    # one here.
                    _offer(last - run_start + 1, run_ink)
                    run_start, run_ink = x, 0
                run_ink += 1
                last = x
        if first >= 0:
            rows += 1
            widest_span = max(widest_span, last - first + 1)
            _offer(last - run_start + 1, run_ink)
    return ChangeProfile(
        count=count,
        widest_dense_run=best_run,
        dense_run_ink=best_ink,
        widest_row_span=widest_span,
        rows=rows,
    )


def actuation_landed(profile: ChangeProfile) -> bool:
    """True only for a change the demo's own typed line could have drawn.

    Both conditions, never either alone: enough pixels to rule out a one-cell
    repaint, *and* a dense run wide enough that only a line of characters
    explains it. See the constants above for why each number is what it is —
    in particular :data:`MIN_HEADLESS_CHANGED_RUN` for why this reads a run
    and not the bounding span it used to read.
    """
    return (
        profile.count >= MIN_HEADLESS_CHANGED_PIXELS
        and profile.widest_dense_run >= MIN_HEADLESS_CHANGED_RUN
    )


def frame_has_content(frame: vitrin_os.Frame) -> bool:
    """True once a frame carries real, non-uniform content from the app.

    Content-bearing iff some pixel is not black *and* more than one colour
    value is present. The shim composites an opaque background before the app
    has drawn anything, and a toolkit's pre-chrome fill is likewise a single
    value; both are uniform, and both fail this. So this is the difference
    between "the app has drawn" and "there is a surface to observe", which
    :func:`_capture_when_ready` alone cannot tell apart — it returns the first
    buffer the realm commits, blank or not.

    :func:`_settle` needs exactly that distinction. Its quiet condition is
    "consecutive captures barely differ", which a frame nobody has drawn into
    satisfies *perfectly* — 0 px, every interval, indefinitely. Without a
    positive test for paint, "settled" and "not started" are the same
    observation.

    Reads ``frame.raw`` the way :func:`change_profile` does — little-endian
    xrgb8888, walked by ``frame.stride``, comparing only the three colour
    bytes so inter-row padding and the constant padding byte are never
    mistaken for content. Returns as soon as the answer is known rather than
    scanning the whole buffer. Same rule as
    ``tests/integration/harness.has_real_content``, inlined for the same
    reason :func:`change_profile` is: this is a *shipped example* launched
    with only ``PYTHONPATH=sdk/python/src``, and must not reach into the test
    tree.
    """
    raw = frame.raw
    stride, width, height = frame.stride, frame.width, frame.height
    first_seen: bytes | None = None
    any_non_black = False
    for y in range(height):
        base = y * stride
        for x in range(width):
            off = base + x * 4
            pixel = raw[off : off + 3]
            if pixel != b"\x00\x00\x00":
                any_non_black = True
            if first_seen is None:
                first_seen = pixel
            elif pixel != first_seen and any_non_black:
                return True
    return False


# --- the observe-race-tolerant first capture --------------------------------

def _capture_when_ready(
    grant: vitrin_os.Grant, *, timeout: float = 8.0, poll: float = 0.02
) -> vitrin_os.Frame:
    """First capture of a freshly-served realm, tolerating the startup race.

    A realm that has not yet committed its first buffer has no surface, and the
    core answers ``observe`` with ``NoSurface`` — the honest reply, judged
    before the rate bucket, so a retry costs no budget. ``await_consent()``
    can return before the shim has drawn, so the agent's first ``observe()``
    can lose that race; the poll model (D6) is to retry until a frame lands.

    Inlined rather than imported from ``tests/integration/harness``
    deliberately: this is a *shipped example* under ``examples/``, launched
    with only ``PYTHONPATH=sdk/python/src``. Reaching into the test tree would
    couple the deliverable to the harness (and its ``VITRIN_REPO`` assumptions)
    for a dozen self-contained lines. The logic is identical to
    ``harness.capture_when_ready``; keeping it here keeps the demo standalone.
    """
    deadline = time.monotonic() + timeout
    while True:
        try:
            return grant.observe()
        except errors.NoSurface:
            if time.monotonic() >= deadline:
                raise
            time.sleep(poll)


def _settle(
    grant: vitrin_os.Grant,
    first: vitrin_os.Frame,
    *,
    timeout: float = SETTLE_TIMEOUT,
    poll: float = 0.15,
    quiet_rounds: int = SETTLE_QUIET_ROUNDS,
    floor: float = SETTLE_FLOOR,
) -> vitrin_os.Frame:
    """Capture until the app has stopped repainting on its own; return the last.

    The frame this returns is the **control capture**: everything the app was
    going to draw by itself — its asynchronous first paint, its shell prompt,
    whatever it does on startup — is already inside it. This is the load-
    bearing half of the headless proof, more so than either threshold below
    it: a real weston-terminal's own startup paint has been measured at 569
    changed pixels spanning 81 px across 19 scanlines, which is a *typed-text-
    shaped* diff produced by no actuation at all. No threshold can tell that
    apart from the real thing; only moving it into the "before" frame can.

    "Settled" means the app has **drawn** (:func:`frame_has_content`) and has
    *then* held still for ``quiet_rounds`` consecutive captures each differing
    from the one before it by fewer than :data:`SETTLE_MAX_CHANGED_PIXELS`
    pixels, never sooner than ``floor`` seconds in. The paint precondition is
    load-bearing and was missing until 2026-07-26: quiescence alone cannot
    distinguish "the app finished painting" from "the app has not started",
    because a frame nobody has drawn into differs from its predecessor by 0 px
    every interval, forever. This loop would return that blank frame as the
    control capture, and the app's own first paint — measured on a real
    weston-terminal at 569 px across a dense run of 81, which already
    satisfies :func:`actuation_landed` on its own — would land afterwards,
    inside the very window the gate measures. The tolerance is sized to absorb
    a blinking cursor. It is emphatically *not* sized to make the gate sound: it is a
    per-interval tolerance and the gate's threshold is cumulative (see
    :data:`SETTLE_MAX_CHANGED_PIXELS`), so quiescence by this definition is
    then checked against the gate's own predicate by :func:`_idle_probe`,
    whose frame — not this loop's last capture — is what this returns. Paced
    at ``poll`` so a default-rate grant's token bucket (20/s) is never emptied
    by the settle loop itself.

    Raises :class:`DemoAssertionError` on timeout rather than proceeding with
    an unsettled control frame: a gate that cannot establish its own baseline
    must fail, not guess.
    """
    started = time.monotonic()
    deadline = started + timeout
    previous = first
    quiet = 0
    profile = None
    # Quiescence only means anything *after* the app has drawn. Until then a
    # run of identical captures is the shim's blank background holding still,
    # which is indistinguishable from a settled app by every metric this loop
    # has except this one.
    painted = frame_has_content(first)
    while time.monotonic() < deadline:
        time.sleep(poll)
        frame = grant.observe()
        profile = change_profile(previous, frame)
        previous = frame
        if not painted:
            # Nothing has been drawn yet: reset rather than accumulate quiet,
            # so the paint that eventually lands is followed by a full
            # `quiet_rounds` of genuine post-paint stillness.
            painted = frame_has_content(frame)
            quiet = 0
            continue
        quiet = quiet + 1 if profile.count < SETTLE_MAX_CHANGED_PIXELS else 0
        if quiet >= quiet_rounds and time.monotonic() - started >= floor:
            return _idle_probe(grant, frame)
    if not painted:
        raise DemoAssertionError(
            f"the real app never drew anything within {timeout:.0f}s: every "
            "capture was uniform (the shim's opaque background), so there is "
            "no app paint for this loop to settle. Returning here would make "
            "the control capture a blank frame, and the app's own first paint "
            "would then land inside the gate's measuring window and read as "
            "the agent's click and typed text."
        )
    raise DemoAssertionError(
        f"the real app never went quiet within {timeout:.0f}s (last comparison: "
        f"{profile}; needed {quiet_rounds} consecutive captures under "
        f"{SETTLE_MAX_CHANGED_PIXELS} changed pixels). Without a settled control "
        "capture, a later pixel diff would prove the app finished painting, not "
        "that the agent's click and typed text reached it."
    )


def _idle_probe(
    grant: vitrin_os.Grant,
    control: vitrin_os.Frame,
    *,
    seconds: float = IDLE_PROBE_SECONDS,
    poll: float = 0.15,
) -> vitrin_os.Frame:
    """Watch the settled app do nothing, for as long as the gate later polls.

    :func:`_settle`'s quiet condition is per-interval; the gate's is
    cumulative over the whole post-actuation window. An app repainting just
    under :data:`SETTLE_MAX_CHANGED_PIXELS` every interval — a spinner, a
    clock, a progress bar — therefore settles happily and can still pile up a
    passing diff with nobody actuating anything. This closes that: with the
    agent deliberately idle, it measures the *cumulative* diff from ``control``
    over :data:`IDLE_PROBE_SECONDS` (>= :data:`ACTUATION_POLL_BUDGET`, the
    window :func:`_capture_after_change` is allowed to poll) and fails if that
    diff ever satisfies :func:`actuation_landed`. If the app can draw the
    gate's own signature unaided, the gate cannot discriminate on this runner,
    and saying so is the honest outcome — passing would not be.

    The frame it returns is the new control capture: the last idle observation,
    so whatever harmless drift the probe did see is behind the "before" frame
    rather than inside the actuation diff.

    What this does NOT establish: that the app is quiet at *every* future
    moment. An app quiet during the probe and bursty a second later still
    slips through, and no pre-actuation probe can exclude that. What it does
    establish is that a *steady* incidental repaint — the realistic shape of
    this failure — cannot masquerade as actuation.
    """
    deadline = time.monotonic() + seconds
    frame = control
    worst = ChangeProfile(
        count=0, widest_dense_run=0, dense_run_ink=0, widest_row_span=0, rows=0
    )
    while time.monotonic() < deadline:
        time.sleep(poll)
        frame = grant.observe()
        profile = change_profile(control, frame)
        if actuation_landed(profile):
            raise DemoAssertionError(
                f"the app drew an actuation-shaped change on its own: {profile} "
                f"accumulated over a {seconds:.1f}s idle probe with the agent "
                "deliberately doing nothing (the settled control capture is the "
                f"baseline; >= {MIN_HEADLESS_CHANGED_PIXELS} px AND a dense run >= "
                f"{MIN_HEADLESS_CHANGED_RUN} px is exactly what this gate later "
                "accepts as proof the click and typed text landed). A gate whose "
                "app can forge its own evidence proves nothing, so this fails "
                "rather than passing on an unusable baseline."
            )
        if profile.count > worst.count:
            worst = profile
    print(f"demo: idle probe ({seconds:.1f}s, no actuation) worst drift: {worst}")
    return frame


def _capture_after_change(
    grant: vitrin_os.Grant,
    before: vitrin_os.Frame,
    *,
    headless: bool,
    settle: float = AFTER_SETTLE,
    attempts: int = AFTER_ATTEMPTS,
    poll: float = AFTER_POLL,
) -> vitrin_os.Frame:
    """Capture the "after" frame once the change is observable.

    Headless: poll until the diff from ``before`` — which is the *settled*
    control capture, see :func:`_settle` — carries the signature only the
    demo's own typed line makes (:func:`actuation_landed`), never merely
    "some pixels moved". Nested: poll until the dominant colour reaches
    :data:`SERVED_RGB` (the page loaded). Polling is paced (a settle, then
    ``attempts`` spaced by ``poll``) so a default-rate grant's token bucket is
    never emptied by the capture loop itself. The defaults come from the
    module constants, because :func:`_idle_probe` watches the idle app for at
    least this whole budget (:data:`ACTUATION_POLL_BUDGET`) — shortening the
    window here without shortening the probe is safe, lengthening it is not.
    The last frame is returned regardless, so a genuine failure yields a
    diagnosable after-frame rather than an exception here.
    """
    time.sleep(settle)
    frame = grant.observe()
    for _ in range(attempts):
        if headless:
            if actuation_landed(change_profile(before, frame)):
                return frame
        elif colors_close(dominant_color(frame), SERVED_RGB):
            return frame
        time.sleep(poll)
        frame = grant.observe()
    return frame


# --- result & assertion -----------------------------------------------------

class DemoAssertionError(AssertionError):
    """The venue's "page changed" claim did not hold."""


@dataclass
class DemoResult:
    """What :func:`run` produced — enough for a caller to re-assert or diff."""

    ok: bool
    url: str
    before: vitrin_os.Frame
    after: vitrin_os.Frame
    out_dir: pathlib.Path
    headless: bool


def _assert_page_changed(before: vitrin_os.Frame, after: vitrin_os.Frame, *, headless: bool) -> None:
    if headless:
        profile = change_profile(before, after)
        if not actuation_landed(profile):
            raise DemoAssertionError(
                f"headless: {profile} between the settled control capture and the "
                f"after-frame — need >= {MIN_HEADLESS_CHANGED_PIXELS} px changed AND a "
                f"dense run >= {MIN_HEADLESS_CHANGED_RUN} px wide on some scanline "
                f"(pixels chained at <= {RUN_GAP_TOLERANCE} px, at least "
                f"{MIN_RUN_INK_RATIO:.0%} of the run inked; the bounding span is "
                "reported for diagnosis only and is not the criterion). "
                "The control capture was taken after the app went quiet, so the app's "
                "own startup paint cannot account for a diff; what is missing is the "
                "shape a line of typed characters draws. The agent's click and typed "
                "text did not visibly reach the real app."
            )
        return
    dominant = dominant_color(after)
    if not colors_close(dominant, SERVED_RGB):
        raise DemoAssertionError(
            f"nested: the after-frame's dominant colour {dominant} does not match the "
            f"served page colour {SERVED_RGB} — the typed URL did not navigate the browser"
        )


def _dump_frames(result_dir: pathlib.Path, before: vitrin_os.Frame | None,
                 after: vitrin_os.Frame | None, recorder: str | os.PathLike[str] | None) -> None:
    """Save whatever frames we have as PNGs and point at the flight recorder."""
    result_dir.mkdir(parents=True, exist_ok=True)
    saved = []
    for name, frame in (("before.png", before), ("after.png", after)):
        if frame is not None:
            path = result_dir / name
            frame.to_png(path)
            saved.append(str(path))
    if saved:
        print("demo: saved failure frames:", *saved, sep="\n  ", file=sys.stderr)
    if recorder is not None:
        print(f"demo: flight recorder: {recorder}", file=sys.stderr)
    else:
        print("demo: flight recorder path not provided to the agent", file=sys.stderr)


# --- the agent ---------------------------------------------------------------

def run(
    socket: str | os.PathLike[str],
    *,
    headless: bool,
    consent: str = "auto-approve",
    out_dir: str | os.PathLike[str] | None = None,
    url: str | None = None,
    recorder: str | os.PathLike[str] | None = None,
    connect_timeout: float = 30.0,
) -> DemoResult:
    """Drive the full demo against a live core at ``socket``.

    The single entry point both venues use. ``consent`` is informational here —
    the agent's conduct is identical (``await_consent`` blocks until the
    petition resolves, whether a human clicked Allow or the guarded
    auto-approve did) — but it is threaded through so the transcript names the
    policy the run relied on. Raises :class:`DemoAssertionError` (or the SDK's
    typed exceptions) on failure, after dumping frames for diagnosis; returns a
    :class:`DemoResult` on success.
    """
    out_dir = pathlib.Path(out_dir) if out_dir else pathlib.Path(
        tempfile.mkdtemp(prefix="vitrin-demo-")
    )
    page: _LocalPage | None = None
    conn: vitrin_os.Connection | None = None
    before: vitrin_os.Frame | None = None
    after: vitrin_os.Frame | None = None
    try:
        # Nested serves a real local page for Firefox to load; headless types
        # a harmless shell line into the real terminal app.
        if headless:
            target = url or DEFAULT_HEADLESS_INPUT
        else:
            page = _LocalPage()
            target = url or page.url

        print(f"demo: connecting to {socket} as {DEMO_IDENTITY} "
              f"({'headless' if headless else 'nested'} venue, consent={consent})")
        conn = vitrin_os.connect(
            str(socket), identity=DEMO_IDENTITY, credential=DEMO_TOKEN,
            timeout=connect_timeout,
        )

        # The ONE MVP grant: whole-realm observe + both actuators, for as long
        # as this connection lives. `resource` is left default (whole realm):
        # version 0 serves no finer scope.
        grant = conn.request_grant(
            realm="realm-0",
            verbs=("observe", "actuate.pointer", "actuate.text"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        print("demo: awaiting consent "
              + ("(auto-approve, R6-guarded)" if consent == "auto-approve"
                 else "(a human must click Allow in the nested prompt)"))
        grant.await_consent()
        print("demo: grant resolved; capturing the before-frame")

        before = _capture_when_ready(grant)
        if headless:
            # The control capture: wait out the real app's own startup paint
            # BEFORE the actuation, so the diff that follows cannot be
            # explained by the app finishing what it had already started, and
            # then watch it idle for the gate's own polling budget so a
            # steadily-repainting app cannot forge the gate's signature
            # (`_settle` -> `_idle_probe`; the probe's last frame is the
            # control). Nested needs no equivalent — its proof is the served
            # page's dominant colour, which the browser's chrome cannot draw.
            before = _settle(grant, before)
            print("demo: the real app is quiet; that frame is the control capture")
        url_x, url_y = locate_url_bar(before, headless=headless)
        print(f"demo: clicking ({url_x}, {url_y}); typing {target!r} + Enter")
        grant.pointer.click(url_x, url_y)
        grant.text.type(target + "\n")  # the trailing newline presses Enter

        after = _capture_after_change(grant, before, headless=headless)
        _assert_page_changed(before, after, headless=headless)
        print("demo: page changed — acceptance criterion met")

        return DemoResult(
            ok=True, url=target, before=before, after=after,
            out_dir=out_dir, headless=headless,
        )
    except BaseException:
        _dump_frames(out_dir, before, after, recorder)
        raise
    finally:
        if conn is not None:
            conn.close()
        if page is not None:
            page.close()


# --- CLI ---------------------------------------------------------------------

def _parse_argv(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="run_demo.py",
        description="Vitrin OS Phase-1 demo agent / M1.5 acceptance test.",
    )
    parser.add_argument("--socket", required=True,
                        help="path to the core's Unix socket (core.sock)")
    parser.add_argument("--headless", action="store_true",
                        help="headless venue: a real terminal app stands behind the real shim")
    parser.add_argument("--consent", default="auto-approve",
                        choices=("auto-approve", "interactive"),
                        help="the consent policy the launched core runs (informational)")
    parser.add_argument("--out", default=None,
                        help="directory for failure PNGs (a temp dir by default)")
    parser.add_argument("--url", default=None,
                        help="override the text typed (nested: the URL navigated to, and it "
                             "serves its own by default; headless: the shell line typed)")
    parser.add_argument("--recorder", default=None,
                        help="flight-recorder path, printed on failure for diagnosis")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_argv(sys.argv[1:] if argv is None else argv)
    try:
        run(
            args.socket,
            headless=args.headless,
            consent=args.consent,
            out_dir=args.out,
            url=args.url,
            recorder=args.recorder,
        )
    except DemoAssertionError as exc:
        print(f"demo: FAILED — {exc}", file=sys.stderr)
        return 1
    except Exception as exc:  # noqa: BLE001 — a demo reports, it does not traceback-dump
        print(f"demo: ERROR — {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1
    print("demo: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
