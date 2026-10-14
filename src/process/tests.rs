use super::*;
use crate::fs::fake::{FakeBackend, USER_UID};
use crate::fs::{CommandOutput, Exit, Paths};
use std::path::Path;

const PS: &[u8] = b"\
    1     0     0 /sbin/launchd
  310     1   501 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  400   310   501 node
  500     1     0 /usr/bin/sudo
  501   500     0 /Users/me/.cargo/bin/rosie
";

fn pids(processes: &[&Process]) -> Vec<u32> {
    processes.iter().map(|p| p.pid).collect()
}

// parsing

#[test]
fn parses_every_field_and_a_command_holding_spaces() {
    let text = b"  355     1   501 /Applications/Foo Bar.app/Contents/MacOS/Foo Bar\n\
                 \n    1     0     0 /sbin/launchd\n";

    let table = ProcessTable::parse(text).expect("valid ps output");

    assert_eq!(
        table.processes,
        vec![
            Process {
                pid: 355,
                ppid: 1,
                uid: 501,
                comm: PathBuf::from("/Applications/Foo Bar.app/Contents/MacOS/Foo Bar"),
            },
            Process {
                pid: 1,
                ppid: 0,
                uid: 0,
                comm: PathBuf::from("/sbin/launchd"),
            },
        ]
    );
}

/// macOS `ps` prints the uid of a user outside `int32` range, such as `nobody`
/// (4294967294), as a negative number: real output seen starting `dhcp6d` on a stock
/// Mac.
#[test]
fn a_negative_uid_is_read_as_its_unsigned_value() {
    let table = ProcessTable::parse(b"51253     1    -2 /usr/libexec/dhcp6d\n")
        .expect("ps prints negative uids for `nobody`");

    assert_eq!(table.processes[0].uid, 4294967294);
}

#[test]
fn keeps_a_command_that_is_not_utf8_byte_for_byte() {
    let table = ProcessTable::parse(b"    7     1   501 /Applications/Caf\xe9.app/x\n")
        .expect("ps output that is not UTF-8");

    let cafe = Path::new(OsStr::from_bytes(b"/Applications/Caf\xe9.app/x"));
    assert_eq!(table.processes[0].comm, cafe);
}

#[test]
fn refuses_a_line_without_every_field() {
    let parsed = ProcessTable::parse(b"  355     1   501\n");

    assert!(
        matches!(&parsed, Err(Error::Unreadable { line, .. }) if line.contains("355")),
        "{parsed:?}"
    );
}

#[test]
fn refuses_a_line_whose_command_is_blank() {
    let parsed = ProcessTable::parse(b"  355     1   501 \n");

    assert!(
        matches!(&parsed, Err(Error::Unreadable { line, .. }) if line.contains("355")),
        "{parsed:?}"
    );
}

#[test]
fn refuses_a_non_numeric_id() {
    for line in [
        "  abc     1   501 /bin/zsh\n",
        "  355   abc   501 /bin/zsh\n",
        "  355     1   abc /bin/zsh\n",
    ] {
        let parsed = ProcessTable::parse(line.as_bytes());

        assert!(matches!(parsed, Err(Error::Unreadable { .. })), "{line:?}");
    }
}

// ancestry

#[test]
fn walks_the_parent_chain_nearest_first() {
    let table = ProcessTable::parse(PS).expect("valid ps output");

    assert_eq!(pids(&table.ancestors(501)), vec![500, 1]);
}

#[test]
fn stops_a_looping_parent_chain() {
    let table = ProcessTable::parse(b"  7  8  0 a\n  8  7  0 b\n").expect("valid");

    assert_eq!(pids(&table.ancestors(7)), vec![8]);
}

// querying

fn ctx(fake: &FakeBackend) -> FsContext<&FakeBackend> {
    let paths = Paths::new(Path::new("/Users/me")).expect("absolute home");
    FsContext::new(fake, paths.bounds(Vec::new(), USER_UID)).expect("absolute bounds")
}

fn output(exit: i32, stdout: &[u8]) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(exit),
        stdout: stdout.to_vec(),
        stderr: b"ps: broken\n".to_vec(),
    }
}

/// The fake answers by this same argv, so only this test pins the columns `parse`
/// reads.
#[test]
fn ps_is_asked_for_pid_ppid_uid_and_comm_without_headers() {
    assert_eq!(
        ProcessTable::argv().to_string(),
        "/bin/ps -axo pid=,ppid=,uid=,comm="
    );
}

#[test]
fn queries_ps_through_the_context() {
    let fake = FakeBackend::new();
    fake.respond(ProcessTable::argv(), output(0, PS));

    let table = ProcessTable::query(&ctx(&fake)).expect("canned ps");

    assert_eq!(pids(&table.ancestors(400)), vec![310, 1]);
}

#[test]
fn a_ps_that_cannot_be_run_fails_the_query() {
    let fake = FakeBackend::new();

    let result = ProcessTable::query(&ctx(&fake));

    assert!(
        matches!(result, Err(Error::Fs(fs::Error::Command { .. }))),
        "{result:?}"
    );
}

#[test]
fn a_failing_ps_fails_the_query() {
    let fake = FakeBackend::new();
    fake.respond(ProcessTable::argv(), output(1, PS));

    let result = ProcessTable::query(&ctx(&fake));

    assert!(
        matches!(&result, Err(Error::Fs(fs::Error::ToolFailed { stderr, .. })) if stderr == "ps: broken"),
        "{result:?}"
    );
}

#[test]
fn reads_the_effective_uid() {
    let fake = FakeBackend::new();
    fake.respond(Argv::new("/usr/bin/id").arg("-u"), output(0, b"0\n"));

    let uid = effective_uid(&ctx(&fake)).expect("canned id");

    assert_eq!(uid, 0);
}
