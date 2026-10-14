//! Refusing `sudo rosie …` (`docs/spec/safety.md#refusing-sudo-rosie`): every root
//! invocation other than the elevated child is refused.

use std::fmt;

use thiserror::Error;

use crate::fs::{self, Backend, FsContext};
use crate::process;

/// The refusal lines, one chosen per run.
const LINES: [&str; 3] = [
    "Jane! Stop this crazy thing!",
    "I swear on my mother's rechargeable batteries!",
    "rosie destroys things. It should never be run under sudo. Trust me, you don't know \
     what you are doing.",
];

#[derive(Debug, Error)]
pub enum RootError {
    #[error("{0}")]
    Refused(RootRefusal),

    #[error(transparent)]
    Process(#[from] process::Error),
}

/// The message a root invocation is refused with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRefusal {
    line: &'static str,
    /// The arguments rosie was given, to repeat without sudo.
    args: Vec<String>,
}

impl RootRefusal {
    /// Picks the line by process id, so it changes from run to run without stored state.
    pub fn new(pid: u32, args: Vec<String>) -> Self {
        let index = pid as usize % LINES.len();
        RootRefusal {
            line: LINES[index],
            args,
        }
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
}

/// The refusal line; the command to run instead is its hint.
impl fmt::Display for RootRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.line)
    }
}

/// Refuses to go on when rosie runs as root; the elevated child never comes here.
/// `args` are rosie's arguments, without the program name.
pub fn refuse_root<B: Backend>(
    ctx: &FsContext<B>,
    pid: u32,
    args: Vec<String>,
) -> Result<(), RootError> {
    match process::effective_uid(ctx)? {
        fs::ROOT_UID => Err(RootError::Refused(RootRefusal::new(pid, args))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::fs::fake::FakeBackend;
    use crate::fs::{Argv, Bounds, CommandOutput, Exit};

    fn ctx_running_as<'a>(fake: &'a FakeBackend, uid: &str) -> FsContext<&'a FakeBackend> {
        fake.respond(
            Argv::new("/usr/bin/id").arg("-u"),
            CommandOutput {
                exit: Exit::Code(0),
                stdout: format!("{uid}\n").into_bytes(),
                stderr: Vec::new(),
            },
        );
        let bounds = Bounds {
            roots: vec![],
            paths: crate::fs::Paths::new(Path::new("/var/root")).expect("absolute home"),
            user_uid: 0,
        };
        FsContext::new(fake, bounds).expect("absolute bounds")
    }

    fn args() -> Vec<String> {
        vec!["clean".into(), "tree".into(), ".".into()]
    }

    #[test]
    fn rotates_through_every_line_by_process_id() {
        let shown: Vec<String> = (300..303)
            .map(|pid| RootRefusal::new(pid, args()).to_string())
            .collect();

        for line in LINES {
            let with_line = shown.iter().filter(|text| text.starts_with(line)).count();
            assert_eq!(with_line, 1, "{line:?} shown once in {shown:?}");
        }
    }

    #[test]
    fn refuses_root_with_a_line_and_the_arguments_to_run_again() {
        let fake = FakeBackend::new();
        let ctx = ctx_running_as(&fake, "0");

        let refused = refuse_root(&ctx, 3, args());

        let Err(RootError::Refused(refusal)) = refused else {
            panic!("expected a refusal, got {refused:?}");
        };
        assert_eq!(refusal.to_string(), "Jane! Stop this crazy thing!");
        assert_eq!(refusal.args(), args());
    }

    #[test]
    fn lets_a_user_through() {
        let fake = FakeBackend::new();
        let ctx = ctx_running_as(&fake, "501");

        assert!(refuse_root(&ctx, 3, args()).is_ok());
    }
}
