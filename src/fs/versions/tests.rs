use super::*;
use crate::fs::fake::{Call, FakeBackend};

fn printed(text: &str) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(0),
        stdout: text.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn probe(tool: SystemTool, args: &[&str]) -> Argv {
    args.iter()
        .fold(Argv::new(tool.path()), |argv, arg| argv.arg(*arg))
}

fn runs_of(fake: &FakeBackend, argv: &Argv) -> usize {
    fake.calls()
        .into_iter()
        .filter(|call| *call == Call::Run(argv.clone()))
        .count()
}

/// Runs a tool's own command through the check, ignoring what the fake answers.
fn use_tool(check: &VersionCheck<&FakeBackend>, tool: SystemTool) {
    let ran = check.run(&Argv::new(tool.path()).arg("whatever"));
    drop(ran);
}

#[test]
fn reads_each_tools_own_version_output() {
    let cases = [
        (
            VersionFormat::Lsof,
            "lsof version information:\n    revision: 4.91\n    latest revision: ftp://x\n",
            Some("4.91"),
        ),
        (
            VersionFormat::Launchctl,
            "Darwin Bootstrapper Version 7.0.0: Fri Jul 31 21:16:41 PDT 2026; root:libxpc\n",
            Some("7.0.0"),
        ),
        (
            VersionFormat::Sudo,
            "Sudo version 1.9.17p2\nSudoers policy plugin version 1.9.17p2\n",
            Some("1.9.17p2"),
        ),
        (
            VersionFormat::Bash,
            "GNU bash, version 3.2.57(1)-release (arm64-apple-darwin25)\nCopyright\n",
            Some("3.2.57"),
        ),
        (
            VersionFormat::Lsof,
            "usage: lsof [-?abhKlnNoOPRtUvVX]\n",
            None,
        ),
        (VersionFormat::Sudo, "", None),
        (
            VersionFormat::Bash,
            "zsh 5.9 (arm64-apple-darwin25)\n",
            None,
        ),
    ];

    for (format, text, expected) in cases {
        assert_eq!(format.read(text), expected, "{format:?} reading {text:?}");
    }
}

#[test]
fn a_tested_version_draws_no_warning() {
    let fake = FakeBackend::new();
    let check = VersionCheck::new(&fake);

    use_tool(&check, SystemTool::LSOF);

    assert_eq!(check.take_warnings(), Vec::new());
    assert_eq!(runs_of(&fake, &probe(SystemTool::LSOF, &["-v"])), 1);
}

#[test]
fn an_untested_version_warns_once_however_often_the_tool_runs() {
    let fake = FakeBackend::new();
    fake.respond(
        probe(SystemTool::LSOF, &["-v"]),
        printed("lsof version information:\n    revision: 4.99\n"),
    );
    let check = VersionCheck::new(&fake);

    use_tool(&check, SystemTool::LSOF);
    use_tool(&check, SystemTool::LSOF);

    let expected = UntestedVersion::Tool {
        tool: "/usr/sbin/lsof",
        found: "4.99".to_owned(),
        tested: "4.91",
    };
    assert_eq!(check.take_warnings(), vec![expected]);
    assert_eq!(runs_of(&fake, &probe(SystemTool::LSOF, &["-v"])), 1);
}

#[test]
fn tools_without_a_version_warn_by_the_macos_version_read_once() {
    let fake = FakeBackend::new();
    let sw_vers = probe(SystemTool::SW_VERS, &["-productVersion"]);
    fake.respond(sw_vers.clone(), printed("27.0\n"));
    let check = VersionCheck::new(&fake);

    use_tool(&check, SystemTool::PS);
    use_tool(&check, SystemTool::PLUTIL);
    use_tool(&check, SystemTool::PS);

    let warned_for = |tool: &'static str| UntestedVersion::MacOs {
        tool,
        found: "27.0".to_owned(),
    };
    assert_eq!(
        check.take_warnings(),
        vec![warned_for("/bin/ps"), warned_for("/usr/bin/plutil")]
    );
    assert_eq!(runs_of(&fake, &sw_vers), 1);
}

#[test]
fn only_the_tools_a_process_runs_are_checked() {
    let fake = FakeBackend::new();
    let check = VersionCheck::new(&fake);

    use_tool(&check, SystemTool::LSOF);

    let probed_sudo = runs_of(&fake, &probe(SystemTool::SUDO, &["-V"]));
    let probed_macos = runs_of(&fake, &probe(SystemTool::SW_VERS, &["-productVersion"]));
    assert_eq!((probed_sudo, probed_macos), (0, 0));
}

#[test]
fn a_tool_rules_own_shell_is_not_checked() {
    let fake = FakeBackend::new();
    let check = VersionCheck::new(&fake);

    let ran = check.run(&Argv::new("/opt/homebrew/bin/fish").arg("-c").arg("true"));
    drop(ran);

    let probes = fake
        .calls()
        .into_iter()
        .filter(
            |call| matches!(call, Call::Run(argv) if argv.program() != "/opt/homebrew/bin/fish"),
        )
        .count();
    assert_eq!(probes, 0);
}

#[test]
fn sudo_checks_sudo_and_an_unreadable_version_warns() {
    let fake = FakeBackend::new();
    fake.respond(
        probe(SystemTool::SUDO, &["-V"]),
        printed("usage: sudo -h\n"),
    );
    let check = VersionCheck::new(&fake);

    let ran = check.sudo(&Argv::new("/x/rosie"), b"");
    drop(ran);

    let warnings = check.take_warnings();
    assert!(
        matches!(
            warnings.as_slice(),
            [UntestedVersion::Unreadable {
                tool: "/usr/bin/sudo",
                ..
            }]
        ),
        "{warnings:?}"
    );
}
