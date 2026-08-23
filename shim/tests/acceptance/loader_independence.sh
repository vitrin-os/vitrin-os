#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# The shim must be loadable from a directory that holds nothing but the shim
# (issue #283).
#
# WHY THIS EXISTS. A confined realm (`vitrind --isolation=default`) binds the
# shim as a SINGLE FILE at a fixed in-realm path -- see `IN_REALM_SHIM` in
# crates/vitrin-realm-init/src/lib.rs -- and nothing else from the build tree
# comes with it. Meson gives an executable that links a VENDORED SHARED
# library the RUNPATH `$ORIGIN/subprojects/<name>`, and `$ORIGIN` is the
# directory the binary is executed from, so inside the realm it resolves to a
# path the mount table never creates. The dynamic loader then kills the shim
# BEFORE `main`, and the core, which is watching a config channel rather than
# a console, reports nothing more useful than `Broken pipe`.
#
# That was the shipped state until #283: CI always builds the vendored wlroots
# (Ubuntu ships 0.17.1, D11 pins 0.19), so `--isolation=default` -- the
# default -- could not start a realm with the shim CI builds. The fix is
# `default_library=static` in meson.build's `project()` defaults; this script
# is what stops it from silently coming undone.
#
# FOUR CHECKS, AND NONE OF THEM IS REDUNDANT:
#
#   1. The shim, COPIED to an empty directory and run with a scrubbed
#      environment, gets into `main`. This is the property itself, measured
#      the way the realm measures it: `$ORIGIN` moves, `LD_LIBRARY_PATH` is
#      gone, and the loader either finds everything or the process dies at
#      127 having printed `error while loading shared libraries`.
#   2. The binary carries no DT_RPATH and no DT_RUNPATH at all. Check 1 alone
#      does NOT cover this: an ABSOLUTE RUNPATH into the build tree resolves
#      perfectly well on the host, so check 1 would pass while a realm --
#      which cannot see the build tree -- still failed.
#   2b. The PT_INTERP program interpreter is under one of those prefixes too.
#      `ldd` prints it WITHOUT a `=>`, so check 3's loop never sees it, and
#      it is the one path the kernel resolves before the process exists at
#      all. Check 1 would only catch a non-stock interpreter on a machine
#      that happens not to have it.
#   3. Every library the copy actually resolves lives under one of
#      REALM_LIB_PREFIXES below -- the library-bearing prefixes a realm's
#      stock mount table mirrors (crates/vitrin-realm-init/src/main.rs,
#      K3/K4); anything else is reachable on this machine and not in a realm.
#
# A missing `readelf` or `ldd` is a FAILURE here, not a skip. A check that
# quietly skips itself when its instrument is absent is a check that has
# stopped checking, and this repo has found that pattern often enough to
# refuse it on sight.
#
# Usage: bash shim/tests/acceptance/loader_independence.sh [SHIM_BIN]
#        BUILD_DIR=/path/to/build bash shim/tests/acceptance/loader_independence.sh
set -Eeuo pipefail

# Resolved from this script's own location, never from the caller's CWD --
# `meson test` runs with the build directory as its working directory.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/../.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$SHIM_DIR/build}"
SHIM_BIN="${1:-${SHIM_BIN:-$BUILD_DIR/vitrin-shim}}"

[[ -x "$SHIM_BIN" ]] || { echo "FAIL: missing $SHIM_BIN (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
SHIM_BIN="$(cd "$(dirname "$SHIM_BIN")" && pwd)/$(basename "$SHIM_BIN")"

# The prefixes a confined realm's mount table carries that a LIBRARY can come
# out of: `/usr` is bound at its own path, and `lib`, `lib64`, `lib32`,
# `libx32` are the entries of `COMPAT_NAMES` in
# crates/vitrin-realm-init/src/main.rs that name library directories --
# mirrored beside `/usr` as whatever they are on the host, symlink or real
# directory.
#
# `COMPAT_NAMES` also carries `bin` and `sbin`, and the realm mounts `/etc`.
# They are DELIBERATELY absent here, so this array is a strict subset of what
# a realm can reach rather than a copy of it: a shared library resolved out of
# `/bin`, `/sbin` or `/etc` would load inside a realm perfectly well and is
# still something this project wants to hear about. That is why the failure
# message below says "not under a prefix a realm mirrors for libraries" and
# not "outside the realm's mount table" -- the second would be false.
#
# This array is the ONLY place the list is written down, and it is HELD IN
# STEP rather than asked to be: the Rust side owns the mount table, so the
# check lives there, where both files are certain to exist and this tree stays
# toolchain-free.
# `crates/vitrin-realm-init/src/main.rs`'s
# `the_shims_realm_lib_prefixes_are_the_library_bearing_compat_names` reads
# this line and requires it to equal `/usr` plus every `COMPAT_NAMES` entry
# beginning `lib` -- so a Rust-side change that adds or drops a library mirror
# goes red HERE instead of leaving this script quietly describing a realm that
# no longer exists. shim/README.md and D-043 point at this line for the
# current list rather than restating it, and `meson test inventories`
# fails if either starts restating it again.
REALM_LIB_PREFIXES=(/usr/ /lib/ /lib64/ /lib32/ /libx32/)

WORK="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-loader.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

FAILED=0
fail() { echo "FAIL: $*" >&2; FAILED=1; }

# Set by check 2b; declared here so `set -u` cannot turn a missing readelf
# (already a FAIL) into an unbound-variable error further down.
INTERP=""

# Is $1 under one of the prefixes above? Used by BOTH the PT_INTERP check and
# the ldd loop, so the two cannot drift into disagreeing about what a realm
# can reach.
prefix_ok() {
  local path="$1" prefix
  for prefix in "${REALM_LIB_PREFIXES[@]}"; do
    [[ "$path" == "$prefix"* ]] && return 0
  done
  return 1
}

# --- 1. Run it from somewhere that is not the build tree ------------------
# A COPY, not a symlink: the loader expands `$ORIGIN` from the resolved path,
# so a symlink would still point at the build tree and the check would pass
# for the wrong reason.
#
# `--not-a-flag` takes the shim's own usage branch, which exits 2 after
# `wlr_log_init` has already called into wlroots -- so a clean exit 2 proves
# both that every NEEDED library was found and that a wlroots symbol was
# reached. It needs no socket, no backend, no `$XDG_RUNTIME_DIR` and no GPU,
# which is why this test can be a plain `meson test` rather than a bring-up.
LONE_DIR="$WORK/lone"
mkdir -p "$LONE_DIR"
cp "$SHIM_BIN" "$LONE_DIR/vitrin-shim"

set +e
RUN_OUT="$(env -i "$LONE_DIR/vitrin-shim" --not-a-flag 2>&1)"
RUN_STATUS=$?
set -e

if [[ $RUN_STATUS -ne 2 ]]; then
  fail "the shim copied to an empty directory and run with an empty environment exited $RUN_STATUS, expected 2 (its usage branch)."
  echo "       output: $RUN_OUT" >&2
  if [[ "$RUN_OUT" == *"error while loading shared libraries"* ]]; then
    echo "       This is issue #283 exactly: the dynamic loader could not resolve a" >&2
    echo "       library because \$ORIGIN moved. A realm binds the shim as a lone file," >&2
    echo "       so it will die the same way, before main, and the core will report only" >&2
    echo "       'Broken pipe'. Fix the LINK (meson.build's default_library=static), not" >&2
    echo "       the mount table -- see docs/plan/20-decision-log.md D-043." >&2
  fi
elif [[ "$RUN_OUT" != *"usage:"* ]]; then
  fail "the shim exited 2 from an empty directory but printed no usage line, so this check is no longer observing the branch it was written for: $RUN_OUT"
fi

# --- 2. No RPATH, no RUNPATH ----------------------------------------------
if ! command -v readelf >/dev/null 2>&1; then
  fail "readelf not found. This check is not skippable: without it an absolute RUNPATH into the build tree passes check 1 and still breaks every confined realm. Install binutils."
else
  # LC_ALL=C or the tag names are translated and the grep below silently
  # matches nothing -- which is how this check would stop checking on a
  # non-English machine (observed: a Turkish locale renders the column
  # headers, and a looser pattern would have found no rows at all).
  DYN="$(LC_ALL=C readelf -d "$LONE_DIR/vitrin-shim")"
  if ! grep -qE '\(NEEDED\)' <<<"$DYN"; then
    fail "readelf reported no NEEDED entries for the shim at all, so this check has no dynamic section to inspect and would pass on any input. Output was: $DYN"
  fi
  if RPATHS="$(grep -E '\((RPATH|RUNPATH)\)' <<<"$DYN")"; then
    fail "the shim carries a run-time library path, so where it is executed from decides whether it loads:"
    echo "$RPATHS" >&2
    echo "       A realm's mount table does not create the build tree, whether the path" >&2
    echo "       is \$ORIGIN-relative or absolute. Link the vendored library IN." >&2
  fi

  # 2b. The PT_INTERP loader itself. Check 3 below reads `ldd`'s `=>` lines,
  # and the program interpreter is printed WITHOUT one -- so without this the
  # one path the kernel resolves before any of the others is the one path
  # never prefix-checked. Check 1 would catch a non-stock interpreter by
  # failing to run at all, but only on a machine that happens to lack it;
  # this states the requirement instead of relying on that.
  INTERP="$(LC_ALL=C readelf -l "$LONE_DIR/vitrin-shim" \
    | sed -n 's/.*\[Requesting program interpreter: \(.*\)\]/\1/p')"
  if [[ -z "$INTERP" ]]; then
    fail "readelf found no PT_INTERP segment in the shim. A dynamically linked executable has one; without it this check inspected nothing."
  elif ! prefix_ok "$INTERP"; then
    fail "the shim's program interpreter is $INTERP, which is not under a prefix a realm mirrors for libraries (${REALM_LIB_PREFIXES[*]}). The kernel resolves PT_INTERP before the process exists, so a realm would refuse to start it at all."
  fi
fi

# --- 3. Everything it loads is in the realm's stock mount table ------------
if ! command -v ldd >/dev/null 2>&1; then
  fail "ldd not found. Not skippable, for the same reason readelf is not: it is the only check here that names WHICH library escapes the realm's mount table."
else
  set +e
  LDD_OUT="$(LC_ALL=C ldd "$LONE_DIR/vitrin-shim" 2>&1)"
  LDD_STATUS=$?
  set -e
  if [[ $LDD_STATUS -ne 0 ]]; then
    fail "ldd on the copied shim exited $LDD_STATUS: $LDD_OUT"
  fi
  # Guard against a report this loop would find nothing wrong in because it
  # found nothing at all.
  if ! grep -q '=>' <<<"$LDD_OUT"; then
    fail "ldd listed no resolved libraries, so the prefix check below inspected nothing. Output was: $LDD_OUT"
  fi
  while IFS= read -r line; do
    if [[ "$line" == *"not found"* ]]; then
      fail "the copied shim cannot resolve a library: ${line# }"
      continue
    fi
    [[ "$line" == *"=> /"* ]] || continue
    path="${line#*=> }"
    path="${path%% (*}"
    path="${path%"${path##*[![:space:]]}"}"
    if ! prefix_ok "$path"; then
      fail "the shim loads $path, which is not under a prefix a realm mirrors for libraries (${REALM_LIB_PREFIXES[*]}). A realm would have to grow a bind mount for it, making the confinement boundary a function of this build's flags."
    fi
  done <<<"$LDD_OUT"
fi

if [[ $FAILED -ne 0 ]]; then
  exit 1
fi

echo "PASS: the shim runs from an empty directory with an empty environment,"
echo "      carries no RPATH/RUNPATH, and resolves its interpreter ($INTERP)"
echo "      and every library it loads from ${REALM_LIB_PREFIXES[*]}."
