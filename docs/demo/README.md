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
4. **Capture → act → capture.** The agent captures a before-frame, locates
   the URL bar by pixels, clicks it, types a URL, presses Enter, and
   captures an after-frame. Zoom or pause on the actuation actually landing
   in the real Firefox chrome (cursor moving, text appearing character by
   character per the `text-input-v3`-avoiding keymap technique, D7).
5. **Hold-Esc revocation (bonus rung, if time allows).** Mid-actuation,
   physically hold Esc for one second; show the agent's next `observe()`/
   actuation failing `revoked` in the terminal log, and the recorder's
   flight log confirming it — the dead-man switch from issue #109.
6. **Wrap.** Show `xtask demo: PASS` (or the interactive equivalent) and the
   flight-recorder path in the terminal.

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
