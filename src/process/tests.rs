use super::*;
use crate::fs::fake::{FakeBackend, USER_UID};
use crate::fs::{Bounds, Exit};

const PS: &[u8] = b"\
    1     0     0 /sbin/launchd
  310     1   501 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  400   310   501 node
  500     1     0 /usr/bin/sudo
  501   500     0 /Users/me/.cargo/bin/rosie
";

fn process(comm: &str) -> Process {
    Process {
        pid: 1,
        ppid: 0,
        uid: USER_UID,
        comm: PathBuf::from(comm),
    }
}

fn pids(processes: &[&Process]) -> Vec<u32> {
    processes.iter().map(|p| p.pid).collect()
}

// parsing

#[test]
fn parses_every_field_and_a_command_holding_spaces() {
    let text = b"  355     1   501 /Applications/Foo Bar.app/Contents/MacOS/Foo Bar\n\
                 \n    1     0     0 /sbin/launchd\n";

    let table = ProcessTable::parse(text).expect("valid ps output");

    let foo = table
        .executing_from(Path::new("/Applications/Foo Bar.app"))
        .expect("Foo Bar runs from its bundle");
    let launchd = table
        .executing_from(Path::new("/sbin/launchd"))
        .expect("launchd is in the table");
    assert_eq!(
        (foo, launchd),
        (
            &Process {
                pid: 355,
                ppid: 1,
                uid: 501,
                comm: PathBuf::from("/Applications/Foo Bar.app/Contents/MacOS/Foo Bar"),
            },
            &Process {
                pid: 1,
                ppid: 0,
                uid: 0,
                comm: PathBuf::from("/sbin/launchd"),
            },
        )
    );
}

/// macOS `ps` prints the uid of a user outside `int32` range, such as `nobody`
/// (4294967294), as a negative number: real output seen starting `dhcp6d` on a stock
/// Mac.
#[test]
fn a_negative_uid_is_read_as_its_unsigned_value() {
    let table = ProcessTable::parse(b"51253     1    -2 /usr/libexec/dhcp6d\n")
        .expect("ps prints negative uids for `nobody`");

    let process = table
        .executing_from(Path::new("/usr/libexec/dhcp6d"))
        .expect("dhcp6d is in the table");
    assert_eq!(process.uid, 4294967294);
}

#[test]
fn keeps_a_command_that_is_not_utf8_byte_for_byte() {
    let table = ProcessTable::parse(b"    7     1   501 /Applications/Caf\xe9.app/x\n")
        .expect("ps output that is not UTF-8");

    let cafe = Path::new(OsStr::from_bytes(b"/Applications/Caf\xe9.app"));
    let lossy = Path::new("/Applications/Caf\u{FFFD}.app");
    assert_eq!(table.executing_from(cafe).map(|p| p.pid), Some(7));
    assert_eq!(table.executing_from(lossy), None);
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

// matching

#[test]
fn a_process_executes_from_its_folder_by_component() {
    let foo = process("/Applications/Foo.app/Contents/MacOS/Foo");

    assert!(foo.executes_from(Path::new("/Applications/Foo.app")));
    assert!(foo.executes_from(Path::new("/Applications/Foo.app/Contents/MacOS/Foo")));
    assert!(!foo.executes_from(Path::new("/Applications/Fo")));
    assert!(
        !process("/Applications/Foo.application/x")
            .executes_from(Path::new("/Applications/Foo.app"))
    );
}

#[test]
fn a_bare_name_matches_nothing() {
    assert!(!process("Foo").executes_from(Path::new("Foo")));
}

#[test]
fn finds_the_process_executing_from_a_path() {
    let table = ProcessTable::parse(PS).expect("valid ps output");

    let chrome = table.executing_from(Path::new("/Applications/Google Chrome.app"));
    let sibling = table.executing_from(Path::new("/Applications/Google"));
    let bare = table.executing_from(Path::new("node"));

    assert_eq!(chrome.map(|p| (p.pid, p.uid)), Some((310, 501)));
    assert_eq!(sibling, None);
    assert_eq!(bare, None);
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

fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
    let bounds = Bounds {
        roots: vec![],
        config_dir: PathBuf::from("/Users/me/.config/rosie"),
        data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
        user_uid: USER_UID,
    };
    Gate::new(fake, bounds).expect("absolute bounds")
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
fn queries_ps_through_the_gate() {
    let fake = FakeBackend::new();
    fake.respond(ProcessTable::argv(), output(0, PS));

    let table = ProcessTable::query(&gate(&fake)).expect("canned ps");

    assert_eq!(pids(&table.ancestors(400)), vec![310, 1]);
}

#[test]
fn a_ps_that_cannot_be_run_fails_the_query() {
    let fake = FakeBackend::new();

    let result = ProcessTable::query(&gate(&fake));

    assert!(
        matches!(result, Err(Error::Fs(fs::Error::Command { .. }))),
        "{result:?}"
    );
}

#[test]
fn a_failing_ps_fails_the_query() {
    let fake = FakeBackend::new();
    fake.respond(ProcessTable::argv(), output(1, PS));

    let result = ProcessTable::query(&gate(&fake));

    assert!(
        matches!(&result, Err(Error::Failed { stderr, .. }) if stderr == "ps: broken"),
        "{result:?}"
    );
}

#[test]
fn reads_the_effective_uid() {
    let fake = FakeBackend::new();
    fake.respond(Argv::new("/usr/bin/id").arg("-u"), output(0, b"0\n"));

    let uid = effective_uid(&gate(&fake)).expect("canned id");

    assert_eq!(uid, 0);
}
