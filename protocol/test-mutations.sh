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

# interface/@verb is a closed four-value set.
check_rejected bad-verb-value \
  's|verb="observe"|verb="observe_frames"|'

# ... and a verb name that reads like a grant verb but names no facet
# interface is rejected too: the set is the facet-bearing verbs, not the
# whole vitrin_grant.verb bitfield.
check_rejected verb-without-facet \
  's|verb="realm_launch"|verb="layout_arrange"|'

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

if [ "$fail" -ne 0 ]; then
  echo "test-mutations: FAILURES"
  exit 1
fi
echo "test-mutations: all cases passed"
