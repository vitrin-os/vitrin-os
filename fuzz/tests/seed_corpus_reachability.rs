// SPDX-License-Identifier: Apache-2.0
//! Every checked-in seed must reach the path its filename claims.
//!
//! # Why this test exists
//!
//! `fuzz/corpus/<target>/` is hand-curated: each file is named for one wire
//! condition (`unsolicited_fd`, `attach_with_fd`, ...) and that name is the
//! only thing a reviewer reads. A seed whose bytes stop reaching that
//! condition is invisible in a `git diff` and silently buys nothing --
//! libFuzzer still replays it, still reports no crash, and the corpus still
//! *looks* like it covers the path.
//!
//! Two such seeds were found on 2026-07-25:
//!
//! * `protocol_decode/attach_with_fd` built its frame with a `fd_count = 0`
//!   header byte while `vitrin_shim_surface.attach` has `HAS_FD = true`, so
//!   `Attach::decode` returned `FdCountMismatch` on every single run and the
//!   successful-decode path (the fd-bearing round-trip the seed exists for)
//!   was never reached.
//! * `ipc_framing/unsolicited_fd` put its fd on a zero-length `sendmsg`;
//!   Linux discards `SCM_RIGHTS` on a zero-length `SOCK_STREAM` send, so the
//!   fd never arrived and the seed degenerated into an ordinary valid frame.
//!   `PeerViolation::UnsolicitedFd` was never seeded at all.
//!
//! # What this proves, and why it is not a model
//!
//! Each seed file is fed to the **real** decoder (`vitrin_protocol`) or the
//! **real** `vitrin_ipc::Connection` over a **real** `socketpair`, parsed by
//! the exact input layout its fuzz target documents, and the outcome is
//! compared to the outcome the seed's name claims. Nothing here re-implements
//! a decoder, so nothing here can drift into agreeing with a wrong answer:
//! if `decode` starts rejecting `attach_with_fd`, this test goes red.
//!
//! `fuzz/seed_corpus.py`'s `verify_seeds` checks the same claims
//! structurally, from the bytes alone, so the failure is also legible without
//! a Rust toolchain -- but *this* file is the authoritative half.
//!
//! Run it with:
//!
//! ```text
//! cargo test --manifest-path fuzz/Cargo.toml
//! ```
//!
//! It is an ordinary `cargo test`, not a fuzzing run: no nightly toolchain,
//! no `cargo-fuzz`, no libFuzzer runtime (the two `[[bin]]` targets set
//! `test = false`). That is deliberate -- the check has to be runnable by
//! anyone who edits `seed_corpus.py`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use vitrin_protocol::generated as gen;
use vitrin_protocol::DecodeError;

use vitrin_ipc::{Connection, PeerViolation, TransportError};

fn fuzz_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    let path = fuzz_dir().join("corpus").join(target).join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn seed_names(target: &str) -> Vec<String> {
    let dir = fuzz_dir().join("corpus").join(target);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()))
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// The decoder table's order, read out of the fuzz target's own source.
//
// `protocol_decode.rs` selects a decoder by `data[0] % DECODERS.len()`, so a
// seed's first byte is meaningless without that table's order. Parsing the
// order out of the source (rather than restating it here) means this test
// cannot drift from the target it is checking: reordering `decoder_table!`
// reorders this list too, and a seed whose claimed message no longer sits at
// its selector fails below.
// ---------------------------------------------------------------------------

fn decoder_table_order() -> Vec<String> {
    let src_path = fuzz_dir().join("fuzz_targets").join("protocol_decode.rs");
    let src = std::fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", src_path.display()));
    let start = src
        .find("static DECODERS")
        .expect("protocol_decode.rs must declare `static DECODERS`");
    let open = src[start..]
        .find("decoder_table!(")
        .expect("DECODERS must be built by the `decoder_table!` macro")
        + start
        + "decoder_table!(".len();
    // The invocation's arguments are bare type identifiers separated by
    // commas, so the first `)` after it is its close.
    let close = src[open..]
        .find(')')
        .expect("the decoder_table! invocation must be closed")
        + open;
    src[open..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("//"))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// protocol_decode
// ---------------------------------------------------------------------------

/// The outcome a `protocol_decode` seed claims. Every variant names a real
/// branch of the generated `decode`, in that function's own check order.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Decode {
    /// Every gate cleared and the message decoded. The only outcome that
    /// exercises the round-trip half of `try_decode`.
    Ok,
    FdCountMismatch,
    SizeMismatch,
    Truncated,
    EmbeddedNul,
}

/// `(seed name, decoder type name as written in `decoder_table!`, claim)`.
///
/// Mirrors `seed_corpus.py`'s `PROTOCOL_DECODE_CLAIMS`; the two are checked
/// against the same files, so a claim added on one side and not the other
/// fails the completeness assertion at the end of the test.
const PROTOCOL_DECODE_CLAIMS: &[(&str, &str, Decode)] = &[
    ("valid_hello", "Hello", Decode::Ok),
    ("truncated_below_header", "Hello", Decode::Truncated),
    ("size_field_mismatch", "Hello", Decode::SizeMismatch),
    (
        "fd_count_mismatch_none_expected",
        "Hello",
        Decode::FdCountMismatch,
    ),
    (
        "fd_count_mismatch_fd_expected",
        "FrameReady",
        Decode::FdCountMismatch,
    ),
    ("attach_with_fd", "Attach", Decode::Ok),
    ("embedded_nul_in_string", "Hello", Decode::EmbeddedNul),
];

fn devnull_fd() -> OwnedFd {
    File::open("/dev/null")
        .expect("/dev/null must be openable")
        .into()
}

fn classify(err: &DecodeError) -> Decode {
    match err {
        DecodeError::FdCountMismatch { .. } => Decode::FdCountMismatch,
        DecodeError::SizeMismatch { .. } => Decode::SizeMismatch,
        DecodeError::Truncated { .. } => Decode::Truncated,
        DecodeError::EmbeddedNul => Decode::EmbeddedNul,
        other => panic!("no seed claims {other:?}; add a `Decode` variant for it"),
    }
}

/// Run one decoder by the name `decoder_table!` gives it. Only the message
/// types the seed corpus actually selects need an arm: a seed selecting any
/// other decoder fails loudly rather than being silently skipped.
fn run_decoder(type_name: &str, bytes: &[u8], fd: Option<OwnedFd>) -> Decode {
    fn outcome<T>(r: Result<(u32, T), DecodeError>) -> Decode {
        match r {
            Ok(_) => Decode::Ok,
            Err(e) => classify(&e),
        }
    }
    match type_name {
        "Hello" => outcome(gen::vitrin_handshake::requests::Hello::decode(bytes, fd)),
        "FrameReady" => outcome(gen::vitrin_view::events::FrameReady::decode(bytes, fd)),
        "Attach" => outcome(gen::vitrin_shim_surface::requests::Attach::decode(
            bytes, fd,
        )),
        other => panic!(
            "a seed selects the `{other}` decoder, which this reachability test does not \
             know how to run. Add an arm above -- a seed nobody can check is the exact \
             rot this file exists to stop."
        ),
    }
}

#[test]
fn every_protocol_decode_seed_reaches_the_path_its_name_claims() {
    let order = decoder_table_order();
    assert_eq!(
        order.len(),
        gen::MESSAGE_COUNT,
        "the decoder table parsed out of protocol_decode.rs must cover every message \
         protocol/vitrin-v0.xml defines"
    );

    for &(name, type_name, claim) in PROTOCOL_DECODE_CLAIMS {
        let data = seed("protocol_decode", name);
        assert!(
            data.len() >= 2,
            "{name}: below protocol_decode's 2-byte input floor, so the target returns \
             without calling any decoder"
        );
        // Exactly the target's own input layout.
        let selector = data[0] as usize % order.len();
        let want_fd = data[1] & 1 == 1;
        let frame = &data[2..];

        assert_eq!(
            order[selector], type_name,
            "{name}: selector byte {} picks decoder #{selector} ({}), not the claimed {}",
            data[0], order[selector], type_name
        );

        let fd = if want_fd { Some(devnull_fd()) } else { None };
        let got = run_decoder(type_name, frame, fd);
        assert_eq!(
            got, claim,
            "{name}: this seed exists to reach {claim:?} in {type_name}::decode, but the \
             real decoder answered {got:?}. A seed that cannot reach its own path buys \
             no coverage -- fix the seed in fuzz/seed_corpus.py and regenerate."
        );
    }

    let claimed: Vec<&str> = PROTOCOL_DECODE_CLAIMS.iter().map(|c| c.0).collect();
    for name in seed_names("protocol_decode") {
        assert!(
            claimed.contains(&name.as_str()),
            "protocol_decode/{name} is checked in but claims nothing: add it to \
             PROTOCOL_DECODE_CLAIMS here and to seed_corpus.py, or delete it"
        );
    }
}

// ---------------------------------------------------------------------------
// ipc_framing
// ---------------------------------------------------------------------------

/// The outcome an `ipc_framing` seed claims after the receiver is drained.
#[derive(Debug, PartialEq, Eq, Clone)]
enum Framing {
    /// `n` frames came out, then a clean close.
    FramesThenEof(usize),
    /// The drain ended on this peer violation.
    Violation(PeerViolation),
    /// The peer closed mid-frame.
    Eof,
}

const IPC_FRAMING_CLAIMS: &[(&str, Framing)] = &[
    ("valid_frame_one_write", Framing::FramesThenEof(1)),
    ("valid_frame_split_mid_header", Framing::FramesThenEof(1)),
    (
        "undersized_size_field",
        Framing::Violation(PeerViolation::UndersizedSizeField { size: 4 }),
    ),
    (
        "unsolicited_fd",
        Framing::Violation(PeerViolation::UnsolicitedFd),
    ),
    ("missing_fd", Framing::Violation(PeerViolation::MissingFd)),
    ("truncated_after_header", Framing::Eof),
    (
        "fd_count_exceeded",
        Framing::Violation(PeerViolation::FdCountExceeded { fd_count: 2 }),
    ),
];

/// Byte-for-byte `ipc_framing.rs`'s `raw_sendmsg`, including its
/// "both empty -> send nothing" short circuit. Copied rather than shared
/// because the fuzz target is a `#![no_main]` binary that cannot be imported;
/// keeping it identical is what makes the replay below faithful.
fn raw_sendmsg(conn: &Connection, bytes: &[u8], fds: &[BorrowedFd<'_>]) {
    if bytes.is_empty() && fds.is_empty() {
        return;
    }
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(4))];
    let mut cmsg = rustix::net::SendAncillaryBuffer::new(&mut space);
    if !fds.is_empty() {
        let _ = cmsg.push(rustix::net::SendAncillaryMessage::ScmRights(fds));
    }
    let _ = rustix::net::sendmsg(
        conn.as_fd(),
        &[IoSlice::new(bytes)],
        &mut cmsg,
        rustix::net::SendFlags::NOSIGNAL | rustix::net::SendFlags::DONTWAIT,
    );
}

/// Replay one seed exactly as `ipc_framing.rs` would, and report what the
/// real `Connection` did with it.
fn replay(data: &[u8]) -> Framing {
    assert!(data.len() >= 3, "below ipc_framing's 3-byte input floor");
    let fds_first = (data[0] as usize) % 4;
    let fds_second = (data[1] as usize) % 4;
    let rest = &data[3..];
    let split = if rest.is_empty() {
        0
    } else {
        (data[2] as usize) % (rest.len() + 1)
    };
    let (first_bytes, second_bytes) = rest.split_at(split);

    let (sender, mut receiver) = Connection::pair().expect("socketpair");

    let first_fds: Vec<File> = (0..fds_first)
        .map(|_| File::open("/dev/null").expect("/dev/null"))
        .collect();
    let first_borrowed: Vec<BorrowedFd<'_>> = first_fds.iter().map(File::as_fd).collect();
    raw_sendmsg(&sender, first_bytes, &first_borrowed);

    let second_fds: Vec<File> = (0..fds_second)
        .map(|_| File::open("/dev/null").expect("/dev/null"))
        .collect();
    let second_borrowed: Vec<BorrowedFd<'_>> = second_fds.iter().map(File::as_fd).collect();
    raw_sendmsg(&sender, second_bytes, &second_borrowed);

    drop(sender);

    let mut frames = 0usize;
    for _ in 0..10_000 {
        match receiver.recv_message() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => return Framing::FramesThenEof(frames),
            Err(TransportError::PeerViolation(v)) => return Framing::Violation(v),
            Err(TransportError::Eof { .. }) => return Framing::Eof,
            Err(other) => panic!("no seed claims {other:?}; add a `Framing` variant for it"),
        }
    }
    panic!("the receiver never terminated: a reassembly bug, not a seed problem");
}

#[test]
fn every_ipc_framing_seed_reaches_the_path_its_name_claims() {
    for (name, claim) in IPC_FRAMING_CLAIMS {
        let data = seed("ipc_framing", name);
        let got = replay(&data);
        assert_eq!(
            &got, claim,
            "{name}: this seed exists to reach {claim:?} in vitrin_ipc's receive path, \
             but a real Connection over a real socketpair answered {got:?}. Fix the seed \
             in fuzz/seed_corpus.py and regenerate."
        );
    }

    let claimed: Vec<&str> = IPC_FRAMING_CLAIMS.iter().map(|c| c.0).collect();
    for name in seed_names("ipc_framing") {
        assert!(
            claimed.contains(&name.as_str()),
            "ipc_framing/{name} is checked in but claims nothing: add it to \
             IPC_FRAMING_CLAIMS here and to seed_corpus.py, or delete it"
        );
    }
}

/// No two seeds for a target may be byte-identical, and -- specifically --
/// `unsolicited_fd` must not be the stream `valid_frame_one_write` already
/// sends. The 2026-07-25 rot made those two deliver the same bytes with the
/// same (absent) ancillary payload, which is what let a dead seed sit in the
/// corpus looking alive.
#[test]
fn no_seed_duplicates_another() {
    for target in ["protocol_decode", "ipc_framing"] {
        let mut seen: BTreeMap<Vec<u8>, String> = BTreeMap::new();
        for name in seed_names(target) {
            let data = seed(target, &name);
            if let Some(prior) = seen.insert(data, name.clone()) {
                panic!("{target}/{name} is byte-identical to {target}/{prior}");
            }
        }
    }

    // The specific pair the rot collapsed, spelled out so a regression names
    // itself rather than surfacing as an anonymous duplicate.
    let unsolicited = seed("ipc_framing", "unsolicited_fd");
    let valid = seed("ipc_framing", "valid_frame_one_write");
    assert_ne!(
        unsolicited, valid,
        "unsolicited_fd must not be valid_frame_one_write's byte stream"
    );
    assert_ne!(
        replay(&unsolicited),
        replay(&valid),
        "unsolicited_fd must not merely differ in bytes -- it must reach a different \
         path in the receiver than valid_frame_one_write does"
    );
}

/// A cheap guard that the two halves of the check stay in step: the Python
/// generator's claim table and this file's must name the same seeds.
#[test]
fn python_and_rust_claim_tables_cover_the_same_seeds() {
    let script = fuzz_dir().join("seed_corpus.py");
    let text = std::fs::read_to_string(&script)
        .unwrap_or_else(|e| panic!("reading {}: {e}", script.display()));
    for &(name, _, _) in PROTOCOL_DECODE_CLAIMS {
        assert!(
            text.contains(&format!("\"{name}\"")),
            "seed_corpus.py never mentions protocol_decode/{name}"
        );
    }
    for (name, _) in IPC_FRAMING_CLAIMS {
        assert!(
            text.contains(&format!("\"{name}\"")),
            "seed_corpus.py never mentions ipc_framing/{name}"
        );
    }
    assert!(
        Path::new(&script).is_file(),
        "the generator must live beside the corpus it generates"
    );
}
