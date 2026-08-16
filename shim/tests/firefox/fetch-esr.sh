#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Fetch and CHECKSUM-VERIFY the pinned Firefox ESR (P1.6.4, issue #36).
#
# The pin lives in firefox-esr.pin beside this script and nowhere else. This
# script's only job is to turn that pin into a verified unpacked browser at a
# known path, idempotently, without sudo and without touching anything the
# repo tracks.
#
# THE VERIFICATION IS THE POINT, so it is arranged to be unskippable: the
# tarball is downloaded to a temporary name, hashed, compared against the pin,
# and only THEN moved into place. A download that fails the comparison is
# deleted, and the script exits nonzero having unpacked nothing -- there is no
# path through this file that leaves a browser on disk whose bytes were not
# checked. (Downloading straight to the final name and hashing afterwards
# would leave a corrupt or substituted tarball sitting where a later run, or
# an impatient human, would treat it as already fetched.)
#
# THE ACCEPTANCE SCRIPT MUST NEVER REACH THE NETWORK, which is why
# `--verify-only` exists. The tarball is ~75 MB and reclaiming it while keeping
# the unpacked browser is a normal thing for a developer to do; a verify step
# that answers "tarball missing" by silently re-downloading turns every
# acceptance run into a CDN fetch, and a network flake into a red test on a
# machine that has a perfectly good pinned browser on disk. So the unpack
# records the hash it was verified against (see PROVENANCE below) and
# `--verify-only` can attest the pin from that alone, offline.
#
# TWO CHECKS, NOT ONE (issue #298). A checksum over the tarball answers "what
# was DOWNLOADED". Until 2026-08-16 that was the only question this script
# asked -- and the answer to it stayed true while the answer to the question
# that actually matters, "what is INSTALLED", changed underneath it. Firefox
# ships an updater that rewrites the unpacked tree in place; on 2026-07-22 it
# did, replacing the pinned 140.12.0esr on the development machine with
# 140.13.0esr while the tarball beside it still hashed to the pin.
# `--verify-only` reported the pin intact throughout, so anything that took
# `--verify-only` as its attestation -- notably the real-core milestone gate,
# tests/integration/test_real_firefox.py, and the CI step that runs it -- was
# attributing its results to a build it was not running.
#
# What the drift was NOT invisible to: shim/tests/acceptance/firefox_bringup.sh
# compares `firefox --version` against the pin itself and would have gone red
# on this particular update. That check is narrower than the one below (a
# respin carries the same version string) and it guards only that script, so
# it is kept and no longer relied on -- see its comment at the pin-verify step.
#
# So there are now two checks and one prevention:
#
#   1. verify()        the tarball's sha256                -- what was downloaded
#   2. verify_tree()   application.ini's Version + BuildID -- what is installed
#   3. harden_tree()   policies.json + a non-writable tree -- so 2 keeps holding,
#                      and verify_hardening() so that losing 3 is red, not silent
#
# Usage:
#   bash shim/tests/firefox/fetch-esr.sh                # fetch if absent, verify always
#   bash shim/tests/firefox/fetch-esr.sh --force        # re-download even if present
#   bash shim/tests/firefox/fetch-esr.sh --print        # print the resolved binary path
#   bash shim/tests/firefox/fetch-esr.sh --verify-only  # verify the pin, NEVER download
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
# shellcheck source=firefox-esr.pin
source "$HERE/firefox-esr.pin"

DEST="$REPO_ROOT/$VITRIN_FIREFOX_DIR"
TARBALL="$DEST/firefox-$VITRIN_FIREFOX_VERSION.tar.xz"
TREE="$DEST/firefox"
BINARY="$TREE/firefox"
# What the UNPACKED tree says about itself, and where the updater is switched
# off. Both are inside $TREE, so `rm -rf "$TREE"` invalidates both at once and
# the re-unpack rebuilds them -- there is no state here that can outlive the
# browser it describes.
APPINI="$TREE/application.ini"
POLICIES_DIR="$TREE/distribution"
POLICIES="$POLICIES_DIR/policies.json"
# application.ini's [App] Version for the pinned tarball. DERIVED by stripping
# the `esr` suffix, which lives in the display version only: the tarball is
# `140.12.0esr` and its application.ini says `140.12.0`. Derived rather than
# pinned separately because firefox-esr.pin's opening sentence -- "there is
# exactly one place the version lives" -- is worth keeping true.
APPINI_VERSION="${VITRIN_FIREFOX_VERSION%esr}"
# PROVENANCE. Written only after a tarball has been checksum-verified AND
# unpacked, recording which hash the browser now sitting in $TREE came
# from. It is what lets `--verify-only` attest the pin once the tarball is
# gone. It lives beside the tarball rather than inside $TREE because
# `rm -rf "$TREE"` is how a re-download invalidates an old unpack --
# the stamp is rewritten on the next unpack, and a stamp whose hash does not
# match the pin is treated as no stamp at all.
STAMP="$DEST/.provenance"

force=0
print_only=0
verify_only=0
for arg in "$@"; do
	case "$arg" in
	--force) force=1 ;;
	--print) print_only=1 ;;
	--verify-only) verify_only=1 ;;
	*) echo "usage: $0 [--force] [--print] [--verify-only]" >&2; exit 2 ;;
	esac
done

if (( print_only )); then
	echo "$BINARY"
	exit 0
fi

if (( force && verify_only )); then
	echo "FAIL: --force and --verify-only contradict each other" >&2
	exit 2
fi

mkdir -p "$DEST"

verify() {
	local file="$1"
	local got
	got="$(sha256sum "$file" | cut -d' ' -f1)"
	if [[ "$got" != "$VITRIN_FIREFOX_SHA256" ]]; then
		echo "FAIL: checksum mismatch for $file" >&2
		echo "  expected: $VITRIN_FIREFOX_SHA256" >&2
		echo "  got:      $got" >&2
		return 1
	fi
	return 0
}

# Attest the pin without the tarball: the unpack recorded the hash it came
# from, and the binary it produced is still here. Weaker than hashing the
# tarball -- it trusts a file this script wrote rather than re-deriving the
# bytes -- so it is only ever a FALLBACK, never preferred over the tarball.
stamp_attests() {
	[[ -f "$STAMP" ]] || return 1
	local stamped
	stamped="$(sed -n 's/^sha256=//p' "$STAMP")"
	[[ "$stamped" == "$VITRIN_FIREFOX_SHA256" ]] || return 1
	[[ -x "$BINARY" ]] || return 1
	return 0
}

# ---------------------------------------------------------------------------
# CHECK 2: what is INSTALLED (issue #298)
# ---------------------------------------------------------------------------
#
# verify() above proves the bytes that arrived; this proves the bytes that
# run. They are different facts, and the gap between them is exactly the bug
# this function exists for: Firefox's updater replaces the unpacked binaries
# and leaves the tarball untouched.
#
# The tree is asked to identify itself two ways, because each alone has a
# hole:
#
#   * application.ini's [App] Version and BuildID -- what the updater rewrites
#     when it lands a new build. BuildID as well as Version, because a respin
#     carries the same version number and different bytes, and "the version
#     matches" would then be a true sentence about the wrong build.
#   * `firefox --version` -- the binary's own claim. Weaker on its own (it is
#     the artefact vouching for itself, and it is the file an updater
#     replaces) but it is the one check that reads the executable rather than
#     a text file sitting next to it, so a tree whose application.ini was
#     restored by hand and whose binary was not still fails here.
#
# Neither is a cryptographic identity: this attests the tree Mozilla's updater
# would have moved, not every byte under $TREE. Hashing the whole unpacked
# tree would be the stronger claim and is not made here -- see the note in
# shim/docs/firefox.md section 1 for what this check does and does not cover.
ini_value() {
	# First match only: application.ini has one [App] section and the keys are
	# unique within it, but `head -n 1` makes that an assumption this function
	# does not depend on.
	sed -n "s/^$1=//p" "$APPINI" 2>/dev/null | head -n 1
}

verify_tree() {
	if [[ ! -f "$APPINI" ]]; then
		echo "FAIL: no application.ini at $APPINI." >&2
		echo "  nothing is unpacked at $TREE, or what is there is not a Firefox." >&2
		return 1
	fi
	local rc=0 got_version got_buildid reported
	got_version="$(ini_value Version)"
	got_buildid="$(ini_value BuildID)"
	if [[ "$got_version" != "$APPINI_VERSION" ]]; then
		echo "FAIL: the UNPACKED tree is not the pinned build (application.ini Version)." >&2
		echo "  expected: $APPINI_VERSION   (pin: $VITRIN_FIREFOX_VERSION)" >&2
		echo "  got:      ${got_version:-<absent>}" >&2
		rc=1
	fi
	if [[ "$got_buildid" != "$VITRIN_FIREFOX_BUILDID" ]]; then
		echo "FAIL: the UNPACKED tree is not the pinned build (application.ini BuildID)." >&2
		echo "  expected: $VITRIN_FIREFOX_BUILDID" >&2
		echo "  got:      ${got_buildid:-<absent>}" >&2
		rc=1
	fi
	reported="$("$BINARY" --version 2>/dev/null || true)"
	if [[ "$reported" != *"$VITRIN_FIREFOX_VERSION"* ]]; then
		echo "FAIL: pinned $VITRIN_FIREFOX_VERSION but the binary reports '${reported:-<nothing>}'" >&2
		rc=1
	fi
	if (( rc )); then
		echo "  The tarball's checksum can still be intact when this fails: Firefox's" >&2
		echo "  own updater rewrites this tree in place and does not touch the tarball." >&2
		echo "  Re-extract from the verified tarball (no network needed if it is on disk):" >&2
		echo "    bash shim/tests/firefox/fetch-esr.sh" >&2
	fi
	return "$rc"
}

# ---------------------------------------------------------------------------
# THE PREVENTION, and the check that it is still in place (issue #298)
# ---------------------------------------------------------------------------
#
# Detecting the drift is not enough on its own -- a red check every time the
# browser updates itself is a chore, not a pin. Two mechanisms, and which one
# is load-bearing matters:
#
#   * distribution/policies.json ASKS Firefox not to update. Unlike the
#     `app.update.*` prefs in tests/firefox/profile.user.js it is
#     INSTALLATION-scoped rather than profile-scoped, which is the gap those
#     prefs leave: they bind runs that use the acceptance profile, the tree
#     moved anyway, and therefore something reached the updater from outside
#     their reach. (Which run, this repository did not observe and does not
#     claim.) It needs no root -- Firefox reads
#     <install dir>/distribution/policies.json as well as
#     /etc/firefox/policies/policies.json -- which keeps this script's
#     "without sudo" contract.
#   * THE TREE IS LEFT NON-WRITABLE, and this is the load-bearing half. It
#     does not depend on Firefox honouring anything: the updater's write fails
#     in the kernel. It is also the only half this repository can MEASURE
#     offline -- verify_hardening() reads the mode bits, whereas "the policy
#     was obeyed" cannot be observed without the network the test profile
#     forbids. So the policy file is stated here as what Mozilla documents it
#     to do, and the permission bits are what is actually held.
#
# Both are re-applied on every non-verify run, which is how they SURVIVE A
# RE-EXTRACT: the tarball carries no distribution/ directory and unpacks with
# the owner's write bit set, so a fresh unpack is hardened immediately
# afterwards, and a tree somebody extracted by hand fails verify_hardening()
# with the command that fixes it.
policies_json() {
	# Byte-exact on purpose: verify_hardening() compares against this text
	# rather than parsing JSON, so no jq is required and a policy file that
	# was edited rather than replaced cannot pass.
	cat <<-'EOF'
		{
		  "policies": {
		    "DisableAppUpdate": true,
		    "AppAutoUpdate": false,
		    "BackgroundAppUpdate": false
		  }
		}
	EOF
}

# Any file or directory under $TREE carrying any write bit. Mode bits, not
# `test -w`: `-w` is true for everything when the caller is root, which would
# make this check pass on exactly the machine where it is least justified.
# `-type f -o -type d` because a symlink's own mode is always rwxrwxrwx and
# `chmod -R` does not follow links -- today's tarball contains none, and this
# check should not start failing on the ESR that adds one.
first_writable() {
	find "$TREE" \( -type f -o -type d \) -perm /222 -print -quit 2>/dev/null || true
}

harden_tree() {
	# u+w first: this runs on an already-hardened tree on every repeat
	# invocation, and writing policies.json into a read-only directory fails.
	chmod -R u+w "$TREE"
	mkdir -p "$POLICIES_DIR"
	policies_json >"$POLICIES"
	chmod -R a-w "$TREE"
}

# The one way an old unpack is thrown away. `rm -rf` alone is not enough once
# harden_tree() has run: unlinking a file needs the write bit on its PARENT
# DIRECTORY, and hardening removes it, so a bare `rm -rf` would make `--force`
# fail on a tree this very script made read-only.
discard_tree() {
	[[ -e "$TREE" ]] || return 0
	chmod -R u+w "$TREE" 2>/dev/null || true
	rm -rf "$TREE"
}

verify_hardening() {
	local rc=0 writable
	if [[ ! -f "$POLICIES" ]] || ! policies_json | cmp -s - "$POLICIES"; then
		echo "FAIL: $POLICIES is missing or is not the pinned policy." >&2
		echo "  without it Firefox's updater is free to check for, download and stage" >&2
		echo "  a new build from any profile that is not the acceptance profile." >&2
		rc=1
	fi
	writable="$(first_writable)"
	if [[ -n "$writable" ]]; then
		echo "FAIL: the unpacked tree is writable, so the updater can replace it in place." >&2
		echo "  first writable path: $writable" >&2
		rc=1
	fi
	if (( rc )); then
		echo "  re-apply both with: bash shim/tests/firefox/fetch-esr.sh" >&2
	fi
	return "$rc"
}

if (( verify_only )); then
	if [[ -f "$TARBALL" ]]; then
		verify "$TARBALL" || exit 1
		[[ -x "$BINARY" ]] || {
			echo "FAIL: tarball verified but nothing is unpacked at $BINARY." >&2
			echo "  run: bash shim/tests/firefox/fetch-esr.sh" >&2
			exit 1
		}
		echo "OK: sha256 $VITRIN_FIREFOX_SHA256 verified (tarball)"
	elif stamp_attests; then
		echo "OK: sha256 $VITRIN_FIREFOX_SHA256 verified (provenance stamp; tarball reclaimed)"
	else
		echo "FAIL: cannot verify the pin offline." >&2
		echo "  no tarball at $TARBALL," >&2
		echo "  and no provenance stamp attesting $BINARY." >&2
		echo "  run: bash shim/tests/firefox/fetch-esr.sh" >&2
		exit 1
	fi
	# Both branches above attest the DOWNLOAD. Neither says anything about the
	# tree that will actually be executed, so it is checked here, on every
	# path, and --verify-only never repairs what it finds: this is the
	# detector, and a detector that silently fixes its own subject reports
	# green on a machine that has been quietly re-extracting for months.
	verify_tree || exit 1
	verify_hardening || exit 1
	reported="$("$BINARY" --version 2>/dev/null || true)"
	echo "OK: unpacked tree is $APPINI_VERSION build $VITRIN_FIREFOX_BUILDID (application.ini)"
	echo "OK: updater disabled (distribution/policies.json) and $TREE non-writable"
	echo "OK: $reported"
	echo "OK: $BINARY"
	exit 0
fi

if (( force )) || [[ ! -f "$TARBALL" ]]; then
	# The one path that is allowed to reach the network, and it is never
	# reached from the acceptance script -- that calls --verify-only.
	tmp="$TARBALL.partial.$$"
	echo "fetching Firefox $VITRIN_FIREFOX_VERSION ..."
	if ! curl -fsSL --retry 3 --retry-delay 2 -o "$tmp" "$VITRIN_FIREFOX_URL"; then
		rm -f "$tmp"
		echo "FAIL: download failed: $VITRIN_FIREFOX_URL" >&2
		exit 1
	fi
	if ! verify "$tmp"; then
		rm -f "$tmp"
		exit 1
	fi
	mv "$tmp" "$TARBALL"
	discard_tree
	rm -f "$STAMP"
fi

# Verify on EVERY run, not just after a download: the whole value of a pin is
# that it keeps holding, and a tarball fetched once is a file on a developer's
# disk for months afterwards.
verify "$TARBALL" || exit 1

# THE REPAIR PATH (issue #298). A tree that no longer matches the pin is
# thrown away and re-extracted from the tarball whose bytes were just
# re-verified -- offline, since the tarball is already here. This is the
# difference between a pin that is audited and a pin that is enforced: the
# fixed state is one `fetch-esr.sh` away, and the only thing a person has to
# remember is the command they already know.
if [[ -x "$BINARY" ]]; then
	if tree_report="$(verify_tree 2>&1)"; then
		:
	else
		# Say WHAT was wrong before destroying the evidence: "re-extracted"
		# with no reason is how a machine that self-updates every week looks
		# exactly like a machine that never has.
		printf '%s\n' "$tree_report" >&2
		echo "the unpacked tree did not match the pin -- re-extracting from the verified tarball"
		discard_tree
	fi
fi

if [[ ! -x "$BINARY" ]]; then
	echo "unpacking into $DEST ..."
	tar -xJf "$TARBALL" -C "$DEST"
fi

[[ -x "$BINARY" ]] || { echo "FAIL: no firefox binary at $BINARY" >&2; exit 1; }

# Switch the updater off and take write permission away BEFORE the tree is
# blessed below, so an unpack can never leave a self-updating browser behind
# even for the length of one acceptance run.
harden_tree

# Stamp AFTER the unpack succeeded, so the stamp can only ever describe a
# browser that is really there and really came from verified bytes.
printf 'sha256=%s\nversion=%s\nbuildid=%s\nstamped=%s\n' \
	"$VITRIN_FIREFOX_SHA256" "$VITRIN_FIREFOX_VERSION" "$VITRIN_FIREFOX_BUILDID" \
	"$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
	>"$STAMP"

# The same two checks --verify-only makes, on the freshly unpacked tree: a
# tarball whose contents disagree with its name, or a hardening step that
# failed, would otherwise be invisible until the next acceptance run.
verify_tree || exit 1
verify_hardening || exit 1

# State the version the binary reports, not the version we asked for.
reported="$("$BINARY" --version 2>/dev/null || true)"
echo "OK: $reported"
echo "OK: sha256 $VITRIN_FIREFOX_SHA256 verified"
echo "OK: unpacked tree is $APPINI_VERSION build $VITRIN_FIREFOX_BUILDID (application.ini)"
echo "OK: updater disabled (distribution/policies.json) and $TREE non-writable"
echo "OK: $BINARY"
