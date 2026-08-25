# Bringing up the DRM backend by hand (WS-E.3.4, issue #220)

**This is a manual procedure, not a CI test — not skipped in CI, not present in
CI.** There is no DRM device and no seat on any GitHub runner, so the only
evidence this backend works is a human executing this page on one machine and
writing down what happened. That is structurally weaker than every other claim
in this repository, and it is published as such in
[`docs/book/src/limits.md`](book/src/limits.md) rather than discovered.

> ## Read this before anything else
>
> **A DRM backend takes DRM master. Starting one from inside the live session
> kills the session it is started from.** That is
> [WS-E §7](plan/14-workstream-session-mode.md)'s non-negotiable safety rule,
> and every step below is arranged around it.
>
> **On this machine there is no SSH escape route, by the owner's decision**
> (2026-08-09, recorded as
> [D-031, the *first* of two entries with that number](plan/20-decision-log.md#d-031--the-drm-bring-up-escape-route-is-a-hyprland-side-shell-and-an-installer-usb-not-ssh-and-not-vt-switching)
> — this line cited `D-028` until 2026-08-10, which is the keymap decision; the
> collision is reported at both D-031 headings).
> Issue #220 required an SSH session from a second machine and called it
> non-negotiable; it is not there. What replaces it is **step 0**, and the cost
> is stated plainly: **a wedged DRM master with no live console is now a reboot,
> where the design assumed it was a command.**

## Status of this document

| | |
|---|---|
| **The backend it describes** | **Exists and has been run on hardware** (#218, merged; first light 2026-08-09, with further rungs recorded on 2026-08-11 and 2026-08-13 below). `crates/vitrin-core/src/backend/drm.rs` landed with it, behind the `drm-backend` feature — which is **off by default**, because two of the crates it pulls panic the build when their `pkg-config` probe is absent (`crates/vitrin-core/Cargo.toml`, "Why it is not in `default`"), so a bare-metal `vitrind` is a build with that feature ON and step 2 below passes `--features drm-backend` explicitly. `grep -rn drm crates/vitrin-core/src/main.rs` names the `--drm` arm and the `--drm`-only flags today. |
| **Why it is written first** | #220's decision 1: the honest limit and the escape route must never be the thing that slipped behind the code. The `limits.md` entry and this page land with or ahead of #218's PR. |
| **Has it been executed?** | **Yes, and more than once.** Two runs on 2026-08-09 — first light, then the fix-confirming run — are recorded under [Record the run](#record-the-run), and step 13a has its own record block, executed 2026-08-13. Steps 12a, 16 and 17 are still marked NOT YET RUN, and #220's frame-cadence field was never captured in fps; the record says so where it stands. |
| **Who wrote it** | Not a human at the machine. Every fact below is marked verified or inferred — see the convention immediately below. |

### Verified vs inferred — the convention, and why it is here

An unverified runbook that reads as verified is worse than one that admits it,
so every claim about this machine carries one of two marks:

- **[verified]** — read from this machine on **2026-08-09**, read-only. No DRM
  device was opened, no module was loaded, no VT was switched, and no `vitrind`
  was started. `/sys`, `/proc`, `lsmod`, `pacman -Q` and `systemctl is-active`
  were the whole of it.
- **[inferred]** — from the source, from a manual page, or from a distribution
  default that is in place. **Never executed here.** Treat an inferred expected
  observation as a hypothesis you are testing, not a promise you are checking.

The failure column is inferred throughout. That is the honest limit of a page
written by someone who could not run it.

## The machine this is written for

Every path, node name and connector below is this laptop's, on purpose. A
generic runbook is one you have to translate at the worst possible moment.

| Fact | Value | |
|---|---|---|
| Kernel | `7.1.5-arch1-2` | [verified] |
| Mesa | `1:26.1.6-1` | [verified] |
| libinput / seatd | `1.31.3-1` / `0.9.3-1` | [verified] |
| Scanout card | `/dev/dri/card1`, driver **i915** | [verified] |
| Connected connector | **`eDP-1`** on `card1` — the only one in `connected` state anywhere | [verified] |
| Other connectors on card1 | `DP-1`, `DP-2`, `HDMI-A-1`, `HDMI-A-2` — all `disconnected` | [verified] |
| **Second card** | **`/dev/dri/card2`, driver `nvidia`, `nvidia_drm` loaded, all four of its connectors `disconnected`** | [verified] — see hazard H1 |
| Panel | 2560x1600, scale 1, 240 Hz ([WS-E §5](plan/14-workstream-session-mode.md)) | from the plan doc |
| Node permissions | `crw-rw----+ root video` on both cards; the operator is in `video` **and** carries the logind ACL | [verified] |
| Current session | Hyprland, logind session 2, `seat0`, **`tty1`**, `Type=wayland`, `Active=yes` | [verified] |
| Free VTs | `tty2`–`tty6`, no getty running on any of them | [verified] |
| VT autospawn | `/etc/systemd/logind.conf` leaves `NAutoVTs=6` and `ReserveVT=6` commented, i.e. at their defaults | [verified] — the autospawn *behaviour* is [inferred] |
| seatd | **installed, not running** | [verified] |
| systemd-logind | **active** | [verified] |
| Operator groups | `taha wheel input video plugdev docker` — **no `seat` group** | [verified] |
| sshd | **inactive, by the owner's choice** | [verified] |
| SysRq mask | **`16`** — sync only. Not `1`. See step 0.4. | [verified] |
| wayvnc | running, `--output=eDP-1`, bound to `127.0.0.1:5900`, reached over Tailscale | [verified] — see step 0.2 |

### Three hazards this machine has that a generic page would not warn about

**H1 — there are two DRM cards, and only one of them can light the panel.**
`card1` is i915 and owns `eDP-1`; `card2` is the NVIDIA GPU with `nvidia_drm`
loaded and **nothing connected**. [verified] Note that
[WS-E §5](plan/14-workstream-session-mode.md) states `nvidia_drm` is *not*
loaded — that is stale, and the module is loaded today. A udev-enumerating
backend that takes the first DRM device it finds can take `card2`, which has
zero connected connectors. #218's own rule ("refuse to start with a named error
on zero or more than one connected connector") then fires on the *wrong card* and
the error will read like a hardware fault rather than a device-selection bug.
**MARKED RESOLVED 2026-08-09 — REOPENED 2026-08-13. The 2026-08-09 entry is
kept below word for word, because what it got wrong is the reusable part:**
"RESOLVED 2026-08-09 — there is no card argument, and none was needed. `--drm`
takes no card selector; the backend calls smithay's
`udev::primary_gpu(&seat_name)` and picks the seat's primary GPU itself. On the
first run it selected `/dev/dri/card1` — the iGPU, which is the one with the
panel — unprompted and correctly. Hazard H1 below did not materialise."

**Why it is reopened:** the step 13a run of 2026-08-13 opened `/dev/dri/card2`
on the same machine, with the same absent card argument — see that run's own
record block below. The choice is now observed twice with two different answers:
`card1` on 2026-08-09, `card2` on 2026-08-13. "There is no card argument" was
never the same statement as "the automatic choice is stable", and the 2026-08-09
entry read the first as proof of the second. Do not hard-code either node as the
right one, here or in a command line: read the node the run itself names in
`DRM device opened card=`, and write it into the record. Why the answer changed
has not been established.

**H2 — `seatd` is not running, so libseat must use its logind backend.**
`libseat.so.1` links `libsystemd` and carries both `seatd_impl` and
`logind_impl`. [verified] With `seatd.service` inactive and the operator not in
a `seat` group, the seatd backend cannot work; the logind backend can, and
**only for a session logind considers active on its own VT**. This is why step 4
is *log in on the VT*, not merely `chvt`. Setting `LIBSEAT_BACKEND=seatd` here
will fail with a connection error to `/run/seatd.sock`, and that error is a
configuration mistake, not a bring-up finding. [inferred]

**H3 — Hyprland keeps its file descriptor on `card1` for the whole bring-up.**
It is not going to close it, and you do not need it to. logind revokes **DRM
master** from a session's devices when that session's VT stops being active, and
grants it to whoever is active. So "verify Hyprland is not holding the card" is
really "verify that your VT is the active one" — `fuser`/`lsof` showing Hyprland
on `/dev/dri/card1` is expected and is not the problem. [inferred]

---

## Step 0 — the escape route, before you type anything else

This is the step that will be tempted away, by the person most likely to skip
it, on the machine that matters, at the end of a long session. It costs about
ninety seconds.

> ## ⚠ CORRECTED BY THE FIRST RUN, 2026-08-09 — UPDATED 2026-08-11
>
> **VT switching did not work on the first run, and it was this page's first
> line of defence. It is implemented now (`crate::vt`, WS-E.3.5) and the chord
> has since been confirmed on hardware: 5 chorded switches on 2026-08-09
> (item 12 below) and 10 of 10 on 2026-08-11 (recovery runbook rung L1).**
> **All 19 of those chords were pressed against a *healthy* compositor**, so
> what is confirmed is that the chord works — not that it gets you out. The one
> deliberate wedge on record, on 2026-08-11, defeated it: a `SIGSTOP`ed
> compositor cannot call `Session::change_vt` because the code that would is
> inside the stopped process.
> On the first bare-metal run `Ctrl+Alt+F1` and `Ctrl+Alt+F2` did nothing and
> the maintainer was trapped on tty3 until `vitrind` was killed from elsewhere.
>
> The reason is not "something is grabbing the keyboard", which is what §12 of
> this page guessed. Once `vitrind` holds the display **the kernel stops
> handling `Ctrl+Alt+F<n>` at all** — the compositor must call
> `Session::change_vt`, and D-030(1) refused to implement it. Nothing was
> grabbing anything; the verb was never there.
>
> **The escape route that worked, and is now first:** a shell in the
> still-running **Hyprland session on tty1**. A `vitrind` session on tty3 does
> not disturb it, so a terminal there reaches the machine. **Resolve the PID
> first and signal that number** — never a `-f` pattern:
>
> ```
> pgrep -x -a vitrind   # -x matches the process NAME, not the command line
> kill -TERM <PID>      # the one number you just read
> ```
>
> **Do not shorten that to `pkill -TERM -f "vitrind --drm"`**, which this page
> published until 2026-08-11 and which is broken three ways at once (#260).
> `pkill -f` matches whole command lines, so the rescuing shell matches its own
> `argv` and `pkill` — which skips its own PID but not its parent — sends the
> `TERM` to the rescuer. It also does not match the target on this machine at
> all: the `~/.local/bin/vitrind` wrapper puts `--shim <path>` between the
> binary and whatever you typed, so the literal string never appears in the real
> process's command line. One injected argument is enough — the wrapper's other
> job is environment variables, which never reach `argv`, and `--blank-idle` is
> your own flag and lands *after* `--drm`. Checked
> read-only against a running session on 2026-08-11, where
> `pgrep -f 'vitrind --drm'` matched **only the invoking shell** while
> `pgrep -x -a vitrind` returned the one real PID. And under `systemd-run` there is
> no shell to strip the quotes for, `--drm` arrives as a second argument,
> nothing matches, and the unit exits 1 having signalled nothing. Full account
> in [the recovery runbook](book/src/recovery.md#route-2--a-shell-somewhere-else-and-a-signal).
> [verified 2026-08-11]
>
> **Do not start a run without a live Hyprland-side shell**, regardless. The
> chord is proven only from a healthy session; the shell is the only route that
> has actually recovered a wedged one. Treat the chord as the thing under test,
> not as the thing you are relying on.

### 0.1 First line: a shell inside the running Hyprland session.

Hyprland is on **tty1** and a `vitrind` session on tty3 does not touch it. That
is the whole property this depends on, and it is the one that has actually been
observed: the first run was recovered exactly this way. Leave a terminal open
there — or an agent session running in one, which is what happened — before you
start anything.

### 0.1b Second line: VT switching — implemented, unconfirmed.

Everything below was written before the first run and assumed `Ctrl+Alt+F<n>`
works. It did not, and now it is implemented rather than absent. Note what the
fix actually binds, because it is not what the first attempt bound: with a
`--keymap` loaded the chord arrives as `XF86Switch_VT_n`, not as `F1..F12` with
modifiers, and a matcher keyed on the F-row fires never.

logind hands the display to whichever VT is active, so switching back *would* be
ordinary and reversible. Hyprland is on **tty1** [verified], and this
configuration deliberately preserves `Ctrl+Alt+F<n>`: the keybind wrapper skips
F1–F12 precisely so the recovery path survives. [verified, as a statement about
the config's intent — and note that preserving the keys in the *wrapper* does
nothing if the *compositor* never acts on them, which is exactly what went
wrong]

**Before you start, prove the escape route works, in this order:**

1. From Hyprland, press `Ctrl+Alt+F2`. A login prompt should appear.
   - *Expected:* `agetty` spawns on tty2 and prints a login prompt. `NAutoVTs=6`
     is at its default, so logind starts one on demand — no `systemctl enable
     getty@tty2` needed. [inferred]
   - *If instead* nothing happens and the screen stays on Hyprland: your VT
     switching is being eaten (a keybind, or a compositor grabbing the chord).
     **Stop here.** You have no first line and no second line, and the runbook
     is now a one-way door. Fix this before continuing.
   - *If instead* the screen goes black and stays black with no prompt: the
     console is switching but nothing is drawing. Press `Ctrl+Alt+F1` blind. If
     you come back to Hyprland, you have a working VT switch and a broken
     console — usable, but you will be flying blind on tty2 and you should fix
     it first.
2. Log in on tty2. **Leave this shell alone for the whole session — it is your
   escape shell and nothing else ever runs on it.** `vitrind` goes on tty3, in
   step 4. An earlier draft of this page put both on tty2, which meant that from
   step 6 onward there was no logged-in shell anywhere and every recovery step
   below pointed at one that did not exist.
3. Press `Ctrl+Alt+F1`. You should be back in Hyprland with your windows intact.
4. Only now proceed.

The point of step 3 is that you have now *executed* the recovery path once,
while nothing was wrong. A recovery path you have never used is a plan.

### 0.2 Second line: your remote control is NOT an escape route. Read why.

`wayvnc` is running on this machine, `--output=eDP-1`, bound to
`127.0.0.1:5900` and reached over Tailscale. [verified]

**It runs through the desktop.** It is a Wayland client of Hyprland using
`wlr-screencopy` against a Hyprland output. The moment `vitrind` takes DRM
master and Hyprland's session goes inactive — or the moment Hyprland dies —
wayvnc has nothing to capture and no compositor to talk to. It cannot show you
`vitrind`'s output, and it cannot give you a shell.

Naming it as an escape route would be worse than omitting it, because you would
count on it in the one situation where it is guaranteed to be gone. It is
listed here **so that you do not reach for it.**

### 0.3 Last line: the Arch installer USB and a chroot.

Real, and slower by orders of magnitude. Physical access to the machine, a hard
power cycle, boot the USB, `mount` the root (and `/boot`, and unlock LUKS if the
disk is encrypted), `arch-chroot`, edit whatever wedged it, `exit`, reboot.
**Minutes to tens of minutes**, against seconds for a console command.

Have the USB physically present in the room before you start. If it is in a
drawer in another room, you do not have a third line; you have an errand.

### 0.4 SysRq: the keyboard combo is inert here, and the sudo path is elsewhere.

`/proc/sys/kernel/sysrq` is **`16`** on this machine, set by
`/usr/lib/sysctl.d/50-default.conf:19`. [verified 2026-08-10] `16` is *sync
only*: `Alt+SysRq+s` works from the physical keyboard and **every other letter
does not** — no `b` (reboot), no `e`/`i` (signal processes), no `u` (remount
read-only), no `r` (take the keyboard out of raw mode). The physical REISUB
sequence is inert here.

> **This page used to recommend raising the mask with `sysctl kernel.sysrq=1`,
> and that recommendation is deleted rather than softened.** The owner settled
> it on 2026-08-10: **the kernel mask is not touched.** Raising it hands
> `REISUB` to anyone at the physical keyboard of a machine whose whole premise
> is confining what runs on it, and no bring-up convenience buys that. Do not
> reinstate it, and do not propose it as an option.

**What replaces it is a `sudo`-only path that does not need the mask at all.**
The kernel gates the *keyboard* path on that bitmask and allows
`/proc/sysrq-trigger` to a privileged user regardless — verified against the
kernel's own documentation and source, with the correct sequence, the
destructive steps marked, and the reachable-shell caveat stated, in
[**the recovery runbook**](book/src/recovery.md#route-3--sysrq-through-procsysrq-trigger-sudo-only).
It is a way to end a session safely without the power button, not a way to
un-wedge a display, and it needs a shell you can still reach.

### 0.5 What you are explicitly NOT doing

- **Not** `systemctl isolate multi-user.target`. It works, and it takes your
  entire graphical session with it — every editor, browser tab and terminal you
  had open. Step 4's VT login gets you the same isolation with Hyprland still
  alive on tty1 to come back to. Reach for `isolate` only if a VT login turns
  out not to be enough, and know that you are trading your session for it.
- **Not** running `vitrind --drm` from a terminal inside Hyprland. That is the
  exact thing WS-E §7 forbids. It will either fail to get master (best case,
  and a confusing error) or take it (worst case, and your desktop is gone with
  its logs on a VT you are not looking at).

---

## Steps at a glance

> **Every "Expect" and every "failure" below is `[inferred]`.** Not one of them
> had been observed when this page was first written, because the backend did
> not exist yet. **The 2026-08-09 run changed that**: the record block at the
> bottom is observation, and where it contradicts an `[inferred]` mark above,
> the record wins. Individual `[verified]` marks appear
> only on *machine facts* — what `/sys`, `lsmod` and `systemctl` said on
> 2026-08-09 — never on what a running `vitrind` will do.
>
> An earlier draft marked four of the fifteen step tables and left the rest
> unmarked, which read as though those eleven had been checked. They had not.
> Read this page as a careful prediction, and correct it from the first real run
> rather than trusting it over your own eyes.

| # | Do | Expect | Worst credible failure |
|---|---|---|---|
| 0 | Prove the escape route | Login prompt on tty2, back to Hyprland on `Ctrl+Alt+F1` | VT switch is eaten — **stop** |
| 1 | Record the baseline from `/sys` | kernel, mesa, `eDP-1 connected`, preferred mode | — (read-only) |
| 2 | Build with `--features drm-backend` | clippy and build clean | Missing dev packages; a `cargo:warning` from smithay's gbm probe |
| 3 | Check the configs | `realm.toml` + `principals.toml` resolve, `--consent=interactive` | Auto-approve accepted where it must be refused |
| 4 | `Ctrl+Alt+F3`, log in | A shell on an active VT with a logind session | libseat cannot open the seat |
| 5 | Confirm the VT is active and the card is the right one | `Active=yes` for your tty3 session | Backend selects `card2` (hazard H1) |
| 6 | Start `vitrind --drm --consent=interactive` | Panel lights, trusted band on top | **Black screen, no console** — go to Recovery |
| 7 | Connector + mode | Log names `eDP-1` and the mode it chose — **paste the literal `mode set:` line** | Wrong connector; wrong refresh |
| 8 | App maps and repaints | Terminal visible, cursor blinking | Mapped but frozen (frame pacing) |
| 9 | Trusted band | 8 rows of one colour along the top edge | Band absent — **stop and file it** |
| 10 | Consent prompt + physical click | Card with a ring in the band's colour; click resolves it | Click does nothing (libinput not routed) |
| 11 | Held-Esc revocation | Hold bar, then every grant revoked | The off-switch does not arm |
| 12 | VT switch away and back | Same band colour after returning | Different colour, or a dead session |
| 13 | Type a letter | The letter appears in the app | Only modifiers/arrows work (no keymap) |
| 14 | Frame cadence | A number, measured, written down | — |
| 15 | Shut down cleanly | Hyprland intact on tty1 | Panel left in a bad mode |
| 16 | Brightness keys, on a second `vitrind --backlight` | The panel dims and brightens, never to black, and the app never sees the key | The key does nothing (a machine permission), or drives the wrong device |
| 17 | Idle inhibition, on a `vitrind --drm --blank-idle 60` with a video playing | The panel stays lit past 60 s while the video plays, and blanks at 60 s once it stops | The panel blanks under the video (nothing relayed), or never blanks again afterwards (a leaked inhibit) |

---

## 1. Record the baseline (read-only, safe from anywhere)

Run this from your normal Hyprland terminal *before* you go anywhere near a VT.
It opens no device.

```bash
uname -r
pacman -Q mesa libinput seatd libdrm
for c in /sys/class/drm/card*-*/status; do printf '%s %s\n' "$c" "$(cat "$c")"; done
cat /sys/class/drm/card1-eDP-1/modes | head -5
ls -l /dev/dri/
```

| Expected | Failure | What it means |
|---|---|---|
| `card1-eDP-1 connected`, everything else `disconnected`; `modes` lists the preferred mode first (widest × tallest at the highest refresh) | `eDP-1` shows `disconnected` | The panel is off or the lid state confused i915 — nothing below will work; do not proceed |
| Two cards, `card1` and `card2` | Only one card | Something changed since 2026-08-09; re-read hazard H1 before assuming which one it is |

**Write the kernel version, the mesa version, the connector and the preferred
mode into the record block at the bottom now**, while you can still copy-paste.
After step 6 you may be on a VT with no clipboard.

## 2. Build the backend

```bash
cd ~/projects/vitrin
cargo clippy -p vitrin-core --all-targets --features drm-backend -- -D warnings
cargo build --release -p vitrin-core --bin vitrind --features drm-backend
meson compile -C shim/build     # the shim must be current too
```

| Expected | Failure | What it means |
|---|---|---|
| Both clean | `pkg-config` panic from `drm-sys` or `libseat-sys` | The dev headers are missing. #218 decision 3: those build scripts `unwrap()` the probe, so a missing header is a build panic rather than a feature-off |
| No warnings | `cargo:warning` about gbm from smithay's build script | **Do not ignore this.** #218 records that smithay's gbm feature probe *fails soft* — a missing gbm header silently selects an older buffer-allocation path with no build failure. A bring-up that misbehaves after a soft-failed probe will look like a driver bug |

## 3. Check the configs, before you lose your comfortable terminal

```bash
cat ~/.config/vitrin/realm.toml
cat ~/.config/vitrin/principals.toml
```

Those two paths are the wrapper's documented defaults. [verified: the wrapper at
`~/.local/bin/vitrind` says so in its own header] The wrapper also sets
`WLR_BACKENDS=headless WLR_RENDERER=pixman` for the shim and passes `--shim`
explicitly, both of which you still need on DRM — the shim is internally
headless regardless of what the core presents on.

Make sure `realm.toml` declares **at least one `autostart = true` realm running
something you can see and type into** — a terminal. If the only realms are
templates, step 6 lights a panel showing the deterministic background and you
will spend ten minutes debugging a working compositor.

| Expected | Failure | What it means |
|---|---|---|
| Both files parse, one autostart realm with a terminal | `vitrind` refuses `--drm --consent=auto-approve` at parse time | Correct, per #218 decision 5. This backend *is* the human's display; auto-approving grants on it is the fail-open posture this repo refuses. Use `--consent=interactive` |

## 4. Get onto a free VT and log in

Press **`Ctrl+Alt+F3`**. Log in as `taha`.

**tty3, not tty2.** tty2 is the escape shell from step 0.1 and must stay a
shell — if `vitrind` runs there, killing it is the one thing you cannot do from
the console you were told to keep.

| Expected | Failure | What it means |
|---|---|---|
| A login prompt, then a shell | No prompt | See step 0.1 — you should already have proven this |
| `loginctl session-status` shows your new session with `Active=yes`, `Seat=seat0`, `TTY=tty3` | `Active=no` | You are not on the active VT. libseat's logind backend will refuse the device (hazard H2) |
| Hyprland's session (session 2, tty1) now shows `Active=no` | Hyprland still `Active=yes` | The VT did not actually switch; you are about to fight Hyprland for DRM master, which is the failure mode this whole page exists to avoid |

Do not skip the `Active=yes` check. It is the one machine-readable statement of
"it is safe to take master now".

## 5. Confirm what you are about to open

```bash
loginctl session-status | head -12
ls -l /dev/dri/
```

| Expected | Failure | What it means |
|---|---|---|
| Your tty3 session, `Active=yes` | as step 4 | — |
| You can `test -r /dev/dri/card1` | Permission denied | The logind ACL did not follow you to this session. You are in `video` [verified] so this should not happen; if it does, do not `chmod` anything — it means logind is not treating this as your active session |

**No card argument exists, and the automatic choice is not stable.** The backend
resolves the card through `udev::primary_gpu(&seat_name)`, and on 2026-08-09 it
chose `/dev/dri/card1` — the iGPU driving the panel. Hazard H1 (the NVIDIA card
present, `nvidia_drm` loaded, nothing connected) did **not** materialise on that
run: the run's log records `DRM device opened card=/dev/dri/card1`. The choice
did not hold, though — on 2026-08-13 the same absent argument opened
`/dev/dri/card2` (step 13a's record block), which is why H1 is reopened above
rather than historical. Keep the hazard written down — it is a property of this
machine, not of the code — and read the node out of the run's own
`DRM device opened card=` line before you believe either one of them.

## 6. Start it — this is the irreversible step

```bash
cd ~/projects/vitrin
vitrind --drm --consent=interactive \
  --realm ~/.config/vitrin/realm.toml \
  --principals ~/.config/vitrin/principals.toml \
  --keymap ~/.config/vitrin/keymap.xkb \
  --recorder ~/vitrin-runs/drm-$(date +%Y%m%d-%H%M%S).jsonl \
  2>&1 | tee ~/vitrin-runs/drm-$(date +%Y%m%d-%H%M%S).log
```

**`--keymap` is not optional, and its absence is silent in the worst way.**
Without it the core resolves only the layout-invariant scancode table — Escape,
Enter, Tab, Space, the arrows, the function keys, the modifiers — and **not one
letter, digit or punctuation mark reaches any app**. Every core chord still
fires, the panel lights, apps map and repaint, and the session looks entirely
healthy. It is simply mute. This command line omitted the flag until
2026-08-13, long after WS-E.3.1 (#217) settled the decision and
`~/.config/vitrin/keymap.xkb` was generated on 2026-08-09 — so step 13 below
would fail for a reason its own table reads as a known gap rather than as a
missing argument. A run that cannot type also silently weakens `L2`/`L3`: an
idle terminal nobody can type into produces no frames, and "correctly idle" is
then indistinguishable from "frozen".

**Artefacts under `~/vitrin-runs/`, not `/tmp`.** `/tmp` is tmpfs on this
machine, so a reboot destroys them — and a bad modeset is exactly the thing that
forces a reboot. A rung whose failure erases its own log cannot be diagnosed.

`tee` is not optional. If the panel does something you cannot read, the log on
disk is the only account of what happened.

**Expect no colour, on screen or in the file.** `vitrind` writes ANSI colour
only when its stderr is a terminal (issue #251), and `| tee` makes it a pipe —
so this command's output is plain on both sides. That is the intent: the two
previous runs' logs interleaved SGR escapes *between* a field's name and its
`=`, so `grep 'connector=' /tmp/vitrind-drm.log` matched nothing and every grep
below needed `sed 's/\x1b\[[0-9;]*m//g'` in front of it. Drop the `sed`. Run
`vitrind` without the pipe and the colour is back; `NO_COLOR=1` still turns it
off on a terminal.

| Expected [inferred] | Failure | What it means / what to do |
|---|---|---|
| The panel blanks briefly, then shows the realm's app with a coloured band along the top | **The screen stays black and the keyboard still works** | The backend did not present. You still have a console — `Ctrl+C`, read `/tmp/vitrind-drm.log`. This is the *good* failure |
| | **The screen goes black and the keyboard is dead** | Master was taken and something wedged before presenting, or libinput never opened. Go to [Recovery](#recovery), path R2. This is the failure the escape route exists for |
| | **The screen shows garbage, tearing, or a wrong-size image** | Mode set succeeded, scanout is wrong. `Ctrl+C` if you have a keyboard; record what it looked like — a mode/format mismatch is a real finding, not a crash |
| | It exits immediately naming a connector count | Hazard H1 — check which card it opened before believing the panel is at fault |
| | It exits naming libseat | Hazard H2 — check `Active=yes`, and do not set `LIBSEAT_BACKEND=seatd` |

---

## The observation checklist

Do these **in this order**. Each one assumes the last one passed. Record every
one as pass or fail with what you actually saw — **a failed observation is a
result, and recording it is the point.**

## 7. Connector and mode selected

**Paste the literal line into the record — do not paraphrase it.** Both previous
runs recorded "the connector name logged empty" in their own words and neither
kept a copy, so the one artefact that would settle what the panel actually said
does not exist and #250 could only rule out causes, never explain the
observation. A paraphrase cannot be argued about; bytes can. The line is
`vitrind`'s own `mode set: this session owns the panel`, and it carries
`connector=`, `width`, `height` and `refresh_hz`:

```bash
# From the escape shell, against the file step 6 tees. Since #251 landed,
# `vitrind` writes no SGR escapes into a redirected stderr, so the field name
# greps directly.
grep 'connector=' /tmp/vitrind-drm.log
```

**On a binary built before #251**, the escapes are still there and land between
`connector` and its `=`, so that grep matches zero lines while `grep connector`
matches. Use `grep -a 'mode set' /tmp/vitrind-drm.log | cat -v` instead — `cat
-v` shows the escapes rather than letting your terminal swallow them — and say
in the record which of the two you ran.

Paste what that prints, verbatim, into the record block at the bottom of this
page.

| Expected [inferred] | Failure | What it means |
|---|---|---|
| The log names `eDP-1` and a mode; the mode matches the first line of `/sys/class/drm/card1-eDP-1/modes` from step 1 | A different connector | Device/connector selection bug (H1) |
| | The grep matches nothing while `grep connector` matches | Escapes are back in the log — the tty test in `init_tracing` regressed (issue #251). Strip them with `sed 's/\x1b\[[0-9;]*m//g'` to finish the run, and file it |
| | The right connector, a lower refresh | The preferred mode was not taken. Not fatal; record the number — the whole GLES+GBM argument in [WS-E §5](plan/14-workstream-session-mode.md) rests on 240 Hz |
| | The connector name renders empty, as both previous runs recorded | **Keep the bytes and file them.** `connector_name` cannot produce an empty name and nothing in the log stack drops it (#250), so a third sighting with the literal line attached is the only thing that can move this |

## 8. A real app maps and repaints

Look at the terminal. Type nothing yet — just watch the cursor blink.

| Expected | Failure | What it means |
|---|---|---|
| The app is visible and its cursor blinks | Visible but completely frozen | The frame-callback path. #218 is explicit: `redraw` must return `Presentation::Scheduled` and the **page-flip handler** must call `session::emit_presented`. Answering `Completed` without presenting hands a `frame_done`-paced shim a fresh permit every dispatch round and it stops throttling — the symptom is usually the opposite (a runaway), so a *frozen* app more likely means `emit_presented` is never called at all |
| | Nothing but the deterministic background | No realm autostarted. Step 3 |

## 9. The trusted band

| Expected | Failure | What it means |
|---|---|---|
| 8 rows of one solid colour along the **top edge** of the panel, present in every frame | **No band** | **Stop the bring-up and file this.** `backend/mod.rs` is explicit that the band must live inside every presentation path's draw list precisely so a third path cannot drop it. A DRM backend presenting without a band is the most serious defect this page can find |
| | Band present but a different colour each frame | The indicator is being regenerated per frame. Same severity |
| | Band present, but client content overlaps it | Ordering violation (D-018). Record and file |

## 10. A consent prompt, answered by a physical click

Run an agent that petitions — the SDK demo agent will do — and let it ask for a
grant.

| Expected | Failure | What it means |
|---|---|---|
| A core-drawn card appears, framed in a ring **the same colour as the band**, and all input goes to it | Card appears, ring is a different colour | The two paths are drawing different secrets (issue #85's class). File it |
| **Clicking Approve with the physical mouse resolves it** | The click does nothing | libinput pointer events are not reaching `input::intake_physical`. Keyboard may still work — check both before concluding |
| The app behind the card receives nothing while it is up | The app reacts to your click | The consent grab is not exclusive. Security defect; stop |

## 11. Held-Esc revocation (the dead-man switch)

Hold Escape for the configured hold time.

| Expected | Failure | What it means |
|---|---|---|
| A hold indicator appears, composited last of all; on completion every grant in the session is revoked and a `dead_man_triggered` entry lands in the recorder log | No hold bar | The unconditional observe tap is not being fed by the libinput intake. This should be *unconstructible* (`crate::input::ConsumingGate`) — treat it as a refactor bug, not a policy one |
| | Bar appears, nothing is revoked | Worse than no bar: the human's off-switch is drawing a lie. Stop the session |

## 12. VT switch away and back (WS-E.3.3)

Press **`Ctrl+Alt+F2`** — your escape shell, and deliberately *not* tty3, which
is the VT `vitrind` occupies. Switching to `vitrind`'s own VT is a no-op, and an
earlier draft told you to do exactly that, so this checklist item would have been
recorded as a pass without a VT switch ever happening.
Wait five seconds. Come back.

| Expected [inferred — this is WS-E.3.3's answer and it is not written yet] | Failure | What it means |
|---|---|---|
| `vitrind` survives the switch, the panel comes back, and **the band is the same colour it was before** | A different band colour | The session secret was regenerated, i.e. the human's anchor moved under them. `TrustedIndicator` is generated once per process, so a changed colour means the process restarted — check whether `vitrind` is even the same PID |
| | `vitrind` died on the switch | `SessionEvent::PauseSession` is unhandled. This is exactly the coupling #218 records: this backend cannot honestly close with that handler unwritten |
| | Panel comes back black, `vitrind` alive | Master was not reacquired on resume. You still have VT switching — go back to your tty2 escape shell and kill it |
| | **You cannot switch away at all** | **THIS IS WHAT HAPPENED, 2026-08-09.** The guess this row used to carry — "something is grabbing the keyboard" — was wrong, and following it would have sent you hunting a phantom. Once `vitrind` holds the display the kernel stops handling `Ctrl+Alt+F<n>` entirely; the compositor must call `Session::change_vt`, and D-030(1) refused to implement it. There is nothing to un-grab. Recover from the Hyprland-side shell in Step 0.1 — `pgrep -x -a vitrind`, then `kill -TERM <PID>`. **Not** `pkill -f`; step 0 says why |

### 12a. The other two seat policies (WS-E, issue #246) — NOT YET RUN

Step 12 exercises the **default**, `--lock-on-seat-change never`. The other two
answers are `immediate` and `idle`, and **no run has ever exercised either**.
This rung is written before it is executed, on D-033's precedent, so that the
thing which has to happen is written down rather than implied; leave the record
block empty until you have actually done it.

Each is a fresh `vitrind` with the step-6 command line plus one flag, and each
needs a **passphrase file** to be worth anything — without one the card is a
privacy screen and Enter dismisses it, which proves the raise but not the cost.

```bash
# 12a-i — immediate: leaving locks, whatever the timers say.
vitrind --drm --lock-passphrase-file ~/.vitrin-pass \
        --lock-on-seat-change immediate ... 2>&1 | tee /tmp/vitrind-drm.log
# 12a-ii — idle: a long absence returns locked, a short one does not.
vitrind --drm --lock-passphrase-file ~/.vitrin-pass --lock-idle 60 \
        --lock-on-seat-change idle ... 2>&1 | tee /tmp/vitrind-drm.log
```

| Rung | Expected [inferred] | Failure | What it means |
|---|---|---|---|
| 12a-i | Switch to tty2, come straight back: the panel shows the **lock card**, and it names the seat (`the seat went to another VT…`), not an idle timer that never fired | Panel comes back unlocked | The policy never reached the lock. `grep on_seat_change /tmp/vitrind-drm.log` — the startup banner names it; if it says `never`, `run_inner` dropped the flag |
| 12a-i | | The card blames the idle timer | Wrong `LockCause` on the raise; the journal's `session_locked` cause should read `seat` |
| 12a-ii | Switch away, wait **~10 s**, come back: the panel is **unlocked** and the countdown has ~50 s of the absence charged against it | Locked after 10 s | The carry is over-charging, or `--lock-idle` is shorter than you think |
| 12a-ii | Switch away, wait **> 60 s**, come back: the panel is **locked** on the first round after you return, cause `idle` | Comes back unlocked | The absence was not charged. This is the exact shape issue #257 fixed once already — the instant must come from `session::note_seat_presence`, not from a cell an input turn wrote |
| 12a-ii | Switch away > 60 s, come back and **type immediately**: still unlocked, and it stays unlocked for a further 60 s | Locks on you as you type | A carry survived physical input, which is the one thing `judge` clears it for |

**Record block — empty on purpose. Do not fill it in from reasoning.**

```text
12a-i   date: ____  result: ____
12a-ii  date: ____  result: ____
```

## 13. Type a letter (WS-E.3.1)

Type `hello` into the terminal, on your real layout.

| Expected [inferred — depends on WS-E.3.1's keymap decision] | Failure | What it means |
|---|---|---|
| `hello` appears | Nothing appears, but arrows and modifiers work | The keymap half is missing. `invariant_keysym` covers Escape, arrows and modifiers and **not a single letter** — this is the exact gap WS-E.3.1 exists to close, and seeing it is a confirmation, not a surprise |
| | Wrong letters | The scancode→keysym resolution is wrong for this layout. Record which key produced which letter |
| | Letters appear doubled or stick | Key pairing moved from the keysym to the scancode (`input/mod.rs`); a mismatched press/release pair is the classic symptom |

### 13a. The touchpad classes (WS-E.4.2, issue #222) — RUN 2026-08-13

Steps 10 and 13 exercise the pointer and the keyboard. This rung exercises the
three classes the seat vocabulary grew afterwards — **relative motion, swipe
and pinch, and an app-requested pointer lock** — and it is the **only** place
in this repository where **libinput's own gesture detection** is exercised at
all.

That is the whole reason it exists, and it is worth being exact about the
boundary. `tests/integration/test_real_gestures.py` covers everything from
`input::intake_physical` onward — the router's pairing, the encode, the wire,
the shim's replay and what a real Wayland client receives — by injecting the
gesture events on the `physical-input-injector` channel. What it structurally
cannot cover is the step *before* that entry point: a touchpad reports finger
contacts, and **libinput** is what decides those contacts are a three-finger
swipe rather than two-finger scroll or a stray palm. Nothing in CI is evidence
that a human's three fingers produce a `gesture_begin` at all. Only fingers on
this laptop's touchpad are, and only through the DRM backend — the nested
backends never see a gesture, because the host compositor consumes it.

Written before it was executed, on D-033's precedent and 12a's, so the thing
that had to happen was written down rather than implied. **Executed 2026-08-13**
— the record block at the end of this rung is filled from that run's artefacts,
and it found the defect in issue #275. The rung earned its cost: what it caught
is unreachable from CI by construction.

The witness is `gesture-probe`, the same client the integration rung uses
(`shim/tests/gesture_probe.c`), because it is the only one that keeps pairing
state and reports `in_flight=` at exit — the one question a log cannot answer.
Build it with the shim (`meson compile -C shim/build`), point a realm at it,
and read its lines out of the `tee`'d log, since the app inherits the core's
stdout:

```toml
# ~/.config/vitrin/realm-gesture.toml -- one realm, the probe as its app.
# --run-ms is generous: you are doing this by hand, you cannot see the probe's
# output while vitrind owns the panel, and the clock does NOT stop while the
# seat is away on another VT. 300000 after the 2026-08-13 run; the 180000 this
# page carried first is not enough for five rungs plus 13a-v's VT round trip.
[[realm]]
id = "realm-0"
autostart = true
command = "/home/taha/projects/vitrin/shim/build/gesture-probe"
args = ["--run-ms", "300000", "--tag", "touchpad"]
```

```bash
# Step 6's command line with that realm file, and nothing else changed.
vitrind --drm --consent=interactive \
  --realm ~/.config/vitrin/realm-gesture.toml \
  --principals ~/.config/vitrin/principals.toml \
  --recorder /tmp/vitrind-drm-$(date +%s).jsonl \
  2>&1 | tee /tmp/vitrind-gestures.log
```

Then, having **moved the pointer at least once** — see the warning below, which
cost two runs on 2026-08-13 before the rung was executed:

**`gesture-probe` maps fullscreen, so there is nothing to aim at.** It fills the
whole *usable* view (`2560x1592` on this panel — the output minus the trusted
band's 8 rows, since issue #304 inset the realm view) with flat slate blue
(`0xff2050a0`, `shim/tests/gesture_probe.c:570`), so the pointer is over its
surface anywhere below the band and **there is no letterbox matte beside or
below it in this configuration**. The panel still looks exactly as it did
before the inset — the band's 8 rows are drawn over the reserved rows on the
way to the human, so what you see is 8 rows of band above a full-width slate
blue field, same as when the app painted those rows and the band covered them.
What still
matters is that the pointer **moves**: `pointer_enter` is delivered on the first
motion, not on map, and a run whose SUMMARY reads `enter=0 motion=0` tested
nothing no matter what else you did in it. Wiggle the cursor first, and treat
`enter=1` as the precondition for reading anything below.

Do not read a flat blue panel as a failed launch. **An empty scene renders the
deterministic test pattern** (`scene::compose`, `test_pattern::render`), never a
flat colour — so flat blue is the app, and a test pattern is its absence.

The generic warning the above replaces still holds for a *windowed* app: a
gesture over the matte belongs to nobody, because the router hit-tests a delta
against the stored pointer position.

| # | Do, on the real touchpad | Expected [inferred] | Failure | What it means |
|---|---|---|---|---|
| 13a-i | Move one finger | `IN pointer_motion` **and** `IN relative_motion` lines, the second with four numbers where `dx`/`dy` and `udx`/`udy` **differ** | Only `pointer_motion` | The relative half is not being minted. The absolute and relative halves come from one libinput event and `intake_physical` produces both; one without the other means the arm at `input/mod.rs` did not fire |
| 13a-i | | | Four numbers, but `dx == udx` and `dy == udy` exactly, every time | Pointer acceleration is off for this device (plausible, and *not* a bug), **or** the accelerated delta was copied into the unaccelerated field (a bug). `libinput debug-events` on another VT tells you which — but see the note below: this machine ships no `/usr/bin/libinput` |
| 13a-ii | Three fingers, swipe left | `IN swipe_begin fingers=3`, one or more `IN swipe_update`, then `IN swipe_end cancelled=0 paired=1` | Nothing at all | **This is the observation this rung exists for.** libinput never classified it, or the classification never reached the core. Check the log for `event=gesture_begin`: present means libinput saw it and the router dropped it; absent means libinput did not classify it |
| 13a-ii | | | `fingers=1` or the wrong count | The count is being flattened in transit. A toolkit dispatches on it (three fingers is a workspace switch where four is an overview), so this is a wrong action, silently |
| 13a-ii | | | `cancelled=1` on a swipe you finished | Inverted `gesture_state` polarity. An app that was previewing a workspace switch reverts it |
| 13a-iii | Two fingers, pinch out then in | `IN pinch_begin fingers=2`, several `IN pinch_update` whose `scale` is **absolute since the begin** — it grows past 1.0 as you spread and comes back as you close — then `IN pinch_end` | `scale` never leaves 1.0, or marches monotonically | Accumulated rather than carried through. The three quantities beside it are deltas and `scale` is not; multiplying successive scales zooms toward zero, and it is the one mistake an app cannot detect |
| 13a-iv | Two fingers, scroll | `IN pointer_axis` lines and **no** gesture lines | Swipe lines with `fingers=2` | Two-finger scroll is served as an axis event and always was (#222's own key decision). A swipe here means the intake is classifying scroll as a gesture, which double-reports every scroll |
| 13a-v | Three fingers down, hold them, then `Ctrl+Alt+F2` mid-gesture and come back | An `IN swipe_end cancelled=1` **before** the switch takes effect, and `in_flight=none` in the SUMMARY at exit | `in_flight=swipe` at exit | The app is latched. This is #222's acceptance criterion 3 on real hardware: the app can no longer receive your fingers, so the core owes it an end, and a missing one is an app accumulating a gesture forever. Step 12's caveat applies — if you cannot switch VT at all, use a second realm and a `layout.focus` instead |
| 13a-vi | Restart with `args = [..., "--lock-pointer"]`, move the pointer onto the app | `IN lock_requested tag=…` then `IN pointer_locked`, **the cursor sprite disappears**, and the pointer **stops moving** however far you swipe | `pointer_locked` but the cursor still moves | The verdict is being reported and not enforced. Worse than no lock: the app has been told it may hide its own cursor |
| 13a-vi | | | `pointer_locked` and the pointer freezes, but the core's own cursor sprite is **still drawn** | `hides_human_sprite` is not consulted in the DRM composite. This is the half **no** headless test can reach — every backend CI runs passes `human_cursor: None` — so it is observable here and nowhere else |
| 13a-vi | | | Held-Esc while locked does not revoke | The dead-man switch is behind the constraint. Stop the session; this is a security defect, not an input one |

**On measuring the device first.** #222's task 1 was taken one layer below
`libinput list-devices`, because this machine ships no `/usr/bin/libinput`
(the `libinput` package is the library only). `libinput-debug-events` is
therefore not available to cross-check what the kernel reported; read
`/proc/bus/input/devices` and the log instead, and record the substitution
rather than making it silently.

**Record block — EXECUTED 2026-08-13.** Filled from the artefacts named below,
not from reasoning. Two earlier attempts that tested nothing are recorded too,
because what made them fail is the reusable part.

```text
13a-i    (relative motion)      date: 2026-08-13  result: PASS
13a-ii   (three-finger swipe)   date: 2026-08-13  result: PASS
13a-iii  (two-finger pinch)     date: 2026-08-13  result: PASS
13a-iv   (two-finger scroll)    date: 2026-08-13  result: PASS
13a-v    (switch mid-gesture)   date: 2026-08-13  result: DEFECT -- issue #275
13a-vi   (pointer lock)         date: 2026-08-13  result: PASS
```

```
  Executed: 2026-08-13, by the maintainer, on the target laptop, tty3, three
            sessions. Artefacts under ~/vitrin-runs/ (NOT /tmp: this machine
            mounts /tmp as tmpfs, and a bad modeset can force the reboot that
            would erase the log explaining it).
              13a-main-20260813-124108.{log,jsonl}   rungs i-iv
              13a-v-20260813-132558.{log,jsonl}      rung v
              13a-lock-20260813-133107.{log,jsonl}   rung vi
    Binary: vitrind rebuilt 12:26 the same day from a clean tree at 9b6239e,
            --features drm-backend, clippy clean, no gbm soft-fail warning.
            Shim and gesture-probe rebuilt 12:28 -- see the wlroots note below.
     Card: /dev/dri/card2, NOT the card1 of the 2026-08-09 record. This page
            already warned the choice was not stable; it is now observed twice
            with two different answers. Do not hard-code either.

  13a-i .. PASS. 699 pointer_motion and 699 relative_motion, exactly 1:1, e.g.
            dx=0.602 against udx=2.000. The unaccelerated delta is genuinely
            minted, not the accelerated one copied. 44 of 699 samples have
            dx==udx: the zero and sub-pixel ones, where acceleration is
            identity. Not the failure mode -- that would be all of them.

  13a-ii ... PASS, and this is the observation the rung exists for. TEN swipes,
            every one fingers=3, every end paired=1. libinput's own gesture
            detection classified three-finger swipes on this touchpad and the
            finger count survived transit.

  13a-iii .. PASS. 4 pinch pairs, fingers=2, scale spanning 0.219 to 4.555 --
            crossing 1.0 in BOTH directions, so scale is absolute since begin
            and not accumulated. This is the mistake an app cannot detect, and
            it is not present.

  13a-iv ... PASS. 361 pointer_axis events and ZERO swipe_begin with fingers=2.
            Two-finger scroll is served as an axis event and is not
            double-reported as a gesture.

  13a-v .... DEFECT, issue #275. The gesture is not latched -- the app gets an
            end and exits in_flight=none -- but the end says `completed` where
            it must say `cancelled`. libinput flushes the in-flight swipe when
            the seat revokes the devices, 95 ms after the switch was requested
            and BEFORE session::suspend_physical_seat runs, so the core's own
            cancel path (InputRouter::end_physical_gesture) finds nothing and
            reports released=0. Proven on the recorder's single mono_us clock,
            with the Ctrl/Alt chord sandwiched between gesture_begin (7.302 s)
            and gesture_end (8.711 s), vt_switch_requested at 8.616 s.

  13a-vi ... PASS on all three halves, including the one no headless test can
            reach. The cursor sprite DISAPPEARED (observed; hides_human_sprite
            is consulted in the DRM composite, and every backend CI run passes
            human_cursor: None). The pointer FROZE: 536 relative_motion while
            locked against ZERO absolute pointer_motion, and the first absolute
            position after unlock is sx=1284.102 sy=798.551 -- dead centre of
            2560x1600, where it locked. Held-Esc REVOKED through the lock:
            `dead-man chord completed ... withdrawn_pointer_constraints=1`,
            and the app received pointer_unlocked. The dead-man switch is not
            behind the constraint.
```

**Two attempts before this tested nothing, and the reason generalises.** The
first (`13a-v`, 12:57) recorded `enter=0 motion=0 swipe_begin=0`: the operator
was looking for an app window to aim at, found a flat blue panel, and never
touched the touchpad. The second failure mode was nearly wasted on advice to
"hold three fingers still" — **wrong**, and it would have produced nothing:
libinput classifies stationary fingers as a **hold** gesture, and the wire has
exactly two kinds (`GestureKind::ALL` = `[Swipe, Pinch]`), so a hold is dropped
and nothing is ever in flight. This machine's hold detector is demonstrably
live; its timer appears in the log by name. **Keep the fingers sliding.**

**A substitution, recorded rather than made silently.** `--run-ms` was raised
from the 180000 this page prints to 300000, and a third realm file
(`realm-gesture-v.toml`, 90 s) was added for retrying 13a-v alone. Five rungs
share one session, 13a-v spends part of it on another VT, and the probe's clock
does not stop while the seat is away.

**The shim did not build, and the binary that existed could not run.** A system
upgrade had replaced wlroots 0.19 with 0.20: the meson build failed on a missing
`wlr/render/wlr_renderer.h`, and `ldd shim/build/vitrin-shim` reported
`libwlroots-0.19.so => not found`, so no app could have run under `vitrind` at
all. Resolved by D11's own vendored fallback rather than a port:
`meson setup --reconfigure --force-fallback-for=wlroots-0.19 shim/build shim`,
which builds wlroots 0.19.3 from `subprojects/wlroots.wrap`. **Check this before
a bring-up session**, not during one.

**Three environment observations, none of which stopped the run.**

- `WARN Unable to become drm master, assuming unprivileged mode`, once at device
  open, before libseat hands over. The panel lit regardless.
- `libinput error: client bug: timer event19 hold: scheduled expiry is in the
  past (-1013ms), your system is too slow` — once per VT return. libinput's hold
  timer measured against a clock that did not advance across the pause.
- `WARN Failed to destroy old mode property blob: No such file or directory` —
  once per modeset, five times across the first session.

## 14. Measure the frame cadence

Do not eyeball it. Get a number.

- From the recorder log: the interval between successive presentation entries.
- Or run something with a known cadence and count.
- Also worth measuring while you are here: **the CPU compose cost on the capture
  path.** #218 flags it — `post_dispatch` refreshes `view_cache` from
  `view_rgba` on every dirty round whether or not an agent is observing, and at
  2560x1600 that is a 16 MB compose per latched batch. If it hurts, that is a
  recorded decision, **not** a quiet gating of the refresh on grant state (which
  would make capture freshness depend on whether a grant happened to exist).

Write the number down. A cadence you did not measure is a cadence you did not
observe.

## 15. Shut down cleanly

From the VT `vitrind` is on: `Ctrl+C`. Then `Ctrl+Alt+F1`.

| Expected | Failure | What it means |
|---|---|---|
| `vitrind` exits, the VT returns to a text console, `Ctrl+Alt+F1` restores Hyprland with your windows intact | Hyprland comes back at the wrong resolution or refresh | The panel was left in a mode Hyprland did not re-set. See Recovery R3 |
| The shim and its app exited too | Stray `vitrin-shim` processes | Realm shutdown ordering. `pkill -x vitrin-shim` from tty2, and file it |

## 16. The brightness keys actuate (D-041, issue #303) — NOT YET RUN

**Written before it can be executed, on D-033's own lesson**, and the record
block below is empty on purpose: nothing on this rung has been observed, CI
cannot observe it (no runner has a seat, an ACPI table or a
`/sys/class/backlight` at all), and a temp-directory unit test proves the
bounds and the failure collapse and proves **exactly nothing about a panel**.
Until somebody fills this in, the honest status of the feature is *landed in
the tree, unproven on hardware*.

This rung needs a **second `vitrind`**, because `--backlight` is opt-in and step
6's command line does not carry it. Read the machine first, from your ordinary
terminal — this is read-only and safe from anywhere:

```bash
ls /sys/class/backlight/                         # which devices exist at all
cat /sys/class/backlight/*/max_brightness        # the ceiling, per device
cat /sys/class/backlight/*/brightness            # where it sits now
id -nG | tr ' ' '\n' | grep -x video            # can this uid write it?
```

Two devices (`acpi_video0` **and** `intel_backlight`, say) is the ordinary case
and the reason `--backlight-device` exists: the auto-pick is the sorted-first
device with a readable `max_brightness`, which is deterministic and on a lot of
hardware is the wrong one. The startup log names what it chose.

```bash
# On the VT, with the step-6 command line plus the flag:
vitrind --drm --backlight ... 2>&1 | tee /tmp/vitrind-drm.log
# ...or, if the auto-pick chose a device that does nothing:
vitrind --drm --backlight --backlight-device intel_backlight ... \
        2>&1 | tee /tmp/vitrind-drm.log
```

Then press `XF86MonBrightnessUp` and `XF86MonBrightnessDown` (the Fn row on this
machine), reading `brightness` from a **second VT** between presses — not from
inside the session, which has no terminal you can trust for this.

| Rung | Expected [inferred] | Failure | What it means |
|---|---|---|---|
| 16-i | The startup log carries `brightness keys armed` and names the `device`, its `max` and its `current` | It carries `brightness keys armed but there is nothing to write` | `no_device` = this machine exposes no panel under the fixed root; `not_writable` = this uid is not in `video` and no udev rule tagged the seat. Neither is a bug in this core, and neither refuses the session |
| 16-ii | **Press Up: the panel visibly brightens**, and `brightness` on the other VT has risen by **one step**, which is `max_brightness × 5%` rounded **up** and never less than 1 raw unit. Work it out from the number step 16's first command printed, before you press: 96000 → 4800, 255 → 13, 100 → 5, **15 → 1, 10 → 1**. On a small-`max` device (`acpi_video0` is usually 10–15) a correct build therefore moves by **1 unit, which is 6.7–10% and not 5%** — that is the floor of one raw unit doing its job, not a failure | Nothing changes, log quiet | Read the journal: `grep backlight_stepped` in the recorder gives `no_device`, `unreadable`, `not_writable` or `at_limit`, which are four different machine problems |
| 16-iii | **The panel itself moved, not only the number.** Look at the screen, not at the file | The value changes but the panel does not | The auto-pick chose the wrong device (`acpi_video0` on Intel hardware is the classic). Re-run with `--backlight-device` |
| 16-iv | **Press Down repeatedly: the panel never goes black.** It stops at the floor — `max_brightness × 5%` rounded **up**, at least 1 raw unit, the same number as 16-ii's step — and further presses do nothing | It reaches 0 | The floor failed, and that is the one defect on this rung that is a **safety** defect: a black panel is indistinguishable from a blanked one. Stop and file it |
| 16-v | The app in the realm **does not see the keys**. Run `wev`, `xev` or a nested compositor in the realm and press brightness: nothing arrives | The app receives `XF86MonBrightnessUp` | The gate is not consuming, so two actors share one interface. D-041's consume clause is not met |
| 16-vi | Each press writes **exactly one** `backlight_stepped` entry, with `direction`, `outcome` and `percent` | Several per press, or none | Key repeat reached the core (it does not on `--drm` today), or the drain is not running |
| 16-vii | **Without** `--backlight`, on a session started from step 6's line, pressing brightness does nothing **and the app receives the key** | The key is consumed anyway | The gate armed without the flag, which charges D-041's cost to a session that gets nothing for it |
| 16-viii | On an **external** display: pressing brightness does nothing at all, and that is correct and published | It changes the internal panel's brightness while you look at the external one | Also correct, also published, and worth writing down as the thing a human will find confusing |

**Paste the literal before/after values and the device's `max_brightness` onto
[#303](https://github.com/vitrin-os/vitrin-os/issues/303).** That paste is the
issue's acceptance criterion and nothing else can stand in for it.

**Record block — empty on purpose. Do not fill it in from reasoning.**

```text
16-i     date: ____  device: ____  max: ____  current: ____
16-ii    date: ____  before: ____  after: ____  step expected from max: ____
16-iii   date: ____  panel visibly moved: yes/no
16-iv    date: ____  floor reached: ____  went black: yes/no
16-v     date: ____  app saw the key: yes/no
16-vi    date: ____  entries per press: ____
16-vii   date: ____  app saw the key without the flag: yes/no
16-viii  date: ____  external display: ____
```

---

## 17. Idle inhibition holds the blank, and lets go of it (D-042, issue #306) — NOT YET RUN

**Written before it can be executed, on D-033's own lesson and rung 16's
precedent**, and the record block below is empty on purpose. Nothing on this
rung has been observed. CI cannot observe any of it: blanking needs a display
controller, no runner has one, and
`shim/tests/acceptance/idle_inhibit.sh` is a **component** test against
`shim/tests/mock_core.c` that settles what the *shim sends* and says nothing
about a panel. Until somebody fills this in, the honest status of the feature is
*landed in the tree, unproven on hardware* — and the sentence
`docs/book/src/limits.md` publishes says exactly that.

**This is the one rung on this page whose subject is a thing a human notices
rather than a thing a test finds.** #306 exists because full-screen video blanked
somebody's screen. So the observation is: *watch a video, and see whether the
screen stays on.*

Use a **short** timeout, or the rung takes half an hour:

```bash
# On the VT, with step 6's command line plus a deliberately short blank:
vitrind --drm --blank-idle 60 ... 2>&1 | tee /tmp/vitrind-drm.log
```

Then, inside the realm, play something full-screen in an app that actually asks.
`mpv --fullscreen` does; so does Firefox on a YouTube page. **Check that the app
asked before concluding anything about the core** — the shim logs it:

```bash
grep 'idle: told the core' /tmp/vitrind-drm.log      # the shim's own relay
grep 'idle inhibit'        /tmp/vitrind-drm.log      # the core's edge, per realm
```

An app that never asks makes every row below vacuous, which is the failure mode
this rung is easiest to fool itself with.

| Rung | Expected [inferred] | Failure | What it means |
|---|---|---|---|
| 17-i | The shim logs `idle: told the core state=1` and the core logs `a realm changed its idle inhibit` with `state=Held` within a second of the video starting | Neither line | The app is not asking (try `mpv --fullscreen`, and check `wayland-info` lists `zwp_idle_inhibit_manager_v1`), or the global was not created — the shim logs that too, at `WLR_ERROR` |
| 17-ii | **Watch the screen: it stays lit for well past 60 s with the video playing, and your hands off the keyboard.** This is the whole rung | The panel goes dark under the video | The relay arrived and the guard did not fire, or the realm the output is bound to is not the realm that asked. `grep 'idle inhibit' ` again and compare the realm id with the one on screen |
| 17-iii | **Stop the video (or close the app). Within about 60 s of the last keypress the panel blanks.** The countdown was postponed, not switched off | It never blanks again | A leaked inhibit — the one failure this feature can cause that a human cannot work around except by killing something. Look for `state=0` in the shim log and `idle inhibit dropped` in the core's. **Stop and file it** |
| 17-iv | **`kill -9` the app while the video is playing.** The core logs `idle inhibit dropped` (realm death) or the shim relays `state=0` (client disconnect), and the panel blanks on schedule afterwards | The panel never blanks | Both layers of the leak defence failed at once. This is the case `idle_inhibit.sh`'s scenario (B) covers against the mock, so a green component test with a red rung here means the CORE half is the broken one |
| 17-v | **With `--lock-idle 120` as well: the lock screen comes up at 120 s while the video is still playing and the panel is still lit.** That is correct and published | The session does not lock | A confined app just switched off a security control. That is D-033(1)'s exact prohibition and D-042's central bound. **Stop and file it** |
| 17-vi | **Switch the output to another realm while the video plays.** The panel blanks on schedule — the inhibit stops counting the moment the human looks away, with no message either way | It stays lit | The gate is not on the bound realm, so any background app can pin the panel |
| 17-vii | Without `--blank-idle` at all: an app holding an inhibitor changes nothing, and nothing is logged as an error | An error, or a refusal | The feature has no CLI surface of its own on purpose: with no blank armed, an inhibit is satisfied vacuously |

**Paste what you saw onto
[#306](https://github.com/vitrin-os/vitrin-os/issues/306)** — at minimum 17-ii,
17-iii and 17-v, in words, with the app you used. Nothing else can stand in for
it, and in particular a green `meson test` cannot: it never touches a panel.

**Record block — empty on purpose. Do not fill it in from reasoning.**

```text
17-i    date: ____  app: ____  shim relayed: yes/no  core logged: yes/no
17-ii   date: ____  seconds lit past the timeout: ____  blanked under the video: yes/no
17-iii  date: ____  blanked after stopping: yes/no  seconds: ____
17-iv   date: ____  killed the app: ____  blanked afterwards: yes/no
17-v    date: ____  --lock-idle used: ____  locked on time: yes/no
17-vi   date: ____  switched realms: ____  blanked on schedule: yes/no
17-vii  date: ____  no --blank-idle: nothing happened: yes/no
```

---

## Recovery

Work down this list. Do not skip to R4 because the first two feel slow.

> **The full recovery page is [`docs/book/src/recovery.md`](book/src/recovery.md).**
> R1–R4 below stay here because they are indexed to *this* procedure's steps.
> The book page is the one to read when you are wedged and not mid-bring-up: it
> carries the `sudo`-only `/proc/sysrq-trigger` path, the `logind` settings the
> lid and suspend behaviour depend on, and the **session-lifecycle checklist
> (L1–L7: 10 VT switches, 5 suspend/resume, 5 lid cycles, blank/unblank, the
> blank-did-not-lock check, one deliberate wedge, and a return from another VT
> with the blank armed)** that the checklist below does not contain and never
> did.

### R1 — the desktop is gone but the keyboard works

**`Ctrl+Alt+F1`.** You are back in Hyprland.

**On whether it "still holds master" — H3 and this step used to disagree, and
the resolution is worth knowing before you type.** logind revokes DRM master
when a session's VT stops being active, so switching away *should* take it from
`vitrind` and give it to Hyprland automatically, and `vitrind` should see a
`PauseDevice` and stop drawing. That is the designed path (H3).

It is also the path that WS-E.3.3 exists to test and that nobody has run. If
`vitrind` ignores or mishandles the pause — a live possibility on a first
bring-up — it keeps drawing to a card it no longer owns and the two fight over
the panel. **You cannot tell which happened by looking**, because the symptom of
both is a screen you do not trust.

So do not reason about it: kill it. Get to your tty2 escape shell
(`Ctrl+Alt+F2`) and:

```bash
pgrep -x -a vitrind               # resolve first: -x matches the NAME, not argv
PID=<the number you just read>
kill -INT "$PID"                  # ask it to shut down cleanly first
sleep 2
kill -0 "$PID" 2>/dev/null && echo "still alive" || echo "gone"
kill -KILL "$PID"                 # only if -INT did nothing
pkill -x vitrin-shim              # shims are children of vitrind but check anyway
```

Resolve, then signal a number. A `pkill -f` pattern for this process signals the
rescuer and misses the target — step 0 and
[the recovery runbook](book/src/recovery.md#route-2--a-shell-somewhere-else-and-a-signal)
carry the full account (#260).

`-INT` before `-KILL` matters: a clean exit runs `shutdown_realm` and drops
master in order. `-KILL` leaves the kernel to reclaim master, which usually
works and occasionally leaves the panel in the state described in R3.

### R2 — the screen is black **and** the keyboard is dead

This is the case that has no good answer here, and the reason the cost of the
escape-route substitution is stated at the top of this page.

1. **Try `Ctrl+Alt+F1` anyway.** The keyboard may only *look* dead because
   nothing is drawing. Give it a few seconds — a mode set is not instant.
2. **Do not reach for `Alt+SysRq`.** With the mask at `16` every letter except
   `s` is inert from the keyboard (step 0.4), and that is not being changed.
   [verified 2026-08-10]
3. **If you can still reach a shell anywhere — a VT, or the Hyprland session on
   tty1 — you are not in R2**, you are in R1, and the `sudo`-only
   `/proc/sysrq-trigger` path in
   [the recovery runbook](book/src/recovery.md#route-3--sysrq-through-procsysrq-trigger-sudo-only)
   is how you bring the machine down safely without the power button. That path
   needs a shell by construction, which is exactly why it does not help in the
   case R2 is written for.
4. **Hold the power button.** With no SSH and no shell, this is where the
   substitution lands.
   State it plainly: **this is a reboot, where #220's design assumed it was a
   command.**
5. If the machine does not come back up cleanly, that is what the installer USB
   in step 0.3 is for: boot it, mount, `arch-chroot`, undo whatever wedged it.

### R3 — the panel is left in a bad mode

Symptoms: Hyprland returns at a wrong resolution or refresh, or the panel is
dark but the machine is clearly alive (you can type blind, log in, and
`loginctl` responds).

**`hyprctl` will not work from the console you are on.** It needs
`HYPRLAND_INSTANCE_SIGNATURE`, which only exists inside the Hyprland session's
own environment — and the state R3 is written for is exactly the one where you
are on a fresh VT or a blind login and do *not* have it. An earlier draft of
this section told you to run it anyway, which would have failed with an
unhelpful error at the worst moment.

Import it from the running Hyprland process first:

```bash
# From your tty2 escape shell or a blind login.
# 1. Find Hyprland and borrow its environment.
HYPR_PID=$(pgrep -u "$USER" -x Hyprland | head -1)
export HYPRLAND_INSTANCE_SIGNATURE=$(
  tr '\0' '\n' < /proc/$HYPR_PID/environ | sed -n 's/^HYPRLAND_INSTANCE_SIGNATURE=//p')
export XDG_RUNTIME_DIR=/run/user/$(id -u)

# 2. Now hyprctl can reach it.
hyprctl monitors                 # what Hyprland thinks it has
hyprctl keyword monitor eDP-1,2560x1600@240,0x0,1
```

If `pgrep` finds no Hyprland, it is gone rather than confused — and on this
machine it is **not** a systemd unit (it starts from a tty login shell via
`/usr/bin/start-hyprland`), so `systemctl --user restart hyprland` will fail.
Log in on tty1 and start it the way you normally do, or reboot.

If nothing re-sets the mode, a **reboot** clears it: nothing about a DRM mode
survives one. A wedged mode is annoying, not persistent — do not go to the
installer USB for this.

### R4 — `vitrind` is dead but nothing will take the panel back

Usually a leaked master fd on a process that has not fully exited. Check for
zombies and stray shims first (`pgrep -a vitrind; pgrep -a vitrin-shim`), then
reboot. There is no way to force-release DRM master from userspace and there is
no point pretending otherwise.

### What none of this covers

**A kernel-side wedge in i915.** If the GPU hangs, none of the above applies —
you get a hard freeze, possibly with `i915` messages in the journal you can read
after the reboot (`journalctl -b -1 -k -g i915`). That is a driver bug, not a
`vitrind` bug, and the only response is to record the trace and the mode that
produced it.

---

## Record the run

Date it, and record the environment, exactly as
[`shim/docs/nested-lock-screen.md`](../shim/docs/nested-lock-screen.md) and
`shim/docs/firefox.md` do. The value of a manual runbook is entirely in whether
anyone can tell when it was last actually executed.

### Second run — 2026-08-09, after the first run's three fixes

```text
Executed:      2026-08-09, by Taha, target laptop, RELEASE build.
Purpose:       confirm the three fixes, and close checklist items 10 and 11,
               which the first run left NOT TESTED.
Connector:     EmbeddedDisplayPort (smithay logs it; vitrind's own line was
               again recorded as rendering it empty -- first run's finding 4,
               still open, but RE-DIAGNOSED 2026-08-11: the code cannot produce
               an empty name and nothing in the log stack drops it, so the
               observation stands with no explanation. No copy of the literal
               line was kept from this run either. See finding 4, #250)

  9.  Trusted band ................... PASS -- band at the TOP. The mirror is
                                       fixed (SCANOUT_TRANSFORM = Normal)
  10. Consent prompt + physical click . PASS -- FIRST TIME EVER. Card drawn on
                                       the panel, clicked with the mouse,
                                       petition granted, 13 captures served
  11. Held-Esc revocation ............ PASS -- FIRST TIME EVER against a LIVE
                                       grant: dead_man_triggered held_ms=1005
                                       revoked=1, grant_revoked in the SAME
                                       millisecond, agent refused and voiced
                                       128 ms later. The first run's revoked=0
                                       proved only reachability
  12. VT switch away and back ........ PASS -- 5 chorded switches honoured,
                                       plus 4 `vt_switch_refused already_here`
                                       when the human chorded the VT they were
                                       already on. Two full pause->activate
                                       cycles: master dropped and reclaimed
  14. Frame cadence .................. 32.9% CPU (release) against 99.1%
                                       (debug) for the same client and mode.
                                       ~3x of the first run's number was the
                                       build profile

Not on the checklist, and confirmed by accident:
      D-030(4) WORKS ON HARDWARE. The observer was launched from Hyprland, so
      the petition arrived while the seat was elsewhere: consent_transition
      went `queued` at 23:27:51 and only `shown` at 23:27:57, when the human
      switched back. A prompt was NOT journalled as shown to a human who could
      not see it -- which is the entire falsehood that decision exists to
      prevent, exercised in a test aimed at something else.

      The capture path is byte-correct: centre pixel read #55aa00ff for a realm
      configured 00aa55, which is exactly xrgb8888 little-endian.

Still open after this run:
      - vitrind's own log line was observed rendering the connector name empty,
        on both runs, and this is UNEXPLAINED. `connector_name` and the fmt
        visitor were ruled out as the cause on 2026-08-11 (#250) -- the code
        cannot produce an empty name -- so what the operator saw has no
        diagnosis. Neither run kept the literal line; step 7 now requires it
      - the shim never emits dmabuf, so the zero-copy scanout path is dead code
        against every real app
      - refresh_view_cache composes for absent consumers
```

```text
Last executed: 2026-08-09, by Taha, on the target laptop (Monster).
               FIRST EXECUTION. The backend had never been run by anyone.
Kernel:        7.1.5-arch1-2
Mesa:          1:26.1.6-1
Connector:     (recorded as logged EMPTY -- see finding 4 below, RE-DIAGNOSED
               2026-08-11: `connector_name` and the fmt visitor are both ruled
               out as the cause, the observation is kept and unexplained, and
               no copy of the literal line was preserved. The panel is the
               laptop's internal eDP and mode selection worked)
Selected mode: 2560x1600 @ 240 Hz
Measured frame cadence: not measured in fps. What WAS measured: `vitrind` at
               99.1% CPU (ps), with the shim forwarding continuous full-frame
               2560x1600 stride=10240 buffers. Run lasted 471 s.
Card opened:   card1 (the iGPU, chosen automatically by udev::primary_gpu --
               hazard H1 did not materialise)

Observation checklist -- pass/fail and what was seen:
  7.  Connector and mode ............. PASS (2560x1600@240 set on card1);
                                       connector NAME recorded as logged empty
                                       -- observation kept, cause re-diagnosed
                                       and unexplained (finding 4, #250). The
                                       PASS itself is sound: `connector_name`
                                       is byte-identical at cf0e7ff, the
                                       commit this run was built from, and it
                                       cannot render an empty name
  8.  App maps and repaints .......... PASS (solid-client's green square drew)
  9.  Trusted band ................... PASS but MIRRORED -- band drawn along the
                                       BOTTOM edge. The band's fixed position is
                                       what made the flip legible; a uniform
                                       green square alone would have hidden it
  10. Consent prompt + physical click . NOT TESTED (no agent connected this run)
  11. Held-Esc revocation ............ NOT TESTED
  12. VT switch away and back ........ **FAIL -- IMPOSSIBLE.** Ctrl+Alt+F1 and
                                       Ctrl+Alt+F2 did nothing. See finding 2
  13. Type a letter .................. PASS -- the Turkish passphrase was typed
                                       into the lock screen and accepted, so the
                                       compiled keymap resolves letters
                                       (recorder: unlock_attempted accepted=true)
  14. Frame cadence (number) ......... not captured; 99.1% CPU instead
  15. Clean shutdown ................. PASS (SIGTERM -> run_ended, no stray
                                       vitrind/vitrin-shim/solid-client left)

  Also observed, not on the checklist:
      88 seat_delivered events -- libinput routed real mouse and keyboard input
      session_locked(chord) -> unlock_attempted(true) -> session_unlocked: the
      lock screen works on bare metal, including the passphrase path

Recovery paths actually used: a SIGTERM to vitrind from a shell in the
      still-running Hyprland session on tty1. VT switching was attempted first,
      as this page instructed, and did not work. Hyprland and three other
      working sessions were completely undisturbed throughout.
      (The exact command typed that day was `pkill -TERM -f "vitrind --drm"`,
      and the session did end. It is NOT the published form any more and must
      not be copied from this record -- #260, 2026-08-11: that form signals the
      rescuing shell, and against a command line produced by the
      ~/.local/bin/vitrind wrapper it does not match vitrind at all. Whether it
      matched vitrind on 2026-08-09 or only the shell is not established by
      anything kept from that run. Resolve the PID and signal it; see step 0.)

Notes: four findings, in severity order --
  1. The image is VERTICALLY MIRRORED (SCANOUT_TRANSFORM = Flipped180).
  2. The human COULD NOT LEAVE. D-030(1) refused to implement change_vt and
     pinned the refusal with a test; once vitrind holds the display the kernel
     stops handling Ctrl+Alt+F<n>. The decision written to keep the escape
     hatch open is what welded it shut. This page's own first line of defence
     did not exist.
  3. 99.1% CPU, laggy cursor, audible fan. solid-client uses wl_shm, so
     zero-copy scanout can never engage and every frame is a full 2560x1600 CPU
     composite -- at 240 Hz that is ~3.9 GB/s, which is precisely the cost
     #218's decision 1 calculated in advance and asked to be measured here.
     How much is the test client and how much is real is being investigated.
  4. `connector=` logs empty. **RE-DIAGNOSED 2026-08-11 (#250): the cause
     this finding named does not exist. The observation is kept and is now
     unexplained.** `connector_name` cannot return a name-less string. It is
     `format!("{}-{}", info.interface().as_str(), info.interface_id())`, and
     `Interface::as_str` in the pinned `drm 0.14.1` is a total match over all
     21 variants returning a non-empty `&'static str` for every one -- `"eDP"`
     for `EmbeddedDisplayPort`, `"Unknown"` for `Unknown`. The floor of that
     `format!` is `"Unknown-0"`; neither `""` nor a bare `"-1"` is reachable,
     so neither "the interface kind formatted empty" nor "the connector id was
     missing" survives. Nothing between it and stderr drops the value either:
     `init_tracing` installs the stock `tracing_subscriber::fmt()` with no
     custom formatter, layer or visitor, `backend/drm.rs` uses the ordinary
     `tracing` macros, and there is no shadowing macro anywhere in `crates/`.
     What was on the operator's screen is therefore NOT explained by this
     code, and neither run kept a copy of the literal line, so it cannot be
     recovered now. One adjacent obstacle to reading it back is filed as #251
     (`vitrind` writes ANSI escapes into a redirected stderr, so `grep
     'connector='` over the file step 6 tees matches zero lines) -- that is an
     obstacle, not an established cause of what was seen. Step 7 now requires
     the literal `mode set:` line to be pasted into the record so a third run
     can be argued about from bytes.
```

> **This runbook HAS been executed: 2026-08-09, on the target machine, results
> recorded above.** #220's acceptance criterion — "executed end to end on the
> target machine and the results recorded: date, kernel version, mesa version,
> connector, selected mode, observed frame cadence, and each checklist
> observation as pass/fail with what was seen" — is met on every field except
> **frame cadence**, which was not captured in fps; 99.1% CPU was measured
> instead, and finding 3 is why that is the more useful number.
>
> **It was not a clean pass, and that is the point.** Three defects came out of
> it, and every one was found by a human looking at a panel — not by 12 green CI
> checks, not by 855 unit tests, not by two rounds of adversarial review. One of
> them is that **this page's own first line of defence did not exist**, which no
> amount of re-reading the page could have revealed.
>
> The first execution is by construction the dangerous one: it takes DRM master
> on the maintainer's own laptop, from a physical keyboard, with a human present
> to use the escape route. That is exactly the class of evidence CI cannot
> produce and an agent must not fake — the same split #212, #214 and #232 wrote
> down for physical input. It was run that way, and the escape route it needed
> turned out to be the one this page had not listed.
>
> **Checklist items 10 and 11 were not tested.** No agent was connected, so
> neither the consent prompt on bare metal nor held-Esc revocation has ever run
> against a display controller. Those are the content of the next run, together
> with re-checking 9 and 12 once the fixes land.