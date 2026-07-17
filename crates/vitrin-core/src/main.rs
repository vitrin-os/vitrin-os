//! `vitrind` — the Vitrin OS trusted core.
//!
//! This binary is the entire Trusted Computing Base (TCB) of a Vitrin
//! session: compositor, capability kernel, grant store, realm/spawn manager,
//! and consent surface (PRD Doc 2 §2). Everything else — per-app shims,
//! agents, shells — is untrusted and talks to this process over the
//! capability protocol (`protocol/vitrin-v0.xml`).
//!
//! TCB discipline, enforced from this first commit:
//!
//! - **No policy in the core.** Window management, decoration, and theming
//!   run in unprivileged components outside this binary (PRD Doc 2 §2 — the
//!   Nitpicker/Qubes lesson). The skeleton renders only its own test
//!   pattern; when client surfaces arrive (P1.3.3) their layout stays
//!   trivial and isolated.
//! - **One enforcement chokepoint.** When capture (P1.3.6) and actuation
//!   (P1.3.7) land, every request is checked at a single site against the
//!   grant table. No second authority-checking code path.
//! - **Budgeted dependencies** (plan risk R7): the skeleton links Smithay's
//!   winit backend, calloop, and a tracing subscriber — nothing else.
//!
//! P1.3.1 scope: nested mode only (`vitrind --nested`) — the core runs as a
//! client of the host compositor, presents exactly one host window, and
//! renders a deterministic test pattern at the host's frame cadence.
//! Headless mode (`--headless`) is P1.3.2; the shim-facing protocol server
//! is P1.3.4.

mod backend;
mod test_pattern;

use std::process::ExitCode;

const USAGE: &str = "\
vitrind — Vitrin OS trusted core

USAGE:
    vitrind --nested     Run nested inside the host compositor (one window,
                         test pattern). Headless mode arrives with P1.3.2.
    vitrind --help       Show this help.
    vitrind --version    Show the version.
";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    RunNested,
    Help,
    Version,
}

/// Hand-rolled argument parsing: the surface is three flags, which does not
/// justify pulling an argument-parsing crate into the TCB (plan risk R7).
fn parse_args<'a, I: IntoIterator<Item = &'a str>>(args: I) -> Result<Action, String> {
    let mut action = None;
    for arg in args {
        match arg {
            "--nested" => match action {
                None => action = Some(Action::RunNested),
                Some(_) => return Err("`--nested` given more than once".into()),
            },
            "--help" | "-h" => return Ok(Action::Help),
            "--version" | "-V" => return Ok(Action::Version),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    action.ok_or_else(|| "no mode given (expected `--nested`)".into())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let action = match parse_args(args.iter().map(String::as_str)) {
        Ok(action) => action,
        Err(msg) => {
            eprintln!("vitrind: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match action {
        Action::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("vitrind {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Action::RunNested => {
            init_tracing();
            match backend::winit::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    tracing::error!("fatal: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Route Smithay's (and our own) `tracing` diagnostics to stderr.
/// `RUST_LOG` selects the filter; the default is `info`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_flag_selects_nested_mode() {
        assert_eq!(parse_args(["--nested"]), Ok(Action::RunNested));
    }

    #[test]
    fn help_and_version_win_over_mode() {
        assert_eq!(parse_args(["--nested", "--help"]), Ok(Action::Help));
        assert_eq!(parse_args(["--version"]), Ok(Action::Version));
    }

    #[test]
    fn no_mode_is_an_error() {
        assert!(parse_args([]).is_err());
    }

    #[test]
    fn unknown_argument_is_an_error() {
        assert!(parse_args(["--headless"]).is_err());
        assert!(parse_args(["--nested", "extra"]).is_err());
    }

    #[test]
    fn duplicate_mode_is_an_error() {
        assert!(parse_args(["--nested", "--nested"]).is_err());
    }
}
