# Demo screencast — recording plan and current status

**Status: not yet recorded.** This page is the honest placeholder issue
[#48](https://github.com/vitrin-os/vitrin-os/issues/48) asks for: a clear
path and instructions for publishing the nested-mode demo screencast,
written instead of a faked or stand-in recording.

## Why there is no recording in this PR

The screencast's subject is the **nested-mode** demo (`cargo xtask demo`,
no `--headless`): a real Wayland compositor session drawing `vitrind`'s
window, with Firefox ESR running inside the realm, a human watching the
core-rendered consent prompt, and the agent's pointer/text actuation
visibly landing on the page. That requires a graphical Wayland session and
an installed browser — neither is available in the sandboxed, headless
environment this documentation pass was authored in, and this PR does not
fake one. Recording it needs a workstation with a display; the
[nested manual-check recipe](../../shim/docs/firefox.md) already exercised
in issue [#109](https://github.com/vitrin-os/vitrin-os/issues/109)'s work
is the same recipe this screencast follows, plus screen capture.

Faking this (a screen-recording of the headless mock-shim path relabeled as
"the demo," or a recording of an unrelated compositor) would misrepresent
what runs today and is exactly the kind of half-believed claim
[`README.md`'s security notes](../../README.md#security-notes--what-the-mvp-does-and-does-not-confine)
say is worse than an honest gap. So: documented here, not faked.

## What the recording should show, in order

This is the shot list a contributor with a graphical Linux workstation
should follow. It mirrors `examples/agent-demo/run_demo.py`'s own steps, so
the recording is a straightforward capture of the acceptance path, not a
separately choreographed demo:

1. **Launch.** `cargo xtask demo` (nested, `--consent interactive`) from a
   terminal on the host compositor. `vitrind`'s window appears.
2. **Realm boot.** Firefox ESR starts inside the realm and renders
   `about:blank` in the nested window — via the real wlroots shim, not the
   mock (confirm the terminal log names `vitrin-shim`, not
   `vitrin-mock-shim`; see the root README's tracked gap on this point).
3. **Consent.** The agent's `request_grant` raises the core-rendered consent
   card over the Firefox window. A human clicks **Allow** — show the card
   occluding real Firefox pixels underneath, proving it is a compositor
   overlay, not an application dialog.
4. **Navigate.** The agent locates the URL bar by pinned geometry (version 1
   has no semantic tree; `VITRIN_DEMO_URL_BAR=x,y` overrides it), clicks it,
   types the local form URL, presses Enter, and waits for the served
   two-field form to appear. Zoom or pause on the actuation actually landing
   in the real Firefox chrome: the **agent's own cursor** — the crosshair
   the core composites at the agent's pointer position (D-019), not your
   desktop's mouse pointer — travels to the URL bar, then text appears
   character by character per the `text-input-v3`-avoiding keymap
   technique, D7. The crosshair is **nested-only**: `vitrind --nested`
   always composites it, a headless run only with `--agent-cursor`, and it
   is drawn into human-visible output alone — the agent's own captured
   frames never contain it, which is why the captured frames show the page
   and no cursor.
5. **Do the thing.** This is the beat the demo exists for. The agent was
   handed a **task record it did not author** (`--task K=V`, order-preserving)
   and now fills it: for each field it locates that field by its **marker
   colour in its own captured frame**, clicks its centre, types the value,
   and confirms ink landed inside that field's rectangle. Then it locates and
   clicks the **yellow submit button** — submission is a click, never an
   Enter key, so this frame is a pointer-actuation proof too.
6. **The receipt.** The page repaints as three full-width coloured bands, and
   the agent decodes them and compares them against bands computed from the
   supplied task **at runtime**. Caption this honestly: the bands are a 36-bit
   **checksum** of the record the page received, *not* the agent reading its
   own text back — see
   [`examples/agent-demo/README.md`](../../examples/agent-demo/README.md) for
   the normative encoding. And nothing here is a language model: the agent is
   deterministic and locates by colour.
7. **Hold-Esc revocation (bonus rung, if time allows).** Mid-actuation,
   physically hold Esc for one second; show the agent's next `observe()`/
   actuation failing `revoked` in the terminal log, and the recorder's
   flight log confirming it — the dead-man switch from issue #109.
8. **Wrap.** Show the flight-recorder path in the terminal. Note that step 7
   and `xtask demo: PASS` are mutually exclusive in a single take: a chord
   fired mid-run revokes the grant, so the demo's next call fails and the
   command exits non-zero. [`RECORDING.md`](RECORDING.md) sets out the two
   honest options (one take ending in revocation, or two labelled clips).

Target length: 60–120 seconds. No narration is required — captions or a
short written walkthrough alongside the video are enough; the point is
proof, not production values.

## Publishing, once recorded

1. Record with any screen-capture tool (`wf-recorder`, `obs-studio`, or the
   compositor's own capture, since this runs on a real Wayland session).
2. Encode to a reasonably small MP4/WebM (a few MB, not tens) or an
   animated GIF if the whole thing fits in a few seconds' worth of frames.
3. Either:
   - commit the file under `docs/demo/` (e.g. `docs/demo/nested-demo.mp4`)
     if it is small enough for the repo, and link it from this file and the
     root README's Status section; or
   - upload it as a GitHub release asset or to the
     `vitrin-os/vitrin-os` issue/PR that first publishes it (GitHub hosts
     video attachments on issues/PRs), and link the hosted URL from here
     instead — preferred if the file is more than a couple of MB, to avoid
     bloating clone size.
4. Update the root [README.md](../../README.md) to link the published
   artifact from its Status section, and remove the "no published demo
   screencast yet" line from the known-gaps list once it is live.
5. Close out the corresponding acceptance-criterion checkbox on issue
   [#48](https://github.com/vitrin-os/vitrin-os/issues/48).

## Prerequisite

Per the root README's tracked gap, the screencast is most convincing once
`cargo xtask demo` itself drives the real shim (issue
[#110](https://github.com/vitrin-os/vitrin-os/issues/110), open PR
[#127](https://github.com/vitrin-os/vitrin-os/pull/127)) rather than
`vitrin-mock-shim`. Recording against today's mock-shim default would show
the consent/actuation choreography correctly but not the "real Firefox
under nested mode" framing the issue asks for; recording against the real
shim directly (as `tests/integration/test_real_firefox.py` already proves
in CI) sidesteps that until #110 lands.
