# SPDX-License-Identifier: Apache-2.0
"""Blocking Unix-socket transport: frame buffering and SCM_RIGHTS receipt.

One :class:`Transport` wraps one connected ``AF_UNIX`` ``SOCK_STREAM``
socket. No threads, no async runtime: reads block, and the single-ordered-
stream guarantee (conventions section 4) makes that sufficient.

File-descriptor receipt follows the libwayland model: every ``recvmsg``
passes ``MSG_CMSG_CLOEXEC`` (so received fds are close-on-exec atomically)
and collects any ``SCM_RIGHTS`` fds into an ordered queue. With
``SOCK_STREAM``, ancillary data is delivered no later than the first byte
of the segment it was sent with, so by the time a frame's bytes are fully
buffered its fd (if the header declares one) is already queued — the header
``fd_count`` then matches fds to frames positionally.
"""

from __future__ import annotations

import array
import os
import socket
from collections import deque

from .errors import ConnectionClosed, ServerContractViolation
from .wire import HEADER_SIZE, unpack_header

_RECV_SIZE = 65536
# Room for a generous burst of fds per recvmsg; version 1 sends at most one
# fd per message, but a pipelined capture burst can deliver several.
_ANC_SIZE = socket.CMSG_SPACE(16 * array.array("i").itemsize)
# Ceiling on received-but-unclaimed fds. A conformant server attaches an fd
# only to the frame that declares it, so the queue stays shallow (bounded by
# frames still in the receive buffer); growth past this is an fd flood.
_MAX_UNCLAIMED_FDS = 16


class Transport:
    """Framed, fd-aware blocking transport over a Unix stream socket."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock
        self._buf = bytearray()
        self._fds: deque[int] = deque()
        self._closed = False

    @classmethod
    def connect_unix(cls, path: str | os.PathLike[str], *, timeout: float | None = None) -> "Transport":
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.settimeout(timeout)
            sock.connect(os.fspath(path))
        except BaseException:
            sock.close()
            raise
        return cls(sock)

    def settimeout(self, timeout: float | None) -> None:
        self._sock.settimeout(timeout)

    # -- sending ------------------------------------------------------------

    def send_frame(self, frame: bytes) -> None:
        """Send one complete frame. Principal clients never send fds in v1."""
        if self._closed:
            raise ConnectionClosed("send on a closed connection")
        try:
            self._sock.sendall(frame)
        except TimeoutError as exc:
            # A timeout mid-sendall leaves the amount actually written
            # unknowable (per the socket docs): the framed stream is
            # indeterminate, so the transport must die, not be reused.
            self.close()
            raise ConnectionClosed(f"send timed out mid-frame: {exc}") from exc
        except (BrokenPipeError, ConnectionResetError) as exc:
            self.close()
            raise ConnectionClosed(f"connection lost while sending: {exc}") from exc

    # -- receiving ----------------------------------------------------------

    def _fill(self) -> None:
        """Block for at least one byte (or fd) more; parse ancillary fds."""
        try:
            data, ancdata, msg_flags, _ = self._sock.recvmsg(
                _RECV_SIZE, _ANC_SIZE, socket.MSG_CMSG_CLOEXEC
            )
        except TimeoutError as exc:
            self.close()
            raise ConnectionClosed(f"read timed out: {exc}") from exc
        except (ConnectionResetError, BrokenPipeError) as exc:
            self.close()
            raise ConnectionClosed(f"connection lost while reading: {exc}") from exc
        for level, ctype, cdata in ancdata:
            if level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS:
                fds = array.array("i")
                fds.frombytes(cdata[: len(cdata) - (len(cdata) % fds.itemsize)])
                self._fds.extend(fds)
        if len(self._fds) > _MAX_UNCLAIMED_FDS:
            self.close()
            raise ServerContractViolation(
                f"more than {_MAX_UNCLAIMED_FDS} unclaimed fds queued (fd flood)"
            )
        if msg_flags & socket.MSG_CTRUNC:
            # An fd bomb overflowed the ancillary buffer; some fds may have
            # been dropped by the kernel. The positional fd/frame matching
            # is no longer trustworthy: close everything and give up.
            self.close()
            raise ServerContractViolation(
                "ancillary data truncated (MSG_CTRUNC): fd/frame pairing lost"
            )
        if not data:
            if not ancdata:
                raise ConnectionClosed("server closed the connection")
        else:
            self._buf += data

    def recv_frame(self) -> tuple[int, int, bytes, int | None]:
        """Block until one whole frame is buffered; return it.

        Returns ``(object_id, opcode, payload, fd)`` where ``payload`` is
        the bytes after the 8-byte header and ``fd`` is the received file
        descriptor when the header declares ``fd_count == 1`` (ownership
        transfers to the caller). Framing violations by the server raise
        :class:`ServerContractViolation` with the connection closed.
        """
        if self._closed:
            raise ConnectionClosed("read on a closed connection")
        while len(self._buf) < HEADER_SIZE:
            self._fill()
        object_id, size, opcode, fd_count = unpack_header(self._buf)
        if size < HEADER_SIZE:
            self.close()
            raise ServerContractViolation(
                f"declared frame size {size} below the {HEADER_SIZE}-byte minimum"
            )
        while len(self._buf) < size:
            self._fill()
        payload = bytes(self._buf[HEADER_SIZE:size])
        del self._buf[:size]
        fd: int | None = None
        if fd_count == 1:
            if not self._fds:
                self.close()
                raise ServerContractViolation(
                    "frame declares one fd but none accompanied it"
                )
            fd = self._fds.popleft()
        elif fd_count != 0:
            self.close()
            raise ServerContractViolation(
                f"fd_count {fd_count} violates the one-fd-per-message invariant"
            )
        if self._fds and not self._buf:
            # Ancillary data is delivered no later than the first byte of
            # the segment it was sent with, so with the receive buffer
            # fully drained every queued fd was attached to bytes already
            # consumed — bytes whose frames declared no fd (or already
            # claimed theirs). That is the "unsolicited fds attached"
            # fd_violation condition (conventions section 2.4), and left
            # unchecked the orphan would silently mispair with the next
            # fd-declaring frame.
            if fd is not None:
                os.close(fd)
            self.close()
            raise ServerContractViolation(
                "unsolicited fd(s) attached to frames that declare none"
            )
        return object_id, opcode, payload, fd

    # -- lifecycle ----------------------------------------------------------

    @property
    def closed(self) -> bool:
        return self._closed

    def close(self) -> None:
        """Close the socket and any received-but-unclaimed fds (no leaks)."""
        if self._closed:
            return
        self._closed = True
        while self._fds:
            try:
                os.close(self._fds.popleft())
            except OSError:
                pass
        self._sock.close()
