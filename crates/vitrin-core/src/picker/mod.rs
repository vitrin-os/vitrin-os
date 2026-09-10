// SPDX-License-Identifier: MPL-2.0
//! The core-drawn file picker: the gesture by which a human hands an agent one
//! file, and nothing else.
//!
//! # What makes this different from a file dialog
//!
//! An ordinary file dialog returns a **path**, and a path is a request the
//! opener has to re-resolve. Between the human choosing and the program
//! opening, anything with write access to a parent directory can swap a
//! component — so the thing approved and the thing opened need not be the same
//! thing. That race is not a bug in any particular dialog; it is what
//! returning a name instead of a thing means.
//!
//! This picker returns an **already-open descriptor**. The human's choice and
//! the core's `openat2` happen with no window between them in which a name
//! could be re-pointed, and what reaches the agent is kernel authority over
//! one inode. "Designation is authorization" is that sentence made literal.
//!
//! # The five modules
//!
//! * [`keys`] — which keys drive it, and the one that deliberately does not.
//! * [`listing`] — one directory, in which no two rows may look alike.
//! * [`resolve`] — the race-free walk from a held directory descriptor.
//! * [`delivery`] — descriptors opened and not yet handed over.
//! * [`session`] — one raised picker's state: where the human is standing,
//!   and the two descriptors every move is made through.
//!
//! # What this module does not decide
//!
//! Authority. Every designation passes the same chokepoint every other verb
//! does; nothing here consults a grant table, and there is no path from a row
//! a human touched to a descriptor that does not go back through
//! [`crate::designation::Ledger::redeem`], which re-derives the answer at the
//! instant of delivery rather than trusting the one taken when the card went
//! up. A card can be on screen for ninety seconds, and a grant can die in far
//! less.

pub(crate) mod delivery;
pub(crate) mod keys;
pub(crate) mod listing;
pub(crate) mod resolve;
pub(crate) mod session;
