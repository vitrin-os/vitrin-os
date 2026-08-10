# Getting out of a wedged session

`vitrind --drm` takes DRM master and the seat. When it is running, it *is* the
display: nothing else can put a pixel on the panel, and the kernel has stopped
handling the chord you would normally use to leave. So the ordinary question a
display server never has to answer becomes the first one this one must — **how
do you get out when it stops responding?**

This page is the answer, written for the one machine this project is developed
on, because a generic recovery page is one you have to translate at the worst
possible moment. It is a companion to
[`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md),
which is the bring-up procedure; that page's **step 0** is where the escape
route is *proven before you start*, and this page is what you read when you
need it. Where the two overlap, step 0 is the source and this page links to it
rather than restating it.

> **The honest frame, first.** Every route below needs something to still be
> working: a keyboard, or a shell, or the ability to power-cycle and boot a USB
> stick. **There is no route that works when all of those are gone.** The
> project declined to run an SSH server on this machine
> ([D-031, the first of two entries with that number](https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md)),
> which is the one mitigation that would have been independent of the display,
> the seat and the local keyboard. That trade was made knowingly and its cost
> is exactly this paragraph.

## Verified vs inferred

Same convention as the bring-up page, for the same reason — a recovery page
that reads as tested when it is not is worse than one that admits it:

- **[verified]** — read from this machine on **2026-08-10**, read-only. No VT
  was switched, no `vitrind` was started, no destructive SysRq letter was
  executed, and no power state was touched.
- **[inferred]** — from the kernel's own source or documentation, or from a
  configuration that is in place but whose *behaviour* was not exercised here.

**No route on this page has been executed end to end.** Route 2 is the only one
that has ever actually recovered a session, on 2026-08-09, and it is recorded on
the bring-up page. Treat everything else as a careful prediction until the
checklist at the bottom is filled in.

## Which route, by symptom

Work down. Do not skip to a later route because an earlier one feels slow — the
later ones cost you more, and the last one costs you the machine's uptime.

| What you are looking at | Route |
|---|---|
| Panel wrong or frozen, **keyboard works** | [1 — `Ctrl-Alt-F<n>`](#route-1--ctrl-alt-fn-which-this-core-implements-itself) |
| Panel dark or wrong, **you have a shell somewhere** (another VT, or the Hyprland session on tty1) | [2 — a shell and a signal](#route-2--a-shell-somewhere-else-and-a-signal) |
| `vitrind` will not die, or it died and the machine is still stuck; **you have a shell with `sudo`** | [3 — SysRq through `/proc/sysrq-trigger`](#route-3--sysrq-through-procsysrq-trigger-sudo-only) |
| Nothing responds at all | [4 — power cycle, then the installer USB](#route-4--the-installer-usb-and-a-chroot) |

## Route 1 — `Ctrl-Alt-F<n>`, which this core implements itself

Press **`Ctrl-Alt-F1`** to get back to the Hyprland session on tty1, or
`Ctrl-Alt-F2`…`F12` for another terminal.

This works **only because the core implements it**. Once a process holds DRM
master and the VT is in graphics mode the kernel stops handling that chord, so a
compositor either implements it or abolishes it — there is no third option, and
the first bare-metal run of this backend proved it the hard way by not
implementing it and trapping the maintainer on tty3
([D-031, the second entry with that number](https://github.com/vitrin-os/vitrin-os/blob/main/docs/plan/20-decision-log.md)).

Four things worth knowing before you rely on it:

- **It works while the screen is locked and while a consent prompt is up.**
  Deliberate: being trapped is worst in the state where you cannot dismiss what
  is in front of you. It is never a way *past* the lock — the lock is still up,
  and still wants your passphrase, when you come back.
- **Know your own VT number before you start.** The startup banner logs it. A
  human who can leave and cannot come back is only half rescued.
- **If the switch fails you will see it on the panel**, in a red band naming
  what happened — `crate::notice::CoreNotice`, added precisely because a log
  line is worth nothing to somebody who cannot leave the screen to read it. The
  flight recorder carries `vt_switch_requested`, `vt_switch_refused` and
  `vt_switch_stalled`. [verified: `crates/vitrin-core/src/recorder.rs`]
- **If that red band appears, this route is gone.** Go to route 2.

## Route 2 — a shell somewhere else, and a signal

**This is the only route that has ever actually recovered a session.** On
2026-08-09 the first bare-metal run wedged with no working VT chord, and what
freed it was a terminal in the *still-running Hyprland session on tty1*:

```bash
pkill -TERM -f "vitrind --drm"
```

The property this depends on is that a `vitrind` session on tty3 does not
disturb Hyprland on tty1 — so a terminal there, or an agent session running in
one, still reaches the machine. Leave one open before you start anything. See
[bring-up step 0.1](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md).

Escalate within the route rather than jumping out of it:

```bash
pkill -INT vitrind          # ask for a clean shutdown first
sleep 2
pgrep -a vitrind || echo "gone"
pkill -KILL vitrind         # only if -INT did nothing
pkill vitrin-shim           # shims are children of vitrind; check anyway
```

`-INT` before `-KILL` matters: a clean exit runs the realm shutdown ladder and
drops DRM master in order. `-KILL` leaves the kernel to reclaim master, which
usually works and occasionally leaves the panel in a bad mode — see the
bring-up page's recovery section R3 for that case.

**`wayvnc` is not this route and must not be reached for.** It runs *through*
Hyprland as a `wlr-screencopy` client, so the moment `vitrind` takes master and
Hyprland goes inactive it has nothing to capture and no compositor to talk to.
It is named here so that you do not count on it. [verified 2026-08-09, recorded
on the bring-up page]

## Route 3 — SysRq through `/proc/sysrq-trigger`, sudo only

This route brings the machine down safely without the power button, when
`vitrind` will not die, or when the ordinary shutdown path is itself stuck.
**It is not a way to un-wedge the display; it is a way to end the session
without corrupting the filesystem.**

> **Read the caveat before the commands. This path needs a reachable shell.**
> It covers *"`vitrind` wedged the display"* once a VT or the Hyprland-side
> shell is reachable. It does **not** cover *"input is completely dead"* —
> which is the only case the physical `Alt+SysRq` combo would have covered, and
> which is not available here (below). That trade is made knowingly. If you have
> no shell, this route does not exist for you; go to route 4.

### The keyboard combo is not available here, and the mask is not being changed

`/proc/sys/kernel/sysrq` is **`16`** on this machine, set by
`/usr/lib/sysctl.d/50-default.conf:19`. [verified 2026-08-10] `16` is
`0x10` — *enable sync command* and nothing else. So from the physical keyboard
`Alt+SysRq+s` works and **every other letter does not**: no `r` (unraw), no
`e`/`i` (signal processes), no `u` (remount read-only), no `b` (reboot). The
physical REISUB sequence is inert here by configuration.

**That configuration stands, and raising it is not on this page.** Handing
`REISUB` to anyone at the physical keyboard of a machine whose entire premise is
confining what runs on it is a trade this project declines. **An earlier version
of the bring-up page recommended raising the mask as an optional pre-step; that
recommendation has been deleted rather than left standing.**

### Why the trigger file works anyway

The mask gates the *keyboard* path only. From the kernel's own documentation,
verbatim:

> Note that the value of `/proc/sys/kernel/sysrq` influences only the invocation
> via a keyboard. Invocation of any operation via `/proc/sysrq-trigger` is
> always allowed (by a user with admin privileges).

— [`Documentation/admin-guide/sysrq.rst`](https://docs.kernel.org/admin-guide/sysrq.html),
read 2026-08-10. [verified]

The mechanism behind that sentence, so it is not taken on trust: the file's
write handler calls `__handle_sysrq(c, false)`, and that second argument is
`check_mask` — the trigger path asks the kernel *not* to consult the bitmask.
[verified against `drivers/tty/sysrq.c::write_sysrq_trigger`, mainline, read
2026-08-10]

`/proc/sysrq-trigger` is `--w------- root root` on this machine. [verified
2026-08-10] So the capability is the root user's alone, which is the whole
point: it is reachable by `sudo` and by nothing at the keyboard.

### The sequence — and it is deliberately not REISUB

**Do not write `_reisub`.** The kernel's own documentation offers
`echo _reisub > /proc/sysrq-trigger` as its bulk-mode example, and on this
machine it would be **a hard reboot with two no-ops in front of it.** Two
independent findings, both checked against the kernel source on 2026-08-10:

1. **`s` and `u` do not do the work; they queue it.** `sysrq_handle_sync` calls
   `emergency_sync()`, which is `schedule_work(do_sync_work)` and **returns
   immediately**; `sysrq_handle_mountro` calls `emergency_remount()`, which is
   `schedule_work(do_emergency_remount)` and returns immediately.
   [verified: `fs/sync.c`, `fs/super.c`] `sysrq_handle_reboot` calls
   `emergency_restart()`, which does not return at all.
2. **Bulk mode runs the whole string inside one `write()`**, with no pause
   between letters — that is exactly what the leading `_` buys. [verified:
   `write_sysrq_trigger` sets `bulk = true` on `_` and loops the buffer]

Put together: `_reisub` queues a sync, queues a remount, and then reboots before
either queued job can run. The kernel documentation says the same thing in its
own words about the sync — *"the sync hasn't taken place until you see the "OK"
and "Done" appear on the screen"* — and bulk mode is precisely the form that
gives you no chance to see them.

**And `e` and `i` are wrong for this machine anyway.** `e` sends `SIGTERM` to
every process except init and `i` sends `SIGKILL`. On this machine the escape
route *is* a shell in the Hyprland session, and that session is the maintainer's
real work — so `e` destroys the thing you are recovering with, along with
everything you have open. Their purpose in the keyboard sequence is to get
processes out of the way when you have no shell; here you have one, and
`pkill` (route 2) is the aimed version of the same idea.

So the correct procedure is **three separate writes, waiting between them**:

```bash
# In a second shell, so you can see the kernel's own completion messages.
# kernel.dmesg_restrict = 1 here, so this needs root. [verified 2026-08-10]
sudo dmesg -w
```

```bash
# STEP 1 — SAFE. Flush the page cache to disk.
printf 's' | sudo tee /proc/sysrq-trigger
#   WAIT for "Emergency Sync complete" in the dmesg -w window before continuing.
```

```bash
# STEP 2 — *** DESTRUCTIVE: every filesystem becomes read-only. ***
# Nothing on this machine can write to disk afterwards. Do not run this and
# then decide to keep working.
printf 'u' | sudo tee /proc/sysrq-trigger
#   WAIT for "Emergency Remount complete" before continuing.
```

```bash
# STEP 3 — *** DESTRUCTIVE: reboots the machine immediately, no unmount. ***
# Everything unsaved that steps 1 and 2 did not reach is gone.
printf 'b' | sudo tee /proc/sysrq-trigger
```

**Step 2 does not strand step 3.** The remount only touches superblocks with a
backing block device — `do_emergency_remount_callback` tests `sb->s_bdev` before
it does anything — and `/proc` has none, so the trigger file is still writable
after every real filesystem has gone read-only, and `sudo dmesg -w` still reads.
[verified: `fs/super.c`, read 2026-08-10] This is worth knowing in advance,
because hesitating between steps 2 and 3 is exactly what a read-only filesystem
invites.

One optional, **non-destructive** rung, useful only in a narrow case:

```bash
# SAFE. Console keyboard is dead after vitrind died: put it back in XLATE mode.
printf 'r' | sudo tee /proc/sysrq-trigger
```

`r` addresses the console **keyboard** mode only — the kernel documents it as
*"Turns off keyboard raw mode and sets it to XLATE"*. It does **not** put the VT
back into text mode, so a console left in graphics mode stays blank whatever `r`
does. Only meaningful once `vitrind` is no longer running. [inferred: the letter's
documented behaviour is verified; that it helps in this specific case is not]

### Two traps, one of them tested here

**The redirect happens in the shell you are already in.** `echo s >
/proc/sysrq-trigger` fails as an unprivileged user, and `sudo echo s >
/proc/sysrq-trigger` fails identically, because `sudo` applies to `echo` and not
to the `>`. It must be `| sudo tee` (above) or `sudo sh -c '...'`.

Tested here, non-destructively: the unprivileged redirect returns
`permission denied` and exits 1, writing nothing. [verified 2026-08-10]

**You will probably not see the kernel's messages on the console.**
`kernel.printk` is `1 4 1 4` on this machine [verified 2026-08-10], so the
console log level is **1** — only `KERN_EMERG` reaches the screen, and the
completion messages are below that. The kernel documentation's advice to *wait
until you see them on the screen* therefore does not work here as configured.
`sudo dmesg -w` in a second shell is the observable, which is another reason
this route needs a shell rather than a keyboard.

### What was not executed, and why

**The `sudo` write path has never been run on this machine.** `s` is the one
harmless letter and it is the only one permitted to be exercised, but
`sudo -n true` fails here — a password is required, and the session that wrote
this page could not supply one. [verified 2026-08-10] So step 1 above is
**documented and unexecuted**.

**Run step 1 yourself, once, while nothing is wrong.** It is a sync; it costs a
second and it is the only way to find out that `tee` typo before the day you
need it. A recovery path you have never used is a plan. That is what bring-up
step 0.1b already says about the VT chord, and it applies here unchanged.

## Route 4 — the installer USB and a chroot

Real, and slower by orders of magnitude: physical access, a hard power cycle,
boot the Arch USB, `mount` the root (and `/boot`, and unlock LUKS if the disk is
encrypted), `arch-chroot`, undo whatever wedged it, `exit`, reboot. **Minutes to
tens of minutes**, against seconds for a console command.

Have the USB physically in the room before you start. If it is in a drawer
somewhere else, you do not have a fourth route; you have an errand. See
[bring-up step 0.3](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md).

Note that a DRM mode does not survive a reboot — a panel left in a bad mode is
annoying rather than persistent, and is not a reason to reach for the USB.

## What none of this covers

- **Input completely dead.** The one case the physical `Alt+SysRq` combo would
  have covered, and it is not available here (route 3). Every route above needs
  either a keyboard or a shell.
- **A kernel-side wedge in i915.** If the GPU hangs, none of this applies: you
  get a hard freeze, and the only response is a power cycle and reading
  `journalctl -b -1 -k -g i915` afterwards. That is a driver bug, not a
  `vitrind` bug.
- **A dark screen that is not a wedge.** As of this release `vitrind` can turn
  the panel off on an idle timer, and a blanked session looks exactly like a
  dead one. **Press a key before you conclude anything** — any physical input
  wakes it. If the panel does not come back within a couple of seconds, treat it
  as a wedge and start at route 1. This is a new confusion that did not exist
  before the blank, and it is published as a limit rather than left to be
  discovered:
  [Where this is honest about its limits](limits.md).

## The settings this depends on, which this repository does not own

Suspend, lid and power-key policy is **systemd-logind's**, not `vitrind`'s —
that is a deliberate decision, not an omission, because reimplementing it would
put session policy inside the trusted core. The consequence is that *"suspend
works"* is not reproducible from a checkout of this repository alone. So the
values are published here, **read from the running logind on 2026-08-10** rather
than transcribed from a config file:

| Property | Value on this machine | |
|---|---|---|
| `HandlePowerKey` | `poweroff` | [verified] |
| `HandlePowerKeyLongPress` | `ignore` | [verified] |
| `HandleSuspendKey` | `suspend` | [verified] |
| `HandleHibernateKey` | `hibernate` | [verified] |
| `HandleLidSwitch` | `suspend` | [verified] |
| `HandleLidSwitchExternalPower` | *unset* — falls through to `HandleLidSwitch` | [verified] |
| `HandleLidSwitchDocked` | `ignore` | [verified] |
| `IdleAction` / `IdleActionSec` | `ignore` / 1800 s | [verified] |
| `InhibitDelayMaxSec` | 5 s | [verified] |
| `BlockInhibited` | *(empty)* | [verified] |
| `DelayInhibited` | `shutdown:sleep` | [verified] |

Read with:

```bash
busctl get-property org.freedesktop.login1 /org/freedesktop/login1 \
  org.freedesktop.login1.Manager \
  HandlePowerKey HandleSuspendKey HandleLidSwitch \
  HandleLidSwitchDocked HandleLidSwitchExternalPower IdleAction
systemd-inhibit --list
```

Four things that matter more than the table:

- **These are the machine's, not the project's.** `/etc/systemd/logind.conf`
  contains a bare `[Login]` header and nothing else on this machine [verified
  2026-08-10], so every value above is systemd's compiled-in default (systemd
  261) rather than a choice anyone made. **They can change without any change
  to this repository**, and a run recorded under different values is a run of a
  different system. Record them with your results.
- **Nothing currently blocks logind's handling.** `BlockInhibited` is empty, so
  no application has taken a `handle-lid-switch`, `handle-power-key` or
  `handle-suspend-key` lock. If one ever does, the lid stops suspending the
  machine and nothing in `vitrind` will tell you why — `systemd-inhibit --list`
  is where that shows up.
- **Six `delay` inhibitors are held** (NetworkManager, rtkit, UPower, and two
  desktop applications) [verified 2026-08-10], bounded by
  `InhibitDelayMaxSec = 5 s`. So a suspend on this machine is delayed by up to
  five seconds and then proceeds. If a resume looks late, that is where the
  first five seconds went.
- **The lid is `SW_LID` on `event0`** [verified 2026-08-10, from
  `/proc/bus/input/devices`], and `vitrind` sees it and **drops it at intake** —
  switch events have no wire event, which `crate::input::intake_physical`'s own
  doc comment states. So closing the lid produces no `vitrind` behaviour at all;
  everything you observe is logind's.

## The hardware checklist

**CI structurally cannot test any of this.** A GitHub runner has no seat, no VT,
no DRM device, no ACPI and no backlight. That is stated rather than dressed up
as a criterion, and it is why this checklist exists.

Run it after a `vitrind --drm` session is up per the bring-up page, with the
Hyprland-side shell of step 0.1 open the whole time. Rungs are numbered `L1`–`L6`
so they do not collide with the bring-up page's own 1–15.

| # | Do | Expect | Worst credible failure |
|---|---|---|---|
| L1 | **10 VT switches away and back.** `Ctrl-Alt-F2`, wait, `Ctrl-Alt-F3` back. Ten times. | Session survives every one; band the same colour each return; recorder shows paired pause/activate | A dead session, or a black panel with `vitrind` alive (master not reacquired) |
| L2 | **5 suspend/resume cycles.** `systemctl suspend` from the escape shell. | Machine sleeps, wakes, panel comes back, apps still there | Panel never returns; or apps frozen because no frame clock restarted |
| L3 | **5 lid close/open cycles.** | logind suspends on close, resumes on open, panel returns | Lid does nothing (check `systemd-inhibit --list`); or resume leaves a black panel |
| L4 | **Blank and unblank.** Start with `--blank-idle`, leave the machine alone past the timeout, then press a key. | Panel goes dark; any physical input brings it back | Panel dark and input swallowed — this is indistinguishable from a wedge, go to route 1 |
| L5 | **Confirm the blank did not lock.** After L4's wake, look at what is on screen. | The session as you left it — **not** a lock card | A lock card, which would mean idle-blank and idle-lock got coupled |
| L6 | **One deliberate wedge, recovered by a documented route.** Cause one on purpose and write down which route got you out. | Route 1 or 2 recovers it | Neither does; record how far down the table you had to go |

Two rungs deserve their own warnings.

**L2 and L3 have never been exercised by anyone on this backend.** Not once, in
any form. They are the newest rungs on this page and the most likely to find
something. Do them with the escape shell open and with nothing unsaved.

**L4 can produce exactly the symptom this whole page is about.** Unblanking is a
full modeset, and a modeset that fails leaves a dark panel with the session
running. If L4 does not come back, you are in route 1 — and that is a result to
record, not a mishap.

### The numbers this checklist owes

Issue #223 stays open until these are pasted into it. **No hardware criterion is
claimed as met by the change that wrote this page**, and nothing in this
repository should be read as claiming otherwise:

- L1: how many of 10 switches survived, and the band colour on each return.
- L2: how many of 5 suspend/resume cycles came back with a working panel, and
  how long the resume took.
- L3: how many of 5 lid cycles behaved as L2 did.
- L4: the measured blank latency and the measured wake latency.
- L6: which route recovered the wedge, and how long it took.

## Record the run

Date it and record the environment, the same shape
[`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md)
uses. The value of a manual runbook is entirely in whether anyone can tell when
it was last actually executed.

```text
Executed:      NOT YET EXECUTED.
By:
Kernel:
systemd:
logind values at the time of the run (read them again, do not copy the table
above -- they are defaults today and a default can move under you):
    HandleLidSwitch=            HandlePowerKey=
    HandleSuspendKey=           IdleAction=
    BlockInhibited=             DelayInhibited=

  L1. 10 VT switches ............. __/10 survived; band colour stable? Y/N
  L2. 5 suspend/resume ........... __/5  panel returned; resume latency ___ ms
  L3. 5 lid close/open ........... __/5  behaved as L2
  L4. Blank and unblank .......... blank after ___ s; wake latency ___ ms
  L5. Blank did not lock ......... PASS / FAIL (what was on screen: ______)
  L6. Deliberate wedge ........... recovered by route ___ in ___ s

SysRq step 1 (`printf 's' | sudo tee /proc/sysrq-trigger`) executed? Y/N
    (documented and unexecuted as of 2026-08-10 -- see route 3)

Anything that happened that is not a row above:

Findings, in severity order:
```

> **This page has not been executed.** Every route except route 2 is a careful
> prediction, and route 2's single execution was on 2026-08-09 against a
> different defect. Read it that way, correct it from your own eyes, and treat a
> failed observation as a result worth recording rather than a step to retry
> until it passes.
