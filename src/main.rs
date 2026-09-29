//! The rosie binary: gathers the environment once and hands it to [`App`].

use std::env;
use std::path::Path;
use std::process::{self, ExitCode};
use std::time::SystemTime;

use rosie::cli::{App, Env, SystemConsole};
use rosie::fs::{Home, RealBackend};
use rosie::packs::Https;

fn main() -> ExitCode {
    let env = match gather_env() {
        Ok(env) => env,
        Err(reason) => {
            eprintln!("rosie: {reason}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = App::new(RealBackend, Https::new(), SystemConsole::new(), env);
    let status = app.run(env::args_os());
    ExitCode::from(status.code())
}

/// The one place rosie reads `$HOME` and the process environment.
fn gather_env() -> Result<Env, String> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    let home = Home::new(Path::new(&home)).map_err(|error| format!("HOME: {error}"))?;
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read the current folder: {error}"))?;
    let exe =
        env::current_exe().map_err(|error| format!("cannot find the rosie executable: {error}"))?;

    Ok(Env {
        home,
        cwd,
        exe,
        pid: process::id(),
        now: SystemTime::now(),
    })
}
