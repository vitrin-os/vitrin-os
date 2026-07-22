//! `cargo xtask`: repo-local developer tooling, aliased via
//! `.cargo/config.toml` (`xtask = "run --package xtask --"`).
//!
//! Subcommands:
//!
//! ```text
//! cargo xtask codegen           Regenerate crates/vitrin-protocol/src/generated
//!                                and shim/include/vitrin-protocol.h from
//!                                protocol/vitrin-v0.xml, in place. This is
//!                                what a human runs after editing the IDL --
//!                                review `git diff` and commit the result.
//!
//! cargo xtask codegen --check    Verify there is no drift between the IDL and
//!                                the checked-in generated files. Never
//!                                writes to the real, checked-in output
//!                                paths at all: it regenerates into an
//!                                isolated scratch directory under `target/`
//!                                and compares the result byte-for-byte
//!                                against the real paths' current on-disk
//!                                content, so the working tree is trivially
//!                                left exactly as found either way it exits.
//!                                This is what CI runs (P1.1.2 acceptance
//!                                criterion 2).
//!
//!                                Deliberately does not use `git diff`/`git
//!                                status` in any form -- comparing against
//!                                `HEAD` or the index is blind to a path
//!                                with no git history at all (as
//!                                crates/vitrin-protocol/src/generated and
//!                                shim/include/vitrin-protocol.h were on the
//!                                branch that introduced this check), so a
//!                                git-based diff would silently report "no
//!                                drift" no matter the content of such a
//!                                path. Direct filesystem comparison has no
//!                                such blind spot: it is correct whether the
//!                                real paths are untracked, staged, or
//!                                committed.
//! ```
//!
//! Calls straight into the `vitrin_scanner` library (`parse`, `rust_gen`,
//! `c_gen`) rather than shelling out to the built `vitrin-scanner` binary --
//! no subprocess, no path-finding, ordinary `anyhow::Result` propagation.

use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Paths this task operates on, relative to the workspace root.
const XML_PATH: &str = "protocol/vitrin-v0.xml";
const RUST_OUT_DIR: &str = "crates/vitrin-protocol/src/generated";
const C_HEADER_PATH: &str = "shim/include/vitrin-protocol.h";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: error: {err:?}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> &'static str {
    "usage: cargo xtask codegen [--check]\n       cargo xtask demo [--headless]"
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(subcommand) = args.first() else {
        bail!("missing subcommand\n\n{}", usage());
    };

    match subcommand.as_str() {
        "codegen" => {
            let mut check = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => check = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'codegen'\n\n{}", usage()),
                }
            }
            codegen(check)
        }
        "demo" => {
            let mut headless = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--headless" => headless = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'demo'\n\n{}", usage()),
                }
            }
            demo(headless)
        }
        "-h" | "--help" => {
            println!("{}", usage());
            Ok(())
        }
        other => bail!("unknown subcommand '{other}'\n\n{}", usage()),
    }
}

/// This crate's own directory (`<repo>/crates/xtask`) is `CARGO_MANIFEST_DIR`,
/// baked in at compile time by Cargo -- resolving the workspace root from it
/// makes this independent of whatever directory `cargo xtask` happened to be
/// invoked from (unlike relying on the process's current directory).
fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../..").canonicalize().with_context(|| {
        format!(
            "resolving workspace root from CARGO_MANIFEST_DIR={}",
            manifest_dir.display()
        )
    })
}

fn codegen(check: bool) -> Result<()> {
    let root = workspace_root()?;
    let xml_path = root.join(XML_PATH);
    let rust_out_dir = root.join(RUST_OUT_DIR);
    let c_header_path = root.join(C_HEADER_PATH);

    let xml =
        fs::read_to_string(&xml_path).with_context(|| format!("reading {}", xml_path.display()))?;
    let protocol = vitrin_scanner::parse::parse(&xml)
        .with_context(|| format!("parsing {}", xml_path.display()))?;

    if check {
        return check_no_drift(&root, &protocol, &rust_out_dir, &c_header_path);
    }

    // Plain `codegen`: regenerate both outputs in place, unconditionally --
    // a human runs this after editing the IDL, reviews `git diff`, and
    // commits the result.
    vitrin_scanner::rust_gen::generate(&protocol, &rust_out_dir)
        .with_context(|| format!("generating Rust modules into {}", rust_out_dir.display()))?;
    vitrin_scanner::c_gen::generate(&protocol, &c_header_path)
        .with_context(|| format!("generating C header at {}", c_header_path.display()))?;
    eprintln!("xtask: regenerated {RUST_OUT_DIR} and {C_HEADER_PATH}");
    eprintln!("xtask: codegen complete -- review `git diff` and commit.");
    Ok(())
}

/// Scratch directory `codegen --check` regenerates into -- entirely separate
/// from the real, checked-in output paths, and never written to by anything
/// else. Deliberately placed inside `target/` (already gitignored) rather
/// than under the OS temp directory: `rustfmt` discovers `rustfmt.toml` by
/// walking up from the *target file's* directory (see
/// `rust_gen::write_formatted`'s doc comment), and a path under the repo's
/// own `target/` walks up to the same repo-root `rustfmt.toml` that the real
/// in-place generation finds -- so scratch output is formatted identically
/// to a real run for reasons beyond just the explicit
/// `--config reorder_modules=false` override, keeping the comparison below
/// free of false positives from formatting differences alone.
const CHECK_SCRATCH_DIR: &str = "target/xtask-codegen-check";

/// `codegen --check`: verify that the checked-in generated files
/// (`real_rust_out_dir`, `real_c_header_path`) exactly match what `protocol`
/// currently generates -- without ever writing to those real paths.
///
/// This deliberately does not use `git diff`/`git status` in any form.
/// `git diff --exit-code HEAD -- <path>` (the previous implementation) is
/// structurally blind to a path with no git history at all: an untracked
/// file is simply absent from both sides of a `HEAD` comparison, so that
/// command reports "no difference" no matter the file's content. On the
/// branch that introduced this check, `real_rust_out_dir` and
/// `real_c_header_path` are exactly such paths -- their first introduction
/// to the repo -- so a git-based diff would silently pass regardless of
/// whether the checked-in files actually matched the IDL. Comparing freshly
/// generated bytes directly against whatever currently sits on disk sidesteps
/// git's tracking state entirely: it is correct whether the real paths are
/// untracked, staged, or committed, both today and in every future PR.
///
/// Implementation: regenerate into an isolated scratch directory under
/// `target/` (see [`CHECK_SCRATCH_DIR`]), then compare file-by-file against
/// the real paths' current on-disk content. This also makes "leaves the
/// working tree exactly as found either way it exits" trivially true --
/// there is nothing to restore, because the real paths are never written to
/// during `--check` in the first place.
fn check_no_drift(
    root: &Path,
    protocol: &vitrin_scanner::ir::Protocol,
    real_rust_out_dir: &Path,
    real_c_header_path: &Path,
) -> Result<()> {
    let scratch_root = root.join(CHECK_SCRATCH_DIR);
    // Purge any leftover scratch state from a previous `--check` run first:
    // otherwise a file some earlier XML revision generated (e.g. for an
    // interface since removed) could linger here and be mistaken for part of
    // *this* run's freshly generated, ground-truth output below.
    if scratch_root.exists() {
        fs::remove_dir_all(&scratch_root).with_context(|| {
            format!(
                "clearing stale scratch directory {}",
                scratch_root.display()
            )
        })?;
    }
    let scratch_rust_out_dir = scratch_root.join("generated");
    let scratch_c_header_path = scratch_root.join("vitrin-protocol.h");

    vitrin_scanner::rust_gen::generate(protocol, &scratch_rust_out_dir).with_context(|| {
        format!(
            "generating Rust modules into scratch directory {}",
            scratch_rust_out_dir.display()
        )
    })?;
    vitrin_scanner::c_gen::generate(protocol, &scratch_c_header_path).with_context(|| {
        format!(
            "generating C header into scratch path {}",
            scratch_c_header_path.display()
        )
    })?;

    let mut drift = Vec::new();
    diff_dir_trees(
        &scratch_rust_out_dir,
        real_rust_out_dir,
        RUST_OUT_DIR,
        &mut drift,
    )?;
    diff_single_file(
        &scratch_c_header_path,
        real_c_header_path,
        C_HEADER_PATH,
        &mut drift,
    )?;

    // Scratch output has served its purpose. Clean it up so it doesn't
    // linger (harmless either way -- `target/` is gitignored -- but tidier,
    // and it means the next run's staleness check above is a no-op).
    let _ = fs::remove_dir_all(&scratch_root);

    if drift.is_empty() {
        eprintln!(
            "xtask: codegen --check: no drift -- generated files match protocol/vitrin-v0.xml"
        );
        return Ok(());
    }

    eprintln!("xtask: codegen --check: drift detected --");
    for line in &drift {
        eprintln!("  {line}");
    }
    bail!(
        "codegen --check: generated files have drifted from protocol/vitrin-v0.xml ({} file(s) \
         listed above differ from what codegen currently produces, under {RUST_OUT_DIR} or \
         {C_HEADER_PATH}). Run `cargo xtask codegen` and commit the result.",
        drift.len()
    );
}

/// Recursively compare two directory trees for byte-for-byte equality,
/// appending one human-readable line per difference (missing, extra, or
/// changed file) to `out`. `expected_root` is freshly generated output
/// (ground truth, from this run's parsed `protocol`); `actual_root` is
/// whatever currently sits on disk at the real, checked-in path, regardless
/// of git state.
fn diff_dir_trees(
    expected_root: &Path,
    actual_root: &Path,
    label: &str,
    out: &mut Vec<String>,
) -> Result<()> {
    let mut expected_files = BTreeMap::new();
    collect_files(expected_root, expected_root, &mut expected_files)
        .with_context(|| format!("walking freshly generated {}", expected_root.display()))?;
    let mut actual_files = BTreeMap::new();
    collect_files(actual_root, actual_root, &mut actual_files)
        .with_context(|| format!("walking checked-in {}", actual_root.display()))?;

    for (rel, expected_path) in &expected_files {
        match actual_files.get(rel) {
            None => out.push(format!(
                "{label}/{rel}: missing on disk (codegen would generate this file)"
            )),
            Some(actual_path) => {
                let expected_bytes = fs::read(expected_path)
                    .with_context(|| format!("reading {}", expected_path.display()))?;
                let actual_bytes = fs::read(actual_path)
                    .with_context(|| format!("reading {}", actual_path.display()))?;
                if expected_bytes != actual_bytes {
                    out.push(format!(
                        "{label}/{rel}: on-disk content differs from fresh codegen output"
                    ));
                }
            }
        }
    }
    for rel in actual_files.keys() {
        if !expected_files.contains_key(rel) {
            out.push(format!(
                "{label}/{rel}: present on disk but codegen no longer produces this file"
            ));
        }
    }
    Ok(())
}

/// Recursively collect every regular file under `dir` into `out`, keyed by
/// its path relative to `root` with `/`-separated components regardless of
/// platform (a stable map key). A missing `dir` (e.g. the real output
/// directory not existing at all yet) yields an empty map rather than an
/// error -- that absence is itself reported as drift by the caller (via the
/// "missing on disk" branch in [`diff_dir_trees`]), not by failing here.
fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading a directory entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat-ing {}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if !file_type.is_file() {
            // A symlink (or fifo, socket, ...) in a generated-output tree is
            // never something codegen produces; silently skipping it would
            // make the drift check blind to whatever it points at (or
            // shadows). Fail loudly instead.
            bail!(
                "unexpected non-regular-file entry {} while walking generated output \
                 (symlinks are not produced by codegen and are not compared)",
                path.display()
            );
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("path was walked from under root, so root must be a prefix of it")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.insert(rel, path);
        }
    }
    Ok(())
}

/// Compare a single freshly generated file against the real, checked-in
/// path's current on-disk content (which may not exist at all), appending a
/// human-readable line to `out` iff they differ.
fn diff_single_file(
    expected_path: &Path,
    actual_path: &Path,
    label: &str,
    out: &mut Vec<String>,
) -> Result<()> {
    let expected_bytes = fs::read(expected_path)
        .with_context(|| format!("reading freshly generated {}", expected_path.display()))?;
    match fs::read(actual_path) {
        Ok(actual_bytes) => {
            if expected_bytes != actual_bytes {
                out.push(format!(
                    "{label}: on-disk content differs from fresh codegen output"
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            out.push(format!(
                "{label}: missing on disk (codegen would generate this file)"
            ));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("reading {}", actual_path.display()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `cargo xtask demo [--headless]` -- the P1.8.4 demo agent / M1.5 acceptance
// ---------------------------------------------------------------------------

/// The demo identity and its pre-shared token. Must match
/// `examples/principals.toml` and `run_demo.py`'s `DEMO_IDENTITY`/`DEMO_TOKEN`:
/// the launcher writes the registry, the agent presents the credential, and
/// the R6 auto-approve guard is only satisfied because the registry written
/// below holds *nothing but* this one row.
const DEMO_IDENTITY: &str = "vitrin://local/agent/demo";
const DEMO_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Headless virtual-output size. Small and fixed: it keeps captures cheap and
/// the URL-bar locator's nominal coordinate comfortably in bounds. Matches the
/// integration harness's default so the two agree on frame geometry.
const HEADLESS_SIZE: &str = "320x200";

/// Frames the mock shim animates for. A CPU budget, not a duration (headless
/// has no output clock): enough to outlive the demo's before/after capture
/// pair and no longer. Mirrors `harness.ANIMATE_FRAMES`.
const DEMO_ANIMATE_FRAMES: u32 = 1200;

/// Default Firefox ESR path for the nested venue. Overridable with
/// `VITRIN_DEMO_FIREFOX` because the binary's name and location vary by distro
/// (`/usr/bin/firefox-esr` on Debian, `/usr/bin/firefox` elsewhere). The realm
/// loader refuses a relative name, so this is always absolute.
const DEFAULT_FIREFOX: &str = "/usr/bin/firefox-esr";

/// `cargo xtask demo`: launch the shipped core and drive the demo agent
/// against it, in whichever venue was selected.
fn demo(headless: bool) -> Result<()> {
    let root = workspace_root()?;
    let bin_dir = binary_dir()?;
    let vitrind = bin_dir.join("vitrind");
    if !vitrind.is_file() {
        bail!(
            "vitrind not found at {} -- run `cargo build --workspace` first",
            vitrind.display()
        );
    }

    let work = make_work_dir().context("creating the demo's throwaway runtime directory")?;
    let principals = work.join("principals.toml");
    let realm = work.join("realm.toml");
    let recorder = work.join("flight.jsonl");
    let frames = work.join("frames");

    // One-principal registry, mode 0600: the exact shape the R6 auto-approve
    // guard requires (and refuses any wider mode of).
    fs::write(
        &principals,
        format!("[[principal]]\nidentity = \"{DEMO_IDENTITY}\"\ntoken = \"{DEMO_TOKEN}\"\n"),
    )
    .with_context(|| format!("writing {}", principals.display()))?;
    fs::set_permissions(&principals, Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", principals.display()))?;

    // Assemble the two venue-specific pieces: the realm config, the core's
    // argv, the environment vitrind runs under, and where its socket lands.
    let mut core_cmd = Command::new(&vitrind);
    core_cmd.env("RUST_LOG", "info");

    let socket: PathBuf = if headless {
        let mock_shim = bin_dir.join("vitrin-mock-shim");
        if !mock_shim.is_file() {
            bail!(
                "vitrin-mock-shim not found at {} -- run `cargo build --workspace` first",
                mock_shim.display()
            );
        }
        // The mock shim stands in for the app: `--seat` so seat events deliver,
        // `--animate` so the two captures differ across the actuation sequence.
        fs::write(
            &realm,
            format!(
                "[[realm]]\nid = \"realm-0\"\ncommand = \"{}\"\n\
                 args = [\"--serve\", \"--seat\", \"--animate\", \"{DEMO_ANIMATE_FRAMES}\"]\n",
                mock_shim.display()
            ),
        )
        .with_context(|| format!("writing {}", realm.display()))?;

        // Headless owns its runtime tree, so the socket is deterministic.
        core_cmd
            .args([
                "--headless",
                "--size",
                HEADLESS_SIZE,
                "--consent=auto-approve",
            ])
            .args(["--principals".as_ref(), principals.as_os_str()])
            .args(["--realm".as_ref(), realm.as_os_str()])
            .args(["--recorder".as_ref(), recorder.as_os_str()])
            .env("XDG_RUNTIME_DIR", &work);
        work.join("vitrin-0").join("core.sock")
    } else {
        // Nested: a real Firefox ESR in the realm. Correct-by-construction; a
        // machine with a display and a browser runs it, CI never does.
        let firefox = std::env::var_os("VITRIN_DEMO_FIREFOX")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FIREFOX));
        // Firefox needs Wayland selection and the session bus. env_allow copies
        // NAMES from vitrind's own environment, so the core must carry the two
        // it sets and pass DBUS through from the ambient session (realm.toml's
        // Firefox note / plan P1.6.4).
        fs::write(
            &realm,
            format!(
                "[[realm]]\nid = \"realm-0\"\ncommand = \"{}\"\n\
                 args = [\"--new-window\", \"about:blank\"]\n\
                 env_allow = [\"HOME\", \"LANG\", \"XDG_SESSION_TYPE\", \
                 \"MOZ_ENABLE_WAYLAND\", \"GDK_BACKEND\", \"DBUS_SESSION_BUS_ADDRESS\"]\n",
                firefox.display()
            ),
        )
        .with_context(|| format!("writing {}", realm.display()))?;

        // Nested draws its window on the host compositor, so vitrind keeps the
        // host session's environment (its WAYLAND_DISPLAY, its XDG_RUNTIME_DIR)
        // and the socket lands under that runtime dir. The two Wayland vars are
        // injected here so env_allow above has something to copy.
        let xdg = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            anyhow::anyhow!(
                "nested demo needs XDG_RUNTIME_DIR set (the host session's runtime dir, \
                 where vitrind binds its socket and finds the host compositor)"
            )
        })?;
        core_cmd
            .arg("--nested")
            .args(["--principals".as_ref(), principals.as_os_str()])
            .args(["--realm".as_ref(), realm.as_os_str()])
            .args(["--recorder".as_ref(), recorder.as_os_str()])
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("GDK_BACKEND", "wayland");
        PathBuf::from(xdg).join("vitrin-0").join("core.sock")
    };

    eprintln!(
        "xtask demo: launching vitrind ({} venue); socket {}",
        if headless { "headless" } else { "nested" },
        socket.display()
    );
    let mut core = core_cmd
        .spawn()
        .with_context(|| format!("spawning {}", vitrind.display()))?;

    // Wait for the socket, or explain why it will never appear.
    if let Err(err) = await_core_socket(&socket, &mut core, Duration::from_secs(30)) {
        terminate(&mut core);
        return Err(err);
    }

    // Run the demo agent. `python3` off the SDK's source path; no venv, no
    // install -- the SDK is stdlib-only (D8), like the integration suite.
    let demo_py = root.join("examples/agent-demo/run_demo.py");
    let pythonpath = root.join("sdk/python/src");
    let mut demo_cmd = Command::new("python3");
    demo_cmd
        .arg(&demo_py)
        .args(["--socket".as_ref(), socket.as_os_str()])
        .args(["--out".as_ref(), frames.as_os_str()])
        .args(["--recorder".as_ref(), recorder.as_os_str()])
        .env("PYTHONPATH", &pythonpath);
    if headless {
        demo_cmd.args(["--headless", "--consent", "auto-approve"]);
    } else {
        demo_cmd.args(["--consent", "interactive"]);
    }
    let status = match demo_cmd.status() {
        Ok(status) => status,
        Err(err) => {
            // Reap the core we already spawned before propagating, so a failure
            // to launch the demo agent (e.g. `python3` off PATH) never leaks a
            // live vitrind + mock-shim -- same guard as the socket-wait branch.
            terminate(&mut core);
            return Err(err).with_context(|| format!("running {}", demo_py.display()));
        }
    };

    // Tear the core down the clean way (SIGTERM), so the recorder's footer is
    // written and the log names this run's end.
    terminate(&mut core);

    eprintln!("xtask demo: flight recorder at {}", recorder.display());
    if status.success() {
        eprintln!("xtask demo: PASS");
        Ok(())
    } else {
        bail!(
            "demo agent exited with {} (frames under {}, recorder {})",
            status,
            frames.display(),
            recorder.display()
        );
    }
}

/// The directory holding the sibling `vitrind`/`vitrin-mock-shim` binaries:
/// this `xtask` executable's own directory. Resolving from `current_exe`
/// honors `CARGO_TARGET_DIR` and whatever profile built us, without guessing
/// `target/debug` from the workspace root.
fn binary_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running xtask binary")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("xtask binary {} has no parent directory", exe.display()))
}

/// A fresh 0700 runtime directory for one demo run, under the OS temp dir. The
/// pid plus a monotonic-ish suffix keeps concurrent runs apart without a
/// randomness dependency.
fn make_work_dir() -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("vitrin-demo-{}-{stamp}", std::process::id()));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Block until `socket` exists, or the core exits first, or the deadline
/// passes. Polls rather than sleeping a fixed interval: the boot path does an
/// R6 audit, a realm load, a flock, a recorder create and a fork, any of which
/// can be slow on a loaded machine.
fn await_core_socket(socket: &Path, core: &mut Child, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = core.try_wait().context("polling vitrind while it boots")? {
            bail!(
                "vitrind exited with {status} during boot, before binding {}",
                socket.display()
            );
        }
        if socket.exists() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "vitrind never bound {} within {:?}",
                socket.display(),
                timeout
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the core with SIGTERM (never SIGKILL): SIGTERM is the path that runs
/// the shutdown ladder and writes the recorder's `run_ended` footer. If it is
/// already gone, this is a no-op. Falls back to a forceful `kill()` only if
/// the process refuses to leave within a grace window.
fn terminate(core: &mut Child) {
    if let Ok(Some(_)) = core.try_wait() {
        return; // already exited
    }
    if let Some(pid) = rustix::process::Pid::from_raw(core.id() as i32) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match core.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = core.kill();
                let _ = core.wait();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return,
        }
    }
}
