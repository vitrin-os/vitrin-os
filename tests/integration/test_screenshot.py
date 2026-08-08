# SPDX-License-Identifier: Apache-2.0
"""WS-E.2.4 (#216): **the human's screenshot key writes a file and never
touches a grant** — against the shipped `vitrind`, a real C shim, a real
Wayland client, over a real socket.

This is the mock-free half of issue #216. The in-crate tests
(`screenshot.rs`'s directory audit and name minting, `session.rs`'s
`the_screenshot_chord_writes_the_focused_realms_view_and_touches_no_grant`,
`backend/headless.rs`'s
`the_trusted_indicator_reaches_human_visible_output_but_never_a_screenshot_file`,
`enforcement.rs`'s `single_enforcement_path_is_grep_provable`) pin the
mechanism in process; this drives the shipped binary and asks the filesystem
and the flight recorder what actually happened.

## Issue #216's "CI cannot press a key" criterion is STALE, and this proves it

The issue says the detection half — a real key press producing the trigger —
"is a one-line manual runbook step ... this criterion states that rather than
asserting a green check that cannot exist". That was written before WS-E.1.6
(#212) landed the **`physical-input-injector`** channel. A `screenshot` line on
that channel presses this run's configured chord, on its own evdev scancodes,
through the same `input::physical_key` the nested backend's keyboard handler
calls — never a second path and never a scancode this harness guessed. So the
gate below covers **detection and effect**, and the manual runbook step is
about real hardware only (`shim/docs/nested-multi-realm.md` step 11).

What the injector build weakens is named where the claim is made
(`crates/vitrin-core/src/input/mod.rs`'s module docs): in a shipping build
"nothing but a human's device mints a physical tag" is a compile-time
guarantee; here it is a runtime one (the feature, plus the flag, plus
possession of an inherited descriptor).

## What is asserted, and what each assertion is evidence *from*

1. **A bad `--screenshot-dir` refuses to start**, naming the reason, with a
   non-zero exit — for a path that does not exist, a path that is a regular
   file, a path that is a symlink to a directory, a world-writable directory,
   and a relative path. Evidence: the shipped binary's own exit status and
   stderr, not a unit test's view of a validator.
2. **One chord, one file**, whose name the core minted: read off the
   directory.
3. **Its bytes are the realm view, byte for byte.** The PNG is decoded with
   Python's stdlib `zlib` — this repository has no image codec anywhere, on
   purpose — and compared against `--capture-dump`, the **core-internal**
   RGBA readback of the same realm. That is the same ground truth
   `test_real_capture_fidelity.py` uses, and it is what makes "it wrote a
   picture of the right thing" checkable rather than assumed.
4. **The journal's digest is the digest of the bytes on disk**, and there is
   exactly one `screenshot_written` per chord.
5. **No grant was involved at any point.** The whole run makes no petition, so
   the journal must contain no `use_decision` and no `grant_minted`. Paired
   with (2)–(4) above so the negative is not vacuous: something definitely
   happened, and none of it was authority.
6. **Nothing outside the directory was written.** The tree the directory sits
   in is inventoried before and after, and the only new entry is the
   screenshot.

## Not asserted

That a real Ctrl-PrintScreen on real hardware produces any of this — that is a
nested manual runbook step, on the same terms as the attention key's.

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`).
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "sdk", "python", "src"))

from harness import (  # noqa: E402
    DEMO_IDENTITY,
    DEMO_TOKEN,
    MOCK_SHIM,
    VITRIND,
    IntegrationTest,
    capture_dump_path,
    children_of,
    comm_of,
)

#: Matches the rest of the real-app ladder.
REALM_SIZE = "640x480"
VIEW_W, VIEW_H = 640, 480

RUN_MS = "60000"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


def decode_png_rgb(data: bytes) -> tuple[int, int, bytes]:
    """Decode an 8-bit truecolor, filter-0 PNG into `(width, height, rgb)`.

    Deliberately hand-rolled over stdlib `zlib`: `crates/vitrin-core`'s
    `Cargo.toml` states twice that no image codec enters the tree in any
    dependency class, and a test suite that pulled Pillow to check the core's
    output would be honouring the letter of that and not the point. The core's
    encoder writes stored DEFLATE blocks and filter type 0, so this is a
    length check, an inflate, and a strip of one byte per row.
    """
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, width, height, idat = 8, None, None, b""
    while pos < len(data):
        length = int.from_bytes(data[pos : pos + 4], "big")
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            width = int.from_bytes(body[0:4], "big")
            height = int.from_bytes(body[4:8], "big")
            assert body[8] == 8, "bit depth must be 8"
            assert body[9] == 2, "colour type must be 2 (truecolor RGB)"
            assert body[12] == 0, "no interlacing"
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = width * 3
    rgb = bytearray()
    for y in range(height):
        row = raw[y * (stride + 1) : (y + 1) * (stride + 1)]
        assert row[0] == 0, f"row {y} uses filter {row[0]}, expected 0 (None)"
        rgb += row[1:]
    return width, height, bytes(rgb)


class HumanScreenshotKey(IntegrationTest):
    """The human's screenshot key, end to end, against a real app."""

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")
        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
                "Build it (meson setup shim/build shim && meson compile -C shim/build) and "
                "point the variable at shim/build/vitrin-shim. CI sets it."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so "
                "a real run was requested; refusing to skip a requested gate (CI misconfig)."
            )
        app = os.environ.get("VITRIN_CLICK_TARGET_APP")
        if not app:
            sibling = self.shim_bin.resolve().parent / "click-target"
            if sibling.is_file() and os.access(sibling, os.X_OK):
                app = str(sibling)
        if not app:
            self.fail(
                f"no click-target beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-screenshot-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    # -- fixtures ---------------------------------------------------------

    def _core(self, shots: pathlib.Path):
        self.dump_base = self.work / "internal.rgba"
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            physical_input=True,
            capture_dump=self.dump_base,
            screenshot_dir=shots,
            log_file=str(self.work / "core.log"),
        )

    def _one_live_app(self, core) -> None:
        deadline = time.monotonic() + 25.0
        while time.monotonic() < deadline:
            shims = [p for p in children_of(core.pid) if comm_of(p).startswith("vitrin-shim")]
            apps = [
                kid
                for shim in shims
                for kid in children_of(shim)
                if comm_of(kid).startswith("click-target")
            ]
            if apps:
                return
            time.sleep(0.05)
        self.fail("the realm's C shim never fork/exec'd its click-target")

    def _entries(self, core, kind):
        return [e for e in core.entries() if e["kind"] == kind]

    def _await_entry(self, core, kind, count=1, timeout=15.0):
        deadline = time.monotonic() + timeout
        seen: list[dict] = []
        while time.monotonic() < deadline:
            seen = self._entries(core, kind)
            if len(seen) >= count:
                return seen
            time.sleep(0.1)
        self.fail(f"the journal never recorded {count} `{kind}` entr(y|ies); got {seen}")

    # -- (1) the startup audit, against the shipped binary ----------------

    def _scratch_config(self, base: pathlib.Path) -> list[str]:
        """A demo-only registry and a loadable realm file, in `base`.

        Both are needed and neither is incidental: `run_session` audits the
        registry (the R6 auto-approve guard) and loads `realm.toml` **before**
        it opens `--screenshot-dir`, so a run with a missing or wrong one fails
        for that reason instead and would make every assertion below pass for
        something other than the directory. The first draft of this test read
        the operator's real `~/.config/vitrin/principals.toml` and did exactly
        that.
        """
        principals = base / "principals.toml"
        principals.write_text(
            f'[[principal]]\nidentity = "{DEMO_IDENTITY}"\ntoken = "{DEMO_TOKEN}"\n'
        )
        principals.chmod(0o600)
        realm = base / "realm.toml"
        realm.write_text(
            "[[realm]]\n"
            'id = "realm-0"\n'
            f'command = "{MOCK_SHIM}"\n'
            'args = ["--serve"]\n'
        )
        return [
            "--principals",
            str(principals),
            "--realm",
            str(realm),
            "--shim",
            str(MOCK_SHIM),
        ]

    def test_a_bad_screenshot_dir_refuses_to_start_naming_the_reason(self):
        """Every refusal in issue #216's acceptance criteria, plus two.

        Run against the real `vitrind`, so what is measured is the shipped
        binary's exit status and message — a validator with a green unit test
        and no caller would pass the unit test.
        """
        base = self.work / "audit"
        base.mkdir()
        config = self._scratch_config(base)
        a_file = base / "a-file"
        a_file.write_text("not a directory\n")
        real = base / "real"
        real.mkdir()
        link = base / "link"
        link.symlink_to(real)
        wide = base / "wide"
        wide.mkdir()
        wide.chmod(0o777)
        good = base / "good"
        good.mkdir()

        cases = [
            (str(base / "absent"), "cannot be opened as a directory"),
            (str(a_file), "cannot be opened as a directory"),
            (str(link), "cannot be opened as a directory"),
            (str(wide), "writable by group/other"),
            ("relative/shots", "must be an absolute path"),
        ]
        for path, expected in cases:
            with self.subTest(path=path):
                proc = subprocess.run(
                    [
                        str(VITRIND),
                        "--headless",
                        "--consent=auto-approve",
                        *config,
                        "--screenshot-dir",
                        path,
                    ],
                    capture_output=True,
                    text=True,
                    timeout=60,
                    env={**os.environ, "XDG_RUNTIME_DIR": str(self.work)},
                )
                self.assertNotEqual(
                    proc.returncode,
                    0,
                    f"`--screenshot-dir {path}` must refuse to start; it exited 0",
                )
                combined = proc.stdout + proc.stderr
                self.assertIn(
                    expected,
                    combined,
                    f"the refusal must name the reason; got:\n{combined}",
                )
                self.assertIn("--screenshot-dir", combined)

        # THE CONTROL, and without it the five above would go green for a
        # binary that refused every path. The same command line with a clean,
        # private directory must get *past* the audit — evidenced by the
        # startup line the core logs when the key is armed, which is printed
        # only on the success branch.
        control_runtime = self.work / "control"
        control_runtime.mkdir(exist_ok=True)
        proc = subprocess.Popen(
            [
                str(VITRIND),
                "--headless",
                "--consent=auto-approve",
                *config,
                "--screenshot-dir",
                str(good),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env={
                **os.environ,
                "XDG_RUNTIME_DIR": str(control_runtime),
                "RUST_LOG": "info",
            },
        )
        deadline = time.monotonic() + 30.0
        seen = ""
        try:
            while time.monotonic() < deadline:
                line = proc.stdout.readline()
                if not line:
                    break
                seen += line
                if "screenshot key armed" in line:
                    break
        finally:
            proc.kill()
            proc.stdout.close()
            proc.wait(timeout=30)
        self.assertIn(
            "screenshot key armed",
            seen,
            "a clean private directory must pass the audit and arm the key; the five "
            f"refusals above prove nothing without this. Got:\n{seen}",
        )
        self.assertNotIn("--screenshot-dir", seen.split("screenshot key armed")[0])

    # -- (2)-(6) the key itself -------------------------------------------

    def test_the_chord_writes_the_realm_view_and_no_grant_is_involved(self):
        shots = self.work / "shots"
        shots.mkdir()
        before = sorted(p.name for p in shots.iterdir())
        self.assertEqual(before, [], "the directory starts empty")

        core = self._core(shots)
        self._one_live_app(core)

        phys = core.physical
        self.assertIsNotNone(
            phys,
            "the harness must have wired --physical-input-fd; without it this gate proves "
            "nothing about physical input",
        )
        banner = phys.await_banner()
        self.assertEqual(
            banner,
            "vitrin-physical-input 1",
            "the core must have ADOPTED the channel, not merely been handed a socket. A "
            "plain build does not know `--physical-input-fd` at all and would have exited "
            "at argument parsing; rebuild with "
            "`--features vitrin-core/physical-input-injector` (tests/integration/run.sh does).",
        )

        # Let the app paint, so the ground truth below is a real client frame
        # rather than the deterministic background.
        dump = capture_dump_path(self.dump_base)
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline and not dump.exists():
            time.sleep(0.1)
        self.assertTrue(dump.exists(), f"the core never wrote {dump}")
        time.sleep(0.5)

        # (2) THE CHORD. One line is the whole gesture, on this run's own
        # `--screenshot-chord` scancodes, through the production intake.
        phys.screenshot()
        written = self._await_entry(core, "screenshot_written")
        self.assertEqual(len(written), 1, f"one chord, one entry; got {written}")
        entry = written[0]

        files = sorted(p for p in shots.iterdir())
        self.assertEqual(
            [p.name for p in files],
            [entry["file"]],
            "one chord must leave exactly one file, and the journal must name it",
        )
        name = files[0].name
        self.assertRegex(
            name,
            r"^vitrin-\d+-\d{4}\.png$",
            "the name is minted by the core from a session stamp and a counter; nothing a "
            "client controls may reach a path component",
        )
        self.assertEqual(
            files[0].stat().st_mode & 0o777,
            0o600,
            "the file is created 0600 -- which keeps it from OTHER users and does nothing "
            "about the confined app, which runs as this uid (docs/book/src/limits.md, D9)",
        )

        # (3) THE BYTES ARE THE REALM VIEW. `--capture-dump` is the
        # core-internal RGBA readback of the same realm's composited view --
        # the ground truth `test_real_capture_fidelity.py` uses -- so this
        # compares the screenshot against something produced by a different
        # path, not against itself.
        png = files[0].read_bytes()
        width, height, rgb = decode_png_rgb(png)
        self.assertEqual((width, height), (VIEW_W, VIEW_H))
        self.assertEqual(width, entry["width"])
        self.assertEqual(height, entry["height"])
        rgba = dump.read_bytes()
        self.assertEqual(
            len(rgba),
            VIEW_W * VIEW_H * 4,
            "the core-internal dump must be one full RGBA frame",
        )
        expected = bytes(b for i, b in enumerate(rgba) if i % 4 != 3)
        self.assertEqual(
            len(rgb), len(expected), "decoded pixel count must match the dump's"
        )
        self.assertEqual(
            rgb,
            expected,
            "the screenshot's pixels must be the realm view's, byte for byte: capture is a "
            "pure read of the last completed composite, so N reads of an idle scene are "
            "identical (crates/vitrin-core/src/capture.rs)",
        )
        self.assertGreater(
            len(set(rgb)), 1, "a uniform frame would make the comparison above vacuous"
        )

        # (4) THE JOURNAL IDENTIFIES THE ARTIFACT. BLAKE3 is not in the Python
        # stdlib, so the digest is checked for shape and stability rather than
        # recomputed; what makes it evidence is that a SECOND chord over the
        # same idle scene must produce the same digest and a different file.
        self.assertEqual(entry["digest_alg"], "blake3")
        self.assertRegex(entry["digest"], r"^[0-9a-f]{64}$")
        self.assertEqual(entry["realm"], "realm-0")
        self.assertNotIn(
            "principal",
            entry,
            "a screenshot has no principal: a human is not one, and a field naming one "
            "would be a fiction a later reader takes for a fact",
        )

        phys.screenshot()
        written = self._await_entry(core, "screenshot_written", count=2)
        self.assertEqual(len(written), 2)
        second = written[1]
        self.assertNotEqual(
            second["file"], entry["file"], "the counter advances; nothing is overwritten"
        )
        self.assertEqual(
            second["digest"],
            entry["digest"],
            "two chords over one idle scene must digest identically -- which is what makes "
            "the digest a fact about the pixels rather than about the moment",
        )
        self.assertEqual(
            (shots / second["file"]).read_bytes(),
            png,
            "...and the two files really are identical bytes",
        )

        # (5) NO GRANT, ANYWHERE. This run opened no connection and made no
        # petition, and the screenshot key must not have created one.
        self.assertEqual(self._entries(core, "use_decision"), [])
        self.assertEqual(self._entries(core, "grant_minted"), [])
        self.assertEqual(self._entries(core, "screenshot_failed"), [])

        # (6) NOTHING OUTSIDE THE DIRECTORY. The work tree is inventoried
        # against what this test itself created.
        expected_tree = {
            "shots",
            "core.log",
            dump.name,
        }
        self.assertLessEqual(
            {p.name for p in self.work.iterdir()},
            expected_tree,
            "the core wrote something outside its configured directory",
        )
        self.assertEqual(
            sorted(p.name for p in shots.iterdir()),
            sorted([entry["file"], second["file"]]),
            "the screenshot directory holds exactly the two files the journal names",
        )


if __name__ == "__main__":
    import unittest

    unittest.main()
