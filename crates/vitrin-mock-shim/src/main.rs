//! The mock shim as an **executable** — the child `vitrin-core`'s spawn
//! manager (P1.5.2, issue #31) actually forks and `exec`s.
//!
//! The library half of this crate ([`vitrin_mock_shim`]) is a compliant shim
//! speaking the real wire protocol over a [`Connection`] a test constructs.
//! This binary is the same fixture on the other side of an `execve`: it
//! reconstructs its core connection from the descriptor the core placed at
//! [`CORE_FD`] and performs the same session bring-up, which is what makes
//! the spawn path testable end to end long before the wlroots shim (E6,
//! issue #33) exists.
//!
//! # This is the test-time `--shim` binary
//!
//! Since issue #103 the core execs a core-known **shim** binary (holding
//! fd 3) that in turn execs the realm's app, rather than execing the app
//! directly. The real shim is the wlroots one (E6/#33); until it lands, this
//! fixture *is* the `--shim` binary every realm test hands the core. The core
//! conveys the app command to a shim as `<shim> -- <app> <app-args...>`, so
//! this binary is invoked with a `-- <app>` tail it must tolerate. It does,
//! by construction: its mode flags are matched anywhere in argv (the scan
//! below ignores the `--` separator and the app path, which match none of
//! them), so a harness places the fixture flags it wants -- `--serve`,
//! `--seat`, `--animate N`, `--spawn-app` -- in the app-argument tail and
//! this binary honors them. It deliberately does **not** exec the app named
//! after `--`: execing the real app is the wlroots shim's job (#104), and
//! this fixture animates a synthetic buffer instead.
//!
//! # The fd-inheritance contract, from the shim's side
//!
//! - **Identity is the descriptor.** There is no handshake, no credential,
//!   no realm name to claim: this process holds fd 3, therefore it *is*
//!   that realm's shim (PRD Doc 2 §4.1). It learns which realm it serves
//!   from the core's `configure`, and the core learns nothing from it.
//! - **Fd 3 arrives with `FD_CLOEXEC` cleared** — that is how it survived
//!   `execve` — so the first thing this program does is set it. Without
//!   that one line, every app the shim spawns would inherit a live core
//!   connection, which is the whole confinement gone. The C shim owes the
//!   same line; `spawn::tests::the_app_does_not_inherit_the_core_connection`
//!   is the assertion that catches either of us forgetting it.
//! - **The app inherits the environment the core composed.** The shim does
//!   not rebuild it: `WAYLAND_DISPLAY` already names the shim's private
//!   socket and nothing names the host session, so passing it through
//!   verbatim is both the simplest and the correct behavior.
//!
//! # Modes
//!
//! - `--serve` — reconstruct the core connection and run the session
//!   bring-up ([`MockShim::start`]), then block until the core hangs up.
//! - `--spawn-app` — with `--serve`, additionally spawn a child standing in
//!   for the confined app (this same binary in `--app` mode), so the
//!   `core → shim → app` topology is real rather than described.
//! - `--animate N` — with `--serve`, run the paced animation loop for `N`
//!   frames before falling into the drain loop, so the realm actually has
//!   a committed surface. The realm-lifecycle tests (P1.5.3) need a shim
//!   that is genuinely mid-frame when it is killed: "the scene updates on
//!   shim death" is only an assertion if something was on the scene, and
//!   "never a stale frame" is only meaningful if a real frame was there to
//!   go stale. `N` is deliberately unbounded so a test can leave the shim
//!   animating indefinitely and kill it at an arbitrary point.
//! - `--seat` — with `--serve`, mint the input-delivery object
//!   ([`MockShim::get_seat`]) during bring-up, so the core's `ShimServer`
//!   has a seat to address. A real shim mints one; the fixture defaulted to
//!   not, since its focus was surface bring-up. A harness that drives seat
//!   delivery — and its flight-recorder audit (issue #83) — needs a shim
//!   that has minted a seat, or every event drops undelivered.
//! - `--app` — the leaf: block until stdin closes. Holds no core
//!   connection, which is exactly what its descriptor table must show.

use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::process::{Command, ExitCode, Stdio};

use vitrin_ipc::Connection;
use vitrin_mock_shim::MockShim;

/// The descriptor the core places the shim's end of the identity socketpair
/// on. Fixed at compile time on both sides and announced nowhere — see
/// `crates/vitrin-core/src/spawn.rs` for why it is neither an environment
/// variable nor an argv element.
const CORE_FD: RawFd = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    // `--animate N`: the value is the following argument.
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u32>().ok())
    };

    if flag("--app") {
        // The confined app: nothing to do but stay alive so the harness can
        // read this process's real descriptor table and parentage.
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        return ExitCode::SUCCESS;
    }

    if !flag("--serve") {
        eprintln!("vitrin-mock-shim: expected --serve or --app");
        return ExitCode::from(2);
    }

    // Prove the descriptor is open *before* claiming ownership of it, with
    // a path read rather than a fabricated descriptor: `OwnedFd` requires an
    // open file descriptor, and this binary is perfectly runnable by hand,
    // where fd 3 is whatever the shell left there — usually nothing.
    if std::fs::read_link(format!("/proc/self/fd/{CORE_FD}")).is_err() {
        eprintln!(
            "vitrin-mock-shim: fd {CORE_FD} is not open. This binary is spawned by the \
             trusted core, which places the shim's end of the identity socketpair there \
             at fork; it is not meant to be run by hand."
        );
        return ExitCode::from(2);
    }

    // SAFETY: fd `CORE_FD` is open (proved immediately above) and is the
    // socketpair end the core placed here at fork (identity-at-fork). This
    // is the only place in the process that claims it, it happens once, at
    // startup, before anything else can touch it, and ownership then lives
    // in the `Connection` for the process's lifetime.
    let fd = unsafe { OwnedFd::from_raw_fd(CORE_FD) };

    // The shim's one non-negotiable line: fd 3 arrived close-on-exec
    // CLEARED, and every process this shim spawns from here on must not
    // inherit the core connection.
    if let Err(err) = rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC) {
        eprintln!("vitrin-mock-shim: cannot re-arm FD_CLOEXEC on fd {CORE_FD}: {err}");
        return ExitCode::FAILURE;
    }

    let conn = match Connection::from_fd(fd) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("vitrin-mock-shim: fd {CORE_FD} is not a core connection: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Blocks on the core's `configure`, the guaranteed-first message on a
    // shim connection, then mints a surface. Reaching this point proves the
    // inherited descriptor is a working, correctly-paired core connection.
    let mut shim = match MockShim::start(conn) {
        Ok(shim) => shim,
        Err(err) => {
            eprintln!("vitrin-mock-shim: session bring-up failed: {err}");
            return ExitCode::FAILURE;
        }
    };

    // `--seat`: mint the input-delivery object so the core has a seat to
    // address. Without it every routed seat event drops undelivered, and the
    // delivery-point audit entry (issue #83) never fires.
    if flag("--seat") {
        if let Err(err) = shim.get_seat() {
            eprintln!("vitrin-mock-shim: get_seat failed: {err}");
            return ExitCode::FAILURE;
        }
    }

    // Only after bring-up, so the topology a harness observes is the real
    // ordering: a shim launches its app once it has a realm to launch it
    // into.
    let mut app = None;
    if flag("--spawn-app") {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                eprintln!("vitrin-mock-shim: cannot locate self to spawn an app: {err}");
                return ExitCode::FAILURE;
            }
        };
        // Deliberately inherits this process's environment: a real shim
        // passes the core-composed environment down untouched, and the app
        // must therefore see the same private WAYLAND_DISPLAY.
        match Command::new(exe).arg("--app").stdin(Stdio::piped()).spawn() {
            Ok(child) => app = Some(child),
            Err(err) => {
                eprintln!("vitrin-mock-shim: cannot spawn the app: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Paint, if asked. The loop is paced on `frame_done`, so it advances
    // only as fast as the core presents -- which is what makes "killed
    // mid-frame" a real state rather than a description. It ends early and
    // silently when the core hangs up or kills this process: a shim that
    // panicked on its core disappearing would be testing the fixture's
    // error handling instead of the core's death path.
    if let Some(frames) = value("--animate") {
        let _ = shim.run_paced_animation(frames);
    }

    // Serve until the core hangs up (or kills us). Every message is drained
    // so a harness driving a real `ShimServer` sees ordinary traffic.
    while let Ok(Some(_)) = shim.connection_mut().recv_message() {}

    if let Some(mut app) = app {
        // Closing the app's stdin is its exit signal; then reap it, so this
        // fixture never leaves a stray process behind.
        drop(app.stdin.take());
        let _ = app.wait();
    }
    ExitCode::SUCCESS
}
