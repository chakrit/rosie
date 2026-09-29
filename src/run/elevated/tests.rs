use std::path::{Path, PathBuf};

use super::*;
use crate::fs::fake::{Call, FakeBackend};
use crate::fs::{Argv, CommandOutput, Exit};
use crate::plan::{Outcome, Size, Tally};
use crate::run::outcomes::{Key, Record};
use crate::run::{ItemResult, OutputWindow};

const HOME: &str = "/Users/me";
const NONCE: &str = "0123456789abcdef0123456789abcdef";
const PID: u32 = 901;
const SUDO_CHAIN: &str = "\
    1     0     0 /sbin/launchd
  800     1   501 /bin/zsh
  900   800     0 /usr/bin/sudo
  901   900     0 /Users/me/.cargo/bin/rosie
";
const PLIST: &str = "/Library/LaunchDaemons/com.x.helper.plist";

#[derive(Default)]
struct Recorder {
    finished: Vec<ItemResult>,
}

impl Reporter for Recorder {
    fn command_started(&mut self, _argv: &Argv) {}

    fn command_output(&mut self, _argv: &Argv, _window: &OutputWindow) {}

    fn item_finished(&mut self, result: &ItemResult) {
        self.finished.push(result.clone());
    }

    fn elevating(&mut self, _items: usize) {}
}

fn output(stdout: &str) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(0),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

/// A machine where rosie runs as root under sudo, with the user's config allowing
/// `/Library/LaunchDaemons` and a loaded daemon's plist in it.
fn machine(uid: &str, ps: &str) -> FakeBackend {
    let fake = FakeBackend::running_as(ROOT_UID);
    fake.add_file(
        "/Users/me/.config/rosie/config.toml",
        "roots = [\"/Library/LaunchDaemons\"]\n",
    );
    fake.add_file(PLIST, "<plist/>");
    fake.chown(PLIST, ROOT_UID);
    fake.add_file("/Library/Caches/other/x", "x");
    fake.chown_with_ancestors(PLIST, ROOT_UID);
    fake.chown_with_ancestors("/Library/Caches/other", ROOT_UID);
    fake.respond(
        Argv::new("/usr/bin/id").arg("-u"),
        output(&format!("{uid}\n")),
    );
    fake.respond(
        Argv::new("/bin/ps")
            .arg("-axo")
            .arg("pid=,ppid=,uid=,comm="),
        output(ps),
    );
    fake.respond(forget(), output(""));
    fake.respond(bootout(), output(""));
    fake
}

fn bootout() -> Argv {
    Argv::new("/bin/launchctl")
        .arg("bootout")
        .arg("system")
        .arg(PLIST)
}

fn forget() -> Argv {
    Argv::new("/usr/sbin/pkgutil")
        .arg("--forget")
        .arg("com.x.pkg")
}

fn items() -> String {
    format!(
        "version = 1\n\
         [[delete]]\npath = \"{PLIST}\"\ntype = \"file\"\nsize = 4096\nrules = [\"app/x\"]\nstatus = \"ticked\"\nsudo = true\n\
         [[delete.bootout]]\nplist = \"{PLIST}\"\ndomain = \"system\"\n\
         [[delete]]\npath = \"/Library/Caches/other\"\ntype = \"folder\"\nsize = 7\nrules = [\"r\"]\nstatus = \"ticked\"\nsudo = true\n"
    )
}

fn piped(nonce: &str, home: &str) -> Stdin {
    let header = Header {
        nonce: Nonce::parse(nonce).expect("valid nonce"),
        home: Home::new(Path::new(home)).expect("absolute home"),
    };
    Stdin::Pipe(header.encode(&items()).expect("encodable"))
}

/// `plan` behind a valid header.
fn pipe_plan(plan: &str) -> Stdin {
    let header = Header {
        nonce: Nonce::parse(NONCE).expect("valid nonce"),
        home: Home::new(Path::new(HOME)).expect("absolute home"),
    };
    Stdin::Pipe(header.encode(plan).expect("encodable"))
}

fn invocation(stdin: Stdin) -> Invocation {
    Invocation {
        nonce: NONCE.to_owned(),
        stdin,
        pid: PID,
    }
}

fn elevate(fake: &FakeBackend, invocation: Invocation) -> (Result<RunStats, Error>, Vec<u8>) {
    let mut out = Vec::new();
    let result = run_elevated(&fake, invocation, &mut Recorder::default(), &mut out);
    (result, out)
}

fn acted(fake: &FakeBackend) -> bool {
    fake.calls().iter().any(|call| {
        matches!(
            call,
            Call::RemoveFile(_) | Call::RemoveEmptyDir(_) | Call::SetMode(..)
        ) || *call == Call::Run(bootout())
    })
}

// admitted

#[test]
fn runs_the_piped_items_as_root_within_the_users_roots() {
    let fake = machine("0", SUDO_CHAIN);

    let (stats, out) = elevate(&fake, invocation(piped(NONCE, HOME)));

    let stats = stats.expect("admitted");
    assert!(!fake.exists(PLIST));
    assert!(
        fake.exists("/Library/Caches/other/x"),
        "outside the user's roots"
    );
    assert_eq!(
        stats.done,
        Tally {
            items: 2,
            size: Size::bytes(4096),
            unknown: 1,
        }
    );
    assert_eq!(stats.failed.items, 1);
    let reported = outcomes::decode(&out).expect("a report for the run");
    assert_eq!(reported.len(), 3);
    assert!(reported.contains(&Record {
        key: Key::Path(PathBuf::from("/Library/Caches/other")),
        outcome: Outcome::Failed,
    }));
}

#[test]
fn runs_system_tools_by_absolute_path() {
    let fake = machine("0", SUDO_CHAIN);
    let plan = format!(
        "{}[[forget]]\npackage = \"com.x.pkg\"\nstatus = \"ticked\"\n",
        items()
    );

    elevate(&fake, invocation(pipe_plan(&plan)))
        .0
        .expect("admitted");

    let calls = fake.calls();
    let ran = |argv: Argv| calls.contains(&Call::Run(argv));
    assert!(ran(Argv::new("/usr/bin/id").arg("-u")));
    assert!(ran(Argv::new("/bin/ps")
        .arg("-axo")
        .arg("pid=,ppid=,uid=,comm=")));
    assert!(ran(bootout()));
    assert!(ran(forget()));
}

#[test]
fn reports_each_item_to_the_run_before_the_next_one_acts() {
    let fake = machine("0", SUDO_CHAIN);
    let mut out = Snapshots {
        fake: &fake,
        writes: Vec::new(),
    };

    run_elevated(
        &fake,
        invocation(piped(NONCE, HOME)),
        &mut Recorder::default(),
        &mut out,
    )
    .expect("admitted");

    let calls = fake.calls();
    let removed = calls
        .iter()
        .position(|c| *c == Call::RemoveFile(PathBuf::from(PLIST)))
        .expect("removed");
    let bootout_reported = out
        .writes
        .iter()
        .find(|(_, text)| text.contains("/bin/launchctl"))
        .map(|(calls_before, _)| *calls_before)
        .expect("the bootout is reported");
    assert!(
        bootout_reported <= removed,
        "reported only after the delete"
    );
}

/// Records how many backend calls had happened at each write.
struct Snapshots<'a> {
    fake: &'a FakeBackend,
    writes: Vec<(usize, String)>,
}

impl Write for Snapshots<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let calls = self.fake.calls().len();
        self.writes
            .push((calls, String::from_utf8_lossy(bytes).into_owned()));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn boots_the_job_out_before_deleting_its_plist() {
    let fake = machine("0", SUDO_CHAIN);

    elevate(&fake, invocation(piped(NONCE, HOME)))
        .0
        .expect("admitted");

    let calls = fake.calls();
    let booted = calls.iter().position(|c| *c == Call::Run(bootout()));
    let removed = calls
        .iter()
        .position(|c| *c == Call::RemoveFile(PathBuf::from(PLIST)));
    assert!(booted.expect("booted out") < removed.expect("removed"));
}

#[test]
fn never_opens_the_users_read_only_folders_as_root() {
    let fake = machine("0", SUDO_CHAIN);
    fake.add_file("/Library/LaunchDaemons/mine/sub/x", "x");
    fake.chown("/Library/LaunchDaemons/mine", ROOT_UID);
    fake.chmod("/Library/LaunchDaemons/mine/sub", 0o555);
    let plan = format!(
        "{}[[delete]]\npath = \"/Library/LaunchDaemons/mine\"\ntype = \"folder\"\nsize = 1\nrules = [\"r\"]\nstatus = \"ticked\"\nsudo = true\n",
        items()
    );
    let header = Header {
        nonce: Nonce::parse(NONCE).expect("valid"),
        home: Home::new(Path::new(HOME)).expect("absolute home"),
    };
    let stdin = Stdin::Pipe(header.encode(&plan).expect("encodable"));

    elevate(&fake, invocation(stdin)).0.expect("admitted");

    assert!(fake.exists("/Library/LaunchDaemons/mine/sub/x"));
    assert!(!fake.calls().iter().any(|c| matches!(c, Call::SetMode(..))));
}

// the four gates

#[test]
fn refuses_unless_running_as_root() {
    let fake = machine("501", SUDO_CHAIN);

    let (result, out) = elevate(&fake, invocation(piped(NONCE, HOME)));

    assert!(matches!(result, Err(Error::NotRoot { uid: 501 })));
    assert!(!acted(&fake));
    assert!(out.is_empty());
}

#[test]
fn refuses_without_a_sudo_ancestor() {
    let chain = "    1     0     0 /sbin/launchd\n  800     1   0 /bin/zsh\n  901   800     0 /Users/me/.cargo/bin/rosie\n";
    let fake = machine("0", chain);

    let (result, _) = elevate(&fake, invocation(piped(NONCE, HOME)));

    assert!(matches!(result, Err(Error::NoSudoAncestor)));
    assert!(!acted(&fake));
}

#[test]
fn refuses_a_sudo_ancestor_not_running_as_root() {
    let chain = "    1     0     0 /sbin/launchd\n  800     1   501 /bin/zsh\n  900   800   501 /usr/bin/sudo\n  901   900     0 /Users/me/.cargo/bin/rosie\n";
    let fake = machine("0", chain);

    let (result, _) = elevate(&fake, invocation(piped(NONCE, HOME)));

    assert!(matches!(result, Err(Error::NoSudoAncestor)));
    assert!(!acted(&fake));
}

#[test]
fn refuses_piped_tool_commands_outright() {
    let fake = machine("0", SUDO_CHAIN);
    let docker = Argv::new("docker").arg("system").arg("prune");
    fake.respond(docker.clone(), output(""));
    let plan = format!(
        "{}[[tool]]\nrules = [\"r\"]\ncmd = [\"docker\", \"system\", \"prune\"]\nstatus = \"ticked\"\n",
        items()
    );

    let (result, out) = elevate(&fake, invocation(pipe_plan(&plan)));

    assert!(matches!(result, Err(Error::ToolElevated)));
    assert!(!acted(&fake));
    assert!(!fake.calls().contains(&Call::Run(docker)));
    assert!(out.is_empty());
}

#[test]
fn refuses_a_stdin_that_is_not_a_pipe() {
    let fake = machine("0", SUDO_CHAIN);

    let (result, _) = elevate(&fake, invocation(Stdin::NotPipe));

    assert!(matches!(result, Err(Error::NotAPipe)));
    assert!(!acted(&fake));
}

#[test]
fn refuses_a_nonce_that_does_not_match_the_argument() {
    let fake = machine("0", SUDO_CHAIN);
    let other = "ffffffffffffffffffffffffffffffff";

    let (result, _) = elevate(&fake, invocation(piped(other, HOME)));

    assert!(matches!(result, Err(Error::NonceMismatch)));
    assert!(!acted(&fake));
}

#[test]
fn refuses_a_header_of_another_version() {
    let fake = machine("0", SUDO_CHAIN);
    let text = format!(
        "rosie-elevated 9\nnonce {NONCE}\nhome {HOME}\n\n{}",
        items()
    );

    let (result, _) = elevate(&fake, invocation(Stdin::Pipe(text.into_bytes())));

    assert!(matches!(
        result,
        Err(Error::Header(header::Error::Version { .. }))
    ));
    assert!(!acted(&fake));
}

// stdin

#[test]
fn captures_a_pipe_to_its_end() {
    let (reader, mut writer) = io::pipe().expect("pipe");
    writer.write_all(b"piped items").expect("write");
    drop(writer);

    let stdin = Stdin::capture(&reader).expect("capture");

    assert_eq!(stdin, Stdin::Pipe(b"piped items".to_vec()));
}

#[test]
fn leaves_a_regular_file_unread() {
    let mut file = tempfile::tempfile().expect("temp file");
    file.write_all(b"not a pipe").expect("write");

    let stdin = Stdin::capture(&file).expect("capture");

    assert_eq!(stdin, Stdin::NotPipe);
}
