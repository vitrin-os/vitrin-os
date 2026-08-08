#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""A **switcher and launcher that is an ordinary SDK client** (WS-E.1.5, #211).

It is not a desktop shell, it draws nothing, it has no hotkey, and it does not
read your keyboard. It is a line-oriented program you run in a terminal *on the
host*, which holds ``layout.focus``, ``layout.arrange`` and ``realm.launch``
grants through the same petition-and-consent path every other client uses, and
turns the five words you type into the four wire requests those verbs authorise.

PRD §5.1 makes "window-management policy lives outside the core" a permanent
invariant, and `docs/plan/20-decision-log.md` D-021(4) records that the cheap
path — a switcher inside the compositor — was available, cost about the same,
and was refused. This file is what that invariant costs, made concrete.

## Why it is line-oriented, and why there is no hotkey

Both are authority arguments, not ergonomics ones. They are restated in
``README.md`` beside this file because a directory listing is read more often
than a docstring, and both are structural gaps rather than missing polish:

* **A principal cannot draw.** ``vitrin_view`` is capture-only and there is no
  principal-facing surface interface anywhere in ``protocol/vitrin-v0.xml``.
  Nothing this program could do would put a pixel on the Vitrin output, so the
  only screen it can write to is the host terminal it was started from.
* **A principal cannot receive physical input.** There is no ``observe_input``
  verb and none is designed. The core owns exactly two physical chords — the
  dead-man switch (``crates/vitrin-core/src/deadman.rs``) and, since #232, the
  attention key (``crates/vitrin-core/src/attention.rs``) — and both are
  core-owned *precisely because* they must not depend on a client being alive
  and correct. A convenience hotkey is not in that class and must not borrow
  that warrant.

## Three rules this program holds itself to

1. **It never refuses on the core's behalf.** A petition the human denied
   still leaves a live ``vitrin_grant`` object on the wire, so ``focus`` on a
   denied realm still *sends* the request and shows you the core's own
   ``refused(layout.focus, not_granted)``. The shell's memory of a denial is
   not authority; the grant table is. A shell that answered locally would be
   inventing a decision, and would also hide an expiry or a revocation behind
   a stale "you were denied" from ten minutes ago.
2. **It shows refusals and never retries them** (#211 decision 5). In
   particular ``preempted`` — which for a shell running *inside* a realm is the
   commonest answer there is, because the Enter that sends ``focus editor`` is
   itself the physical input that forbids it. This shell runs on the host, so
   its keystrokes never reach ``vitrind`` and it does not hit that loop; it
   still prints the whole story when the core says ``preempted``, because
   #211's decision 2 names the eventual shape as this program running *as a
   realm*, where it would. **There is no timer here and no retry.**
3. **It reads nothing but its own stdin.** No device nodes, no evdev, no X11
   or Wayland connection of its own, no polling of anything global. The only
   input channel is the line editor of the terminal you started it in.

## The command vocabulary, in full

``list``, ``focus <realm>``, ``launch <template>``, ``fullscreen on|off``,
``help``, ``quit``. That is not a subset of a larger design: it is the whole of
what version 2 of the protocol lets a layout holder ask for. There is no
``place``, no ``resize``, no ``raise`` and no stacking, because the scene shows
one unstacked realm and a request that silently does less than its name is
worse than no request (``docs/book/src/limits.md``).

## Output contract (so a script can drive it)

Every command emits zero or more explanatory lines, each indented by two
spaces, and then exactly one status line matching ``^(ok|refused|error) ``.
Startup emits the same shape and ends with a single ``ready`` line. Read until
the status line; never parse the explanations.

Run with ``python3 -u`` when driving it from a pipe — this file flushes every
print, but the interpreter's own buffering is not this file's to fix.
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from dataclasses import dataclass, field as dc_field
from typing import Callable, TextIO

import vitrin_os
from vitrin_os import errors


#: The verbs this program petitions, and nothing else. It holds no `observe`
#: and no `actuate.*`: a switcher that could read your screen or type into
#: your apps would be asking for authority its job does not need, and the
#: consent card would have to say so.
FOCUS_VERBS = ("layout.focus",)
ARRANGE_VERBS = ("layout.arrange",)
LAUNCH_VERBS = ("realm.launch",)


# --- how a refusal is explained ---------------------------------------------
#
# One entry per typed exception the SDK can raise for these four requests.
# Written out rather than formatted from the wire code, because the useful
# half of a refusal is *what the human can do about it*, and the core has no
# way to say that.

def _preempted_note(exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "the human's own hand owns the realm their input is following, and a layout "
        "request yields to it for half a second after any physical event.",
        "the core owns an attention key for exactly this: tap Super on the Vitrin "
        "machine (or the key `--attention-chord` names) and re-issue the command. "
        "That press opens a one-second, single-use window in which this request is "
        "not refused preempted.",
        "this shell does not retry for you. A silent retry would teach you that "
        "layout is unreliable, when what actually happened is that the human "
        "currently owns the output (#211 decision 5).",
    )


def _consent_held_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "a consent prompt is up on the Vitrin output. Everything this principal "
        "holds is suspended until the human answers it — that is what makes the "
        "prompt trustworthy, so it is not a failure.",
        "answer the prompt, then re-issue the command.",
    )


def _not_granted_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "this grant does not carry that verb. Either the human denied the petition, "
        "or they narrowed the verb set when they approved it.",
        "the request was sent anyway, on purpose: this shell does not answer for the "
        "core (see the module docstring, rule 1). What you are reading is the grant "
        "table's answer, not this program's memory of an old prompt.",
    )


def _revoked_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "the grant was revoked — by the human, or by the dead-man switch, which "
        "revokes every grant in the session at once.",
        "restart this shell to petition again. Nothing here re-petitions on its own.",
    )


def _expired_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return ("the grant's expiry has passed. Restart this shell to petition again.",)


def _rate_limited_note(exc: errors.GrantRefused) -> tuple[str, ...]:
    retry = getattr(exc, "retry_after_ms", 0) or 0
    return (
        f"the grant's rate ceiling binds; the core says retry after {retry} ms.",
        "this shell does not sleep and re-send. Wait and type it again.",
    )


def _capacity_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "the session already holds MAX_REALMS (16) realms that have not exited.",
        "nothing can close a realm: there is no wire request that ends one, and "
        "revoking this launch grant does not close what it started "
        "(docs/book/src/limits.md). The only remedy is restarting vitrind.",
    )


def _no_surface_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return (
        "that realm has no live surface. A template that has never been launched "
        "never paints, and a realm whose app exited never paints again.",
    )


def _internal_note(_exc: errors.GrantRefused) -> tuple[str, ...]:
    return ("the core reported an internal failure. Its journal is where to look.",)


#: Refusal class -> explanation. Exhaustive against `vitrin_grant.refusal`;
#: `_explain_refusal` falls back loudly rather than silently on a code this
#: table does not know, because a code the SDK grew and this file did not is
#: the drift #211's risk section names.
REFUSAL_NOTES: dict[type, Callable[[errors.GrantRefused], tuple[str, ...]]] = {
    errors.NotGranted: _not_granted_note,
    errors.GrantExpired: _expired_note,
    errors.Revoked: _revoked_note,
    errors.RateLimited: _rate_limited_note,
    errors.Preempted: _preempted_note,
    errors.ConsentHeld: _consent_held_note,
    errors.NoSurface: _no_surface_note,
    errors.OperationFailed: _internal_note,
    errors.AtCapacity: _capacity_note,
}

#: Petition outcome exception -> explanation, same contract.
RESOLUTION_NOTES: dict[type, tuple[str, ...]] = {
    errors.GrantDenied: (
        "the human said no. That is a complete answer and this shell does not ask "
        "again; restart it if you want a fresh prompt.",
    ),
    errors.ConsentTimeout: (
        "nobody answered the prompt before it timed out.",
    ),
    errors.RealmUnavailable: (
        "no such realm in this session. There is no way to enumerate realms on the "
        "wire, so this shell only knows the names you gave it — check the spelling "
        "against the core's realm.toml.",
    ),
    errors.GrantUnsupported: (
        "this deployment does not serve that verb. `unsupported` is a petition "
        "outcome, not a refusal: it means not here and not now, never that the "
        "protocol lacks the verb.",
    ),
    errors.Busy: (
        "the core is holding too many pending petitions to raise another prompt. "
        "That is the consent-fatigue valve, not a denial.",
    ),
    errors.LayoutHeld: (
        "another live grant already holds layout.arrange for this output. D-018(4) "
        "makes arrangement single-holder per output, session-wide — one tool "
        "arranges, or none do.",
        "this shell keeps working without it: focus and launch are unaffected, and "
        "`fullscreen` will show you the core's refusal if you ask for it.",
    ),
}


def _refusal_code_name(exc: errors.GrantRefused) -> str:
    code = getattr(exc, "code", None)
    if code is None:
        return "unknown"
    try:
        return vitrin_os.Refusal(int(code)).name.lower()
    except ValueError:  # pragma: no cover - a code the SDK grew and we did not
        return f"code-{int(code)}"


def _explain_refusal(exc: errors.GrantRefused) -> tuple[str, ...]:
    note = REFUSAL_NOTES.get(type(exc))
    if note is None:  # pragma: no cover - see REFUSAL_NOTES' docstring
        return (
            f"this shell has no explanation for {type(exc).__name__}. That is a gap "
            "in examples/shell/run_shell.py, not in the core: the SDK knows the code "
            "and this table does not.",
        )
    return note(exc)


# --- what the shell was told about ------------------------------------------


@dataclass
class Binding:
    """One realm name the shell was told about, and the grant it holds over it.

    ``grant`` is kept even when the petition was denied or refused
    ``layout_held``: the wire object is alive either way, and rule 1 of the
    module docstring is that this program does not answer for the core.
    """

    name: str
    kind: str  # "realm" | "template" | "launched"
    verbs: tuple[str, ...]
    grant: object | None = None
    outcome: str = "not petitioned"
    detail: tuple[str, ...] = dc_field(default_factory=tuple)


class Shell:
    """The switcher, as an object, so a test can drive it without a terminal."""

    def __init__(
        self,
        conn,
        *,
        realms: list[str],
        templates: list[str],
        arrange: str | None,
        out: TextIO,
    ) -> None:
        self.conn = conn
        self.out = out
        self.focus_bindings: dict[str, Binding] = {
            name: Binding(name, "realm", FOCUS_VERBS) for name in realms
        }
        self.launch_bindings: dict[str, Binding] = {
            name: Binding(name, "template", LAUNCH_VERBS) for name in templates
        }
        # Exactly one arrange petition, ever, and the reason is not thrift:
        # D-018(4) makes `layout.arrange` single-holder **per output**, checked
        # at admission, so a second one from this same connection would resolve
        # `layout_held` and buy nothing. Which realm gets it is therefore a
        # choice the operator makes, not one this program can defer.
        self.arrange_realm = arrange if arrange is not None else (realms[0] if realms else None)
        self.arrange: Binding | None = (
            Binding(self.arrange_realm, "realm", ARRANGE_VERBS)
            if self.arrange_realm is not None
            else None
        )
        self.running = True

    # -- output ------------------------------------------------------------

    def _say(self, line: str) -> None:
        print(f"  {line}", file=self.out, flush=True)

    def _status(self, line: str) -> None:
        print(line, file=self.out, flush=True)

    # -- startup -----------------------------------------------------------

    def _petition(self, binding: Binding) -> None:
        """One petition, announced before it is sent and reported after.

        Announced first on purpose: the request blocks on an unbounded human
        consent delay, so a caller reading this stream (a human or a test) has
        to be told which prompt is about to go up *before* it goes up.
        """
        verbs = "+".join(binding.verbs)
        self._say(f"petition {verbs} {binding.name}")
        grant = self.conn.request_grant(
            realm=binding.name,
            verbs=binding.verbs,
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        )
        # Kept whatever the outcome is (rule 1).
        binding.grant = grant
        try:
            grant.await_consent()
        except errors.GrantResolutionError as exc:
            binding.outcome = type(exc).__name__
            binding.detail = RESOLUTION_NOTES.get(type(exc), ())
            self._say(f"not granted: {verbs} {binding.name} ({type(exc).__name__})")
            for note in binding.detail:
                self._say(f"  {note}")
            return
        binding.outcome = "granted"
        held = "+".join(sorted(_verb_names(grant.effective_verbs))) or "(none)"
        self._say(f"granted {binding.name} verbs={held} persistence=while-running")

    #: The core's per-connection petition bucket, mirrored here so startup does
    #: not walk into it: burst 8, refill 1/s
    #: (`crates/vitrin-core/src/principal.rs`, `PETITION_RATE_BURST` /
    #: `PETITION_REFILL_PER_SEC`). Exceeding it is **fatal**
    #: `resource_exhausted`, not a recoverable refusal — the connection dies —
    #: so a shell started with nine or more bindings used to die mid-startup
    #: with a traceback and never print `ready`.
    #:
    #: Pacing to avoid a FATAL error is not the same thing as retrying a
    #: REFUSAL, which this shell still never does (decision 5). The difference
    #: is that a refusal is the core's answer and belongs on screen, while this
    #: is the client declining to ask a question it already knows the transport
    #: will kill it for. It is announced rather than silent, for the same reason.
    _PETITION_BURST = 8
    _PETITION_REFILL_SECONDS = 1.05

    def start(self) -> None:
        """Petition once per realm, once for arrangement, once per template."""
        self._say(
            "this shell draws nothing and reads no input but this terminal's stdin; "
            "see README.md for why neither is a missing feature"
        )
        pending = list(self.focus_bindings.values())
        if self.arrange is not None:
            pending.append(self.arrange)
        pending.extend(self.launch_bindings.values())

        if len(pending) > self._PETITION_BURST:
            self._say(
                f"{len(pending)} petitions to raise but the core's burst is "
                f"{self._PETITION_BURST} (refill "
                f"{self._PETITION_REFILL_SECONDS:.0f}/s): pacing the rest, so startup "
                f"takes about {len(pending) - self._PETITION_BURST} extra second(s). "
                "Exceeding the bucket is fatal, not a refusal."
            )
        for i, binding in enumerate(pending):
            if i >= self._PETITION_BURST:
                time.sleep(self._PETITION_REFILL_SECONDS)
            try:
                self._petition(binding)
            except errors.ResourceExhausted:
                # Reachable only if the pacing above is wrong (a shared
                # connection, a changed core constant). Say so in the shell's
                # own vocabulary instead of dying with a traceback, because a
                # caller driving this over a pipe otherwise just sees EOF.
                self._status(
                    "error start resource-exhausted — the core's petition bucket is "
                    "empty and the connection is gone. Restart with fewer --realm / "
                    "--template bindings, or stagger them."
                )
                raise
        self._status(
            f"ready {len(self.focus_bindings)} realm(s), "
            f"{len(self.launch_bindings)} template(s)"
        )

    # -- commands ----------------------------------------------------------

    def execute(self, line: str) -> None:
        """Run one command line. Always ends by printing exactly one status."""
        parts = line.split()
        if not parts:
            self._status("ok noop")
            return
        verb, args = parts[0], parts[1:]
        handler = {
            "list": self._cmd_list,
            "focus": self._cmd_focus,
            "launch": self._cmd_launch,
            "fullscreen": self._cmd_fullscreen,
            "help": self._cmd_help,
            "quit": self._cmd_quit,
        }.get(verb)
        if handler is None:
            self._status(f"error unknown-command {verb}")
            return
        try:
            handler(args)
        except errors.GrantRefused as exc:
            for note in _explain_refusal(exc):
                self._say(note)
            self._status(f"refused {verb} {' '.join(args)} {_refusal_code_name(exc)}".rstrip())
        except errors.GrantResolutionError as exc:
            for note in RESOLUTION_NOTES.get(type(exc), ()):
                self._say(note)
            self._status(f"error {verb} {type(exc).__name__}")

    def _cmd_help(self, _args: list[str]) -> None:
        self._say("list                 what this shell was told about, and what it holds")
        self._say("focus <realm>        bind the output to that realm; the human's input follows")
        self._say("launch <template>    fork a new realm from that template")
        self._say("fullscreen on|off    make the arrange realm fill the output, or not")
        self._say("quit                 close the connection and exit")
        self._say("")
        self._say("that is the whole vocabulary the protocol offers a layout holder.")
        self._status("ok help")

    def _cmd_list(self, _args: list[str]) -> None:
        self._say(
            "the wire carries no realm enumeration, so this is what you TOLD this "
            "shell about, not what exists. A name missing here may still be running."
        )
        for binding in self.focus_bindings.values():
            mark = " *arrange" if binding.name == self.arrange_realm else ""
            self._say(f"realm     {binding.name:<24} layout.focus: {binding.outcome}{mark}")
        if self.arrange is not None:
            self._say(f"arrange   {self.arrange.name:<24} layout.arrange: {self.arrange.outcome}")
        for binding in self.launch_bindings.values():
            self._say(f"template  {binding.name:<24} realm.launch: {binding.outcome}")
        self._say(
            f"attention presses seen: {self.conn.attention_count} "
            "(the human's key; it confers nothing and this shell acts on none of them)"
        )
        self._status(f"ok list {len(self.focus_bindings)}")

    def _cmd_focus(self, args: list[str]) -> None:
        if len(args) != 1:
            self._status("error focus usage:-focus-<realm>")
            return
        name = args[0]
        binding = self.focus_bindings.get(name)
        if binding is None or binding.grant is None:
            self._say(
                f"this shell was never told about '{name}'. Start it with --realm {name} "
                "(there is no way to ask the core what realms exist)."
            )
            self._status(f"error focus {name} unknown-realm")
            return
        binding.grant.focus()
        # Fire-and-forget: the refusal, if any, arrives at this barrier and is
        # raised as the typed exception `execute` renders. Nothing is retried.
        self.conn.sync(binding.grant)
        self._status(f"ok focus {name}")

    def _cmd_launch(self, args: list[str]) -> None:
        if len(args) != 1:
            self._status("error launch usage:-launch-<template>")
            return
        name = args[0]
        binding = self.launch_bindings.get(name)
        if binding is None or binding.grant is None:
            self._say(f"this shell holds no realm.launch grant over '{name}'.")
            self._status(f"error launch {name} unknown-template")
            return
        realm = binding.grant.launch()
        self._say(f"the core minted the realm id '{realm}'; it was not this shell's to choose.")
        self._say(
            "a launch confers nothing over what it launched, so focusing it needs its "
            "own petition — raising one now."
        )
        new = Binding(realm, "launched", FOCUS_VERBS)
        self.focus_bindings[realm] = new
        self._petition(new)
        self._status(f"ok launch {name} {realm}")

    def _cmd_fullscreen(self, args: list[str]) -> None:
        if len(args) != 1 or args[0] not in ("on", "off"):
            self._status("error fullscreen usage:-fullscreen-on|off")
            return
        if self.arrange is None or self.arrange.grant is None:
            self._say("this shell holds no layout.arrange petition; start it with --arrange.")
            self._status("error fullscreen no-arrange-realm")
            return
        self._say(
            f"arrangement applies to '{self.arrange.name}' and to no other realm: the "
            "grant names one realm, and only one grant per output may hold the verb."
        )
        self._say(
            "at equal output and realm sizes the two modes are indistinguishable — the "
            "protocol says so in as many words, so expect to see nothing change."
        )
        self.arrange.grant.set_fullscreen(args[0] == "on")
        self.conn.sync(self.arrange.grant)
        self._status(f"ok fullscreen {args[0]}")

    def _cmd_quit(self, _args: list[str]) -> None:
        self.running = False
        self._status("ok quit")

    # -- the loop ----------------------------------------------------------

    def repl(self, stream: TextIO, *, prompt: bool) -> None:
        while self.running:
            if prompt:
                print("vitrin> ", end="", file=self.out, flush=True)
            line = stream.readline()
            if not line:  # EOF: the terminal closed, or the pipe ended.
                self._status("ok eof")
                return
            self.execute(line.strip())


def _verb_names(bits) -> set[str]:
    from vitrin_os import protocol

    return {name for name, bit in protocol.VERB_BY_DOTTED_NAME.items() if int(bits) & int(bit)}


# --- entry point -------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run_shell.py",
        description=(
            "A line-oriented switcher and launcher that is an ordinary Vitrin SDK "
            "client. It draws nothing and reads no input but this terminal's stdin."
        ),
    )
    parser.add_argument("--socket", default=os.environ.get("VITRIN_SOCKET", ""))
    parser.add_argument(
        "--identity", default=os.environ.get("VITRIN_IDENTITY", "vitrin://local/agent/demo")
    )
    parser.add_argument("--token", default=os.environ.get("VITRIN_TOKEN", ""))
    parser.add_argument(
        "--realm",
        action="append",
        default=[],
        metavar="NAME",
        help=(
            "a realm to hold layout.focus over. Repeatable. Required because the "
            "wire has no realm enumeration: this is not an ergonomic shortcut."
        ),
    )
    parser.add_argument(
        "--template",
        action="append",
        default=[],
        metavar="NAME",
        help="a realm template to hold realm.launch over. Repeatable; one grant each.",
    )
    parser.add_argument(
        "--arrange",
        default=None,
        metavar="NAME",
        help=(
            "the one realm to hold layout.arrange over (default: the first --realm). "
            "One, because D-018(4) makes arrangement single-holder per output."
        ),
    )
    return parser


def run(argv: list[str] | None = None, *, stdin: TextIO | None = None, out: TextIO | None = None) -> int:
    args = build_parser().parse_args(argv)
    out = out or sys.stdout
    stream = stdin or sys.stdin
    if not args.socket:
        print("error no-socket (pass --socket or set VITRIN_SOCKET)", file=out, flush=True)
        return 2
    realms = args.realm or ["realm-0"]
    conn = vitrin_os.connect(
        args.socket,
        identity=args.identity,
        credential_type="static-token",
        credential=args.token,
    )
    shell = Shell(
        conn,
        realms=realms,
        templates=args.template,
        arrange=args.arrange,
        out=out,
    )
    try:
        shell.start()
        shell.repl(stream, prompt=stream.isatty())
    finally:
        conn.close()
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(run())
