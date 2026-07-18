"""A scripted mock core for driving the blocking SDK under test.

The SDK itself is threadless; the *tests* run this scripted server on a
thread so the blocking client has a peer. The server executes a script of
steps against one accepted connection:

    ("expect", frame_bytes)        read len(frame_bytes) bytes, assert equal
    ("send", frame_bytes)          write bytes
    ("send_fd", frame_bytes, fd)   sendmsg bytes with one SCM_RIGHTS fd;
                                   the server closes its copy after sending
    ("close",)                     close the connection immediately
    ("linger",)                    hold the connection open, discarding any
                                   inbound bytes, until the client closes

Assertion failures inside the thread are re-raised in the test thread by
``join()`` (the conftest fixture always joins).
"""

from __future__ import annotations

import os
import socket
import struct
import threading


class MockServer:
    def __init__(self, path: str) -> None:
        self.path = path
        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._listener.bind(path)
        self._listener.listen(1)
        self._listener.settimeout(5.0)
        self._thread: threading.Thread | None = None
        self._failure: BaseException | None = None

    def run(self, script: list[tuple]) -> None:
        assert self._thread is None, "mock server already running"
        self._thread = threading.Thread(
            target=self._serve, args=(script,), daemon=True
        )
        self._thread.start()

    def _serve(self, script: list[tuple]) -> None:
        try:
            conn, _ = self._listener.accept()
            conn.settimeout(5.0)
            try:
                self._execute(conn, script)
            finally:
                conn.close()
        except BaseException as exc:  # re-raised by join()
            self._failure = exc

    def _execute(self, conn: socket.socket, script: list[tuple]) -> None:
        for step in script:
            match step:
                case ("expect", expected):
                    got = self._read_exactly(conn, len(expected))
                    assert got == expected, (
                        f"client sent unexpected bytes:\n"
                        f"  expected {expected.hex()}\n"
                        f"  got      {got.hex()}"
                    )
                case ("send", payload):
                    conn.sendall(payload)
                case ("send_fd", payload, fd):
                    conn.sendmsg(
                        [payload],
                        [(socket.SOL_SOCKET, socket.SCM_RIGHTS, struct.pack("i", fd))],
                    )
                    os.close(fd)  # sender closes its own copy after sending
                case ("close",):
                    return
                case ("linger",):
                    while conn.recv(4096):
                        pass
                    return
                case _:
                    raise AssertionError(f"unknown script step {step!r}")

    @staticmethod
    def _read_exactly(conn: socket.socket, count: int) -> bytes:
        buf = bytearray()
        while len(buf) < count:
            chunk = conn.recv(count - len(buf))
            if not chunk:
                raise AssertionError(
                    f"client closed after {len(buf)} of {count} expected bytes"
                )
            buf += chunk
        return bytes(buf)

    def join(self) -> None:
        """Wait for the script to finish; re-raise any in-thread failure."""
        if self._thread is not None:
            self._thread.join(timeout=10.0)
            assert not self._thread.is_alive(), "mock server thread hung"
        if self._failure is not None:
            raise self._failure

    def close(self) -> None:
        self._listener.close()
