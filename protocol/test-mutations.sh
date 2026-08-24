#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Negative-mutation corpus for the Vitrin protocol IDL schema.
#
# Each case applies one illegal mutation to a copy of protocol/vitrin-v0.xml
# and asserts that protocol/vitrin-v0.rng rejects it. A mutation that still
# validates, or a sed pattern that no longer applies to the current XML, is a
# failure. The pristine document must validate (positive control).
#
# Usage: protocol/test-mutations.sh   (requires xmllint)
set -u

here="$(cd "$(dirname "$0")" && pwd)"
xml="$here/vitrin-v0.xml"
rng="$here/vitrin-v0.rng"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail=0

check_rejected() { # name, sed-script...
  local name="$1"
  shift
  sed "$@" "$xml" > "$tmp/$name.xml"
  if cmp -s "$xml" "$tmp/$name.xml"; then
    echo "FAIL  $name: sed pattern did not apply (XML drifted; update this case)"
    fail=1
    return
  fi
  if xmllint --noout --relaxng "$rng" "$tmp/$name.xml" 2>/dev/null; then
    echo "FAIL  $name: mutated document still validates"
    fail=1
  else
    echo "ok    $name"
  fi
}

# Positive control: the pristine document validates.
if xmllint --noout --relaxng "$rng" "$xml" 2>/dev/null; then
  echo "ok    positive-control (pristine document validates)"
else
  echo "FAIL  positive-control: pristine document does not validate"
  fail=1
fi

# Untyped new_id: strip @interface from hello's principal argument.
check_rejected strip-newid-interface \
  's|<arg name="principal" type="new_id" interface="vitrin_principal"|<arg name="principal" type="new_id"|'

# Eighth argument type: the closed set of seven admits no "array".
check_rejected eighth-arg-type \
  's|<arg name="cookie" type="uint" summary="client-chosen value echoed by done"/>|<arg name="cookie" type="array" summary="client-chosen value echoed by done"/>|'

# B2: a vitrin_shim_seat event without its origin tag.
check_rejected drop-seat-origin \
  '/<event name="motion">/,/<\/event>/{/<arg name="origin"/d}'

# B2: origin present but not the final argument.
check_rejected seat-origin-not-last \
  '/<event name="key">/,/<\/event>/{s|<arg name="keysym" type="uint" summary="xkbcommon keysym, modifier-resolved"/>|<arg name="origin" type="uint" enum="origin" summary="who caused this event"/><arg name="keysym" type="uint" summary="xkbcommon keysym, modifier-resolved"/>|; /^      <arg name="origin" type="uint" enum="origin" summary="who caused this event"\/>$/d}'

# B2: vitrin_shim_seat defines no requests.
check_rejected request-on-seat \
  's|<event name="motion">|<request name="bogus"><description summary="x">x</description></request><event name="motion">|'

# allow-null is legal only on string and object arguments.
check_rejected allow-null-on-uint \
  's|<arg name="cookie" type="uint" |<arg name="cookie" type="uint" allow-null="true" |'

# protocol/@version is required.
check_rejected no-protocol-version \
  's|<protocol name="vitrin" version="2">|<protocol name="vitrin">|'

# interface/@verb is a closed value set.
check_rejected bad-verb-value \
  's|verb="observe"|verb="observe_frames"|'

# ... and a verb name that reads like a grant verb but names no facet
# interface is rejected too: the set is the facet-bearing verbs, not the
# whole vitrin_grant.verb bitfield.
#
# vitrin-verb-set: facetless-verbs = observe_cursor
#
# ONE verb can play that part today, and it is the one that can never stop
# playing it: `observe_cursor` is facetless BY CONSTRUCTION -- it widens what
# capture_frame composites rather than adding a request, so no interface will
# ever carry the attribute for it.
#
# It was two until P2.7.2's second half. `egress` was facetless only YET, the
# comment here said so, and the facet then landed (vitrin_egress) -- at which
# point the schema admits the name and a mutation pinned on it would have
# become a legal document.
#
# That is not hypothetical, it is this case's own history, twice over. The
# mutation was ("realm_launch" -> "layout_arrange") until WS-E.1.4 gave
# `layout_arrange` an interface, at which point the mutated document was legal
# and this case reported the schema as broken when the schema was right. The
# comment that re-pinned it claimed the case "cannot go stale the same way
# again" and then went stale the same way again -- not the CASE, which still
# works, but the claim above it, which said `observe_cursor` was the only
# facetless verb left and was falsified by the very next verb the IDL gained.
# Pinning on the by-construction verb is what kept this case working through
# both.
#
# So the claim is a marker rather than a sentence: `cargo xtask verb-sets`
# derives the facetless set from the IDL and fails if the line above
# disagrees with it. Choosing the by-construction verb for the mutation is
# still the right choice; it just is not a claim about how many there are.
check_rejected verb-without-facet \
  's|verb="realm_launch"|verb="observe_cursor"|'

# Descriptions are required on every interface.
check_rejected drop-interface-description \
  '/<interface name="vitrin_consent" version="1">/,/<event name="state">/{/<description summary="consent prompt visibility (events only)">/,/<\/description>/d}'

# Enum references are legal only on int and uint arguments.
check_rejected enum-ref-on-fixed \
  '/<event name="motion">/,/<\/event>/{s|<arg name="x" type="fixed" summary="realm-view x"/>|<arg name="x" type="fixed" enum="axis" summary="realm-view x"/>|}'

# Every argument carries a summary.
check_rejected drop-arg-summary \
  's|<arg name="width" type="uint" summary="frame width in pixels"/>|<arg name="width" type="uint"/>|'

# Every string argument's summary carries its "(max N bytes)" bound token.
check_rejected string-summary-without-bound \
  's|summary="realm name (max 64 bytes); |summary="realm name; |'

# --- WS-E.2.1 (issue #213): the cross-realm clipboard messages ---------------
#
# Each of these is pinned on a message this issue ADDED, so they exercise the
# schema against the newest surface rather than against the oldest.

# The byte cap is the whole of what bounds a clipboard payload, and it lives in
# a machine-readable summary token. Dropping it from `selection`'s `data` must
# be a schema failure, not a message that silently accepts any length: the
# generated decoder reads the bound out of this token, so a missing one is an
# unbounded string argument in the trusted core's shim path.
check_rejected clipboard-data-without-bound \
  's|summary="the selection as UTF-8, empty unless status is ok (max 61440 bytes)"|summary="the selection as UTF-8, empty unless status is ok"|'

# The clipboard payload is a string, and the closed set of seven admits no
# `array` -- a payload that arrived as an array would have no bound token, no
# UTF-8 validation and no NUL rule.
check_rejected clipboard-data-as-array \
  's|<arg name="data" type="string" summary="the selection as UTF-8, empty unless status is ok (max 61440 bytes)"/>|<arg name="data" type="array" summary="the selection as UTF-8, empty unless status is ok (max 61440 bytes)"/>|'

# Enum entry values are required and immutable. An unvalued `selection_status`
# entry would leave the wire value to document order, which is exactly the
# renumbering hazard the "values are immutable" rule exists to forbid.
check_rejected clipboard-status-entry-without-value \
  's|<entry name="too_large" value="3" summary="the selection exceeds data'"'"'s byte bound"/>|<entry name="too_large" summary="the selection exceeds data'"'"'s byte bound"/>|'

# Enum references are legal only on int and uint arguments: `status` may not be
# carried by the string beside it.
check_rejected clipboard-status-enum-on-string \
  's|<arg name="mime" type="string" summary="MIME type of data, empty unless status is ok (max 32 bytes)"/>|<arg name="mime" type="string" enum="selection_status" summary="MIME type of data, empty unless status is ok (max 32 bytes)"/>|'

# Every enum carries a description.
check_rejected clipboard-status-without-description \
  '/<enum name="selection_status">/,/<\/enum>/{/<description summary="why a selection answer carries no data">/,/<\/description>/d}'

# --- P2.6.5 (issue #189): the powerbox messages ------------------------------
#
# Each case is pinned on a message this issue ADDED, for the reason the
# WS-E.2.1 clipboard block states: the corpus should exercise the schema
# against the newest surface, not only against the oldest.
#
# ONE OF #189's ACCEPTANCE CRITERIA IS DELIBERATELY NOT HERE. It asked for an
# `fd_count` mismatch case dying fatal `fd_violation`. `fd_count` is a byte of
# the frame header, not a construct of the IDL dialect, so no mutation of this
# document could be rejected for it and a case named for it would be
# exercising something else. It lives in
# `crates/vitrin-protocol/tests/decode_errors.rs` instead, against the real
# decoder and asserting the real wire code; the relocation is recorded as
# D-045 in `docs/plan/20-decision-log.md` so the criterion reads as moved
# rather than dropped.

# The @verb set is closed even though this issue WIDENED it. `designate_file`
# joining the choice list is a dialect change, and the risk a dialect change
# carries is that the list stops being closed at all -- so mutate the new value
# to a verb name the set does not admit. It must be rejected: an unadmitted
# verb name would otherwise reach a backend and emit a chokepoint entry
# nothing enforces.
#
# RE-PINNED FROM `egress` TO `observe_cursor`, and the reason is this file's
# oldest recorded failure mode arriving for the third time. The case was
# written as `verb="designate_file"` -> `verb="egress"` because `egress` was
# then "the next verb the allocation registry names (128, E2.7) and one with
# no facet interface today". P2.7.2 landed `vitrin_egress` in parallel, the
# schema's closed set gained `egress`, and the mutated document became LEGAL --
# so the case would have reported the schema as broken when the schema was
# right, exactly as `verb-without-facet` did when `layout_arrange` gained an
# interface. `observe_cursor` is facetless BY CONSTRUCTION and is the only
# name that cannot do this again; see the block above `verb-without-facet` for
# the full history. This case stays distinct from that one because it is
# pinned on the surface P2.6.5 added rather than on the launcher's.
check_rejected powerbox-verb-not-in-the-closed-set \
  's|verb="designate_file"|verb="observe_cursor"|'

# `designated` carries one of the protocol's five `fd` arguments, and
# `allow-null` is
# legal only on string and object arguments. An `allow-null` fd would be a
# descriptor argument with a null form, which the one-fd framing invariant has
# no way to express -- fd_count is 0 or 1 and is read from the header, not from
# the payload.
check_rejected designated-fd-allow-null \
  's|<arg name="fd" type="fd" summary="the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it"/>|<arg name="fd" type="fd" allow-null="true" summary="the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it"/>|'

# The display name is a bounded string, and the bound lives in a
# machine-readable summary token the generated decoder reads. Dropping it from
# `designated` must be a schema failure rather than an unbounded string
# argument arriving over the wire.
check_rejected designated-name-without-bound \
  's|summary="basename of what the human chose, for display only - never a path (max 255 bytes)"|summary="basename of what the human chose, for display only - never a path"|'

# Enum references are legal only on int and uint arguments: the designation's
# `kind` may not be carried by the string beside it.
check_rejected designation-kind-enum-on-string \
  's|<arg name="name" type="string" summary="basename of what the human chose, for display only - never a path (max 255 bytes)"/>|<arg name="name" type="string" enum="vitrin_powerbox.kind" summary="basename of what the human chose, for display only - never a path (max 255 bytes)"/>|'

# The argument-type set is closed at seven and admits no `array`. Retyping
# `request_file`'s `mode` is the cheapest way to exercise that on a message
# this issue added -- the mutation also drops the enum reference, so a schema
# that had quietly stopped rejecting `array` would be caught by either half.
#
# (Corrected twice, so what is true is stated plainly rather than patched
# again. This comment first described an array OF DESCRIPTORS, which no
# mutation here performs; the correction then named `designated-fd-allow-null`
# as the case exercising the one-fd rule, in the same sentence that said the
# schema does not model that rule -- and pointed the wrong way to find it. The
# one-fd rule is a framing invariant no mutation of this document can reach:
# fd_count is a header byte. `designated-fd-allow-null` exercises the dialect
# rule that `allow-null` is admitted only on string and object arguments, which
# is why an `fd` argument cannot be nullable; the framing invariant is that
# case's motivation, not what its rejection proves. The invariant itself is
# checked only at runtime, by decode_errors.rs's fd_count pair. Cases here are
# cross-referenced by name and never by direction -- a direction is a claim
# about this file's order that nothing checks and any reordering falsifies
# silently.)
check_rejected powerbox-mode-as-array \
  's|<arg name="mode" type="uint" enum="mode" summary="the access this ask is for; the human may narrow it, and designated.mode carries what was actually approved"/>|<arg name="mode" type="array" summary="the access this ask is for; the human may narrow it, and designated.mode carries what was actually approved"/>|'

# `get_powerbox` is a structural mint, and a new_id argument MUST name the
# interface it mints -- an untyped new_id would leave codegen with nothing to
# emit and a client with no way to know what it just allocated. Pinned on the
# mint this issue added rather than on one of the three that came before it.
check_rejected get-powerbox-new-id-without-interface \
  's|<arg name="powerbox" type="new_id" interface="vitrin_powerbox" |<arg name="powerbox" type="new_id" |'

# Enum references are legal only on int and uint arguments. `vitrin_powerbox`
# defines a refusal voice of its own, distinct from vitrin_grant.refused, and
# its one argument is the enum that carries the code: a string form of it
# would put an unbounded, uncheckable name where a closed set belongs.
check_rejected powerbox-refused-code-as-string \
  's|<arg name="code" type="uint" enum="refusal" summary="why the ask produced no descriptor"/>|<arg name="code" type="string" enum="refusal" summary="why the ask produced no descriptor (max 32 bytes)"/>|'

# Enum entry values are required and immutable. An unvalued `refusal` entry
# would leave the wire value to document order -- the renumbering hazard the
# "values are immutable" rule exists to forbid.
check_rejected powerbox-refusal-entry-without-value \
  's|<entry name="cancelled" value="0" |<entry name="cancelled" |'

# Every request carries a description. `request_dir` has no arguments at all,
# so its description is the whole of what the message documents.
check_rejected powerbox-request-dir-without-description \
  '/<request name="request_dir" since="2">/,/<\/request>/{/<description summary="ask the human to designate one directory subtree">/,/<\/description>/d}'

# --- P2.7.2 (issue #196), first half: the egress verb bit --------------------
#
# The surface that half added is one `vitrin_grant.verb` entry, and these two
# cases are pinned there for the same reason the clipboard block above is
# pinned on its own: a case that exercises the schema against the newest
# surface is the one that catches a rule the newest surface forgot.
#
# A verb entry without its value would leave the wire bit to document order --
# and a verb bit is allocated once, repo-wide, and is immutable, so "whatever
# position it happens to sit in" is the one thing it must never be.
check_rejected egress-entry-without-value \
  's|<entry name="egress" value="128" |<entry name="egress" |'

# A verb entry without a summary is a bit with no stated authority. The
# consent prompt, the SDK's dotted name and the served/unserved derivation in
# `sdk/python/tests/test_verb_parity.py` all read that text; an entry that
# carried none would be an authority nobody can describe to a human.
check_rejected egress-entry-without-summary \
  's|<entry name="egress" value="128" summary="[^"]*"/>|<entry name="egress" value="128"/>|'

# --- P2.7.2 (issue #196), second half: the egress facet ----------------------
#
# That half added four messages -- `vitrin_grant.get_egress` and
# `vitrin_egress`'s `request_connect`, `connected` and `connect_failed` -- and
# the block above said, correctly at the time, that a task adding no message
# has no per-message case to write. This one does, so there is one case per
# added message, each pinned on a DIFFERENT schema rule so the block is
# coverage rather than repetition. Two of the four rules below were not
# exercised anywhere in this file before: `fd` arguments are bare, and a
# per-MESSAGE description is required (only the per-interface one was covered).

# `get_egress`: a `new_id` argument must name its interface. An untyped
# `new_id` is the one shape this dialect refuses outright -- codegen is a
# straight-line table and an untyped mint would have nothing to bind the id
# to, so the facet would exist on the wire with no interface behind it.
check_rejected get-egress-untyped-newid \
  's|<arg name="egress" type="new_id" interface="vitrin_egress"|<arg name="egress" type="new_id"|'

# `request_connect`: a string argument's summary carries its machine-readable
# "(max N bytes)" bound, and the generated decoder reads the bound out of that
# token. Dropping it here would put an unbounded, attacker-supplied host
# string into the trusted core's enforcement path -- which is the argument the
# chokepoint compares against the grant's selector.
check_rejected request-connect-host-without-bound \
  's|summary="the host half of the grant'"'"'s net: selector, byte-exact, IPv6 literals WITHOUT brackets (max 253 bytes)"|summary="the host half of the grant'"'"'s net: selector, byte-exact, IPv6 literals WITHOUT brackets"|'

# `connected`: `fd` arguments are BARE -- name, type, summary and nothing
# else. `allow-null` is legal only on string and object arguments, and a
# nullable fd is not a thing the wire can express: fd presence is carried by
# the header's `fd_count` byte and by the ancillary payload, not by a value in
# the buffer, so "null" would have no encoding. (The existing
# `allow-null-on-uint` case covers the scalar arm of the same rule; this is
# the fd arm, which nothing covered.)
check_rejected connected-fd-allow-null \
  's|<arg name="fd" type="fd" summary="the connected stream socket, owned by the receiving principal"/>|<arg name="fd" type="fd" allow-null="true" summary="the connected stream socket, owned by the receiving principal"/>|'

# `connect_failed`: every request and every event carries a description, not
# just every interface. `drop-interface-description` above covers the
# interface arm; this is the per-message arm, and it matters most for exactly
# this message -- a terminal event whose whole contribution is a distinction
# ("the far end did not answer" versus "the authority was withheld") that
# exists nowhere but in its prose.
check_rejected connect-failed-without-description \
  '/<event name="connect_failed" since="2">/,/<\/event>/{/<description summary="an admitted request_connect that the far end did not answer">/,/<\/description>/d}'

# The `failure` enum's entry values are required and immutable, on exactly the
# terms `clipboard-status-entry-without-value` states: an unvalued entry would
# leave the wire value to document order, and this enum's values are what a
# client routes its retry decision on. Pinned on `resolution_failed` rather
# than on `timed_out`, which reads like the obvious choice and is NOT unique
# in this document -- `vitrin_grant.outcome` has a `timed_out` = 2 entry too,
# so a sed on that name mutates two enums and the case stops being pinned on
# the surface its name claims.
check_rejected egress-failure-entry-without-value \
  's|<entry name="resolution_failed" value="3" |<entry name="resolution_failed" |'

if [ "$fail" -ne 0 ]; then
  echo "test-mutations: FAILURES"
  exit 1
fi
echo "test-mutations: all cases passed"
