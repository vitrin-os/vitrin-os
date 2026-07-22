#!/usr/bin/env python3
"""Phase 1's integrating demo agent — and the M1.5 acceptance test.

One script, two venues, exactly the same agent code path in both:

    connect (the static demo identity)
      -> request the ONE MVP grant (observe + actuate.pointer + actuate.text
         on realm-0, `while-running`)
      -> await consent (a human clicks Allow in nested; a guarded
         auto-approve resolves it headless)
      -> capture a "before" frame
      -> locate the URL bar by pixels
      -> click it, type a URL, press Enter (the trailing "\\n")
      -> capture an "after" frame
      -> assert the page changed.

The two venues differ only in *what stands in for the app* and in *how "the
page changed" is proven* — never in the agent's protocol conduct:

* **Nested** (`cargo xtask demo`): a real Firefox ESR runs in realm-0. This
  script serves a deterministic solid-colour page from a stdlib
  ``http.server`` bound to ``127.0.0.1:0`` on a daemon thread, types that
  local URL into Firefox's URL bar, and proves the navigation happened by the
  *dominant colour* of the after-frame matching the colour it served (robust
  to anti-aliasing via colour bucketing). A human answers the consent prompt.
  This venue needs a display and a browser; it is correct-by-construction here
  but is exercised on a workstation, never in CI.

* **Headless** (`cargo xtask demo --headless`, and CI): the ``vitrin-mock-shim``
  animated buffer stands in for the app — CI must never depend on Firefox or a
  GPU (plan risk R6/R1). "The page changed" here means the two captures differ
  (the animation advanced across the actuation sequence). What proves the
  actuation *causally reached the app* is not pixels but the flight recorder's
  ``use_decision`` entries — an allowed ``move`` at the clicked coordinate and
  an allowed ``type`` whose ``chars`` equals the URL's length — exactly as
  ``tests/integration/test_actuation.py`` establishes. The mock shim is not
  asked to visibly react to input; nothing in version 1 makes it.

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

#: The URL the headless venue "navigates" to. The mock shim ignores it; only
#: its length reaches the flight recorder (typed text is recorded by shape,
#: never verbatim). A stable literal keeps the recorder assertion deterministic.
DEFAULT_HEADLESS_URL = "http://127.0.0.1/vitrin-demo"


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

#: Headless nominal Y: the mock shim has no URL bar, so any in-bounds point
#: serves — what matters headless is that *some* coordinate reaches the
#: chokepoint and is recorded. Kept near the top for parity with a real
#: toolbar, and clamped into the frame below.
HEADLESS_NOMINAL_Y = 16


def locate_url_bar(frame: vitrin_os.Frame, *, headless: bool) -> tuple[int, int]:
    """Pick the pixel to click. Small and honest by construction.

    Headless: a nominal in-bounds coordinate (the mock shim models no URL bar).
    Nested: the fixed geometry of the pinned ESR when the window is the pinned
    size, with a proportional fallback for any other size.
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

    Headless: poll until the raw bytes differ from ``before`` (the animation
    advanced across the actuation round trips). Nested: poll until the
    dominant colour reaches :data:`SERVED_RGB` (the page loaded). Polling is
    paced (a settle, then ``attempts`` spaced by ``poll``) so a default-rate
    grant's token bucket is never emptied by the capture loop itself. The last
    frame is returned regardless, so a genuine failure yields a diagnosable
    after-frame rather than an exception here.
    """
    time.sleep(settle)
    frame = grant.observe()
    for _ in range(attempts):
        if headless:
            if frame.raw != before.raw:
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
        if before.raw == after.raw:
            raise DemoAssertionError(
                "headless: the two captures are byte-identical — the animation did not "
                "advance across the actuation sequence, so nothing proves the realm is "
                "live between the frames"
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
        # Nested serves a real local page for Firefox to load; headless types a
        # nominal URL the mock shim ignores.
        if headless:
            target = url or DEFAULT_HEADLESS_URL
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
        print(f"demo: clicking the URL bar at ({url_x}, {url_y}); typing {target!r} + Enter")
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
                        help="headless venue: the mock shim stands in for the app")
    parser.add_argument("--consent", default="auto-approve",
                        choices=("auto-approve", "interactive"),
                        help="the consent policy the launched core runs (informational)")
    parser.add_argument("--out", default=None,
                        help="directory for failure PNGs (a temp dir by default)")
    parser.add_argument("--url", default=None,
                        help="override the URL to navigate to (nested serves its own by default)")
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
