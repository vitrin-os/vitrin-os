#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Phase 1's integrating demo agent — and the M1.5 acceptance test.

**The demo is goal-directed.** The agent is handed a *task record it did not
author* — field names and values — and it fills that record into a form in a
real app inside ``realm-0``, submits it, and then proves *from pixels alone*
that the confirmation reflects exactly the values it was told to enter::

    connect (the static demo identity)
      -> request the ONE MVP grant (observe + actuate.pointer + actuate.text
         on realm-0, `while-running`)
      -> await consent (a human clicks Allow in nested; a guarded
         auto-approve resolves it headless)
      -> for each (key, value) in the SUPPLIED task:
             locate the field by its marker colour in the agent's OWN capture
             click its centroid
             baseline the field's ink profile   <- after the click, before the type
             type the value                     (no trailing newline)
             confirm ink landed INSIDE that field's rectangle
      -> locate the submit button by its marker colour; click it
      -> decode the confirmation's three receipt bands and compare them
         against bands computed from the SUPPLIED task, at runtime

Three claims, stated the only way they are true:

* **There is no language model here.** This agent is deterministic. "Locate"
  means: scan the agent's own captured frame for a known marker colour and
  click that region's centre. Nothing reasons, plans or interprets — and no
  sentence in this file, in ``README.md`` or in the gate may imply otherwise.
* **The receipt is a CHECKSUM, not glyph recognition.** The agent never reads
  back the characters it typed. It reads back a **36-bit function of the
  record the app received** and checks it equals the same function of the task
  it was given. "The agent read back what it typed" would be false.
* **The task is an input, not a constant.** ``--task K=V`` is repeatable and
  order-preserving; the expected bands are computed from whatever was supplied.
  That is what makes the assertion non-vacuous — it cannot be a hardcoded
  constant that would pass regardless of what landed.

The receipt encoding is **normative in ``examples/agent-demo/README.md``**.
The Python below is its reference implementation; ``form.html`` (JS) and
``shim/tests/form_target.c`` (C) restate it and are pinned against this one by
``tests/integration/test_demo.py``.

Both venues run the SAME real chain: the shipped ``vitrind`` execs the real
per-app Wayland shim (``vitrin-shim``), which fork/execs a real app inside its
own private, confined Wayland socket — ``vitrin-mock-shim`` is a unit-test
fixture and stands in for nothing here. They differ only in *which real app*
stands behind the shim, plus one nested-only preamble:

* **Headless** (``cargo xtask demo --headless``, and CI): ``form-target``
  (``shim/tests/form_target.c``), a bare wl_shm + xdg-shell + wl_pointer +
  wl_keyboard client co-built with the shim. GPU-free, so CI never depends on
  Firefox or a GPU (plan risk R6/R1). **Disclosure: this app is
  repo-authored.** It is a real Wayland client and is neither
  ``vitrin-mock-shim`` nor ``shim/tests/mock_core.c``, so D12 holds literally
  — but "the app is written by the same repo that asserts on it" is a fair
  criticism and is answered, not dodged, in ``README.md`` and in
  ``docs/plan/01-phase-1-mvp.md``'s D12 seam table: the ``click-target``
  precedent in the M1.4 gate, and the third-party rungs
  (``test_real_app.py`` / ``test_real_gtk.py`` / ``test_real_firefox.py``)
  staying green.
* **Nested** (``cargo xtask demo``): a real Firefox ESR runs in realm-0 on the
  host's real display. This script serves ``form.html`` from a stdlib
  ``http.server`` bound to ``127.0.0.1:0`` on a daemon thread, types that
  local URL into Firefox's URL bar (a pinned geometry constant — version 1 has
  no semantic tree — overridable with ``VITRIN_DEMO_URL_BAR``), waits for the
  page's first field marker to appear, and then runs the *identical* field
  loop and receipt decode. A human answers the consent prompt.

Each venue also produces an **out-of-band, byte-exact ground truth** beside the
pixels: ``form-target`` prints ``SUBMIT ... canon=<hex>`` to stdout, and the
nested page fires a ``GET /submitted?...`` beacon this script records in
:attr:`_LocalPage.submitted`. Neither is pixels; both are what the app says it
received.

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
import sys
import tempfile
import threading
import time
import urllib.parse
from dataclasses import dataclass, field as dc_field

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


# --- the task ---------------------------------------------------------------

#: How many fields both venues' forms have. ``form-target``'s layout and
#: ``form.html`` both draw exactly this many, so a task with any other number
#: of pairs is rejected at parse time rather than half-filled.
FIELD_COUNT = 2

#: The shipped default task, used when no ``--task K=V`` is supplied. Order is
#: part of the record (:func:`canonical_task`), so this is a tuple of pairs
#: and never a dict.
#:
#: ``crates/xtask``'s ``DEFAULT_TASK`` must name the same keys, because the
#: launcher passes them to the app as ``--field NAME``;
#: ``tests/integration/test_demo.py`` pins the two together.
TASK_DEFAULT: tuple[tuple[str, str], ...] = (
    ("name", "Ada Lovelace"),
    ("email", "ada@example.org"),
)

#: The IDL's cap on one ``vitrin_actuator_text.type`` payload
#: (``protocol/vitrin-v0.xml``): 4096 bytes of UTF-8.
MAX_TEXT_BYTES = 4096


class TaskError(ValueError):
    """A supplied ``--task K=V`` cannot be delivered as typed text."""


def _reject_control_chars(what: str, value: str) -> None:
    """Refuse the whole Unicode Cc category, plus ``\\n`` and ``\\t``.

    The IDL makes every C0 (U+0000-U+001F), DEL (U+007F) and C1
    (U+0080-U+009F) character except newline and tab a **fatal**
    ``invalid_argument`` on ``vitrin_actuator_text.type`` — "a correct client
    never emits them" — so a task carrying one would kill the connection
    rather than fail an assertion. Newline and tab are legal on the wire (they
    are delivered as Return and Tab) and are refused here for a different
    reason: they are *actuations*, not characters a text field holds, so a
    record containing one could never round-trip through the form and the
    receipt would correctly refuse to match.
    """
    for index, ch in enumerate(value):
        cp = ord(ch)
        if cp < 0x20 or cp == 0x7F or 0x80 <= cp <= 0x9F:
            raise TaskError(
                f"{what} contains U+{cp:04X} at position {index}: control characters "
                "cannot be delivered as typed text (the IDL makes all of Cc except "
                "newline/tab a fatal invalid_argument, and newline/tab are Return/Tab "
                "actuations rather than field content)"
            )


def parse_task(specs: list[str] | None) -> tuple[tuple[str, str], ...]:
    """Parse repeated ``K=V`` strings into an ORDER-PRESERVING tuple of pairs.

    Order matters: the canonical string — and therefore every band colour —
    depends on it, so the same pairs in a different order are a *different*
    record. ``None`` or an empty list yields :data:`TASK_DEFAULT`.
    """
    if not specs:
        return TASK_DEFAULT
    pairs: list[tuple[str, str]] = []
    for spec in specs:
        key, sep, value = spec.partition("=")
        if not sep or not key:
            raise TaskError(f"--task {spec!r} is not of the form K=V")
        # The key is never typed — it reaches the app as an argv `--field NAME`
        # and the page as a `?k=` query parameter — so validating it is hygiene
        # rather than protocol conformance. Said plainly rather than implied.
        _reject_control_chars(f"the key of --task {spec!r}", key)
        _reject_control_chars(f"the value of --task {spec!r}", value)
        encoded = len(value.encode("utf-8"))
        if encoded > MAX_TEXT_BYTES:
            raise TaskError(
                f"the value of --task {key}=... is {encoded} UTF-8 bytes; the IDL caps "
                f"one vitrin_actuator_text.type payload at {MAX_TEXT_BYTES}"
            )
        pairs.append((key, value))
    if len(pairs) != FIELD_COUNT:
        raise TaskError(
            f"the demo's form has exactly {FIELD_COUNT} fields in both venues "
            f"(shim/tests/form_target.c's layout and examples/agent-demo/form.html), "
            f"so exactly {FIELD_COUNT} --task K=V pairs are needed; got {len(pairs)}"
        )
    return tuple(pairs)


def canonical_task(task: tuple[tuple[str, str], ...]) -> str:
    """The normative canonical string: ``"k0=v0\\nk1=v1"``, no trailing newline."""
    return "\n".join(f"{key}={value}" for key, value in task)


# --- the receipt encoding (normative: examples/agent-demo/README.md) --------

#: FNV-1a, 32-bit. Six lines, no library, no ambiguity — chosen for exactly
#: that reason: the same six lines exist in ``form.html`` (JS) and
#: ``shim/tests/form_target.c`` (C), and ``tests/integration/test_demo.py``
#: pins both against this one. Nothing about it is a security property; it is
#: a checksum over pixels an observe grant may capture anyway.
_FNV32_OFFSET = 0x811C9DC5
_FNV32_PRIME = 0x01000193
_U32 = 0xFFFFFFFF

#: How many receipt bands the confirmation view paints. Three bands are
#: 3 x 12 = 36 bits, so a *wrong* record whose bands all matched would be a
#: ~1.5e-11 coincidence. That is the whole strength of the pixel claim.
BAND_COUNT = 3

#: The row the bands start at in the headless venue's pinned 640x480 view
#: (``form_target.c``'s ``BAND_TOP``), documented here for readers. The decoder
#: below deliberately does NOT use it: it finds the bands by *colour*, so the
#: same code works against Firefox's very different geometry.
BAND_TOP = 96


def fnv1a32(data: bytes) -> int:
    """FNV-1a-32 over ``data``. The reference for the JS and C restatements."""
    hashed = _FNV32_OFFSET
    for byte in data:
        hashed = ((hashed ^ byte) * _FNV32_PRIME) & _U32
    return hashed


def receipt_bands(task: tuple[tuple[str, str], ...]) -> tuple[tuple[int, int, int], ...]:
    """The :data:`BAND_COUNT` band colours this task's record must paint.

    Band ``i``'s colour is ``fnv1a32(canon + "#" + str(i))``, taking three
    nibbles as the channels and scaling each by ``0x11``. Every channel is a
    multiple of ``0x11`` because that is this repo's established convention
    for a colour that survives the capture path **and** a 4-bit-per-channel
    histogram exactly, with no tolerance (``tests/integration/harness.py``'s
    ``dominant_colour``/``locate_colour``; ``shim/tests/click_target.c``'s
    three colours). So the band check below is an equality, never a distance.
    """
    canon = canonical_task(task).encode("utf-8")
    bands = []
    for index in range(BAND_COUNT):
        hashed = fnv1a32(canon + b"#" + str(index).encode("ascii"))
        bands.append(
            (
                ((hashed >> 8) & 0xF) * 0x11,
                ((hashed >> 4) & 0xF) * 0x11,
                (hashed & 0xF) * 0x11,
            )
        )
    return tuple(bands)


def rgb_hex(rgb: tuple[int, int, int]) -> str:
    """``(r, g, b)`` as ``"rrggbb"`` — the form the C app prints and the gate reads."""
    return "%02x%02x%02x" % rgb


# --- the form's marker colours and geometry ---------------------------------
#
# The SAME colours and the SAME reading order in both venues
# (`shim/tests/form_target.c`, `examples/agent-demo/form.html`), which is what
# makes the locator code below literally identical against a bare wl_shm
# client and against real Firefox.

#: One marker colour per field, in reading order. Channels are multiples of
#: 0x11, so the 4-bit histogram reads them back exactly.
FIELD_MARKERS = ("00ff00", "00ffff")

#: The submit button's marker colour.
SUBMIT_MARKER = "ffff00"

#: How many pixels of a marker colour must be on screen before the agent
#: believes it located that feature. ``form-target``'s fields are 560x44 =
#: 24 640 px and its button 560x56 = 31 360 px, and ``form.html``'s are larger
#: still, so 8000 clears with >= 3x margin while rejecting a stray pixel or an
#: anti-aliased edge that happens to quantise to the marker colour.
MIN_MARKER_PIXELS = 8000

#: Pixels trimmed off every side of a located field before measuring the ink
#: typed into it.
#:
#: This is one of the two mitigations for the focus-ring trap (see
#: :func:`_baseline_after_click`). A real app draws a focus indicator when a
#: field is clicked, and that indicator is a change *inside* the field's
#: bounding box that no typing produced. ``form-target`` draws its ring 2 px
#: inside the field rectangle *deliberately*, and ``form.html`` uses a 3 px
#: inset box-shadow, so 4 excludes both geometrically.
FIELD_RECT_INSET = 4

#: Minimum changed pixels inside a field's (inset) rectangle for "the value I
#: typed landed in the field I clicked".
#:
#: Derivation, at the pinned 640x480 headless view: ``form-target`` rasterises
#: no font — it draws one filled 4x12 ink cell per received UTF-8 byte — so a
#: value of N bytes inks exactly 48N px, and 120 is cleared by any value of 3
#: bytes or more. The shipped default task's values are 12 and 15 bytes
#: (576 px and 720 px), a >= 4.8x margin. What it rejects: a blinking text
#: caret (~2x24 = 48 px in either venue) and a focus ring (~2400 px for a
#: 560x44 field, which the inset above additionally removes from the
#: measurement entirely). Nested Firefox renders the same values as real
#: glyphs in a 24 px font, far above this.
#:
#: The honest limitation: a task whose value inks fewer than 120 px — under 3
#: bytes headless — would fail this *localisation* check even though the value
#: did land. The receipt bands and the app's own byte-exact report are what
#: prove the content; this check only proves *where* the ink went.
MIN_FIELD_INK_PIXELS = 120

#: Two consecutive captures whose in-rectangle diff is at or under this count
#: as "the click's own repaint is finished" (:func:`_baseline_after_click`).
#: Sized to absorb a blinking caret (~48 px) while staying an order of
#: magnitude below a focus ring (~2400 px), which is the thing this wait
#: exists to get *into* the baseline.
FIELD_QUIET_MAX = 64

#: A scanline counts as part of a band only if at least this fraction of it is
#: exactly the band's colour. Not 1.0: a real toolkit can leave a sub-pixel
#: seam or a scrollbar column at an edge, and the claim is "a full-width band
#: of this colour", not "every last pixel".
SOLID_ROW_SHARE = 0.90

#: How many consecutive solid rows a band must span to count.
#:
#: Derivation: at the pinned 640x480 view the bands fill everything below
#: :data:`BAND_TOP`, i.e. ``(480 - 96) / 3 = 128`` rows each — so 24 is a
#: 5.3x margin. Nested, at 1280x800 minus Firefox's chrome, they are ~230 rows
#: each. And 24 consecutive full-width rows of one *specific* colour is far
#: more than any incidental strip a toolkit draws.
MIN_BAND_ROWS = 24


# --- nested-only: the URL bar -----------------------------------------------

#: The window size Firefox ESR is pinned to in nested mode — the nested
#: backend's initial window (``vitrind``'s ``DEFAULT_HEADLESS_SIZE``, which the
#: nested window matches). Used to pick the fixed URL-bar coordinate below.
ESR_WINDOW_SIZE = (1280, 800)

#: Fixed URL-bar target for the pinned ESR at :data:`ESR_WINDOW_SIZE`: the
#: horizontal centre, in the toolbar band a few dozen pixels below the top.
#: Deliberately a documented constant, not a vision model — version 1 has no
#: semantic tree, so a pinned browser is located by geometry. A different
#: Firefox build lays its toolbar out elsewhere; ``VITRIN_DEMO_URL_BAR=x,y``
#: overrides it.
ESR_URL_BAR = (640, 72)

#: Vertical band the URL bar sits in, used by the proportional fallback when
#: the window is some size other than :data:`ESR_WINDOW_SIZE`.
ESR_TOOLBAR_Y = 72

#: Environment override for :func:`url_bar_target`, as ``"x,y"``.
URL_BAR_ENV = "VITRIN_DEMO_URL_BAR"


def url_bar_target(frame: vitrin_os.Frame) -> tuple[int, int]:
    """The pixel to click to focus Firefox's URL bar (nested venue only).

    :data:`URL_BAR_ENV` wins when set; otherwise the pinned geometry for the
    pinned window size, with a proportional fallback for any other size.
    """
    override = os.environ.get(URL_BAR_ENV)
    if override:
        parts = override.split(",")
        if len(parts) != 2:
            raise TaskError(f"{URL_BAR_ENV}={override!r} is not 'x,y'")
        try:
            x, y = int(parts[0]), int(parts[1])
        except ValueError as exc:
            raise TaskError(f"{URL_BAR_ENV}={override!r} is not 'x,y'") from exc
        if not (0 <= x < frame.width and 0 <= y < frame.height):
            raise TaskError(
                f"{URL_BAR_ENV}={override!r} is outside the "
                f"{frame.width}x{frame.height} realm view"
            )
        return x, y
    if (frame.width, frame.height) == ESR_WINDOW_SIZE:
        return ESR_URL_BAR
    return (frame.width // 2, min(ESR_TOOLBAR_Y, frame.height - 1))


# --- the served page (nested venue) -----------------------------------------

#: The page is a **data file**, not a Python string literal: 60 lines of HTML
#: plus JS is where the old ``_SOLID_PAGE`` idiom stops paying. It reads its
#: field names from its own query string (``?k=name&k=email``), so it stays a
#: pure data file rather than a template the server rewrites.
FORM_PATH = pathlib.Path(__file__).resolve().parent / "form.html"


class _FormHandler(http.server.BaseHTTPRequestHandler):
    """Two routes, and 404 for everything else.

    The 404 branch is load-bearing rather than tidy: Firefox *will* request
    ``/favicon.ico``, and a handler that served the form for every path would
    answer that request with the form.
    """

    def do_GET(self) -> None:  # noqa: N802 (http.server's fixed spelling)
        path, _, query = self.path.partition("?")
        if path == "/":
            body = self.server.form_bytes  # type: ignore[attr-defined]
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if path == "/submitted":
            # `parse_qsl`, the ORDERED form of `parse_qs`: the record's order
            # is part of the record (`canonical_task`), and `parse_qs`'s dict
            # would lose it the moment a key repeated.
            pairs = urllib.parse.parse_qsl(query, keep_blank_values=True)
            self.server.record_submission(pairs)  # type: ignore[attr-defined]
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_error(404)

    def log_message(self, *_args: object) -> None:
        # A demo's stdout is its transcript; the http server's per-request
        # noise does not belong in it.
        pass


class _PageServer(http.server.ThreadingHTTPServer):
    """The form bytes and the recorded submissions, guarded by a lock."""

    daemon_threads = True

    def __init__(self, address, handler, form_bytes: bytes) -> None:
        super().__init__(address, handler)
        self.form_bytes = form_bytes
        self._lock = threading.Lock()
        self._submitted: list[tuple[str, str]] = []

    def record_submission(self, pairs: list[tuple[str, str]]) -> None:
        with self._lock:
            self._submitted.extend(pairs)

    def submissions(self) -> list[tuple[str, str]]:
        with self._lock:
            return list(self._submitted)


class _LocalPage:
    """A stdlib ``http.server`` on ``127.0.0.1:0``, served from a daemon thread.

    Bound to an ephemeral port and to loopback only: nothing this demo serves
    should be reachable off the machine. The thread is a daemon so a crashed
    demo never wedges the process on a live server, and :meth:`close` is
    idempotent so every teardown path can call it.

    :attr:`submitted` is the nested venue's **byte-exact ground truth** — the
    ordered ``(key, value)`` pairs the page's own beacon reported, the
    analogue of ``gtk-entry-probe``'s ``ENTRY_HEX`` in
    ``test_real_actuation.py`` and of ``form-target``'s ``SUBMIT`` line.
    """

    def __init__(self) -> None:
        self._server = _PageServer(
            ("127.0.0.1", 0), _FormHandler, FORM_PATH.read_bytes()
        )
        self._thread = threading.Thread(
            target=self._server.serve_forever, name="vitrin-demo-httpd", daemon=True
        )
        self._thread.start()
        self._closed = False

    @property
    def url(self) -> str:
        host, port = self._server.server_address[:2]
        return f"http://{host}:{port}/"

    def url_for(self, task: tuple[tuple[str, str], ...]) -> str:
        """The page URL carrying this task's field NAMES (never its values)."""
        query = urllib.parse.urlencode([("k", key) for key, _ in task])
        return f"{self.url}?{query}"

    @property
    def submitted(self) -> list[tuple[str, str]]:
        return self._server.submissions()

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=2.0)


# --- pixel analysis ---------------------------------------------------------
#
# Inlined rather than imported from `tests/integration/harness.py`
# deliberately: this is a *shipped example* under `examples/`, launched with
# only `PYTHONPATH=sdk/python/src`. Reaching into the test tree would couple
# the deliverable to the harness (and its `VITRIN_REPO` assumptions). The
# quantisation is the same 4-bit-per-channel histogram the harness's
# `dominant_colour`/`locate_colour` apply, which is why every colour in this
# demo has channels that are multiples of 0x11.

#: Keep only a byte's top nibble — the 4-bit-per-channel quantisation.
_TOP_NIBBLE = bytes(i & 0xF0 for i in range(256))

#: ``_eq_table(t)[b]`` is 0xFF iff ``b == t``. Cached because a translate table
#: is 256 bytes and the same handful of colours are asked for repeatedly.
_EQ_TABLES: dict[int, bytes] = {}


def _eq_table(target: int) -> bytes:
    table = _EQ_TABLES.get(target)
    if table is None:
        table = bytes(0xFF if i == target else 0x00 for i in range(256))
        _EQ_TABLES[target] = table
    return table


def _packed(frame: vitrin_os.Frame) -> bytes:
    """``frame.raw`` as tightly-packed 4-byte ``B, G, R, X`` pixels.

    Stride-generic per the IDL (row ``r`` begins at ``r * stride`` and carries
    ``width * 4`` payload bytes); version 1 pins ``stride == width * 4`` on the
    wire, so the fast path is the normal one.
    """
    raw = frame.raw
    row_len = frame.width * 4
    if frame.stride == row_len:
        return raw
    return b"".join(
        raw[r * frame.stride : r * frame.stride + row_len] for r in range(frame.height)
    )


def _match_rows(frame: vitrin_os.Frame, rgb: tuple[int, int, int]) -> list[bytes]:
    """One row of ``0xFF``/``0x00`` bytes per scanline: 0xFF where the pixel's
    quantised colour is exactly ``rgb``.

    The three colour planes are each translated to a match mask and AND-ed
    together as one big integer, so the whole frame's per-pixel conjunction is
    done at C speed rather than in a Python loop — which matters because this
    runs on every polled capture. The padding (``X``) plane is never read: the
    C shim composites an opaque background whose padding byte is a constant
    regardless of the client (``harness.colour_bytes`` makes the same point).
    """
    packed = _packed(frame)
    width, height = frame.width, frame.height
    masks = []
    for plane_offset, channel in ((2, rgb[0]), (1, rgb[1]), (0, rgb[2])):
        plane = packed[plane_offset::4].translate(_TOP_NIBBLE)
        masks.append(plane.translate(_eq_table(channel & 0xF0)))
    combined = (
        int.from_bytes(masks[0], "big")
        & int.from_bytes(masks[1], "big")
        & int.from_bytes(masks[2], "big")
    ).to_bytes(len(masks[0]), "big")
    return [combined[y * width : (y + 1) * width] for y in range(height)]


@dataclass(frozen=True)
class Rect:
    """A half-open pixel rectangle, ``[x0, x1) x [y0, y1)``."""

    x0: int
    y0: int
    x1: int
    y1: int

    @property
    def width(self) -> int:
        return self.x1 - self.x0

    @property
    def height(self) -> int:
        return self.y1 - self.y0

    @property
    def centre(self) -> tuple[int, int]:
        return ((self.x0 + self.x1) // 2, (self.y0 + self.y1) // 2)

    def inset(self, n: int) -> "Rect":
        """This rectangle shrunk by ``n`` on every side (never past empty)."""
        x0, y0 = self.x0 + n, self.y0 + n
        x1, y1 = max(self.x1 - n, x0), max(self.y1 - n, y0)
        return Rect(x0, y0, x1, y1)

    def __str__(self) -> str:
        return f"({self.x0},{self.y0})-({self.x1},{self.y1})"


@dataclass(frozen=True)
class Marker:
    """A located marker region: where it is, how big it is, where to click."""

    hex6: str
    rect: Rect
    count: int

    @property
    def click_point(self) -> tuple[int, int]:
        return self.rect.centre

    def __str__(self) -> str:
        return f"#{self.hex6} {self.rect} ({self.count} px)"


def locate_marker(frame: vitrin_os.Frame, hex6: str) -> Marker | None:
    """Find a marker colour's bounding rectangle in the agent's OWN capture.

    This is the whole "locate" story, and it is small on purpose: quantise,
    match a known colour exactly, take the bounding box. The click point is
    that box's centre — the marker regions are rectangles in both venues, so
    the centre is the centroid, and taking the centre additionally keeps the
    click well inside the region for any convex marker. Returns ``None`` when
    the colour is absent.

    Same technique the M1.4 gate already uses against ``click-target`` via
    ``harness.locate_colour`` — see this section's header for why it is
    restated here rather than imported.
    """
    rgb = (int(hex6[0:2], 16), int(hex6[2:4], 16), int(hex6[4:6], 16))
    rows = _match_rows(frame, rgb)
    count = 0
    x0 = y0 = x1 = y1 = None
    for y, row in enumerate(rows):
        hits = row.count(0xFF)
        if hits == 0:
            continue
        count += hits
        first, last = row.find(b"\xff"), row.rfind(b"\xff")
        x0 = first if x0 is None else min(x0, first)
        x1 = last + 1 if x1 is None else max(x1, last + 1)
        if y0 is None:
            y0 = y
        y1 = y + 1
    if count == 0:
        return None
    assert x0 is not None and y0 is not None and x1 is not None and y1 is not None
    return Marker(hex6=hex6, rect=Rect(x0, y0, x1, y1), count=count)


def changed_in_rect(
    before: vitrin_os.Frame, after: vitrin_os.Frame, rect: Rect
) -> int:
    """How many pixels inside ``rect`` differ in colour between two frames.

    Compares only the three colour bytes; the fourth, padding, byte carries no
    content. Raises if the frames are not the same size, which would be a bug
    in the caller rather than a "changed" frame.
    """
    if (before.width, before.height) != (after.width, after.height):
        raise ValueError(
            f"frame size mismatch: {before.width}x{before.height} vs "
            f"{after.width}x{after.height}"
        )
    pa, pb = _packed(before), _packed(after)
    width = before.width
    x0 = max(rect.x0, 0)
    x1 = min(rect.x1, width)
    y0 = max(rect.y0, 0)
    y1 = min(rect.y1, before.height)
    changed = 0
    span = (x1 - x0) * 4
    for y in range(y0, y1):
        base = (y * width + x0) * 4
        row_a = pa[base : base + span]
        row_b = pb[base : base + span]
        if row_a == row_b:
            continue
        for i in range(0, span, 4):
            if row_a[i : i + 3] != row_b[i : i + 3]:
                changed += 1
    return changed


@dataclass(frozen=True)
class SolidRun:
    """A maximal run of consecutive scanlines that are one solid colour."""

    rgb: tuple[int, int, int]
    first_row: int
    rows: int

    def __str__(self) -> str:
        return f"#{rgb_hex(self.rgb)} rows {self.first_row}..{self.first_row + self.rows - 1}"


def solid_row_runs(
    frame: vitrin_os.Frame, colours: tuple[tuple[int, int, int], ...]
) -> list[SolidRun]:
    """Every maximal run of scanlines that are solidly one of ``colours``.

    Restricted to the colours the caller is looking for, which is what makes
    this cheap *and* what makes the check a **positive content check**: the
    question is never "did the frame change" but "does the frame carry these
    specific colours as full-width bands". Geometry-free on purpose — the same
    code has to work against ``form-target``'s pinned 640x480 layout and
    against Firefox's chrome-offset viewport.
    """
    width, height = frame.width, frame.height
    need = int(width * SOLID_ROW_SHARE)
    per_row: list[tuple[int, int, int] | None] = [None] * height
    seen: set[tuple[int, int, int]] = set()
    for rgb in colours:
        if rgb in seen:
            continue
        seen.add(rgb)
        for y, row in enumerate(_match_rows(frame, rgb)):
            if per_row[y] is None and row.count(0xFF) >= need:
                per_row[y] = rgb
    runs: list[list] = []
    for y, rgb in enumerate(per_row):
        if rgb is None:
            continue
        if runs and runs[-1][0] == rgb and runs[-1][1] + runs[-1][2] == y:
            runs[-1][2] += 1
        else:
            runs.append([rgb, y, 1])
    return [SolidRun(rgb=r[0], first_row=r[1], rows=r[2]) for r in runs]


def match_bands(
    runs: list[SolidRun],
    expected: tuple[tuple[int, int, int], ...],
    *,
    min_rows: int = MIN_BAND_ROWS,
) -> bool:
    """True iff ``expected`` appears, in order, as bands of ``min_rows`` rows.

    In-order and greedy. One run may satisfy several *consecutive* expected
    bands, which is exactly what two adjacent bands of the same colour look
    like on screen — a single taller run — and which a task whose hash
    collides between adjacent bands would otherwise fail on for no good
    reason.
    """
    index, rows_left = -1, 0
    for want in expected:
        while True:
            if index >= 0 and rows_left >= min_rows and runs[index].rgb == want:
                rows_left -= min_rows
                break
            index += 1
            if index >= len(runs):
                return False
            rows_left = runs[index].rows
    return True


# --- capture helpers --------------------------------------------------------

class DemoAssertionError(AssertionError):
    """The demo's proof did not hold."""


def _observe(
    grant: vitrin_os.Grant, *, timeout: float = 8.0, poll: float = 0.02
) -> vitrin_os.Frame:
    """One capture, tolerating the two honest refusals a poll model produces.

    ``NoSurface``: a realm that has not committed its first buffer has no
    surface, and the core says so — judged before the rate bucket, so a retry
    costs no budget. ``await_consent()`` can return before the shim has drawn,
    so the agent's first ``observe()`` can lose that race; the poll model (D6)
    is to retry until a frame lands. ``RateLimited``: honour the core's own
    ``retry_after_ms`` rather than guessing.
    """
    deadline = time.monotonic() + timeout
    while True:
        try:
            return grant.observe()
        except errors.NoSurface:
            if time.monotonic() >= deadline:
                raise
            time.sleep(poll)
        except errors.RateLimited as limited:
            if time.monotonic() >= deadline:
                raise
            time.sleep(max(limited.retry_after_ms / 1000.0, poll))


#: Per-step poll budgets. Deliberately tight: the integration harness kills a
#: test at 90 s (``harness.TEST_TIMEOUT_S``), and this flow has six polled
#: steps plus a page load, so a generous timeout in each would blow that
#: budget before any of them failed.
LOCATE_TIMEOUT = 15.0
LOCATE_POLL = 0.1
FOCUS_SETTLE_TIMEOUT = 2.0
FOCUS_SETTLE_POLL = 0.06
FOCUS_QUIET_ROUNDS = 2
INK_TIMEOUT = 8.0
INK_POLL = 0.1
RECEIPT_TIMEOUT = 12.0
RECEIPT_POLL = 0.15
PAGE_LOAD_TIMEOUT = 30.0


def _locate_with_poll(
    grant: vitrin_os.Grant,
    hex6: str,
    *,
    what: str,
    timeout: float = LOCATE_TIMEOUT,
    poll: float = LOCATE_POLL,
    hint: str = "",
) -> tuple[vitrin_os.Frame, Marker]:
    """Observe until ``hex6`` is on screen with enough pixels to be the feature."""
    deadline = time.monotonic() + timeout
    frame = _observe(grant)
    best = 0
    while True:
        marker = locate_marker(frame, hex6)
        if marker is not None:
            best = max(best, marker.count)
            if marker.count >= MIN_MARKER_PIXELS:
                return frame, marker
        if time.monotonic() >= deadline:
            raise DemoAssertionError(
                f"{what} (#{hex6}) never reached the agent within {timeout:.0f}s: the "
                f"largest matching region seen was {best} px, and "
                f"{MIN_MARKER_PIXELS} px are needed to believe it is the feature."
                + (f" {hint}" if hint else "")
            )
        time.sleep(poll)
        frame = _observe(grant)


def _baseline_after_click(
    grant: vitrin_os.Grant,
    rect: Rect,
    *,
    timeout: float = FOCUS_SETTLE_TIMEOUT,
    poll: float = FOCUS_SETTLE_POLL,
    rounds: int = FOCUS_QUIET_ROUNDS,
) -> vitrin_os.Frame:
    """The per-field ink baseline, taken AFTER the click and BEFORE the type.

    **This ordering is the point, and it is the class of defect this repo has
    already been burned by twice.** A real app draws a focus indicator when a
    field is clicked. That indicator is a change *inside* the field's bounding
    box which no typing produced, and it is often *larger* than the typed text
    (a 2 px ring around a 560x44 field is ~2400 px; the shipped task's value
    inks ~576 px). A baseline taken before the click would put the ring inside
    the measured diff, and a naive "did anything change in the field?" check
    would then pass with nothing typed at all.

    So the baseline is captured after the click, and this loop waits for the
    in-rectangle diff between consecutive captures to go quiet first — the
    indicator has to have *arrived* before the baseline is taken, or the
    ordering buys nothing. On timeout it returns its last capture rather than
    failing: :data:`FIELD_RECT_INSET` excludes the ring geometrically as the
    second mitigation, and a text caret that never stops blinking must not
    fail the run.
    """
    frame = _observe(grant)
    quiet = 0
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        time.sleep(poll)
        nxt = _observe(grant)
        quiet = quiet + 1 if changed_in_rect(frame, nxt, rect) <= FIELD_QUIET_MAX else 0
        frame = nxt
        if quiet >= rounds:
            return frame
    return frame


def _await_ink(
    grant: vitrin_os.Grant,
    baseline: vitrin_os.Frame,
    rect: Rect,
    *,
    what: str,
    timeout: float = INK_TIMEOUT,
    poll: float = INK_POLL,
) -> tuple[vitrin_os.Frame, int]:
    """Observe until enough pixels changed INSIDE ``rect`` to be the typed value."""
    deadline = time.monotonic() + timeout
    best = 0
    while True:
        frame = _observe(grant)
        ink = changed_in_rect(baseline, frame, rect)
        best = max(best, ink)
        if ink >= MIN_FIELD_INK_PIXELS:
            return frame, ink
        if time.monotonic() >= deadline:
            raise DemoAssertionError(
                f"nothing the agent typed reached {what}: only {best} px changed inside "
                f"{rect} within {timeout:.0f}s, and {MIN_FIELD_INK_PIXELS} px are needed. "
                "The baseline was taken AFTER the click and the rectangle is inset past "
                f"any focus ring ({FIELD_RECT_INSET} px), so a focus indicator cannot "
                "account for this either way: the typed text did not land in the field "
                "the agent clicked."
            )
        time.sleep(poll)


def _await_receipt(
    grant: vitrin_os.Grant,
    bands: tuple[tuple[int, int, int], ...],
    *,
    timeout: float = RECEIPT_TIMEOUT,
    poll: float = RECEIPT_POLL,
) -> tuple[vitrin_os.Frame, list[SolidRun]]:
    """Observe until the confirmation carries THIS task's receipt bands, in order."""
    deadline = time.monotonic() + timeout
    while True:
        frame = _observe(grant)
        runs = solid_row_runs(frame, bands)
        if match_bands(runs, bands):
            return frame, runs
        if time.monotonic() >= deadline:
            wanted = ", ".join("#" + rgb_hex(b) for b in bands)
            seen = ", ".join(str(r) for r in runs) or "none"
            raise DemoAssertionError(
                f"the confirmation never carried this task's receipt within "
                f"{timeout:.0f}s. Wanted {BAND_COUNT} full-width bands of >= "
                f"{MIN_BAND_ROWS} rows, in this order: {wanted}. Solid runs of those "
                f"colours actually seen: {seen}. These colours are a pure function of "
                "the SUPPLIED task, computed at runtime, so a frame either carries this "
                "record's checksum or it does not."
            )
        time.sleep(poll)


# --- result & reporting -----------------------------------------------------

@dataclass
class DemoResult:
    """What :func:`run` produced — enough for a caller to re-assert or diff."""

    ok: bool
    headless: bool
    task: tuple[tuple[str, str], ...]
    canon: str
    bands: tuple[tuple[int, int, int], ...]
    before: vitrin_os.Frame
    after: vitrin_os.Frame
    out_dir: pathlib.Path
    #: The realm-view points the agent clicked, in order: one per field, then
    #: the submit button. The gate recomputes nothing — it reads these back.
    clicks: list[tuple[int, int]] = dc_field(default_factory=list)
    #: Per field, how many pixels the CLICK ALONE changed inside that field's
    #: rectangle, measured between the frame the field was located in and the
    #: post-click baseline — i.e. the focus indicator's own footprint.
    #:
    #: Exported so a gate can assert the focus-ring trap is *real in this run*
    #: rather than only in an in-process fixture: a venue where this is 0 is a
    #: venue where the baseline ordering (:func:`_baseline_after_click`) has
    #: not been exercised at all, and saying so is more useful than quietly
    #: passing.
    focus_changes: list[int] = dc_field(default_factory=list)
    #: The nested venue's out-of-band ground truth (the page's beacon), or
    #: ``None`` headless, where ``form-target``'s ``SUBMIT`` line on the core's
    #: stdout plays the same role.
    submitted: list[tuple[str, str]] | None = None
    #: The nested page's URL, or ``""`` headless.
    url: str = ""


def _dump_frames(
    result_dir: pathlib.Path,
    before: vitrin_os.Frame | None,
    after: vitrin_os.Frame | None,
    recorder: str | os.PathLike[str] | None,
) -> None:
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
    task: tuple[tuple[str, str], ...] = TASK_DEFAULT,
    out_dir: str | os.PathLike[str] | None = None,
    url: str | None = None,
    recorder: str | os.PathLike[str] | None = None,
    connect_timeout: float = 30.0,
) -> DemoResult:
    """Drive the full goal-directed demo against a live core at ``socket``.

    The single entry point both venues use. ``consent`` is informational here —
    the agent's conduct is identical (``await_consent`` blocks until the
    petition resolves, whether a human clicked Allow or the guarded
    auto-approve did) — but it is threaded through so the transcript names the
    policy the run relied on. Raises :class:`DemoAssertionError` (or the SDK's
    typed exceptions) on failure, after dumping frames for diagnosis; returns a
    :class:`DemoResult` on success.
    """
    task = tuple(task)
    if len(task) != FIELD_COUNT:
        raise TaskError(
            f"the demo's form has exactly {FIELD_COUNT} fields; got {len(task)} pairs"
        )
    canon = canonical_task(task)
    bands = receipt_bands(task)

    out_dir = pathlib.Path(out_dir) if out_dir else pathlib.Path(
        tempfile.mkdtemp(prefix="vitrin-demo-")
    )
    page: _LocalPage | None = None
    conn: vitrin_os.Connection | None = None
    before: vitrin_os.Frame | None = None
    after: vitrin_os.Frame | None = None
    clicks: list[tuple[int, int]] = []
    focus_changes: list[int] = []
    try:
        page_url = ""
        if not headless:
            page = _LocalPage()
            page_url = url or page.url_for(task)

        print(f"demo: connecting to {socket} as {DEMO_IDENTITY} "
              f"({'headless' if headless else 'nested'} venue, consent={consent})")
        print(f"demo: task (supplied, not a constant): "
              + ", ".join(f"{k}={v!r}" for k, v in task))
        print("demo: this record's receipt is "
              + " ".join("#" + rgb_hex(b) for b in bands)
              + " — a 36-bit checksum of the record, NOT the glyphs")
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
        print("demo: grant resolved")

        if not headless:
            # Nested-only preamble: navigate the real browser to the served
            # form. Headless needs no equivalent — `form-target` IS the form.
            frame = _observe(grant)
            bar_x, bar_y = url_bar_target(frame)
            print(f"demo: clicking Firefox's URL bar at ({bar_x}, {bar_y}); "
                  f"typing {page_url!r} + Enter")
            grant.pointer.click(bar_x, bar_y)
            grant.text.type(page_url + "\n")  # the trailing newline presses Enter
            _locate_with_poll(
                grant, FIELD_MARKERS[0],
                what="the served form's first field",
                timeout=PAGE_LOAD_TIMEOUT,
                hint=(
                    f"That means THE PAGE DID NOT LOAD, not that the form is missing a "
                    f"field: the URL bar was clicked at ({bar_x}, {bar_y}), which is a "
                    f"pinned geometry constant for a {ESR_WINDOW_SIZE[0]}x"
                    f"{ESR_WINDOW_SIZE[1]} window. If this Firefox build lays its "
                    f"toolbar out elsewhere, set {URL_BAR_ENV}=x,y and re-run."
                ),
            )
            print("demo: the served form is on screen")

        # --- the field loop: identical code in both venues -----------------
        for index, (key, value) in enumerate(task):
            marker_hex = FIELD_MARKERS[index]
            frame, marker = _locate_with_poll(
                grant, marker_hex, what=f"field {index} ({key})"
            )
            if before is None:
                before = frame
            click_x, click_y = marker.click_point
            print(f"demo: located field {index} ({key}) at {marker}; "
                  f"clicking ({click_x}, {click_y})")
            grant.pointer.click(click_x, click_y)
            clicks.append((click_x, click_y))

            # AFTER the click, BEFORE the type — see `_baseline_after_click`.
            baseline = _baseline_after_click(grant, marker.rect)
            # What the click alone drew inside the field: the focus
            # indicator's footprint, and the size of the trap this ordering
            # exists to disarm. Measured on the FULL rectangle, so it is the
            # honest "what would a naive check have credited to the typing".
            focus_changes.append(changed_in_rect(frame, baseline, marker.rect))
            grant.text.type(value)  # no trailing newline: submission is a click
            measured = marker.rect.inset(FIELD_RECT_INSET)
            after, ink = _await_ink(
                grant, baseline, measured, what=f"field {index} ({key})"
            )
            print(f"demo: the click alone changed {focus_changes[-1]} px inside "
                  f"{marker.rect} (the focus indicator, baselined out); "
                  f"typed {value!r} -> {ink} px of ink inside {measured}")

        # --- submit --------------------------------------------------------
        frame, submit = _locate_with_poll(
            grant, SUBMIT_MARKER, what="the submit button"
        )
        click_x, click_y = submit.click_point
        print(f"demo: located the submit button at {submit}; "
              f"clicking ({click_x}, {click_y})")
        grant.pointer.click(click_x, click_y)
        clicks.append((click_x, click_y))

        # --- the receipt ---------------------------------------------------
        after, runs = _await_receipt(grant, bands)
        print("demo: receipt decoded from pixels: "
              + "; ".join(str(r) for r in runs))
        print("demo: the confirmation carries THIS task's 36-bit checksum — "
              "acceptance criterion met")

        submitted = page.submitted if page is not None else None
        if page is not None:
            print(f"demo: the page's own beacon reported {submitted!r}")

        assert before is not None and after is not None
        return DemoResult(
            ok=True, headless=headless, task=task, canon=canon, bands=bands,
            before=before, after=after, out_dir=out_dir, clicks=clicks,
            focus_changes=focus_changes, submitted=submitted, url=page_url,
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
        description="Vitrin OS Phase-1 goal-directed demo agent / M1.5 acceptance test.",
    )
    parser.add_argument("--socket", required=True,
                        help="path to the core's Unix socket (core.sock)")
    parser.add_argument("--headless", action="store_true",
                        help="headless venue: `form-target` stands behind the real shim")
    parser.add_argument("--consent", default="auto-approve",
                        choices=("auto-approve", "interactive"),
                        help="the consent policy the launched core runs (informational)")
    parser.add_argument("--task", action="append", metavar="K=V", default=None,
                        help="one field of the task record, repeatable and "
                             "ORDER-PRESERVING (the canonical string, and so every "
                             "receipt band, depends on the order). Defaults to "
                             + ", ".join(f"{k}={v}" for k, v in TASK_DEFAULT))
    parser.add_argument("--out", default=None,
                        help="directory for failure PNGs (a temp dir by default)")
    parser.add_argument("--url", default=None,
                        help="nested only: override the URL typed into Firefox's URL "
                             "bar (this script serves its own page by default)")
    parser.add_argument("--recorder", default=None,
                        help="flight-recorder path, printed on failure for diagnosis")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_argv(sys.argv[1:] if argv is None else argv)
    try:
        task = parse_task(args.task)
    except TaskError as exc:
        print(f"demo: bad --task — {exc}", file=sys.stderr)
        return 2
    try:
        run(
            args.socket,
            headless=args.headless,
            consent=args.consent,
            task=task,
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
