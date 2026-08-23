# SPDX-License-Identifier: Apache-2.0
"""Issue #139: the trusted band's NEGATIVE half, against the shipped `vitrind`
+ the real C shim + a real app over a real socket -- no mock on any seam, no
in-process runtime.

Issue #85's property has two halves and only one of them can ever be a gate:

- **The negative half, which this proves.** A confined app's own rendering can
  never reach the trusted band's rows on the human-visible output, and never
  reaches the capture path at all. It is an *invariance* claim, so checking it
  needs no knowledge of the session's colour.
- **The positive half, which is permanently un-CI-able.** That a human, having
  learned the colour off the band, can tell a genuine prompt's frame from a
  forgery. That needs an eye and a display, and this gate does not narrow the
  gap by one inch. What exists for it today is one clause of
  `shim/docs/firefox.md` §9 ("visible in the trusted indicator band"), not a
  written procedure for checking a forgery against it -- a gap
  `docs/plan/01-phase-1-mvp.md` §5 records under "the fourth direction" rather
  than leaving it implied. **Do not read a green run here as "the trusted
  indicator is unforgeable."**

Until this landed, unspoofability had no gate-level evidence at all: the
existing proofs are `consent/mod.rs`'s band and frame tests,
`backend/headless.rs`'s `a_prompt_reaches_human_visible_output_but_never_a_capture`
and the real-app `c_shim_consent_prompt_occludes_...`, and `backend/winit.rs`'s
`no_presentation_path_can_drop_the_trusted_band` -- all of which plan §5 D12
classes as **component** evidence, because they build a view in-process rather
than driving the shipped binary. #138's consent gate could not close it either,
and never will: that gate is built so the harness never learns the colour, so
it cannot assert anything about the band at all.

# The constraint that shapes everything below

The harness must not learn, derive, or export the session's indicator colour,
and no pixel containing it may reach a file or a descriptor. That rules out the
obvious design (mirror the human-visible frame and look at row 0) and it also
rules out a subtler one, which is worth stating because it looks safe:

> Let the core export one boolean -- "the band's rows equal the realm view's
> rows beneath them". No pixels, no digest of the secret. It is an **oracle**:
> a confined app paints candidate colour `C` over its whole surface and reads
> the bit, which is exactly `S == C`. `TrustedIndicator::generate` scales each
> channel into `[64, 255]`, so `S` has at most `192**3` (about 7.1 million)
> values and the app recovers it by search.

So the rule the core-side witness holds to is stricter than "export no pixels":
**every field it exports must be a constant function of the run, independent of
the value of the indicator**, which is checked mechanically by
`band_witness.rs`'s `a_report_does_not_depend_on_the_bands_colour` -- two runs
identical but for the secret must produce byte-identical reports.

## Why not the commitment scheme #139 sketches

The issue asks for the reasoning either way, so: rejected, on three independent
grounds, any one of which is fatal.

1. **The secret is too small to commit to.** 22.8 bits. Any `H(S)` the core
   publishes is invertible by exhaustive search in well under a second, on the
   same machine, by the same-uid confined app that reads it out of
   `/proc/<core>/fd`. That is the same argument `consent/indicator.rs` already
   makes for never writing the colour to a log; a commitment does not escape
   it, it just adds a step.
2. **Salting it makes it uncheckable.** `H(S || nonce)` with a retained nonce
   cannot be verified by a harness that never holds a band pixel, and
   publishing the nonce collapses it to (1). A harness that *did* hold band
   pixels would have the secret in its address space and one `write(2)` from a
   file -- precisely the arrangement the design refuses.
3. **Replay, which is #139's own worry and is the general one.** Even a scheme
   immune to (1) and (2) publishes its witness on a channel a same-uid observer
   can reach, and the human's check is supposed to require *seeing the screen*.
   A witness the app can read is a witness the app can quote.

# What this gate proves, end to end, against `click-target`

`click-target` paints black with a centred green square and, on a landed
pointer click, repaints its **whole surface** red -- top rows included. That is
the strongest counterfeit a confined app can attempt against a colour it cannot
observe: own every pixel of the rows the band occupies, and change them.

1. **The capture path never carries the band.** The band's rows, read out of
   two independently transported artifacts of the same instant -- the agent's
   own `observe()` frame and the core-internal `--capture-dump` -- are
   *exactly* the colour the app painted, every pixel, before and after the
   repaint. Both of the app's colours have a channel below 64, and every
   channel of every mintable indicator is at or above 64
   (`indicator.rs::a_generated_indicator_is_opaque_and_visible`), so this is
   certainty rather than a 1-in-7-million coincidence. Artifact A's check is
   `_settle`'s own loop condition -- it returns only a frame whose band rows
   are exactly the app's colour, over `SETTLE_READS` consecutive identical
   reads -- so that is where a capture-path regression fails, with the
   two-cause diagnostic. The assertions restating it in the test body are
   deliberate restatements, not independent evidence, and say so.
2. **The app's rendering never reaches the band on the human-visible output.**
   The core-side witness reports `band_changes == 0` over every composite *it
   evaluated*, which includes composites it was not asked for (see 3) and the
   composite at which the app repainted its whole surface red -- `probe_changes`
   rising is what says the witness saw that repaint. That the set it
   evaluated is *every* composite of the session is a fact about where
   `BandWitness::observe` is called -- from `HeadlessOutput::present`, the
   backend's single composition path -- and rests on reading that code, not on
   this gate. What the gate rules out is the weaker and more likely mistake: a
   witness that only ever looks when asked.
3. **...and that zero is not vacuous.** An absence is equally true of a witness
   that never looked -- the P1.9.8 lesson, restated in plan §5 D12. So the same
   reply carries the counterweights, all cross-checked:
   - `probe_changes` **increases** across the click, so the witness does see
     change when there is change;
   - `composites` rises by **at least two** across a span containing exactly one
     `band` request. That bound is the load-bearing part: `answer_band`
     recomposites before reporting, so every read bumps the counter by one on
     its own and a bare "it went up" would be satisfied by a witness wired only
     into the reply path. The second composite is one the witness saw without
     being asked -- one of the session's;
   - `probe_fnv` -- a digest of *realm-view* pixels just below the band --
     equals the digest this harness computes over its own `--capture-dump` read
     of the same instant, and differs between the two samples. So the witness
     was evaluated on the frame the harness is holding, not on a stale or
     synthetic one;
   - `tracks_view == 1`: the human-visible frame below the band is
     byte-identical to the realm view. A frozen or erased output framebuffer
     would hold its band rows constant too, and fails here;
   - `band_uniform == 1`: the band's rows are one fully opaque colour, so a
     band blended with or partly overdrawn by client content fails;
   - `refusals == 0`: no composite went unmeasured.

# What this gate does NOT prove

- **Nothing about the colour.** A build whose indicator was a hard-coded
  constant, or minted after the listener bound, satisfies every assertion here.
  `indicator.rs`'s own tests and `run_session`'s ordering hold that, at
  component level.
- **Nothing about the per-prompt trusted ring.** The band and the ring are two
  paintings of one secret; only the band is measured. `consent/mod.rs`'s frame
  tests hold the ring.
- **Nothing about the nested backend.** This is the headless CPU composite.
  The nested backend's two presentation paths (CPU and zero-copy dmabuf) are
  held against each other by `backend/winit.rs`'s
  `no_presentation_path_can_drop_the_trusted_band` -- component evidence.
- **Nothing a human would call unforgeable.** See the split at the top.

# The instrumentation, and how narrowly it is gated

The reply comes from a `band` request on the **existing** `consent-injector`
channel (issue #138) -- not a second channel. Three gates stand in front of it,
the same three #138's injector stands behind:

1. the `vitrin-core/consent-injector` cargo feature, never enabled in a
   deployment build (`crates/vitrin-core/Cargo.toml`);
2. `--headless`, which the parser requires for the flag;
3. `--consent-injector-fd N` naming an **inherited socketpair** -- no name in
   the filesystem for the confined app to connect to. Without the flag the
   channel does not exist at runtime, so a feature build with no flag answers
   nothing.

The witness itself (`crates/vitrin-core/src/backend/band_witness.rs`) is
compiled under `cfg(any(test, feature = "consent-injector"))` so its arithmetic
stays unit-tested in a plain `cargo test`; its **wiring** -- the field on
`HeadlessOutput`, the call in that backend's composite, and the `band` reply --
is under the feature alone. A shipping `vitrind` computes nothing and answers
nothing. It reads no framebuffer: it is handed the two buffers the composite
already holds.

Reusing #138's channel rather than opening a second one is deliberate. This
request is a **read** that confers nothing, so it adds no authority to a channel
that already carries the power to answer a consent prompt; a second unnamed
socket would be a second thing to keep right for no gain.

# Watched failing (plan §5 D12 item 4)

Mock-freeness proves what a test is wired to, never what it discriminates. Four
breakages were applied to the real binary and each turned this gate red on a
different assertion:

- **`composite_trust_band` made a no-op** (the band never overdraws the
  client): `first["band_changes"] == 0` fails with `1 != 0`, naming the
  property.
- **The human-visible frame composited into `view_framebuffer` too** (the
  P1.7.1 fork folded, so the band reaches the capture path): the phase-1 settle
  fails, reporting 5120 of 5120 band-row pixels off-colour across 1 distinct
  colour — the whole band, in one colour that is not the app's.
- **`BandWitness::observe` stubbed out** (a witness that stops looking): the
  band's height comes back 0 and the run fails before it believes any zero.
- **`probe_fnv` computed over a synthetic buffer** (a witness reading something
  other than the presented frame): the digest cross-check fails, naming the
  disagreement.

Where the criterion is a computed metric, it is additionally pinned in-process
and binary-free, as plan §5 D12 requires: `band_witness.rs`'s
`a_band_that_did_not_overdraw_the_client_is_counted_as_a_change` feeds the
witness the exact frame pair a no-op `composite_trust_band` produces and
requires `band_changes == 1`, and
`an_erased_human_visible_frame_is_refused_by_tracks_view` requires the
frozen-framebuffer reading — which satisfies `band_changes == 0` perfectly — to
be rejected by `tracks_view` instead.

# Skip-or-fail policy (matches the real-app ladder)

- `VITRIN_SKIP_REAL_APP=1` -> skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim or `click-target` is missing ->
  fail (both co-built by the shim's Meson build).
- A `vitrind` built or invoked without the injector -> fail, naming the feature
  and the rebuild, never a silent skip: a real run was requested.
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
    capture_dump_path,
    children_of,
    comm_of,
    descendant_named,
    dominant_colour,
    exe_identity,
    file_identity,
    fnv1a64,
    locate_colour,
    packed_xrgb,
    require_binaries,
    shims_of,
)

require_binaries()

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

#: Same realm shape as the actuation, dead-man and consent gates, so a
#: `click-target` centroid found here means what it means there.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: How much green must be on screen before the agent trusts it located the
#: target. Same bar the actuation, dead-man and consent gates use.
MIN_TARGET_PIXELS = 5000

#: `crates/vitrin-core/src/consent/indicator.rs`'s visible floor: every channel
#: of a minted indicator lands in `[64, 255]`, asserted there by
#: `a_generated_indicator_is_opaque_and_visible`. This gate leans on it once,
#: and only in the safe direction: a colour with any channel BELOW this cannot
#: be the indicator in any session, so finding one of `click-target`'s colours
#: in the band's rows of a capture is proof the indicator is not there --
#: certainty rather than a 1-in-7-million coincidence. Nothing here depends on
#: the floor's exact value being 64; a raised floor only widens the margin.
INDICATOR_CHANNEL_FLOOR = 64

#: How many consecutive identical `--capture-dump` reads mean "the app has
#: settled". The P1.9.8 gate-integrity lesson: a control taken while the app is
#: still painting lets the app forge the later evidence.
SETTLE_READS = 4
SETTLE_INTERVAL = 0.15
SETTLE_TIMEOUT = 20.0

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


def _read_dump(path: str, size: tuple[int, int]) -> bytes:
    """Block until `--capture-dump`'s atomic temp+rename has produced a whole
    frame, then return its raw RGBA bytes (the realm view, never the output)."""
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


def _rgba_rows(buf: bytes, width: int, y0: int, y1: int) -> bytes:
    """Rows `[y0, y1)` of a tightly packed raw-RGBA frame."""
    return buf[y0 * width * 4 : y1 * width * 4]


def _off_colour_rgba(rows: bytes, rgb: tuple[int, int, int]) -> int:
    """How many pixels of a raw-RGBA row block are NOT exactly `rgb`.

    Exact, not quantised: `click-target` writes flat 32-bit fills into an shm
    buffer, the shim forwards them and `Scene::compose` blits them 1:1, so any
    tolerance here would be tolerance for the one thing being looked for.
    """
    return sum(
        1
        for off in range(0, len(rows), 4)
        if (rows[off], rows[off + 1], rows[off + 2]) != rgb
    )


def _off_colour_xrgb(frame, y0: int, y1: int, rgb: tuple[int, int, int]) -> int:
    """The same count over a wire frame (`B, G, R, X` per pixel)."""
    packed = packed_xrgb(frame)
    rows = packed[y0 * frame.width * 4 : y1 * frame.width * 4]
    r, g, b = rgb
    return sum(
        1
        for off in range(0, len(rows), 4)
        if (rows[off + 2], rows[off + 1], rows[off]) != (r, g, b)
    )


class RealTrustBand(IntegrationTest):
    """Issue #139: a confined app repaints its whole surface, band rows and
    all, and reaches neither the band on the human's display nor the band's
    rows on the agent's capture."""

    #: The colours `click-target` paints. Both carry a channel strictly below
    #: `INDICATOR_CHANNEL_FLOOR`, which is what makes "the band is not in this
    #: capture" a certainty rather than a probability.
    BACKGROUND = (0x00, 0x00, 0x00)
    HIT = (0xFF, 0x00, 0x00)
    TARGET_HEX = "00ff00"
    HIT_HEX = "ff0000"

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
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-band-"))
        self.addCleanup(shutil.rmtree, self.work, True)
        #: The `--capture-dump` base the core is launched with. Nothing is
        #: written here: the core writes one frame per realm at
        #: `PATH.<realm-id>` (WS-E.1.3), so readers use `self.dump_path`.
        self.dump = str(self.work / "internal.rgba")
        #: `realm-0`'s dump -- the realm this gate's grant names.
        self.dump_path = str(capture_dump_path(self.dump))
        self.core_log = self.work / "core.log"
        #: Read from the core once the channel is up; never hard-coded, so a
        #: change to `TRUST_BAND_HEIGHT` cannot leave this measuring the wrong
        #: rows while still passing.
        self.band_h = 0
        for colour in (self.BACKGROUND, self.HIT):
            self.assertTrue(
                min(colour) < INDICATOR_CHANNEL_FLOOR,
                f"this gate reads 'the band is absent' off {colour} being outside every "
                "mintable indicator; a colour wholly at or above the floor would make that "
                "reading a coincidence instead of a proof",
            )

    # -- the core, under the one policy the channel permits -----------------

    def real_core(self):
        """A real chain with the injector channel wired.

        `--consent=interactive` is not a choice this gate makes for its own
        sake: `main.rs` refuses `--consent-injector-fd` under auto-approve, so
        the channel that carries the witness only exists under interactive
        consent. The consequence is that the agent's petition raises a real
        prompt, which the test answers over the channel before it measures
        anything -- and that is fine, because the band's rows must survive a
        prompt going up and coming down exactly as they survive a repaint.
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
        """A RUNNING instrumented core is identifiable by how it was invoked,
        not only by how it was built (the C2 check `test_real_consent.py`
        makes, for the same channel and the same reason)."""
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
            # `shims_of`, not `children_of`: at --isolation=default (the
            # default since P2.6.2, #186) the core's direct child is the
            # `vitrin-realm-init` supervisor and the shim is ITS child, so a
            # direct-children walk finds no shim at all.
            found = shims_of(core.pid)
            if found:
                shim_pid = found[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        # The mock-freeness check, by INODE rather than by name. A confined
        # shim is bound at the core-chosen `/vitrin/vitrin-shim`, so its
        # `comm` comes from that basename whichever binary it is (P2.6.2,
        # #186; renamed from `/vitrin/shim` by #283) and a name test stopped
        # telling the real shim from `vitrin-mock-shim`. The running image's
        # inode does, and more sharply: a name says what a program is called,
        # an inode says which file is executing.
        self.assertEqual(
            exe_identity(shim_pid),
            file_identity(self.shim_bin),
            f"the realm's shim (pid {shim_pid}, comm {comm_of(shim_pid)!r}) is not "
            f"the C shim this gate named ({self.shim_bin}) -- vitrin-mock-shim must "
            "appear nowhere in this path",
        )
        app_pid = descendant_named(core.pid, "click-target", timeout=15.0)
        self.assertIsNotNone(app_pid, "the C shim never fork/exec'd click-target")

    def _settle(self, want: tuple[int, int, int]) -> bytes:
        """Block until the realm view is a settled frame whose band rows are
        exactly `want`, and return it.

        Two jobs in one loop. It is the P1.9.8 settle -- N identical reads, so
        the later "nothing moved" evidence is not taken against a still-moving
        app -- and it is also the barrier that says the app has actually
        painted the state this phase of the test is about.
        """
        deadline = time.monotonic() + SETTLE_TIMEOUT
        stable = 0
        last: bytes | None = None
        while time.monotonic() < deadline:
            frame = _read_dump(self.dump_path, REALM_WH)
            band_ok = (
                _off_colour_rgba(_rgba_rows(frame, REALM_WH[0], 0, self.band_h), want) == 0
            )
            if band_ok and frame == last:
                stable += 1
                if stable >= SETTLE_READS:
                    return frame
            else:
                stable = 0
            last = frame
            time.sleep(SETTLE_INTERVAL)
        rows = _rgba_rows(last, REALM_WH[0], 0, self.band_h) if last else b""
        off = _off_colour_rgba(rows, want) if last else -1
        total = REALM_WH[0] * self.band_h
        # The COUNT of distinct colours, never their values. On a build whose
        # capture path had started carrying the band, printing what is in those
        # rows would print the session secret into a CI log -- which is the
        # thing this whole gate is arranged not to do, and a diagnostic is no
        # excuse for doing it.
        distinct = len({rows[i : i + 4] for i in range(0, len(rows), 4)})
        self.fail(
            f"the realm view never settled with band rows == {want} within "
            f"{SETTLE_TIMEOUT:.0f}s: {off} of {total} band-row pixels were off-colour on the "
            f"last read, across {distinct} distinct colour(s), and it did not repeat "
            f"{SETTLE_READS} times.\nTwo causes, and those numbers tell them apart. If nearly "
            "all of the band's rows are off-colour in ONE colour, the human-visible band has "
            "reached the CAPTURE path -- the P1.7.1 fork has been folded, and that is the "
            "failure this gate exists for. If the frame simply never stopped moving, the app "
            "did not settle; without a settled frame every later assertion here would be about "
            "a moving target."
        )

    # -- agent-side helpers -------------------------------------------------

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
            cx, cy, count = locate_colour(frame, self.TARGET_HEX)
            if count >= MIN_TARGET_PIXELS:
                return frame, cx, cy
            time.sleep(0.1)
        self.fail(
            f"the green target never reached the agent within {timeout:.0f}s: click-target's "
            "frame did not arrive, so there is nothing to click"
        )

    def _await_flip(self, grant, timeout=20.0):
        """Observe until the app's surface is the HIT colour; return the frame."""
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
            if colour == self.HIT_HEX and pct > 90:
                return frame
            time.sleep(0.1)
        self.fail(
            f"the surface never flipped to #{self.HIT_HEX} within {timeout:.0f}s after the "
            "click. THE WHOLE-SURFACE REPAINT IS THIS GATE'S COUNTERFEIT ATTEMPT: without it "
            "the app never paints the band's rows in a second colour and `band_changes == 0` "
            "proves nothing. Dominant-colour sequence: " + " -> ".join("#" + c for c in seen)
        )

    def _witness(self, injector, dump: bytes) -> dict[str, int]:
        """Read the band witness and cross-check it against `dump`.

        The cross-check is what stops `band_changes == 0` being an absence over
        numbers of unproven provenance (plan §5 D12): the digest the core
        reports over the realm view's probe strip has to equal the digest this
        harness computes over the same rows of the frame it is holding. A
        witness reading a stale, synthetic or unrelated buffer fails here,
        before its zero is believed.
        """
        report = injector.band()
        self.assertEqual(report["refusals"], 0, "a composite went unmeasured")
        self.assertEqual(
            report["band_h"],
            self.band_h,
            "the core changed its mind about the band's height mid-run",
        )
        self.assertEqual((report["view_w"], report["view_h"]), REALM_WH)
        self.assertEqual(
            report["tracks_view"],
            1,
            "the human-visible frame below the band is not the realm view: it is frozen, "
            "erased, or carrying an overlay -- and a frozen frame would hold its band rows "
            "constant for free, which is exactly the vacuous reading this refuses",
        )
        self.assertEqual(
            report["band_uniform"], 1, "the band's rows are not one opaque colour"
        )
        # This session runs WITHOUT `--status` (WS-E.2.3, issue #215): the
        # status strip is opt-in precisely so that this gate's byte-for-byte
        # comparison of the human-visible frame against the realm view does not
        # become a function of the time of day. A non-zero height here means a
        # future default flipped, and `tracks_view` above would then be about
        # fewer rows than this gate believes it is about.
        self.assertEqual(
            report["strip_h"],
            0,
            "this session did not pass `--status`, so no rows below the band may be "
            "reserved for the status strip; `tracks_view` is otherwise about a smaller "
            "region than this gate thinks",
        )
        self.assertEqual(report["strip_changes"], 0)
        probe = _rgba_rows(dump, REALM_WH[0], self.band_h, 2 * self.band_h)
        self.assertEqual(
            report["probe_fnv"],
            fnv1a64(probe),
            "the witness's digest of the realm view's probe strip disagrees with this "
            "harness's digest of the same rows: the witness is not reading the frame the "
            "capture path is serving, so its counters are about some other buffer",
        )
        return report

    # -- the gate ----------------------------------------------------------

    def test_a_confined_apps_whole_surface_repaint_never_reaches_the_trusted_band(self):
        core = self.real_core()
        injector = self._assert_instrumented(core)
        self._spine(core)

        # The band's height comes from the core, once, and every later
        # assertion is expressed in it: a gate that hard-coded 8 would silently
        # measure the wrong rows if the constant ever moved.
        self.band_h = injector.band()["band_h"]
        self.assertGreater(self.band_h, 0, "a zero-height band is not a band")
        self.assertLess(self.band_h, REALM_WH[1])

        # A real petition raises a real prompt; answering it is a precondition,
        # not a claim -- #138's gate is what proves the consent path. Doing it
        # here at all is forced by `--consent=interactive` (see `real_core`),
        # and it doubles as a second thing the band has to survive.
        actor = core.connect()
        grant = actor.request_grant(
            verbs=("observe", "actuate.pointer"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        petition, token = injector.await_raised()
        self.assertEqual(injector.decide(token, "allow-while-running"), "queued")
        injector.await_lowered(petition)
        grant.await_consent()

        # == Phase 1: the app owns the band's rows in its own composition ==
        before = self._settle(self.BACKGROUND)
        first = self._witness(injector, before)
        self.assertGreater(first["composites"], 0, "the witness saw no composite at all")
        self.assertEqual(
            first["band_changes"],
            0,
            "client content reached the trusted band's rows on the human-visible output",
        )

        # Capture half, artifact A: the core-internal realm view. The band's
        # rows are the app's black, every pixel -- so the genuine band, whose
        # every channel is >= INDICATOR_CHANNEL_FLOOR, is not among them.
        #
        # DELIBERATELY REDUNDANT, and not independent evidence: this restates
        # `_settle`'s own loop exit condition on the frame `_settle` returned,
        # so it cannot fail here. `_settle` is where artifact A is actually
        # checked, and where a capture-path regression is reported with the
        # diagnostic that tells the two causes apart. It is restated at the
        # point of use so a reader of this phase sees the claim being made,
        # rather than having to reconstruct it from a helper's postcondition.
        self.assertEqual(
            _off_colour_rgba(_rgba_rows(before, REALM_WH[0], 0, self.band_h), self.BACKGROUND),
            0,
            "the core-internal capture's band rows are not the app's own colour: the "
            "human-visible band has reached the capture path",
        )

        # Capture half, artifact B: the agent's own frame off the wire, through
        # the enforcement chokepoint. Same instant, independent transport.
        frame, cx, cy = self._locate_target(grant)
        self.assertEqual((frame.width, frame.height), REALM_WH)
        self.assertEqual(
            _off_colour_xrgb(frame, 0, self.band_h, self.BACKGROUND),
            0,
            "the agent's captured frame carries something other than the app's own colour in "
            "the band's rows: `vitrin_view.frame_ready` is delivering the human-visible output",
        )

        # == Phase 2: the counterfeit attempt =================================
        #
        # A landed click repaints click-target's WHOLE surface red, band rows
        # included. That is the most a confined app can do against a colour it
        # has never observed: own every pixel of those rows and change them.
        grant.pointer.click(cx, cy)
        flipped = self._await_flip(grant)
        after = self._settle(self.HIT)
        self.assertNotEqual(before, after, "the realm view did not actually change")

        second = self._witness(injector, after)

        # The property, over every composite the witness evaluated -- the
        # repaint's included, since `probe_changes` below shows it saw the
        # client's rows move.
        self.assertEqual(
            second["band_changes"],
            0,
            "the app's whole-surface repaint reached the trusted band's rows on the "
            "human-visible output: a confined app can paint over the one reference a human "
            "checks a consent prompt against (issue #85)",
        )

        # ...and the counterweights, so that zero is not an absence over
        # numbers nothing tested (plan §5 D12).
        #
        # Two, not one: `answer_band` recomposites before it reports (headless.rs
        # `answer_band` -> `HeadlessView::redraw` -> `HeadlessOutput::present`,
        # which is where the witness is called), so a `band` request always bumps
        # `composites` by one on its own. Exactly one such request separates
        # `first` from `second`, so `assertGreater` here would be tautological --
        # satisfied by a witness wired only into the reply path, which is
        # precisely the "stopped looking" reading it is meant to refuse.
        # Requiring two demands at least one composite the witness saw WITHOUT
        # being asked: one of the session's own.
        self.assertGreaterEqual(
            second["composites"] - first["composites"],
            2,
            "the witness counted no composite it was not asked for: exactly one `band` request "
            "separates these two readings and each such request costs one composite of its own, "
            "so a rise of 1 is the read paying for itself and says nothing about whether the "
            "witness sees the session's own frames",
        )
        self.assertGreater(
            second["probe_changes"],
            first["probe_changes"],
            "the witness saw no change in the client's own rows just below the band, across a "
            "repaint that demonstrably changed them: it does not see change when there is any",
        )
        self.assertNotEqual(
            second["probe_fnv"],
            first["probe_fnv"],
            "the probe digest did not move across the repaint",
        )

        # Capture half again, after the repaint: both artifacts, both exactly
        # the app's red. The first of the two is the same deliberate restatement
        # of `_settle`'s exit condition as in phase 1 -- it cannot fail here;
        # the second, over the agent's own frame, is the independent one.
        self.assertEqual(
            _off_colour_rgba(_rgba_rows(after, REALM_WH[0], 0, self.band_h), self.HIT),
            0,
            "the core-internal capture's band rows are not the app's own colour after the "
            "repaint",
        )
        self.assertEqual(
            _off_colour_xrgb(flipped, 0, self.band_h, self.HIT),
            0,
            "the agent's captured frame's band rows are not the app's own colour after the "
            "repaint",
        )

        print(
            f"\n[real-trust-band] {second['composites']} composites, "
            f"{second['probe_changes']} client repaints of the rows below the band, "
            f"0 changes to the band's own {self.band_h} rows; both capture artifacts carried "
            "only the app's colours there, before and after a whole-surface counterfeit."
        )


class FnvAgreesWithThePublishedVectors(unittest.TestCase):
    """Pin this suite's FNV-1a-64 against the published test vectors.

    The gate above checks that the core's digest and this harness's digest of
    the same bytes agree. Mutual agreement between two implementations is not
    the same as either one being right -- two ports of the same mistake agree
    perfectly -- so the Python side is additionally pinned to known answers.
    Binary-free: runs everywhere, needs no shim.
    """

    def test_known_answers(self):
        self.assertEqual(fnv1a64(b""), 0xCBF29CE484222325)
        self.assertEqual(fnv1a64(b"a"), 0xAF63DC4C8601EC8C)
        self.assertEqual(fnv1a64(b"foobar"), 0x85944171F73967E8)


class TheGateNeverAsksForPixels(unittest.TestCase):
    """C1 (issue #85), grep-proved against this module's own source.

    The constraint on #139 is that the harness must not learn the session's
    indicator colour and that no pixel carrying it may reach a file or a
    descriptor. The `band` request is shaped so it cannot carry pixels --
    `ConsentInjector.band` raises if a descriptor arrives with the reply -- but
    the *other* request on this channel, `describe`, does export pixels (the
    consent card's footprint). This module has no business calling it, and this
    says so mechanically rather than by convention.

    What it proves is narrow and worth stating: that this file does not ask for
    the pixel-bearing request. It is not a proof about the core, which is where
    the real guarantee lives (`consent_occlusion_window`'s four fail-closed
    guards, and the whole-frame readback being `#[cfg(test)]`).
    """

    def test_this_module_never_calls_describe(self):
        # Assembled rather than written out, so this check does not match
        # itself -- the first draft did, and reported its own line as the
        # offender.
        needle = "." + "describe("
        source = pathlib.Path(__file__).read_text()
        offenders = [
            line.strip()
            for line in source.splitlines()
            if needle in line and not line.strip().startswith("#")
        ]
        self.assertEqual(
            offenders,
            [],
            "this gate must never ask the injector channel for pixels; found: " f"{offenders}",
        )


if __name__ == "__main__":
    unittest.main()
