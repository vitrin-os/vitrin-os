#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Spawn one real realm and report which AppArmor label every process in the
spine ended up with (issue #286).

**Not a test module, and named so it cannot become one.** The hyphen in the
filename makes it unimportable, so `unittest discover -p 'test_*.py'` cannot
collect it and `tests/integration/run.sh`'s three lists have nothing to say
about it. The precedent is `tests/integration/landlock-denials.sh`: a
diagnostic that drives the shipped binaries, invoked by name from one CI step,
and not part of any suite's coverage claim.

## What question this answers, and what it deliberately does not

Ubuntu's per-binary user-namespace grant is an AppArmor profile attached to a
**path**. `vitrind` does not issue the `unshare` itself — it `execve`s
`vitrin-realm-init`, which does. So the question that decides whether
`packaging/apparmor/vitrind` is worth shipping is not "does the core start"
but "did the grant survive the exec into the helper", and nothing the core
prints about itself can answer it. Only a realm actually spawning can.

So this boots one core with `--isolation=default` and waits for a shim to
appear two levels down. That path runs the whole confinement chain: the
six-flag `unshare`, the `MS_REC|MS_PRIVATE` remount that is what fails on an
unprofiled Ubuntu host, `pivot_root`, the Landlock ruleset, `PR_SET_NO_NEW_PRIVS`
and the final `execve` into the shim.

What it does **not** prove: that a real Wayland application renders inside that
realm. The shim here is `vitrin-mock-shim`. That is a deliberate division of
labour with the CI job that calls this — `test_real_confinement.py` runs the
real C shim and a real client under the same profile in the same job, and it is
that gate, not this script, which is evidence about a working realm. This
script exists because it is cheap enough to run **twice**, which is what makes
it usable as the lever: remove the profile, run it, require the failure.

## Why the label of every process is printed rather than just the core's

Four outcomes produce three distinct errnos between them, and two of them are
indistinguishable without the labels:

* the profile did not attach at all — every label reads `unconfined`;
* it attached to `vitrind` only — the core's label names `vitrind`, the
  helper's does not, and the spawn dies at the helper's `unshare`;
* it attached to both and granted nothing — both labels name `vitrind`, and the
  spawn dies at the mount with `EACCES`, which is byte-identical to having no
  profile at all;
* it worked.

The third case is not hypothetical, and it is why the labels are non-optional.
Ubuntu 24.04 ships a fallback profile, `unprivileged_userns`, that carries
``allow userns,`` together with ``audit deny capability,`` — it permits the
unshare and denies every capability inside it, which is *exactly* issue #286's
measured signature. So a wrong profile fails in the same errno as no profile,
and only the label separates them.

Labels are matched by NAME, never by equality against ``vitrind``. A label is a
profile name plus an optional ``(mode)`` suffix, and under this profile's flag
the kernel is expected to render ``vitrind (unconfined)`` — the string
``crates/vitrin-core/src/spawn/isolation.rs`` pins in its own unit test.

`/proc/<pid>/attr/apparmor/current` is read rather than
`/proc/<pid>/attr/current` wherever it exists, for the reason
`crates/vitrin-core/src/spawn/isolation.rs` states: the unqualified file
belongs to whichever LSM is active and returns an SELinux context on a
SELinux machine.
"""

from __future__ import annotations

import argparse
import os
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from harness import (  # noqa: E402  (path insert must precede the import)
    VITRIND,
    Core,
    CoreFailed,
    await_shims,
    children_of,
    comm_of,
)

#: What issue #286 measured on a stock `ubuntu-latest`, as it appears in the
#: core's own refusal. Both halves are required of a "profile absent" run: the
#: mechanism the preflight names, and the fact that it was a *policy*
#: restriction rather than the mechanism being absent outright. A run that
#: failed for any other reason is not the control this script claims to be.
REFUSAL_MARKERS = ("namespaces", "restricted-by-policy")


def label_of(pid: int) -> str:
    """`pid`'s AppArmor label, or a token saying why there is none."""
    for path in (f"/proc/{pid}/attr/apparmor/current", f"/proc/{pid}/attr/current"):
        if path.endswith("attr/current") and not _apparmor_is_the_lsm():
            # Same trap the core's own reader avoids: on a SELinux box this
            # file answers with an SELinux context, and printing that under an
            # "AppArmor label" heading would invent a confinement nobody has.
            continue
        try:
            raw = pathlib.Path(path).read_text()
        except OSError:
            continue
        cleaned = raw.strip("\0 \t\r\n")
        if cleaned:
            return cleaned
    return "<absent>"


def _apparmor_is_the_lsm() -> bool:
    try:
        return pathlib.Path("/sys/module/apparmor/parameters/enabled").read_text().strip() == "Y"
    except OSError:
        return False


def describe_spine(core_pid: int) -> None:
    """Print `comm` and label for the core, its helper, and the shim."""
    print(f"  core          pid={core_pid} comm={comm_of(core_pid)!r} label={label_of(core_pid)}")
    for kid in children_of(core_pid):
        print(f"  child         pid={kid} comm={comm_of(kid)!r} label={label_of(kid)}")
        for grandkid in children_of(kid):
            print(
                f"    grandchild  pid={grandkid} comm={comm_of(grandkid)!r} "
                f"label={label_of(grandkid)}"
            )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--expect",
        choices=("spawn", "refusal"),
        required=True,
        help=(
            "`spawn`: a realm must reach a shim (the profile is loaded and works). "
            "`refusal`: the core must refuse to start with issue #286's signature "
            "(the profile is removed -- this is the lever)."
        ),
    )
    args = parser.parse_args()

    print(f"==> vitrind under test: {VITRIND}")
    print(f"==> this process's own label: {label_of(os.getpid())}")

    try:
        core = Core(isolation="default")
    except CoreFailed as exc:
        text = str(exc)
        print("==> the core did NOT start. Its own words:")
        print(text)
        if args.expect == "refusal":
            missing = [m for m in REFUSAL_MARKERS if m not in text]
            if missing:
                print(
                    f"VERDICT: refusal-wrong-shape (expected markers absent: {missing}). "
                    "The core refused, but not for the reason this lever is a control for, "
                    "so this run does not establish that the profile was what granted the "
                    "namespace."
                )
                return 1
            print(
                "VERDICT: refusal -- the core refused to start with issue #286's signature, "
                "which is what the profile's absence is supposed to produce."
            )
            return 0
        print("VERDICT: no-start -- the core refused to start with the profile loaded.")
        return 1

    with core:
        shims = await_shims(core.pid, 1)
        # A moment for the helper's own `execve` to settle before the labels
        # are read; `await_shims` already waits for it, and this only makes a
        # failing run's output less confusing.
        time.sleep(0.1)
        print("==> process spine and labels:")
        describe_spine(core.pid)

    # OUTSIDE the `with`, and that is not a style choice. `Core.output()`
    # returns `""` while the core is still running -- it only reads the log
    # back once the process has exited, which `Core.__exit__` arranges by
    # terminating and then snapshotting. Printing it from inside the block
    # therefore printed an EMPTY log in the one branch that exists to be read,
    # so a failed profile produced a verdict naming three possible causes and
    # no evidence for choosing between them. The log IS this script's
    # deliverable on a failing run.
    log = core.output()

    if args.expect == "refusal":
        # The core STARTED. For this direction that is already the answer, and
        # it is a failure of the lever whether or not a realm went on to spawn.
        #
        # The previous version returned 0 here whenever no shim appeared, on
        # the reasoning that confinement was absent either way. That cannot
        # tell the two cases apart that this whole job exists to separate:
        # Ubuntu's shipped fallback profile `unprivileged_userns` ALLOWS the
        # unshare and carries `audit deny capability,`, so a task under it
        # reproduces issue #286's EACCES exactly. A WRONG profile therefore
        # fails identically to NO profile, and a lever that accepts "the realm
        # did not spawn" as its control would score both as `refused`.
        #
        # The control this script claims to be is narrower: with the profile
        # removed, the core must not get past its own preflight at all. Any
        # start is a result that needs explaining before anything else in the
        # run may be cited.
        print("==> the core STARTED with the profile supposedly removed. Core log:")
        print(log)
        print(
            f"VERDICT: started-anyway -- expected the core to refuse at its preflight and it "
            f"did not (a realm reached {len(shims)} shim(s)). This run is NOT the control it "
            "was invoked as, and nothing measured with the profile loaded may be attributed "
            "to the profile. Read the labels above: if they still name a profile, the removal "
            "did not take effect; if they read `unconfined`, something other than this "
            "profile is granting the namespace on this machine."
        )
        return 1

    if not shims:
        print("==> the core started but no realm reached a shim. Core log:")
        print(log)
        print(
            "VERDICT: spawn-failed -- the core cleared its own preflight and the realm "
            "did not spawn. Read the log above for the helper's Stage: `Unshare` means "
            "the grant did not reach vitrin-realm-init; a mount EACCES means it reached "
            "it and conferred nothing (the `unprivileged_userns` signature -- the profile "
            "attached and granted too little); a failure at the shim's execve is the "
            "no_new_privs transition regression."
        )
        return 1

    print(f"VERDICT: spawn -- {len(shims)} realm shim(s) running under a confined realm.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
