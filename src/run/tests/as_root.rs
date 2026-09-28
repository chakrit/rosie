//! The elevated child's deletes, acting as root.

use super::*;

/// Runs `plan` as root, as the elevated child does.
fn run_as_root(fake: &FakeBackend, roots: &[&str], plan: &Plan) -> Recorder {
    let mut recorder = Recorder::default();
    Runner::as_root(gate(fake, roots)).execute(plan, &mut recorder);
    recorder
}

/// A machine run as root, where every folder is root-owned `0755` unless a test
/// changes it, with one file inside `item`.
fn root_machine(item: &str) -> FakeBackend {
    root_tree(item, "x")
}

/// A machine run as root holding the file `inside` below `item`. `item` and every
/// folder above it are root-owned `0755`; folders between `item` and the file are the
/// user's, as fixtures are, unless a test changes them.
fn root_tree(item: &str, inside: &str) -> FakeBackend {
    let fake = FakeBackend::running_as(ROOT_UID);
    fake.add_file(format!("{item}/{inside}"), "x");
    fake.chown_with_ancestors(item, ROOT_UID);
    respond_ps(&fake, "");
    fake
}

fn mutated(fake: &FakeBackend) -> bool {
    fake.calls().iter().any(|call| {
        matches!(
            call,
            Call::RemoveFile(_) | Call::RemoveEmptyDir(_) | Call::SetMode(..)
        )
    })
}

fn assert_left_as_unsafe(fake: &FakeBackend, recorder: &Recorder, item: &str, ancestor: &str) {
    assert_refused_at(fake, recorder, item, ancestor, &format!("{item}/x"));
}

/// The item is skipped as unsafe to elevate because of `folder`, before anything is
/// removed or has its permissions changed; `kept` is still there.
fn assert_refused_at(
    fake: &FakeBackend,
    recorder: &Recorder,
    item: &str,
    folder: &str,
    kept: &str,
) {
    let result = result_for(recorder, item);
    assert_eq!(
        result.outcome,
        Outcome::Skipped(SkipReason::UnsafeToElevate)
    );
    assert!(
        result.reason.starts_with(&format!("{folder} ")),
        "the reason names {folder}: {}",
        result.reason
    );
    assert!(result.reason.ends_with("delete it manually"));
    assert!(fake.exists(kept));
    assert!(!mutated(fake), "{:?}", fake.calls());
}

// the folders above the item

#[test]
fn elevated_refuses_an_item_below_a_user_owned_folder() {
    let item = "/Users/me/Library/Caches/root";
    let fake = root_machine(item);
    fake.chown(HOME, USER_UID);

    let recorder = run_as_root(
        &fake,
        &["/Users/me/Library/Caches"],
        &plan(&sudo_folder(item, 3)),
    );

    assert_left_as_unsafe(&fake, &recorder, item, HOME);
}

#[test]
fn elevated_deletes_an_item_below_root_owned_unwritable_folders() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_eq!(result_for(&recorder, item).outcome, Outcome::Done);
    assert!(!fake.exists(item));
}

#[test]
fn elevated_refuses_an_item_below_a_group_writable_folder() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);
    fake.chmod("/Library/Caches", 0o775);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_left_as_unsafe(&fake, &recorder, item, "/Library/Caches");
}

#[test]
fn elevated_refuses_an_item_below_an_other_writable_folder() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);
    fake.chmod("/Library", 0o757);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_left_as_unsafe(&fake, &recorder, item, "/Library");
}

#[test]
fn elevated_passes_a_sticky_folder_above_a_root_owned_parent() {
    let item = "/private/tmp/root/item";
    let fake = root_machine(item);
    fake.chmod("/private/tmp", 0o1777);

    let recorder = run_as_root(&fake, &["/private/tmp"], &plan(&sudo_folder(item, 3)));

    assert_eq!(result_for(&recorder, item).outcome, Outcome::Done);
    assert!(!fake.exists(item));
}

#[test]
fn elevated_refuses_an_item_whose_parent_is_sticky_and_writable() {
    let item = "/private/tmp/item";
    let fake = root_machine(item);
    fake.chmod("/private/tmp", 0o1777);

    let recorder = run_as_root(&fake, &["/private/tmp"], &plan(&sudo_folder(item, 3)));

    assert_left_as_unsafe(&fake, &recorder, item, "/private/tmp");
}

#[test]
fn the_users_own_run_deletes_below_folders_others_can_write() {
    let item = "/Users/me/Library/Caches/mine";
    let fake = FakeBackend::new();
    fake.add_file(format!("{item}/x"), "x");
    fake.chmod("/Users/me/Library", 0o777);
    respond_ps(&fake, "");

    let (stats, _) = run(
        &fake,
        &["/Users/me/Library/Caches"],
        &plan(&folder(item, 3, "ticked")),
    );

    assert_eq!(stats.done, tally(1, 3, 0));
    assert!(!fake.exists(item));
}

#[test]
fn elevated_refuses_an_item_below_a_symlink() {
    let fake = root_machine("/Library/real/item");
    fake.add_symlink("/Library/link", "/Library/real");
    fake.chown_with_ancestors("/Library/link", ROOT_UID);
    let item = "/Library/link/item";

    let recorder = run_as_root(&fake, &["/Library"], &plan(&sudo_folder(item, 3)));

    let result = result_for(&recorder, item);
    assert_eq!(
        result.outcome,
        Outcome::Skipped(SkipReason::UnsafeToElevate)
    );
    assert!(result.reason.starts_with("/Library/link is a symlink"));
    assert!(!mutated(&fake));
}

#[test]
fn elevated_fails_an_item_whose_folders_cannot_be_checked() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);
    fake.fail_on("/Library", Op::Lstat, ErrorKind::PermissionDenied);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    let result = result_for(&recorder, item);
    assert_eq!(result.outcome, Outcome::Failed);
    assert!(
        result
            .reason
            .starts_with("cannot check the folders above it")
    );
    assert!(!mutated(&fake));
}

// the item and every folder inside it

#[test]
fn elevated_refuses_an_item_that_is_itself_user_owned() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);
    fake.chown(item, USER_UID);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_left_as_unsafe(&fake, &recorder, item, item);
}

#[test]
fn elevated_refuses_an_item_others_can_write() {
    let item = "/Library/Caches/root";
    let fake = root_machine(item);
    fake.chmod(item, 0o777);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_left_as_unsafe(&fake, &recorder, item, item);
}

#[test]
fn elevated_refuses_a_user_owned_folder_inside_the_item_before_entering_it() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "sub/junk");

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    let sub = format!("{item}/sub");
    assert_refused_at(&fake, &recorder, item, &sub, &format!("{sub}/junk"));
    assert!(!fake.calls().contains(&Call::ReadDir(PathBuf::from(&sub))));
}

#[test]
fn elevated_leaves_a_closed_user_owned_folder_inside_the_item_unopened() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "sub/junk");
    let sub = format!("{item}/sub");
    fake.chmod(&sub, 0o077);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_refused_at(&fake, &recorder, item, &sub, &format!("{sub}/junk"));
}

#[test]
fn elevated_refuses_a_root_owned_folder_inside_the_item_others_can_write() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "sub/junk");
    let sub = format!("{item}/sub");
    fake.chown(&sub, ROOT_UID);
    fake.chmod(&sub, 0o777);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_refused_at(&fake, &recorder, item, &sub, &format!("{sub}/junk"));
}

#[test]
fn elevated_refuses_a_sticky_folder_inside_the_item_others_can_write() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "sub/junk");
    let sub = format!("{item}/sub");
    fake.chown(&sub, ROOT_UID);
    fake.chmod(&sub, 0o1777);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_refused_at(&fake, &recorder, item, &sub, &format!("{sub}/junk"));
}

#[test]
fn elevated_checks_folders_at_every_depth_inside_the_item() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "a/b/junk");
    fake.chown(format!("{item}/a"), ROOT_UID);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    let b = format!("{item}/a/b");
    assert_refused_at(&fake, &recorder, item, &b, &format!("{b}/junk"));
}

#[test]
fn elevated_deletes_a_root_owned_tree_without_changing_permissions() {
    let item = "/Library/Caches/root";
    let fake = root_tree(item, "a/b/junk");
    fake.chown(format!("{item}/a"), ROOT_UID);
    fake.chown(format!("{item}/a/b"), ROOT_UID);
    fake.chmod(format!("{item}/a/b"), 0o500);

    let recorder = run_as_root(&fake, &["/Library/Caches"], &plan(&sudo_folder(item, 3)));

    assert_eq!(result_for(&recorder, item).outcome, Outcome::Done);
    assert!(!fake.exists(item));
    let calls = fake.calls();
    assert!(!calls.iter().any(|c| matches!(c, Call::SetMode(..))));
}

// a user-owned item elevated only for its `system` bootout

#[test]
fn elevated_boots_a_user_items_job_out_and_leaves_the_delete_to_the_user() {
    let item = "/Users/me/Library/Application Support/X";
    let plist = "/Users/me/Library/Application Support/X/com.x.helper.plist";
    let fake = FakeBackend::running_as(ROOT_UID);
    fake.add_file(plist, "<plist/>");
    respond_ps(&fake, "");
    let bootout = Argv::new("/bin/launchctl")
        .arg("bootout")
        .arg("system")
        .arg(plist);
    fake.respond(bootout.clone(), output(0, "", ""));
    let plan = plan(&format!(
        "{}[[delete.bootout]]\nplist = \"{plist}\"\ndomain = \"system\"\n",
        folder(item, 1, "ticked")
    ));

    let recorder = run_as_root(&fake, &["/Users/me/Library"], &plan);

    assert!(fake.calls().contains(&Call::Run(bootout)));
    assert!(fake.exists(plist));
    assert!(!mutated(&fake), "{:?}", fake.calls());
    assert_eq!(recorder.finished.len(), 1, "only the bootout is reported");
}
