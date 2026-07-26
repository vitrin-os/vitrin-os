# Recording the demo screencast — the operator's runbook

`README.md` in this directory is the *why* and the shot list.
This page is the *how*: the exact commands, in order, for someone sitting at
a graphical Linux workstation with about twenty minutes.

Target: **60–120 seconds**, no narration required.

## 0. Preflight

```sh
# A Wayland session -- should print "wayland"
echo "$XDG_SESSION_TYPE"

# A browser for the realm. The demo defaults to firefox-esr; anything
# Firefox-shaped works via VITRIN_DEMO_FIREFOX.
command -v firefox-esr || command -v firefox
#   Arch:          sudo pacman -S firefox
#   Debian/Ubuntu: sudo apt-get install firefox-esr

# A screen recorder
command -v wf-recorder || command -v obs
#   Arch:          sudo pacman -S wf-recorder
#   Debian/Ubuntu: sudo apt-get install wf-recorder

# The shim must be built -- the demo refuses to substitute anything
ls -l shim/build/vitrin-shim || {
  bash shim/ci/install-deps.sh
  meson setup shim/build shim && meson compile -C shim/build
}
```

### On a tiling compositor, float the window first

**This bites on Hyprland, Sway and river, and it is not optional.** The
nested window asks for 1280x800 (`INITIAL_SIZE` in
`crates/vitrin-core/src/backend/winit.rs`), but a size request is only a
*request*: a tiling compositor ignores it and hands the window whatever its
tile happens to be. Everything downstream that names an absolute coordinate
-- the Firefox URL-bar step, and any framing you rehearse in step 1 -- is
then measured against the wrong geometry.

The window's Wayland `app_id` is **`vitrind`** (`NESTED_APP_ID`, kept stable
precisely so these recipes can match on it). Hyprland, in
`~/.config/hypr/hyprland.conf`:

```conf
# Hyprland >= 0.56: `windowrule` (windowrulev2 was removed) and the
# `match:` selector spelling. Check yours with `hyprctl version`.
windowrule = float,          match:class ^(vitrind)$
windowrule = size 1280 800,  match:class ^(vitrind)$
windowrule = center,         match:class ^(vitrind)$
```

Sway or river:

```conf
for_window [app_id="vitrind"] floating enable, resize set 1280 800
```

Then confirm it actually took, because Hyprland does not validate matcher
fields -- a clean `hyprctl configerrors` does **not** prove a rule fires:

```sh
hyprctl clients | grep -A6 'class: vitrind'   # expect floating: 1, size 1280 800
```

The form-filling steps themselves do not depend on this: the agent locates
every field by its marker colour in its own capture, so it is
resolution-independent by construction. It matters for the Firefox URL-bar
click and for your framing.

## 1. Rehearse it once, unrecorded

Do not record the first run. Watch where the consent card appears and where
Firefox's URL bar lands, so the real take has no hunting.

```sh
FF=$(command -v firefox-esr || command -v firefox)
VITRIN_DEMO_FIREFOX="$FF" cargo xtask demo
```

Confirm as it runs:

- The terminal log names `vitrin-shim`, **not** `vitrin-mock-shim`. If it
  says mock, the recording is worthless — that is the exact
  misrepresentation `README.md` in this directory refuses to make.
- Firefox renders inside `vitrind`'s window.
- The consent card appears **over** the Firefox pixels.
- After you click Allow, text appears in the URL bar character by character,
  the two-field form loads, and the agent then fills **both fields** and
  clicks the yellow submit button.
- The page ends as a **coloured receipt**: three full-width horizontal bands.
  Those colours are a 36-bit checksum of the record the page received — read
  the honesty note under the take table before you caption them.

## 2. Set the stage

- **Font size up** in the terminal. The log lines are part of the evidence
  and they must be readable in a 1280-wide video.
- **Move the terminal beside the `vitrind` window**, not behind it. Both are
  on screen the whole time: the window is the claim, the log is the proof.
- **Clear the terminal** immediately before recording.
- Close anything with notifications.

## 3. Record

```sh
# Whole output, 30 fps. -a drops audio; there is no narration.
wf-recorder -f ~/vitrin-demo-raw.mp4 -r 30
# ... perform the take ...
# Ctrl-C to stop
```

OBS equivalent: a single Screen Capture source, 1920×1080 canvas, 30 fps,
recording to MP4.

### The take, with timings

| t | What | Why it is in the shot |
|---|---|---|
| 0:00–0:05 | Terminal, clear. Run the command. | Shows there is nothing up your sleeve. |
| 0:05–0:20 | `vitrind`'s window opens; Firefox paints inside it. | A real browser, inside a realm, through the real shim. |
| 0:20–0:35 | The consent card appears. **Pause here.** | The card is occluding real Firefox pixels — it is a compositor overlay, not an application dialog. Let it sit on screen long enough to read. |
| 0:35–0:40 | Click **Allow**. | A human decided. |
| 0:40–1:00 | The agent clicks the URL bar and types. | Watch the **agent's** cursor move — the crosshair the core composites at the agent's own pointer position (D-019), not your desktop's mouse pointer, which the host draws outside the realm view — and characters land one at a time. Nested-only: `vitrind --nested` always composites it (a headless run needs `--agent-cursor`), and it is drawn into human-visible output alone, so it is in your screen recording and never in the agent's captured frames. |
| 1:00–1:15 | **Hold Escape for one second.** | The money shot — see below. |
| 1:15–1:25 | Terminal shows `revoked`; `xtask demo: PASS` and the flight-recorder path. | The evidence. |

**The hold-Escape beat is the one worth retaking until it is clean.** Do it
*while the agent is still typing*, not after. The point is an authority
dying mid-action, and the terminal should show the agent's very next call
failing `revoked`. If the typing finishes first, the shot only proves a
grant can be cancelled when nothing is happening.

Nested mode with a real held Escape is the recipe in
[`shim/docs/firefox.md`](../../shim/docs/firefox.md) §9.

## 4. Encode

Keep it small — this is going in a README and a web page.

```sh
# WebM/VP9, good quality, small. Usually a couple of MB for 90s.
ffmpeg -i ~/vitrin-demo-raw.mp4 \
  -c:v libvpx-vp9 -crf 34 -b:v 0 -an \
  -vf "scale=1280:-2" \
  docs/demo/nested-demo.webm

# MP4/H.264 fallback for browsers that want it
ffmpeg -i ~/vitrin-demo-raw.mp4 \
  -c:v libx264 -crf 25 -preset slow -an -movflags +faststart \
  -vf "scale=1280:-2" \
  docs/demo/nested-demo.mp4

# A poster frame -- pick a moment with the consent card up
ffmpeg -i docs/demo/nested-demo.webm -ss 00:00:28 -vframes 1 \
  docs/demo/nested-demo-poster.png

ls -lh docs/demo/nested-demo.*
```

If the result is more than a few MB, drop to `-crf 38` or trim rather than
committing a large binary — clone size is forever.

## 5. Publish

1. Commit the encoded files under `docs/demo/` if they are small enough
   (a few MB), otherwise attach them to a release or an issue and link the
   hosted URL from here instead.
2. **Landing page** — in `site/index.html`, find the comment
   `<!-- DEMO-VIDEO-SLOT` and replace the placeholder `<div class="demo">`
   with:

   ```html
   <div class="demo">
     <video controls preload="metadata" poster="nested-demo-poster.png">
       <source src="nested-demo.webm" type="video/webm">
       <source src="nested-demo.mp4" type="video/mp4">
     </video>
     <div class="cap">
       An agent petitions for a capability, a human approves a prompt the
       core drew itself, the agent drives a real Firefox inside a realm —
       and a held Escape revokes it mid-keystroke.
     </div>
   </div>
   ```

   and copy the three files into `site/` (or add them to the workflow's
   assemble step).
3. **README** — link it from the Status section.
4. **This directory's `README.md`** — replace "not yet recorded" with the
   published artifact.

## What not to do

The reasons are in `README.md` and they have not changed:

- **Do not record the headless run and label it "the demo".** Headless has
  no consent card to click and no key to hold; it proves different things.
- **Do not record against `vitrin-mock-shim`.** It is a unit-test fixture.
  It appears in no demo venue, and a recording of it would show the
  choreography without the substance.
- **Do not re-shoot the consent beat in a way that hides the app
  underneath.** The card occluding real application pixels *is* the claim.
