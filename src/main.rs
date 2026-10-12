//! The rosie binary: gathers the environment once and hands it to [`App`].

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::{self, ExitCode};
use std::time::SystemTime;

use rosie::cli::{App, Env, SystemConsole};
use rosie::fs::{Paths, RealBackend};
use rosie::output::ColorChoice;
use rosie::packs::Https;

fn main() -> ExitCode {
    let console = SystemConsole::new(color_choice(env::var_os("NO_COLOR")));
    let mut app = App::new(RealBackend, Https::new(), console);
    let status = app.run(env::args_os(), gather_env);
    ExitCode::from(status.code())
}

/// `NO_COLOR` set and not empty turns color off (<https://no-color.org>).
fn color_choice(no_color: Option<OsString>) -> ColorChoice {
    match no_color {
        Some(value) if !value.is_empty() => ColorChoice::Never,
        Some(_) | None => ColorChoice::Auto,
    }
}

/// The one place rosie reads `$HOME` and the process environment, besides `NO_COLOR`,
/// which the console needs before any command.
fn gather_env() -> Result<Env, String> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    let paths = Paths::new(Path::new(&home)).map_err(|error| format!("HOME: {error}"))?;
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read the current folder: {error}"))?;
    let exe =
        env::current_exe().map_err(|error| format!("cannot find the rosie executable: {error}"))?;

    Ok(Env {
        paths,
        cwd,
        exe,
        pid: process::id(),
        now: SystemTime::now(),
    })
}
