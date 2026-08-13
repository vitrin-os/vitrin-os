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

- **[verified]** — read from this machine on **2026-08-10**, read-only (no VT
  switched, no `vitrind` started, no destructive SysRq letter executed, no power
  state touched), or **observed on hardware during the 2026-08-11 run** recorded
  at the bottom of this page.
- **[inferred]** — from the kernel's own source or documentation, or from a
  configuration that is in place but whose *behaviour* was not exercised here.

**Route 2 is the only route that has ever recovered a wedged session** — on
2026-08-09, and again on 2026-08-13. Both times the command this page published
was **wrong for the wedge in front of it**, in two different ways, so read the
route itself before you rely on it: the `pkill -f` form was broken three ways
([#260](https://github.com/vitrin-os/vitrin-os/issues/260)) and the `kill -TERM`
that replaced it is inert against a stopped process
([#277](https://github.com/vitrin-os/vitrin-os/issues/277)). What actually
recovered the 2026-08-13 wedge was **`kill -CONT`**, and it recovered it in the
next logged millisecond.

Route 1's chord is confirmed to work **from a healthy session** (10 of 10 on
2026-08-11, L1 below; 5 more on 2026-08-09). That is a different claim from *it
gets you out of a wedge*, and 2026-08-13 sharpened how it fails: against a
`SIGSTOP`ped session the chord is **not refused and not lost — it is queued**,
and it completes the instant the session resumes. For 163.8 s the panel showed
the *previous* VT while the kernel already considered the target VT active.

**Route 3 is documented and unexecuted** and **route 4 has never been used.**
Treat both as careful predictions.

## Which route, by symptom

Work down. Do not skip to a later route because an earlier one feels slow — the
later ones cost you more, and the last one costs you the machine's uptime.

| What you are looking at | Route |
|---|---|
| Panel wrong or frozen, **keyboard works** | [1 — `Ctrl-Alt-F<n>`](#route-1--ctrl-alt-fn-which-this-core-implements-itself) |
| Panel dark or wrong, **you have a shell somewhere** (another VT, or the Hyprland session on tty1) | [2 — a shell and a signal](#route-2--a-shell-somewhere-else-and-a-signal) |
| `vitrind` will not die, or it died and the machine is still stuck; **you have a shell with `sudo`** | [3 — SysRq through `/proc/sysrq-trigger`](#route-3--sysrq-through-procsysrq-trigger-sudo-only) |
| Nothing responds at all | [4 — power cycle, then the installer USB](#route-4--the-installer-usb-and-a-chroot) |

**A wedged session does not have to look dark, and on 2026-08-13 it did not.**
The operator pressed `Ctrl-Alt-F3` to return to a `SIGSTOP`ped session and the
panel kept showing **the previous VT's last console content** — tty2's shell,
sitting there apparently fine. The kernel had already made tty3 active
(`/sys/class/tty/tty0/active` read `tty3` for the whole wedge), but the stopped
compositor never acknowledged the acquire and never set a mode, so the
framebuffer simply retained what was last scanned out.

To a human that reads as *"the VT switch did nothing"*, which is the wrong
diagnosis and points at the wrong route. **Check `/sys/class/tty/tty0/active`
before believing the screen**: if it names the VT you asked for and you are not
looking at it, the session on that VT is wedged, not the switch.

The switch was never refused, either. It sat **pending for the whole 163.8 s**
and completed in the same millisecond the process resumed — the first line
logged after `SIGCONT` was `the seat activated this session; reclaiming the
panel`. Route 1's chord had been accepted all along and was queued behind the
wedge.

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

**Confirmed working; not confirmed as an escape.** All 19 chords on record — 10
in L1 below, 9 in the bring-up page's item 12 — were pressed against a *healthy*
compositor, and every one behaved. The one deliberate wedge on record, L6, is
exactly the case this route exists for, and the chord did not get the operator
out of it: a `SIGSTOP`ed compositor cannot call `Session::change_vt`, because
the code that would call it is inside the stopped process. That is the boundary
of what a compositor-implemented chord can do rather than a defect in it, and it
is why route 2 is below this one on the page and still ahead of it in evidence.

## Route 2 — a shell somewhere else, and a signal

**This is the only route that has ever actually recovered a session.** On
2026-08-09 the first bare-metal run wedged with no working VT chord, and what
freed it was a terminal in the *still-running Hyprland session on tty1*.

**Resolve the PID first, then signal that number.** Never signal a pattern:

```bash
# 1. Find it. `-x` matches the process NAME, so nothing that merely mentions
#    vitrind in its own command line can match. `-a` prints each command line,
#    so you can see which session you are about to end before you end it.
pgrep -x -a vitrind

# 2. READ ITS STATE. This decides which signal, and getting it wrong looks
#    exactly like the signal not working.
ps -o pid,stat,args -p <PID>

# 3a. STAT contains `T` -- the process is STOPPED. `kill -TERM` is INERT here:
#     a stopped process cannot handle SIGTERM, so it queues as pending and
#     nothing observable happens. SIGCONT is the recovery, and it PRESERVES
#     the session -- the compositor resumes and carries on.
kill -CONT <PID>

# 3b. STAT is `S` or `R` -- running but unresponsive. This is the case route 2
#     was written for, and here TERM is right.
kill -TERM <PID>
```

**Step 3a is not a footnote; it is [#277](https://github.com/vitrin-os/vitrin-os/issues/277).**
This page's own `L6` rung wedges the session with `SIGSTOP`, and until
2026-08-13 the only signal it published was `TERM` — which cannot recover that
wedge. Verified twice on that date, once against a controlled process and once
against the real `vitrind`:

```text
14:16:46  SIGSTOP           -> state Tl+
14:16:49  SIGTERM, 3 s wait -> STILL ALIVE, state Tl+
14:16:52  SIGCONT           -> EXITED immediately; the pending TERM landed on resume
```

`SIGKILL` also works on a stopped process, immediately — but it discards the
session, where `SIGCONT` gives it back.

**Half of that is verified and half is not, and the difference matters here.**
Step 1 was run read-only against a live session on 2026-08-11 and returned
exactly that session's PID and nothing else [verified 2026-08-11]. **Step 2 has
never been used to recover a wedged session in this form** — route 2's one real
recovery, on 2026-08-09, was typed as the broken command below. Run step 1 once
while nothing is wrong, for the same reason bring-up step 0.1b makes you
exercise the VT chord before you need it.

> **Do not "simplify" that back to `pkill -TERM -f "vitrind --drm"`.** That is
> what this page published until 2026-08-11, and it is broken three ways at
> once ([#260](https://github.com/vitrin-os/vitrin-os/issues/260)):
>
> - **Through a shell it is too greedy, and it aims at you.** `pkill -f`
>   matches whole command lines, so a shell that runs the command has the
>   pattern in its own `argv`. `pkill` skips its own PID but **not its
>   parent** — so `-TERM` ends the rescuer at the moment the rescue is being
>   attempted.
> - **On this machine it never matches the target at all.** The
>   `~/.local/bin/vitrind` wrapper inserts `--shim <path>` between the binary
>   and whatever you typed, so the literal string `vitrind --drm` does not
>   appear anywhere in the real process's command line — it is not a pattern
>   that describes this process. One injected argument is enough; the wrapper's
>   other job is environment variables, which never reach `argv` at all, and
>   `--blank-idle` is the operator's own flag, which lands *after* `--drm`.
>   Checked read-only against the running session: `pgrep -f 'vitrind --drm'`
>   returned **only the invoking shell**, while `pgrep -x -a vitrind` returned
>   the one real PID. [verified 2026-08-11]
> - **Wrapped in a unit it is silently empty.** `systemd-run` does not go
>   through a shell, so the quotes are stripped and `--drm` arrives as a second
>   argument. `pkill` takes one pattern, matches nothing, and the unit exits
>   `1` having signalled nothing — while the operator believes the session was
>   rescued. That is exactly what happened on 2026-08-11. [verified 2026-08-11]
>
> A recovery command that fails silently is worse than one that fails loudly,
> because it is used precisely when nobody is reading the output.

**A signal wrapped in a unit or a timer must not depend on shell quoting.**
There is no shell there to do the quoting. Resolve the PID *before* you arm it
and give the unit a literal number:

```bash
# A standby rescue, armed before you wedge anything, with the PID already known.
systemd-run --on-active=120 --unit=l6-rescue /usr/bin/kill -CONT <PID>
```

The property this route depends on is that a `vitrind` session on tty3 does not
disturb Hyprland on tty1 — so a terminal there, or an agent session running in
one, still reaches the machine. Leave one open before you start anything. See
[bring-up step 0.1](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md).

Escalate within the route rather than jumping out of it, on the same PID
throughout:

```bash
PID=<the number you read above>   # one number, not a pattern
kill -INT "$PID"                  # ask for a clean shutdown first
sleep 2
kill -0 "$PID" 2>/dev/null && echo "still alive" || echo "gone"
kill -KILL "$PID"                 # only if -INT did nothing
pkill -x vitrin-shim              # shims are children of vitrind; check anyway
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
processes out of the way when you have no shell; here you have one, and route
2's signal to one resolved PID is the aimed version of the same idea.

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

  **From a shell, you can now tell the two apart without looking at the panel.**
  A blank logs `the session went idle` — it always did — but until this release
  that was the *only* line the whole cycle produced, so a wake that worked and a
  modeset that left the panel dark were both followed by silence. A wake now
  logs `the panel is lit again`, and a wake that never completed logs `THE WAKE
  WAS NOT CONFIRMED` at `WARN` — the case that is genuinely indistinguishable
  from a wedge *at the panel*, and the one that means route 1. The flight
  recorder carries the pair as `screen_blanked` and `screen_woke`, neither of
  which existed before; the wake entry's `outcome` field is `flip_landed` (the
  panel came back), `no_flip` (it may not have) or `seat_lost` (the blank ended
  because you switched VT), and both entries carry `locked`, because an idle
  blank never raises a lock but can perfectly well go up behind one.
  [verified: `crates/vitrin-core/src/session.rs`, `crates/vitrin-core/src/recorder.rs`]

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
Hyprland-side shell of step 0.1 open the whole time. Rungs are numbered `L1`–`L7`
so they do not collide with the bring-up page's own 1–15. (`L7` was added by the
fix for [#257](https://github.com/vitrin-os/vitrin-os/issues/257), which the first
run of `L4` found; a rung added because a run found something is what this table
is for.)

| # | Do | Expect | Worst credible failure |
|---|---|---|---|
| L1 | **10 VT switches away and back.** `Ctrl-Alt-F2`, wait, `Ctrl-Alt-F3` back. Ten times. | Session survives every one; band the same colour each return; recorder shows paired pause/activate | A dead session, or a black panel with `vitrind` alive (master not reacquired) |
| L2 | **5 suspend/resume cycles.** `systemctl suspend` from the escape shell. | Machine sleeps, wakes, panel comes back, apps still there | Panel never returns; or apps frozen because no frame clock restarted |
| L3 | **5 lid close/open cycles.** | logind suspends on close, resumes on open, panel returns | Lid does nothing (check `systemd-inhibit --list`); or resume leaves a black panel |
| L4 | **Blank and unblank.** Start with `--blank-idle`, leave the machine alone past the timeout, then press a key. | Panel goes dark; any physical input brings it back; the log carries `the session went idle` **and** `the panel is lit again`, and the recorder a `screen_blanked`/`screen_woke` pair with `outcome: flip_landed` | Panel dark and input swallowed — this is indistinguishable from a wedge, go to route 1. `THE WAKE WAS NOT CONFIRMED` in the log, or `outcome: no_flip` in the recorder, is that case naming itself |
| L5 | **Confirm the blank did not lock.** After L4's wake, look at what is on screen, and at the `screen_blanked` entry the recorder wrote. | The session as you left it — **not** a lock card — and `locked: false` on the entry | A lock card, which would mean idle-blank and idle-lock got coupled. `locked: true` with no lock card on screen means the *entry* is wrong, which is its own defect |
| L6 | **One deliberate wedge, recovered by a documented route.** **Choose the route before you wedge anything and write it down as you use it** — see the warning below. | Route 1 or 2 recovers it | Neither does; record how far down the table you had to go |
| L7 | **Leave and come back with the blank armed.** With `--blank-idle 60` (and, on a second pass, `--lock-idle 60` as well), switch to another VT, stay there **longer than the timeout**, and switch back. Time how long the panel stays lit after the return. | The panel stays lit for the full timeout measured **from the return** — 60 s, not 1.5 s — and the lock does not raise | The panel blanks within a couple of seconds of coming back, or the session demands a passphrase for returning: the idle clock is being stamped with an instant from before the absence ([#257](https://github.com/vitrin-os/vitrin-os/issues/257)) |

Three rungs deserve their own warnings.

**L2 and L3 are complete as of 2026-08-13, and the second run fixed more than
the counts.** 2026-08-11 managed 4 of 5 suspend/resume cycles and 2 of 5 lid
cycles. 2026-08-13 added the fifth suspend and three more lid cycles that
reached sleep, taking both to **5 of 5**.

It also closed a hole the first run could not have seen. The 2026-08-11 session
carried no `--keymap`, so **nothing could be typed into the app** — and an idle
terminal produces no frames, which makes "the app is correctly idle" and "the
app is frozen" the same artefact. The counts were met over a client that could
not be proven alive. On 2026-08-13, with the keymap passed, the operator typed
after each resume and the log carries the proof directly:

```text
                    56 frames,  96 keys      before any suspend
  === suspend / resume ===
                     7 frames,  18 keys      typed after the systemctl suspend
  === lid close / open ===
                    13 frames,  24 keys      typed after the lid cycle
```

New frames after both resumes: the frame clock restarts, and the failure mode
this rung names does not occur. **Run these with a keymap** — without one the
rung passes on evidence it does not have.

Also worth keeping: on 2026-08-13 a lid close reopened within the same second
**never reached sleep at all**. That is correct behaviour, not a miss, and it is
the short-lid-close case one sample could never have established.

Do them with the escape shell open and with nothing unsaved.

**L6's answer was lost once, and is now recovered.** On 2026-08-11 the wedge
recovered in ~69 s and **which route did it could not be reconstructed
afterwards** — not from the journal, not from either flight recorder, not from
the process tree, not from two units' exit codes. The page then guessed "`fg`
typed blind".

**2026-08-13 settled it, and the guess was right in substance**: `fg` sends
`SIGCONT`, and `SIGCONT` is what recovered the second wedge. The route is now
named, timed, and mechanised:

```text
14:07:55.141  last log line -- session paused
              ... SIGSTOP, 163.8 s wedged ...
14:10:38.963  "the seat activated this session; reclaiming the panel"
```

Three things that run established which the first could not:

1. **`kill -TERM` does nothing to this wedge** — verified against the real
   binary ([#277](https://github.com/vitrin-os/vitrin-os/issues/277)). Route 2's
   published command was the wrong signal for the page's own rung.
2. **`kill -CONT` recovers it and keeps the session**, in the next logged
   millisecond.
3. **Route 1's chord was queued, not defeated.** It completed the instant the
   process resumed, having sat pending for the whole wedge.

Decide the route before you wedge anything anyway, and write the time down *as*
you use it. Reconstructing it four minutes later did not work the first time.

**L4 can produce exactly the symptom this whole page is about.** Unblanking is a
full modeset, and a modeset that fails leaves a dark panel with the session
running. If L4 does not come back, you are in route 1 — and that is a result to
record, not a mishap.

### The numbers this checklist owes

These were owed to issue #223 and were pasted into it on **2026-08-11**. The
second run, **2026-08-13**, discharged the rest. Both runs are recorded below.

- **L1 — 10 of 10** (19 switches, 0 stalled; chord-to-pause median 240 ms).
  2026-08-11.
- **L2 — 5 of 5.** Four on 2026-08-11 (resume-to-panel 24–31 ms, one 2100 ms
  outlier), the fifth on 2026-08-13. **Liveness proven on the second run only**:
  the app took keystrokes and produced new frames after the resume. The first
  run had no keymap and therefore no way to tell an idle app from a frozen one.
- **L3 — 5 of 5.** Two on 2026-08-11 (one usable), three on 2026-08-13, all
  three reaching sleep, with the same typed-after-resume liveness proof. A
  fourth close/open inside one second correctly never suspended.
- **L4 — blank at 61.2 s; wake confirmed.** 2026-08-11 for the transition,
  2026-08-12 for the log line and recorder pair
  ([#258](https://github.com/vitrin-os/vitrin-os/issues/258),
  [#259](https://github.com/vitrin-os/vitrin-os/issues/259)), and **observed a
  second time on 2026-08-13's L7 run** — `the panel is lit again` present,
  `THE WAKE WAS NOT CONFIRMED` absent, `outcome: flip_landed`. No wake has ever
  failed on this machine, so the WARN arm remains unexercised; that is the pass
  condition, not a gap.
- **L6 — `SIGCONT`, 163.8 s.** The route is named at last, and the mechanism
  with it: `kill -TERM` is inert against this wedge
  ([#277](https://github.com/vitrin-os/vitrin-os/issues/277)), route 1's chord
  was *queued* rather than defeated, and the panel showed the previous VT rather
  than going dark. 2026-08-13.
- **L7 — 61.214 s, measured.** 2026-08-13, `--blank-idle 60`. The seat returned
  at 14:22:21.655 and `screen_blanked` was journalled at 14:23:22.869, so the
  panel stayed lit for **61.214 s counted from the return** against a 60 s
  timeout. The ~1.2 s over is service-loop granularity and matches L4's
  independently measured 61.2 s.

  This replaces the by-eye pass of 2026-08-11, which ran at a 20 s timeout and
  could not distinguish "the full 20 s" from "17 s". The figure this rung asked
  for exists now. Both instants come from `vitrind` itself — the seat-return
  line from the log, the blank from the recorder's `wall_ms` — so no
  cross-process clock is involved. See the note on clocks in the 2026-08-13
  record below before computing anything of this shape yourself.

## Record the run

Date it and record the environment, the same shape
[`docs/drm-bringup.md`](https://github.com/vitrin-os/vitrin-os/blob/main/docs/drm-bringup.md)
uses. The value of a manual runbook is entirely in whether anyone can tell when
it was last actually executed. The next run copies the shape below, blanks the
values and fills them in from its own eyes.

### First run — 2026-08-11, L1–L6 on the target machine

Numbers below are read from the flight recorders, the `tee`'d logs and
`journalctl`; the visual observations are the owner's. Nothing here is inferred
from source.

```text
Executed:      2026-08-11, JST
By:            @tahaayan
Kernel:        7.1.6-arch1-1        Mesa 26.1.6
GPU:           i915, /dev/dri/card1, eDP-1 @ 2560x1600, scale 1
Binary:        target/release/vitrind --features drm-backend, built 2026-08-11

logind values, read at the time of the run (/etc/systemd/logind.conf carries
only the [Login] header, so these are the defaults in effect):
    HandleLidSwitch=suspend        HandlePowerKey=poweroff
    HandleSuspendKey=suspend       IdleAction=ignore
    InhibitDelayMaxSec=5s
    Delay inhibitors on sleep: NetworkManager, rtkit-daemon, upowerd

  L1. 10 VT switches ............. 10/10 survived; band colour stable? YES
  L2. suspend/resume ............. 4 of 5 cycles run; 4/4 panel returned
  L3. lid close/open ............. 2 of 5 cycles run; 1 suspended and behaved
                                   as L2, 1 never reached Sleep at all
  L4. Blank and unblank .......... blank after 61.2 s; unblank OK
  L5. Blank did not lock ......... PASS (session as left, no lock card)
  L6. Deliberate wedge ........... recovered in ~69 s, route INDETERMINATE
  L7. Return from another VT ..... RUN SEPARATELY, later on 2026-08-11, at
                                   --blank-idle 20 --lock-idle 20 (not 60).
                                   Panel stayed lit on the return -- it did NOT
                                   blank in ~1.5 s. Lock did NOT raise on the
                                   return; it raised only after the countdown
                                   ran again from the return. Both by eye:
                                   seconds NOT timed, absence NOT timed.
                                   Next run: panel stayed lit ___ s after the
                                   return (must be the full --blank-idle
                                   timeout); absence ___ s.

SysRq step 1 (`printf 's' | sudo tee /proc/sysrq-trigger`) executed? NO
    (still documented and unexecuted)
```

**L1 — 10/10, and the chord is confirmed on hardware from a healthy session.**
**0 refused, 0 stalled** over this run's 10, out of 19 chords across the two
runs. The other nine are on the bring-up page's item 12, which records them as
5 switches honoured *plus 4 `vt_switch_refused already_here`* — the human
chording the VT he was already on, i.e. the code declining a no-op rather than a
switch failing. The two records are quoted side by side rather than merged into
one refusal count, because only the operator's recorder logs can say whether the
`0 refused` above was scoped to this run or to all 19, and nobody has gone back
to them.

Zero stalls is a positive result rather than absent instrumentation:
`VtSwitchStalled` is live code fired from a timer for the case where
`libseat_switch_session` returns `Ok` and no `PauseSession` follows — the "chord
appears to work and does not" shape that trapped the maintainer on the first
bare-metal run. It never fired. Chord → seat pause
latency, n=9: **min 209 ms, median 240 ms, max 312 ms.** Pause/activate pairing
is exact: 9 pauses against 8 activates in the second run, the missing activate
being the pause the session was left in.

**What L1 does not establish is route 1.** Every one of those 19 chords was
pressed against a *healthy* compositor. The rung asks whether the chord works,
not whether it gets you out — and L6 below, the only wedge on record, is the
case where it does not.

**L2 — 4 cycles, not 5.** Kernel resume → `vitrind` reclaimed the panel:

| resume | latency |
|---|---|
| 13:03:44 | 24 ms |
| 13:04:33 | 31 ms |
| 13:06:26 | 2100 ms |
| 13:07:00 | 31 ms |

**Recorded as 4/5, not 5/5.** The journal shows four `systemctl suspend` cycles
where the rung asks for five. Every cycle that ran returned with a working panel
and live apps. The 2100 ms outlier followed the shortest suspend of the set
(5.6 s).

**L3 — 2 cycles, not 5, and they disagreed.** **Recorded as 2/5.** Both cycles
behaved as L2 *when they suspended*, but only one of the two suspended at all:

- Lid closed 13:07:12.96, opened 13:07:19.30 (6.3 s closed) — **never reached
  Sleep.** No suspend entry in the journal.
- Lid closed 13:07:27.77 → suspend entry 13:07:28.08 → opened 13:07:38.57 →
  suspend exit 13:07:39.29 → panel reclaimed 30 ms later.

Whether a short lid close reliably does not suspend on this machine is **not
established by two samples** and is not claimed here.

**The summary line above is corrected, not transcribed.** The #223 comment this
record comes from writes L3's one-line summary as `2/2 behaved as L2` while its
own detail — the two bullets above, which are verbatim — says only one of the
two suspended at all. The detail is what is right, so the line in the block
reads `1 suspended and behaved as L2, 1 never reached Sleep at all`. Against a
rung asking for 5 cycles, L3 is **1 usable sample**.

**L4 / L5 — the blank and the lock behaved; the run still filed three defects
against L4.** Blank fired at 61.2 s against `--blank-idle 60`. The panel
returned on ordinary physical input, twice, with the session unchanged and no
lock card — so idle blank and idle lock are confirmed uncoupled on hardware, as
D-033 intends, which is L5 and it passes. **L4 is not a clean pass**: the run
found #257 on the *return* path, and #258 and #259 came out of the same session
— the unblank logged nothing, so a successful wake and a modeset that left the
panel dark were indistinguishable, and neither transition reached the flight
recorder at all. All three now have fixes. **#257's has since been observed on
hardware** — see the L7 record below, run later the same day — but **#258's and
#259's have not**: the enriched expectations in L4's own row (the `the panel is
lit again` line, the `screen_blanked`/`screen_woke` pair) describe output that
did not exist when this run was made and was not looked for during the L7 run
either.

**L6 — recovered, route indeterminate.** The wedge was produced by `SIGSTOP` on
the compositor while it held DRM master and the libinput devices — a faithful
"compositor hung", reversible, and it does defeat `Ctrl+Alt+F<n>` exactly as
this page predicts.

```text
13:15:01.8   SIGSTOP -- wedge begins
13:16:10.8   alive again, processing a VT chord      ~69 s wedged
13:16:29     standby rescue fired into an already-running process (no-op)
```

**The route that recovered it is not recoverable after the fact**, and that is
recorded rather than guessed. Ruled out by evidence: it was not `Ctrl+C` (the
`tee` in the same foreground process group survived, and `SIGINT` does not
resume a stopped process); it was not either standby timer (one ran 80 s before
the wedge and exited 1 — that is the `pkill -f` defect in route 2 above — and
the other fired 19 s after recovery). Something delivered `SIGCONT` from the
tty3 session, most plausibly `fg` typed blind, but the operator did not recall
four minutes later and no artefact records it. **That indeterminacy is itself
the finding**, and it is why L6 now tells you to choose the route first.

**Not done, and not quietly dropped:**

- **The VKMS rung was not attempted by hand during this run.** It is, however,
  attempted by CI on every pull request, and on 2026-08-13 that attempt was read
  rather than assumed. See the VKMS note below the third run's record.
- **`/proc/sysrq-trigger` route 3 was not exercised.** Still documented and
  unexecuted.
- L2 and L3 are short of their stated counts, as recorded above.
- **`L7` did not exist during this run** — it was written *from* this run's
  #257. It has since been executed, separately and later the same day; the
  result is the block below.
- **L4's new log and recorder expectations were observed by nobody.** They were
  added by the #258/#259 fixes after this run, so nothing in the block above
  reached them — and the L7 run that followed did not look at the log or the
  recorder either. **A third run, on 2026-08-12, did: see `L4 (second
  execution)` below.**

**Filed from this run:** #257 (returning to a paused session blanks the panel in
~1.5 s), #258 (the unblank is silent; success and failure look identical), #259
(blank/unblank leave no flight-recorder event) and #260 (this page's published
recovery command signalled the rescuer under a shell and nothing at all under
`systemd-run` — corrected in route 2 above).

### L7 — same day, separate run, `--blank-idle 20 --lock-idle 20`

Run after the fixes for #257–#259 landed on `main`, at a **20 s** timeout rather
than the 60 s the rung suggests. Both passes were done in one sitting.

```text
Executed:      2026-08-11, JST, same machine and binary family as above
                              (rebuilt from main after #263 merged)
Flags:         --drm --blank-idle 20         (pass 1)
               --drm --blank-idle 20 --lock-idle 20   (pass 2)
               --lock-on-seat-change: not passed, so the default `never`

  Pass 1. Panel on return ........ STAYED LIT. It did not blank on the way back
                                   in, which is the ~1.5 s symptom #257 filed.
  Pass 2. Lock on return ......... DID NOT RAISE on the return. It raised only
                                   after the countdown ran again *from* the
                                   return, with the session sitting idle --
                                   which is what `--lock-on-seat-change never`
                                   is specified to do.

  Timed? ....................... NO. Both observations are by eye. The seconds
                                 the panel stayed lit were not measured, and
                                 the absence was not measured either.
  Log lines checked? ........... NO -- `the panel is lit again` (#258) was not
                                 looked for.
  Recorder checked? ............ NO -- the `screen_blanked`/`screen_woke` pair
                                 and `outcome: flip_landed` (#259) were not
                                 looked at.
```

**What this settles and what it does not.** It settles #257, which is a symptom
question — the panel blanked ~1.5 s after a return, and it no longer does; the
lock demanded a passphrase for coming back, and it no longer does. Both symptoms
are gone on the machine that produced them, under the default seat policy. It
settles **nothing** about #258 or #259: those are about what the wake *says*,
and nobody read the log or the recorder during this run. It also produces no
number — an eyeball pass at a 20 s timeout cannot distinguish "the full 20 s"
from "17 s", so the rung's own question, *how long did the panel stay lit*, is
still unanswered and the record block above says so.

### L4 (second execution) — 2026-08-12, `--blank-idle 60`

The first run that read the log and the recorder rather than only the panel.
**#258 and #259 are settled by it**, and nothing else is.

```
  Executed: 2026-08-12 14:00:57 JST (+0900), by the maintainer, on the same
            machine as every block above.
    Binary: vitrind rebuilt from `main` at 13:46 the same day.
            --blank-idle 60; --lock-idle NOT passed.

  Panel .......................... blanked on the idle timer, stayed dark, and
                                   came back on a keypress. Observed by eye.

  Log line (#258) ................ YES.
      the panel is lit again: physical input woke the session and the modeset
      was accepted. The wake itself restores no authority -- an idle blank
      never took any.
  THE WAKE WAS NOT CONFIRMED ..... 0 occurrences.

  Recorder (#259) ................ YES, the pair, from the same wake:
      {"kind":"screen_blanked","live_grants":0,"locked":false}
      {"kind":"screen_woke","dark_ms":5630,"outcome":"flip_landed",
       "live_grants":0,"locked":false}

  Timed? ......................... NO. `dark_ms` is how long the panel was dark
                                   before a key was pressed, not a latency: it
                                   measures the human, not the wake.
```

**The earlier run recorded nothing because of the binary, not the code.** The
13:42 attempt the same day used a `vitrind` built on 2026-08-11 at 12:34 — five
hours older than the commit that added both the line and the pair — so it
blanked and woke while carrying no code to write either down. A wake that
logs nothing and a build that cannot log are indistinguishable in the artifact;
only the binary's mtime tells them apart. **Check what you are running before
reading a silence as a result.**

**What it does not settle.** The WARN arm is unexercised: no wake failed, so
`THE WAKE WAS NOT CONFIRMED` has still never been emitted on hardware, and its
absence here is the pass condition rather than a gap. No figure was taken, so
L7's question is still unanswered.

**A narrow point about `L5`, which passed on 2026-08-11 and is not reopened
here.** That row asks for two things from one run: no lock card on screen, and
`locked: false` on the `screen_blanked` entry. The 2026-08-11 run checked the
screen half with the lock armed and passed it; the recorder entry did not exist
yet, so there was nothing to read. This run has the entry and it reads
`locked: false`, but it did not arm the lock, so its clean screen is what an
unarmed lock looks like rather than evidence about the boundary. **Both halves
hold, from different runs; the row as written has not been satisfied by a single
one.**

**Adjudicated closed by the maintainer on 2026-08-12**, and recorded as an
adjudication rather than as an observation, because that is what it is. His
reading is that the 2026-08-11 pass is the substantive one — the lock was armed,
the blank fired, and no card came up — and that a second run to put both halves
in one artifact is bookkeeping rather than evidence. That is a reasonable call
and it is his to make; what it costs is stated here so nobody later reads `L5`
as something it was not. **What a single arming pass would still buy, and only
it:** the case where the screen and the journal disagree — a `locked: true`
entry under a screen with no card, which this row's own failure column names as
its own defect. Two runs cannot catch a disagreement between them by
construction. Nobody believes that state is live; it simply has not been looked
for.

### Third run — 2026-08-13, L2/L3 completion, L6 and L7

```
  Executed: 2026-08-13, by the maintainer, on the same machine as every block
            above. Artefacts in ~/vitrin-runs/223-{cycles,l7}-*.{log,jsonl}.
    Binary: vitrind rebuilt 12:26 that day from a clean tree at 9b6239e,
            --features drm-backend. Shim rebuilt 12:28 against the VENDORED
            wlroots 0.19.3 -- a system upgrade had replaced 0.19 with 0.20 and
            the previously built shim could not start at all.

  L2 ....... 5 of 5. The fifth cycle, plus liveness (below).
  L3 ....... 5 of 5. Three more cycles reaching sleep, plus liveness. A fourth
             close/open inside one second correctly never suspended.
  LIVENESS . NEW, and the reason the counts now mean something. With --keymap
             passed, typing after each resume produced frames:
               before any suspend .... 56 frames, 96 keys
               after the suspend ......  7 frames, 18 keys
               after the lid cycle .... 13 frames, 24 keys
  L6 ....... SIGCONT, 163.8 s wedged. Route named for the first time.
  L7 ....... 61.214 s lit, measured from the seat's return, against
             --blank-idle 60. The figure this rung has owed since it was
             written.
  L4 ....... re-observed in passing on the L7 run: `the panel is lit again`
             present, `THE WAKE WAS NOT CONFIRMED` absent, flip_landed.
  L7 pass 2  ATTEMPTED, and it did not exercise what it was for. See below.
```

**`L7`'s second pass is still owed, and saying so costs nothing.** A
`--blank-idle 60 --lock-idle 60` session was run at 14:46:03. Its recorder
carries **zero seat activations**: the first seat event of the run is the pause
at 14:47:29, when the operator left at the end. There was no absence and no
return inside it. What it recorded instead is the plain idle path, which is
worth keeping:

```text
  14:47:10.060  session_locked   cause: idle      both 60 s timers, same expiry
  14:47:10.084  screen_blanked   locked: true     24 ms after the lock
  14:47:19.793  screen_woke      flip_landed      woken while locked
  14:47:24.577  session_unlocked
```

So `--lock-idle` fires with the right cause, a wake works *while locked*, and
unlock works. None of that is the question pass 2 exists to ask, which is
whether the lock raises **on the return** from an absence longer than the
timeout. That still rests on one by-eye observation at a 20 s timeout from
2026-08-11 — it passed, and it has never been measured.

**Note for whoever runs it:** the idle lock cannot fire *during* the absence —
losing the seat stops the idle clock under the default policy (D-030(7)), which
is the whole of [#257](https://github.com/vitrin-os/vitrin-os/issues/257)'s fix.
The measurement is therefore entirely about what happens *after* the seat comes
back, and the run is only valid if the recorder shows a `seat activated` line
inside it. Check for one before reading any result.

**Two method notes, both of which cost time before they were understood.**

**Never correlate the shim's clock with `vitrind`'s.** The shim's `00:00:00.000`
starts at *shim* launch, not core launch, and the tee'd log interleaves two
writers with different buffering, so **line order is not time order**. A defect
was briefly read into existence this way on the same day's 13a run. Where one
clock is needed, use the flight recorder: every entry carries both `mono_us` and
`wall_ms`, and `wall_ms` correlates directly with the log's tracing timestamps
because both come from the same process. L7's figure above was computed exactly
that way.

**A second `vitrind` refuses to start, and says so.** An accidental second
launch during this session exited with `fatal: another vitrind already holds
this runtime tree (its lock on /run/user/1000/vitrin-0/core.sock.lock…)`. It did
not fight for DRM master. Worth knowing, because the failed attempt still writes
its own near-empty log, and picking that file by timestamp will make a
successful run look like it recorded nothing.

### The VKMS rung: attempted every PR, and what it actually returns

`.github/vkms/run-advisory.sh` runs on every pull request, and **the green check
means nothing** — the script exits 0 on a declared skip exactly as it does on a
real probe, deliberately, so the rung can never start gating merges. The
evidence is in the job log, not the checkmark. Read on 2026-08-13:

```text
-- module state: loaded
-- no vkms card node appeared; skipping the GBM/EGL probe
-- probe summary: no-vkms-card-node
```

So the honest state is a **third** outcome, and not the one the rung's own
acceptance criterion anticipated. The module is not unavailable — it loads. But
no `/dev/dri/card*` node appears behind it on the hosted runner, so nothing
downstream runs: no connector enumeration, no mode set, no atomic commit, no
page flip, and no GBM/EGL probe. **The rung is attempted continuously and
currently covers nothing.**

That is worth stating plainly rather than leaving as "not attempted", because
the two are different claims and only one of them is true. What would change it
is a host where the card node does appear — a local machine with udev and root,
rather than a container. That has **not** been done here, and the reason is
recorded rather than skipped: loading a new DRM device on a machine running a
live compositor risks that compositor enumerating it and attaching an output to
it. On the maintainer's one laptop that is a live-session risk taken for a rung
which, by its own header, **can never prove the thing that matters** — that the
backend lights a real panel. `docs/drm-bringup.md`, executed by a human, remains
the only evidence for that.

> **This page has been executed twice, on 2026-08-11 and 2026-08-13, and is now
> a pass on every rung it can reach.** L1 through L7 have all been run and all
> have their numbers, with L2/L3 at full count and proven live, L6's route
> named, and L7 timed. What remains unexecuted is named rather than implied:
> **routes 3 and 4 are still careful predictions**, the **VKMS rung is attempted
> on every PR and currently covers nothing** (the module loads, no card node
> appears), **`L7`'s second pass has not yet caught a real absence** so
> lock-on-return rests on a 20 s by-eye pass, `L5` is adjudicated closed rather
> than re-run, and the WARN arm of L4 has never fired because no wake has ever
> failed here.
>
> Both runs' headline findings were defects in **this page's own recovery
> command** — `pkill -f` in 2026-08-11
> ([#260](https://github.com/vitrin-os/vitrin-os/issues/260)), `kill -TERM`
> against a stopped process in 2026-08-13
> ([#277](https://github.com/vitrin-os/vitrin-os/issues/277)). A recovery page
> that has been wrong twice about its own central instruction is a page to read
> sceptically. Correct it from your own eyes, and treat a failed observation as
> a result worth recording rather than a step to retry until it passes.
