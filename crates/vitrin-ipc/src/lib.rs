//! Unix-socket transport for the Vitrin OS wire protocol.
//!
//! This crate moves **frames and file descriptors** between processes and
//! nothing else: it owns the listening socket, the accepted connection, the
//! length-prefixed frame reassembly, the `SCM_RIGHTS` fd transfer, and the
//! `SO_PEERCRED` capture at accept time. It deliberately contains **no
//! protocol semantics** -- no dispatch, no object table, no handshake state
//! machine -- so every type in here is testable with a plain `socketpair`.
//! The frame format itself (the 8-byte header: `object_id`, `size`,
//! `opcode`, `fd_count`) is defined by [`vitrin_protocol::wire`] and
//! `docs/protocol/00-conventions.md` section 2; this crate reuses that
//! codec, it does not re-implement it.
//!
//! Layering (PRD Doc 2 sections 2 and 3.2): `vitrin-protocol` is pure data
//! and encode/decode; `vitrin-ipc` (this crate) is bytes-and-fds-on-a-socket;
//! the trusted core (`vitrin-core`, P1.3+) stacks identity, grants, and
//! dispatch on top. Event-loop integration (calloop on the core side) is
//! P1.2.2 and builds on [`Connection`]'s `AsFd` implementation; the
//! backpressure and misbehavior *policy* (P1.2.3) lives in [`event_loop`]
//! and consumes the [`error::PeerViolation`] / [`error::TransportError`]
//! values surfaced here.
//!
//! # Backpressure & misbehavior posture (P1.2.3)
//!
//! Wayland's posture, adopted wholesale: **a misbehaving client dies; the
//! compositor loop never blocks and never buffers unboundedly on a client's
//! behalf.** Concretely:
//!
//! - **Slow readers.** The non-blocking send path
//!   ([`Connection::send_or_queue`]) parks bytes the kernel will not take in
//!   a per-connection queue capped at [`MAX_SEND_QUEUE_BYTES`]; a send that
//!   would exceed the cap fails with [`error::TransportError::SendQueueFull`]
//!   and the event-loop glue disconnects the client
//!   (`DisconnectReason::SlowReader`). The queue is *drained* on
//!   write-readiness -- a stalled peer costs the loop nothing.
//! - **Oversized messages / fd-bombs / framing violations.** Already fatal
//!   at the receive path (see below); the event-loop glue classifies each
//!   into a [`event_loop::DisconnectReason`], logs it via `tracing`, and
//!   removes the connection from the loop.
//! - **No goodbye message.** A connection killed for misbehavior is closed
//!   without a best-effort terminal protocol error on the wire. Deliberate:
//!   protocol v0 defines no transport-level "you are being disconnected"
//!   event, and inventing one here would be an unpaired wire-format change
//!   (the IDL + prose pages are `track:protocol`'s to evolve). A peer that
//!   just violated framing cannot be assumed to parse a farewell anyway --
//!   the same conclusion libwayland reached. If a terminal error event is
//!   ever wanted, it lands in `protocol/vitrin-v0.xml` first.
//!
//! # Maximum message size
//!
//! [`MAX_MESSAGE_SIZE`] is **65 535 bytes** (`u16::MAX`), the largest value
//! the frame header's 16-bit `size` field can express. Rationale for
//! choosing exactly the field ceiling rather than a smaller policy number:
//!
//! - **Agreement by construction.** The wire format already makes a larger
//!   frame *inexpressible*: a sender cannot encode one
//!   (`vitrin_protocol::wire::patch_size` asserts) and a receiver cannot be
//!   told about one. Any smaller transport-level cap would be a second,
//!   independently-drifting constant that every implementation (Rust core,
//!   C shim, Python SDK, fuzzer) would have to re-agree on.
//! - **Headroom is already policed upstream.** The largest schema-legal
//!   message in protocol v0 (`vitrin_handshake.hello` with every string at
//!   its documented bound) stays under 35 KiB, comfortably inside the
//!   ceiling; per-argument bounds -- not the transport -- are the mechanism
//!   that keeps real messages small.
//! - **Bounded memory per connection.** Receive-side buffering is
//!   `O(MAX_MESSAGE_SIZE)` per connection (one 64 KiB read scratch plus at
//!   most one partial frame), so a hostile peer cannot balloon the core's
//!   memory through frame size alone; flooding *rate* is P1.2.3's problem.
//! - **A crisp boundary for hardening.** The P1.2.3 misbehavior policy and
//!   the P1.9.3 fuzz corpus get one number to probe: a header whose `size`
//!   is below [`vitrin_protocol::wire::HEADER_LEN`] is the only size
//!   violation the wire can express (fatal `oversized` in the conventions'
//!   error taxonomy), and 65 535 is the exact edge for corpus entries.
//!
//! # fd-passing invariants
//!
//! - **At most one fd per message** ([`MAX_FDS_PER_MESSAGE`]), the protocol
//!   v0 constraint the codegen also enforces (`fd_count` is `0` or `1`).
//! - fds are matched to frames **positionally**, in byte-stream order, per
//!   conventions section 2.2: the ancillary payload rides with the frame's
//!   bytes in one `sendmsg`, and the receiver claims queued fds as frames
//!   complete. Matching is *strict*: the receiver records the byte-stream
//!   offset each fd arrived attached to (the kernel never coalesces a
//!   `recvmsg` across an `SCM_RIGHTS` boundary, so that offset is exact)
//!   and requires it to fall within the declaring frame's bytes. A frame
//!   declaring `fd_count = 1` whose bytes carry no fd is
//!   [`error::PeerViolation::MissingFd`]; an fd attached to the bytes of a
//!   frame that does not declare it -- "fds attached to a message that
//!   declares none", conventions 2.4 -- is
//!   [`error::PeerViolation::UnsolicitedFd`]. Both are enforced here
//!   because no layer above the transport can observe an undeclared fd,
//!   and an undetected one would shift positional matching for every later
//!   fd-bearing frame.
//! - A peer that ships fds attached to bytes that never complete a frame
//!   hits [`MAX_UNCLAIMED_FDS`] and dies with
//!   [`error::PeerViolation::UnclaimedFdOverflow`]; more fds in a single
//!   `sendmsg` than the receiver's ancillary buffer holds is
//!   [`error::PeerViolation::AncillaryTruncated`] (`MSG_CTRUNC`). Both are
//!   transport-level fd-bomb backstops; the graduated policy is P1.2.3.
//!
//! # CLOEXEC discipline
//!
//! Every fd this crate creates or receives is close-on-exec from birth --
//! there is no window where a concurrent `fork`/`exec` (the realm spawn
//! path, P1.5.2) could inherit it:
//!
//! - listening sockets, connecting sockets, and socketpairs: `SOCK_CLOEXEC`
//!   at `socket(2)`/`socketpair(2)` time;
//! - accepted connections: `accept4(2)` with `SOCK_CLOEXEC`;
//! - received fds: `recvmsg(2)` with `MSG_CMSG_CLOEXEC`;
//! - the listener's lock file: Rust's std opens all files `O_CLOEXEC`.
//!
//! # Socket path convention
//!
//! The principal-facing listener lives at `$XDG_RUNTIME_DIR/vitrin-0/core.sock`
//! and each spawned shim gets a private runtime directory under the same
//! root -- see [`paths`] for the constants and helpers. Shim connections to
//! the core do not use a named socket at all: they inherit one end of a
//! socketpair at fork ([`Connection::pair`] + [`Connection::from_fd`]),
//! which is what makes their realm identity assigned, never claimed
//! (conventions section 1.2).

//! # Cargo features
//!
//! - **`client`** (base) -- the blocking transport: [`Listener`],
//!   [`Connection`], framing, `SCM_RIGHTS` fd passing, and `SO_PEERCRED`.
//!   Pulls no calloop; this is the slice the Rust `vitrin-sdk` reference
//!   client links.
//! - **`server`** (default, implies `client`) -- adds the [`event_loop`]
//!   glue that plugs the transport into the trusted core's calloop loop, so
//!   `vitrind` services N connections as just another event source with no
//!   dedicated IPC thread and no async runtime (P1.2.2).
//!
//! The default is `server` so the workspace's own `cargo test`/`clippy`
//! (which pass no `--features`) exercise the core-side glue; a consumer that
//! wants only the light client selects `default-features = false,
//! features = ["client"]`.

#[cfg(feature = "client")]
pub mod connection;
#[cfg(feature = "client")]
pub mod error;
#[cfg(feature = "server")]
pub mod event_loop;
#[cfg(feature = "client")]
pub mod listener;
#[cfg(feature = "client")]
pub mod paths;

#[cfg(feature = "client")]
pub use connection::{Connection, Message, PeerCred};
#[cfg(feature = "client")]
pub use error::{LocalMisuse, PeerViolation, TransportError};
#[cfg(feature = "server")]
pub use event_loop::{
    reply, ConnectionEvent, ConnectionSource, DisconnectReason, ListenerEvent, ListenerSource,
};
#[cfg(feature = "client")]
pub use listener::Listener;
#[cfg(feature = "client")]
pub use paths::PathError;

// Re-exported so transport consumers can name the frame-header type that
// [`Message`] exposes without also depending on `vitrin-protocol` directly.
#[cfg(feature = "client")]
pub use vitrin_protocol::wire::{FrameHeader, HEADER_LEN};

/// Hard ceiling on a single frame's size in bytes, including its 8-byte
/// header: `u16::MAX`, the largest value the header's `size` field can
/// express. See the crate docs for the full rationale. A frame *declaring*
/// less than [`HEADER_LEN`] is the inverse violation
/// ([`error::PeerViolation::UndersizedSizeField`]).
#[cfg(feature = "client")]
pub const MAX_MESSAGE_SIZE: usize = u16::MAX as usize;

/// Protocol v0 allows at most one file descriptor per message (`fd_count`
/// is `0` or `1` -- the P1.1.2 codegen constraint). A received header
/// declaring more is [`error::PeerViolation::FdCountExceeded`].
#[cfg(feature = "client")]
pub const MAX_FDS_PER_MESSAGE: usize = 1;

/// Cap on received-but-not-yet-claimed fds queued on one [`Connection`],
/// and the per-`recvmsg` ancillary capacity. A compliant peer attaches
/// each fd to its declaring frame's own bytes, so its unclaimed queue
/// depth stays at 1 except for transient fragmentation; 8 is a generous
/// margin that still stops fd-bombs at the transport before the P1.2.3
/// policy layer even sees them.
///
/// This is a bound of *this implementation's* receive path, not a
/// normative wire rule: the conventions specify positional matching but
/// no ancillary batching ceiling. Whether one should become normative
/// (so the C shim and Python SDK agree by spec rather than by margin) is
/// flagged to the protocol track rather than legislated here.
#[cfg(feature = "client")]
pub const MAX_UNCLAIMED_FDS: usize = 8;

/// Cap on the bytes a [`Connection`]'s non-blocking send path will park for
/// a peer that is not reading ([`Connection::send_or_queue`]): **256 KiB**,
/// room for exactly four max-size frames (4 x 65 535 = 262 140 <= 262 144).
///
/// Sizing rationale (the reference posture is libwayland's: a client whose
/// outgoing buffer cannot drain is disconnected -- the compositor never
/// blocks on it and never buffers without bound):
///
/// - **On the order of what the kernel already buffers.** A default AF_UNIX
///   socket gives a peer roughly `net.core.wmem_default` (~208 KiB) of
///   in-flight slack; this queue about doubles it. A compliant client that
///   falls more than ~half a MiB behind on a local socket is not "slow", it
///   has stopped reading.
/// - **Real replies are small.** Per-argument bounds keep every schema-legal
///   v0 message under 35 KiB (see [`MAX_MESSAGE_SIZE`]); four *maximum*
///   frames of headroom already assumes a pathological burst.
/// - **Bounded hostile cost.** Worst case a hostile connection pins
///   `MAX_SEND_QUEUE_BYTES` + one receive scratch + one reassembly buffer,
///   so even hundreds of hostile connections cost the core tens of MiB, not
///   gigabytes.
/// - **Never less than one frame.** The cap exceeds [`MAX_MESSAGE_SIZE`],
///   so a single frame can always be parked into an empty queue: exceeding
///   the cap is always the peer's accumulated slowness, never one legal
///   message's size.
#[cfg(feature = "client")]
pub const MAX_SEND_QUEUE_BYTES: usize = 256 * 1024;
