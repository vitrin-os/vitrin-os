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
foreach p : protocols
  name = p[0]
  xml = wl_protocol_dir / p[1]
  protocol_headers += custom_target(
    name + '-server-header',
    input: xml,
    output: name + '-protocol.h',
    command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
  )
  protocol_sources += custom_target(
    name + '-private-code',
    input: xml,
    output: name + '-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
endforeach

inc = include_directories('include')
shim_deps = [wlroots, wayland_server, pixman, xkbcommon, drm]

executable(
  'vitrin-shim',
  files(
    'src/main.c',
    'src/server.c',
    'src/globals.c',
    'src/output.c',
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
