<!-- GENERATED FILE -- DO NOT EDIT.

Produced by `cargo xtask session-matrix` from the corpus in
crates/xtask/src/session_matrix.rs. `cargo xtask session-matrix --check`
re-renders and compares byte-for-byte, and CI runs it, so a hand edit to
this file is a red build. To change what this page says, change the
measurement -- see the runbook at the bottom.
-->

# Session app matrix

Which applications this project has **actually run**, on **one machine**, at what
**bar**, with the **observable** that was checked named in every cell.

This page is generated, and the generator can only emit a cell somebody executed. An
application nobody ran does not appear as a row here no matter how confident anyone is
that it would work; it appears in [Requested, and not
emitted](#requested-and-not-emitted) instead. That is the whole design: the app set
cannot widen by assertion, only by running something and landing its evidence.

## The machine, the build, and the date

**One machine was measured.** Everything below is one laptop, one Intel iGPU, one
internal panel, one kernel. None of it generalises, and reading this page as "Vitrin
runs these applications" would be false.

- **Inventory read**: 2026-08-10
- **Last recorded run in this corpus**: 2026-08-09 — the second bare-metal DRM run (`docs/drm-bringup.md`)
- **Kernel**: `7.1.6-arch1-1` — **this is not the kernel the bare-metal evidence was taken on.** Both DRM runs ran on `7.1.5-arch1-2` (`docs/drm-bringup.md`); the machine has since moved up one release
- **Mesa**: `1:26.1.6-1`
- **wlroots**: `wlroots0.19 0.19.3-1` — the built shim links `libwlroots-0.19.so` (`readelf -dW shim/build/vitrin-shim`). 0.17 and 0.20 are also installed on this machine and are not what is used
- **`vitrind` revision**: `38978f6` — `v0.1.0-56-g38978f6`, the revision of the tree at the last regeneration. Not a self-reported `vitrind --version`: nothing was launched to produce this page. Individual runs predate it and name their own where one is recorded — the nested lock-screen run recorded core `f9f2b8a`, and the second DRM run followed the three fixes in `cf0e7ff`
- **Machine**: one laptop. Intel iGPU on `/dev/dri/card1` (`i915`) drives the only connected output, `eDP-1`, at 2560x1600 @ 240 Hz, scale 1. A second card (`/dev/dri/card2`, `nvidia`) has every connector disconnected and is not in the display path
- **Host compositor for nested runs**: Hyprland 0.56.2-1, `XDG_SESSION_TYPE=wayland`

**CI cannot produce this page's contents.** A GitHub runner has no DRM device, no seat
and no GPU, so it cannot run `vitrind` against a panel and cannot run a GUI application
at all. What CI *can* do, and does, is assert that the checked-in page is byte-identical
to what the generator emits — which catches a hand edit, and catches nothing else.
The measurements themselves come from a human executing the runbook at the bottom of
this page, on the target machine.

## How to read a cell

### The bar is weak, and here is exactly how weak

Most rows below were scored at one bar: **the application mapped a window and
repainted**. Nothing was typed into it, nothing was clicked, and nothing was checked
for correct rendering.

An application can map a window and repaint and still be **unusable**, because a realm
is missing things a desktop application assumes it has:

- **No cross-realm clipboard through the app's own clipboard interface.** The shim
advertises `wl_data_device_manager`, but see `shim/src/globals.c:217-224`: "THIS GLOBAL
STILL GRANTS NOTHING ACROSS THE REALM BOUNDARY" — a shim serves exactly one client on
exactly one private socket, so both ends of any transfer through it are the same
application. What exists across realms instead is a core-mediated channel driven by two
physical human chords (WS-E.2.1, D-024), reachable by no client at any verb set.
- **No portals.** No file chooser, no screen share, no opening a link.
- **No session bus.** A realm has no session D-Bus of its own, so anything that expects
one degrades or fails.
- **No IME.** Nothing here serves `text-input`/`input-method`, so composing text in any
non-Latin script does not work.
- **No XWayland child processes.** There is no X server anywhere in this stack, so an
application that forks an X11 helper loses that helper.

Where a row was proved by a **named task** instead — an integration gate, a runbook
checklist, an issue's acceptance criterion — the row names that task, and the task is
what the row means. Those rows assert something specific and are much stronger than the
weak bar.

### The three evidence classes on this page

| Class | What it proves | Where it appears |
|---|---|---|
| Named task | The stated assertion held in a run of the shipped chain | `Bar` column reads `named task: ...` |
| Weak bar | The application mapped a window and repainted, and nothing else was checked | `Bar` column reads `weak bar` |
| Linkage | An ELF/`strings` measurement of a binary on the machine. **Not a run.** | The three inventory tables near the bottom |

Linkage is the weakest class here and it has demonstrated false positives **in both
directions** on this very machine — see [What this page does not
measure](#what-this-page-does-not-measure). It is published because the question "what
on this machine actually needs X11" has no better answer that does not require
launching everything, and it is never mixed into the two execution tables.

## Desktop applications executed against `vitrind`

Software a person would daily drive. Every row is a recorded execution against `vitrind`; there are no inferred rows.

| App | Version | Where it ran | Bar | Observable checked | Outcome | Recorded, and where |
|---|---|---|---|---|---|---|
| Firefox ESR | 140.12.0esr, sha256 3323ee13…f433d92 (pinned by this repo) | headless | named task: `tests/integration/test_real_firefox.py` | real `vitrind` execs the real `vitrin-shim`, which execs this pinned Firefox rendering a local `file://` page; the real Python SDK captures a frame through the real enforcement/capture path and asserts its dominant colour is the served `#0000ff`, and that the globals ledger contains nothing outside `shim/docs/firefox-refused-globals.txt`. No mock on any seam | met the bar | every PR in CI since `1ebeee2` (2026-07-22) — `tests/integration/test_real_firefox.py`, `shim/docs/firefox.md` §7 (the M1.2 milestone proof) |
| alacritty | 0.17.0-1 (installed at inventory) | nested under Hyprland | named task: issue #203 acceptance criterion 1 | "a real toolkit terminal (alacritty) runs to completion under `vitrind --nested`" — i.e. a live nested run to completion, after the eager-`set_mode` abort that killed it beforehand | met the bar | 2026-08-06 (the fix landed in `af98130` at 16:38:43 +0900) — issue #203, checked acceptance criterion; commit `af98130` |
| kitty | 0.48.2-1 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| Chromium | 151.0.7922.108-1 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| Visual Studio Code | visual-studio-code-bin 1.131.0-1 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| nautilus | 50.2.2-1 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| gimp | 3.2.4-2 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| inkscape | 1.4.4-4 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| pavucontrol | 1:6.2-1 (installed at inventory) | headless | weak bar | mapped a window and repainted. Nothing was typed into it, clicked, or checked for correct rendering | met the bar | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| xterm | 410-1 (installed at inventory) | headless | weak bar | started in a realm and never mapped: it reported it could not open a display. There is no XWayland anywhere in this stack — not in the core, not in the shim's advertised global set, not as a process the shim ever execs | **did not map** — needs an X server, and there is none | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |
| waybar | 0.15.0-2 (installed at inventory) | headless | weak bar | connected, bound six globals, and never mapped. The shim advertises no `zwlr_layer_shell_v1`, which is the interface a bar maps through | **did not map** — there is no `zwlr_layer_shell_v1`, so it has nothing to map into. **Not an X11 gap**; owned by WS-E Stage 2, which owns layer-shell | **undated**; recorded by `7863702` (2026-08-06) — the same row, plus D-021's cost list in `docs/plan/20-decision-log.md`, which attaches the measurement: "measured: waybar connects, binds six globals and never maps" |
| rofi | 2.0.0-1 (installed at inventory) | headless | weak bar | started in a realm and never mapped, in the same recorded row as waybar | **did not map** — there is no `zwlr_layer_shell_v1`, so it has nothing to map into. **Not an X11 gap**; owned by WS-E Stage 2, which owns layer-shell | **undated**; recorded by `7863702` (2026-08-06) — `docs/plan/14-workstream-session-mode.md` §2 "What already works, measured" |

**What these rows over-claim if read quickly:**

- **Firefox ESR** — this is the pinned Mozilla ESR tarball, not a distro package — Arch ships no `firefox-esr`, and what is installed on the measured machine is `firefox-developer-edition`, which was never run against `vitrind`. The row is about 140.12.0esr and nothing else. The commit that first brought Firefox up in a realm, `cae70f0` (2026-07-20), wired only `firefox_bringup.sh` into CI — under the mock core and declared SKIPPED with `VITRIN_SKIP_FIREFOX_GATE=1` — so it is not this row's provenance.
- **alacritty** — #203 records only `vitrind --nested`; it does not name the host compositor, so the venue here is this machine's host and not something the record states.
- **kitty** — kitty was proved to **crash** before #203's fix — the abort was reproduced with alacritty and with kitty. #203's acceptance criterion names alacritty alone as re-run to completion afterwards, and no record anywhere shows kitty re-run post-fix. So this row is the weak bar and not #203's named task.
- **Visual Studio Code** — the recorded row reads "Electron (VS Code — so also Discord, Slack, Obsidian)". Only VS Code was run; the other three are an inference from a shared runtime and are declined below rather than given rows.
- **xterm** — the repository holds only the fragment `Can't open display`, never the as-emitted line. `libXt`'s format string is `Can't open display: %s`, so the real line carries a colon and the display name; those bytes were not captured and are not reconstructed here.
- **rofi** — the six-globals count is recorded for waybar specifically, in D-021; the recorded row groups rofi with it but attaches no separate measurement. rofi also keeps an X11 path, so it appears in the Wayland-mode inventory table too.

## Repository test clients executed against `vitrind`

Clients this repository wrote. Nobody daily drives them, and **the strongest evidence in the tree is about them** — which is exactly why they are in a separate table. In particular: no desktop application has ever run on bare-metal DRM/KMS. Both bare-metal runs used `solid-client`.

| App | Version | Where it ran | Bar | Observable checked | Outcome | Recorded, and where |
|---|---|---|---|---|---|---|
| weston-terminal | weston 15.0.1-3 on the measured machine — NOT the build the cited CI run used | headless | named task: `tests/integration/test_real_app.py` | real `vitrind` execs the real C shim which fork/execs `weston-terminal`; the application's own frames flow shim → core and are byte-captured, and its identity is read from procfs. The #105 / M1.2 bottom-rung gate, no mock on any seam | met the bar | every PR in CI since `e742f25` (2026-07-22) — `tests/integration/test_real_app.py` (`APP_NAME = "weston-terminal"`) |
| gtk-entry-probe | `shim/tests/gtk_entry_probe.c` at this revision (built from this repo) | headless | named task: `tests/integration/test_real_gtk.py` | a real GTK3 `GtkEntry` client renders a grey toplevel with a white text field headlessly, and its committed frame carries real non-uniform content through the real chain. The gate asserts render, explicitly not input | met the bar | every PR in CI since `1ebeee2` (2026-07-22) — `tests/integration/test_real_gtk.py`, `shim/tests/gtk_entry_probe.c` |
| clipboard-peer | `shim/tests` at this revision (built from this repo) | headless | named task: `tests/integration/test_real_clipboard.py` | two realms, two real `clipboard-peer`s under two real C shims: the offer chord against an empty slot sends nothing, the promote chord still reaches no other realm, and only the second chord in the realm the output moved to produces `offer_selection` — after which the receiving application has the string byte for byte through a real `wl_data_device` transfer | met the bar | every PR in CI since `2f7c7cf` (2026-08-08) — `tests/integration/test_real_clipboard.py`, described in `tests/integration/README.md` (WS-E.2.1, issue #213) |
| solid-client | `shim/tests` at this revision, debug build (built from this repo) | bare-metal DRM/KMS | named task: `docs/drm-bringup.md` observation checklist, first run | first execution of the DRM/KMS backend by anyone: 2560x1600 @ 240 Hz set on `card1`; the client's green square drew; the trusted band drew MIRRORED along the bottom edge; VT switch away and back was **impossible**, so the human could not leave; `vitrind` at 99.1% CPU over a 471 s run | met the bar | 2026-08-09, on kernel 7.1.5-arch1-2 — `docs/drm-bringup.md`, first-run record |
| solid-client | `shim/tests` at this revision, release build (built from this repo) | bare-metal DRM/KMS | named task: `docs/drm-bringup.md` observation checklist, second run | after the first run's three fixes: trusted band at the TOP (the mirror is fixed); consent card drawn on the panel and clicked with a real mouse, petition granted, 13 captures served; held-Esc revocation against a LIVE grant (`dead_man_triggered held_ms=1005 revoked=1`); 5 chorded VT switches honoured; 32.9% CPU release against 99.1% debug; centre pixel read `#55aa00ff` for a realm configured `00aa55`, which is exactly xrgb8888 little-endian | met the bar | 2026-08-09, after the three fixes in `cf0e7ff` — `docs/drm-bringup.md`, second-run record |
| input-echo-client | `shim/tests` at this revision (built from this repo) | nested under Hyprland | named task: `shim/docs/nested-lock-screen.md`, eight-step by-eye checklist | PASS on steps 1–7 with the client resolving keys through xkbcommon as a real toolkit does (`a`/`b`/`c` → `keysym=0x61/0x62/0x63`); a held `Shift` did not postpone the idle lock, and its release while locked was handled | met the bar | 2026-08-09, core `f9f2b8a` — `shim/docs/nested-lock-screen.md`, executed record |

**What these rows over-claim if read quickly:**

- **weston-terminal** — a real third-party Wayland client with a mock-free gate — stronger evidence than any weak-bar row above — but nobody daily drives it, so it says nothing about a desktop. The cited gate runs on `ubuntu-latest` against apt's `weston` (`shim/ci/install-deps.sh`), whose version this page does not record.
- **gtk-entry-probe** — this is a GTK3 **fixture**, not nautilus, gimp or inkscape, and it must not be cited as evidence for those three. No GTK4 or Qt6 fixture exists.
- **clipboard-peer** — the repository's own precedent for how to record a substitution honestly: #213 names alacritty and Firefox, neither of which can be made in CI to put a *known* string on a clipboard without a human's mouse, so a toolkit-free client stands in and the README says so. Not asserted: that a real chord on real hardware produces any of it.
- **solid-client** — `solid-client` commits `wl_shm` buffers and is not a desktop application. No desktop application has ever run on bare metal, and the second run's own open list notes the shim never emits dmabuf, so the zero-copy scanout path is dead code against every real application.
- **solid-client** — still `solid-client`. Three items stayed open after this run: `vitrind`'s own log line renders the connector name empty, the shim never emits dmabuf, and `refresh_view_cache` composes for absent consumers.
- **input-echo-client** — the run found a shipped defect — `TextureKey::current` enumerated every input to `compose_human_visible` except the lock, which is a locked session that looked unlocked. Two further warnings: the page states step 7 is weaker than it reads (`input-echo-client` is static, so every frame carried the identical digest), and the page contradicts itself by also carrying an empty "not yet executed" record block further down. It *was* executed.

## Failures that are not X11 gaps

Issue #221 asks for this section by name, because a reader counting failures would
otherwise fold every one of them into "no X11" and overstate what an X11 shim would
buy.

- **waybar** — there is no `zwlr_layer_shell_v1`, so it has nothing to map into. This is WS-E Stage 2, which owns layer-shell.
- **rofi** — there is no `zwlr_layer_shell_v1`, so it has nothing to map into. This is WS-E Stage 2, which owns layer-shell.

**Issue #203 is closed and fixed on `main`, and is described here in the past tense.**
It belongs in this section because it removed applications from the measurable set for a
reason that had nothing to do with X11: `on_new_deco` answered an `xdg-decoration`
request eagerly, and `wlr_xdg_toplevel_decoration_v1_set_mode` schedules a configure
whose `wlr_xdg_surface_schedule_configure` asserts `surface->initialized` — which is
correctly false at `new_toplevel_decoration` time, because xdg-decoration requires the
decoration object to be created *before* the surface's first commit. Every
decoration-aware client therefore aborted the shim at startup. The fix defers the mode
reply to the initial commit; `shim/src/globals.c:55-77` carries the reasoning, and
`shim/tests/xdg_conformance_client.c`'s FACT 4 is the regression test. Firefox never
binds `zxdg_decoration_manager_v1` at all, which is the only reason the entire
acceptance suite missed it.

## The machine's X11-only software (linkage, not execution)

Measured on this machine by ELF linkage, **not** by running anything. These are the applications that would need Phase 3 **E3.2** (per-app rootless X server with an embedded window manager) before they could run in a realm at all.

Two entries are here for completeness and are **not** an argument for E3.2: `picom` and `openbox` are an X11 compositor and an X11 window manager, so they are X11-only by definition rather than for want of a port, and running either inside a per-app rootless X server is not a thing anyone wants.

**The interim, stated as what it is.** Until E3.2 lands, the owner keeps a second session for the software in this table. So "I did not have to reboot into Hyprland" is **false for this named set**, and the claim must not be made without that carve-out. This is a workaround the owner accepts as a cost, not a mitigation the project offers: nothing in this stack confines that second session, it runs another compositor with full access to the same devices, and switching to it leaves the confined world entirely. See [Where this is honest about its limits](limits.md).

| Binary or app | Version | Wayland mode | What was measured |
|---|---|---|---|
| xterm | 410-1 (installed at inventory) | none exists — there is no Wayland port of xterm | `readelf -dW /usr/bin/xterm` → `libXaw.so.7`, `libXt.so.6`, `libX11.so.6`, `libXmu.so.6`, `libXext.so.6`, `libICE.so.6` and no Wayland library. `strings -a` matching any of `libwayland-client`, `wl_compositor` or `wayland` → 0 hits |
| feh | 3.12.2-1 (installed at inventory) | none — raw Xlib plus imlib2, so there is no toolkit backend to switch | `readelf -dW /usr/bin/feh` → `libX11.so.6`, `libXinerama.so.1`, `libImlib2.so.1`; no Wayland library in the transitive closure. `strings -a` wayland matches → 0 |
| nvidia-settings | 610.57.04-1 (installed at inventory) | none — `libXxf86vm` is an X-server-only extension with no Wayland equivalent | `readelf -dW` → exactly `libXxf86vm.so.1`, `libjansson.so.4`, `libX11.so.6`, `libXext.so.6`, `libm.so.6`, `libc.so.6`. It **does** carry nine case-insensitive `wayland` strings, and they are a trap: every one is an NVIDIA probe symbol — `wconn_get_wayland_display`, `wconn_get_wayland_output_info`, `libnvidia-wayland-client.so.610.57.04`, `Wayland Connector Library failed to connect.` — for querying outputs under a Wayland session, not a GUI backend. A grep-only method misclassifies this one |
| dmenu | 5.4-1 (installed at inventory) | none — suckless raw-Xlib launcher | `readelf -dW /usr/bin/dmenu` → `libX11.so.6`, `libXinerama.so.1`, `libXft.so.2`, `libfontconfig.so.1`, `libc.so.6` — no Wayland library, and `strings -a` finds no Wayland name to dlopen either. Note it would be unusable for **two** independent reasons: it is X11-only *and* it is a launcher, which is the layer-shell gap above |
| slock | 1.7-1 (installed at inventory) | none, and none could exist — it locks by grabbing the X server | `readelf -dW /usr/bin/slock` → `libX11.so.6`, `libXrandr.so.2`, `libcrypt.so.2`, `libc.so.6`; wayland strings → 0 |
| polybar | 3.7.2-2 (installed at inventory) | none — EWMH and ICCCM are X11 window-manager protocols with no Wayland analogue | `readelf -dW /usr/bin/polybar` → `libxcb-ewmh.so.2`, `libxcb-icccm.so.4`, `libxcb-randr.so.0`, `libxcb-xkb.so.1`, `libxcb-cursor.so.0` and the rest of the xcb set; wayland strings → 0 |
| lemonbar | lemonbar-git v1.5.r2.g59b0d28-1 (installed at inventory) | none — raw xcb bar | `readelf -dW /usr/bin/lemonbar` → `libxcb.so.1`, `libxcb-randr.so.0`; wayland strings → 0 |
| picom | picom-git 2855_12.197.g6d676824_2026.06.02-1 (installed at inventory) | none, by definition — it **is** an X11 compositing manager | `readelf -dW /usr/bin/picom` → `libxcb-composite.so.0`, `libxcb-damage.so.0`, `libxcb-glx.so.0`, `libxcb-present.so.0`, `libxcb-xfixes.so.0`, `libX11-xcb.so.1`; wayland strings → 0. **Not an E3.2 requirement**: E3.2 gives each X application its own rootless X server, which is not a thing to run a compositor inside |
| openbox | 3.6.1-14 (installed at inventory) | none, by definition — it **is** an X11 window manager | `readelf -dW /usr/bin/openbox` → `libX11.so.6`, `libXcursor.so.1`, `libXinerama.so.1`, `libXrandr.so.2`, `libXext.so.6`, `libSM.so.6`, `libICE.so.6`; the 64-soname transitive closure contains no `libwayland-*` and wayland strings → 0. **Not an E3.2 requirement**, for the same reason as picom |
| xsel | 1.2.1-2 (installed at inventory) | none — an Xlib selection client | `readelf -dW /usr/bin/xsel` → `libX11.so.6`, `libc.so.6`, and nothing else; the 6-soname closure contains no `libwayland-*`. Directly relevant to this workstream: the cross-realm clipboard (D-024) is the project's answer to this class and `xsel` cannot participate in it. `xdotool` and `xclip` are **not installed** on this machine |
| OpenJDK desktop AWT/Swing (the system JVM) | jdk-openjdk 26.0.2.u10-1 (installed at inventory) | none — the system JVM ships no Wayland AWT backend | `find /usr/lib/jvm -name 'libawt*.so'` → exactly `libawt.so`, `libawt_xawt.so`, `libawt_headless.so`, and nothing else. `libawt_xawt.so`'s closure is `libX11.so.6`, `libXext.so.6`, `libXi.so.6`, `libXrender.so.1`, `libXtst.so.6`, `libxcb.so.1` with no Wayland library. A capability statement about the runtime; no Swing application was launched |

## Software with a Wayland mode, so not an X11 dependency (linkage, not execution)

Measured on this machine by ELF linkage and `strings`, **not** by running anything. These are **not** X11 dependencies: each carries a Wayland path, selected by the switch in the third column. Counting any of them toward the X11 gap would overstate it.

| Binary or app | Version | Wayland mode | What was measured |
|---|---|---|---|
| Chromium | 151.0.7922.108-1 (installed at inventory) | `--ozone-platform=wayland`, or `--ozone-platform-hint=auto` | linkage alone would have called this X11-only and been **wrong**. The 89-soname `DT_NEEDED` closure carries `libX11.so.6` and **no `libwayland-*` at all**, because Chromium statically links its own libwayland from `third_party`. What settles it is `strings -a /usr/lib/chromium/chromium`, which yields the flag `ozone-platform` and the Wayland wire-protocol interface names `wl_compositor` and `xdg_wm_base`. Corroborated by its weak-bar execution row above |
| Visual Studio Code | visual-studio-code-bin 1.131.0-1 (installed at inventory) | `--ozone-platform-hint=auto`, already configured by the owner | the strongest non-execution evidence on this page, because the owner has already configured it: `~/.config/code-flags.conf` exists and contains, verbatim, `--ozone-platform-hint=auto` and `--enable-wayland-ime` under the comment "Native Wayland + text-input-v3 so fcitx5 works without GTK_IM_MODULE". Linkage is weaker and must be read carefully: `/usr/share/code/code` links **no** `libwayland-*` directly; its 95-soname closure reaches all three only through `libgtk-3.so.0`, which is GTK's Wayland and not Electron's. The binary's own 5 ozone/Wayland strings are what say the ozone machinery is compiled in |
| Electron runtime (and the Electron applications on it) | electron43 43.3.0-1, plus ten other runtimes (installed at inventory) | `--ozone-platform=wayland` / `--ozone-platform-hint=auto` | `/usr/lib/electron43/electron` links **no** `libwayland-*` directly — its 103-soname closure reaches `libwayland-client.so.0`, `libwayland-cursor.so.0` and `libwayland-egl.so.1` only through `libgtk-3.so.0`. What says the ozone machinery is present is `strings -a`, which matches `ozone-platform`/`wl_compositor`/`libwayland-client` 5 times. `/usr/lib/slack/slack` measures identically (95-soname closure, no direct Wayland linkage). `discord 1:1.0.152-1`, `obsidian 1.13.4-2` and `element-desktop 1.12.23-1` are installed but their real binaries could not be resolved from their launchers without executing them, so they are asserted by runtime family and **not measured individually** — which is exactly why they are declined rather than given rows |
| alacritty | 0.17.0-1 (installed at inventory) | native — prefers Wayland when `WAYLAND_DISPLAY` is set, falls back to X11 | the `DT_NEEDED` closure carries **neither** `libX11` nor `libwayland-client`: everything is dlopened. `strings -a /usr/bin/alacritty` yields both `libwayland-client.so.0` and `libX11.so.6`. Corroborated by the #203 run, which happened under `vitrind --nested`, a stack serving no X11 at all |
| kitty | 0.48.2-1 (installed at inventory) | native, via dlopen | linkage says nothing either way — only 5 sonames in the closure and neither family among them, and `/usr/bin/kitty` is a small launcher so `strings` finds neither name. What places it here is that this repository ran it under `vitrind`, which serves no X11 |
| nautilus, gimp, inkscape (GTK3/GTK4) | 50.2.2-1, 3.2.4-2, 1.4.4-4 (installed at inventory) | `GDK_BACKEND=wayland` — GTK selects Wayland automatically when `WAYLAND_DISPLAY` is set | each carries both families, and each by a different route, which is the point. `/usr/bin/nautilus` links `libwayland-client.so.0` **directly** (145-soname closure, which adds cursor and egl). `/usr/bin/gimp-3.2` links none directly; its 115-soname closure reaches all three through GTK. `/usr/bin/inkscape` is a 19-soname launcher stub (7 direct `NEEDED` entries) whose GUI lives in `/usr/lib/inkscape/libinkscape_base.so`, a 147-soname closure carrying all three. GTK carries both backends in one library and picks at run time — which is exactly why `shim/docs/firefox.md` sets `GDK_BACKEND=wayland` explicitly, so a stray `DISPLAY` cannot silently drop the browser onto X11 |
| blender | 17:5.2.0-4 (installed at inventory) | native (`GHOST_Wayland`), via dlopen | a second case where linkage alone is **wrong**: the 266-soname closure carries `libX11.so.6` and `libX11-xcb.so.1` and **no `libwayland-*` at all**, yet `strings -a /usr/bin/blender` matches `libwayland-client`, `wl_compositor` or `GHOST_Wayland` 3 times — Blender dlopens its Wayland backend |
| scrcpy | 4.1-2, on sdl3 3.4.14-1 (installed at inventory) | native via SDL3, which dlopens `libwayland-client.so.0` and `libdecor-0.so.0` | the clearest false positive in the scan. scrcpy's direct `NEEDED` is `libavformat`, `libavcodec`, `libavutil`, `libswresample`, `libSDL3.so.0`, `libavdevice`, `libusb-1.0` — so the X11 in its 163-soname closure arrives through `libavdevice`, which is ffmpeg's x11grab **capture input**, not its GUI. Its GUI is SDL3, and `libSDL3.so.0` has a 3-soname closure with no hard X11 and no hard Wayland while its strings carry `libwayland-client.so.0`, `libdecor-0.so.0` and `wl_compositor` |
| openrgb | 1.0rc3-1 (installed at inventory) | `QT_QPA_PLATFORM=wayland` | `readelf -dW /usr/bin/openrgb` → `libQt5Widgets.so.5`, `libQt5Gui.so.5` and nothing else relevant: Qt loads its platform plugin **by name** at run time, which is why the binary shows zero wayland strings. The plugin is installed (`qt5-wayland 5.15.19+kde+r55-1`; `libqwayland-egl.so` and `libqwayland-generic.so` are present) |
| rpi-imager | 2.0.9-1 (installed at inventory) | `QT_QPA_PLATFORM=wayland` | Qt6 Quick application (`libQt6Quick.so.6`, `libQt6Gui.so.6`); `qt6-wayland 6.11.1-1` is installed and `libqwayland.so` sits beside `libqxcb.so` in the Qt6 platform plugin directory |
| fcitx5-config-qt | fcitx5-configtool 5.1.14-1 (installed at inventory) | `QT_QPA_PLATFORM=wayland` | Qt6 (`libQt6Widgets.so.6`, `libQt6Gui.so.6`) with `qt6-wayland` installed and `libqwayland.so` present |
| Android Studio (JetBrains Runtime) | android-studio 2026.1.3.7-1, JBR 25.0.2 (installed at inventory) | a native Wayland AWT backend in the bundled runtime | `find /opt/android-studio -name 'libawt_*.so'` → `libawt_xawt.so`, `libawt_headless.so` **and `libawt_wlawt.so`**, against a system JDK that ships no such file. A genuine split worth recording, because it contradicts the system-JDK row above. Not launched, and it was not verified that the backend is enabled by default |
| gamescope | 3.16.25-1 (installed at inventory) | native — it is itself a Wayland compositor, and a Wayland client when nested | links both families directly: a 56-soname closure with `libwayland-client.so.0` and `libwayland-server.so.0` beside `libX11.so.6`, `libICE.so.6` and `libSM.so.6`. Named here for two reasons: it is the one piece of the Steam stack on this machine that speaks Wayland natively, and it is the prior art `docs/plan/03-phase-3-network-x11-fleet.md` cites for E3.2's per-app-Xwayland design |
| waybar, rofi | 0.15.0-2, 2.0.0-1 (installed at inventory) | native — both link `libwayland-client` directly | waybar links `libwayland-client.so.0` **directly**; its 108-soname closure adds cursor and egl, and its X11 entries are GTK's. rofi links `libwayland-client.so.0` and `libwayland-cursor.so.0` directly while keeping its whole xcb path (`libxcb-ewmh`, `libxcb-icccm`, `libxcb-randr`, `libxcb-cursor`) in a 60-soname closure, so it classifies as **both**. Listed here so their failure above is never misfiled as an X11 gap — it is layer-shell. `wofi` is **not installed**, so its cell cannot be regenerated on this machine at all |

## Where linkage did not settle the question (linkage, not execution)

Linkage contradicted itself and only a run settles these. They are published as explicit unknowns rather than guessed into one of the tables above.

**Games are recorded here as a measured dependency of this machine and as nothing else.** E3.2's exit criteria (`docs/plan/03-phase-3-network-x11-fleet.md` §E3.2) say nothing about games; games additionally need relative pointer, pointer constraints, gamepads and GPU features far beyond E3.2. This row is a measurement, not a commitment, and no schedule anywhere in this repository covers it.

| Binary or app | Version | Wayland mode | What was measured |
|---|---|---|---|
| steamwebhelper (the Steam client UI) | a self-updating CEF build under `~/.local/share/Steam/ubuntu12_64`, dated 2026-07-22; the Arch `steam 1.0.0.87-1` package ships only a shell wrapper and does not describe what runs (vendor-bundled) | **unknown** — the executable carries no Wayland path; the library that draws does. Only a run settles it | the executable's own 36-soname `readelf -dW` closure carries `libX11.so.6`, `libXi.so.6`, `libXrandr.so.2`, `libXcomposite.so.1` and **no** `libwayland-*`, and `strings -a` matches `ozone-platform`/`wl_compositor`/`libwayland-client` **0** times. That reads as settled and is not. Its direct `NEEDED` entries include `libcef.so` (219 444 168 bytes, 2026-07-09) and `libSDL3.so.0`, neither on `ldconfig`'s path, so the walk stops exactly where the answer is: `strings -a` on `libcef.so` matches — `ozone-platform`, `ozone-platform-hint`, `wl_compositor`, `enable-wayland-ime`, `Failed to initialize Wayland platform` — and SDL3 dlopens `libwayland-client.so.0` (see its row). This is `google-chrome`'s case one library deeper. Steam has been run on this machine — `~/.local/share/Steam/steamapps` holds 7 appmanifests, of which exactly one is a game and the rest are Proton and the Steam Linux Runtimes. **No game binary was inspected and no game was run** |
| google-chrome | 151.0.7922.71-1 (installed at inventory) | **unknown** — presumably `--ozone-platform=wayland`, unverified | the contradiction **is** the finding. The 71-soname closure of `/opt/google/chrome/chrome` carries `libX11.so.6`, `libXi.so.6`, `libXrandr.so.2` and **no `libwayland-*` at all** — yet the same binary matches `ozone-platform`/`wl_compositor`/`libwayland-client` 6 times, and Arch's chromium measures **identically** (89 sonames, zero Wayland linkage) while demonstrably having a working `--ozone-platform=wayland`. Linkage cannot settle this one. Only a run can, and nothing was launched |

## Requested, and not emitted

These applications are on the list somebody wants covered, and the generator **refused
to give them a row** because no execution against `vitrind` is recorded for them. They
are named here rather than dropped silently, so the absence is legible.

- **wofi** — named in the recorded failing row, but not installed on the measured machine (`pacman -Q wofi` reports it absent), so no package version can be recorded and the cell cannot be regenerated here even by a live runbook. See the waybar and rofi rows for the same failure.
- **Discord** — the recorded Electron row reads "VS Code — so also Discord, Slack, Obsidian". That parenthetical is an inference from a shared runtime, not a measurement: no record anywhere shows it executed against `vitrind`. See the Electron runtime row in the Wayland-mode table.
- **Slack** — same inferred parenthetical as Discord; never executed against `vitrind`. See the Electron runtime row in the Wayland-mode table.
- **Obsidian** — same inferred parenthetical as Discord; never executed against `vitrind`. See the Electron runtime row in the Wayland-mode table.
- **Steam (steamwebhelper)** — the owner named Steam and games as a real dependency of this machine. Nothing in the Steam stack has ever been executed against `vitrind`, and no schedule in this repository covers games. See its row in the inconclusive table: the client's windowing path came out **unknown**, which is a measurement and not a commitment.
- **google-chrome** — the packaged Google build, distinct from Arch's chromium. Never executed against `vitrind`, and its linkage contradicts itself. See the inconclusive table.

## What this page does not measure

- **Most of the seed rows are undated.** The weak-bar rows carry no log, no recorder
dump and no screenshot; the only bound available is the commit that wrote the record
down, which bounds when the *row was written*, not when the *application was run*. The
`Recorded, and where` column says `undated` where that is the case rather than reusing
a commit date as if it were a run date.
- **The verbatim `xterm` failure line was never captured.** What this repository holds
is the fragment `Can't open display`. The real format string in `libXt` is `Can't open
display: %s`, so the emitted line has the shape `<progname>: <error type>: Can't open
display: <display>` — but the bytes `xterm` actually wrote in that realm are gone, and
they are not reconstructed here. Capturing them is a runbook step.
- **No desktop application has ever run on bare metal.** Both DRM/KMS runs used
`solid-client`. There is a named reason to expect a difference, recorded by the second
run itself: the shim never emits dmabuf, so the zero-copy scanout path is dead code
against every real application.
- **Every inventory row is linkage, not behaviour**, and linkage has demonstrated false
positives in both directions on this machine. Chromium, Blender, scrcpy, OpenRGB and
rpi-imager all classify X11-only by `DT_NEEDED` and all five have Wayland paths
(Chromium statically links its own libwayland; Blender and SDL3 `dlopen` theirs; Qt
loads its platform plugin by name). Alacritty and kitty classify as *neither*, because
they `dlopen` everything. Method, for reproducibility: `readelf -d` walked recursively
with sonames resolved from `ldconfig -p`, plus `strings -a`. Never `ldd` — `ldd` invokes
the dynamic loader and can execute the binary under inspection.
- **That walk does not follow `DT_RUNPATH`, so a private library directory ends it.**
Measured, not theorised: `/usr/bin/inkscape` is a 19-soname launcher stub (7 direct `NEEDED` entries) carrying neither
family, because its entire GUI lives in `/usr/lib/inkscape/libinkscape_base.so` — a path
`ldconfig -p` does not know — whose own closure is 147 sonames and carries all three
Wayland libraries. Any row here whose closure looks implausibly small is this case, and
the fix is to point the walk at the real library.
- **The installed set is not the used set.** A bulk scan of `/usr/bin` on 2026-08-10
classified **494** ELF binaries as X11-only, owned by **114** packages, 37 of them
`xorg-*` and 58 explicitly installed. (Method, so the number is reproducible: every
non-symlink ELF file in `/usr/bin`, transitive `DT_NEEDED` closure via `readelf -d` with
sonames resolved from `ldconfig -p`, counted when the closure contains an X11 soname and
no `libwayland-*`. The dlopen false-positive above applies to this count too, so it is an
upper bound on the X11-only set, not an exact one.) Which of those the owner actually
uses is **not measured here**: no shell history was read, no access times, no launcher
history. The requirement list handed to E3.2 comes from the owner naming what he needs,
never from ranking a `/usr/bin`.
- **`xlsclients` was empty at inventory time, and that means less than it looks.**
XWayland *is* running under the host compositor and the connection genuinely worked
(`xprop -root _NET_SUPPORTING_WM_CHECK` returned a window id), yet `xlsclients` listed
zero X11 clients. That is one instant on one day in a session a few hours old. It is
not evidence that the owner never runs X11 software — the installed set proves he can —
and a truthful version needs sampling over days, which nobody has done.
- **Steam games themselves are entirely unmeasured.** What was measured is
`steamwebhelper`, the client UI. No game binary was inspected: a Proton title's
windowing path runs through Wine inside a container, which no static scan here reaches.
- **Nothing was launched to produce this page.** No `vitrind`, no realm, no nested
compositor, no application under test. Every machine row is an inference from bytes on
disk, and every execution row is a transcription of a run somebody else recorded
earlier. Widening the matrix means executing the runbook below.

## Runbook: regenerate this page on the target machine

**CI has no DRM device, no seat and no GPU, and structurally cannot run this.** It can
only assert that the checked-in page matches a regeneration. Everything below is done
by a human on the target machine.

### 0. Safety, non-negotiable

Run every step of section 2 in a **nested** host window. Never against the DRM/TTY
backend from inside a live session: that takes DRM master and the seat, and kills the
session you are sitting in. Bare-metal runs happen from a spare TTY, with the escape
route rehearsed first — see `docs/drm-bringup.md`.

### 1. Re-take the read-only machine inventory

Nothing here launches anything. Record what these print; they are the header fields and
the three inventory tables.

```bash
uname -srvmo                       # kernel -> header
pacman -Q mesa wlroots0.19         # mesa, wlroots -> header
git -C "$REPO" describe --tags --dirty   # vitrind revision -> header
pacman -Q <app>                    # one per row, for the Version column
xlsclients -l                      # X11 clients live in the current session

# Linkage, per candidate binary. NEVER use ldd: it invokes the dynamic
# loader and can execute the binary under inspection.
readelf -dW /usr/bin/<app> | grep NEEDED
strings -a /usr/bin/<app> | grep -iE 'libwayland-client|wl_compositor|ozone-platform'
```

A binary that shows neither family in `NEEDED` is not evidence of anything: alacritty,
kitty, Blender, SDL3 and every Qt application load their backend at run time. Check
`strings` before concluding, and if the two disagree, the row belongs in the
inconclusive table, not in a guess.

### 2. Run an application in a realm, and record what you saw

Build first:

```bash
cargo build --workspace
meson compile -C shim/build
```

Write a one-realm file naming the application by **absolute** path:

```bash
cat > /tmp/matrix-realm.toml <<'EOF'
[[realm]]
id = "realm-0"
command = "/usr/bin/xterm"
args = []
env_allow = []
EOF
```

`realm.toml` refuses a relative `command`. Then run the core nested, capturing **both**
streams — the failure you are measuring is usually on the application's stderr, not in
the core's log:

```bash
./target/debug/vitrind --nested \
  --realm /tmp/matrix-realm.toml \
  --shim "$PWD/shim/build/vitrin-shim" \
  2>&1 | tee /tmp/matrix-xterm.log
```

Record, for the row:

1. **The observable you checked.** Not "it worked". Either the specific thing a named
task asserts, or — if all you did was look at it — the weak bar, `mapped a window and
repainted`, and nothing more.
2. **The verbatim failure line**, copied out of the log, if it failed. A fragment is not
a quote.
3. **The package version** from step 1, and **the date**.
4. **The cause**, if it failed: was an X server missing, or was it something else? A
failure with a non-X11 cause goes in `Cause::NotAnX11Gap` with its owner named, or the
page will overstate the X11 gap.

### 3. Land the measurement, and regenerate

Edit **`crates/xtask/src/session_matrix.rs`** — never this page. Add the application to
`REQUESTED` if it is not there, then add an `Execution` (or a `Linkage`) carrying the
evidence. The generator refuses a cell whose observable is a bare `pass`/`works`/`ok`,
and refuses an `Execution` for an application that is not in `REQUESTED`.

```bash
cargo xtask session-matrix           # rewrite this page in place
git diff -- docs/book/src/session-app-matrix.md   # review what changed
cargo test -p xtask                  # the generator's own gates
cargo xtask session-matrix --check   # what CI runs; must print "no drift"
```

### 4. If you want a row and cannot get evidence for it

Add it to `REQUESTED` with the reason, and leave it declined. It will appear under
[Requested, and not emitted](#requested-and-not-emitted) with your reason attached,
which is the honest outcome and the one this page is built to make cheap.
