# SPDX-License-Identifier: Apache-2.0
"""The powerbox client: asking a human for a file, and what comes back.

Every test here drives the real `Connection` against the scripted mock core,
so what is asserted is the bytes the SDK puts on the wire and what it does
with the descriptor that comes back. Three properties get their own tests
because they are the three a caller can get wrong in a way nothing else in
this SDK punishes: the descriptor is the caller's to close, a refusal is a
value rather than an exception, and the agent's copy shares a file offset with
the realm's.
"""

from __future__ import annotations

import os

import pytest

import flows
from vitrin_os import (
    DesignatedEvent,
    DesignationKind,
    DesignationMode,
    NotGranted,
    PowerboxRefusal,
    PowerboxRefusedEvent,
    ServerContractViolation,
    Verb,
    connect,
)
from vitrin_os.messages import encode_sync

CONTENT = b"the human picked this one\n"


def _connect(server):
    return connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=5.0,
    )


def _granted_steps() -> list[tuple]:
    """Handshake + a petition for `designate.file` alone, auto-approved."""
    verbs = int(Verb.DESIGNATE_FILE)
    return [
        *flows.handshake_steps(),
        ("expect", flows.get_realm_frame()),
        ("expect", flows.request_grant_frame(verbs=verbs)),
        ("send", flows.resolved_frame(outcome=0, verbs=verbs)),
    ]


def _grant(conn):
    return conn.request_grant(verbs=Verb.DESIGNATE_FILE).await_consent()


def _file(tmp_path, name: str = "notes.txt", content: bytes = CONTENT) -> str:
    path = tmp_path / name
    path.write_bytes(content)
    return str(path)


def _fd_count() -> int:
    """Descriptors open in THIS process right now.

    `os.listdir` opens the directory it reads, and that descriptor is in the
    listing it returns — which costs nothing here, because every call pays the
    same one and the tests compare counts.
    """
    return len(os.listdir("/proc/self/fd"))


def test_request_file_mints_the_facet_once_and_hands_over_the_descriptor(
    server, tmp_path
) -> None:
    """One mint, one ask per call, and the answer's fd is a working file.

    The facet is minted lazily on first use and remembered, exactly as the
    launcher and the two layout facets are: `request_grant`'s five `new_id`
    arguments are frozen forever, so every facet added after version 1 arrives
    as its own structural mint on the grant.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=1,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            # No second mint: the facet is remembered.
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=2,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="second.txt",
                ),
                os.open(_file(tmp_path, "second.txt"), os.O_RDONLY),
            ),
            ("expect", encode_sync(1)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)

    first = conn.request_file(grant)
    assert isinstance(first, DesignatedEvent)
    assert first.designation_id == 1
    assert first.kind == DesignationKind.FILE
    assert first.mode == DesignationMode.READ
    # Display only: a basename, never a path. Nothing in this SDK resolves it.
    assert first.name == "notes.txt"
    # The descriptor is real authority over a real file, and it is ours to
    # close — nothing in this SDK will do it for us.
    assert os.pread(first.fd, len(CONTENT), 0) == CONTENT
    os.close(first.fd)

    second = conn.request_file(grant)
    assert isinstance(second, DesignatedEvent)
    assert second.designation_id == 2
    assert second.name == "second.txt"
    os.close(second.fd)

    conn.sync()
    conn.close()


def test_the_descriptor_shares_its_file_offset_with_the_other_copy(
    server, tmp_path
) -> None:
    """A read through the agent's copy moves the other copy's cursor.

    The core resolves the human's choice ONCE and sends that one descriptor
    twice — to the agent here, and to the realm's shim — and `SCM_RIGHTS`
    installs in each receiver a descriptor onto the SAME open file description.
    So the two halves share a file position, which is what the `designated`
    docstring warns about and what a caller doing sequential reads has to plan
    around.

    The mock plays the other holder: it sends a `dup` of a descriptor the test
    keeps, so `keeper` and the client's copy share one description exactly as
    the realm's copy and the agent's do. Reading through one and measuring the
    other's offset is the whole of the claim — and it also pins that this SDK
    hands over the descriptor it received rather than copying the bytes out and
    opening something else, which is what `observe()` does with a frame.
    """
    keeper = os.open(_file(tmp_path), os.O_RDONLY)
    try:
        server.run(
            [
                *_granted_steps(),
                ("expect", flows.get_powerbox_frame()),
                ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
                (
                    "send_fd",
                    flows.designated_frame(
                        designation_id=3,
                        kind=int(DesignationKind.FILE),
                        mode=int(DesignationMode.READ),
                        name="notes.txt",
                    ),
                    os.dup(keeper),
                ),
                ("linger",),
            ]
        )
        conn = _connect(server)
        grant = _grant(conn)
        assert os.lseek(keeper, 0, os.SEEK_CUR) == 0

        designation = conn.request_file(grant)
        assert isinstance(designation, DesignatedEvent)
        assert os.read(designation.fd, 4) == CONTENT[:4]
        # The other holder's cursor moved, without that holder doing anything.
        assert os.lseek(keeper, 0, os.SEEK_CUR) == 4
        # Positional I/O is the way out, and it leaves both cursors alone.
        assert os.pread(designation.fd, 4, 10) == CONTENT[10:14]
        assert os.lseek(keeper, 0, os.SEEK_CUR) == 4

        os.close(designation.fd)
        conn.close()
    finally:
        os.close(keeper)


def test_a_write_ask_answered_read_is_an_approval_not_a_refusal(
    server, tmp_path
) -> None:
    """`mode` in the answer is the EFFECTIVE access, and may be narrower.

    `write=True` is what the ask is *for* and what chrome the picker opens
    with; the human may narrow it at the card. A client that read a narrowed
    mode as failure would discard a file the human deliberately handed over
    read-only — so this is asserted rather than left to a docstring.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ_WRITE))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=4,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    designation = conn.request_file(grant, write=True)
    assert isinstance(designation, DesignatedEvent)  # an approval, not a refusal
    assert designation.mode == DesignationMode.READ
    os.close(designation.fd)
    conn.close()


def test_request_dir_takes_no_mode_and_answers_one_directory_descriptor(
    server, tmp_path
) -> None:
    """A subtree arrives as ONE directory fd, and the ask carries no arguments.

    The receiver walks it with the kernel's own `openat` from that descriptor,
    which is what makes "subtree" a containment boundary the kernel enforces
    rather than a prefix match on strings this protocol never carries.
    """
    subtree = tmp_path / "project"
    subtree.mkdir()
    (subtree / "inner.txt").write_bytes(CONTENT)
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_dir_frame()),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=5,
                    kind=int(DesignationKind.DIRECTORY),
                    mode=int(DesignationMode.READ_WRITE),
                    name="project",
                ),
                os.open(str(subtree), os.O_RDONLY | os.O_DIRECTORY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    designation = conn.request_dir(grant)
    assert isinstance(designation, DesignatedEvent)
    assert designation.kind == DesignationKind.DIRECTORY
    assert designation.mode == DesignationMode.READ_WRITE
    assert designation.name == "project"
    # A real directory descriptor: the subtree is walked from it, by the
    # kernel, without any path this connection ever carried.
    assert os.listdir(designation.fd) == ["inner.txt"]
    inner = os.open("inner.txt", os.O_RDONLY, dir_fd=designation.fd)
    try:
        assert os.pread(inner, len(CONTENT), 0) == CONTENT
    finally:
        os.close(inner)
    os.close(designation.fd)
    conn.close()


def test_a_refusal_is_a_value_and_never_an_exception(server, tmp_path) -> None:
    """A human declining to hand over a file is the system working.

    So `refused` comes back as a value: an agent that treated it as an error
    would be treating the human as a fault. The connection is untouched and the
    next ask on the same facet is served, which is what "asking again later is
    legal" means in practice.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            ("send", flows.powerbox_refused_frame(code=int(PowerboxRefusal.CANCELLED))),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=6,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)

    refusal = conn.request_file(grant)
    assert isinstance(refusal, PowerboxRefusedEvent)
    assert refusal.code == PowerboxRefusal.CANCELLED
    assert not conn.closed

    again = conn.request_file(grant)
    assert isinstance(again, DesignatedEvent)
    os.close(again.fd)
    conn.close()


def test_a_chokepoint_refusal_is_raised_typed_and_leaves_the_socket_alive(
    server, tmp_path
) -> None:
    """The third terminal, and the one that IS an exception.

    `refused(designate_file, ...)` arrives on the grant, from the enforcement
    chokepoint, and answers whether this grant may ask at all — a different
    question, asked of a different party, than the facet's own `refused`.
    Collapsing the two would make "the human said no" indistinguishable from
    "your grant expired": one is worth asking again, the other is not.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            ("send", flows.refused_frame(verb=int(Verb.DESIGNATE_FILE), code=0)),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=7,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)

    with pytest.raises(NotGranted) as excinfo:
        conn.request_file(grant)
    assert excinfo.value.verb == int(Verb.DESIGNATE_FILE)
    assert excinfo.value.grant_id == flows.GRANT_ID
    assert not conn.closed  # recoverable, like every other use-time refusal

    # And the refusal was CONSUMED: a later ask does not re-raise it.
    designation = conn.request_file(grant)
    assert isinstance(designation, DesignatedEvent)
    os.close(designation.fd)
    conn.close()


def test_both_terminals_queue_in_one_queue_in_arrival_order(server, tmp_path) -> None:
    """One queue, so a refusal cannot overtake a designation unnoticed.

    The IDL's rule is exactly one terminal per request, in request order, and
    it is checkable by a client only if the two kinds of terminal share a
    queue: two queues would deliver whichever kind the client looked at first,
    and nothing would notice the reordering.

    This SDK's blocking API sends one ask at a time, so the queue never holds
    more than one terminal through the public surface; the protocol permits
    pipelining, and this test reaches past the surface to construct the case
    the rule is actually about — two asks owed, two terminals arriving in a
    known order.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("send", flows.powerbox_refused_frame(code=int(PowerboxRefusal.BUSY))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=8,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    facet = grant._powerbox_facet()
    # Two asks owed. Sent by hand because `request_file` blocks on its own
    # terminal, which is the SDK declining to pipeline rather than the wire
    # forbidding it.
    facet._owed = 2

    conn._dispatch_one()
    conn._dispatch_one()

    kinds = [type(terminal) for terminal in facet._terminals]
    assert kinds == [PowerboxRefusedEvent, DesignatedEvent]
    assert facet._terminals[0].code == PowerboxRefusal.BUSY

    os.close(facet._terminals[1].fd)
    conn.close()


def test_a_terminal_nobody_asked_for_is_a_violation_and_leaks_no_descriptor(
    server, tmp_path
) -> None:
    """A second terminal for one ask is a server contract violation.

    "Exactly one terminal per request" means terminals can never outnumber the
    asks outstanding. The check earns its place here rather than anywhere else
    in this SDK because an unclaimed `designated` holds a **descriptor**: with
    nothing watching, it would sit in the facet's queue unreachable and
    unclosed for the life of the process. So the fd count is asserted flat
    across the whole exchange, which is the half a `raises` alone would miss.

    The second frame is read with `_dispatch_one` — "the client reads the next
    frame", which is all any blocking call here does — rather than through a
    `sync` barrier, so that a build with the check removed fails this test
    instead of blocking forever on a `done` the violation would have preceded.
    """
    before = _fd_count()
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=9,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=10,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="unasked.txt",
                ),
                os.open(_file(tmp_path, "unasked.txt"), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    designation = conn.request_file(grant)
    assert isinstance(designation, DesignatedEvent)
    os.close(designation.fd)

    with pytest.raises(ServerContractViolation, match="never made"):
        conn._dispatch_one()
    assert conn.closed

    server.join()  # the mock's own descriptors are gone before we count
    assert _fd_count() == before


def test_an_undefined_enum_closes_the_descriptor_before_the_connection_dies(
    server, tmp_path
) -> None:
    """A `kind` outside the enum is the server breaking the contract.

    Sanctioned by disconnect, like every other server contract violation — and
    the descriptor that arrived with it is closed first. Ownership transferred
    to us on receipt, so nothing else in the process will ever close it.
    """
    before = _fd_count()
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=11,
                    kind=7,  # vitrin_powerbox.kind defines 0 and 1
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    with pytest.raises(ServerContractViolation, match="undefined enum"):
        conn.request_file(grant)
    assert conn.closed

    server.join()
    assert _fd_count() == before


def test_an_undefined_refusal_code_dies_rather_than_reaching_the_caller(
    server,
) -> None:
    """The other half of the dispatch-time enum check, on the fd-free terminal.

    `PowerboxRefusal` is validated at dispatch for the same reason `kind` and
    `mode` are: `request_file` hands the event straight back, so a caller
    converting `code` to the enum must not be the first thing in the process to
    discover the server invented a value. Written because a check nothing
    exercises is the shape that rots — the descriptor half of `_accept` had two
    tests and this half had none.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            # vitrin_powerbox.refusal defines 0..3.
            ("send", flows.powerbox_refused_frame(code=9)),
            ("linger",),
        ]
    )
    conn = _connect(server)
    grant = _grant(conn)
    with pytest.raises(ServerContractViolation, match="undefined powerbox refusal"):
        conn.request_file(grant)
    assert conn.closed


def test_the_wait_outlasts_the_connect_timeout(server, tmp_path) -> None:
    """There is no client-side deadline on a designation, and none may be added.

    The wait is human-long by construction: the picker is up and a person is
    reading it. `connect`'s timeout governs connect and handshake only — it is
    cleared before the connection is returned — so an ask that takes longer
    than it still completes. A client-side timeout would race the core's own
    deadline (which answers `timed_out`) and could abandon a descriptor the
    core was about to deliver, leaking it for the life of the connection.

    The sleep is short because what is being pinned is that the deadline does
    not apply at all, not how long a human takes: a 250 ms answer under a 50 ms
    connect timeout would fail on any client that kept one.
    """
    server.run(
        [
            *_granted_steps(),
            ("expect", flows.get_powerbox_frame()),
            ("expect", flows.request_file_frame(mode=int(DesignationMode.READ))),
            ("sleep", 0.25),
            (
                "send_fd",
                flows.designated_frame(
                    designation_id=12,
                    kind=int(DesignationKind.FILE),
                    mode=int(DesignationMode.READ),
                    name="notes.txt",
                ),
                os.open(_file(tmp_path), os.O_RDONLY),
            ),
            ("linger",),
        ]
    )
    conn = connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=0.05,
    )
    grant = _grant(conn)
    designation = conn.request_file(grant)
    assert isinstance(designation, DesignatedEvent)
    os.close(designation.fd)
    conn.close()
