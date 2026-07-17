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
use crate::{MAX_FDS_PER_MESSAGE, MAX_MESSAGE_SIZE, MAX_UNCLAIMED_FDS};

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
    /// Peer process id at connect time.
    pub pid: i32,
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
    /// order == frame order (positional matching, conventions 2.2).
    pending_fds: VecDeque<OwnedFd>,
    /// Set on the first peer violation; replayed on every later receive so
    /// a caller can never read desynchronized frames past a violation.
    poisoned: Option<PeerViolation>,
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
        let cred = net::sockopt::socket_peercred(&fd)?;
        Ok(Connection {
            fd,
            peer_cred: PeerCred {
                pid: cred.pid.as_raw_nonzero().get(),
                uid: cred.uid.as_raw(),
                gid: cred.gid.as_raw(),
            },
            recv_buf: Vec::new(),
            scratch: vec![0u8; MAX_MESSAGE_SIZE].into_boxed_slice(),
            pending_fds: VecDeque::new(),
            poisoned: None,
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
        if frame.len() < HEADER_LEN {
            return Err(LocalMisuse::FrameTooShort { len: frame.len() }.into());
        }
        let header =
            FrameHeader::decode(frame).expect("frame length checked against HEADER_LEN above");
        if header.size as usize != frame.len() {
            // Also enforces MAX_MESSAGE_SIZE: a u16 size field cannot
            // declare more, so an oversized buffer always mismatches.
            return Err(LocalMisuse::SizeFieldMismatch {
                declared: header.size,
                actual: frame.len(),
            }
            .into());
        }
        if header.fd_count as usize > MAX_FDS_PER_MESSAGE || (header.fd_count == 1) != fd.is_some()
        {
            return Err(LocalMisuse::FdCountMismatch {
                declared: header.fd_count,
                attached: fd.is_some(),
            }
            .into());
        }

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
                    let fd = if header.fd_count == 1 {
                        match self.pending_fds.pop_front() {
                            Some(fd) => Some(fd),
                            // The fd travels with the frame's own bytes, so
                            // a completed frame with an empty queue means
                            // the peer never attached one.
                            None => return Err(self.poison(PeerViolation::MissingFd)),
                        }
                    } else {
                        None
                    };
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
        for cm in cmsg.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = cm {
                for fd in fds {
                    if self.pending_fds.len() >= MAX_UNCLAIMED_FDS {
                        // `fd` and the rest of the iterator drop (close)
                        // here; queued fds close when the connection does.
                        return Err(self.poison(PeerViolation::UnclaimedFdOverflow));
                    }
                    self.pending_fds.push_back(fd);
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
