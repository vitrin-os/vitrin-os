project(
  'vitrin-shim',
  'c',
  version: '0.1.0',
  # D-005 / issue #133: the shim's own sources are MPL-2.0; the one generated
  # header (include/vitrin-protocol.h) is Apache-2.0, transliterated from
  # protocol/vitrin-v0.xml. Per-file SPDX headers are normative; the root
  # NOTICE is the full map. The GPL-3.0-only carve-out (shim/wlcs/) is
  # deliberately NOT listed here: it is never built by default, never
  # installed and never linked into vitrin-shim, so declaring it would tell
  # `meson introspect` consumers the shim is GPL-encumbered when it is not.
  # See shim/wlcs/README.md and meson_options.txt.
  license: ['MPL-2.0', 'Apache-2.0'],
  meson_version: '>=1.3',
  default_options: [
    'c_std=c11',
    'warning_level=2',
    'werror=true',
    # ISSUE #283: anything this build VENDORS is linked INTO the shim, never
    # loaded from beside it. Subprojects inherit this value, so it covers the
    # pinned wlroots fallback below and every nested wrap wlroots itself might
    # resolve.
    #
    # The defect it removes is not a build-tree nicety. A confined realm
    # (`--isolation=default`) binds the shim as a single file at a fixed
    # in-realm path, and Meson gives an executable that links a vendored
    # SHARED library the RUNPATH `$ORIGIN/subprojects/<name>`. `$ORIGIN` is
    # the directory the binary is EXECUTED from, so inside the realm it is
    # the bind target's directory -- a path the realm's mount table never
    # creates. The dynamic loader then kills the shim before `main`, and the
    # core sees only the config channel's EOF and reports `Broken pipe`. CI
    # always hits this, because Ubuntu ships wlroots 0.17.1 and D11 pins
    # 0.19, so the vendored fallback is the CI configuration.
    #
    # Chosen over the alternative -- have the realm's mount table follow the
    # shim's own `ldd` closure and forward `LD_LIBRARY_PATH` -- because that
    # makes the width of a confinement boundary a function of a build flag.
    # This keeps the mount table fixed and auditable, which is the property
    # D-037(3) rests on. See docs/plan/20-decision-log.md D-043.
    #
    # HELD BY A TEST, not by this comment: `meson test loader-independence`
    # copies the built shim to an empty directory, runs it there with a
    # scrubbed environment, and fails if its interpreter or anything it needs
    # falls outside the prefixes a realm mirrors (that script's
    # REALM_LIB_PREFIXES). Overriding this option on the command line
    # (`-Ddefault_library=shared`) is therefore loud, not silent.
    'default_library=static',
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
  if name == 'xdg-decoration'
    xdg_decoration_code = code
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

# --- WS-E.4.2: the one probe interface that became real ------------------
# `wlr_pointer_constraints_v1.h` includes the CANONICALLY named generated
# header ("pointer-constraints-unstable-v1-protocol.h") for
# `enum zwp_pointer_constraints_v1_lifetime`, so src/constraint.c cannot
# compile without it. The probe loop above emits the same XML under a
# `probe-` prefixed name (Meson requires distinct output filenames), which
# wlroots' `#include` cannot find.
#
# SERVER HEADER ONLY, deliberately: the marshalling tables for this interface
# are already compiled and linked from `probe-zwp_pointer_constraints_v1-
# protocol.c` above, and a second `private-code` for the same XML would be a
# duplicate definition of `zwp_pointer_constraints_v1_interface` at link time.
# wayland-scanner derives a header's include guard from the protocol name, so
# both headers carry the SAME guard and including both is a no-op.
#
# The row stays in `probe_protocols` even though the interface is now in the
# v0 contract, exactly as `zwp_relative_pointer_manager_v1` and
# `zwp_pointer_gestures_v1` do: `ledger.c`'s `in_v0_contract` is what declines
# to arm a probe for it at runtime (never shadowing a real global with an
# inert one), and the catalogue size is a number a bring-up run asserts.
protocol_headers += custom_target(
  'pointer-constraints-server-header',
  input: wl_protocol_dir / 'unstable/pointer-constraints/pointer-constraints-unstable-v1.xml',
  output: 'pointer-constraints-unstable-v1-protocol.h',
  command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
)

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

# Client-side xdg-decoration stubs, for the conformance client's FACT 4: the
# shim's decoration reply is scheduled relative to the initial commit, and
# only the client end can observe that ordering.
xdg_decoration_client_header = custom_target(
  'xdg-decoration-client-header',
  input: wl_protocol_dir / 'unstable/xdg-decoration/xdg-decoration-unstable-v1.xml',
  output: 'xdg-decoration-unstable-v1-client-protocol.h',
  command: [wayland_scanner, 'client-header', '@INPUT@', '@OUTPUT@'],
)

# Client-side stubs for the three pointer-side extensions WS-E.4.2 (#222)
# made real, for `gesture_probe.c` -- which is a Wayland CLIENT of the shim,
# so it needs the client half of each, and the marshalling tables besides.
#
# Its own `private-code`, not the probe loop's. The probe targets above emit
# the same XML under `probe-` prefixed names and are linked into `vitrin-shim`;
# a client executable links only what it lists, so a second copy here is a
# second object file, never a duplicate symbol. Sharing them would instead
# couple this client to a catalogue whose whole point is that rows can be
# dropped when a machine's wayland-protocols is a release behind -- and a
# dropped row would then be a client that fails to LINK rather than one that
# reports a missing global, which is the failure this client is written to
# report clearly.
# EXISTENCE-CHECKED, on `probe_protocols`' precedent above and for the reason
# stated there: a bare `wl_protocol_dir / 'unstable/...'` makes a moved XML a
# Meson *missing input* error, and that error takes the whole subproject down
# -- `vitrin-shim` itself would stop configuring because a test CLIENT wanted a
# file that moved, which is strictly worse than not having the client. A row
# that resolves to nothing therefore drops `gesture-probe` with a warning
# naming the protocol; the shim still builds, and the operator is told why
# before `test_real_gestures.py` fails with "no gesture-probe beside the C
# shim".
#
# No candidate LIST here, unlike `probe_protocols` -- deliberately, and it is
# the honest shape. A promotion of one of these to `stable/` renames its
# interfaces (`zwp_` -> `wp_`), so a stable copy would not satisfy
# `gesture_probe.c`'s `#include`s or its symbol references anyway: falling back
# to one would trade a clear configure-time warning for a confusing compile
# error. What the check buys is the failure MODE, not a second source.
client_protocols = [
  ['relative-pointer-unstable-v1', 'unstable/relative-pointer/relative-pointer-unstable-v1.xml'],
  ['pointer-gestures-unstable-v1', 'unstable/pointer-gestures/pointer-gestures-unstable-v1.xml'],
  ['pointer-constraints-unstable-v1', 'unstable/pointer-constraints/pointer-constraints-unstable-v1.xml'],
]
pointer_client_headers = []
pointer_client_code = []
client_protocols_missing = []
foreach p : client_protocols
  if not fs.exists(wl_protocol_dir / p[1])
    client_protocols_missing += p[0]
    continue
  endif
  xml = wl_protocol_dir / p[1]
  pointer_client_headers += custom_target(
    p[0] + '-client-header',
    input: xml,
    output: p[0] + '-client-protocol.h',
    command: [wayland_scanner, 'client-header', '@INPUT@', '@OUTPUT@'],
  )
  pointer_client_code += custom_target(
    p[0] + '-client-private-code',
    input: xml,
    output: p[0] + '-client-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
endforeach

if client_protocols_missing.length() > 0
  warning('gesture-probe: no XML found for ' + ', '.join(client_protocols_missing) +
    ' -- gesture-probe will NOT be built, so tests/integration/test_real_gestures.py' +
    ' will fail with "no gesture-probe beside the C shim". The shim itself is' +
    ' unaffected.')
endif

# --- WS-E.4.4 (#306): idle inhibition, from the app's side ---------------
# `idle-probe` binds `zwp_idle_inhibit_manager_v1` and holds an inhibitor, which
# is the only way to exercise this seam at all: an inhibit is REQUESTED by the
# app, so no injector line and no core-side stimulus can produce one.
#
# Its own resolution check and its own gate, deliberately NOT a row in
# `client_protocols` above: that list gates `gesture-probe`, and coupling the two
# would mean a machine missing one protocol stops building a client that never
# needed it. The same reasoning that list's own comment gives for existence
# checking applies here -- a bare `wl_protocol_dir / ...` makes a moved XML a
# Meson *missing input* error that takes `vitrin-shim` itself down with it.
idle_client_headers = []
idle_client_code = []
idle_inhibit_client_xml = wl_protocol_dir / 'unstable/idle-inhibit/idle-inhibit-unstable-v1.xml'
have_idle_client = fs.exists(idle_inhibit_client_xml)
if have_idle_client
  idle_client_headers += custom_target(
    'idle-inhibit-unstable-v1-client-header',
    input: idle_inhibit_client_xml,
    output: 'idle-inhibit-unstable-v1-client-protocol.h',
    command: [wayland_scanner, 'client-header', '@INPUT@', '@OUTPUT@'],
  )
  # Its own `private-code`, on the pointer clients' precedent above: a client
  # executable links only what it lists, so this is a second object file rather
  # than a duplicate of the probe catalogue's symbol.
  idle_client_code += custom_target(
    'idle-inhibit-unstable-v1-client-private-code',
    input: idle_inhibit_client_xml,
    output: 'idle-inhibit-unstable-v1-client-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
else
  warning('idle-probe: no XML found for unstable/idle-inhibit -- idle-probe will' +
    ' NOT be built and the idle-inhibit acceptance test will not run. The shim' +
    ' itself is unaffected, but note that the same XML feeds the probe catalogue,' +
    ' so a machine missing it also probes one interface fewer.')
endif

inc = include_directories('include')
shim_deps = [wlroots, wayland_server, pixman, xkbcommon, drm]

shim_exe = executable(
  'vitrin-shim',
  files(
    'src/main.c',
    'src/server.c',
    'src/globals.c',
    'src/output.c',
    'src/xdg.c',
    'src/seat.c',
    'src/clipboard.c',
    'src/constraint.c',
    'src/idle.c',
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

# --- The shim must load from a directory holding nothing but itself -------
# Issue #283. A confined realm binds the shim as a lone file at a fixed
# in-realm path, so anything the loader has to find NEXT TO the binary, or
# anywhere else in the build tree, is a shim that dies before `main` inside
# every realm while working perfectly on the developer's box. This test moves
# the binary to an empty directory, empties the environment, and runs it --
# which is the realm's condition without needing a realm.
#
# It is a `meson test` on the xdg-conformance precedent: it needs nothing but
# the shim built here, no wayland-info, no weston, no GTK, no GPU, and it
# finishes in milliseconds.
test(
  'loader-independence',
  find_program('bash'),
  args: [
    files('tests/acceptance/loader_independence.sh'),
    shim_exe,
  ],
  depends: [shim_exe],
  timeout: 30,
)

# --- P1.9.4 advisory WLCS integration (issue #47) -----------------------
# Builds shim/wlcs/integration.c -- together with every phase file the real
# `vitrin-shim` executable above uses, except main.c (argv parsing, app
# spawn and the core link are not needed in this standalone-only mode) --
# into a WLCS-loadable plugin. This is the SAME bring-up/teardown code the
# real shim runs, not a second compositor written for the occasion. See
# shim/wlcs/README.md for what this proves, what it does not, and the
# GPL-3.0-only license boundary this option draws around shim/wlcs/ alone
# (the rest of this file, and everything it builds, is MPL-2.0, except the
# generated include/vitrin-protocol.h, which is Apache-2.0).
#
# LICENSE CONSTRAINT, DO NOT BREAK IT: this target combines the GPL-3.0-only
# shim/wlcs/integration.c with the MPL-2.0 shim sources into one shared
# module. That is lawful only because MPL-2.0 names GPL-3.0 a Secondary
# License (section 1.12) and permits distributing the Larger Work under GPL
# terms (section 3.3) -- which is exactly what MPL's Exhibit B switches off.
# Never add the Exhibit B "Incompatible With Secondary Licenses" notice to
# any file under shim/; doing so makes this module undistributable.
# (include/vitrin-protocol.h is Apache-2.0, which is one-way compatible into
# GPL-3.0, so that leg is clean too.)
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
      'src/clipboard.c',
      'src/constraint.c',
      'src/idle.c',
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

# The WS-E.2.1 cross-realm clipboard gate's app (issue #213): a bare
# wl_shm + xdg-shell + wl_data_device client that owns a selection with a
# KNOWN string and writes whatever selection it is later offered into a file.
# Neither half is reproducibly obtainable from a general-purpose app in CI --
# a terminal cannot be made to put a chosen string on its clipboard without a
# human's mouse, and cannot be made to report what it received at all. Co-built
# with vitrin-shim so the gate resolves it as a sibling, exactly as it resolves
# click-target.
clipboard_peer = executable(
  'clipboard-peer',
  files('tests/clipboard_peer.c'),
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

# The M1.5 demo's headless app (issue #110, goal-directed rewrite): a two-field
# form with a locatable marker colour per field and per button, which
# accumulates typed text into the focused field and, on a click landing on its
# submit button, repaints itself as a receipt -- three full-width bands whose
# colours are a pure function of the whole record it received. That is what
# lets the demo agent prove FROM PIXELS ALONE that the confirmation reflects
# exactly the task values it was handed, rather than that "enough pixels
# moved"; the encoding is normative in examples/agent-demo/README.md. It also
# prints a byte-exact `SUBMIT ... canon=<hex>` line, the out-of-band ground
# truth beside the pixels (gtk-entry-probe's ENTRY_HEX role). Needs xkbcommon
# for the same reason input-echo-client does: resolving the shim's DYNAMICALLY
# GENERATED keymap is the app's job (D7). Co-built with vitrin-shim so the
# demo and its gate resolve it as a sibling, exactly as they resolve
# click-target.
form_target = executable(
  'form-target',
  files('tests/form_target.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client, xkbcommon],
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

# --- WS-E.4.2 (#222): the touchpad classes, from the app's side ---------
# `input-echo-client` witnesses `wl_pointer` and `wl_keyboard`; relative
# motion, gestures and pointer constraints arrive on three OTHER globals, and
# the fact that gate needs is not "what arrived" but "what is this app still
# holding" -- pairing state no existing client keeps. It is also the only way
# to drive a pointer constraint at all: a lock is REQUESTED by the app, so no
# injector line can produce one. Co-built with vitrin-shim so
# `tests/integration/test_real_gestures.py` resolves it as a sibling, exactly
# as the other gates resolve click-target.
#
# Guarded by the same resolution the client-protocol loop above reports: this
# program `#include`s all three generated client headers, so building it
# against a partial set is a compile error rather than a smaller client. The
# warning up there names which protocol went missing; here it is simply not
# built, and `vitrin-shim` is untouched.
if client_protocols_missing.length() == 0
  gesture_probe = executable(
    'gesture-probe',
    files('tests/gesture_probe.c'),
    xdg_shell_client_header,
    xdg_shell_code,
    pointer_client_headers,
    pointer_client_code,
    include_directories: inc,
    dependencies: [wayland_client],
  )
endif

# --- WS-E.4.4 (#306): the idle-inhibit relay, driven from the app's side --
# `idle-probe` is the only way to make an inhibit happen at all (see its header:
# an inhibit is REQUESTED by the app, so nothing core-side can produce one), and
# the acceptance script beside it is a COMPONENT test against `mock_core.c` --
# never milestone acceptance, per CLAUDE.md.
#
# A `meson test` for xdg-conformance's reason: the shim, the mock core and this
# client, all built here, headless, pixman, no GPU, no DRM and no seat device.
if have_idle_client
  idle_probe = executable(
    'idle-probe',
    files('tests/idle_probe.c'),
    idle_client_headers,
    idle_client_code,
    include_directories: inc,
    dependencies: [wayland_client],
  )

  test(
    'idle-inhibit',
    find_program('bash'),
    args: [
      files('tests/acceptance/idle_inhibit.sh'),
    ],
    env: {
      'BUILD_DIR': meson.current_build_dir(),
    },
    depends: [shim_exe, mock_core, idle_probe],
    # Two scenarios, each bounded by the mock core's own --run-ms; the budget is
    # for the shim's bring-up, which the script polls for rather than racing.
    timeout: 60,
  )
endif

# --- xdg-shell conformance, from the app's side ------------------------
# Three facts about src/xdg.c that only a client can observe: the initial
# `xdg_surface.configure` is sent on the client's initial commit and NOT
# before it, `xdg_toplevel.wm_capabilities` precedes that configure carrying
# exactly the capabilities the shim implements, and a POPUP gets its own
# configure on its own initial commit -- without which the app is not merely
# menu-less, it is disconnected. The reasoning for each assertion is in the
# client's own header comment.
#
# This one IS a `meson test`, unlike the scripts beside it in
# tests/acceptance/: it needs nothing but the shim and this client, both
# built here, and runs headless with the pixman software renderer. The
# others need wayland-info, weston, GTK or Firefox and so stay separate CI
# steps that can be skipped when the dependency is absent.
xdg_conformance_client = executable(
  'xdg-conformance-client',
  files('tests/xdg_conformance_client.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  xdg_decoration_client_header,
  xdg_decoration_code,
  include_directories: inc,
  dependencies: [wayland_client],
)

test(
  'xdg-conformance',
  find_program('bash'),
  args: [
    files('tests/acceptance/xdg_conformance.sh'),
    shim_exe,
    xdg_conformance_client,
  ],
  depends: [shim_exe, xdg_conformance_client],
  # Generous: the body is two compositor round trips, but the shim has to
  # come up and bind a socket first, and the script polls for that with its
  # own SOCKET_TIMEOUT rather than racing it.
  timeout: 60,
)

# --- keyboard focus succession between sibling windows -----------------
# Issue #268, found on bare metal: an app that opens a SECOND toplevel (a file
# chooser, a preferences window, any GTK dialog) and closes it again left the
# realm's keyboard focused on nobody, so the surviving window was visible,
# still scrolled under the pointer, and could not be typed into. This client
# maps several toplevels on ONE shim connection, unmaps the one holding the
# keyboard and asserts the `wl_keyboard.enter` comes back naming the right
# survivor, then unmaps the last one too and asserts focus is released -- so
# the fix cannot become "always keep the keyboard somewhere" and aim it at a
# surface about to be freed. It also checks that `xdg_toplevel`'s ACTIVATED
# state names the front window, which is what an app reads to draw a solid
# rather than a hollow text cursor, and it repeats the sequence with an
# `xdg_popup.grab` open: wlroots defers every focus change to an active
# keyboard grab and a menu is one, so that is the single input that makes the
# whole mechanism silently do nothing.
#
# A `meson test` for exactly the reason xdg-conformance is one: the shim plus a
# client built here, headless, pixman, no GPU, no DRM and no seat device -- the
# virtual keyboard is created unconditionally at bring-up (src/globals.c), so
# `--no-upstream` is enough to observe real focus events.
focus_succession_client = executable(
  'focus-succession-client',
  files('tests/focus_succession_client.c'),
  xdg_shell_client_header,
  xdg_shell_code,
  include_directories: inc,
  dependencies: [wayland_client],
)

test(
  'focus-succession',
  find_program('bash'),
  args: [
    files('tests/acceptance/focus_succession.sh'),
    shim_exe,
    focus_succession_client,
  ],
  depends: [shim_exe, focus_succession_client],
  # Same reasoning as xdg-conformance's: the assertions are a handful of round
  # trips; the budget is for the shim's bring-up, which the script polls for.
  timeout: 60,
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
