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
BINARY="$DEST/firefox/firefox"
# PROVENANCE. Written only after a tarball has been checksum-verified AND
# unpacked, recording which hash the browser now sitting in $DEST/firefox came
# from. It is what lets `--verify-only` attest the pin once the tarball is
# gone. It lives beside the tarball rather than inside $DEST/firefox because
# `rm -rf "$DEST/firefox"` is how a re-download invalidates an old unpack --
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
	reported="$("$BINARY" --version 2>/dev/null || true)"
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
	rm -rf "$DEST/firefox"
	rm -f "$STAMP"
fi

# Verify on EVERY run, not just after a download: the whole value of a pin is
# that it keeps holding, and a tarball fetched once is a file on a developer's
# disk for months afterwards.
verify "$TARBALL" || exit 1

if [[ ! -x "$BINARY" ]]; then
	echo "unpacking into $DEST ..."
	tar -xJf "$TARBALL" -C "$DEST"
fi

[[ -x "$BINARY" ]] || { echo "FAIL: no firefox binary at $BINARY" >&2; exit 1; }

# Stamp AFTER the unpack succeeded, so the stamp can only ever describe a
# browser that is really there and really came from verified bytes.
printf 'sha256=%s\nversion=%s\nstamped=%s\n' \
	"$VITRIN_FIREFOX_SHA256" "$VITRIN_FIREFOX_VERSION" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
	>"$STAMP"

# State the version the binary reports, not the version we asked for: a
# tarball whose contents disagree with its name would otherwise be invisible.
reported="$("$BINARY" --version 2>/dev/null || true)"
echo "OK: $reported"
echo "OK: sha256 $VITRIN_FIREFOX_SHA256 verified"
echo "OK: $BINARY"
