# SPDX-License-Identifier: Apache-2.0
"""The in-realm half of the **principal-socket reachability measurement** (#311).

This file is the realm's *app*. It is copied — never bound from the checkout —
into a scratch directory that the realm's `binds` list names, so the only thing
this measurement adds to a realm is the adversary's own code, which every realm
has by construction. It writes one line-oriented report and then holds the realm
open so the core does not tear the run down underneath the reader.

**It decides nothing.** Every line here is an observation with its errno; the
verdicts live in `test_real_principal_socket_reach.py`, which reads this report
from the host side and compares the confined run against the `--isolation=off`
run of the same argv. A probe that concluded on its own behalf would be a probe
whose failure mode is a confident wrong answer.

Report grammar — one record per line, `KIND field=value ...`, every free-form
value base64 (no whitespace, no quoting rules, no locale). `P311-END` is written
last and is the reader's completeness marker, exactly as `PROBE-END` is for
`test_real_confinement.py`.
"""

from __future__ import annotations

import base64
import errno
import os
import socket
import stat
import sys
import time

VERSION = 1

#: How much of a walked filesystem is enough. A realm root is a few thousand
#: inodes; the cap exists so a mount table that accidentally exposed a real
#: filesystem cannot turn the probe into an unbounded crawl. `truncated=1` on
#: `P311-WALK-DONE` says the cap was hit, so a reader can never mistake a
#: bounded walk for an exhaustive one.
WALK_LIMIT = 60000

#: Trees the walk records but does not descend. `/proc` and `/sys` are
#: enumerated deliberately elsewhere; `/dev` holds device nodes a `stat` can
#: block on; `/usr` and `/etc` are the host's own read-only system trees, bound
#: identically in both isolation settings, and crawling half a million inodes
#: there would spend the whole budget somewhere the core socket cannot be.
WALK_SKIP = ("/proc", "/sys", "/dev", "/usr", "/etc")

#: Seed directories for the walk, beside every mount point in this namespace.
#: A seed list is required and not a convenience: a confined realm's Landlock
#: domain denies `READ_DIR` on `/` itself, so a walk that started only at the
#: root would report "no sockets found" having enumerated nothing at all --
#: which is the vacuous negative this whole file exists to avoid.
WALK_SEEDS = (
    "/",
    "/run",
    "/run/vitrin",
    "/run/user",
    "/tmp",
    "/vitrin",
    "/vitrin/home",
    "/dev/shm",
    "/home",
    "/var",
)


def b64(value) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        value = value.encode("utf-8", "surrogateescape")
    return base64.b64encode(value).decode("ascii")


class Report:
    def __init__(self, path: str) -> None:
        self.fh = open(path, "w", encoding="utf-8")

    def emit(self, record: str, **fields) -> None:
        parts = [record]
        for key, value in fields.items():
            parts.append(f"{key}={value}")
        self.fh.write(" ".join(parts) + "\n")
        self.fh.flush()

    def close(self) -> None:
        self.fh.close()


def errno_of(exc: OSError) -> int:
    return exc.errno if exc.errno is not None else 0


# -- observations -----------------------------------------------------------


def observe_self(rep: Report) -> None:
    try:
        cwd = os.getcwd()
    except OSError as exc:
        cwd = f"<errno {errno_of(exc)}>"
    rep.emit(
        "P311-SELF",
        pid=os.getpid(),
        uid=os.getuid(),
        euid=os.geteuid(),
        gid=os.getgid(),
        groups=",".join(str(g) for g in os.getgroups()),
        hostname=b64(socket.gethostname()),
        cwd=b64(cwd),
        executable=b64(sys.executable),
    )


def observe_env(rep: Report) -> None:
    """The composed environment, twice: as the process sees it and as the
    kernel holds it.

    Both, because they can disagree — anything this interpreter set for itself
    would show in `os.environ` and not in `/proc/self/environ`, and the question
    is what the *core* handed the app.
    """
    for name, value in sorted(os.environ.items()):
        rep.emit("P311-ENV", name=b64(name), value=b64(value))
    rep.emit("P311-ENV-COUNT", n=len(os.environ))
    try:
        with open("/proc/self/environ", "rb") as fh:
            raw = fh.read()
        rep.emit("P311-PROC-ENVIRON", ok=1, errno=0, raw=b64(raw))
    except OSError as exc:
        rep.emit("P311-PROC-ENVIRON", ok=0, errno=errno_of(exc), raw="")


def observe_namespaces(rep: Report) -> None:
    for kind in ("user", "mnt", "pid", "ipc", "uts", "net", "cgroup", "time", "pid_for_children"):
        try:
            link = os.readlink(f"/proc/self/ns/{kind}")
            rep.emit("P311-NS", kind=kind, ok=1, errno=0, link=b64(link))
        except OSError as exc:
            rep.emit("P311-NS", kind=kind, ok=0, errno=errno_of(exc), link="")


def observe_file(rep: Report, kind: str, path: str) -> None:
    try:
        with open(path, "rb") as fh:
            raw = fh.read()
        rep.emit(kind, ok=1, errno=0, raw=b64(raw))
    except OSError as exc:
        rep.emit(kind, ok=0, errno=errno_of(exc), raw="")


def observe_fds(rep: Report) -> None:
    """Every descriptor that survived the `execve`, and what it is.

    `spawn.rs` closes the core's descriptors `CLOSE_RANGE_CLOEXEC` after the
    fork, so the expectation is stdio and whatever this interpreter opened. The
    measurement is what makes that an observation instead of a citation: a
    socket here would be a route no path check would ever find.
    """
    try:
        names = sorted(int(n) for n in os.listdir("/proc/self/fd") if n.isdigit())
    except OSError as exc:
        rep.emit("P311-FD-ERROR", errno=errno_of(exc))
        return
    rep.emit("P311-FD-COUNT", n=len(names))
    for fd in names:
        try:
            target = os.readlink(f"/proc/self/fd/{fd}")
        except OSError as exc:
            target = f"<errno {errno_of(exc)}>"
        try:
            st = os.fstat(fd)
        except OSError as exc:
            rep.emit("P311-FD", fd=fd, target=b64(target), issock=-1, errno=errno_of(exc))
            continue
        is_sock = 1 if stat.S_ISSOCK(st.st_mode) else 0
        sockname = peername = family = ""
        if is_sock:
            try:
                dup = os.dup(fd)
                sock = socket.socket(fileno=dup)
                family = str(int(sock.family))
                try:
                    sockname = b64(str(sock.getsockname()))
                except OSError:
                    sockname = ""
                try:
                    peername = b64(str(sock.getpeername()))
                except OSError:
                    peername = ""
                sock.detach()
                os.close(dup)
            except OSError:
                pass
        rep.emit(
            "P311-FD",
            fd=fd,
            target=b64(target),
            issock=is_sock,
            errno=0,
            dev=st.st_dev,
            ino=st.st_ino,
            family=family,
            sockname=sockname,
            peername=peername,
        )


#: How many `/proc/<pid>/fd` tables to report. The confined realm has two
#: processes; an unconfined one shares the host's procfs and a full dump would
#: be tens of thousands of lines of the operator's own session.
PEER_FD_BUDGET = 40


_peer_fd_budget = [PEER_FD_BUDGET]


def observe_peer_fds(rep: Report, pid: int) -> None:
    """Another process's descriptor table, and whether any of it is usable here.

    **The sharpest same-realm route, and the one no path check finds.** The
    realm's shim is the app's own parent and holds a live connection to the core
    on an inherited socketpair (`SHIM_CORE_FD`); the app runs as the same uid, so
    `/proc/<shim>/fd` is a directory it may be allowed to read. A socket cannot
    be re-opened through procfs -- Linux answers `ENXIO` -- but "cannot" is the
    kind of claim this file exists to measure rather than repeat, so every
    socket descriptor found is `open`ed and the errno recorded.

    Not the principal socket either way: the shim's channel speaks the
    core-to-shim protocol, not `vitrin_handshake`. It is reported because #311
    asks what a realm can reach *of the core*, and a reader deciding that
    question should see this seam rather than discover it afterwards.
    """
    if _peer_fd_budget[0] <= 0:
        return
    try:
        fds = sorted(int(n) for n in os.listdir(f"/proc/{pid}/fd") if n.isdigit())
    except OSError as exc:
        rep.emit("P311-PEER-FD-DENIED", pid=pid, errno=errno_of(exc))
        return
    _peer_fd_budget[0] -= 1
    rep.emit("P311-PEER-FD-COUNT", pid=pid, n=len(fds))
    for fd in fds:
        path = f"/proc/{pid}/fd/{fd}"
        try:
            target = os.readlink(path)
            link_errno = 0
        except OSError as exc:
            target = ""
            link_errno = errno_of(exc)
        open_ok, open_errno = 0, 0
        if target.startswith("socket:"):
            try:
                handle = os.open(path, os.O_RDWR)
                open_ok = 1
                os.close(handle)
            except OSError as exc:
                open_errno = errno_of(exc)
        rep.emit(
            "P311-PEER-FD",
            pid=pid,
            fd=fd,
            target=b64(target),
            link_errno=link_errno,
            reopen=open_ok,
            reopen_errno=open_errno,
        )


def observe_pids(rep: Report, core_sock: str) -> None:
    """Which processes this realm can see, and whether any of them is a door.

    `/proc/<pid>/root` is the classic way out of a mount namespace when the pid
    namespace is shared and the target process is same-uid: it is a magic link
    the kernel resolves in *that* process's mount namespace, so a host pid whose
    root contains the core socket makes the socket reachable without any mount
    of ours. Under `--isolation=off` the realm shares the host's pid namespace
    and this is expected to work; under `default` the fresh procfs should hold
    nothing but the realm's own processes.
    """
    try:
        pids = sorted(int(n) for n in os.listdir("/proc") if n.isdigit())
    except OSError as exc:
        rep.emit("P311-PID-ERROR", errno=errno_of(exc))
        return
    rep.emit("P311-PID-COUNT", n=len(pids))
    reached = 0
    for pid in pids:
        try:
            with open(f"/proc/{pid}/comm", "rb") as fh:
                comm = fh.read().strip()
        except OSError as exc:
            comm = f"<errno {errno_of(exc)}>".encode()
        try:
            root_link = os.readlink(f"/proc/{pid}/root")
            root_errno = 0
        except OSError as exc:
            root_link = ""
            root_errno = errno_of(exc)
        try:
            cwd_link = os.readlink(f"/proc/{pid}/cwd")
            cwd_errno = 0
        except OSError as exc:
            cwd_link = ""
            cwd_errno = errno_of(exc)
        via = f"/proc/{pid}/root{core_sock}"
        stat_ok, stat_errno = 0, 0
        conn_ok, conn_errno = 0, 0
        try:
            st = os.stat(via)
            stat_ok = 1 if stat.S_ISSOCK(st.st_mode) else 2
        except OSError as exc:
            stat_errno = errno_of(exc)
        if stat_ok == 1:
            conn_ok, conn_errno = try_connect(via)
            if conn_ok:
                reached += 1
        rep.emit(
            "P311-PID",
            pid=pid,
            comm=b64(comm),
            root=b64(root_link),
            root_errno=root_errno,
            cwd=b64(cwd_link),
            cwd_errno=cwd_errno,
            via_stat=stat_ok,
            via_stat_errno=stat_errno,
            via_connect=conn_ok,
            via_connect_errno=conn_errno,
        )
        observe_peer_fds(rep, pid)
    rep.emit("P311-PID-REACHED", n=reached)


def observe_abstract(rep: Report) -> None:
    """The abstract Unix namespace, which is scoped by the **network**
    namespace and by nothing else.

    `/proc/net/unix` is itself netns-scoped, so its contents are the measurement
    of whether this realm shares the host's namespace: a realm in its own netns
    sees only sockets its own processes made. Every abstract name found is then
    connected to, because a name being listed is not the same as it being
    reachable and the question here is reachability.
    """
    lines: list[bytes] = []
    try:
        with open("/proc/net/unix", "rb") as fh:
            raw = fh.read()
        lines = raw.splitlines()
        rep.emit("P311-PROC-NET-UNIX", ok=1, errno=0, raw=b64(raw))
    except OSError as exc:
        rep.emit("P311-PROC-NET-UNIX", ok=0, errno=errno_of(exc), raw="")
    names: list[str] = []
    for line in lines[1:]:
        fields = line.split()
        if len(fields) >= 8:
            name = fields[7].decode("utf-8", "replace")
            if name.startswith("@"):
                names.append(name)
    rep.emit("P311-ABSTRACT-COUNT", n=len(names))
    for name in sorted(set(names)):
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(2.0)
        try:
            sock.connect("\0" + name[1:])
            rep.emit("P311-ABSTRACT", name=b64(name), connect=1, errno=0)
        except OSError as exc:
            rep.emit("P311-ABSTRACT", name=b64(name), connect=0, errno=errno_of(exc))
        finally:
            sock.close()


def is_skipped(path: str) -> bool:
    """Is `path` inside a :data:`WALK_SKIP` tree (or one of them exactly)?

    Applied to seeds as well as to children, because a `WALK_SKIP` tree that is
    also a mount point would otherwise enter the walk as a seed and bypass the
    child-side skip — which is how `/proc` came to be crawled to the budget cap.
    """
    return any(path == skip or path.startswith(skip + "/") for skip in WALK_SKIP)


def walk_seeds() -> list[str]:
    """Where the walk starts: every mount point in this namespace, plus
    :data:`WALK_SEEDS`, minus the :data:`WALK_SKIP` trees.

    Mount points come from `/proc/self/mountinfo` rather than from a list,
    because the question "what does the mount table happen to expose" is
    answered by the mount table and not by what the probe's author expected it
    to contain.
    """
    roots = set(WALK_SEEDS)
    try:
        with open("/proc/self/mountinfo", "rb") as fh:
            for line in fh:
                fields = line.split()
                if len(fields) >= 5:
                    roots.add(fields[4].decode("unicode_escape"))
    except OSError:
        pass
    return sorted(r for r in roots if not is_skipped(r))


def observe_walk(rep: Report) -> None:
    """Every AF_UNIX socket inode reachable by *walking* the realm's tree.

    This is the route no candidate list can cover: "any path the mount table
    happens to expose". A named-path probe answers the paths someone thought of;
    the walk answers the question those paths are a proxy for.

    Every directory that refuses enumeration is reported with its errno, so the
    difference between "there is nothing here" and "I was not allowed to look"
    is in the record rather than in the reader's assumption.
    """
    seen = 0
    found = 0
    seeds = walk_seeds()
    rep.emit("P311-WALK-SEEDS", n=len(seeds), roots=b64("\n".join(seeds)))
    seed_set = set(seeds)
    stack = list(reversed(seeds))
    visited: set[tuple[int, int]] = set()
    while stack and seen < WALK_LIMIT:
        current = stack.pop()
        try:
            dir_st = os.stat(current)
            key = (dir_st.st_dev, dir_st.st_ino)
            if key in visited:
                continue
            visited.add(key)
        except OSError as exc:
            rep.emit("P311-WALK-DENIED", path=b64(current), errno=errno_of(exc))
            continue
        try:
            entries = list(os.scandir(current))
        except OSError as exc:
            rep.emit("P311-WALK-DENIED", path=b64(current), errno=errno_of(exc))
            continue
        if current in seed_set:
            # Only the seeds, so a reader can tell an enumerated root from a
            # refused one without the record carrying one line per directory of
            # an unconfined host's filesystem.
            rep.emit("P311-WALK-DIR", path=b64(current), entries=len(entries))
        for entry in entries:
            seen += 1
            if seen >= WALK_LIMIT:
                break
            path = entry.path
            if is_skipped(path):
                rep.emit("P311-WALK-SKIP", path=b64(path))
                continue
            try:
                st = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            if stat.S_ISSOCK(st.st_mode):
                found += 1
                conn_ok, conn_errno = try_connect(path)
                rep.emit(
                    "P311-WALK-SOCKET",
                    path=b64(path),
                    dev=st.st_dev,
                    ino=st.st_ino,
                    connect=conn_ok,
                    errno=conn_errno,
                )
            elif stat.S_ISDIR(st.st_mode):
                stack.append(path)
    rep.emit("P311-WALK-DONE", seen=seen, sockets=found, truncated=1 if seen >= WALK_LIMIT else 0)


# -- the candidate paths ----------------------------------------------------


def try_connect(path: str) -> tuple[int, int]:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(3.0)
    try:
        sock.connect(path)
        return 1, 0
    except OSError as exc:
        return 0, errno_of(exc)
    finally:
        sock.close()


def candidates(core_sock: str) -> list[tuple[str, str]]:
    """`(label, path)` for every route a program inside a realm could name.

    Deliberately un-normalised: the `..` components are handed to the kernel
    as-is, because the question is what *path resolution in this mount
    namespace* does with them, and `os.path.normpath` would answer a different
    question in userspace.
    """
    xdg = os.environ.get("XDG_RUNTIME_DIR", "")
    home = os.environ.get("HOME", "")
    wayland = os.environ.get("WAYLAND_DISPLAY", "")
    uid = os.getuid()
    out: list[tuple[str, str]] = []
    # The in-realm positive control. The app is *given* this socket, so it must
    # connect in both runs; without it a report of "nothing connected" would be
    # satisfied by a probe whose socket layer never worked at all.
    if wayland:
        out.append(("wayland-display", wayland))
        out.append(("wayland-dotdot", os.path.dirname(wayland) + "/../core.sock"))
    if xdg:
        out.append(("xdg-dotdot", xdg + "/../core.sock"))
        out.append(("xdg-dotdot-twice", xdg + "/../../vitrin-0/core.sock"))
        out.append(("xdg-dotdot-above-root", xdg + "/../../../../../../../../core.sock"))
        out.append(("xdg-sibling", xdg + "/core.sock"))
    if home:
        out.append(("home-dotdot", home + "/../core.sock"))
    out.append(("cwd-dotdot", "../core.sock"))
    out.append(("core-sock-host-absolute", core_sock))
    out.append(("xdg-convention", f"/run/user/{uid}/vitrin-0/core.sock"))
    out.append(("in-realm-runtime-dotdot", "/run/vitrin/../core.sock"))
    out.append(("in-realm-runtime-sibling", "/run/vitrin-0/core.sock"))
    out.append(("vitrin-prefix", "/vitrin/core.sock"))
    out.append(("proc-self-root", "/proc/self/root" + core_sock))
    out.append(("proc-1-root", "/proc/1/root" + core_sock))
    return out


def observe_paths(rep: Report, core_sock: str) -> list[tuple[str, str]]:
    connectable: list[tuple[str, str]] = []
    for label, path in candidates(core_sock):
        try:
            st = os.stat(path)
            rep.emit(
                "P311-PATH",
                label=label,
                path=b64(path),
                stat=1,
                errno=0,
                dev=st.st_dev,
                ino=st.st_ino,
                issock=1 if stat.S_ISSOCK(st.st_mode) else 0,
                mode=oct(stat.S_IMODE(st.st_mode)),
            )
        except OSError as exc:
            rep.emit(
                "P311-PATH", label=label, path=b64(path), stat=0, errno=errno_of(exc),
                dev=0, ino=0, issock=0, mode="0",
            )
        ok, err = try_connect(path)
        rep.emit("P311-CONNECT", label=label, path=b64(path), connect=ok, errno=err)
        if ok:
            connectable.append((label, path))
    return connectable


# -- the handshake ----------------------------------------------------------


def observe_handshake(rep: Report, connectable, identity: str, token: str, bad: str) -> None:
    """How far a reachable socket actually gets.

    Reachable-but-rejected and reachable-and-accepted are different answers, so
    a path that connects is driven all the way through the shipped SDK's
    `hello`, once with the deployment's real credential and once with a wrong
    one. The wrong-credential arm is the control that makes the accepted arm
    mean something: a core that bound *anything* would bind both.
    """
    try:
        import vitrin_os
    except Exception as exc:  # pragma: no cover - reported, never raised
        rep.emit("P311-SDK", ok=0, detail=b64(f"{type(exc).__name__}: {exc}"))
        return
    rep.emit("P311-SDK", ok=1, detail=b64(getattr(vitrin_os, "__file__", "?")))
    for label, path in connectable:
        if label.startswith("wayland"):
            # The shim's own Wayland socket. Connecting to it is the control
            # for the socket layer; speaking Vitrin at it is not a measurement
            # of anything and would only confuse libwayland's accept loop.
            rep.emit("P311-HANDSHAKE", label=label, cred="skipped", result="skipped", detail="")
            continue
        for cred_name, cred in (("good", token), ("bad", bad)):
            try:
                conn = vitrin_os.connect(
                    path, identity=identity, credential=cred, timeout=10.0
                )
            except Exception as exc:
                rep.emit(
                    "P311-HANDSHAKE",
                    label=label,
                    cred=cred_name,
                    result="refused",
                    detail=b64(f"{type(exc).__name__}: {exc}"),
                )
                continue
            try:
                bound = conn.identity
            except Exception as exc:
                bound = f"<{type(exc).__name__}: {exc}>"
            rep.emit(
                "P311-HANDSHAKE",
                label=label,
                cred=cred_name,
                result="bound",
                detail=b64(bound),
            )
            try:
                conn.close()
            except Exception:
                pass


# -- entry point ------------------------------------------------------------


def parse_args(argv: list[str]) -> dict:
    out = {
        "out": "p311-probe.txt",
        "core-sock": "",
        "identity": "",
        "token": "",
        "bad-token": "",
        "hold-ms": "20000",
    }
    i = 0
    while i < len(argv):
        key = argv[i]
        if not key.startswith("--"):
            i += 1
            continue
        name = key[2:]
        if name in out and i + 1 < len(argv):
            out[name] = argv[i + 1]
            i += 2
        else:
            i += 1
    return out


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    base = os.environ.get("XDG_RUNTIME_DIR") or os.getcwd()
    out_path = os.path.join(base, args["out"])
    rep = Report(out_path)
    rep.emit("P311-BEGIN", version=VERSION, out=b64(out_path))
    try:
        observe_self(rep)
        observe_env(rep)
        observe_namespaces(rep)
        observe_file(rep, "P311-MOUNTINFO", "/proc/self/mountinfo")
        observe_file(rep, "P311-MOUNTS", "/proc/mounts")
        observe_fds(rep)
        observe_abstract(rep)
        observe_pids(rep, args["core-sock"])
        connectable = observe_paths(rep, args["core-sock"])
        observe_walk(rep)
        observe_handshake(
            rep, connectable, args["identity"], args["token"], args["bad-token"]
        )
    except BaseException as exc:  # the report must say it died, not go quiet
        rep.emit("P311-ABORT", detail=b64(f"{type(exc).__name__}: {exc}"))
        rep.emit("P311-END", ok=0)
        rep.close()
        raise
    rep.emit("P311-END", ok=1)
    rep.close()
    # Hold the realm open. The app exiting is a realm ending, and a reader that
    # raced the teardown would be reading a tree the core had already purged.
    hold = int(args["hold-ms"]) / 1000.0
    deadline = time.monotonic() + hold
    while time.monotonic() < deadline:
        time.sleep(0.2)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
