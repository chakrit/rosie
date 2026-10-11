//! A tool rule's command: a string handed to a shell as `<shell> -c <cmd>`, with an
//! environment added (`docs/spec/rules.md#tool-strategy`). Rosie never parses or splits
//! the string; the shell does.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::backend::{Argv, SystemTool};

/// A command run as `<shell> -c <cmd>` with `env` added to rosie's environment. Checked
/// once, when it is built: the shell is an absolute path, the command is not blank,
/// every variable name is a shell name, and nothing holds a NUL byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    shell: PathBuf,
    cmd: String,
    env: BTreeMap<String, String>,
}

/// Why a tool command cannot be run.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidCommand {
    #[error("shell {0:?} is not an absolute path")]
    RelativeShell(String),

    #[error("cmd is empty")]
    EmptyCmd,

    #[error("env name {0:?} is not letters, digits, and `_`, starting with no digit")]
    BadEnvName(String),

    #[error("{0:?} holds a NUL byte, which no command can carry")]
    NulByte(String),
}

impl ShellCommand {
    /// The shell a tool rule runs when it names none.
    pub const DEFAULT_SHELL: &'static str = SystemTool::BASH.path();

    pub fn new(
        shell: String,
        cmd: String,
        env: BTreeMap<String, String>,
    ) -> Result<Self, InvalidCommand> {
        refuse_nul(&shell)?;
        refuse_nul(&cmd)?;
        if !shell.starts_with('/') {
            return Err(InvalidCommand::RelativeShell(shell));
        }
        if cmd.trim().is_empty() {
            return Err(InvalidCommand::EmptyCmd);
        }
        for (name, value) in &env {
            refuse_nul(name)?;
            refuse_nul(value)?;
            if !is_shell_name(name) {
                return Err(InvalidCommand::BadEnvName(name.clone()));
            }
        }

        Ok(ShellCommand {
            shell: PathBuf::from(shell),
            cmd,
            env,
        })
    }

    pub fn shell(&self) -> &Path {
        &self.shell
    }

    pub fn cmd(&self) -> &str {
        &self.cmd
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// What runs: `<shell> -c <cmd>`, with the environment added.
    pub fn argv(&self) -> Argv {
        let argv = Argv::new(&self.shell).arg("-c").arg(&self.cmd);
        self.env
            .iter()
            .fold(argv, |argv, (name, value)| argv.env(name, value))
    }
}

/// The command as written, for a checklist line: `run brew cleanup`.
impl fmt::Display for ShellCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.cmd)
    }
}

fn refuse_nul(text: &str) -> Result<(), InvalidCommand> {
    match text.contains('\0') {
        true => Err(InvalidCommand::NulByte(text.to_owned())),
        false => Ok(()),
    }
}

/// A POSIX shell name: a letter or `_`, then letters, digits, or `_`. Anything else is
/// refused, so a name can never be read as an `env` option or carry an `=`.
fn is_shell_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
    first_ok && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn command(
        shell: &str,
        cmd: &str,
        vars: &[(&str, &str)],
    ) -> Result<ShellCommand, InvalidCommand> {
        ShellCommand::new(shell.to_owned(), cmd.to_owned(), env(vars))
    }

    #[test]
    fn runs_the_string_whole_through_the_shell_with_the_environment() {
        let command = command(
            "/bin/zsh",
            "brew cleanup --prune=all && echo 'a  b'",
            &[("HOMEBREW_NO_ENV_HINTS", "1")],
        )
        .expect("valid");

        let argv = command.argv();

        assert_eq!(argv.program(), "/bin/zsh");
        assert_eq!(
            argv.args(),
            [
                OsString::from("-c"),
                OsString::from("brew cleanup --prune=all && echo 'a  b'")
            ]
        );
        assert_eq!(
            argv.envs(),
            [(OsString::from("HOMEBREW_NO_ENV_HINTS"), OsString::from("1"))]
        );
    }

    #[test]
    fn refuses_what_no_shell_can_run() {
        let cases = [
            (
                command("bash", "x", &[]),
                InvalidCommand::RelativeShell("bash".into()),
            ),
            (command("/bin/bash", "  \n", &[]), InvalidCommand::EmptyCmd),
            (
                command("/bin/bash", "a\0b", &[]),
                InvalidCommand::NulByte("a\0b".into()),
            ),
            (
                command("/bin/bash", "x", &[("-i", "1")]),
                InvalidCommand::BadEnvName("-i".into()),
            ),
            (
                command("/bin/bash", "x", &[("A=B", "1")]),
                InvalidCommand::BadEnvName("A=B".into()),
            ),
            (
                command("/bin/bash", "x", &[("9LIVES", "1")]),
                InvalidCommand::BadEnvName("9LIVES".into()),
            ),
            (
                command("/bin/bash", "x", &[("A", "b\0")]),
                InvalidCommand::NulByte("b\0".into()),
            ),
        ];

        for (result, want) in cases {
            assert_eq!(result, Err(want));
        }
    }

    #[test]
    fn accepts_shell_names_with_underscores_and_digits() {
        let result = command("/bin/bash", "x", &[("_A1", ""), ("b_2", "v")]);

        assert!(result.is_ok(), "{result:?}");
    }
}
