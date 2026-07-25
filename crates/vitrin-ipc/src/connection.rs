// SPDX-License-Identifier: MPL-2.0
//! One transport connection: blocking frame send/receive with `SCM_RIGHTS`
//! fd passing and the peer credentials captured when the connection came
//! into existence.
//!
//! A [`Connection`] does framing only. It neither decodes argument payloads
//! nor knows what an object id means; [`Message`] hands the complete frame
//! (exactly what the generated `decode(bytes, fd)` functions in
//! `vitrin-protocol` take) to the layer above. Blocking semantics are
//! deliberate at this layer -- the core's calloop integration (P1.2.2)
//! builds readiness-driven reads on top of [`AsFd`], and the SDK side wants
//! plain blocking calls anyway.

use std::collections::VecDeque;
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

use rustix::net::{
    self, AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
    SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketFlags, SocketType,
};
use vitrin_protocol::wire::{FrameHeader, HEADER_LEN};

use crate::error::{LocalMisuse, PeerViolation, TransportError};
use crate::{
    MAX_FDS_PER_MESSAGE, MAX_MESSAGE_SIZE, MAX_SEND_QUEUE_BYTES, MAX_SEND_QUEUE_FDS,
    MAX_UNCLAIMED_FDS,
};

/// Kernel-reported peer credentials (`SO_PEERCRED`), captured once when the
/// [`Connection`] is created -- at `accept(2)` time for accepted
/// connections, which is the value the P1.4.1 identity layer consults as
/// one leg of the sender-constraint triple (connection, verified
/// credential, `SO_PEERCRED`; conventions section 1.3). For a `socketpair`
/// or a `connect`ed client the same mechanism reports the creating/serving
/// process. The kernel guarantees these values were valid at connect time;
/// they are not re-read later, matching "recorded at accept" in the
/// conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    /// Peer process id at connect time, or `None` when the peer's pid is
    /// not mappable into this process's pid namespace (the kernel reports
    /// `0` then, per unix(7)) -- e.g. an SDK client in a container
    /// connecting to a host core, or vice versa. `uid`/`gid` remain
    /// meaningful in that case (subject to user-namespace overflow
    /// mapping); a `None` pid must never be treated as identity evidence.
    pub pid: Option<i32>,
    /// Peer effective user id at connect time.
    pub uid: u32,
    /// Peer effective group id at connect time.
    pub gid: u32,
}

/// One received frame: the complete bytes (header included) plus the fd
/// that rode alongside it, if its header declared one. Feed `bytes` and
/// `fd` straight into the matching generated `decode`.
#[derive(Debug)]
pub struct Message {
    /// The already-decoded 8-byte header (routing data: `object_id`,
    /// `opcode`, `size`, `fd_count`).
    pub header: FrameHeader,
    /// The complete frame, header included, exactly `header.size` bytes.
    pub bytes: Vec<u8>,
    /// The fd received via `SCM_RIGHTS` iff `header.fd_count == 1`.
    /// Close-on-exec from birth (`MSG_CMSG_CLOEXEC`); closed on drop if
    /// unused.
    pub fd: Option<OwnedFd>,
}

/// One received `SCM_RIGHTS` fd batch awaiting the frame that declares it.
///
/// The kernel delivers an fd batch with the `recvmsg` that first consumes
/// bytes of the `sendmsg` that carried it, and never delivers two batches
/// in one `recvmsg` -- but it *does* glue preceding fd-less bytes from the
/// same sender onto the head of that `recvmsg` (`unix_stream_read_generic`
/// merges consecutive skbs whose credentials match; the fd array is not
/// part of that comparison). So the receiver knows the byte-stream *span*
/// of the delivering `recvmsg`, not the exact offset the sender attached
/// the fds at; positional matching is enforced against that span.
struct PendingFd {
    /// Stream offset of the first byte of the `recvmsg` that delivered
    /// this fd. The true attach offset is `>= span_start`.
    span_start: u64,
    /// Stream offset one past the last byte of that `recvmsg`. Because a
    /// `recvmsg` ends at (or inside) the fd-bearing segment, the true
    /// attach offset is `< span_end`.
    span_end: u64,
    fd: OwnedFd,
}

/// One outgoing frame parked by the non-blocking send path
/// ([`Connection::send_or_queue`]) because the kernel would not take its
/// bytes yet.
struct QueuedFrame {
    /// The unsent tail of the frame (a fully-unsent frame parks whole).
    bytes: Vec<u8>,
    /// Bytes of `bytes` already written to the socket by a partial flush.
    offset: usize,
    /// The fd that must ride the frame's first `sendmsg` (positional
    /// matching, conventions 2.2). A duplicate owned by the queue -- the
    /// caller's `BorrowedFd` cannot outlive the call -- and `None` once
    /// delivered (any successful `sendmsg` carries the whole ancillary
    /// payload) or when the head of the frame already went out before the
    /// frame was parked.
    fd: Option<OwnedFd>,
}

/// A connected transport endpoint over a Unix stream socket.
pub struct Connection {
    fd: OwnedFd,
    peer_cred: PeerCred,
    /// Reassembly buffer: bytes received but not yet returned as frames.
    recv_buf: Vec<u8>,
    /// recvmsg scratch, one max-size frame long; heap-allocated once so
    /// per-connection memory stays visibly bounded.
    scratch: Box<[u8]>,
    /// Received fds not yet claimed by a completed frame, in arrival
    /// order == frame order (positional matching, conventions 2.2), each
    /// tagged with the byte-stream span of the `recvmsg` that delivered it
    /// (see [`PendingFd`]) -- which lets `recv_message` enforce, as tightly
    /// as the kernel's delivery semantics allow, that an fd was attached to
    /// the bytes of the frame that declares it, and to no other
    /// (conventions 2.4).
    pending_fds: VecDeque<PendingFd>,
    /// Total bytes already consumed from the stream as completed frames,
    /// i.e. the stream offset of `recv_buf[0]`.
    consumed: u64,
    /// Set on the first peer violation; replayed on every later receive so
    /// a caller can never read desynchronized frames past a violation.
    poisoned: Option<PeerViolation>,
    /// Outgoing frames the non-blocking send path has parked, oldest first
    /// (FIFO preserves wire order). Empty on the blocking client path.
    send_queue: VecDeque<QueuedFrame>,
    /// Total unsent bytes across `send_queue` -- the number
    /// [`MAX_SEND_QUEUE_BYTES`] caps. Maintained incrementally so the
    /// event-loop's per-dispatch checks are O(1).
    queued_send_bytes: usize,
    /// Undelivered duplicate fds across `send_queue` -- the number
    /// [`MAX_SEND_QUEUE_FDS`] caps. Counted separately from bytes because
    /// small fd-bearing frames would otherwise let the byte cap admit
    /// thousands of parked fds (see the constant's docs).
    queued_send_fds: usize,
    /// Set when a send would have pushed the queue past either
    /// [`MAX_SEND_QUEUE_BYTES`] or [`MAX_SEND_QUEUE_FDS`]; sticky (like
    /// `poisoned`) so every later send fails identically and the event loop
    /// reliably observes the overflow after any dispatch.
    send_overflowed: bool,
    /// Set on the first fatal (non-`WouldBlock`) I/O error on the
    /// non-blocking send path; sticky. A partial `try_send_now` write that
    /// then errors leaves a torn frame prefix in the kernel buffer, so no
    /// later send may ever touch the stream again -- and the event loop
    /// checks this after each dispatch to remove the connection (a send
    /// error with an empty queue would otherwise never surface as a
    /// terminal condition).
    send_poisoned: Option<io::ErrorKind>,
}

impl std::fmt::Debug for Connection {
    /// Concise on purpose: a derived impl would dump the multi-kilobyte
    /// `scratch`/`recv_buf` byte buffers on every log line. The fields worth
    /// seeing are the peer identity and how much is mid-reassembly.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("fd", &self.fd)
            .field("peer_cred", &self.peer_cred)
            .field("buffered", &self.recv_buf.len())
            .field("pending_fds", &self.pending_fds.len())
            .field("poisoned", &self.poisoned)
            .field("queued_send_bytes", &self.queued_send_bytes)
            .field("queued_send_fds", &self.queued_send_fds)
            .field("send_overflowed", &self.send_overflowed)
            .field("send_poisoned", &self.send_poisoned)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// Wrap an already-connected Unix stream socket, capturing
    /// `SO_PEERCRED` immediately. This is the accepted-socket path
    /// ([`Listener::accept`](crate::Listener::accept) calls it) and the
    /// inherited-socketpair path: the realm spawn manager (P1.5.2) passes
    /// one [`Connection::pair`] end to the shim, which reconstructs its
    /// core connection from the inherited fd with this function.
    ///
    /// The fd should have been created close-on-exec; every constructor in
    /// this crate guarantees it. An fd inherited across `exec` (the shim
    /// case) was *deliberately* re-opened for inheritance by the spawn
    /// manager clearing `FD_CLOEXEC` on its one intended fd -- that
    /// exception is P1.5.2's to manage, not a hole in this crate's
    /// discipline.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        // Deliberately *not* rustix's `socket_peercred`: its `UCred` backs
        // the pid with a `NonZeroI32`, but the kernel reports `pid = 0` for
        // a peer whose pid has no mapping in our pid namespace (unix(7)),
        // which would be undefined behavior. nix keeps the raw
        // `struct ucred`, so pid 0 is representable and mapped to `None`.
        let cred = nix::sys::socket::getsockopt(&fd, nix::sys::socket::sockopt::PeerCredentials)
            .map_err(io::Error::from)?;
        Ok(Connection {
            fd,
            peer_cred: PeerCred {
                pid: match cred.pid() {
                    0 => None,
                    pid => Some(pid),
                },
                uid: cred.uid(),
                gid: cred.gid(),
            },
            recv_buf: Vec::new(),
            scratch: vec![0u8; MAX_MESSAGE_SIZE].into_boxed_slice(),
            pending_fds: VecDeque::new(),
            consumed: 0,
            poisoned: None,
            send_queue: VecDeque::new(),
            queued_send_bytes: 0,
            queued_send_fds: 0,
            send_overflowed: false,
            send_poisoned: None,
        })
    }

    /// A connected pair (`socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC)`),
    /// each end a full [`Connection`]. This is both the unit-test harness
    /// and the real core-to-shim channel primitive.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = net::socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )?;
        Ok((Self::from_fd(a)?, Self::from_fd(b)?))
    }

    /// Connect to a listening socket (client side: the SDK, or a test
    /// against a [`Listener`](crate::Listener)). The socket is created
    /// `SOCK_CLOEXEC`.
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let fd = net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )?;
        let addr = SocketAddrUnix::new(path.as_ref())?;
        net::connect(&fd, &addr)?;
        Self::from_fd(fd)
    }

    /// The peer credentials captured when this connection was created.
    pub fn peer_cred(&self) -> PeerCred {
        self.peer_cred
    }

    /// Send one complete frame, with its fd riding the same `sendmsg` as
    /// `SCM_RIGHTS` ancillary data (so the receiver's positional matching
    /// holds by construction). `frame` must be exactly one well-formed
    /// frame -- generated `encode` output already is -- and `fd` must match
    /// its header's `fd_count`; contradictions return
    /// [`TransportError::LocalMisuse`] before any byte is written.
    ///
    /// Blocking; `MSG_NOSIGNAL` throughout, so a closed peer surfaces as an
    /// `EPIPE` [`TransportError::Io`], never `SIGPIPE`.
    pub fn send_message(
        &mut self,
        frame: &[u8],
        fd: Option<BorrowedFd<'_>>,
    ) -> Result<(), TransportError> {
        validate_outgoing(frame, fd)?;

        let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut cmsg = SendAncillaryBuffer::new(&mut space);
        let fds;
        if let Some(fd) = fd {
            fds = [fd];
            let pushed = cmsg.push(SendAncillaryMessage::ScmRights(&fds));
            assert!(pushed, "ancillary buffer sized for exactly one fd");
        }

        // The first sendmsg carries the ancillary data with the frame's
        // leading bytes; EINTR before anything was sent is retried with the
        // ancillary intact.
        let iov = [IoSlice::new(frame)];
        let mut sent =
            retry_eintr(|| net::sendmsg(&self.fd, &iov, &mut cmsg, SendFlags::NOSIGNAL))?;

        // A partial send (possible on a blocking stream socket under signal
        // or buffer pressure) already delivered the fd; push the remaining
        // bytes with plain send.
        while sent < frame.len() {
            let n = retry_eintr(|| net::send(&self.fd, &frame[sent..], SendFlags::NOSIGNAL))?;
            if n == 0 {
                return Err(TransportError::Io(io::ErrorKind::WriteZero.into()));
            }
            sent += n;
        }
        Ok(())
    }

    /// Send one frame on a **non-blocking** socket, parking whatever the
    /// kernel will not take in the connection's bounded send queue -- the
    /// event-loop side of sending (the core's fd is non-blocking; the
    /// blocking SDK path stays on [`send_message`](Self::send_message)).
    ///
    /// Semantics:
    ///
    /// - Same argument validation as `send_message`; contradictions are
    ///   [`TransportError::LocalMisuse`] before any byte moves.
    /// - Attempts an immediate write (after opportunistically flushing any
    ///   already-parked frames, preserving wire order). On `EAGAIN` the
    ///   remainder is queued and `Ok(())` returned -- the caller's event
    ///   loop flushes it on write-readiness via
    ///   [`flush_send_queue`](Self::flush_send_queue).
    /// - A queued fd is duplicated (`F_DUPFD_CLOEXEC` via
    ///   `try_clone_to_owned`), so the caller's fd can be closed as soon as
    ///   this returns, exactly as with the blocking path.
    /// - If parking the frame would push the queue past
    ///   [`MAX_SEND_QUEUE_BYTES`] bytes or [`MAX_SEND_QUEUE_FDS`] parked
    ///   fds, **nothing is queued** and the sticky
    ///   [`TransportError::SendQueueFull`] is returned: the peer has stopped
    ///   reading, and the P1.2.3 policy is to disconnect it -- the event
    ///   loop observes [`send_queue_overflowed`](Self::send_queue_overflowed)
    ///   after each dispatch and removes the connection.
    /// - A fatal (non-`WouldBlock`) I/O error poisons the send side
    ///   ([`send_poisoned`](Self::send_poisoned)), also sticky: a partial
    ///   write followed by an error leaves a torn frame prefix in the kernel
    ///   buffer, so appending anything afterwards would desynchronize the
    ///   whole outgoing stream. Every later call replays the error without
    ///   touching the wire; the event loop observes the poison after each
    ///   dispatch and removes the connection.
    pub fn send_or_queue(
        &mut self,
        frame: &[u8],
        fd: Option<BorrowedFd<'_>>,
    ) -> Result<(), TransportError> {
        validate_outgoing(frame, fd)?;
        if self.send_overflowed {
            return Err(TransportError::SendQueueFull {
                queued: self.queued_send_bytes,
            });
        }
        if let Some(kind) = self.send_poisoned {
            return Err(TransportError::Io(kind.into()));
        }
        if !self.send_queue.is_empty() {
            self.flush_send_queue()?;
        }
        if self.send_queue.is_empty() {
            let sent = self.try_send_now(frame, fd)?;
            if sent == frame.len() {
                return Ok(());
            }
            // Any successful sendmsg carried the whole ancillary payload, so
            // the fd is still owed only if *zero* bytes went out.
            let parked_fd = if sent == 0 {
                fd.map(|fd| fd.try_clone_to_owned()).transpose()?
            } else {
                None
            };
            self.park(frame[sent..].to_vec(), parked_fd)
        } else {
            // Earlier frames are still parked; this one must queue whole
            // behind them to preserve wire order (fd included).
            let parked_fd = fd.map(|fd| fd.try_clone_to_owned()).transpose()?;
            self.park(frame.to_vec(), parked_fd)
        }
    }

    /// Write as much of the parked send queue as the kernel will take.
    /// `EAGAIN` is success ("wait for the next write-readiness"), so `Ok`
    /// does **not** mean empty -- check
    /// [`queued_send_bytes`](Self::queued_send_bytes). Real I/O errors (e.g.
    /// `EPIPE` after the peer closed) poison the send side
    /// ([`send_poisoned`](Self::send_poisoned)) and are returned; the
    /// connection should be dropped.
    pub fn flush_send_queue(&mut self) -> Result<(), TransportError> {
        while let Some(front) = self.send_queue.front_mut() {
            let result = if let Some(fd) = front.fd.as_ref() {
                debug_assert_eq!(front.offset, 0, "an fd rides its frame's first byte");
                let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(1))];
                let mut cmsg = SendAncillaryBuffer::new(&mut space);
                let fds = [fd.as_fd()];
                let pushed = cmsg.push(SendAncillaryMessage::ScmRights(&fds));
                assert!(pushed, "ancillary buffer sized for exactly one fd");
                let iov = [IoSlice::new(&front.bytes)];
                retry_eintr(|| net::sendmsg(&self.fd, &iov, &mut cmsg, SendFlags::NOSIGNAL))
            } else {
                retry_eintr(|| {
                    net::send(&self.fd, &front.bytes[front.offset..], SendFlags::NOSIGNAL)
                })
            };
            match result {
                Ok(0) => return Err(self.poison_send(io::ErrorKind::WriteZero.into())),
                Ok(n) => {
                    // Delivered with the first successful sendmsg; drop our
                    // duplicate.
                    if front.fd.take().is_some() {
                        self.queued_send_fds -= 1;
                    }
                    front.offset += n;
                    self.queued_send_bytes -= n;
                    if front.offset == front.bytes.len() {
                        self.send_queue.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(self.poison_send(e)),
            }
        }
        Ok(())
    }

    /// Unsent bytes currently parked by [`send_or_queue`](Self::send_or_queue)
    /// (the quantity [`MAX_SEND_QUEUE_BYTES`] caps). Non-zero means the
    /// event loop should watch for write-readiness and
    /// [`flush_send_queue`](Self::flush_send_queue).
    pub fn queued_send_bytes(&self) -> usize {
        self.queued_send_bytes
    }

    /// Undelivered duplicate fds currently parked in the send queue (the
    /// quantity [`MAX_SEND_QUEUE_FDS`] caps).
    pub fn queued_send_fds(&self) -> usize {
        self.queued_send_fds
    }

    /// Whether a send has hit the [`MAX_SEND_QUEUE_BYTES`] or
    /// [`MAX_SEND_QUEUE_FDS`] cap: the peer is a slow reader and the P1.2.3
    /// policy is to disconnect it. Sticky.
    pub fn send_queue_overflowed(&self) -> bool {
        self.send_overflowed
    }

    /// The sticky fatal send-side I/O error, if one has occurred
    /// (e.g. `EPIPE` from a peer that shut down its read side, or an error
    /// after a partial write that left a torn frame on the wire). The event
    /// loop checks this after each dispatch -- a send failure with an empty
    /// queue has no other observation point -- and removes the connection.
    pub fn send_poisoned(&self) -> Option<io::ErrorKind> {
        self.send_poisoned
    }

    /// One immediate non-blocking write attempt of a fresh frame (empty
    /// queue): the ancillary fd rides the first `sendmsg`, then plain sends
    /// continue until done or `EAGAIN`. Returns the bytes written (`0` means
    /// the fd was *not* delivered); `EAGAIN` is never an error here.
    fn try_send_now(
        &mut self,
        frame: &[u8],
        fd: Option<BorrowedFd<'_>>,
    ) -> Result<usize, TransportError> {
        let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut cmsg = SendAncillaryBuffer::new(&mut space);
        let fds;
        if let Some(fd) = fd {
            fds = [fd];
            let pushed = cmsg.push(SendAncillaryMessage::ScmRights(&fds));
            assert!(pushed, "ancillary buffer sized for exactly one fd");
        }
        let iov = [IoSlice::new(frame)];
        let mut sent =
            match retry_eintr(|| net::sendmsg(&self.fd, &iov, &mut cmsg, SendFlags::NOSIGNAL)) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(0),
                Err(e) => return Err(self.poison_send(e)),
            };
        while sent < frame.len() {
            match retry_eintr(|| net::send(&self.fd, &frame[sent..], SendFlags::NOSIGNAL)) {
                Ok(0) => return Err(self.poison_send(io::ErrorKind::WriteZero.into())),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // An error *after* a partial write leaves a torn frame
                // prefix in the kernel buffer; the poison guarantees no
                // later send can append past it.
                Err(e) => return Err(self.poison_send(e)),
            }
        }
        Ok(sent)
    }

    /// Record a fatal send-side I/O error (sticky) and convert it. Every
    /// non-`WouldBlock` error on the non-blocking send path funnels through
    /// here so the event loop can observe the failure after the dispatch
    /// even when nothing is parked in the queue.
    fn poison_send(&mut self, e: io::Error) -> TransportError {
        self.send_poisoned = Some(e.kind());
        TransportError::Io(e)
    }

    /// Park an unsent (tail of a) frame, enforcing both queue caps: bytes
    /// ([`MAX_SEND_QUEUE_BYTES`]) and parked duplicate fds
    /// ([`MAX_SEND_QUEUE_FDS`] -- without it, small fd-bearing frames would
    /// let a non-reading peer pin thousands of fds inside the byte cap). On
    /// overflow nothing is queued, the sticky overflow flag is set, and the
    /// dropped duplicate fd (if any) closes here -- the connection is about
    /// to die anyway.
    fn park(&mut self, bytes: Vec<u8>, fd: Option<OwnedFd>) -> Result<(), TransportError> {
        debug_assert!(!bytes.is_empty());
        if self.queued_send_bytes + bytes.len() > MAX_SEND_QUEUE_BYTES
            || self.queued_send_fds + usize::from(fd.is_some()) > MAX_SEND_QUEUE_FDS
        {
            self.send_overflowed = true;
            return Err(TransportError::SendQueueFull {
                queued: self.queued_send_bytes,
            });
        }
        self.queued_send_bytes += bytes.len();
        self.queued_send_fds += usize::from(fd.is_some());
        self.send_queue.push_back(QueuedFrame {
            bytes,
            offset: 0,
            fd,
        });
        Ok(())
    }

    /// Receive the next complete frame, blocking until one is available.
    ///
    /// Returns `Ok(None)` on a clean end-of-stream (peer closed between
    /// frames). A close *inside* a frame is [`TransportError::Eof`], and
    /// framing/fd violations are [`TransportError::PeerViolation`] --
    /// after which the connection is poisoned and every further call
    /// returns the same violation (drop the connection; policy in P1.2.3).
    pub fn recv_message(&mut self) -> Result<Option<Message>, TransportError> {
        if let Some(v) = self.poisoned {
            return Err(v.into());
        }
        loop {
            if self.recv_buf.len() >= HEADER_LEN {
                let header = FrameHeader::decode(&self.recv_buf)
                    .expect("buffer holds at least a full header");
                let size = header.size as usize;
                if size < HEADER_LEN {
                    return Err(
                        self.poison(PeerViolation::UndersizedSizeField { size: header.size })
                    );
                }
                if header.fd_count as usize > MAX_FDS_PER_MESSAGE {
                    return Err(self.poison(PeerViolation::FdCountExceeded {
                        fd_count: header.fd_count,
                    }));
                }
                if self.recv_buf.len() >= size {
                    let bytes = self.recv_buf[..size].to_vec();
                    self.recv_buf.drain(..size);
                    // This frame occupied [frame_start, frame_end) of the
                    // byte stream. Matching is against each pending fd's
                    // recvmsg *span* (see [`PendingFd`]): the kernel can
                    // glue fd-less bytes onto the head of the recvmsg that
                    // delivers an fd batch, so the exact attach offset is
                    // only known to lie inside the span.
                    let frame_start = self.consumed;
                    let frame_end = frame_start + size as u64;
                    self.consumed = frame_end;
                    let fd = if header.fd_count == 1 {
                        match self.pending_fds.front() {
                            // The declared fd. A compliant peer attaches it
                            // to the frame's first byte, and the recvmsg
                            // that consumed that byte -- delivering the fd
                            // -- necessarily started at or before it, so
                            // `span_start <= frame_start` holds for the
                            // frame's own fd (conventions 2.2/2.4).
                            Some(entry) if entry.span_start <= frame_start => {
                                let entry = self.pending_fds.pop_front().expect("front exists");
                                Some(entry.fd)
                            }
                            // An empty queue -- or a front fd whose whole
                            // delivering recvmsg lies after the frame began
                            // -- means the frame's own bytes carried no fd.
                            // (A front fd from strictly *earlier* bytes is
                            // structurally impossible: it would have been
                            // claimed or fataled when the frame covering
                            // those bytes completed.)
                            _ => return Err(self.poison(PeerViolation::MissingFd)),
                        }
                    } else {
                        None
                    };
                    // Any fd still queued whose delivering recvmsg ended
                    // within this frame's bytes was attached at an offset
                    // below `frame_end`, i.e. to this frame or earlier --
                    // and no frame declared it: either fds on a frame
                    // declaring none, or a second fd on a one-fd frame.
                    // Both are fatal fd_violation (conventions 2.4) --
                    // detected here, at the only layer that can see them,
                    // so a smuggled fd can never shift positional matching
                    // for a later frame. (A span reaching past `frame_end`
                    // may belong to the *next* frame's first byte; it stays
                    // pending and is judged when that frame completes.)
                    if let Some(front) = self.pending_fds.front() {
                        if front.span_end <= frame_end {
                            return Err(self.poison(PeerViolation::UnsolicitedFd));
                        }
                    }
                    return Ok(Some(Message { header, bytes, fd }));
                }
            }
            let n = self.fill()?;
            if n == 0 {
                return if self.recv_buf.is_empty() {
                    // Clean EOF. Any fds still pending belonged to frames
                    // that never came; dropping the connection closes them.
                    Ok(None)
                } else {
                    Err(TransportError::Eof {
                        buffered: self.recv_buf.len(),
                    })
                };
            }
        }
    }

    /// One `recvmsg` round: harvest ancillary fds into the pending queue
    /// and append payload bytes to the reassembly buffer. Returns the byte
    /// count (0 = EOF).
    fn fill(&mut self) -> Result<usize, TransportError> {
        let mut space =
            [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(MAX_UNCLAIMED_FDS))];
        let mut cmsg = RecvAncillaryBuffer::new(&mut space);

        // Stream offset of the first byte this recvmsg will return: the
        // start of the span the harvested fds are tagged with. The kernel
        // stops a recvmsg after delivering one fd batch (so at most one
        // batch arrives here), but it may glue preceding fd-less bytes onto
        // the head, so the batch's true attach offset is only known to lie
        // within [span_start, span_end) -- see [`PendingFd`].
        let span_start = self.consumed + self.recv_buf.len() as u64;

        let (bytes, flags) = {
            let mut iov = [IoSliceMut::new(&mut self.scratch)];
            // MSG_CMSG_CLOEXEC: every received fd is close-on-exec from the
            // instant it exists in this process.
            let msg = retry_eintr(|| {
                net::recvmsg(&self.fd, &mut iov, &mut cmsg, RecvFlags::CMSG_CLOEXEC)
            })?;
            (msg.bytes, msg.flags)
        };

        if flags.contains(ReturnFlags::CTRUNC) {
            // More fds in one sendmsg than our ancillary buffer admits: the
            // kernel closed the overflow, the drop of `cmsg` closes the
            // delivered remainder, and the connection dies.
            return Err(self.poison(PeerViolation::AncillaryTruncated));
        }
        let span_end = span_start + bytes as u64;
        for cm in cmsg.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = cm {
                for fd in fds {
                    if self.pending_fds.len() >= MAX_UNCLAIMED_FDS {
                        // `fd` and the rest of the iterator drop (close)
                        // here; queued fds close when the connection does.
                        return Err(self.poison(PeerViolation::UnclaimedFdOverflow));
                    }
                    self.pending_fds.push_back(PendingFd {
                        span_start,
                        span_end,
                        fd,
                    });
                }
            }
        }
        self.recv_buf.extend_from_slice(&self.scratch[..bytes]);
        Ok(bytes)
    }

    fn poison(&mut self, v: PeerViolation) -> TransportError {
        self.poisoned = Some(v);
        v.into()
    }
}

impl AsFd for Connection {
    /// The underlying socket, for readiness integration (calloop, P1.2.2).
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// The shared outgoing-frame validation of [`Connection::send_message`] and
/// [`Connection::send_or_queue`]: `frame` must be exactly one well-formed
/// frame and `fd` must match its header's `fd_count`. Contradictions are
/// [`LocalMisuse`], caught before any byte hits the wire.
fn validate_outgoing(frame: &[u8], fd: Option<BorrowedFd<'_>>) -> Result<(), TransportError> {
    if frame.len() < HEADER_LEN {
        return Err(LocalMisuse::FrameTooShort { len: frame.len() }.into());
    }
    let header = FrameHeader::decode(frame).expect("frame length checked against HEADER_LEN above");
    if header.size as usize != frame.len() {
        // Also enforces MAX_MESSAGE_SIZE: a u16 size field cannot declare
        // more, so an oversized buffer always mismatches.
        return Err(LocalMisuse::SizeFieldMismatch {
            declared: header.size,
            actual: frame.len(),
        }
        .into());
    }
    if header.fd_count as usize > MAX_FDS_PER_MESSAGE || (header.fd_count == 1) != fd.is_some() {
        return Err(LocalMisuse::FdCountMismatch {
            declared: header.fd_count,
            attached: fd.is_some(),
        }
        .into());
    }
    Ok(())
}

/// Retry a syscall on `EINTR` (harmless here: for these blocking calls the
/// kernel only returns it when nothing was transferred), converting other
/// errnos to `io::Error`.
fn retry_eintr<T>(mut f: impl FnMut() -> rustix::io::Result<T>) -> io::Result<T> {
    loop {
        match f() {
            Err(rustix::io::Errno::INTR) => continue,
            r => return r.map_err(io::Error::from),
        }
    }
}
