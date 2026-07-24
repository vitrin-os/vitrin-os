#!/usr/bin/env python3
"""Phase 1's integrating demo agent — and the M1.5 acceptance test.

One script, two venues, exactly the same agent code path in both:

    connect (the static demo identity)
      -> request the ONE MVP grant (observe + actuate.pointer + actuate.text
         on realm-0, `while-running`)
      -> await consent (a human clicks Allow in nested; a guarded
         auto-approve resolves it headless)
      -> capture a "before" frame
      -> locate the input target by pixels
      -> click it, type text, press Enter (the trailing "\\n")
      -> capture an "after" frame
      -> assert the page changed.

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
  R6/R1). "The page changed" here means the typed text visibly landed: enough
  *real* pixels changed between the two captures that a stray blinking cursor
  could not explain it (see :data:`MIN_HEADLESS_CHANGED_PIXELS`) — never an
  autonomous mock animation running regardless of actuation. What additionally
  proves the actuation *causally reached the app* is the flight recorder's
  ``use_decision`` entries — an allowed ``move`` at the clicked coordinate and
  an allowed ``type`` whose ``chars`` equals the typed text's length — exactly
  as ``tests/integration/test_actuation.py`` establishes.

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

#: Above this many changed pixels, the headless "page changed" assertion
#: trusts the diff as real actuation-driven content — typed text landing in
#: the terminal renders many glyph pixels — rather than an incidental
#: blinking cursor (a handful of pixels in one character cell). Mirrors
#: ``tests/integration/test_real_actuation.py``'s ``MIN_CHANGED_PIXELS``
#: reasoning (the D7 text gate) at a lower bar sized for this smaller view.
MIN_HEADLESS_CHANGED_PIXELS = 24


def count_changed_pixels(before: vitrin_os.Frame, after: vitrin_os.Frame) -> int:
    """How many pixels' colour channels differ between two same-size frames.

    Reads ``frame.raw`` directly (little-endian xrgb8888, ``B, G, R, X`` per
    pixel) and compares only the three colour bytes — the fourth, padding,
    byte carries no content and the C shim composites an opaque background
    whose padding plane is a constant regardless of the client (see
    ``tests/integration/harness.py``'s ``colour_bytes`` for the same
    reasoning). Raises if the two frames are not the same size, which would
    be a bug in the caller, not a "changed" frame. Inlined rather than
    imported from the harness — this is a *shipped example*, launched with
    only ``PYTHONPATH=sdk/python/src`` (see :func:`_capture_when_ready`'s
    docstring for why the demo stays standalone).
    """
    a, b = before.raw, after.raw
    if len(a) != len(b):
        raise ValueError(f"frame size mismatch: {len(a)} vs {len(b)} bytes")
    changed = 0
    for i in range(0, len(a), 4):
        if a[i : i + 3] != b[i : i + 3]:
            changed += 1
    return changed


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


def _capture_after_change(
    grant: vitrin_os.Grant,
    before: vitrin_os.Frame,
    *,
    headless: bool,
    settle: float = 0.4,
    attempts: int = 20,
    poll: float = 0.15,
) -> vitrin_os.Frame:
    """Capture the "after" frame once the change is observable.

    Headless: poll until enough real pixels have changed from ``before`` to
    rule out a stray blinking cursor (:data:`MIN_HEADLESS_CHANGED_PIXELS`) —
    the typed text actually landing in the real terminal app, never an
    autonomous mock animation. Nested: poll until the dominant colour reaches
    :data:`SERVED_RGB` (the page loaded). Polling is paced (a settle, then
    ``attempts`` spaced by ``poll``) so a default-rate grant's token bucket is
    never emptied by the capture loop itself. The last frame is returned
    regardless, so a genuine failure yields a diagnosable after-frame rather
    than an exception here.
    """
    time.sleep(settle)
    frame = grant.observe()
    for _ in range(attempts):
        if headless:
            if count_changed_pixels(before, frame) >= MIN_HEADLESS_CHANGED_PIXELS:
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
        changed = count_changed_pixels(before, after)
        if changed < MIN_HEADLESS_CHANGED_PIXELS:
            raise DemoAssertionError(
                f"headless: only {changed} pixel(s) changed between the two captures "
                f"(need >= {MIN_HEADLESS_CHANGED_PIXELS}) — the typed text did not visibly "
                "land in the real app; this venue runs a real terminal behind the real "
                "shim, not a mock animation, so a genuine actuation effect is required"
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
