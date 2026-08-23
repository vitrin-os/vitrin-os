#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Every hand-written inventory in this tree, checked against its source.
#
# WHY THIS EXISTS. This tree keeps writing the same list down twice and then
# updating one copy. Issue #283 wired a fourth script into `meson test` and
# updated exactly one of the two README paragraphs that state how many there
# are, so the README contradicted itself inside the very commit whose subject
# was a test that stops a fact coming undone silently. Issue #306 promoted
# `zwp_idle_inhibit_manager_v1` into the v0 global contract and updated one of
# the two acceptance scripts that state that contract, leaving
# `firefox_bringup.sh` red for anyone who actually had the browser -- which CI
# does not, so nothing said so. Neither is a proofreading failure. Both are
# lists with no owner, and both have a machine-readable source.
#
# WHAT IT CHECKS -- all of it derived, nothing typed twice:
#
#   1. The test names in README.md's `meson test -C build` comment are exactly
#      the names meson.build declares with `test('...')`.
#   2. Exactly ONE line of README.md states a count of tests/acceptance/
#      scripts wired into `meson test`. A second copy is how the last drift
#      happened, so a second copy fails here whether or not it agrees today.
#   3. That count matches the number of tests/acceptance/*.sh files meson.build
#      hands to a `test()` call.
#   4. The paragraph carrying that count links every wired script, and links no
#      acceptance script that is NOT wired -- so the sentence cannot be right
#      about how many and wrong about which.
#   5. README.md does not restate `REALM_LIB_PREFIXES` inline. That list has one
#      source (tests/acceptance/loader_independence.sh), which
#      `vitrin-realm-init`'s own tests derive from the realm's `COMPAT_NAMES`;
#      a prose copy is a fourth place to forget.
#   6. The v0 global contract is stated once, in `src/ledger.c`'s
#      `vitrin_v0_contract[]` -- the array the shim itself cross-checks
#      `globals.c` against at teardown. README.md's "Globals advertised in v0"
#      table, `shim_globals_and_client.sh`'s `expected`, and
#      `firefox_bringup.sh`'s `want_v0` must all be that array, with the
#      `--dmabuf`-only row where each of them says it belongs.
#
# This is not an acceptance test of the shim, which is why it does not live in
# tests/acceptance/ -- it builds nothing, runs nothing, and needs no shim. It
# is a `meson test` anyway, so a developer who never reads a CI log still sees
# it go red.
#
# Usage: bash shim/tests/inventories.sh [MESON_BUILD] [README]
set -Eeuo pipefail

# EVERY tool below is locale-sensitive and this check is all ranges and
# collation, so C is not tidiness. Measured on a `tr_TR.UTF-8` box while
# writing this: `grep -oE "[A-Za-z0-9_]+"` matched `xdg_conformance.sh` and
# silently skipped `loader_independence.sh`, `idle_inhibit.sh` and
# `focus_succession.sh` -- Turkish collation puts `i` outside `[a-z]`, so the
# three script names containing an `i` vanished and the count this file exists
# to defend came out as one. `sort -u` compares under the same collation.
# `tests/acceptance/loader_independence.sh` was bitten by the same locale in
# its `readelf` parsing; this is that lesson applied to the whole script.
export LC_ALL=C

# Resolved from this script's own location, never from the caller's CWD --
# `meson test` runs with the build directory as its working directory.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/.." && pwd)"
MESON_BUILD="${1:-$SHIM_DIR/meson.build}"
README="${2:-$SHIM_DIR/README.md}"
ACCEPTANCE_DIR="$SHIM_DIR/tests/acceptance"

for f in "$MESON_BUILD" "$README"; do
  [[ -r "$f" ]] || { echo "FAIL: cannot read $f" >&2; exit 1; }
done
[[ -d "$ACCEPTANCE_DIR" ]] || { echo "FAIL: no $ACCEPTANCE_DIR" >&2; exit 1; }

FAILED=0
fail() { echo "FAIL: $*" >&2; FAILED=1; }

# Meson comments run `#` to end of line, and nothing in this build file puts a
# `#` inside a string, so stripping comments first is exact here. It matters:
# without it a rationale comment quoting `test('foo', ...)` would be counted as
# a declared test.
CODE="$(sed 's/#.*//' "$MESON_BUILD")"

# --- What meson declares ---------------------------------------------------
# A `test()` call puts its name on the same line (`test('header-compiles', x)`)
# or on the next one, so newlines are folded away before matching. `test` must
# not be preceded by an identifier character, or `header_test = ...` matches.
MESON_TESTS="$(printf '%s' "$CODE" | tr '\n' ' ' \
  | grep -oE "(^|[^A-Za-z0-9_])test\([[:space:]]*'[^']+'" \
  | grep -oE "'[^']+'" | tr -d "'" | sort -u || true)"

WIRED_SCRIPTS="$(printf '%s' "$CODE" \
  | grep -oE "files\('tests/acceptance/[A-Za-z0-9_]+\.sh'\)" \
  | grep -oE "[A-Za-z0-9_]+\.sh" | sort -u || true)"

# Vacuity guards. Every comparison below passes trivially against an empty set,
# which is precisely how a check stops checking after a syntax change in the
# file it parses.
[[ -n "$MESON_TESTS" ]] || fail "parsed no test() declarations out of $MESON_BUILD, so checks 1-4 would compare two empty sets. What changed is the parser here, not the README."
[[ -n "$WIRED_SCRIPTS" ]] || fail "parsed no tests/acceptance/*.sh arguments out of $MESON_BUILD, so the script count below is 0 and agrees with nothing. What changed is the parser here, not the README."

# --- 1. The test-name list in the README's build recipe --------------------
# The recipe reads `meson test -C build   # a, b,` and continues on
# comment-only lines; the block ends at the first line carrying no `#`.
README_TESTS="$(awk '
  index($0, "meson test -C build") { inblk = 1 }
  inblk {
    hash = index($0, "#")
    if (hash == 0) { inblk = 0; next }
    print substr($0, hash + 1)
  }
' "$README" | tr ',' '\n' | tr -d '[:blank:]' | grep -v '^$' | sort -u || true)"

if [[ "$README_TESTS" != "$MESON_TESTS" ]]; then
  fail "README.md's \`meson test -C build\` comment does not list the tests meson.build declares:"
  diff --label 'README.md says' --label 'meson.build declares' -u \
    <(printf '%s\n' "$README_TESTS") <(printf '%s\n' "$MESON_TESTS") >&2 || true
fi

# --- 2 + 3. The one stated count -------------------------------------------
# A number word immediately followed by `scripts`, on a line that also says
# "wired into `meson test`". "one of the ... scripts" deliberately does not
# match: prose may say a script is wired in as often as it likes; it may say
# HOW MANY are in exactly one place.
COUNT_HITS="$(grep -nE "wired into \`meson test\`" "$README" \
  | grep -E "\b(one|two|three|four|five|six|seven|eight|nine|ten) scripts\b" || true)"
HIT_COUNT=0
[[ -z "$COUNT_HITS" ]] || HIT_COUNT="$(printf '%s\n' "$COUNT_HITS" | wc -l)"

EXPECTED_N="$(printf '%s\n' "$WIRED_SCRIPTS" | grep -c . || true)"
WORDS=(zero one two three four five six seven eight nine ten)
EXPECTED_WORD="${WORDS[$EXPECTED_N]:-$EXPECTED_N}"

if [[ "$HIT_COUNT" -ne 1 ]]; then
  fail "README.md states how many tests/acceptance/ scripts are wired into \`meson test\` on $HIT_COUNT lines; it must state it on exactly one. Two copies of a count is how #283 shipped a README that contradicted itself. Lines found:"
  printf '%s\n' "$COUNT_HITS" >&2
else
  COUNT_LINE_NO="${COUNT_HITS%%:*}"
  STATED_WORD="$(printf '%s' "$COUNT_HITS" \
    | grep -oE "\b(one|two|three|four|five|six|seven|eight|nine|ten) scripts\b" \
    | head -n1 | cut -d' ' -f1)"
  if [[ "$STATED_WORD" != "$EXPECTED_WORD" ]]; then
    fail "README.md:$COUNT_LINE_NO says \"$STATED_WORD scripts\" are wired into \`meson test\`; meson.build hands $EXPECTED_N ($EXPECTED_WORD) tests/acceptance/ scripts to a test() call: $(printf '%s ' $WIRED_SCRIPTS)"
  fi

  # --- 4. And the paragraph around it names exactly those scripts ----------
  # Paragraph = the blank-line-delimited block the count sits in, which is
  # where the list of scripts is. A list one paragraph away from its own count
  # is not the shape being checked.
  PARA="$(awk -v n="$COUNT_LINE_NO" '
    NF == 0 { if (NR > n) { exit } ; start = NR + 1; buf = ""; next }
    { buf = buf $0 "\n" }
    END { printf "%s", buf }
  ' "$README")"
  if [[ -z "$PARA" ]]; then
    fail "could not isolate the paragraph containing README.md:$COUNT_LINE_NO, so check 4 inspected nothing."
  fi
  for script in $WIRED_SCRIPTS; do
    if ! grep -qF "tests/acceptance/$script" <<<"$PARA"; then
      fail "meson.build wires tests/acceptance/$script into \`meson test\`, and the paragraph at README.md:$COUNT_LINE_NO that counts those scripts does not link it."
    fi
  done
  for path in "$ACCEPTANCE_DIR"/*.sh; do
    script="$(basename "$path")"
    if grep -qxF "$script" <<<"$WIRED_SCRIPTS"; then
      continue
    fi
    if grep -qF "tests/acceptance/$script" <<<"$PARA"; then
      fail "the paragraph at README.md:$COUNT_LINE_NO counts the scripts wired into \`meson test\` and links tests/acceptance/$script, which meson.build does not hand to a test() call -- it is a separate CI step."
    fi
  done
fi

# --- 5. The realm's library prefixes are named once, in the script ---------
LOADER="$ACCEPTANCE_DIR/loader_independence.sh"
if [[ ! -r "$LOADER" ]]; then
  fail "missing $LOADER, the single source of REALM_LIB_PREFIXES this check anchors the README to."
else
  PREFIX_LINE="$(grep -E '^REALM_LIB_PREFIXES=\(' "$LOADER" || true)"
  if [[ -z "$PREFIX_LINE" ]]; then
    fail "no REALM_LIB_PREFIXES=( ... ) assignment in $LOADER, so check 5 has nothing to anchor to and would pass on any README."
  fi
  CHECKED=0
  for prefix in $(printf '%s' "$PREFIX_LINE" | cut -d= -f2- | tr -d '()' | tr -d '/'); do
    # `/usr` and `/lib` are skipped: both occur in ordinary prose here
    # ("library", "/usr/bin"), so grepping for them would report a
    # restatement that is not one. Any faithful copy of this list carries the
    # narrow entries too, and a copy that drops them is already wrong.
    case "$prefix" in usr | lib) continue ;; esac
    CHECKED=$((CHECKED + 1))
    if grep -qF "/$prefix" "$README"; then
      fail "README.md names \`/$prefix\`, an entry of REALM_LIB_PREFIXES. That list lives in tests/acceptance/loader_independence.sh and is derived from vitrin-realm-init's COMPAT_NAMES by a Rust-side test; a prose copy is a fourth place to forget it. Point at the script instead."
    fi
  done
  if [[ "$CHECKED" -eq 0 ]]; then
    fail "every entry of REALM_LIB_PREFIXES was skipped as too generic to grep for, so check 5 inspected nothing. Re-read the skip list above."
  fi
fi

# --- 6. The v0 global contract, stated once in src/ledger.c ----------------
# `vitrin_v0_contract[]` is not a convenient list to parse, it is the ONE the
# shim itself holds `globals.c` to at teardown (`globals-contract-drift`). The
# array body is one quoted name per line and its comments carry no quoted
# strings, so a line-shaped match over it is exact.
LEDGER="$SHIM_DIR/src/ledger.c"
FF="$ACCEPTANCE_DIR/firefox_bringup.sh"
SG="$ACCEPTANCE_DIR/shim_globals_and_client.sh"
for f in "$LEDGER" "$FF" "$SG"; do
  [[ -r "$f" ]] || fail "missing $f, one of the four places check 6 compares. It cannot be skipped: with a file gone this check would compare what is left and call it agreement."
done
if [[ -r "$LEDGER" && -r "$FF" && -r "$SG" ]]; then
  BODY="$(awk '/vitrin_v0_contract\[\] = \{/ {inside=1; next} inside && /^\};/ {exit} inside' "$LEDGER")"
  ALL="$(printf '%s\n' "$BODY" | sed -n 's/^[[:space:]]*"\([a-z0-9_]*\)".*/\1/p' | sort -u)"
  # The optional rows mark themselves in the array, in the source of truth, so
  # even the partition is read rather than restated here.
  OPT="$(printf '%s\n' "$BODY" | grep -F '/* only with' | sed -n 's/^[[:space:]]*"\([a-z0-9_]*\)".*/\1/p' | sort -u || true)"
  CORE="$(comm -23 <(printf '%s\n' "$ALL") <(printf '%s\n' "$OPT" | grep -v '^$' || true))"

  [[ -n "$ALL" ]] || fail "parsed no interface names out of $LEDGER's vitrin_v0_contract[], so check 6 would compare every list against an empty one and pass. The parser here is what changed."
  [[ -n "$CORE" ]] || fail "every row of vitrin_v0_contract[] parsed as optional, which leaves nothing for the two acceptance scripts to assert. Read the '/* only with' markers in $LEDGER."

  FF_WANT_LINES="$(grep -cE '^want_v0="' "$FF" || true)"
  if [[ "$FF_WANT_LINES" -ne 1 ]]; then
    fail "$FF has $FF_WANT_LINES \`want_v0=\"...\"\` assignments; check 6 reads exactly one."
  else
    FF_WANT="$(grep -E '^want_v0="' "$FF" | sed 's/^want_v0="//; s/"[[:space:]]*$//' | tr ' ' '\n' | grep -v '^$' | sort -u)"
    if [[ "$FF_WANT" != "$CORE" ]]; then
      fail "$FF's \`want_v0\` is not src/ledger.c's vitrin_v0_contract[] minus its \`--dmabuf\`-only row. This is how #306 shipped: the global went into the contract and into shim_globals_and_client.sh, and this list kept the old set -- red for anyone with the browser, and CI declares this gate skipped, so nothing said so."
      diff --label "want_v0 says" --label "ledger.c requires" -u \
        <(printf '%s\n' "$FF_WANT") <(printf '%s\n' "$CORE") >&2 || true
    fi
  fi

  SG_BASE_LINES="$(grep -cE '^expected=\(' "$SG" || true)"
  if [[ "$SG_BASE_LINES" -ne 1 ]]; then
    fail "$SG has $SG_BASE_LINES \`expected=( ... )\` assignments; check 6 reads exactly one."
  else
    SG_BASE="$(grep -E '^expected=\(' "$SG" | sed 's/^expected=(//; s/).*$//' | tr ' ' '\n' | grep -v '^$' | sort -u)"
    if [[ "$SG_BASE" != "$CORE" ]]; then
      fail "$SG's \`expected\` is not src/ledger.c's vitrin_v0_contract[] minus its \`--dmabuf\`-only row:"
      diff --label "expected says" --label "ledger.c requires" -u \
        <(printf '%s\n' "$SG_BASE") <(printf '%s\n' "$CORE") >&2 || true
    fi
    SG_OPT="$(grep -E '^[[:space:]]*expected\+=\(' "$SG" | sed 's/^[[:space:]]*expected+=(//; s/).*$//' | tr ' ' '\n' | grep -v '^$' | sort -u || true)"
    if [[ "$SG_OPT" != "$OPT" ]]; then
      fail "$SG appends '$SG_OPT' under its --dmabuf branch; src/ledger.c marks '$OPT' as the row that is only advertised with --dmabuf."
    fi
  fi

  # The README table. Scoped to the one that introduces itself, so the other
  # tables on the page cannot contribute a row.
  RM_GLOBALS="$(awk '
    index($0, "Globals advertised in v0") { seen = 1; next }
    seen && substr($0, 1, 1) == "|" { intab = 1 }
    intab && substr($0, 1, 1) != "|" { exit }
    intab { print }
  ' "$README" | sed -n 's/^|[[:space:]]*`\([a-z0-9_]*\)`.*/\1/p' | sort -u)"
  if [[ -z "$RM_GLOBALS" ]]; then
    fail "found no \"Globals advertised in v0\" table in $README, so check 6 read no README rows at all and would agree with anything."
  elif [[ "$RM_GLOBALS" != "$ALL" ]]; then
    fail "$README's \"Globals advertised in v0\" table is not src/ledger.c's vitrin_v0_contract[]:"
    diff --label "README.md says" --label "ledger.c requires" -u \
      <(printf '%s\n' "$RM_GLOBALS") <(printf '%s\n' "$ALL") >&2 || true
  fi
fi

if [[ $FAILED -ne 0 ]]; then
  exit 1
fi

echo "PASS: README.md lists the $(printf '%s\n' "$MESON_TESTS" | grep -c .) tests meson.build declares"
echo "      ($(printf '%s ' $MESON_TESTS)),"
echo "      states the tests/acceptance/ script count once ($EXPECTED_WORD), links exactly those"
echo "      ($(printf '%s ' $WIRED_SCRIPTS)),"
echo "      and restates no REALM_LIB_PREFIXES entry. The v0 global contract is"
echo "      src/ledger.c's $(printf '%s\n' "$ALL" | grep -c .) rows, and the README table, shim_globals_and_client.sh"
echo "      and firefox_bringup.sh all say exactly that (optional: $(printf '%s ' $OPT))."
