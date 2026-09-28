//! The `--sh` export: the plan as a bash script the user inspects and runs with sudo
//! themselves (`docs/spec/plan.md#shell-script-export`). Rosie cannot run it.
//!
//! - Every path and argument is quoted with [`quote::word`], so none can spill onto a
//!   line of its own, and every command is written as `( exec -- <words> ) </dev/null`,
//!   so bash runs exactly its argv.
//! - The entries sit inside a `main` function that the last line calls. Bash reads the
//!   whole script before running any of it, so a script piped to bash and cut short runs
//!   nothing, and a command that reads its input cannot consume the rest of the script
//!   and make bash resume inside a commented-out line. No command reads bash's input.
//! - Unticked and blocked entries are commented out; report-only entries are comments.

use std::path::Path;

use super::{Plan, Status, blocked_hint, quote};
use crate::fs::Argv;

/// `:` keeps `main` valid bash when every entry in it is commented out.
const HEADER: &str = "\
#!/bin/bash
# rosie plan, exported as a shell script. rosie cannot run this file.
# Inspect every line, then run it yourself: sudo bash path/to/this-file.sh
# Unticked and blocked entries are commented out.

main() {
:
";

/// On one line, so bash has read the call and the `exit` before `main` runs.
const FOOTER: &str = "}

main \"$@\"; exit
";

impl Plan {
    /// Bootouts come first, so every launch job is unloaded before its plist is deleted.
    pub fn to_sh(&self) -> String {
        let mut script = HEADER.to_owned();

        for delete in &self.deletes {
            for bootout in &delete.bootouts {
                let note = format!(
                    "unload {}",
                    quote::comment_text(&bootout.domain.to_string())
                );
                let line = Line::of(delete.status, &delete.path, note);
                script.push_str(&line.render(&command_line(&bootout.argv())));
            }
        }
        for delete in &self.deletes {
            let note = format!("{} · {}", delete.size, rules_text(&delete.rules));
            let line = Line::of(delete.status, &delete.path, note);
            script.push_str(&line.render(&command_line(&delete_argv(&delete.path))));
        }
        for tool in &self.tools {
            let note = format!("size unknown · {}", rules_text(&tool.rules));
            let line = Line::unblocked(tool.selection.into(), note);
            script.push_str(&line.render(&command_line(&tool.command.argv())));
        }
        for receipt in &self.receipts {
            let note = "size unknown · forget receipt".to_owned();
            let line = Line::unblocked(receipt.selection.into(), note);
            script.push_str(&line.render(&command_line(&receipt.argv())));
        }
        for report in &self.reports {
            script.push_str(&format!(
                "\n# report only, do by hand: {}\n",
                quote::comment_text(report.subject())
            ));
            for step in report.steps() {
                script.push_str(&format!("#   {}\n", quote::comment_text(step)));
            }
        }

        script.push_str(FOOTER);
        script
    }
}

/// One entry's comment and command, commented out unless the entry is ticked.
struct Line {
    status: Status,
    note: String,
}

impl Line {
    /// An entry with a path, whose blocked note carries the `rosie roots add` hint.
    fn of(status: Status, path: &Path, note: String) -> Self {
        let note = match status {
            Status::Blocked => format!("{note}; {}", blocked_hint(path)),
            Status::Ticked | Status::Unticked => note,
        };
        Line { status, note }
    }

    /// A command entry, which the roots gate does not cover.
    fn unblocked(status: Status, note: String) -> Self {
        Line { status, note }
    }

    fn render(&self, command: &str) -> String {
        match self.status {
            Status::Ticked => format!("\n# {}\n{command}\n", self.note),
            Status::Unticked => format!("\n# unticked: {}\n# {command}\n", self.note),
            Status::Blocked => format!("\n# {}\n# {command}\n", self.note),
        }
    }
}

/// An argv as one line that runs exactly that argv, reading no input:
/// `( exec -- <quoted words> ) </dev/null`.
///
/// As arguments to `exec`, the words are never read as a builtin, a function, a reserved
/// word, a job, or an assignment, so the program is always the one `PATH` finds. The
/// subshell keeps `exec` from replacing the script.
fn command_line(argv: &Argv) -> String {
    let words = std::iter::once(argv.program())
        .chain(argv.args().iter().map(|arg| arg.as_os_str()))
        .map(|word| quote::word(&word.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    format!("( exec -- {words} ) </dev/null")
}

/// `-x` keeps `rm` inside the file system the path is on, as rosie's own walk does: a
/// volume mounted inside a matched folder is left alone.
fn delete_argv(path: &Path) -> Argv {
    Argv::new("rm").arg("-rfx").arg("--").arg(path)
}

fn rules_text(rules: &[String]) -> String {
    quote::comment_text(&rules.join(", "))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;
    use crate::plan::{
        AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, Report, RunAs,
        Size, ToolCmds, Twin, quote,
    };

    const HOSTILE: &str = "/w/it's \"$HOME\"`id`\nrm -rf ~ #";

    fn found(path: &str, reach: Reach, twin: Twin) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(2_000_000),
            run_as: RunAs::User,
            reach,
            rule: "rosie/node".to_owned(),
            twin,
        }
    }

    fn plan() -> Plan {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let paths = [
            found(HOSTILE, Reach::InRoots, Twin::Normal),
            found(&format!("/x{HOSTILE}"), Reach::OutsideRoots, Twin::Normal),
            found(&format!("/y{HOSTILE}"), Reach::InRoots, Twin::Aggressive),
        ];
        for found in paths {
            builder.add_path(found).expect("valid");
        }
        builder
            .add_launch_job(
                found(
                    "/Library/LaunchDaemons/com.x.plist",
                    Reach::InRoots,
                    Twin::Normal,
                ),
                LaunchDomain::System,
            )
            .expect("valid");
        builder
            .add_tool(
                "rosie/docker",
                ToolCmds::Normal("docker system prune --force"),
            )
            .expect("valid");
        builder
            .add_tool(
                "rosie/tool",
                ToolCmds::Aggressive("tool clean --dir=/tmp/x$y"),
            )
            .expect("valid");
        builder
            .add_receipt("com.x.pkg", Twin::Normal)
            .expect("valid");
        builder.add_report(
            Report::new(
                "login item X".to_owned(),
                vec!["Remove X\nrm -rf ~".to_owned()],
            )
            .expect("valid"),
        );
        builder.build()
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
    /// line after `# unticked:`, just past the `# ` that comments out the next command.
    fn stubs() -> tempfile::TempDir {
        let bin = tempfile::tempdir().expect("temp folder");
        let logs_name = "#!/bin/bash\nprintf '%s\\n' \"${0##*/}\" >>\"$LOG\"\n";
        let reads_input = "while IFS= read -r line; do\n\
                           \x20 printf 'read %s\\n' \"$line\" >>\"$LOG\"\n\
                           \x20 case $line in '# unticked:'*) read -r -n 2 _; exit ;; esac\n\
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
    /// `bin`, and returns what the stubs logged.
    fn run_bash(args: &[&Path], input: &[u8], bin: &Path) -> String {
        use std::io::Write;
        use std::process::Stdio;

        let log = bin.join("log");
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

    const TICKED_PROGRAMS: &str = "launchctl\nrm\nrm\ndocker\npkgutil\n";

    /// `rosie scan --sh | sudo bash` feeds the script on stdin. A ticked tool that reads
    /// its input must not consume the script and make bash resume inside a commented-out
    /// line.
    #[test]
    fn piped_to_bash_only_ticked_entries_run() {
        let bin = stubs();

        let log = run_bash(&[], plan().to_sh().as_bytes(), bin.path());

        assert_eq!(log, TICKED_PROGRAMS);
    }

    /// No exported command reads the input of the bash that runs the script.
    #[test]
    fn exported_commands_read_no_input() {
        let bin = stubs();
        let script = bin.path().join("plan.sh");
        std::fs::write(&script, plan().to_sh()).expect("write script");

        let log = run_bash(&[&script], b"y\ny\n", bin.path());

        assert_eq!(log, TICKED_PROGRAMS);
    }

    /// A script cut short, as when rosie is killed while printing it into a pipe, runs
    /// none of its entries.
    #[test]
    fn a_truncated_script_runs_nothing() {
        let bin = stubs();
        let script = plan().to_sh();
        let first_command = script.find("( exec -- launchctl").expect("a bootout");
        let line_end = first_command + script[first_command..].find('\n').expect("line end");

        let log = run_bash(&[], &script.as_bytes()[..=line_end], bin.path());

        assert_eq!(log, "");
    }

    #[test]
    fn only_ticked_entries_are_executable_in_run_order() {
        let script = plan().to_sh();

        assert_eq!(
            executable_lines(&script),
            [
                "( exec -- launchctl bootout system /Library/LaunchDaemons/com.x.plist ) </dev/null"
                    .to_owned(),
                "( exec -- rm -rfx -- /Library/LaunchDaemons/com.x.plist ) </dev/null".to_owned(),
                format!("( exec -- rm -rfx -- {} ) </dev/null", quote::word(HOSTILE)),
                "( exec -- docker system prune --force ) </dev/null".to_owned(),
                "( exec -- pkgutil --forget com.x.pkg ) </dev/null".to_owned(),
            ],
            "{script}"
        );
    }

    #[test]
    fn unticked_and_blocked_entries_are_commented_out() {
        let script = plan().to_sh();

        assert!(
            script.contains(&format!(
                "\n# ( exec -- rm -rfx -- {} ) </dev/null\n",
                quote::word(&format!("/x{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "\n# ( exec -- rm -rfx -- {} ) </dev/null\n",
                quote::word(&format!("/y{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script.contains("\n# ( exec -- tool clean '--dir=/tmp/x$y' ) </dev/null\n"),
            "{script}"
        );
        assert!(script.contains("rosie roots add "), "{script}");
    }

    #[test]
    fn is_valid_bash() {
        // A plan with nothing to run leaves `main` holding comments alone.
        for plan in [plan(), Plan::default()] {
            let script = plan.to_sh();

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
    fn a_plist_inside_a_ticked_folder_is_booted_out_before_the_folder_goes() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        builder
            .add_launch_job(
                found("/w/app/com.x.plist", Reach::InRoots, Twin::Aggressive),
                LaunchDomain::Gui(501),
            )
            .expect("valid");
        builder
            .add_path(found("/w/app", Reach::InRoots, Twin::Normal))
            .expect("valid");

        let script = builder.build().to_sh();

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
        let words = words.iter().copied().map(str::to_owned).collect();
        let command = crate::plan::Command::from_words(words).expect("valid");
        let line = command_line(&command.argv());

        Command::new("/bin/bash")
            .arg("-c")
            .arg(&line)
            .env_clear()
            .env("PATH", bin)
            .output()
            .expect("run /bin/bash")
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
