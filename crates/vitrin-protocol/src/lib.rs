//! Vitrin OS wire protocol: message types and codec.
//!
//! This crate is pure data plus encode/decode logic -- **no I/O, no
//! sockets**. Getting bytes and file descriptors on and off a real
//! connection (including the `SCM_RIGHTS` transfer for the one fd-bearing
//! message on some interfaces) is a different crate's job.
//!
//! - [`error::DecodeError`] is this crate's own hand-written Rust-side error
//!   type for decode failures. It is distinct from
//!   [`generated::vitrin_handshake::Error`], which is wire *data* (the
//!   `vitrin_handshake.error` enum defined in `protocol/vitrin-v0.xml`) --
//!   see that type's bridge method [`error::DecodeError::to_wire_error`].
//! - [`fixed::Fixed`] is the 24.8 fixed-point wire type.
//! - [`wire`] holds the low-level, hand-written byte-buffer primitives every
//!   generated message's `encode`/`decode` is built from.
//! - [`generated`] holds one module per protocol interface, produced by
//!   `vitrin-scanner` from `protocol/vitrin-v0.xml`. Regenerate with
//!   `cargo xtask codegen`; never hand-edit anything under `src/generated/`.

pub mod error;
pub mod fixed;
pub mod generated;
pub mod wire;

pub use error::DecodeError;
pub use fixed::Fixed;
