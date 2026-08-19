// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! One module per protocol interface, in `protocol/vitrin-v0.xml` document order.

/// The `vitrin` protocol's single wire version integer (`protocol/@version`);
/// also the first argument of `vitrin_handshake::Hello`, whose accepted value
/// becomes the connection's negotiated version. A server implements every
/// version up to its maximum and refuses anything above it with
/// `version_unsupported` -- downgrade is refusal, not negotiation.
pub const PROTOCOL_VERSION: u32 = 2;

/// Total number of messages (requests + events) across every interface.
/// Exists so exhaustiveness can be *asserted* rather than assumed: a test
/// enumerating every message (e.g. the round-trip table) checks its own
/// length against this, so a message added to the IDL cannot ship silently
/// untested.
pub const MESSAGE_COUNT: usize = 48;

/// Every `interface/@verb` in the document, in interface document order:
/// the grant verbs that have a facet interface to be exercised through.
///
/// Exists so the facet-bearing/facetless partition of `vitrin_grant.verb`
/// can be *derived* rather than hand-listed. Before this constant the
/// partition was a hand-maintained array of per-interface `VERB` consts, and
/// the check over it could not see a seventh facet appearing on an existing
/// verb -- which is exactly the next scheduled protocol change. Adding an
/// `interface/@verb` now moves this length, and the assertions that pin it
/// go red.
pub const FACET_VERBS: &[&str] = &[
    "observe",
    "actuate_pointer",
    "actuate_text",
    "realm_launch",
    "layout_focus",
    "layout_arrange",
];

pub mod vitrin_handshake;
pub mod vitrin_principal;
pub mod vitrin_realm;
pub mod vitrin_grant;
pub mod vitrin_consent;
pub mod vitrin_view;
pub mod vitrin_actuator_pointer;
pub mod vitrin_actuator_text;
pub mod vitrin_shim_session;
pub mod vitrin_shim_surface;
pub mod vitrin_shim_seat;
pub mod vitrin_launcher;
pub mod vitrin_layout_focus;
pub mod vitrin_layout_arrange;
