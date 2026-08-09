#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# run-advisory.sh -- the ADVISORY VKMS rung (WS-E.3.4, issue #220).
#
# Attempts to load the virtual KMS driver on a CI runner, then measures what a
# DRM backend would actually need from it, and writes the answer -- including
# "nothing, the module is unavailable" -- to $GITHUB_STEP_SUMMARY.
#
# THIS SCRIPT ALWAYS EXITS 0. That is one of the two independent mechanisms
# keeping this rung advisory, exactly as shim/wlcs/run-advisory.sh does for the
# wlcs rung; the other is `continue-on-error: true` on the job. Either alone is
# one bad rebase away from silently starting to gate merges (issue #220
# decision 4: a rung whose availability depends on the runner image carrying
# linux-modules-extra must never block a merge when that image changes).
#
# What this rung can and cannot prove is decided in advance and is NOT a
# function of what it happens to find:
#
#   CAN plausibly cover  -- connector enumeration, mode setting, atomic commit,
#                           the page-flip event loop.
#   UNKNOWN until measured -- the GBM + GLES scanout path. vkms exposes no
#                           render node, so the GLES half needs a software
#                           renderer and may not import into a vkms scanout
#                           buffer at all. probe-gbm-egl.c measures it.
#   NEVER covers         -- that the DRM backend lights a real panel, presents
#                           a real frame, or delivers a real input event. No
#                           green check in this repository will ever prove
#                           those; docs/drm-bringup.md, executed by a human, is
#                           the only evidence for them.
#
# Usage: run-advisory.sh <output-dir>
set -uo pipefail   # deliberately NOT -e: a failed measurement is a result.

OUT="${1:?usage: run-advisory.sh <output-dir>}"
mkdir -p "$OUT"
LOG="$OUT/vkms-advisory.log"
SUMMARY="${GITHUB_STEP_SUMMARY:-$OUT/summary.md}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

say() { printf '%s\n' "$*" | tee -a "$LOG"; }
sum() { printf '%s\n' "$*" >> "$SUMMARY"; }

say "== vkms-advisory =="
say "kernel: $(uname -r)"
say "date:   $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# ---------------------------------------------------------------------------
# 1. Try to make the module available, then load it.
# ---------------------------------------------------------------------------
# On GitHub's Ubuntu images vkms lives in linux-modules-extra-$(uname -r),
# which may or may not be installable for the exact running kernel. Both the
# install and the modprobe are allowed to fail; neither is a test result.
MODULE_STATE="unavailable"
MODULE_WHY=""

if [ -e "/sys/module/vkms" ]; then
    MODULE_STATE="already-loaded"
else
    if ! modinfo vkms >/dev/null 2>&1; then
        say "-- vkms not present; trying linux-modules-extra-$(uname -r)"
        sudo apt-get update -qq >>"$LOG" 2>&1
        sudo apt-get install -y -qq "linux-modules-extra-$(uname -r)" \
            >>"$LOG" 2>&1 || say "-- apt could not install linux-modules-extra"
    fi
    if modinfo vkms >/dev/null 2>&1; then
        if sudo modprobe vkms >>"$LOG" 2>&1; then
            MODULE_STATE="loaded"
        else
            MODULE_STATE="unavailable"
            MODULE_WHY="modprobe vkms failed (module present, load refused)"
        fi
    else
        MODULE_WHY="no vkms module for kernel $(uname -r) after installing linux-modules-extra"
    fi
fi
say "-- module state: $MODULE_STATE ${MODULE_WHY:+($MODULE_WHY)}"

# ---------------------------------------------------------------------------
# 2. Declared skip. Never a silent pass.
# ---------------------------------------------------------------------------
if [ "$MODULE_STATE" = "unavailable" ]; then
    say "-- DECLARED SKIP"
    {
        sum "## vkms-advisory: SKIPPED (vkms unavailable on this runner)"
        sum ""
        sum "\`sudo modprobe vkms\` did not produce a device on this runner image."
        sum ""
        sum "- kernel: \`$(uname -r)\`"
        sum "- reason: ${MODULE_WHY:-unknown}"
        sum ""
        sum "**This is a setup gap on the runner, not a test result.** Nothing was"
        sum "exercised and nothing is claimed. This rung is ADVISORY and never gates a"
        sum "PR (issue #220 decision 4)."
        sum ""
        sum "Whether the DRM backend works is **not** measured by this job in any"
        sum "circumstance -- see \`docs/book/src/limits.md\` (\"The DRM/KMS backend will"
        sum "never have a green gate behind it\") and \`docs/drm-bringup.md\`, the manual"
        sum "runbook that is the only evidence for it."
    }
    exit 0
fi

# ---------------------------------------------------------------------------
# 3. Inventory: which DRM nodes appeared, and which one is vkms.
# ---------------------------------------------------------------------------
say "-- /dev/dri:"
ls -l /dev/dri 2>&1 | tee -a "$LOG"

VKMS_CARD=""
CONNECTOR_LINES=""
for card in /sys/class/drm/card[0-9]*; do
    # Connector entries (card1-eDP-1) share the prefix AND resolve a
    # device/driver symlink of their own, so the name test must come first.
    case "$card" in *-*) continue ;; esac
    [ -e "$card/device/driver" ] || continue
    drv="$(basename "$(readlink -f "$card/device/driver")")"
    n="$(basename "$card")"
    say "-- $n driver=$drv"
    [ "$drv" = "vkms" ] && VKMS_CARD="/dev/dri/$n"
done
for c in /sys/class/drm/card[0-9]*-*/status; do
    [ -e "$c" ] || continue
    CONNECTOR_LINES="${CONNECTOR_LINES}$(basename "$(dirname "$c")")=$(cat "$c") "
done
say "-- connectors: ${CONNECTOR_LINES:-none}"

RENDER_NODES="$(ls /dev/dri/renderD* 2>/dev/null | tr '\n' ' ')"
say "-- render nodes: ${RENDER_NODES:-none}"

# ---------------------------------------------------------------------------
# 4. The measurement issue #220 decision 5 asks for, taken rather than presumed.
# ---------------------------------------------------------------------------
PROBE_OUT="$OUT/probe.txt"
PROBE_SUMMARY="not-run"
if [ -z "$VKMS_CARD" ]; then
    say "-- no vkms card node appeared; skipping the GBM/EGL probe"
    PROBE_SUMMARY="no-vkms-card-node"
elif ! cc -Wall -Wextra -o "$OUT/probe-gbm-egl" "$HERE/probe-gbm-egl.c" \
        $(pkg-config --cflags --libs libdrm gbm egl glesv2) >>"$LOG" 2>&1; then
    say "-- probe failed to BUILD (missing dev packages?)"
    PROBE_SUMMARY="probe-build-failed"
else
    say "-- probing $VKMS_CARD"
    "$OUT/probe-gbm-egl" "$VKMS_CARD" > "$PROBE_OUT" 2>>"$LOG"
    cat "$PROBE_OUT" | tee -a "$LOG"
    PROBE_SUMMARY="$(grep -o 'summary=[^ ]*' "$PROBE_OUT" | tail -1 | cut -d= -f2)"
    PROBE_SUMMARY="${PROBE_SUMMARY:-probe-produced-no-summary}"
fi
say "-- probe summary: $PROBE_SUMMARY"

# ---------------------------------------------------------------------------
# 5. Publish. What it exercised AND what it did not, on every run.
# ---------------------------------------------------------------------------
{
    sum "## vkms-advisory (issue #220, ADVISORY -- never blocks this PR)"
    sum ""
    sum "vkms: **$MODULE_STATE** on kernel \`$(uname -r)\`."
    sum ""
    sum "| Measurement | Result |"
    sum "|---|---|"
    sum "| vkms card node | \`${VKMS_CARD:-none}\` |"
    sum "| DRM connectors seen | \`${CONNECTOR_LINES:-none}\` |"
    sum "| render nodes | \`${RENDER_NODES:-none}\` |"
    sum "| **GBM + EGL/GLES scanout path** | **\`$PROBE_SUMMARY\`** |"
    sum ""
    sum "Per-stage probe output (\`open\`, \`modeset-resources\`, \`gbm-device\`,"
    sum "\`egl-display\`, \`egl-config\`, \`gbm-scanout-surface\`, \`gles-context\`,"
    sum "\`render-and-lock-front\`):"
    sum ""
    sum '```'
    if [ -s "$PROBE_OUT" ]; then cat "$PROBE_OUT" >> "$SUMMARY"; else sum "(probe did not run: $PROBE_SUMMARY)"; fi
    sum '```'
    sum ""
    sum "### What this run does NOT show"
    sum ""
    sum "Read this every time; it does not change with the numbers above."
    sum ""
    sum "- **It does not show the DRM backend lights a panel, presents a frame, or"
    sum "  delivers an input event.** Runners have no real display controller and no"
    sum "  seat. The only evidence for those is \`docs/drm-bringup.md\`, executed by a"
    sum "  human on one machine and dated -- one GPU, one panel, one kernel."
    sum "- **It sets no mode and takes no DRM master.** The probe is a capability"
    sum "  measurement; \`drmSetMaster\`, \`drmModeSetCrtc\` and \`drmModePageFlip\` are"
    sum "  never called."
    sum "- **A green tick here is not a \"drm\" tick.** This rung is advisory and is"
    sum "  never to be cited without that word."
    sum ""
    sum "Full log: the \`vkms-advisory-results\` artifact. Standing statement of what"
    sum "this covers: \`docs/book/src/limits.md\`."
}

say "-- done (exit 0, advisory)"
exit 0
