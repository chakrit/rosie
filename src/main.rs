//! The rosie binary: gathers the environment once and hands it to [`App`].

#![deny(warnings)]

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
    let mut app = App::new(RealBackend, Https::new(), console, process::id());
    let status = app.run(env::args_os(), gather_env);
    ExitCode::from(status.code())
}

/// `NO_COLOR` set, even empty, turns color off (`docs/spec/cli.md#output`).
fn color_choice(no_color: Option<OsString>) -> ColorChoice {
    no_color.map_or(ColorChoice::Auto, |_| ColorChoice::Never)
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
        now: SystemTime::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_no_color_turns_color_off_even_empty() {
        assert_eq!(color_choice(None), ColorChoice::Auto);
        assert_eq!(color_choice(Some("1".into())), ColorChoice::Never);
        assert_eq!(color_choice(Some("".into())), ColorChoice::Never);
    }
}
