<!-- SPDX-License-Identifier: Apache-2.0 -->
# This is not a desktop shell

`run_shell.py` is a **line-oriented switcher and launcher that is an ordinary
SDK client**. It draws nothing. It has no hotkey. It does not read your
keyboard. You type `focus editor` into a terminal on the host and press Enter,
and it sends one wire request. That is the whole of it.

Vitrin OS does not have a shell, and this directory is not one arriving. It
exists to prove that the switcher *can* be a client — which is a claim about
where authority lives, not about ergonomics — and to make the price of that
claim visible instead of theoretical.

## Why it is a client at all

[PRD](../../docs/PRD.md) §5.1 makes "window-management policy lives outside the
core" a permanent invariant.
[D-021(4)](../../docs/plan/20-decision-log.md) records that the cheap path — a
switcher inside the compositor — was available, cost about the same, and was
refused. An invariant that never costs anything has not been tested; this is
the issue where it costs something, and this program is the bill.

## Why it is not graphical: a principal cannot draw

Not "not yet polished". There is **no principal-facing surface interface
anywhere in [`protocol/vitrin-v0.xml`](../../protocol/vitrin-v0.xml)**.
`vitrin_view` is capture-only: a grant holding `observe` can read a realm's
pixels and can put none back. So there is no request this program could send
that would place a single pixel on the Vitrin output, and the only screen it
can write to is the host terminal you started it in.

The intended eventual shape is this program running **as a realm** — drawing
through its own shim like any other app, while holding the layout verbs through
the ordinary grant path. That needs no new protocol. What it does need is for
the shell's realm to reach the core socket — a confinement question of its own,
answered on 2026-08-24 by
[D-046](../../docs/plan/20-decision-log.md#d-046--a-shell-realm-reaches-the-core-socket-through-a-descriptor-the-core-mints-and-passes-down-the-spawn-path-it-already-has-the-authority-is-an-operators-declaration-and-a-humans-consent-and-what-the-connection-may-carry-is-fenced-structurally-rather-than-by-consent)
(issue [#311](https://github.com/vitrin-os/vitrin-os/issues/311)): the core
mints the connection and passes it into the realm as an inherited descriptor,
on an operator's `realm.toml` declaration **and** a human's consent, and it may
carry `layout_focus`, `layout_arrange` and `realm_launch` and never `observe` or
either actuation verb. **Nothing of that is built** — this program is still the
line-oriented one, for the reason above.

## Why it has no hotkey: a principal cannot receive physical input

Also structural. There is **no `observe_input` verb and none is designed**.
The core owns exactly two physical chords, and owns both for the same reason:

- the **dead-man switch** ([`deadman.rs`](../../crates/vitrin-core/src/deadman.rs)) —
  the human's off-switch must not depend on a client being alive and correct;
- the **attention key** ([`attention.rs`](../../crates/vitrin-core/src/attention.rs),
  since #232) — a one-bit signal that suspends the core's own `preempted`
  courtesy for one layout request.

A convenience hotkey is not in that class and **must not borrow that warrant**.
"Super+Tab switches windows" would mean the core reserving a chord on behalf of
whichever client happened to ask first, which is window-management policy
wearing a keyboard's clothes.

## The commands, in full

| Command | What it sends |
|---|---|
| `list` | nothing — prints what the shell was **told** about, and what it holds |
| `focus <realm>` | `vitrin_layout_focus.focus` |
| `launch <template>` | `vitrin_launcher.launch`, then a fresh `layout.focus` petition over the realm the core minted |
| `fullscreen on\|off` | `vitrin_layout_arrange.set_fullscreen` |
| `help`, `quit` | nothing |

There is no `place`, no `resize`, no `raise` and no stacking, because the
protocol has no such requests — see
[`docs/book/src/limits.md`](../../docs/book/src/limits.md). The vocabulary is
short because the authority is small, not because the front end is unfinished.

## Running it

```console
$ python3 -u examples/shell/run_shell.py \
    --socket "$XDG_RUNTIME_DIR/vitrin-0/core.sock" \
    --identity vitrin://local/agent/demo --token "$VITRIN_TOKEN" \
    --realm realm-0 --realm editor --template kiosk
  this shell draws nothing and reads no input but this terminal's stdin; ...
  petition layout.focus realm-0
  granted realm-0 verbs=layout.focus persistence=while-running
  petition layout.focus editor
  granted editor verbs=layout.focus persistence=while-running
  petition layout.arrange realm-0
  granted realm-0 verbs=layout.arrange persistence=while-running
  petition realm.launch kiosk
  granted kiosk verbs=realm.launch persistence=while-running
ready 2 realm(s), 1 template(s)
vitrin> focus editor
ok focus editor
```

`--realm` and `--template` are **required inputs, not shortcuts**: the wire
carries no realm enumeration, so a client cannot ask what exists. `list` says
so every time it runs, because a listing that looked like a discovery would be
a lie.

**Output contract.** Each command prints zero or more explanatory lines
indented by two spaces, then exactly one status line matching
`^(ok|refused|error) `. Startup ends with a single `ready` line. Read until the
status line and never parse the explanations — that is how
[`tests/integration/test_shell.py`](../../tests/integration/test_shell.py)
drives it.

## Three rules it holds itself to

**1. It never refuses on the core's behalf.** A petition the human denied
still leaves a live `vitrin_grant` object on the wire, so `focus` over a denied
realm still *sends* the request and shows you the core's own
`refused(layout.focus, not_granted)`. The shell's memory of a denial is not
authority; the grant table is. A shell that answered locally would be inventing
a decision, and would also hide an expiry or a revocation behind a stale "you
were denied" from ten minutes ago.

**2. It shows refusals and never retries them.** There is no timer in this
program. `preempted` in particular gets the whole story printed:

```
vitrin> focus editor
  the human's own hand owns the realm their input is following, and a layout
  request yields to it for half a second after any physical event.
  the core owns an attention key for exactly this: tap Super on the Vitrin
  machine ... and re-issue the command.
  this shell does not retry for you. A silent retry would teach you that layout
  is unreliable, when what actually happened is that the human currently owns
  the output.
refused focus editor preempted
```

That refusal is rare for *this* program and would be the common case for the
in-realm version of it, because there the Enter that sends `focus editor` is
itself the physical input that forbids it. This shell runs on the host, so its
keystrokes never reach `vitrind` at all.

One thing to be uneasy about, and it is published as a limit rather than
argued away: a shell printing "tap Super and re-issue" is doing the right
thing, and a malicious client printing the same string to bank the timing is
indistinguishable from it. What bounds that is that the press confers nothing
the client did not already hold — but a human who has learned "press Super when
the screen tells me to" has learned a habit an attacker can invoke.

**3. It reads nothing but its own stdin.** No device nodes, no evdev, no X11
or Wayland connection of its own, no polling of anything global. Running a
switcher on a machine somebody is using must never mean a program that watches
their keyboard.

## What it makes worse

- **Killing it loses window management.** See the next section.
- **It holds `layout.arrange` for the whole output.** D-018(4) makes
  arrangement single-holder per output, session-wide, so while this shell lives
  a second tool that wants to arrange anything is refused `layout_held` at
  admission. That is the designed behaviour and it is also a restriction people
  hit before they understand why. This shell therefore petitions arrangement
  over **exactly one** realm (`--arrange`, defaulting to the first `--realm`),
  and `fullscreen` names that realm every time so it is never a mystery which
  realm it acted on.
- **Twelve apps means twelve prompts, every session.** The durable persistence
  rungs (`until_revoked`, `always`) resolve `unsupported` in version 1, so every
  grant here is `while_running` and every restart re-petitions from zero. That
  is the consent-fatigue cost, it is not small, and it is tracked as Q9 rather
  than solved here.
- **It is not a daily-driver switcher.** WS-E's premise is dogfooding, and this
  does not by itself make a machine usable without a terminal already open.

## If the shell dies

This is D-021's own stated cost, and it is asserted rather than described:
[`tests/integration/test_shell.py`](../../tests/integration/test_shell.py)
`SIGKILL`s the shell and then checks what survives.

**What survives a shell crash:** everything except window management. Both
realms keep running — their shims and their apps are children of `vitrind`, not
of this program — and the realm the shell last focused **keeps receiving the
human's physical input**, because the output binding lives in the core and
nothing revokes it when a client dies.

**What you lose:** the ability to re-aim any of it. There is no core-side
fallback switcher, by design. Until you start a shell again, the output stays
where it was pointed and no key or gesture will move it.

**Recovering** means running this program again, which re-petitions from zero
and raises a fresh prompt per realm. And here is the wedge, stated plainly
because it is the part that bites: **in a real session, the terminal you would
restart it from must already be the bound realm.** If the shell died while the
output was pointed at a realm with no terminal in it, there is nothing on
screen that can start the shell, and the remedies are all outside Vitrin — an
SSH session from another machine, a VT switch, or restarting `vitrind`. This
issue documents that; it does not solve it.

## Not a criterion

That this shell is pleasant to use, and that a hotkey works. The first is not
measurable and the second does not exist.
