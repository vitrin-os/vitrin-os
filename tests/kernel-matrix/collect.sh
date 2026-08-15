#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Collect the per-kernel isolation matrix (issue #281): boot each kernel named
# in kernels.manifest under QEMU with an initramfs holding the SHIPPED
# `vitrind`, and write -- or verify -- one checked-in row per kernel.
#
# `--check` is the posture `cargo xtask isolation-matrix --check` and
# `cargo xtask session-matrix --check` already have, one level lower down: it
# re-measures and holds the checked-in rows to what the machines answer now,
# rather than to what somebody typed.
#
#   tests/kernel-matrix/collect.sh                 re-measure and rewrite rows/
#   tests/kernel-matrix/collect.sh --check         re-measure and diff; writes nothing
#   tests/kernel-matrix/collect.sh --only ID       one row (PARTIAL: says so, loudly)
#
# Environment:
#   VITRIND      binary to put in the image  (default target/release/vitrind)
#   QEMU         qemu binary                 (default qemu-system-x86_64)
#   QEMU_EXTRA   extra qemu args, split on whitespace (e.g. -L /path/share/qemu)
#   ACCEL        -accel value                (default tcg. Every checked-in row
#                was taken under tcg -- no kvm run has been made here, so this
#                script does not claim the two agree. The accelerator is
#                recorded in each row's `#~ accel:` line, and `--check` ignores
#                that line and compares everything else, so a kvm run that DID
#                differ would be reported rather than absorbed.)
#   CACHE        download/extract cache      (default target/kernel-matrix)
#   MAX_ROW_AGE_DAYS  age at which --check calls a row stale (default 180)
#
# ---------------------------------------------------------------------------
# WHAT THIS SCRIPT IS WRITTEN AGAINST
# ---------------------------------------------------------------------------
#
# A VM that panics, hangs, or never reaches userspace must not look like a
# measurement. Every check below is there because its absence would let some
# failure render as an empty cell, a default, or an "unmeasured" that reads
# like an answer:
#
#   1. the tools must exist                 -- a missing qemu must not skip
#   2. the .deb must match its pinned sha   -- a mirror serving something else
#                                              is not this kernel
#   3. the vmlinuz must match its pinned sha
#   4. the guest must reach our init        -- VITRIN-INIT-BEGIN
#   5. the guest must not have died mid-way -- VITRIN-INIT-DONE, no VITRIN-FATAL
#   6. the probe must have exited 0         -- a missing /vitrind exits 127
#   7. the row must be schema-tagged and complete
#   8. kernel.release must equal the PINNED release -- booting the wrong image
#      is the single most plausible way to publish a row about a kernel nobody
#      ran
#   9. exactly one startup verdict must be present -- neither zero nor both
#  10. every manifest record must produce a row, and every row file must have a
#      manifest record (full runs only)
#  11. a run that processed no records at all is a failure, not a pass
#
# Anything that trips these prints `FAIL:` and exits nonzero. There is no
# branch that prints a row it did not measure.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

MANIFEST="$here/kernels.manifest"
ROWS="$here/rows"
VITRIND="${VITRIND:-$root/target/release/vitrind}"
QEMU="${QEMU:-qemu-system-x86_64}"
ACCEL="${ACCEL:-tcg}"
CACHE="${CACHE:-$root/target/kernel-matrix}"
MAX_ROW_AGE_DAYS="${MAX_ROW_AGE_DAYS:-180}"
read -r -a QEMU_EXTRA_ARGS <<<"${QEMU_EXTRA:-}"

# The kernel command line every row shares. `panic=1` plus qemu's `-no-reboot`
# turns a panic into an exited qemu with no VITRIN-INIT-DONE, which check 5
# reports as a dead boot instead of letting it hang until the timeout.
APPEND_BASE="console=ttyS0 panic=1 quiet"
MEMORY_MIB=512
VCPUS=1
BOOT_TIMEOUT_SECONDS=300

check=0
only=()
while [ $# -gt 0 ]; do
  case "$1" in
    --check) check=1 ;;
    --only) shift; [ $# -gt 0 ] || { echo "FAIL: --only needs an id" >&2; exit 2; }; only+=("$1") ;;
    --max-age-days) shift; MAX_ROW_AGE_DAYS="$1" ;;
    # The header block down to the end of the environment table. The range is
    # bounded by a pattern rather than a line number so an edit above cannot
    # silently truncate the help or spill the next section into it.
    -h|--help) sed -n '3,/^# ----*$/p' "${BASH_SOURCE[0]}" | sed '$d'; exit 0 ;;
    *) echo "FAIL: unknown argument '$1'" >&2; exit 2 ;;
  esac
  shift
done

fail() { echo "FAIL: $*" >&2; exit 1; }
note() { echo "kernel-matrix: $*" >&2; }

# ---------------------------------------------------------------------------
# 1. Tools. A missing tool is a failure, never a skip: a collector that quietly
#    did nothing is the exact shape issue #288 exists to forbid.
# ---------------------------------------------------------------------------
missing=()
for tool in "$QEMU" gcc cpio gzip curl sha256sum ar tar zstd xz diff; do
  command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
[ ${#missing[@]} -eq 0 ] || fail "these tools are not on PATH: ${missing[*]}. This script measures kernels; without them it can measure nothing, and it will not pretend otherwise."
[ -x "$VITRIND" ] || fail "\$VITRIND ($VITRIND) is not an executable. Build it first: cargo build --release --bin vitrind"

mkdir -p "$CACHE/deb" "$CACHE/vmlinuz" "$CACHE/logs" "$ROWS"

# ---------------------------------------------------------------------------
# 2. The initramfs, built once from the binary under test.
# ---------------------------------------------------------------------------
build_initramfs() {
  local img="$CACHE/initramfs.cpio.gz"
  local dir="$CACHE/image"
  rm -rf "$dir"
  mkdir -p "$dir"
  gcc -static -Os -Wall -Wextra -o "$dir/init" "$here/init.c" \
    || fail "compiling init.c (a static PID 1 is what makes the image independent of any distribution userspace)"
  cp "$VITRIND" "$dir/vitrind"
  # vitrind's own library closure, at the paths it names. Both `ldd` shapes are
  # handled: `name => /path` for a needed library, and a bare absolute path for
  # the ELF interpreter on some distributions.
  local line lhs rhs
  while read -r line; do
    lhs="${line%% *}"
    if [[ "$line" == *"=>"* ]]; then
      rhs="$(sed -E 's/.*=> ([^ ]+).*/\1/' <<<"$line")"
      [ -e "$rhs" ] || continue
      mkdir -p "$dir/$(dirname "$rhs")"
      cp -L "$rhs" "$dir/$rhs"
      # The interpreter is named by its LEFT-hand path inside the ELF header,
      # which on a /lib64 -> /usr/lib64 distribution is not where ldd resolved
      # it. Place it at both, or the guest cannot exec vitrind at all.
      if [[ "$lhs" == /* ]]; then
        mkdir -p "$dir/$(dirname "$lhs")"
        cp -L "$rhs" "$dir/$lhs"
      fi
    elif [[ "$lhs" == /* && -e "$lhs" ]]; then
      mkdir -p "$dir/$(dirname "$lhs")"
      cp -L "$lhs" "$dir/$lhs"
    fi
  done < <(ldd "$VITRIND")
  ( cd "$dir" && find . -print0 | cpio --null --create --format=newc --quiet ) | gzip -9 >"$img" \
    || fail "packing the initramfs"
  echo "$img"
}

# ---------------------------------------------------------------------------
# 3. Kernels: fetch by URL, but pin by sha256. The checksum is the identity;
#    the URL is only where these bytes were found on the collection date.
# ---------------------------------------------------------------------------
fetch_deb() {
  local url="$1" want="$2" out="$CACHE/deb/$2.deb" got
  if [ ! -f "$out" ]; then
    note "downloading $url"
    curl --fail --location --silent --show-error --retry 3 --retry-delay 2 \
      --output "$out.part" "$url" \
      || fail "downloading $url. Distribution pools prune superseded kernels, so this URL dying is expected eventually -- the remedy is another mirror serving sha256 $want, NOT a re-measurement against whatever the pool holds now."
    mv "$out.part" "$out"
  fi
  got="$(sha256sum "$out" | cut -d' ' -f1)"
  [ "$got" = "$want" ] || fail "$url has sha256 $got, and kernels.manifest pins $want. These are not the same bytes; refusing to measure them and call it the pinned kernel."
  echo "$out"
}

extract_vmlinuz() {
  local deb="$1" member="$2" want="$3" out="$CACHE/vmlinuz/$3" tmp got
  if [ ! -f "$out" ]; then
    tmp="$(mktemp -d "$CACHE/extract.XXXXXX")"
    ( cd "$tmp" && ar x "$deb" ) || fail "ar x $deb"
    local data
    data="$(ls "$tmp"/data.tar* 2>/dev/null | head -n1)" || true
    [ -n "$data" ] || fail "$deb holds no data.tar* member"
    case "$data" in
      *.zst) zstd -dc "$data" | tar -x -C "$tmp" "$member" ;;
      *.xz)  xz -dc "$data"   | tar -x -C "$tmp" "$member" ;;
      *.gz)  gzip -dc "$data" | tar -x -C "$tmp" "$member" ;;
      *)     tar -xf "$data" -C "$tmp" "$member" ;;
    esac || fail "extracting $member from $data"
    [ -f "$tmp/$member" ] || fail "$member is not in $deb"
    mv "$tmp/$member" "$out"
    rm -rf "$tmp"
  fi
  got="$(sha256sum "$out" | cut -d' ' -f1)"
  [ "$got" = "$want" ] || fail "the extracted $member has sha256 $got, and kernels.manifest pins $want"
  echo "$out"
}

# ---------------------------------------------------------------------------
# 4. One boot, and every way it can lie about having measured something.
# ---------------------------------------------------------------------------

# Strip the kernel's own printk lines and any carriage returns, so a row is
# `vitrind`'s output and not the console's.
#
# ORDER IS LOAD-BEARING, and it is the one bug this collector shipped before it
# ever produced a row: `-nographic` puts the guest's console on a serial port,
# so EVERY line in the log ends `\r\n`. An anchored pattern like
# `/^VITRIN-PHASE-END floor$/` matched against the raw log therefore matches
# nothing -- the line ends `floor\r`. Decluttering must happen BEFORE any
# anchored match, never after it, which is why this is a filter every reader
# below pipes through first rather than a step at the end.
#
# The escape-sequence strip is the same hazard once removed: the kernel resets
# the console with `ESC c ESC [?7l ESC [2J` on its way up, and a terminal reset
# that lands mid-line would otherwise ride into a checked-in row. `init.c`
# breaks the line; this makes sure nothing survives even if it does not.
declutter() {
  tr -d '\r' \
    | sed -E $'s/\033c//g; s/\033\\[[0-9;?]*[a-zA-Z]//g' \
    | grep -v -E '^\[[ ]*[0-9]+\.[0-9]+\]' || true
}

phase_body() {
  local log="$1" name="$2"
  declutter <"$log" \
    | sed -n "/^VITRIN-PHASE-BEGIN $name\$/,/^VITRIN-PHASE-END $name /p" \
    | sed '1d;$d'
}

phase_status() {
  local log="$1" name="$2"
  declutter <"$log" | grep -m1 -E "^VITRIN-PHASE-END $name " \
    | sed -E "s/^VITRIN-PHASE-END $name //"
}

boot() {
  local vmlinuz="$1" initramfs="$2" append_extra="$3" log="$4"
  local append="$APPEND_BASE"
  [ "$append_extra" = "-" ] || append="$append $append_extra"
  set +e
  # `</dev/null` is not tidiness. `-nographic` muxes the guest's serial port
  # onto this process's stdin as well as its stdout, and this function is called
  # from a loop over the manifest -- so without it qemu EATS the manifest,
  # types it at the guest's console, and every record after the first silently
  # never runs. That is precisely the "measured nothing, exited 0" shape the
  # checks in this file exist to forbid, arriving through the one channel none
  # of them watches.
  timeout "$BOOT_TIMEOUT_SECONDS" "$QEMU" "${QEMU_EXTRA_ARGS[@]}" \
    -accel "$ACCEL" -m "$MEMORY_MIB" -smp "$VCPUS" -nographic -no-reboot \
    -kernel "$vmlinuz" -initrd "$initramfs" -append "$append" \
    </dev/null >"$log" 2>&1
  local rc=$?
  set -e
  # A qemu that could not start at all leaves a log with no sentinels; the
  # checks below name that case, but the exit status is worth reporting first
  # because "qemu is not usable here" and "the guest died" are different repairs.
  if [ $rc -eq 124 ]; then
    fail "the guest did not power off within ${BOOT_TIMEOUT_SECONDS}s -- last 20 lines:
$(tail -n 20 "$log")"
  fi
}

# ---------------------------------------------------------------------------
# 5. Render one row from one boot log.
# ---------------------------------------------------------------------------
render_row() {
  local id="$1" class="$2" release="$3" url="$4" deb_sha="$5" member="$6" vmlinuz_sha="$7" append_extra="$8" log="$9"

  grep -q '^VITRIN-INIT-BEGIN' <(declutter <"$log") \
    || fail "[$id] the guest never reached our init -- no VITRIN-INIT-BEGIN. This is a dead boot, not a kernel that answered. Last 20 lines:
$(tail -n 20 "$log")"
  if grep -q '^VITRIN-FATAL' <(declutter <"$log"); then
    fail "[$id] the init could not set the guest up: $(declutter <"$log" | grep -m1 '^VITRIN-FATAL'). A probe over a half-mounted guest renders 'unset' cells that read like kernel facts; refusing."
  fi
  grep -q '^VITRIN-INIT-DONE' <(declutter <"$log") \
    || fail "[$id] the guest died part-way through -- VITRIN-INIT-BEGIN with no VITRIN-INIT-DONE. Last 20 lines:
$(tail -n 20 "$log")"

  local st_isolation st_floor
  st_isolation="$(phase_status "$log" isolation)"
  st_floor="$(phase_status "$log" floor)"
  [ "$st_isolation" = "exit=0" ] \
    || fail "[$id] \`vitrind --print-isolation\` reported $st_isolation, not exit=0. $(phase_body "$log" isolation | tail -n 3)"
  [ "$st_floor" = "exit=0" ] \
    || fail "[$id] \`vitrind --print-floor\` reported $st_floor, not exit=0"

  local isolation floor
  isolation="$(phase_body "$log" isolation)"
  floor="$(phase_body "$log" floor)"

  grep -qE '^vitrin-isolation [0-9]+$' <<<"$isolation" \
    || fail "[$id] the isolation phase produced no schema-tagged row"
  grep -qE '^tier=' <<<"$isolation" \
    || fail "[$id] the row has no tier= line, so it is a truncated read rather than a complete one"
  grep -qE '^vitrin-floor [0-9]+$' <<<"$floor" \
    || fail "[$id] the floor phase produced no schema-tagged block"

  # Check 8: this must be the kernel that was PINNED.
  local booted
  booted="$(grep -m1 '^kernel.release=' <<<"$isolation" || true)"
  [ "$booted" = "kernel.release=$release" ] \
    || fail "[$id] booted kernel is not the pinned one: the row says '$booted', kernels.manifest pins 'kernel.release=$release'"

  # Check 9: exactly one startup verdict.
  local startup verdict verbatim refusals admissions
  startup="$(phase_body "$log" startup | sed -E 's/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z +//')"
  refusals="$(grep -c "fatal: this build's isolation floor requires" <<<"$startup" || true)"
  admissions="$(grep -c 'realms will be confined:' <<<"$startup" || true)"
  if [ "$refusals" -ge 1 ] && [ "$admissions" -eq 0 ]; then
    verdict=refused
    verbatim="$(grep -m1 "fatal: this build's isolation floor requires" <<<"$startup")"
  elif [ "$admissions" -ge 1 ] && [ "$refusals" -eq 0 ]; then
    verdict=admitted
    verbatim="$(grep -m1 'realms will be confined:' <<<"$startup")"
  else
    fail "[$id] the startup phase produced $refusals refusal lines and $admissions admission lines. Exactly one of the two is a verdict; anything else means \`vitrind\` did not reach its isolation preflight, and this harness will not guess. Phase output:
$startup"
  fi

  # The one normalized cell, named here and named on the page. It is derived
  # from guest memory and tracks the COMPRESSED initramfs size, so it moves
  # whenever the binary in the image changes size -- which is every unrelated
  # commit. The raw reading is published in the header rather than dropped.
  local raw_mun
  raw_mun="$(grep -m1 '^policy.max_user_namespaces=' <<<"$isolation" | cut -d= -f2)"
  isolation="$(sed -E 's/^policy\.max_user_namespaces=.*/policy.max_user_namespaces=<normalized; raw reading in this row'"'"'s header>/' <<<"$isolation")"

  local qemu_line="qemu-system-x86_64 -accel <accel> -m $MEMORY_MIB -smp $VCPUS -nographic -no-reboot -kernel <vmlinuz> -initrd <initramfs> -append \"$APPEND_BASE"
  [ "$append_extra" = "-" ] || qemu_line="$qemu_line $append_extra"
  qemu_line="$qemu_line\""

  cat <<EOF
# vitrin per-kernel isolation row. GENERATED by tests/kernel-matrix/collect.sh.
# DO NOT HAND-EDIT: \`collect.sh --check\` re-boots this kernel and diffs, so an
# edit here is a red build rather than a corrected row.
#
# Lines beginning \`#~\` are provenance a legitimate re-run may change (a date, a
# memory-derived reading). Every other line -- header and body alike -- is
# compared byte-for-byte.
# id: $id
# class: $class
# kernel.release: $release
# source.url: $url
# source.sha256: $deb_sha
# vmlinuz.member: $member
# vmlinuz.sha256: $vmlinuz_sha
# boot: $qemu_line
# userspace: minimal initramfs from tests/kernel-matrix/init.c -- static PID 1, /vitrind and its library closure, /proc /sys /dev /run mounted, NO distribution policy loaded, NO /etc/subuid, uid 0 with a full capability set. This is a KERNEL row and never a distribution row.
# normalized: policy.max_user_namespaces -- memory-derived, tracks the compressed initramfs size, so it would go stale on commits that only change vitrind's size
#~ collected: $(date -u +%Y-%m-%d)
#~ accel: $ACCEL
#~ policy.max_user_namespaces (raw): $raw_mun
--- vitrind --print-isolation ---
$isolation
--- vitrind --print-floor ---
$floor
--- vitrind --headless --consent=auto-approve: the isolation preflight's own line ---
verdict: $verdict
$verbatim
EOF
}

# ---------------------------------------------------------------------------
# 6. Drive the manifest.
# ---------------------------------------------------------------------------
initramfs="$(build_initramfs)"
note "initramfs: $initramfs ($(stat -c%s "$initramfs") bytes), vitrind: $VITRIND, accel: $ACCEL"

processed=0
seen_ids=()
problems=0

# The manifest is read into an array BEFORE the first boot rather than streamed
# into the loop. Anything the loop runs that touches stdin -- and `-nographic`
# qemu does -- would otherwise consume records nothing ever notices are gone.
# The `</dev/null` in `boot` fixes that at the source; this makes the fix
# structural rather than a discipline the next tool added here has to keep.
records=()
while IFS= read -r line; do
  case "$line" in ''|'#'*) continue ;; esac
  records+=("$line")
done <"$MANIFEST"
[ ${#records[@]} -gt 0 ] || fail "kernels.manifest holds no records"

for record in "${records[@]}"; do
  read -r id class release url deb_sha member vmlinuz_sha append_extra rest <<<"$record"
  [ -z "$rest" ] || fail "kernels.manifest: record '$id' has more than the eight documented fields"
  [ -n "$append_extra" ] || fail "kernels.manifest: record '$id' has fewer than the eight documented fields"
  seen_ids+=("$id")
  if [ ${#only[@]} -gt 0 ]; then
    local_match=0
    for want in "${only[@]}"; do [ "$want" = "$id" ] && local_match=1; done
    [ $local_match -eq 1 ] || continue
  fi

  deb="$(fetch_deb "$url" "$deb_sha")"
  vmlinuz="$(extract_vmlinuz "$deb" "$member" "$vmlinuz_sha")"
  log="$CACHE/logs/$id.log"
  note "booting $id ($release)"
  boot "$vmlinuz" "$initramfs" "$append_extra" "$log"
  row="$(render_row "$id" "$class" "$release" "$url" "$deb_sha" "$member" "$vmlinuz_sha" "$append_extra" "$log")"
  processed=$((processed + 1))

  target="$ROWS/$id.row"
  if [ "$check" -eq 1 ]; then
    [ -f "$target" ] || fail "[$id] there is no checked-in row at ${target#$root/}. The kernel answered; nothing holds that answer. Run tests/kernel-matrix/collect.sh and commit rows/."
    if ! diff -u <(grep -v '^#~' "$target") <(grep -v '^#~' <<<"$row"); then
      echo "FAIL: [$id] the checked-in row no longer matches what this kernel answers. Regenerate with tests/kernel-matrix/collect.sh and commit, or explain the change." >&2
      problems=$((problems + 1))
      continue
    fi
    # The ageing flag. The bytes above still hold; this says how long ago
    # anybody wrote them down, because a row nobody re-measures is a claim
    # that ages silently.
    collected="$(sed -n 's/^#~ collected: //p' "$target" | head -n1)"
    [ -n "$collected" ] || fail "[$id] ${target#$root/} carries no '#~ collected:' date, so its age cannot be stated"
    age=$(( ( $(date -u +%s) - $(date -u -d "$collected" +%s) ) / 86400 ))
    if [ "$age" -gt "$MAX_ROW_AGE_DAYS" ]; then
      echo "FAIL: [$id] the row still matches, but it was written $age days ago and this repository's limit is $MAX_ROW_AGE_DAYS. Re-run tests/kernel-matrix/collect.sh and commit, so the published date is a date somebody measured on." >&2
      problems=$((problems + 1))
      continue
    fi
    note "[$id] OK -- $release, written $age days ago"
  else
    printf '%s\n' "$row" >"$target"
    note "[$id] wrote ${target#$root/}"
  fi
done

# Check 11: a run that measured nothing is a failure.
[ "$processed" -gt 0 ] || fail "no kernel was processed. The manifest has ${#seen_ids[@]} records and --only matched none of them; a collector that measured nothing must not exit 0."

# Check 10: the row set and the manifest must be the same set (full runs).
if [ ${#only[@]} -eq 0 ]; then
  for f in "$ROWS"/*.row; do
    [ -e "$f" ] || continue
    b="$(basename "$f" .row)"
    found=0
    for id in "${seen_ids[@]}"; do [ "$id" = "$b" ] && found=1; done
    [ $found -eq 1 ] || fail "rows/$b.row has no record in kernels.manifest. A row nothing can re-measure is not a row -- add the record, or delete the file."
  done
else
  note "PARTIAL RUN (--only ${only[*]}): ${#seen_ids[@]} records in the manifest, $processed measured. This cannot verify the row set."
fi

if [ "$problems" -gt 0 ]; then
  echo "FAIL: $problems of $processed rows are stale." >&2
  exit 1
fi

if [ "$check" -eq 1 ]; then
  note "--check: $processed rows re-measured and unchanged"
else
  note "$processed rows written to ${ROWS#$root/} -- review \`git diff\` and commit"
  note "next: cargo xtask kernel-matrix   (renders docs/book/src/isolation-kernels.md from these rows)"
fi
