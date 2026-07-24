project(
  'vitrin-shim',
  'c',
  version: '0.1.0',
  license: 'MIT',
  meson_version: '>=1.3',
  default_options: [
    'c_std=c11',
    'warning_level=2',
    'werror=true',
  ],
)

# Every wlr/* header guards itself behind WLR_USE_UNSTABLE.
add_project_arguments('-DWLR_USE_UNSTABLE', language: 'c')

# D11 -- pin wlroots 0.19.x and vendor it as a Meson subproject. System-first:
# when a matching wlroots-0.19 is installed (fast local + CI) it is used and
# subprojects/wlroots.wrap stays dormant; `meson setup --wrap-mode=forcefallback`
# (or a machine without it) builds the pinned 0.19.3 checkout instead. The
# [provide] name in the wrap resolves the fallback to wlroots' own dependency.
# When the vendored fallback builds, keep it lean and faithful to how the shim
# runs: no example apps, and no XWayland (the shim is Wayland-only until Phase
# 3). Renderers/backends stay 'auto' -- wlroots auto-enables only those whose
# deps are present and falls back to its own nested wraps otherwise, so the
# build never hard-fails on a missing optional dep. These options are ignored
# when a system wlroots-0.19 is used.
wlroots = dependency(
  'wlroots-0.19',
  version: ['>=0.19', '<0.20'],
  fallback: ['wlroots', 'wlroots'],
  default_options: ['examples=false', 'xwayland=disabled'],
)

wayland_server = dependency('wayland-server')
pixman = dependency('pixman-1')
xkbcommon = dependency('xkbcommon')
# drm_fourcc.h, pulled in transitively by the linux-dmabuf headers.
drm = dependency('libdrm')

# wayland-protocols only ships XML; we read its data dir and run the scanner
# ourselves. System-first with a pinned fallback, mirroring wlroots.
wl_protocols = dependency(
  'wayland-protocols',
  version: '>=1.41',
  fallback: ['wayland-protocols', 'wayland_protocols'],
  default_options: ['tests=false'],
)
wl_protocol_dir = wl_protocols.get_variable(
  pkgconfig: 'pkgdatadir',
  internal: 'pkgdatadir',
)

# Resolve the build-machine wayland-scanner. From a system pkg-config dep it
# comes via the `wayland_scanner` .pc variable; from the vendored wayland
# subproject (wrap fallback) that variable isn't retrievable, so fall back to
# the program name, which the subproject registers via override_find_program.
wl_scanner_dep = dependency('wayland-scanner', native: true)
wayland_scanner = find_program(
  wl_scanner_dep.get_variable(
    pkgconfig: 'wayland_scanner',
    internal: 'wayland_scanner',
    default_value: 'wayland-scanner',
  ),
  native: true,
)

protocols = [
  ['xdg-shell', 'stable/xdg-shell/xdg-shell.xml'],
  ['xdg-decoration', 'unstable/xdg-decoration/xdg-decoration-unstable-v1.xml'],
  ['linux-dmabuf', 'stable/linux-dmabuf/linux-dmabuf-v1.xml'],
]

protocol_headers = []
protocol_sources = []
xdg_shell_code = []
foreach p : protocols
  name = p[0]
  xml = wl_protocol_dir / p[1]
  protocol_headers += custom_target(
    name + '-server-header',
    input: xml,
    output: name + '-protocol.h',
    command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
  )
  # `private-code` emits the marshalling tables, which are the same object on
  # both sides of the wire -- so the acceptance client below reuses the very
  # same generated file the shim links, rather than a second copy of it.
  code = custom_target(
    name + '-private-code',
    input: xml,
    output: name + '-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
  protocol_sources += code
  if name == 'xdg-shell'
    xdg_shell_code = code
  endif
endforeach

# --- P1.6.4 probe catalogue --------------------------------------------
# Interfaces the shim DOES NOT IMPLEMENT, compiled in so `--probe-globals`
# can advertise them and make an app's demand for one an observable
# `wl_registry.bind` (shim/include/ledger.h explains why demand is otherwise
# invisible). Only the marshalling tables are used; there is no server
# implementation of any of these and there is not meant to be one.
#
# THE LIST IS EVIDENCE-DERIVED, not aspirational: it is the union of the
# registry interface names referenced by the three toolkits on the R4
# bring-up ladder, read out of the shipped binaries --
#   strings libxul.so /usr/lib/libgdk-3.so.0 /usr/lib/libgtk-4.so.1
# -- restricted to registry-bindable singletons. An interface no toolkit on
# the ladder mentions cannot be demanded by one, so probing it would only
# dilute the report.
#
# `wl_subcompositor` is deliberately NOT here: it lives in the core protocol,
# so its marshalling table is already linked, and probing it is a one-line
# entry in the catalogue array rather than a scanner invocation.
#
# Each row is [candidate xml paths..., 'interface_symbol_stem']. Candidates
# are tried in order and the first that exists wins, because protocols
# migrate between unstable/ and stable/ across wayland-protocols releases and
# a hard-coded path would make the shim fail to CONFIGURE on a machine whose
# wayland-protocols is a version off. A row that resolves to nothing is
# reported at configure time and dropped from the catalogue -- the catalogue
# size is stamped into every ledger report, so a run can never silently probe
# fewer interfaces than its reader assumes.
fs = import('fs')
probe_protocols = [
  [['stable/viewporter/viewporter.xml'], 'wp_viewporter'],
  [['stable/presentation-time/presentation-time.xml'], 'wp_presentation'],
  [['unstable/pointer-constraints/pointer-constraints-unstable-v1.xml'], 'zwp_pointer_constraints_v1'],
  [['unstable/relative-pointer/relative-pointer-unstable-v1.xml'], 'zwp_relative_pointer_manager_v1'],
  [['unstable/pointer-gestures/pointer-gestures-unstable-v1.xml'], 'zwp_pointer_gestures_v1'],
  [['unstable/primary-selection/primary-selection-unstable-v1.xml'], 'zwp_primary_selection_device_manager_v1'],
  [['unstable/idle-inhibit/idle-inhibit-unstable-v1.xml'], 'zwp_idle_inhibit_manager_v1'],
  [['unstable/keyboard-shortcuts-inhibit/keyboard-shortcuts-inhibit-unstable-v1.xml'], 'zwp_keyboard_shortcuts_inhibit_manager_v1'],
  [['unstable/text-input/text-input-unstable-v3.xml'], 'zwp_text_input_manager_v3'],
  [['stable/tablet/tablet-v2.xml', 'unstable/tablet/tablet-unstable-v2.xml'], 'zwp_tablet_manager_v2'],
  [['unstable/xdg-output/xdg-output-unstable-v1.xml'], 'zxdg_output_manager_v1'],
  [['unstable/xdg-foreign/xdg-foreign-unstable-v2.xml'], 'zxdg_exporter_v2'],
  [['staging/xdg-activation/xdg-activation-v1.xml'], 'xdg_activation_v1'],
  [['staging/fractional-scale/fractional-scale-v1.xml'], 'wp_fractional_scale_manager_v1'],
  [['staging/single-pixel-buffer/single-pixel-buffer-v1.xml'], 'wp_single_pixel_buffer_manager_v1'],
  [['staging/cursor-shape/cursor-shape-v1.xml'], 'wp_cursor_shape_manager_v1'],
  [['staging/content-type/content-type-v1.xml'], 'wp_content_type_manager_v1'],
  [['staging/xdg-dialog/xdg-dialog-v1.xml'], 'xdg_dialog_v1'],
  [['staging/xdg-system-bell/xdg-system-bell-v1.xml'], 'xdg_system_bell_v1'],
  [['staging/xdg-toplevel-icon/xdg-toplevel-icon-v1.xml'], 'xdg_toplevel_icon_manager_v1'],
]

# wl_subcompositor comes from the core protocol, already linked.
probe_externs = []
probe_rows = ['\t&wl_subcompositor_interface,']
probe_missing = []
foreach p : probe_protocols
  xml = ''
  foreach candidate : p[0]
    if xml == '' and fs.exists(wl_protocol_dir / candidate)
      xml = wl_protocol_dir / candidate
    endif
  endforeach
  if xml == ''
    probe_missing += p[1]
    continue
  endif
  # `probe-` prefixed output names: these are separate custom_targets from the
  # ones above and Meson requires distinct output filenames.
  protocol_headers += custom_target(
    'probe-' + p[1] + '-server-header',
    input: xml,
    output: 'probe-' + p[1] + '-protocol.h',
    command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
  )
  protocol_sources += custom_target(
    'probe-' + p[1] + '-private-code',
    input: xml,
    output: 'probe-' + p[1] + '-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
  probe_externs += 'extern const struct wl_interface @0@_interface;'.format(p[1])
  probe_rows += '\t&@0@_interface,'.format(p[1])
endforeach

if probe_missing.length() > 0
  # Not an error: a machine one wayland-protocols release behind still builds
  # a working shim, it just probes a smaller catalogue -- and the catalogue
  # size travels in the report, so nobody reads a short run as a complete one.
  #
  # warning(), not message(): a dropped row silently narrows what
  # firefox_bringup.sh check (D) can possibly observe, and message() is
  # invisible in a normal ninja build. The acceptance script also asserts the
  # catalogue size outright (EXPECT_CATALOGUE), so a short catalogue fails the
  # test rather than quietly weakening it -- this warning is what tells the
  # operator WHY before they get there.
  warning('probe catalogue: no XML found for ' + ', '.join(probe_missing) +
    ' -- these will NOT be probed by --probe-globals, so firefox_bringup.sh' +
    ' check (D) will fail its EXPECT_CATALOGUE assertion')
endif

probe_catalogue_h = configure_file(
  output: 'probe-catalogue.h',
  configuration: {
    'VITRIN_PROBE_EXTERNS': '\n'.join(probe_externs),
    'VITRIN_PROBE_ROWS': '\n'.join(probe_rows),
  },
  input: 'src/probe-catalogue.h.in',
)

# Client-side xdg-shell stubs, for the acceptance client below (which is a
# Wayland CLIENT of the shim, so it needs those, not the server ones).
xdg_shell_client_header = custom_target(
  'xdg-shell-client-header',
  input: wl_protocol_dir / 'stable/xdg-shell/xdg-shell.xml',
  output: 'xdg-shell-client-protocol.h',
  command: [wayland_scanner, 'client-header', '@INPUT@', '@OUTPUT@'],
)

inc = include_directories('include')
shim_deps = [wlroots, wayland_server, pixman, xkbcommon, drm]

executable(
  'vitrin-shim',
  files(
    'src/main.c',
    'src/server.c',
    'src/globals.c',
    'src/output.c',
    'src/xdg.c',
    'src/seat.c',
    'src/wire.c',
    'src/upstream.c',
    'src/ledger.c',
  ),
  protocol_sources,
  protocol_headers,
  probe_catalogue_h,
  include_directories: inc,
  dependencies: shim_deps,
  install: true,
)

# --- P1.9.4 advisory WLCS integration (issue #47) -----------------------
# Builds shim/wlcs/integration.c -- together with every phase file the real
# `vitrin-shim` executable above uses, except main.c (argv parsing, app
# spawn and the core link are not needed in this standalone-only mode) --
# into a WLCS-loadable plugin. This is the SAME bring-up/teardown code the
# real shim runs, not a second compositor written for the occasion. See
# shim/wlcs/README.md for what this proves, what it does not, and the
# GPL-3.0-only license boundary this option draws around shim/wlcs/ alone
# (the rest of this file, and everything it builds, stays MIT).
#
# 'auto' (the default): silently skipped when the `wlcs` pkg-config
# dependency is not found, so an ordinary shim build is unaffected. The CI
# job that runs this (advisory, never gating a PR -- see .github/workflows/
# ci.yml's wlcs-advisory job) passes -Dwlcs=enabled, turning a missing wlcs
# package into a loud configure error instead of a silent skip.
wlcs_dep = dependency('wlcs', required: false)
wlcs_opt = get_option('wlcs')
if wlcs_opt.enabled() and not wlcs_dep.found()
  error('wlcs option is "enabled" but the wlcs pkg-config dependency was ' +
    'not found -- install the wlcs package (Ubuntu/Debian: `wlcs`) or ' +
    'pass -Dwlcs=disabled')
endif
if wlcs_opt.enabled() or (wlcs_opt.auto() and wlcs_dep.found())
  wlcs_module = shared_module(
    'vitrin-shim-wlcs',
    files(
      'wlcs/integration.c',
      'src/server.c',
      'src/globals.c',
      'src/output.c',
      'src/xdg.c',
      'src/seat.c',
      'src/wire.c',
      'src/upstream.c',
      'src/ledger.c',
    ),
    protocol_sources,
    protocol_headers,
    probe_catalogue_h,
    include_directories: inc,
    dependencies: shim_deps + [wlcs_dep, dependency('threads')],
    # Never installed and never linked into vitrin-shim: a dev/CI-only test
    # harness artifact, and the one GPL-3.0-only target in this file.
    install: false,
    name_prefix: '',
  )
else
  message('wlcs option: skipped (the wlcs pkg-config dependency was not ' +
    'found; pass -Dwlcs=enabled to turn that into a configure error)')
endif

# Compile-proof of the checked-in generated wire header (mirrors the CI check
# in the Cargo workspace; runs standalone here too).
header_test = executable(
  'test-header-compiles',
  files('tests/test_header_compiles.c'),
  include_directories: inc,
)
test('header-compiles', header_test)

# --- P1.6.2 acceptance harness -----------------------------------------
# The core side of the shim wire, in C. It deliberately links NOTHING but
# libc and the generated header: the shim's CI job must not pull a Rust
# toolchain onto PATH (shim/ci/install-deps.sh), so the upstream-link proof
# has to be able to stand entirely on its own.
mock_core = executable(
  'mock-core',
  files('tests/mock_core.c'),
  include_directories: inc,
)

# A deterministic Wayland client: known damage rectangles and a frame counter,
# which is what turns "damage-only updates" and "paced by the core" into
# mechanical assertions rather than eyeballed logs.
wayland_client = dependency('wayland-client')
damage_client = executable(
  'damage-client',
  files('tests/damage_client.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client],
)

# The P1.8.5 real-app capture rung's app (issue #107): a bare wl_shm client
# that paints one known solid colour over the whole surface and never
# animates. That is what the M1.3 exit gate captures through the real chain to
# assert (a) the agent frame's dominant colour equals the served colour, and
# (b) it agrees by SSIM with the core-internal capture -- a static, toolkit-free
# scene neither weston-terminal nor gtk-entry-probe can be. Built beside
# vitrin-shim so the integration gate resolves it as a sibling of the shim
# binary, exactly as it resolves gtk-entry-probe.
solid_client = executable(
  'solid-client',
  files('tests/solid_client.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client],
)

# The P1.8.6 agent-actuation gate's D10 app (issue #108): a wl_shm + wl_pointer
# client with a locatable TARGET square that repaints the whole surface to a
# HIT colour when a click lands on it. That whole-frame colour flip is the
# clean, GPU-free "the click landed on the observed feature" response the
# agent's own observe() asserts -- neither weston-terminal nor gtk-entry-probe
# gives one. Co-built with vitrin-shim so the gate resolves it as a sibling.
click_target = executable(
  'click-target',
  files('tests/click_target.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client],
)

# --- P1.6.3 acceptance harness -----------------------------------------
# The input half of the wire, from the app's side: a client that receives
# seat input and resolves every key through the keymap the shim sent it,
# with xkbcommon, exactly as a real toolkit does. That is what makes the
# dynamic keymap (D7) testable rather than merely plausible -- and "no
# duplicates" is a claim only the receiving end can settle.
input_echo_client = executable(
  'input-echo-client',
  files('tests/input_echo_client.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client, xkbcommon],
)

# The R5 gate's app: a real GTK text entry, so "héllo→世界 arrives intact in a
# GTK text field" is measured through a real input-method stack rather than
# inferred from a keysym stream. OPTIONAL on purpose -- the shim's CI job
# must stay a small C/wlroots job and must not grow a GTK dependency to
# build (shim/ci/install-deps.sh), so the probe is built where GTK happens
# to be present and the acceptance script skips the gate, loudly, where it
# is not. GTK3 first: its GtkEntry is the widget in most of the legacy apps
# this shim exists to confine.
gtk3 = dependency('gtk+-3.0', required: false)
gtk4 = dependency('gtk4', required: false)
gtk_entry_probe = disabler()
if gtk3.found()
  gtk_entry_probe = executable(
    'gtk-entry-probe',
    files('tests/gtk_entry_probe.c'),
    dependencies: [gtk3],
  )
elif gtk4.found()
  gtk_entry_probe = executable(
    'gtk-entry-probe',
    files('tests/gtk_entry_probe.c'),
    dependencies: [gtk4],
  )
else
  message('GTK not found: the R5 text-field gate will report SKIP, not PASS')
endif
