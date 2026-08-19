#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Dependency install for the CI shim job (.github/workflows/ci.yml).
#
# OWNERSHIP: the c-shim track. The PR that lands shim/meson.build (P1.6.1)
# activates the Meson CI steps and runs this script for real for the first
# time -- it MUST adjust this list to the exact build-dependency set of its
# vendored wlroots subproject. CI deliberately delegates the list to this
# file so the workflow and the shim's dependency reality cannot drift apart.
#
# D11 (docs/plan/01-phase-1-mvp.md §6): wlroots is pinned + vendored as a
# Meson subproject. This list therefore contains wlroots's *build*
# dependencies -- NOT libwlroots-dev: Ubuntu 24.04 ships wlroots 0.17.1,
# which would pin the shim to an API the project did not choose and shadow
# the vendored subproject.
#
# libexpat1-dev + libxml2-dev are for wlroots's NESTED wayland subproject:
# Ubuntu 24.04 ships wayland 1.22, below wlroots 0.19's >=1.23.1 floor, so the
# vendored build compiles wayland from source, and its wayland-scanner needs
# expat + libxml2 (DTD-validated protocol parsing). Drop them if the base image
# ever ships wayland >= 1.23.1.
#
# The last two lines add the shim's headless-acceptance tooling (not wlroots
# build deps): wayland-info (wayland-utils) checks the advertised globals and
# weston-terminal (weston) is the blind client; fonts-dejavu-core lets
# weston-terminal render glyphs in the bare container.
#
# libgtk-3-dev is for the P1.6.3 R5 gate -- "héllo→世界 arrives intact in a GTK
# text field". That criterion is about the toolkit's INPUT-METHOD layer, not
# about keysyms, so nothing but a real GtkEntry can prove it; and a criterion
# that only ever reports SKIP in CI is a criterion nobody is holding. It is a
# pure C/GLib package, so the no-Rust invariant below is untouched.
#
# binutils is for `meson test loader-independence` (issue #283), which reads
# the shim's dynamic section with `readelf` and FAILS rather than skips when
# the tool is absent -- that policy is the point of the test, so the tool has
# to be named here rather than inherited. The container gets binutils
# transitively from gcc today; an unnamed dependency means a change to gcc's
# dependency chain would turn the shim job red with a message about realms.
# `ldd` needs no line of its own: it ships in libc-bin, which is Essential.
#
# Invariant checked by CI after this script runs: nothing here may pull
# rustc/cargo onto PATH (the shim job's no-Rust acceptance criterion).
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
  meson ninja-build pkg-config binutils \
  libwayland-dev wayland-protocols \
  libpixman-1-dev libxkbcommon-dev \
  libdrm-dev libgbm-dev libinput-dev libudev-dev libseat-dev \
  libdisplay-info-dev libliftoff-dev hwdata \
  libexpat1-dev libxml2-dev \
  weston wayland-utils fonts-dejavu-core \
  libgtk-3-dev
