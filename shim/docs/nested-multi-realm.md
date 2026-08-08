# Watching the right realm: the nested multi-realm runbook (WS-E.1.3, issue #209)

**This is a manual procedure, not a CI test — not skipped in CI, not present
in CI.** It exists because one clause of issue #209 cannot be automated, and
saying so plainly is cheaper than a gate that is cited for a property it never
checked.

## What CI proves, and the one thing it cannot

Issue #209 gave every realm its own scene and its own capture, and bound the
one output to one of them. Three of the four claims that follow are
machine-checkable and are checked:

| Claim | Where it is proved | Backend |
|---|---|---|
| A capture returns **the granted realm's** pixels, never the output's | `tests/integration/test_two_realms.py` (mock-free, two real `solid-client`s, cross-checked against each realm's own `--capture-dump`); `session.rs`'s `a_capture_returns_the_granted_realms_pixels_and_never_the_outputs`; `scene/realms.rs` | headless |
| A **hidden** realm keeps being paced and keeps repainting | `test_two_realms.py`'s `RealTwoRealmsHiddenKeepsPainting` (a paced `damage-client` off-screen beside a static `solid-client` on it); `session.rs`'s `a_hidden_realm_is_paced_and_its_capture_keeps_changing` | headless |
| The nested window composes the **bound** realm, and a bind re-uploads its texture | `backend/winit.rs`'s `the_window_shows_the_bound_realm_and_a_bind_re_uploads` — display-free assertions on `window_pixels` and `TextureKey` only | none (pure functions) |
| **A human sees the right realm in the host window** | *this page* | nested, by hand |
| **A real key held across a real focus switch is released into the realm being left** | *this page*, step 9 (WS-E.1.6/#212) | nested, by hand |
| **A real Super press on real hardware opens the attention window** | *this page*, step 10 (WS-E.1.7/#232) | nested, by hand |

The fourth is not automatable here and the reason is structural rather than a
missing budget: **GitHub runners have no display**, and D-019(4) records
headless as the only backend CI can run. The nested backend presents through
EGL/GLES into a host compositor's window; there is no runner that can open
one. So `backend/winit.rs` pins the two decisions `try_redraw` makes — *which
pixels to upload* and *when to re-upload them* — and leaves the GL submit and
the human's eye to this page. A gate that claimed otherwise would be the exact
class of dishonesty `tests/integration/README.md` is written to avoid.

## Prerequisites and a nesting host

Identical to `shim/docs/firefox.md` §8, which carries the host table in full
(nested GNOME via `dbus-run-session -- gnome-shell --devkit`, or `Hyprland`
from a spare TTY). In short:

```bash
cargo build --workspace
meson compile -C shim/build
```

## The realm file: two realms, two obviously different pictures

The whole point is to tell one realm from another by eye, so give the two
realms colours nobody could confuse. `solid-client` is co-built with the shim
and paints one flat colour over its whole surface.

```bash
cat > /tmp/two-realms.toml <<'EOF'
[[realm]]
id = "realm-0"
command = "REPO/shim/build/solid-client"
args = ["--run-ms", "600000", "--colour", "0000ff"]
env_allow = []

[[realm]]
id = "second"
command = "REPO/shim/build/solid-client"
args = ["--run-ms", "600000", "--colour", "ff0000"]
env_allow = []
EOF
sed -i "s#REPO#$PWD#g" /tmp/two-realms.toml
```

`realm-0` is mandatory in every configuration and sorts first, so it is the
realm the output binds to at startup; `second` is the hidden one. Moving the
binding afterwards is a client's to do, through the `layout.focus` grant verb
(WS-E.1.4/#210) — step 8 below does it, and step 9 uses it for the one thing
about **input routing** that no CI gate can reach.

Then, from inside your nesting host:

```bash
target/debug/vitrind --nested --consent=auto-approve \
  --shim "$PWD/shim/build/vitrin-shim" \
  --realm /tmp/two-realms.toml \
  --principals examples/principals.toml \
  --recorder /tmp/two-realms.jsonl
```

## What to check, by eye

1. **One window, one realm.** The host window is **solid blue** — `realm-0`,
   the realm bound to the output. It is *not* red, and it is not a mixture:
   `second` is running and painting, and none of its pixels are on screen.
   Confirm `second` really is alive rather than merely absent — `pstree -p
   $(pgrep -n vitrind)` shows **two** `vitrin-shim` processes, each with its
   own `solid-client`.
2. **The hidden realm is still costing you something.** `top -p $(pgrep -n
   vitrind)` while both realms run: the core composites the bound realm and
   composes the hidden realm's capture on every dirty round. This is the
   published cost (`docs/book/src/limits.md`), and seeing it is the point of
   listing it here — it is not a bug to report.
3. **An agent observing the hidden realm gets red, not blue.** From a second
   terminal, with the SDK on `PYTHONPATH`:

   ```bash
   PYTHONPATH="$PWD/sdk/python/src" python3 - <<'PY'
   import collections, os
   import vitrin_os

   socket = os.path.join(
       os.environ.get("XDG_RUNTIME_DIR", "/run/user/%d" % os.getuid()),
       "vitrin-0", "core.sock",
   )
   conn = vitrin_os.connect(
       socket,
       identity="vitrin://local/agent/demo",
       credential=open("examples/principals.toml").read().split('"')[-2],
   )
   for realm in ("realm-0", "second"):
       grant = conn.request_grant(realm=realm, verbs=("observe",))
       grant.await_consent()
       frame = grant.observe()
       px = collections.Counter(
           bytes(frame.raw[i : i + 3]) for i in range(0, len(frame.raw), 4)
       ).most_common(1)[0][0]
       # xrgb8888 is little-endian, so the bytes read back B, G, R.
       print(realm, "->", "#%02x%02x%02x" % (px[2], px[1], px[0]))
   PY
   ```

   Expect `realm-0 -> #0000ff` and `second -> #ff0000`. The headless gate
   already asserts those colours byte-exactly; what you are confirming here
   is that the *nested* backend serves them too, since it composes captures
   on the CPU rather than reading its window back.
4. **The agent cursor follows the visible realm only — but the actuation does
   not.** Actuate a pointer move under the grant over `realm-0` and watch the
   cyan crosshair appear in the window. Do the same under the grant over
   `second`: the click really is **delivered into `second`'s app** (WS-E.1.6
   routes per realm; before it, the core refused this `internal`) and yet
   **no sprite appears at all** — the sprite is painted in the output's coordinates over the
   output's realm, so an agent acting inside a hidden realm draws nothing.
   That is a *published limit*, not a defect to file: it reintroduces, for
   hidden realms, exactly the "the human cannot see that an agent is acting"
   defect D-019 exists to close, and the fix (a per-realm indicator in the
   trusted band) is not built. If you see a crosshair while actuating
   `second`, **that** is the bug.
5. **Resize the host window — and expect letterbox bars.** *Resize is not
   handled.* The core sends `configure` exactly once, when a realm's shim
   session starts (`spawn.rs`'s `start_shim_session`), and nothing re-sends it:
   the nested backend's `Resized` handler only tells `RealmScenes` the new view
   size, drops the uploaded texture and asks for a redraw. So both apps stay at
   their **startup** surface size and the composite places each one centered
   and 1:1 in the new view (`scene::layout::place`). Grow the window and you
   get blue in the middle with the deterministic background around it; shrink
   it and the surface is center-cropped. The window keeps showing `realm-0`
   throughout, which is the part this step is really checking.

   That is a real gap, not a decision: issue #209 decision 3 declined
   *per-realm* resize (one output, one size, no stacking, no overlap), which is
   a different question from re-configuring every realm when the one output
   changes size. It is published as a limit in
   [`docs/book/src/limits.md`](../../docs/book/src/limits.md), because a gap a
   reader can hit is one the project states rather than one it leaves to be
   discovered. No automated test can observe it: the headless backend — the
   only one CI runs — has a fixed virtual output that never resizes, which is
   why this runbook step exists at all.
6. **Kill the hidden realm's app** (`pkill -f 'colour ff0000'`). The window is
   unchanged — a sibling's death takes only that realm's surface — and a
   capture under the grant over `second` now refuses `no_surface` while
   `realm-0`'s still delivers.
7. **Kill the *visible* realm's app** (`pkill -f 'colour 0000ff'`). The window
   turns **red**: the output does not stay bound to a realm that is gone, it
   moves to the first still-serving realm in id order — the same rule
   `session::physical_seat_target` uses, so the realm you are watching and the
   realm **your own** keystrokes reach keep agreeing. (An *agent's* actuation
   never followed this; since WS-E.1.6 it goes to the realm its own grant
   names, watched or not.) Kill `second` too and the window falls back to the
   deterministic background, because now nothing is serving.

8. **Move the output with a real `focus`, and watch the human's input move
   with it.** Two realms are running; bind the output to the red one from a
   client, then type into it.

   ```bash
   PYTHONPATH="$PWD/sdk/python/src" python3 - <<'PY'
   import os, time
   import vitrin_os

   socket = os.path.join(
       os.environ.get("XDG_RUNTIME_DIR", "/run/user/%d" % os.getuid()),
       "vitrin-0", "core.sock",
   )
   conn = vitrin_os.connect(
       socket,
       identity="vitrin://local/agent/demo",
       credential=open("examples/principals.toml").read().split('"')[-2],
   )
   grant = conn.request_grant(realm="second", verbs=("observe", "layout.focus"))
   grant.await_consent()
   time.sleep(3)          # time to put your hands on the keyboard, for step 9
   grant.focus()
   conn.sync()
   print("focused 'second'")
   PY
   ```

   The window turns **red**, and — this is the part to check by eye — the
   human's keyboard and pointer go with it. Replace `solid-client` with
   `input-echo-client` in the realm file if you want to read that back: it
   prints one `IN ...` line per event it receives, on the core's stdout.

9. **THE STEP NO CI GATE CAN REACH: hold a real key across a real switch.**
   This is issue #212's manual criterion and it is the reason this section
   exists. Run the previous step again with `input-echo-client` in **both**
   realms, and this time **hold a physical modifier down** — Left Ctrl is a
   good choice, because it is in the core's layout-invariant scancode table
   and so resolves without any host keymap — from before `grant.focus()` fires
   until well after it does.

   ```toml
   # in /tmp/two-realms.toml, for this step only
   command = "REPO/shim/build/input-echo-client"
   args = ["--run-ms", "600000"]
   ```

   What must happen, in the core's stdout:

   - **before** the focus: `realm-0`'s app prints
     `IN key keycode=... state=1 ... name=Control_L` — the press.
   - **at** the focus, with your finger still down: `realm-0`'s app prints the
     matching `state=0` **release**. Nobody let go. The core sent it because
     your real release is now addressed to `second`, and an entry left behind
     would latch `Ctrl` down in an app you can no longer see.
   - `second`'s app prints **nothing** for that key. It never saw the press,
     so it is owed no release.
   - when you actually let go, nothing further arrives anywhere: the release
     pairs with no delivered press in `second` and is dropped.

   **The app cannot tell that synthesised release from a real one, and that is
   the published cost** (`docs/book/src/limits.md`, and
   `InputRouter::bind_to`'s own docs). The alternative is a latched modifier
   forever. If instead you see `realm-0` *keep* believing Ctrl is down — every
   later keystroke in it arriving shifted or Ctrl-modified once you focus back
   — the drain did not run, and `InputRouter::bind_to` is where to look.

   Why this is not a CI gate: `SeatInput::physical` is private to
   `crate::input`, its only production producer is the nested backend's winit
   intake, headless has no input device, and headless is the only backend CI
   runs (D-019(4)). The `physical-input-injector` build
   (`tests/integration/test_input_switch.py`) covers the *routing* half in CI
   through the same `intake_physical` entry point — a real finger on a real
   key across a real host compositor is what only this page can hold.

10. **THE OTHER STEP NO CI GATE CAN REACH: press a real Super key.**
    Issue #232's manual criterion, and the exact counterpart of step 9 — CI can
    prove what a completed attention press *does*, never that a real finger on a
    real key produces one. The `physical-input-injector` build
    (`tests/integration/test_attention.py`) covers the consequence half through
    the same `physical_key` entry point; a real keyboard is what only this page
    can hold.

    Set up as in step 8 (two realms, a client holding `layout.focus` over
    `second`), and this time **type into `realm-0` immediately before asking
    for the switch** — which is the whole scenario: in an in-realm shell the
    Enter that sends `focus()` is the physical input that forbids it.

    ```
    # with the demo agent, or any client holding layout.focus over `second`
    #   1. put your hand on the keyboard: type anything into realm-0
    #   2. within half a second, have the client call grant.focus()
    ```

    What must happen:

    - **Without** pressing Super: the client is refused
      `Preempted`, the window keeps showing `realm-0`, and the core's stdout
      carries no `attention_pressed` entry. That is the loop this key exists to
      close, seen from the human's side.
    - **Tap Super** (left Super by default; `--attention-chord rsuper` for the
      right one). Three things must be true at once:
      - the confined app in `realm-0` prints **nothing** for that key — run
        `input-echo-client` there and watch: the chord is **consumed**, so no
        app in any realm ever learns the human pressed it;
      - a small marker appears in the host window **just below** the trusted
        band, for about a second — beside the band, never inside it. If it
        overlaps the band, the band's "exactly one correct appearance" property
        is broken and that is a defect, not a cosmetic one;
      - the flight recorder gains an `attention_pressed` entry with
        `"chord":"super"`, `"opened":true` and a `notified` count equal to the
        number of clients holding a layout verb.
    - **Now** have the client call `grant.focus()` again, with your hand still
      on the keyboard: it is admitted, the window shows `second`, and the
      journal gains an `attention_claimed` entry naming that principal.
    - **Immediately ask for a second layout change** (`focus` back, or
      `set_fullscreen`), still with your hand on the keyboard: it must be
      refused `Preempted`. One press admits **one** layout use, and a holder
      that could focus, then fullscreen, then focus again would be spending one
      human gesture three times.

    Two things this step must **not** show, and each is a real defect if it
    does:

    - the dead-man switch behaving any differently. Hold Escape for a second
      while tapping Super in the same moment: the revocation must fire exactly
      as it does in `shim/docs/firefox.md` §9, and the hold bar must stay
      visible *over* the attention marker. `DeadManHook` is stacked outside
      `AttentionHook` precisely so a chord press wins.
    - the attention marker appearing in an agent's captured frame. Take a
      capture while the window is open and compare it against
      `--capture-dump`: the marker is drawn on the human-visible side of the
      output-stage fork, so a capture that contains it is the same class of
      leak issue #85 is about.

    Why this is not a CI gate: identical to step 9's reasoning.
    `SeatInput::physical` is private, headless has no input device, and headless
    is the only backend CI runs (D-019(4)).

## What a failure here looks like

- The window shows red, or flickers between red and blue → the output is not
  bound to one realm, or the bound realm's texture is being replaced by a
  sibling's commit. `TextureKey`'s `realm` field and `RealmScenes::bound` are
  where to look.
- Killing the *hidden* realm blanks or freezes the window → the teardown funnel
  is clearing the wrong realm's scene, or the retained-image scrub is not
  followed by a recomposite (`close_realm` sets dirty **and** calls
  `request_present`; both are needed on this backend).
- Killing the *visible* realm leaves the window on the background while `second`
  is still painting → `session::rebind_output_after_death` did not run, or ran
  and did not request a present. The output was bound once and never moved
  before that function existed, which is exactly what this looks like.
- On the zero-copy path the window shows the **hidden** realm's picture → the
  retained GPU content is not realm-keyed. `backend/winit.rs`'s
  `zero_copy_source` and `dmabuf.rs`'s `RealmGpuContent` are where to look. The
  CPU path was keyed on the bound realm from the start, so this failure appears
  only on the GPU path — which means only here: CI has no GPU, and the selection
  is all a display-free test can hold
  (`a_hidden_realms_dmabuf_import_is_never_what_the_window_presents`). Reaching
  it needs the shim's `--dmabuf`, and `vitrind --shim` takes a path with no
  arguments, so point it at a two-line wrapper that execs the real shim with
  `--dmabuf "$@"`.
- An agent observing `second` gets blue → the cross-realm capture leak is back.
  That one *is* covered by CI (`test_two_realms.py`), so a failure here with
  that gate green means the nested backend's `view_rgba` diverged from the
  headless one, which is D-019(3)'s named drift risk.
