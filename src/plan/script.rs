//! The `--sh` export: the plan as a bash script the user inspects and runs with sudo
//! themselves (`docs/spec/plan.md#shell-script-export`). Rosie cannot run it.
//!
//! - Every command is one plain line of words quoted with [`shell::quote_word`], so none
//!   can spill onto a line of its own, and its first word is the program's absolute path,
//!   so bash runs exactly its argv: a word with a `/` is never read as a builtin, a
//!   function, an alias, a reserved word, or an assignment.
//! - The entries sit inside a `main` function that the last line calls. Bash reads the
//!   whole script before running any of it, so a script piped to bash and cut short runs
//!   nothing, and a command that reads its input cannot consume the rest of the script
//!   and make bash resume inside a commented-out line.
//! - `keep` and `outside-roots` entries are commented out; report-only entries are
//!   comments.
//! - A plan with an unresolved rule conflict is refused: only a TOML plan can be
//!   resolved (`docs/spec/plan.md#rule-conflicts`).

use std::path::Path;

use super::conflict::conflict_hint;
use super::{Error, Plan, Status, comment, outside_roots_hint};
use crate::fs::{Argv, SystemTool};
use crate::shell;

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

/// The programs an exported script runs for rosie's own entries, each by its absolute
/// path. A tool entry runs its rule's own shell, which is absolute too.
struct Programs<'p> {
    rm: &'p str,
    launchctl: &'p str,
    pkgutil: &'p str,
}

impl Programs<'static> {
    const SYSTEM: Programs<'static> = Programs {
        rm: "/bin/rm",
        launchctl: SystemTool::LAUNCHCTL.path(),
        pkgutil: SystemTool::PKGUTIL.path(),
    };
}

impl Plan {
    pub fn to_sh(&self) -> Result<String, Error> {
        self.script(&Programs::SYSTEM)
    }

    /// Bootouts come first, so every launch job is unloaded before its plist is deleted.
    fn script(&self, programs: &Programs) -> Result<String, Error> {
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
                let argv = bootout.argv_for(programs.launchctl);
                script.push_str(&line.render(&command_line(&argv)));
            }
        }
        for delete in self.deletes.iter() {
            let note = format!("{} · {}", delete.size, rules_text(&delete.rules));
            let line = Line::of(delete.status.clone(), &delete.path, note);
            let argv = delete_argv(programs.rm, &delete.path);
            script.push_str(&line.render(&command_line(&argv)));
        }
        for tool in &self.tools {
            let note = format!("size unknown · {}", shell::comment_text(&tool.rule));
            let line = Line::pathless(tool.selection.into(), note);
            script.push_str(&line.render(&command_line(&tool.command.argv())));
        }
        for receipt in &self.receipts {
            let note = "size unknown · forget receipt".to_owned();
            let line = Line::pathless(receipt.selection.into(), note);
            let argv = receipt.argv_for(programs.pkgutil);
            script.push_str(&line.render(&command_line(&argv)));
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

/// An argv as one plain line that runs exactly that argv: `<quoted words>`, or, with an
/// added environment, `NAME=<quoted value> … <quoted words>`.
///
/// Every program the export runs is named by its absolute path, so the first word holds
/// a `/` and bash runs it as a file, never as a builtin, a function, an alias, a reserved
/// word, or an assignment. Names are shell names, checked when the tool command was built.
fn command_line(argv: &Argv) -> String {
    let assignments = argv.envs().iter().map(|(name, value)| {
        format!(
            "{}={}",
            name.to_string_lossy(),
            shell::quote_word(&value.to_string_lossy())
        )
    });
    let words = std::iter::once(argv.program())
        .chain(argv.args().iter().map(|arg| arg.as_os_str()))
        .map(|word| shell::quote_word(&word.to_string_lossy()));

    assignments.chain(words).collect::<Vec<_>>().join(" ")
}

/// `-x` keeps `rm` inside the file system the path is on, so a volume mounted inside a
/// matched folder is left alone.
fn delete_argv(rm: &str, path: &Path) -> Argv {
    Argv::new(rm).arg("-rfx").arg("--").arg(path)
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

    /// A folder of stub programs, one per program the export of [`plan`] runs. Each
    /// appends its name to the file `$LOG`. The `docker` stub also reads its input a line
    /// at a time, logging each line as `read <line>`, and stops two bytes into the line
    /// after `# keep:`, just past the `# ` that comments out the next command.
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

    /// The export of [`plan`] with rosie's own programs swapped for the stubs in `bin`,
    /// so running it touches nothing outside the test. Its tools run `/bin/bash`, which
    /// finds the `docker` stub on `PATH`.
    fn stubbed_script(bin: &Path) -> String {
        let path = |program: &str| bin.join(program).to_str().expect("UTF-8").to_owned();
        let (rm, launchctl, pkgutil) = (path("rm"), path("launchctl"), path("pkgutil"));
        let programs = Programs {
            rm: &rm,
            launchctl: &launchctl,
            pkgutil: &pkgutil,
        };
        plan().script(&programs).expect("no conflicts")
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

        let log = run_bash(&[], stubbed_script(bin.path()).as_bytes(), bin.path());

        assert_eq!(log, CLEAN_PROGRAMS);
    }

    /// A script cut short, as when rosie is killed while printing it into a pipe, runs
    /// none of its entries.
    #[test]
    fn a_truncated_script_runs_nothing() {
        let bin = stubs();
        let script = stubbed_script(bin.path());
        let first_command = script.find(" bootout ").expect("a bootout");
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
                "/bin/launchctl bootout system /Library/LaunchDaemons/com.x.plist".to_owned(),
                "/bin/rm -rfx -- /Library/LaunchDaemons/com.x.plist".to_owned(),
                format!("/bin/rm -rfx -- {}", shell::quote_word(HOSTILE)),
                "/bin/bash -c 'docker system prune --force'".to_owned(),
                "/usr/sbin/pkgutil --forget com.x.pkg".to_owned(),
            ],
            "{script}"
        );
    }

    #[test]
    fn keep_and_outside_roots_entries_are_commented_out() {
        let script = plan().to_sh().expect("no conflicts");

        assert!(
            script.contains(&format!(
                "\n# /bin/rm -rfx -- {}\n",
                shell::quote_word(&format!("/x{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "\n# /bin/rm -rfx -- {}\n",
                shell::quote_word(&format!("/y{HOSTILE}"))
            )),
            "{script}"
        );
        assert!(
            script.contains("\n# /bin/bash -c 'tool clean --dir=/tmp/x$y'\n"),
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
                "/bin/launchctl bootout gui/501 /w/app/com.x.plist",
                "/bin/rm -rfx -- /w/app"
            ],
            "{script}"
        );
    }

    fn run_line(line: &str) -> std::process::Output {
        Command::new("/bin/bash")
            .arg("-c")
            .arg(line)
            .env_clear()
            .output()
            .expect("run /bin/bash")
    }

    /// A tool's exported line hands its `cmd` whole to its shell, with its environment.
    #[test]
    fn an_exported_tool_runs_its_cmd_through_its_shell_with_its_env() {
        let tool = ShellCommand::new(
            "/bin/sh".to_owned(),
            "printf '%s|%s' \"$GREETING\" \"$(echo inner)\"".to_owned(),
            [("GREETING".to_owned(), "it's \"here\" $HOME".to_owned())].into(),
        )
        .expect("valid");

        let output = run_line(&command_line(&tool.argv()));

        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"it's \"here\" $HOME|inner");
    }

    /// Bash builtins, reserved words, job specs, and assignments are all words bash reads
    /// as something other than a program when one starts a command. Named by an absolute
    /// path, a program with such a name runs, and its args reach it as written.
    #[test]
    fn exported_commands_run_their_program_with_their_args() {
        let names = [
            "eval",
            "source",
            "exec",
            "cd",
            "trap",
            "time",
            "if",
            "!",
            "{",
            "%",
            "%1",
            "%a=b",
            "FOO=1",
            "PATH=bin",
            "it's \"$HOME\" `id`",
        ];
        let args = ["a b", "=c", "%d", "$(echo EVALUATED)", ";echo EVALUATED"];
        let bin = tempfile::tempdir().expect("temp folder");

        for name in names {
            let program = bin.path().join(name);
            std::os::unix::fs::symlink("/usr/bin/printf", &program).expect("link printf");
            let argv = [r"%s\0"]
                .into_iter()
                .chain(args)
                .fold(Argv::new(&program), |argv, arg| argv.arg(arg));

            let output = run_line(&command_line(&argv));

            assert!(output.status.success(), "program {name:?}: {output:?}");
            assert_eq!(
                output.stdout, b"a b\0=c\0%d\0$(echo EVALUATED)\0;echo EVALUATED\0",
                "program {name:?}"
            );
        }
    }
}
