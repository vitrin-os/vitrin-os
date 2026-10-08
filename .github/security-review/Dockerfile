# SPDX-License-Identifier: Apache-2.0
#
# The security-review image: everything a reviewer needs to build, test and
# fuzz this tree, fetched while the network is up, so that the review itself
# can run with no network at all.
#
# WHAT IT IS FOR
# --------------
# Some reviewers -- people or automated tools -- work in a sandbox that has
# network access while it builds an image and none afterwards. This file is
# that build. It is not a release artifact, not a supported way to run
# vitrind, and not evidence for any milestone. docs/security-review.md says
# what to do inside it; SECURITY.md says what is in scope.
#
# THE CONTRACT
# ------------
#   * The repository checkout is the build context. `COPY . /src` puts it at
#     /src, which is also the working directory.
#   * Everything the review needs is downloaded HERE: the Rust toolchain
#     rust-toolchain.toml pins, every crate of the root workspace and of the
#     separate fuzz/ workspace, cargo-fuzz, the shim's Meson subprojects
#     (wlroots, wayland-protocols) and the Python SDK's test dependency.
#     After this file finishes, `cargo test --offline`, `cargo fuzz run
#     --sanitizer none`, the Meson-built shim and `python -m pytest
#     sdk/python/tests` need nothing from the network.
#     .github/workflows/security-review-image.yml builds this image and then
#     runs exactly those with `--network none`, so this sentence is checked
#     rather than hoped.
#   * Only default features are built. No command line here turns a cargo
#     feature on, and crates/xtask/src/limits.rs lists this directory among
#     the places it holds to that.
#   * The VITRIN_REQUIRE_* variables are deliberately NOT set. They are claims
#     about a CI runner (see the `rust` job in ci.yml), not about this image.
#     A test that needs what the host cannot give -- unprivileged user
#     namespaces, a Landlock ABI, a GPU -- therefore SKIPS here and prints a
#     marker saying so. A skip is not a pass; docs/security-review.md says how
#     to tell them apart.
#
# Build it from the repository root:
#
#   docker build -f .github/security-review/Dockerfile -t vitrin-review .
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/opt/venv/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# The C toolchain, git (the xtask gates ask git which paths are build output,
# so they need a work tree), Python, and the two libraries vitrind links for
# its Smithay backends -- the same two the `rust` job in ci.yml installs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         build-essential ca-certificates curl git python3 python3-venv \
         libxkbcommon-dev libpixman-1-dev \
    && rm -rf /var/lib/apt/lists/*

# rustup with no default toolchain: the pin in rust-toolchain.toml is the only
# toolchain this tree is built with, and `rustup toolchain install` below
# reads it, exactly as ci.yml does.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain none --no-modify-path

COPY . /src
WORKDIR /src

# The shim's build dependencies, from the script the `shim` job in ci.yml runs.
# One script, so the two lists cannot drift apart.
RUN bash shim/ci/install-deps.sh && rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install && rustup show active-toolchain

# The root workspace, and every test binary in it, compiled now so that
# running them later needs no download.
RUN cargo build --workspace --all-targets --locked \
    && cargo test --workspace --no-run --locked

# fuzz/ is its own workspace with its own lockfile. `--sanitizer none` is the
# stable-toolchain spelling fuzz/README.md explains; ASan would need nightly,
# which this image does not install.
RUN cargo install cargo-fuzz --locked \
    && cargo fuzz build --sanitizer none protocol_decode \
    && cargo fuzz build --sanitizer none ipc_framing

# `meson setup` is what fetches the wlroots and wayland-protocols wraps, so it
# must happen while the network is up. shim/build is gitignored.
RUN meson setup shim/build shim \
    && meson compile -C shim/build

# The Python SDK and its one test dependency, in a venv that is first on PATH.
# Editable, so a reviewer's change to sdk/python/src is what the tests run.
RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir -e './sdk/python[dev]'

# Cheap tests, so a reviewer knows the image works. A failure here is printed,
# not fatal: the image's job is to be usable for review, and a tree with a
# failing test is still worth reviewing.
RUN (cargo test -p vitrin-protocol --locked \
       || echo "WARNING: cargo test -p vitrin-protocol reported failures") \
    && (python -m pytest -q sdk/python/tests \
       || echo "WARNING: the Python SDK tests reported failures") \
    && (meson test -C shim/build --suite vitrin-shim --print-errorlogs \
       || echo "WARNING: meson test reported failures")
