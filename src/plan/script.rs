//! The `--sh` export: the plan as a bash script the user inspects and runs with sudo
//! themselves (`docs/spec/plan.md#shell-script-export`). Rosie cannot run it.
//!
//! - Every path and argument is quoted with [`shell::quote_word`], so none can spill onto a
//!   line of its own, and every command is written as `( exec -- <words> ) </dev/null`,
//!   so bash runs exactly its argv.
//! - The entries sit inside a `main` function that the last line calls. Bash reads the
//!   whole script before running any of it, so a script piped to bash and cut short runs
//!   nothing, and a command that reads its input cannot consume the rest of the script
//!   and make bash resume inside a commented-out line. No command reads bash's input.
//! - `keep` and `outside-roots` entries are commented out; report-only entries are
//!   comments.
//! - A plan with an unresolved rule conflict is refused: only a TOML plan can be
//!   resolved (`docs/spec/plan.md#rule-conflicts`).

use std::path::Path;

use super::conflict::conflict_hint;
use super::{Error, Plan, Status, comment, outside_roots_hint};
use crate::fs::Argv;
use crate::shell;

/// Adds a tool's environment; by its absolute path, so the script's `PATH` cannot swap
/// it.
const ENV_PROGRAM: &str = "/usr/bin/env";

/// `:` keeps `main` valid bash when every entry in it is commented out.
const HEADER: &str = "\
#!/bin/bash
# rosie plan, exported as a shell script. rosie cannot run this file.
# Inspect every line, then run it yourself: sudo bash path/to/this-file.sh
# keep and outside-roots entries are commented out.

main() {
:
";

/// On one line, so bash has read the call and the `exit` before `main` runs.
const FOOTER: &str = "}

main \"$@\"; exit
";

impl Plan {
    /// Bootouts come first, so every launch job is unloaded before its plist is deleted.
    pub fn to_sh(&self) -> Result<String, Error> {
        let conflicts = self.conflicts().len();
        if conflicts > 0 {
            return Err(Error::UnresolvedConflicts { count: conflicts });
        }

        let mut script = HEADER.to_owned();
        for delete in self.deletes.iter() {
            for bootout in &delete.bootouts {
                let note = format!(
                    "unload {}",
                    shell::comment_text(&bootout.domain.to_string())
                );
                let line = Line::of(delete.status.clone(), &delete.path, note);
                script.push_str(&line.render(&command_line(&bootout.argv())));
            }
        }
        for delete in self.deletes.iter() {
            let note = format!("{} · {}", delete.size, rules_text(&delete.rules));
            let line = Line::of(delete.status.clone(), &delete.path, note);
            script.push_str(&line.render(&command_line(&delete_argv(&delete.path))));
        }
        for tool in &self.tools {
            let note = format!("size unknown · {}", shell::comment_text(&tool.rule));
            let line = Line::pathless(tool.selection.into(), note);
            script.push_str(&line.render(&command_line(&tool.command.argv())));
        }
        for receipt in &self.receipts {
            let note = "size unknown · forget receipt".to_owned();
            let line = Line::pathless(receipt.selection.into(), note);
            script.push_str(&line.render(&command_line(&receipt.argv())));
        }
        for report in &self.reports {
            script.push_str(&format!(
                "\n# report only, do by hand: {}\n",
                shell::comment_text(report.subject())
            ));
            for step in report.steps() {
                script.push_str(&format!("#   {}\n", shell::comment_text(step)));
            }
        }

        script.push_str(FOOTER);
        Ok(script)
    }
}

/// One entry's comment and command, commented out unless the entry is `clean`.
struct Line {
    status: Status,
    note: String,
}

impl Line {
    /// An entry with a path, whose `outside-roots` note carries the `rosie roots add`
    /// hint.
    fn of(status: Status, path: &Path, note: String) -> Self {
        let note = match &status {
            Status::OutsideRoots => format!("{note}; {}", outside_roots_hint(path)),
            Status::Conflict(disagreement) => format!("{note}; {}", conflict_hint(disagreement)),
            Status::Clean | Status::Keep => note,
        };
        Line { status, note }
    }

    /// A command entry, which the roots gate does not cover.
    fn pathless(status: Status, note: String) -> Self {
        Line { status, note }
    }

    fn render(&self, command: &str) -> String {
        match &self.status {
            Status::Clean => format!("\n{}{command}\n", comment(&self.note)),
            Status::Keep => format!(
                "\n{}# {command}\n",
                comment(&format!("keep: {}", self.note))
            ),
            Status::OutsideRoots | Status::Conflict(_) => {
                format!("\n{}# {command}\n", comment(&self.note))
            }
        }
    }
}

/// An argv as one line that runs exactly that argv, reading no input:
/// `( exec -- <quoted words> ) </dev/null`, or, with an added environment,
/// `( exec -- /usr/bin/env NAME=value … <quoted words> ) </dev/null`.
///
/// As arguments to `exec`, the words are never read as a builtin, a function, a reserved
/// word, a job, or an assignment. The subshell keeps `exec` from replacing the script.
fn command_line(argv: &Argv) -> String {
    let assignments = argv.envs().iter().map(|(name, value)| {
        let mut assignment = name.clone();
        assignment.push("=");
        assignment.push(value);
        assignment
    });
    let env_prefix = match argv.envs().is_empty() {
        true => Vec::new(),
        false => std::iter::once(ENV_PROGRAM.into())
            .chain(assignments)
            .collect(),
    };

    let words = env_prefix
        .iter()
        .map(|word| word.as_os_str())
        .chain(std::iter::once(argv.program()))
        .chain(argv.args().iter().map(|arg| arg.as_os_str()))
        .map(|word| shell::quote_word(&word.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    format!("( exec -- {words} ) </dev/null")
}

/// `-x` keeps `rm` inside the file system the path is on, so a volume mounted inside a
/// matched folder is left alone.
fn delete_argv(path: &Path) -> Argv {
    Argv::new("rm").arg("-rfx").arg("--").arg(path)
}

fn rules_text(rules: &[String]) -> String {
    shell::comment_text(&rules.join(", "))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::{Arc, Mutex, Weak};

    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::fs::ShellCommand;
    use crate::plan::{
        AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, Report, RunAs,
        Size, tool_command,
    };
    use crate::tier::{Tier, Twin};

    const HOSTILE: &str = "/w/it's \"$HOME\"`id`\nrm -rf ~ #";

    fn found(path: &str, reach: Reach, tier: Tier) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(2_000_000),
            run_as: RunAs::User,
            reach,
            rule: "rosie/node".to_owned(),
            tier,
        }
    }

    fn plan() -> Plan {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let paths = [
            found(HOSTILE, Reach::InRoots, Tier::Normal),
            found(&format!("/x{HOSTILE}"), Reach::OutsideRoots, Tier::Normal),
            found(&format!("/y{HOSTILE}"), Reach::InRoots, Tier::Aggressive),
        ];
        for found in paths {
            builder.add_path(found).expect("valid");
        }
        builder
            .add_launch_job(
                found(
                    "/Library/LaunchDaemons/com.x.plist",
                    Reach::InRoots,
                    Tier::Normal,
                ),
                LaunchDomain::System,
            )
            .expect("valid");
        builder.add_tool(
            "rosie/docker",
            &Twin::Normal(tool_command("docker system prune --force")),
        );
        builder.add_tool(
            "rosie/tool",
            &Twin::Aggressive(tool_command("tool clean --dir=/tmp/x$y")),
        );
        builder
            .add_receipt("com.x.pkg", Tier::Normal)
            .expect("valid");
        builder.add_report(
            Report::new(
                "login item X".to_owned(),
                vec!["Remove X\nrm -rf ~".to_owned()],
            )
            .expect("valid"),
        );
        builder.build().plan
    }

    /// The lines that run a program: everything that is neither blank, a comment, nor
    /// the `main` function that holds the entries.
    fn executable_lines(script: &str) -> Vec<&str> {
        let wrapper = ["main() {", ":", "}", "main \"$@\"; exit"];
        script
            .lines()
            .skip(1)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter(|line| !wrapper.contains(line))
            .collect()
    }

    /// A `PATH` folder of stub programs, one per program the export of [`plan`] runs.
    /// Each appends its name to the file `$LOG`. The `docker` stub also reads its input a
    /// line at a time, logging each line as `read <line>`, and stops two bytes into the
    /// line after `# keep:`, just past the `# ` that comments out the next command.
    ///
    /// Tests running at the same time share one folder: macOS pays a first-execution cost
    /// per fresh binary, so writing this folder anew in every test dominated their
    /// runtime. The last test holding it removes it, so no run leaves it behind.
    fn stubs() -> Arc<TempDir> {
        static SHARED: Mutex<Weak<TempDir>> = Mutex::new(Weak::new());

        let mut shared = SHARED.lock().expect("stub folder lock");
        if let Some(live) = shared.upgrade() {
            return live;
        }

        let fresh = Arc::new(build_stubs());
        *shared = Arc::downgrade(&fresh);
        fresh
    }

    fn build_stubs() -> TempDir {
        let bin = tempfile::tempdir().expect("temp folder");
        let logs_name = "#!/bin/bash\nprintf '%s\\n' \"${0##*/}\" >>\"$LOG\"\n";
        let reads_input = "while IFS= read -r line; do\n\
                           \x20 printf 'read %s\\n' \"$line\" >>\"$LOG\"\n\
                           \x20 case $line in '# keep:'*) read -r -n 2 _; exit ;; esac\n\
                           done\n";
        for program in ["launchctl", "rm", "docker", "tool", "pkgutil"] {
            let body = match program {
                "docker" => format!("{logs_name}{reads_input}"),
                _ => logs_name.to_owned(),
            };
            let path = bin.path().join(program);
            std::fs::write(&path, body).expect("write stub");
            let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
            std::fs::set_permissions(&path, executable).expect("make stub executable");
        }
        bin
    }

    /// Runs `/bin/bash` with `args`, writing `input` to its stdin, `PATH` holding only
    /// `bin`, and returns what the stubs logged. The log lives outside `bin`, which
    /// tests share, so concurrent runs never race on the same log file.
    fn run_bash(args: &[&Path], input: &[u8], bin: &Path) -> String {
        use std::io::Write;
        use std::process::Stdio;

        let log_dir = tempfile::tempdir().expect("temp folder");
        let log = log_dir.path().join("log");
        std::fs::write(&log, "").expect("create log");
        let mut child = Command::new("/bin/bash")
            .args(args)
            .env_clear()
            .env("PATH", bin)
            .env("LOG", &log)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run /bin/bash");
        let written = child.stdin.take().expect("stdin").write_all(input);
        // Bash may finish without reading all of its input; that is not a failure here.
        if let Err(error) = written
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            panic!("write bash's stdin: {error}");
        }
        child.wait().expect("wait for /bin/bash");

        std::fs::read_to_string(&log).expect("read log")
    }

    const CLEAN_PROGRAMS: &str = "launchctl\nrm\nrm\ndocker\npkgutil\n";

    /// `rosie scan --sh | sudo bash` feeds the script on stdin. A `clean` tool that reads
    /// its input must not consume the script and make bash resume inside a commented-out
    /// line.
    #[test]
    fn piped_to_bash_only_clean_entries_run() {
        let bin = stubs();

        let log = run_bash(
            &[],
            plan().to_sh().expect("no conflicts").as_bytes(),
            bin.path(),
        );

        assert_eq!(log, CLEAN_PROGRAMS);
    }

    /// No exported command reads the input of the bash that runs the script.
    #[test]
    fn exported_commands_read_no_input() {
        let bin = stubs();
        let script = bin.path().join("exported-commands-read-no-input.sh");
        std::fs::write(&script, plan().to_sh().expect("no conflicts")).expect("write script");

        let log = run_bash(&[&script], b"y\ny\n", bin.path());

        assert_eq!(log, CLEAN_PROGRAMS);
    }

    /// A script cut short, as when rosie is killed while printing it into a pipe, runs
    /// none of its entries.
    #[test]
    fn a_truncated_script_runs_nothing() {
        let bin = stubs();
        let script = plan().to_sh().expect("no conflicts");
        let first_command = script.find("( exec -- launchctl").expect("a bootout");
        let line_end = first_command + script[first_command..].find('\n').expect("line end");

        let log = run_bash(&[], &script.as_bytes()[..=line_end], bin.path());

        assert_eq!(log, "");
    }

    #[test]
    fn only_clean_entries_are_executable_in_run_order() {
        let script = plan().to_sh().expect("no conflicts");

        assert_eq!(
            executable_lines(&script),
            [
                "( exec -- launchctl bootout system /Library/LaunchDaemons/com.x.plist ) </dev/null"
                    .to_owned(),
                "( exec -- rm -rfx -- /Library/LaunchDaemons/com.x.plist ) </dev/null".to_owned(),
                format!("( exec -- rm -rfx -- {} ) </dev/null", shell::quote_word(HOSTILE)),
                "( exec -- /bin/bash -c 'docker system prune --force' ) </dev/null".to_owned(),
                "( exec -- pkgutil --forget com.x.pkg ) </dev/null".to_owned(),
            ],
            "{script}"
        );
    }

    #[test]
    fn keep_and_outside_roots_entries_are_commented_out() {
        let script = plan().to_sh().expect("no conflicts");

        assert!(
            script.contains(&format!(
                "\n# ( exec -- rm -rfx -- {} ) </dev/null\n",
                shell::quote_word(&format!("/x{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "\n# ( exec -- rm -rfx -- {} ) </dev/null\n",
                shell::quote_word(&format!("/y{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script
                .contains("\n# ( exec -- /bin/bash -c 'tool clean --dir=/tmp/x$y' ) </dev/null\n"),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "\n# to allow just it:\n\
                 #   rosie roots add {}\n\
                 # or to allow its folder:\n\
                 #   rosie roots add /x/w\n",
                shell::quote_word(&format!("/x{HOSTILE}"))
            )),
            "{script}"
        );
    }

    #[test]
    fn is_valid_bash() {
        // A plan with nothing to run leaves `main` holding comments alone.
        for plan in [plan(), Plan::default()] {
            let script = plan.to_sh().expect("no conflicts");

            let output = Command::new("/bin/bash")
                .arg("-n")
                .arg("-c")
                .arg(&script)
                .env_clear()
                .output()
                .expect("run /bin/bash");

            assert!(output.status.success(), "{script}");
        }
    }

    #[test]
    fn refuses_a_plan_with_an_unresolved_conflict() {
        let mut builder = PlanBuilder::new(AggressiveItems::Clean);
        for (rule, tier) in [("rosie/a", Tier::Normal), ("alice/b", Tier::Aggressive)] {
            let found = PathMatch {
                rule: rule.to_owned(),
                ..found("/w/a", Reach::InRoots, tier)
            };
            builder.add_path(found).expect("valid");
        }

        let refused = builder.build().plan.to_sh();

        assert!(
            matches!(refused, Err(Error::UnresolvedConflicts { count: 1 })),
            "{refused:?}"
        );
    }

    #[test]
    fn a_plist_inside_a_clean_folder_is_booted_out_before_the_folder_goes() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        builder
            .add_launch_job(
                found("/w/app/com.x.plist", Reach::InRoots, Tier::Aggressive),
                LaunchDomain::Gui(501),
            )
            .expect("valid");
        builder
            .add_path(found("/w/app", Reach::InRoots, Tier::Normal))
            .expect("valid");

        let script = builder.build().plan.to_sh().expect("no conflicts");

        assert_eq!(
            executable_lines(&script),
            [
                "( exec -- launchctl bootout gui/501 /w/app/com.x.plist ) </dev/null",
                "( exec -- rm -rfx -- /w/app ) </dev/null"
            ],
            "{script}"
        );
    }

    /// What bash prints for a command's exported line, run with `PATH` holding only
    /// `bin`.
    fn run_exported_line(words: &[&str], bin: &Path) -> std::process::Output {
        let (program, args) = words.split_first().expect("a program");
        let argv = args
            .iter()
            .fold(Argv::new(program), |argv, arg| argv.arg(arg));
        run_line(&command_line(&argv), bin)
    }

    fn run_line(line: &str, bin: &Path) -> std::process::Output {
        Command::new("/bin/bash")
            .arg("-c")
            .arg(line)
            .env_clear()
            .env("PATH", bin)
            .output()
            .expect("run /bin/bash")
    }

    /// A tool's exported line hands its `cmd` whole to its shell, with its environment.
    #[test]
    fn an_exported_tool_runs_its_cmd_through_its_shell_with_its_env() {
        let bin = tempfile::tempdir().expect("temp folder");
        let tool = ShellCommand::new(
            "/bin/sh".to_owned(),
            "printf '%s|%s' \"$GREETING\" \"$(echo inner)\"".to_owned(),
            [("GREETING".to_owned(), "it's \"here\" $HOME".to_owned())].into(),
        )
        .expect("valid");

        let output = run_line(&command_line(&tool.argv()), bin.path());

        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"it's \"here\" $HOME|inner");
    }

    /// A `PATH` folder whose only program is a link named `program` to
    /// `/usr/bin/printf`, which prints the remaining words through the format `%s\0`.
    fn printf_named(program: &str) -> tempfile::TempDir {
        let bin = tempfile::tempdir().expect("temp folder");
        std::os::unix::fs::symlink("/usr/bin/printf", bin.path().join(program))
            .expect("link printf");
        bin
    }

    /// Bash builtins, reserved words, job specs, and assignments are all words bash reads
    /// as something other than a program to look up on `PATH` when they start a command.
    #[test]
    fn exported_commands_run_their_program_with_their_args() {
        let programs = [
            "eval", "source", "exec", "cd", "trap", "time", "if", "!", "{", "%", "%1", "%a=b",
            "FOO=1", "PATH=bin",
        ];
        let args = ["a b", "=c", "%d", "$(echo EVALUATED)", ";echo EVALUATED"];

        for program in programs {
            let bin = printf_named(program);
            let words = [program, r"%s\0"].into_iter().chain(args);

            let output = run_exported_line(&words.collect::<Vec<_>>(), bin.path());

            assert!(output.status.success(), "program {program:?}: {output:?}");
            assert_eq!(
                output.stdout, b"a b\0=c\0%d\0$(echo EVALUATED)\0;echo EVALUATED\0",
                "program {program:?}"
            );
        }
    }

    /// No file can be named `.`, so the exported line finds no program to run, and the
    /// file named after it is never sourced.
    #[test]
    fn an_exported_dot_command_sources_nothing() {
        let bin = tempfile::tempdir().expect("temp folder");
        let script = bin.path().join("script");
        std::fs::write(&script, "echo SOURCED\n").expect("write script");

        let output = run_exported_line(&[".", &script.to_string_lossy()], bin.path());

        assert!(!output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"", "{output:?}");
    }
}
