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
  ),
  protocol_sources,
  protocol_headers,
  include_directories: inc,
  dependencies: shim_deps,
  install: true,
)

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
