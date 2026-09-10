// SPDX-License-Identifier: Apache-2.0
//! Frame builders for the powerbox's two terminals and the realm's copy.
//!
//! # Why these exist rather than the generated encoders being called directly
//!
//! Two independent reasons, and each alone would be enough.
//!
//! 1. **The fd-bearing messages own their descriptor.**
//!    `vitrin_powerbox.designated` and `vitrin_shim_session.designation` both
//!    declare an `fd` argument, so the generated struct holds an `OwnedFd` --
//!    but the byte frame does not contain it (it rides `SCM_RIGHTS`
//!    out-of-band). A sender that has *one* descriptor to hand to *two*
//!    receivers therefore cannot build either struct without giving up
//!    ownership of the thing it still has to send twice. These builders take
//!    no descriptor at all: they produce exactly the bytes, and the caller
//!    passes the same borrowed fd alongside each of the two writes.
//!
//! 2. **`vitrin-core` may not utter the identifier `Refused`.** Its
//!    `single_enforcement_path_is_grep_provable` test counts whole-identifier
//!    occurrences of the `vitrin_grant.refused` event type across the whole
//!    core and requires them to be exactly two, both in `enforcement.rs`.
//!    That census is deliberately blind to *which* enum a bare `Refused`
//!    belongs to, because a census that could be talked out of a hit is not a
//!    census. So the powerbox's own unrelated `refused` event is built here,
//!    behind a lowercase name, and the core never spells the identifier.
//!
//! # These bytes are pinned to the generated encoders, not merely believed to
//! match them
//!
//! Each builder writes the argument list by hand through [`crate::wire`],
//! which is a second expression of a layout the generator already owns. The
//! tests below construct the generated message and assert **byte equality**
//! with the builder's output, for every enum value in each field's domain --
//! so an argument appended to either message in `protocol/vitrin-v0.xml`
//! makes this module fail rather than silently emit a short frame.

use crate::generated::vitrin_powerbox::{Kind, Mode, Refusal};
use crate::generated::{vitrin_powerbox, vitrin_shim_session};

/// The longest `name` either designation message carries (`string@max`).
const NAME_MAX: u32 = 255;

/// Turn a filename's raw bytes into the `name` argument these two messages
/// carry.
///
/// # Why this exists rather than the caller writing `from_utf8_lossy`
///
/// It did, and that was a **panic in the trusted core reachable from a
/// filename**. `crate::wire::write_string` asserts its `string@max` bound, and
/// `String::from_utf8_lossy` does not preserve length: every byte that is not
/// valid UTF-8 becomes U+FFFD, which is *three* bytes. A 255-byte name — the
/// longest a Linux filesystem will hold, so an ordinary one — made of bytes
/// that are not valid UTF-8 becomes 765 bytes, and the assertion fires inside
/// the dispatch callback that was about to hand the descriptor over. The
/// display server dies, and with it every realm on it, because a human picked
/// a file whose name was not UTF-8.
///
/// So the clamp lives here, beside the bound it is clamping to, rather than at
/// each of the two call sites where forgetting it is silent until the day
/// somebody has such a file.
///
/// # It truncates, and says so rather than refusing
///
/// `name` is **display only** — the descriptor is already open and nothing
/// resolves this string — so a name the wire cannot carry whole must not fail
/// a designation the human already made. It is cut at a `char` boundary, so
/// the result is still UTF-8 and still one value the receiver can decode; a
/// cut in the middle of a replacement character would produce bytes no decoder
/// accepts.
pub fn display_name(raw: &[u8]) -> String {
    let mut name = String::from_utf8_lossy(raw).into_owned();
    if name.len() > NAME_MAX as usize {
        // `floor_char_boundary` is unstable, so the boundary is walked here.
        let mut cut = NAME_MAX as usize;
        while cut > 0 && !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
    }
    name
}

/// `vitrin_powerbox.designated` -- the asking agent's copy, minus the fd.
///
/// `object_id` is the powerbox facet the ask arrived on. The caller sends the
/// descriptor alongside these bytes.
pub fn powerbox_designated_frame(
    object_id: u32,
    designation_id: u32,
    kind: Kind,
    mode: Mode,
    name: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    crate::wire::FrameHeader {
        object_id,
        size: 0,
        opcode: vitrin_powerbox::events::Designated::OPCODE,
        fd_count: vitrin_powerbox::events::Designated::HAS_FD as u8,
    }
    .encode_with_placeholder_size(&mut out);
    crate::wire::write_uint(&mut out, designation_id);
    crate::wire::write_uint(&mut out, kind.to_wire());
    crate::wire::write_uint(&mut out, mode.to_wire());
    crate::wire::write_string(&mut out, name, NAME_MAX);
    crate::wire::patch_size(&mut out);
    out
}

/// `vitrin_powerbox.refused` -- the other terminal of an admitted ask.
///
/// Lowercase by necessity, not by style: see this module's second reason.
pub fn powerbox_refusal_frame(object_id: u32, code: Refusal) -> Vec<u8> {
    let mut out = Vec::new();
    crate::wire::FrameHeader {
        object_id,
        size: 0,
        opcode: vitrin_powerbox::events::Refused::OPCODE,
        fd_count: vitrin_powerbox::events::Refused::HAS_FD as u8,
    }
    .encode_with_placeholder_size(&mut out);
    crate::wire::write_uint(&mut out, code.to_wire());
    crate::wire::patch_size(&mut out);
    out
}

/// `vitrin_shim_session.designation` -- the realm's copy, minus the fd.
///
/// `object_id` is the shim's session object. The descriptor sent alongside
/// these bytes MUST be the same one the agent's copy carried: two opens are
/// two race windows and can name two different inodes, which would make the
/// single `(st_dev, st_ino)` pair the core journals a claim about only one of
/// them.
pub fn shim_designation_frame(
    object_id: u32,
    designation_id: u32,
    kind: Kind,
    mode: Mode,
    name: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    crate::wire::FrameHeader {
        object_id,
        size: 0,
        opcode: vitrin_shim_session::events::Designation::OPCODE,
        fd_count: vitrin_shim_session::events::Designation::HAS_FD as u8,
    }
    .encode_with_placeholder_size(&mut out);
    crate::wire::write_uint(&mut out, designation_id);
    crate::wire::write_uint(&mut out, kind.to_wire());
    crate::wire::write_uint(&mut out, mode.to_wire());
    crate::wire::write_string(&mut out, name, NAME_MAX);
    crate::wire::patch_size(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any real descriptor: the generated struct needs one to exist, and
    /// nothing about these bytes depends on what it names.
    fn a_descriptor() -> std::os::fd::OwnedFd {
        std::fs::File::open("/dev/null")
            .expect("/dev/null opens")
            .into()
    }

    /// **The pin.** Hand-written bytes must equal the generated encoder's,
    /// over every value of every enum-typed argument.
    #[test]
    fn the_agent_copy_matches_the_generated_encoder() {
        for kind in Kind::ALL {
            for mode in Mode::ALL {
                for name in ["", "notes.txt", &"n".repeat(NAME_MAX as usize)] {
                    let generated = vitrin_powerbox::events::Designated {
                        fd: a_descriptor(),
                        designation_id: 7,
                        kind: *kind,
                        mode: *mode,
                        name: name.to_owned(),
                    }
                    .encode(20);
                    assert_eq!(
                        powerbox_designated_frame(20, 7, *kind, *mode, name),
                        generated,
                        "the hand-written designated frame drifted from the generated encoder \
                         ({kind:?}/{mode:?}, name len {})",
                        name.len()
                    );
                }
            }
        }
    }

    #[test]
    fn the_realm_copy_matches_the_generated_encoder() {
        for kind in Kind::ALL {
            for mode in Mode::ALL {
                let generated = vitrin_shim_session::events::Designation {
                    fd: a_descriptor(),
                    designation_id: 3,
                    kind: *kind,
                    mode: *mode,
                    name: "photo.jpg".into(),
                }
                .encode(11);
                assert_eq!(
                    shim_designation_frame(11, 3, *kind, *mode, "photo.jpg"),
                    generated,
                    "the hand-written designation frame drifted from the generated encoder"
                );
            }
        }
    }

    #[test]
    fn the_refusal_matches_the_generated_encoder() {
        for code in Refusal::ALL {
            let generated = vitrin_powerbox::events::Refused { code: *code }.encode(20);
            assert_eq!(
                powerbox_refusal_frame(20, *code),
                generated,
                "the hand-written powerbox refusal frame drifted from the generated encoder"
            );
        }
    }

    /// The frames a receiver actually gets back are the messages they claim
    /// to be -- decoded through the generated decoder, not merely compared
    /// against another encoder.
    #[test]
    fn a_built_frame_decodes_as_its_message() {
        let bytes = powerbox_designated_frame(20, 9, Kind::Directory, Mode::ReadWrite, "src");
        let (object_id, msg) =
            vitrin_powerbox::events::Designated::decode(&bytes, Some(a_descriptor()))
                .expect("the built frame decodes");
        assert_eq!(object_id, 20);
        assert_eq!(msg.designation_id, 9);
        assert_eq!(msg.kind, Kind::Directory);
        assert_eq!(msg.mode, Mode::ReadWrite);
        assert_eq!(msg.name, "src");

        let bytes = powerbox_refusal_frame(20, Refusal::Busy);
        let (object_id, msg) = vitrin_powerbox::events::Refused::decode(&bytes, None)
            .expect("the built frame decodes");
        assert_eq!(object_id, 20);
        assert_eq!(msg.code, Refusal::Busy);
    }

    /// **A filename that is not UTF-8 must not kill the display server.**
    ///
    /// The bug this pins: the two callers used to pass
    /// `String::from_utf8_lossy(name)` straight into a builder, and lossy
    /// conversion *grows* — one invalid byte becomes three. A 255-byte name of
    /// invalid bytes is 765 bytes of replacement characters, and
    /// `write_string`'s `string@max` assertion fires inside the dispatch
    /// callback that was handing the descriptor over. That is a panic in the
    /// trusted core reachable from a file the human picked.
    ///
    /// Both halves are asserted, because either alone would be misleading: the
    /// frame is built at all (no panic), **and** it decodes, which is what
    /// proves the truncation landed on a `char` boundary rather than in the
    /// middle of a replacement character.
    #[test]
    fn a_name_that_is_not_utf8_is_carried_rather_than_fatal() {
        // 100 bytes no decoder accepts -> 300 bytes of U+FFFD, past the bound.
        let raw = vec![0xffu8; 100];
        let name = display_name(&raw);
        assert!(
            name.len() <= NAME_MAX as usize,
            "the clamp let {} bytes through a {NAME_MAX}-byte bound",
            name.len()
        );
        assert!(
            name.len() > NAME_MAX as usize - 3,
            "control: this input must actually reach the bound, or the clamp is untested here"
        );

        let bytes = powerbox_designated_frame(20, 1, Kind::File, Mode::Read, &name);
        let (_, msg) = vitrin_powerbox::events::Designated::decode(&bytes, Some(a_descriptor()))
            .expect("a clamped name must still decode: the cut must be on a char boundary");
        assert_eq!(msg.name, name);

        let bytes = shim_designation_frame(11, 1, Kind::File, Mode::Read, &name);
        let (_, msg) =
            vitrin_shim_session::events::Designation::decode(&bytes, Some(a_descriptor()))
                .expect("the realm's copy too");
        assert_eq!(msg.name, name);

        // A name that already fits is passed through unchanged, so this is a
        // clamp and not a mangler.
        assert_eq!(display_name(b"notes.txt"), "notes.txt");
    }
}
