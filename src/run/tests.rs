use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::header::{Header, Nonce};
use super::*;
use crate::fs::fake::{Call, FakeBackend, USER_UID};
use crate::fs::{Bounds, CommandOutput, Home, Op, ROOT_UID};
use crate::plan::{Size, Tally};

mod as_root;

const HOME: &str = "/Users/me";
const EXE: &str = "/Users/me/.cargo/bin/rosie";

// fixtures

fn gate<'a>(fake: &'a FakeBackend, roots: &[&str]) -> Gate<&'a FakeBackend> {
    let bounds = Bounds {
        roots: roots.iter().map(PathBuf::from).collect(),
        config_dir: PathBuf::from("/Users/me/.config/rosie"),
        data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
        user_uid: USER_UID,
    };
    Gate::new(fake, bounds).expect("absolute bounds")
}

fn elevation() -> Elevation {
    Elevation {
        exe: PathBuf::from(EXE),
        home: Home::new(Path::new(HOME)).expect("absolute home"),
    }
}

fn output(code: i32, stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

/// `ps` output with the given `pid ppid uid comm` rows.
fn respond_ps(fake: &FakeBackend, rows: &str) {
    fake.respond(ProcessTable::argv(), output(0, rows, ""));
}

fn plan(entries: &str) -> Plan {
    Plan::parse(&format!("version = 1\n{entries}")).expect("valid plan")
}

fn folder(path: &str, size: u64, status: &str) -> String {
    format!(
        "[[delete]]\npath = \"{path}\"\ntype = \"folder\"\nsize = {size}\nrules = [\"r\"]\nstatus = \"{status}\"\n"
    )
}

fn sudo_folder(path: &str, size: u64) -> String {
    format!("{}sudo = true\n", folder(path, size, "ticked"))
}

/// Records every result it is handed.
#[derive(Default)]
struct Recorder {
    finished: Vec<ItemResult>,
    windows: Vec<Vec<String>>,
}

impl Reporter for Recorder {
    fn command_started(&mut self, _argv: &Argv) {}

    fn command_output(&mut self, _argv: &Argv, window: &OutputWindow) {
        self.windows.push(window.clone().into_lines());
    }

    fn item_finished(&mut self, result: &ItemResult) {
        self.finished.push(result.clone());
    }

    fn elevating(&mut self, _items: usize) {}
}

fn run(fake: &FakeBackend, roots: &[&str], plan: &Plan) -> (RunStats, Recorder) {
    let mut recorder = Recorder::default();
    let stats = Runner::new(gate(fake, roots)).run(plan, &elevation(), &mut recorder);
    (stats, recorder)
}

fn result_for<'a>(recorder: &'a Recorder, path: &str) -> &'a ItemResult {
    recorder
        .finished
        .iter()
        .find(|result| result.subject == Subject::Path(PathBuf::from(path)))
        .unwrap_or_else(|| panic!("no result for {path}"))
}

fn tally(items: usize, bytes: u64, unknown: usize) -> Tally {
    Tally {
        items,
        size: Size::bytes(bytes),
        unknown,
    }
}

fn position(calls: &[Call], wanted: &Call) -> usize {
    calls
        .iter()
        .position(|call| call == wanted)
        .unwrap_or_else(|| panic!("{wanted:?} was not called"))
}

fn sudo_call(fake: &FakeBackend) -> (Argv, Vec<u8>) {
    fake.calls()
        .into_iter()
        .find_map(|call| match call {
            Call::Sudo(argv, stdin) => Some((argv, stdin)),
            _ => None,
        })
        .expect("sudo was called")
}

// deleting

#[test]
fn deletes_only_ticked_entries() {
    let fake = FakeBackend::new();
    fake.add_file("/w/a/node_modules/x.js", "x");
    fake.add_file("/w/b/node_modules/x.js", "x");
    respond_ps(&fake, "");
    let plan = plan(
        &(folder("/w/a/node_modules", 10, "ticked") + &folder("/w/b/node_modules", 20, "unticked")),
    );

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert!(!fake.exists("/w/a/node_modules"));
    assert!(fake.exists("/w/b/node_modules"));
    assert_eq!(stats.done, tally(1, 10, 0));
    assert_eq!(stats.freed(), Size::bytes(10));
}

#[test]
fn skips_a_missing_entry_as_stale() {
    let fake = FakeBackend::new();
    fake.add_dir("/w");
    respond_ps(&fake, "");
    let plan = plan(&folder("/w/gone", 30, "ticked"));

    let (stats, recorder) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.stale, tally(1, 30, 0));
    assert_eq!(
        result_for(&recorder, "/w/gone").outcome,
        Outcome::Skipped(SkipReason::Stale)
    );
}

#[test]
fn skips_an_entry_whose_type_changed_as_stale() {
    let fake = FakeBackend::new();
    fake.add_file("/w/target", "now a file");
    respond_ps(&fake, "");
    let plan = plan(&folder("/w/target", 40, "ticked"));

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.stale, tally(1, 40, 0));
    assert!(fake.exists("/w/target"));
}

#[test]
fn refuses_an_item_a_process_executes_from() {
    let fake = FakeBackend::new();
    fake.add_file("/w/app/target/debug/server", "bin");
    respond_ps(&fake, "  77  1  501 /w/app/target/debug/server\n");
    let plan = plan(&folder("/w/app/target", 50, "ticked"));

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.running_process, tally(1, 50, 0));
    assert!(fake.exists("/w/app/target/debug/server"));
}

#[test]
fn fails_every_delete_when_the_process_table_cannot_be_read() {
    let fake = FakeBackend::new();
    fake.add_file("/w/app/target/x", "x");
    let plan = plan(&folder("/w/app/target", 50, "ticked"));

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.failed, tally(1, 50, 0));
    assert!(fake.exists("/w/app/target/x"));
}

#[test]
fn reports_a_failed_item_and_goes_on_with_the_rest() {
    let fake = FakeBackend::new();
    fake.add_file("/w/a/target/busy", "x");
    fake.add_file("/w/b/target/free", "x");
    fake.fail_on("/w/a/target/busy", Op::RemoveFile, ErrorKind::ResourceBusy);
    respond_ps(&fake, "");
    let plan = plan(&(folder("/w/a/target", 1, "ticked") + &folder("/w/b/target", 2, "ticked")));

    let (stats, recorder) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.failed, tally(1, 1, 0));
    assert_eq!(stats.done, tally(1, 2, 0));
    assert!(stats.any_failed());
    assert!(!fake.exists("/w/b/target"));
    assert!(
        result_for(&recorder, "/w/a/target")
            .reason
            .contains("/w/a/target/busy")
    );
}

#[test]
fn checks_roots_against_the_config_never_the_plan() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/Documents/thesis.tex", "x");
    respond_ps(&fake, "");
    let plan = plan(&folder("/Users/me/Documents", 5, "ticked"));

    let (stats, _) = run(&fake, &["/Users/me/code"], &plan);

    assert_eq!(stats.failed, tally(1, 5, 0));
    assert!(fake.exists("/Users/me/Documents/thesis.tex"));
}

#[test]
fn removes_a_symlink_inside_a_target_as_a_link() {
    let fake = FakeBackend::new();
    fake.add_file("/src/lib/keep.js", "x");
    fake.add_symlink("/w/app/node_modules/lib", "/src/lib");
    respond_ps(&fake, "");
    let plan = plan(&folder("/w/app/node_modules", 1, "ticked"));

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.done.items, 1);
    assert!(!fake.exists("/w/app/node_modules"));
    assert!(fake.exists("/src/lib/keep.js"));
}

#[test]
fn makes_user_owned_read_only_folders_writable_as_it_goes() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/go/pkg/mod/x@v1/sub/go.mod", "module x");
    fake.chmod("/Users/me/go/pkg/mod/x@v1/sub", 0o555);
    fake.chmod("/Users/me/go/pkg/mod/x@v1", 0o555);
    respond_ps(&fake, "");
    let plan = plan(&folder("/Users/me/go/pkg/mod/x@v1", 1, "ticked"));

    let (stats, _) = run(&fake, &["/Users/me/go/pkg/mod"], &plan);

    assert_eq!(stats.done.items, 1);
    assert!(!fake.exists("/Users/me/go/pkg/mod/x@v1"));
}

#[test]
fn refuses_a_dataless_placeholder_without_opening_it() {
    let fake = FakeBackend::new();
    fake.add_file("/w/docs/cloud/paper.pdf", "x");
    fake.make_dataless("/w/docs/cloud");
    respond_ps(&fake, "");
    let plan = plan(&folder("/w/docs", 1, "ticked"));

    let (stats, recorder) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.failed.items, 1);
    assert!(fake.exists("/w/docs/cloud/paper.pdf"));
    assert!(
        !fake
            .calls()
            .contains(&Call::ReadDir(PathBuf::from("/w/docs/cloud")))
    );
    assert!(
        result_for(&recorder, "/w/docs")
            .reason
            .contains("placeholder")
    );
}

#[test]
fn deletes_a_large_tree_completely() {
    let fake = FakeBackend::new();
    for dir in 0..20 {
        for file in 0..20 {
            fake.add_file(format!("/w/big/d{dir}/f{file}"), "x");
        }
    }
    respond_ps(&fake, "");
    let plan = plan(&folder("/w/big", 1, "ticked"));

    let (stats, _) = run(&fake, &["/w"], &plan);

    assert_eq!(stats.done.items, 1);
    assert!(!fake.exists("/w/big"));
    assert!(fake.exists("/w"));
}

// commands

#[test]
fn runs_tool_argv_directly_keeping_the_last_three_lines() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let docker = Argv::new("docker")
        .arg("system")
        .arg("prune")
        .arg("--force");
    fake.respond(docker.clone(), output(0, "one\ntwo\nthree\n", "four\n"));
    let plan = plan(
        "[[tool]]\nrules = [\"r\"]\ncmd = [\"docker\", \"system\", \"prune\", \"--force\"]\nstatus = \"ticked\"\n",
    );

    let (stats, recorder) = run(&fake, &[], &plan);

    assert!(fake.calls().contains(&Call::Run(docker.clone())));
    assert_eq!(stats.done, tally(1, 0, 1));
    assert_eq!(
        recorder.finished[0].subject,
        Subject::Command {
            argv: docker,
            output: vec!["two".into(), "three".into(), "four".into()],
        }
    );
    assert_eq!(recorder.windows.len(), 4);
}

#[test]
fn a_tool_absent_from_the_machine_fails_normally() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let gradle = Argv::new("gradle").arg("--stop");
    let plan =
        plan("[[tool]]\nrules = [\"r\"]\ncmd = [\"gradle\", \"--stop\"]\nstatus = \"ticked\"\n");

    let (stats, recorder) = run(&fake, &[], &plan);

    assert!(fake.calls().contains(&Call::Run(gradle)));
    assert_eq!(stats.failed, tally(1, 0, 1));
    assert!(
        recorder.finished[0]
            .reason
            .contains("cannot run `gradle --stop`")
    );
}

#[test]
fn a_tool_exiting_non_zero_fails() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    fake.respond(Argv::new("brew").arg("cleanup"), output(1, "", "locked\n"));
    let plan =
        plan("[[tool]]\nrules = [\"r\"]\ncmd = [\"brew\", \"cleanup\"]\nstatus = \"ticked\"\n");

    let (stats, recorder) = run(&fake, &[], &plan);

    assert_eq!(stats.failed.items, 1);
    assert_eq!(recorder.finished[0].reason, "exited with code 1");
}

#[test]
fn boots_a_job_out_before_deleting_its_plist() {
    let fake = FakeBackend::new();
    let plist = "/Users/me/Library/LaunchAgents/com.x.plist";
    fake.add_file(plist, "<plist/>");
    respond_ps(&fake, "");
    let bootout = Argv::new("/bin/launchctl")
        .arg("bootout")
        .arg("gui/501")
        .arg(plist);
    fake.respond(bootout.clone(), output(0, "", ""));
    let plan = plan(&format!(
        "[[delete]]\npath = \"{plist}\"\ntype = \"file\"\nsize = 4096\nrules = [\"app/x\"]\nstatus = \"ticked\"\n\
         [[delete.bootout]]\nplist = \"{plist}\"\ndomain = \"gui/501\"\n"
    ));

    let (stats, _) = run(&fake, &["/Users/me/Library/LaunchAgents"], &plan);

    let calls = fake.calls();
    let booted = position(&calls, &Call::Run(bootout));
    let removed = position(&calls, &Call::RemoveFile(PathBuf::from(plist)));
    assert!(booted < removed, "bootout ran after the delete: {calls:?}");
    assert_eq!(stats.done, tally(2, 4096, 1));
}

// elevation

#[test]
fn does_the_users_items_before_elevating_once_for_the_rest() {
    let fake = FakeBackend::new();
    fake.add_file("/w/mine/x", "x");
    fake.add_file("/Library/Caches/root/x", "x");
    fake.chown("/Library/Caches/root", ROOT_UID);
    fake.respond_to_sudo(output(1, "", ""));
    respond_ps(&fake, "");
    let plan = plan(
        &(folder("/w/mine", 1, "ticked")
            + &sudo_folder("/Library/Caches/root", 2)
            + "[[forget]]\npackage = \"com.x.pkg\"\nstatus = \"ticked\"\n"),
    );

    let (stats, _) = run(&fake, &["/w", "/Library/Caches"], &plan);

    let calls = fake.calls();
    let sudo_calls = calls.iter().filter(|c| matches!(c, Call::Sudo(..))).count();
    let removed = position(&calls, &Call::RemoveEmptyDir(PathBuf::from("/w/mine")));
    let elevated = calls
        .iter()
        .position(|c| matches!(c, Call::Sudo(..)))
        .expect("sudo called");
    assert_eq!(sudo_calls, 1);
    assert!(removed < elevated);
    assert!(fake.exists("/Library/Caches/root/x"));
    assert!(
        !calls.contains(&Call::Run(
            Argv::new("/usr/sbin/pkgutil")
                .arg("--forget")
                .arg("com.x.pkg")
        ))
    );
    assert_eq!(stats.done, tally(1, 1, 0));
}

#[test]
fn does_not_elevate_for_sudo_items_that_are_not_ticked() {
    let fake = FakeBackend::new();
    fake.respond_to_sudo(output(1, "", ""));
    respond_ps(&fake, "");
    let blocked = format!("{}sudo = true\n", folder("/Library/Caches/b", 2, "blocked"));
    let plan = plan(
        &(format!(
            "{}sudo = true\n",
            folder("/Library/Caches/u", 1, "unticked")
        ) + &blocked),
    );

    let (stats, _) = run(&fake, &["/Library/Caches"], &plan);

    assert!(!fake.calls().iter().any(|c| matches!(c, Call::Sudo(..))));
    assert_eq!(stats, RunStats::default());
}

#[test]
fn pipes_the_sudo_items_behind_a_header_matching_the_argument() {
    let fake = FakeBackend::new();
    fake.add_file("/w/mine/x", "x");
    fake.respond_to_sudo(output(1, "", ""));
    respond_ps(&fake, "");
    let plan = plan(&(folder("/w/mine", 1, "ticked") + &sudo_folder("/Library/Caches/root", 2)));

    run(&fake, &["/w"], &plan);

    let (argv, stdin) = sudo_call(&fake);
    let (header, body) = Header::decode(&stdin).expect("stdin starts with a header");
    let piped = Plan::parse(body).expect("the items are a plan");
    assert_eq!(argv.program(), EXE);
    assert_eq!(argv.args()[0], elevated::SUBCOMMAND);
    assert_eq!(
        Nonce::parse(&argv.args()[1].to_string_lossy()),
        Ok(header.nonce)
    );
    assert_eq!(&*header.home, Path::new(HOME));
    let piped_paths: Vec<&Path> = piped.deletes().iter().map(|d| d.path.as_path()).collect();
    assert_eq!(piped_paths, vec![Path::new("/Library/Caches/root")]);
}

#[test]
fn skips_the_sudo_items_as_sudo_refused_when_sudo_fails() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    fake.respond_to_sudo(output(1, "", "Sorry, try again.\n"));
    let plan = plan(
        &(sudo_folder("/Library/Caches/a", 5)
            + &sudo_folder("/Library/Caches/b", 6)
            + "[[forget]]\npackage = \"com.x.pkg\"\nstatus = \"ticked\"\n"),
    );

    let (stats, recorder) = run(&fake, &["/Library/Caches"], &plan);

    assert_eq!(stats.sudo_refused, tally(3, 11, 1));
    assert!(!stats.any_failed());
    assert!(
        result_for(&recorder, "/Library/Caches/a")
            .reason
            .starts_with("sudo exited with code 1")
    );
}

#[test]
fn skips_the_sudo_items_when_sudo_cannot_start() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let plan = plan(&sudo_folder("/Library/Caches/a", 5));

    let (stats, _) = run(&fake, &["/Library/Caches"], &plan);

    assert_eq!(stats.sudo_refused, tally(1, 5, 0));
}

#[test]
fn counts_the_elevated_childs_outcomes_by_item() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let report = "rosie-outcomes 1\0\
                  outcome = \"failed\"\npath = \"/Library/Caches/b\"\n\0\
                  outcome = \"done\"\npath = \"/Library/Caches/a\"\n\0";
    fake.respond_to_sudo(output(0, report, ""));
    let plan = plan(&(sudo_folder("/Library/Caches/a", 5) + &sudo_folder("/Library/Caches/b", 6)));

    let (stats, recorder) = run(&fake, &["/Library/Caches"], &plan);

    assert_eq!(stats.done, tally(1, 5, 0));
    assert_eq!(stats.failed, tally(1, 6, 0));
    assert!(stats.any_failed());
    assert!(
        recorder.finished.is_empty(),
        "the child reports its own items"
    );
}

#[test]
fn keeps_what_a_crashed_child_reported_and_refuses_only_the_rest() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let cut_short = "rosie-outcomes 1\0\
                     outcome = \"done\"\npath = \"/Library/Caches/a\"\n\0\
                     outcome = \"done\"\npa";
    fake.respond_to_sudo(CommandOutput {
        exit: Exit::Signal(11),
        stdout: cut_short.as_bytes().to_vec(),
        stderr: Vec::new(),
    });
    let plan = plan(&(sudo_folder("/Library/Caches/a", 5) + &sudo_folder("/Library/Caches/b", 6)));

    let (stats, recorder) = run(&fake, &["/Library/Caches"], &plan);

    assert_eq!(stats.done, tally(1, 5, 0));
    assert_eq!(stats.sudo_refused, tally(1, 6, 0));
    assert_eq!(recorder.finished.len(), 1);
    assert!(
        result_for(&recorder, "/Library/Caches/b")
            .reason
            .starts_with("sudo killed by signal 11")
    );
}

const HELPER: &str = "/Users/me/Library/Application Support/X";
const HELPER_PLIST: &str = "/Users/me/Library/Application Support/X/com.x.helper.plist";

/// A user-owned folder holding the plist of a `system` launch daemon.
fn user_folder_with_system_plist() -> (FakeBackend, Plan) {
    let fake = FakeBackend::new();
    fake.add_file(HELPER_PLIST, "<plist/>");
    respond_ps(&fake, "");
    let plan = plan(&format!(
        "{}[[delete.bootout]]\nplist = \"{HELPER_PLIST}\"\ndomain = \"system\"\n",
        folder(HELPER, 1, "ticked")
    ));
    (fake, plan)
}

/// What the elevated child reports for the helper's bootout.
fn helper_bootout_report(outcome: &str) -> String {
    format!(
        "rosie-outcomes 1\0outcome = \"{outcome}\"\n\
         command = [\"/bin/launchctl\", \"bootout\", \"system\", \"{HELPER_PLIST}\"]\n\0"
    )
}

#[test]
fn pipes_a_user_folder_holding_a_system_plist_for_its_bootout() {
    let (fake, plan) = user_folder_with_system_plist();
    fake.respond_to_sudo(output(1, "", ""));

    run(&fake, &["/Users/me/Library"], &plan);

    let (_, stdin) = sudo_call(&fake);
    let (_, body) = Header::decode(&stdin).expect("header");
    let piped = Plan::parse(body).expect("plan");
    assert_eq!(piped.deletes()[0].path, PathBuf::from(HELPER));
    assert_eq!(piped.deletes()[0].bootouts.len(), 1);
}

#[test]
fn deletes_a_user_folder_itself_after_the_elevated_child_boots_its_job_out() {
    let (fake, plan) = user_folder_with_system_plist();
    fake.respond_to_sudo(output(0, &helper_bootout_report("done"), ""));

    let (stats, recorder) = run(&fake, &["/Users/me/Library"], &plan);

    let calls = fake.calls();
    let elevated = calls
        .iter()
        .position(|c| matches!(c, Call::Sudo(..)))
        .expect("sudo called");
    let removed = position(&calls, &Call::RemoveFile(PathBuf::from(HELPER_PLIST)));
    assert!(elevated < removed, "deleted before the bootout: {calls:?}");
    assert!(!fake.exists(HELPER));
    assert_eq!(result_for(&recorder, HELPER).outcome, Outcome::Done);
    assert_eq!(stats.done, tally(2, 1, 1));
}

#[test]
fn deletes_a_user_folder_whose_system_bootout_ran_and_failed() {
    let (fake, plan) = user_folder_with_system_plist();
    fake.respond_to_sudo(output(0, &helper_bootout_report("failed"), ""));

    let (stats, _) = run(&fake, &["/Users/me/Library"], &plan);

    assert!(!fake.exists(HELPER));
    assert_eq!(stats.done, tally(1, 1, 0));
    assert_eq!(stats.failed, tally(1, 0, 1));
}

#[test]
fn keeps_a_user_folder_whose_system_bootout_never_ran() {
    let (fake, plan) = user_folder_with_system_plist();
    fake.respond_to_sudo(output(1, "", ""));

    let (stats, recorder) = run(&fake, &["/Users/me/Library"], &plan);

    assert!(fake.exists(HELPER_PLIST));
    assert_eq!(
        result_for(&recorder, HELPER).outcome,
        Outcome::Skipped(SkipReason::SudoRefused)
    );
    assert_eq!(stats.sudo_refused, tally(2, 1, 1));
}

#[test]
fn refuses_a_report_naming_items_it_was_not_sent() {
    let fake = FakeBackend::new();
    respond_ps(&fake, "");
    let report = "rosie-outcomes 1\0\
                  outcome = \"done\"\npath = \"/Library/Caches/a\"\n\0\
                  outcome = \"done\"\npath = \"/etc\"\n\0";
    fake.respond_to_sudo(output(0, report, ""));
    let plan = plan(&sudo_folder("/Library/Caches/a", 5));

    let (stats, _) = run(&fake, &["/Library/Caches"], &plan);

    assert_eq!(stats.done, tally(0, 0, 0));
    assert_eq!(stats.sudo_refused, tally(1, 5, 0));
}

#[test]
fn keeps_a_user_folder_when_the_elevated_child_never_launches() {
    let (fake, plan) = user_folder_with_system_plist();
    // A home holding a newline fails `Header::encode`, so `sudo` is never even called.
    let never_launches = Elevation {
        exe: PathBuf::from(EXE),
        home: Home::new(Path::new("/Users/me\nnonce x")).expect("absolute home"),
    };

    let mut recorder = Recorder::default();
    let stats =
        Runner::new(gate(&fake, &["/Users/me/Library"])).run(&plan, &never_launches, &mut recorder);

    assert!(!fake.calls().iter().any(|c| matches!(c, Call::Sudo(..))));
    assert!(fake.exists(HELPER_PLIST));
    assert!(fake.exists(HELPER));
    assert_eq!(
        result_for(&recorder, HELPER).outcome,
        Outcome::Skipped(SkipReason::SudoRefused)
    );
    assert_eq!(stats.sudo_refused, tally(2, 1, 1));
}
