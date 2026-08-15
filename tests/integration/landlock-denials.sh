#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Harvest the filesystem accesses a realm is DENIED, from one integration-test
# run, and print the distinct paths.
#
# This is the instrument P2.6.9 needs and P2.6.3 did not have. The enumerated
# read set in `crates/vitrin-realm-init/src/landlock.rs` is a list somebody
# wrote down; until this script existed the only way to ask "what does a real
# app actually reach for that the list omits?" was to read the list again.
#
#   bash tests/integration/landlock-denials.sh                  # test_real_gtk.py
#   bash tests/integration/landlock-denials.sh test_real_app.py test_real_gtk.py
#   bash tests/integration/landlock-denials.sh --control        # + a Landlock-off run
#
# It is NOT a gate. It asserts nothing, it is not in `run.sh`'s discovery, and
# a red test inside it is not a surprise; the denials are what we came for and
# the exit status reflects the harvest, never the test. (When this was written
# the GTK gates were themselves red at the shipped default. They are not since
# 2026-08-15 -- docs/book/src/limits.md,
# `landlock-breaks-nested-image-sandboxes` -- so the harvest now comes from
# passing runs.)
#
# ===========================================================================
# TWO BACKENDS, AND WHY THIS BOX NEEDS THE SECOND ONE
# ===========================================================================
#
# The kernel can name its own denials. Landlock ABI 7 (kernel 6.15) added a
# `flags` word to `landlock_restrict_self`; `LANDLOCK_RESTRICT_SELF_LOG_NEW_
# EXEC_ON` keeps the audit log running past the `execve`, which is the only
# side of that boundary where a realm's app runs. `VITRIN_LANDLOCK_AUDIT=1` in
# vitrind's environment is what makes the helper set it (the core forwards the
# name into the helper and nothing else can).
#
# Those records go to the kernel's **audit subsystem**, and that subsystem has
# a global on/off switch that has nothing to do with Landlock:
#
#   * `audit_enabled=0` -- the kernel default with no `audit=1` on the command
#     line and no auditd. `landlock_log_denial` returns before writing
#     anything, so the flag is set, the domain is enforced, and **no record
#     exists to read**, in dmesg, in the journal or in an audit log.
#   * `audit_enabled=1`, no auditd -- records reach the kernel ring buffer;
#     `journalctl -k` / `dmesg` show `type=1423` lines.
#   * auditd running -- `ausearch -m LANDLOCK_ACCESS` and
#     `/var/log/audit/audit.log`.
#
# MEASURED on the box this repo is developed on (Arch, kernel 7.1.8-arch1-3,
# Landlock ABI 9, 2026-08-14): `audit_enabled=0`, auditd inactive, and the boot
# journal says so in as many words --
#
#     audit: initializing netlink subsys (disabled)
#     audit: type=2000 audit(...): state=initialized audit_enabled=0 res=1
#     systemd-journald: Collecting audit messages is disabled.
#
# Turning it on needs root (`auditctl -e 1`, or `audit=1` on the kernel command
# line, plus `Audit=yes` in journald.conf or a running auditd to collect). So
# on an unprivileged developer box the audit backend harvests nothing, and this
# script says so and falls back rather than printing an empty list that looks
# like a clean result.
#
# The fallback is a **tracer on the outside**: `strace -D -f`, whose `-D` keeps
# vitrind a direct child of the harness so the process-tree walks every gate
# does still count the same levels (see `harness.CORE_WRAPPER`). What it gives
# up against the audit backend is attribution: a tracer sees `EACCES`, not
# *who* said it, so a DAC refusal and a Landlock refusal look alike. `--control`
# is the answer -- a second run at `--landlock=off`, same everything else, and
# the difference is the set the ruleset is responsible for.
# ===========================================================================

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

CONTROL=0
MODULES=()
for arg in "$@"; do
  case "$arg" in
    --control) CONTROL=1 ;;
    -h|--help) sed -n '3,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) MODULES+=("$arg") ;;
  esac
done
[ ${#MODULES[@]} -eq 0 ] && MODULES=(test_real_gtk.py)

PY="${PYTHON:-python3}"
OUT="$(mktemp -d -t vitrin-landlock-denials-XXXXXX)"
echo "==> artefacts: $OUT"

# ---- what this box can actually observe -----------------------------------
# `2>&1`, not `2>/dev/null`: the isolation matrix is written to STDERR.
abi="$("$REPO/target/debug/vitrind" --print-isolation 2>&1 \
        | grep -oE 'landlock\.abi=[0-9]+' | cut -d= -f2 | head -1)"
audit_state="$(journalctl -k -b --no-pager 2>/dev/null \
                | grep -oE 'audit_enabled=[0-9]+' | tail -1)"
auditd="$(systemctl is-active auditd 2>/dev/null || true)"
echo "==> kernel        : $(uname -r)"
echo "==> landlock abi  : ${abi:-unknown} (from vitrind --print-isolation)"
echo "==> audit_enabled : ${audit_state:-unknown} (from the boot journal)"
echo "==> auditd        : ${auditd:-unknown}"

BACKEND=strace
if [ "$audit_state" = "audit_enabled=1" ]; then
  BACKEND=audit
fi
echo "==> backend       : $BACKEND"
if [ "$BACKEND" = strace ]; then
  cat <<'WHY'
    The kernel's audit subsystem is OFF, so LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON
    would be set and would produce nothing. To use the audit backend instead, as
    root, one of:

        auditctl -e 1                       # this boot only
        # or add `audit=1` to the kernel command line and reboot

    then re-run. Read the records with, in order of preference:

        ausearch -m LANDLOCK_ACCESS -ts recent       # if auditd is running
        journalctl -k --since "<start>" | grep 'type=1423'
        dmesg | grep 'type=1423'                     # needs CAP_SYSLOG when
                                                     # kernel.dmesg_restrict=1
WHY
fi

# ---- one run --------------------------------------------------------------
#
# $1 = label, $2 = the --landlock setting ("" = the shipped default).
run_once() {
  local label="$1" landlock="$2"
  local trace="$OUT/$label.strace" log="$OUT/$label.log"
  local since; since="$(date '+%Y-%m-%d %H:%M:%S')"

  echo ""
  echo "==> run: $label (landlock=${landlock:-<shipped default>})"

  local -a env=(
    "PYTHONPATH=$REPO/sdk/python/src"
    "VITRIN_REPO=$REPO"
    "VITRIN_C_SHIM_BIN=${VITRIN_C_SHIM_BIN:-$REPO/shim/build/vitrin-shim}"
  )
  [ -n "$landlock" ] && env+=("VITRIN_LANDLOCK=$landlock")

  if [ "$BACKEND" = audit ]; then
    env+=("VITRIN_LANDLOCK_AUDIT=1")
  else
    # `-D` keeps vitrind a DIRECT child of the harness -- see the header.
    # `%file` plus `connect` is every syscall that can be refused by a path
    # rule or by RESOLVE_UNIX; `status=failed` drops the ~99% of lines that
    # succeeded, which is what keeps a GTK startup traceable inside the
    # harness's own 90-second per-test alarm.
    env+=("VITRIN_CORE_WRAPPER=strace -D -f -qq -s 512 -e trace=%file,connect -e status=failed -o $trace")
  fi

  env "${env[@]}" "$PY" -m unittest discover -s tests/integration \
      -p "$label" -v >"$log" 2>&1
  local status=$?
  echo "    unittest exit $status (a red gate here is expected at the shipped default)"
  grep -E '\.\.\. (ok|FAIL|ERROR|skipped)' "$log" | sed 's/^/      /' || true

  if [ "$BACKEND" = audit ]; then
    # Every reader, because which one has the records depends on the host.
    { ausearch -m LANDLOCK_ACCESS -ts "$since" 2>/dev/null || true
      journalctl -k --since "$since" --no-pager 2>/dev/null | grep 'type=1423' || true
      dmesg 2>/dev/null | grep 'type=1423' || true
    } >"$OUT/$label.audit"
    # Landlock's record spells the path as path="..." beside blockers=...
    sed -n 's/.*\(blockers=[^ ]*\).*\(path="[^"]*"\).*/\1\t\2/p' "$OUT/$label.audit" \
      | sort -u >"$OUT/$label.denials"
    grep -oE 'path="[^"]*"' "$OUT/$label.audit" | sed 's/^path="//; s/"$//' \
      | sort -u >"$OUT/$label.paths"
  else
    # `-o` with `-D`: the tracer flushes on exit, which is after the core it
    # traced. A short settle beats a truncated harvest.
    sleep 1
    # The path is the first quoted string on the line for every syscall in
    # `%file`; `connect` carries it as sun_path="...". Denials only: EACCES is
    # what a Landlock **path rule** answers, EPERM what the **mount-topology**
    # denial answers -- and those two are different findings about the same
    # ruleset, so the errno and the syscall are kept beside the path. Collapsing
    # them into a bare path list makes `openat("/")` and `mount(NULL,"/")` read
    # as one denial, which is exactly the confusion this measurement exists to
    # remove: one is a grant the enumeration declined, the other is a right no
    # grant can restore.
    #
    # **`EPERM mount /` is a correct attribution, and the reasoning that makes
    # it look wrong is a category error worth writing down here.** "Landlock has
    # no mount right, so it cannot gate mount(2)" is backwards: it has no mount
    # *right* precisely because it does not permit mount at all. Any task
    # holding a Landlock **filesystem** domain -- one whose `handled_access_fs`
    # is non-zero -- gets an unconditional `EPERM` from `mount(2)`,
    # `umount(2)`, a remount, `pivot_root(2)` and `move_mount(2)`. There is
    # nothing to grant and no rule that restores it, so no `--landlock` rung, no
    # entry in `landlock::grants` and no wider mask changes this line. Measured
    # 2026-08-15 by flipping that one field with the rights held constant at
    # "everything on `/`": non-zero mask -> EPERM, `handled_access_fs = 0` with
    # only `scoped` set -> the same mount returns 0.
    sed -nE 's/^[0-9]+[[:space:]]+([a-z_0-9]+)\(.*"(\/[^"]*)".* = -1 (EACCES|EPERM) .*/\3\t\1\t\2/p' \
      "$trace" 2>/dev/null | sort -u >"$OUT/$label.denials"
    cut -f3 "$OUT/$label.denials" | sort -u >"$OUT/$label.paths"
  fi
  echo "    denials:  $(wc -l <"$OUT/$label.denials") distinct (errno, syscall, path)"
  echo "    paths:    $(wc -l <"$OUT/$label.paths") distinct"
}

for module in "${MODULES[@]}"; do
  run_once "$module" ""
  if [ "$CONTROL" -eq 1 ]; then
    mv "$OUT/$module.paths" "$OUT/$module.default.paths"
    mv "$OUT/$module.denials" "$OUT/$module.default.denials"
    run_once "$module" "off"
    mv "$OUT/$module.paths" "$OUT/$module.off.paths"
    mv "$OUT/$module.denials" "$OUT/$module.off.denials"
    comm -23 "$OUT/$module.default.denials" "$OUT/$module.off.denials" \
      >"$OUT/$module.attributable.denials"
    comm -23 "$OUT/$module.default.paths" "$OUT/$module.off.paths" \
      >"$OUT/$module.attributable.paths"
  fi
done

# ---- the answer -----------------------------------------------------------
echo ""
echo "==========================================================================="
for module in "${MODULES[@]}"; do
  default_denials="$OUT/$module.denials"
  default_paths="$OUT/$module.paths"
  [ -f "$default_denials" ] || default_denials="$OUT/$module.default.denials"
  [ -f "$default_paths" ] || default_paths="$OUT/$module.default.paths"
  echo "$module -- SHIPPED DEFAULT ($BACKEND backend, errno / syscall / path)"
  echo "==========================================================================="
  cat "$default_denials"
  echo ""
  echo "--- distinct denied paths ---"
  cat "$default_paths"
  if [ "$CONTROL" -eq 1 ]; then
    echo ""
    echo "--- also denied at --landlock=off, so NOT the ruleset's doing ---"
    cat "$OUT/$module.off.denials"
    echo ""
    echo "--- ATTRIBUTABLE TO THE RULESET (default minus off) ---"
    cat "$OUT/$module.attributable.denials"
  fi
done
echo "==========================================================================="
echo "raw traces and unittest logs: $OUT"
