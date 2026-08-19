# Locking a nested session: the manual runbook (WS-E.2.2, issue #214)

**This is a manual procedure, not a CI test — not skipped in CI, not present in
CI.** It exists because the one claim a lock screen actually makes cannot be
automated here, and saying so plainly is cheaper than a gate cited for a
property it never checked.

## What CI proves, and the two things it cannot

Issue #214 landed a core-drawn lock screen: a surface the trusted core paints
itself, on the consent stack, whose gate consumes **every physical event** while
it is up. Most of that is machine-checkable and is checked.

| Claim | Where it is proved | Backend |
|---|---|---|
| The rendered card is byte-stable across architectures | `crates/vitrin-core/tests/golden/lock_screen.txt` (`lock::tests::lock_screen_golden`) | none (pure) |
| The lock reaches the human-visible framebuffer and is **byte-absent** from a capture of the same instant | `backend/headless.rs`'s `the_lock_screen_reaches_human_visible_output_but_never_a_capture` — the retained image *and* the memfd a `capture_frame` would seal | headless |
| **The dead-man chord still arms and fires while the lock consumes every event** | `lock/gate.rs`'s `the_dead_man_chord_arms_and_fires_through_a_locked_gate`, driven through the real `InputRouter<LockGate<ConsentGate<DeadManHook<ClipboardHook<AttentionHook<NoopHook>>>>>>` | none (real stack, synthetic intake) |
| A modifier held when the lock raises is **released** into the confined app, never latched | `lock/gate.rs`'s `a_modifier_held_when_the_lock_raises_is_released_in_the_app`, through the real router | none (real router) |
| N wrong passphrases write exactly N `unlock_attempted` entries, and the session stays locked | `lock/gate.rs`'s `typing_the_right_passphrase_unlocks_and_a_wrong_one_does_not` | none |
| `--lock-passphrase-file` on a keymap-less backend is refused at startup, naming the reason | `main.rs`'s `a_passphrase_is_refused_on_a_backend_that_cannot_deliver_the_alphabet` | none |
| `LockGate` is the **outermost** hook | `backend/winit.rs`'s `the_lock_gate_is_the_outermost_hook` — a type-level assertion, so a reorder fails to compile | none |
| **A human at a real keyboard cannot get past it** | *this page* | nested, by hand |
| **A real held modifier, a real chord, a real passphrase** | *this page* | nested, by hand |

The last two are not automatable, and the reason is structural rather than a
missing budget:

1. **No runner has a display.** D-019(4) records headless as the only backend CI
   can run; the nested backend presents through EGL/GLES into a host
   compositor's window, and there is no runner that can open one.
2. **No runner has a keyboard, and `SeatInput::physical` is private by design.**
   The core cannot mint a physical-origin event outside nested intake, which is
   the guarantee `crate::input`'s docs rest on. The
   `physical-input-injector` channel exists for exactly this gap on other
   features — but it is headless-only, and **every `--lock-*` flag is refused
   with `--headless`** (a headless session has no device that could dismiss a
   lock it raised), so it cannot reach this one.

There is therefore **no named integration gate for the lock screen**, and none
is listed in `tests/integration/run.sh`. That is deliberate. A gate that ran
headless would either prove nothing or require weakening the startup refusal
that keeps a headless session from wedging itself.

## What this page cannot make true either

Read the honest bound before running it:

- **In nested mode the lock locks a window, not a session.** `vitrind` is a
  client of your host compositor, which is above it and owns the real seat.
  Anyone can alt-tab away. This runbook checks that the *core's* claim holds
  inside its own window; the host's own screen lock is what protects the
  machine.
- **A locked screen does not suspend agents.** An `observe` holder keeps
  capturing across the lock. That is D-025, it is on the card, and step 7 below
  checks that it is really true rather than taking the card's word for it.

## Prerequisites and a nesting host

Identical to `shim/docs/nested-multi-realm.md`, which carries the host table in
full (nested GNOME via `dbus-run-session -- gnome-shell --devkit`, or `Hyprland`
from a spare TTY).

```bash
cargo build --workspace
meson compile -C shim/build
```

**Safety, non-negotiable:** run this in a *nested* host window. Never against a
DRM/TTY backend from inside a live session — that takes DRM master and the seat
and kills the session you are sitting in (WS-E §7).

## The realm file

One realm, painting something you would notice the absence of.

```bash
cat > /tmp/lock-realm.toml <<'EOF'
[[realm]]
id = "realm-0"
command = "REPO/shim/build/solid-client"
args = ["--run-ms", "600000", "--colour", "0000ff"]
env_allow = []
EOF
```

Replace `REPO` with your absolute checkout path; `realm.toml` refuses a
relative `command`.

## A passphrase file

```bash
read -rs PASS
printf %s "$PASS" | ./target/debug/vitrind --lock-hash > /tmp/lock.hash
chmod 600 /tmp/lock.hash
unset PASS
head -c 40 /tmp/lock.hash; echo ' ...'
```

The first line must read `vitrin-lock-v1 argon2id m=19456 t=2 p=1 ...`. The
passphrase itself never appears in `argv` (`/proc/<pid>/cmdline` is
world-readable) and never in the file.

`chmod 600` is not advice: the core **refuses to start** if the file is readable
or writable by group or other, because whoever can read it gets an offline
attack on your session passphrase. That is one bit stricter than `realm.toml`'s
rule, deliberately.

## Run it

```bash
./target/debug/vitrind --nested \
  --realm /tmp/lock-realm.toml \
  --shim "$PWD/shim/build/vitrin-shim" \
  --lock-passphrase-file /tmp/lock.hash \
  --lock-idle 30
```

The startup log must carry a `lock screen armed` line naming the chord
(`ctrl+alt+delete`), `idle_s=30` and `passphrase=true`. If it does not, nothing
below is testing what you think it is.

## What to check, by eye

**1. The chord raises it.** Press `Ctrl-Alt-Delete`. The blue realm disappears
behind an opaque near-black cover with a card on it, framed in this session's
trusted colour — the same colour as the strip along the top of the window. The
card says `Session locked`, names the realm (`realm-0`), and says `Locked by:
the lock chord`.

*If the frame's colour does not match the top strip, you are looking at a
forgery an app painted. Do not type your passphrase into it.*

**2. Nothing reaches the app.** Type anything. Move the mouse. Click. The realm
behind the lock must not change in any way when you unlock — `solid-client`
paints a flat colour, so use `damage-client` here instead if you want a stronger
signal.

**3. The passphrase.** Type a **wrong** passphrase and press Enter. Nothing
visible happens and the session stays locked. Do it three more times. Then check
the journal:

```bash
grep unlock_attempted "$XDG_RUNTIME_DIR"/vitrin-0/flight-recorder-*.jsonl
```

There must be **exactly four** entries, each `"accepted":false`, and **none of
them may carry anything about what you typed** — no bytes, no digest, no length.
Four separate lines rather than a count is the point: a summary cannot tell four
attempts a second apart from four a day apart.

**4. Editing works.** Type some characters, press Backspace a few times, press
Escape. Then type the **correct** passphrase and press Enter. The lock comes
down, the blue realm is back exactly as it was, and the journal gains one
`"unlock_attempted"` with `"accepted":true` followed by one `session_unlocked`.

**5. A held modifier is not latched — the P1.7.2 regression, re-checked by
hand.** Start a text app in the realm instead of `solid-client` (any of the
`shim/docs/firefox.md` targets will do). Hold `Shift` down. While still holding
it, have a second person press `Ctrl-Alt-Delete`, or wait out `--lock-idle`.
Unlock. Now type lowercase letters into the app.

They must be **lowercase**. If they come out capitalised, the gate consumed a
release whose press the router had delivered, and every keystroke in that app is
now being reinterpreted — the exact failure P1.7.2 fixed for the consent grab.

**6. The off-switch survives — the most important step on this page.** Lock the
screen. Now hold `Escape` for a full second (the dead-man chord and its default
hold).

- The amber hold bar must appear **across the top of the window, above the lock
  screen**. A lock that could hide a hold in progress would be a lock that hides
  the human's own off-switch.
- After a second, the journal must gain a `dead_man_triggered` entry.
- The session must **stay locked**. Revoking authority is not proof that a human
  is present, and a dead-man trigger that unlocked the screen would be a way
  past it.

*If the bar never appears or no entry is written, stop. That is the failure this
whole issue is written against: a human who locked their screen and can no
longer revoke an agent's authority is strictly worse off than one who did not
lock it.*

**7. The published surprise, checked rather than trusted.** With an agent
holding an `observe` grant over `realm-0` and capturing in a loop
(`sdk/python/`'s demo agent will do), lock the screen and let it keep running.

Its frames must **keep arriving and keep showing the realm**, not the lock
screen and not black. That is D-025 working as decided: the lock takes *your*
input away, not an agent's authority, and the card says so in as many words. If
you want the agent to stop, that is what step 6 is for.

**8. The idle raise.** Unlock, then take your hands off the keyboard for
`--lock-idle` seconds. The lock must raise itself with `Locked by: no physical
input for the configured idle time`.

Then, with the agent from step 7 still actuating, unlock and go idle again. It
must **still** lock after the same interval: an agent working through the night
is not a human at the keyboard, and its actuations must not postpone the lock.

## What a failure here looks like

| Symptom | What it means |
|---|---|
| The card's frame colour does not match the top strip | You are looking at a client-painted forgery, or the trusted indicator is not reaching one of the two paths (issue #85). |
| The realm is still visible behind the lock | The zero-copy dmabuf branch presented the client's texture. `LockSurface::is_raised` must be in `overlay_up` (`backend/winit.rs`). |
| The realm is still visible, but only on the *second* lock of a session, or the cover appears late when something unrelated moves | The presentation cache key is not carrying `LockSurface::generation`, so the composed cover is uploaded only when some other keyed input changes. **Your session is genuinely locked and eating your input** — type the passphrase blind. This shipped once; see "What the first run found" below. |
| Keys come out capitalised after step 5 | The gate consumed a release whose press was delivered — the pairing contract (`crate::input::PreemptionHook`) is broken for this gate. |
| No hold bar, or no `dead_man_triggered`, in step 6 | The observe tap is not being forwarded. This should be *unconstructible* (`crate::input::ConsumingGate`), so treat it as a compiler or refactor bug, not a policy one. |
| The agent's frames go black in step 7 | Somebody "fixed" D-025 by blanking the realm view, which is the lie-by-omission the decision explicitly rejects. |
| The session unlocks after step 6 | A dead-man trigger is being read as proof of presence. It is not. |

## Record the run

Date it and note the host compositor, exactly as `shim/docs/firefox.md` does.
The value of a manual runbook is entirely in whether anyone can tell when it was
last actually executed.

**Last executed:** 2026-08-09
**Host compositor:** Hyprland 0.56.2 (Arch Linux, 2560x1600, nested window
2554x1558)
**Core:** `f9f2b8a`, `--lock-idle 30`, `--consent interactive`
**Result:** PASS on steps 1-7, with two gaps named below. **The first execution
of this page found a shipped defect** — see below.

### What the first run found

Steps 1 and 8 passed. Then the *second* `ctrl+alt+delete` of the session
consumed input behind a screen still showing the unlocked application.

`TextureKey::current` in `backend/winit.rs` enumerated every input to
`compose_human_visible` **except the lock**, so the composed cover was
presented only when some unrelated keyed input next happened to move. Against
an idle client that is never. The first raise had drawn only because the client
was still producing startup frames and `scene_generation` moved with it.

Nothing else was wrong: the recorder showed `session_locked` /
`unlock_attempted accepted` / `session_unlocked` / `session_locked` in order,
and `service_lock` marked the frame dirty and requested the redraw. The redraw
ran, compared keys, and re-presented the stale texture. So the gate consumed
input exactly as designed while the cover's confidentiality property was
silently not delivered — **a locked session that looked unlocked**, which is
the row this page's failure table did not have.

Fixed in `f9f2b8a`. This is why the page exists: every automated surface listed
in "What CI proves" was green throughout.

### Evidence for this run

Recorder wall times, one session unless noted:

- **1 — the chord raises it.** Card up, `Locked by: the lock chord`, frame
  colour matching the top strip. Confirmed by eye; nothing else can confirm it.
- **2 — nothing reaches the app.** Stronger than the page asks. Across three
  lock windows the recorder logs **zero** `seat_delivered` entries: not the
  passphrase keystrokes, not the dead-man's `Escape` hold.
- **3 — the passphrase.** Five rejections (the page asks four), each its own
  entry a second apart, then one acceptance. Field set across all six is
  `schema_version, run_id, seq, mono_us, wall_ms, kind, accepted` — no bytes,
  no digest, and **no length**.
- **4 — editing works.** Correct passphrase unlocked on every one of six
  attempts across the sittings.
- **5 — a held modifier is not latched.** Run with `input-echo-client`, which
  resolves keys through xkbcommon as a real toolkit does. `Shift` pressed
  04:07:00.992, held; idle lock raised 04:07:31.094 (**a held key did not
  postpone it**); `Shift` **released 04:07:36.386, while locked**; unlocked
  04:07:50.133; `a`/`b`/`c` resolved `keysym=0x61/0x62/0x63`, lowercase, with
  `depressed=0x0`. The release reached the app through a raised gate, which is
  the P1.7.2 pairing contract holding live.
- **6 — the off-switch survives.** `dead_man_triggered chord=esc held_ms=1000
  revoked_grants=1`, then `grant_revoked cause=dead_man_chord`, then the
  agent's next capture `refused (revoked, refusal_voiced=true)`. Session stayed
  locked a further **82 seconds**. The amber hold bar was seen over the cover.
  An earlier sitting logged `revoked_grants:0` with no agent connected, which
  proves only that the chord is *reachable* while locked — run this step with a
  live grant or it is half a test.
- **7 — the published surprise.** Agent holding `observe` on `realm-0` captured
  once a second across a lock: 13 `use_decision → allowed` entries spanning
  `session_locked`, near-black fraction flat at 0.50, no cover in the frames.
  D-025 as decided.
- **8 — the idle raise.** Confirmed three times, `cause:"idle"`.

### Gaps this run did not close

- **Step 7 is weaker than it reads.** `input-echo-client` is static, so every
  frame carried the identical digest. "Kept capturing the live realm" and
  "served a stale cached frame" are indistinguishable from outside. Re-run with
  `damage-client` to settle it.
- **Step 8's second half is untested.** The agent held `observe` only and was
  revoked before the idle lock, so "an agent actuating must not postpone the
  lock" has never been exercised. Needs a grant carrying `actuate.pointer`.
- **Step 7 needs no goal-directed agent.** The page says "`sdk/python/`'s demo
  agent will do", but `run_demo.py` needs a real app serving a form — far more
  setup than this step. Roughly forty lines against `vitrin_os.connect` +
  `request_grant(verbs=("observe",))` + `observe()` in a loop is enough, and
  printing each frame's near-black fraction is what distinguishes the two
  failures named above.

> **This runbook has been executed, and the dated record above is that run** —
> 2026-08-09, nested under Hyprland 0.56.2, core `f9f2b8a`. #214 asks for "a
> documented manual runbook in `shim/docs/`, executed on the target laptop and
> dated"; the document exists and one dated execution of it now exists, so the
> half of the criterion that was outstanding when this block was written is
> outstanding no longer. The empty template that stood here is removed rather
> than left standing beside a dated PASS, because a page that says both is a
> page nobody can act on.
>
> What one nested run does **not** settle is recorded above under "Gaps this run
> did not close": step 7 is weaker than it reads, step 8's second half has never
> been exercised, and a re-run with `damage-client` is still owed. Nor does it
> reach bare metal — this run was nested, where, as this page says at the top,
> the lock locks a window and not a session.
>
> Why no gate replaces this page, then or now: it needs a real nested session on
> the maintainer's own laptop, driven by a human at a real keyboard holding a
> real chord. That is exactly the class of evidence CI cannot produce and an
> agent must not fake — the same split #212 and #232 wrote down for physical
> input. Everything the automated suite *can* reach is covered: the gate, the
> pairing contract, the off-switch surviving a locked gate, the passphrase path,
> the startup refusals, and the golden card. What the run adds that none of
> those do: that a human looking at a real screen sees the card, can type into
> it, and gets back in.
