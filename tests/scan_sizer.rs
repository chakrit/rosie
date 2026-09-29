//! `Sizer` is public so another mode can size an entry it found by its own means,
//! without going through `scan::Scanner`. This builds one from outside the `scan`
//! module, the way such a caller would: the entry comes from `Entry::lstat`, the only
//! public way to get one, which refuses what must never be sized: a path through a
//! symlink, a dataless placeholder, and the root of a mounted volume.

mod scan_fixture;

use std::path::{Path, PathBuf};

use rosie::fs::{self, Op};
use rosie::plan::{Size, WalkSkip};
use rosie::scan::{Entry, Log, NoProgress, NotAnEntry, Sizer, Skipped};
use scan_fixture::{HOME, fake_home, gate};

#[test]
fn an_entry_through_a_symlink_is_refused() {
    let fake = fake_home();
    fake.add_sized_file(format!("{HOME}/elsewhere/big"), 5000);
    fake.add_dir(format!("{HOME}/Library"));
    fake.add_symlink(
        format!("{HOME}/Library/Caches"),
        format!("{HOME}/elsewhere"),
    );
    let gate = gate(&fake);

    let through = Entry::lstat(&gate, Path::new(&format!("{HOME}/Library/Caches/big")));
    let at = Entry::lstat(&gate, Path::new(&format!("{HOME}/Library/Caches")));

    for result in [through, at] {
        assert!(
            matches!(&result, Err(NotAnEntry::Fs(fs::Error::Symlink { .. }))),
            "expected a symlink refusal, got {result:?}"
        );
    }
}

#[test]
fn a_placeholder_entry_is_refused() {
    let fake = fake_home();
    fake.add_sized_file(format!("{HOME}/cloud/big"), 5000);
    fake.make_dataless(format!("{HOME}/cloud"));
    let gate = gate(&fake);

    let result = Entry::lstat(&gate, Path::new(&format!("{HOME}/cloud")));

    assert!(
        matches!(&result, Err(NotAnEntry::Placeholder { path }) if path == Path::new(&format!("{HOME}/cloud"))),
        "expected a placeholder refusal, got {result:?}"
    );
}

#[test]
fn a_mount_root_entry_is_refused_so_no_whole_volume_is_sized() {
    let fake = fake_home();
    fake.add_sized_file(format!("{HOME}/disk/big"), 5000);
    fake.mount(format!("{HOME}/disk"), 7);
    let gate = gate(&fake);

    let result = Entry::lstat(&gate, Path::new(&format!("{HOME}/disk")));

    assert!(
        matches!(&result, Err(NotAnEntry::Mount { path }) if path == Path::new(&format!("{HOME}/disk"))),
        "expected a mount refusal, got {result:?}"
    );
}

#[test]
fn a_sizer_built_from_outside_scan_measures_an_entry() {
    let fake = fake_home();
    fake.add_sized_file(format!("{HOME}/one-file"), 4096);

    let gate = gate(&fake);
    let log = Log::default();
    let sizer = Sizer::walking(&gate, &NoProgress, &log);

    let entry =
        Entry::lstat(&gate, Path::new(&format!("{HOME}/one-file"))).expect("file is present");
    let total = rayon::scope(|scope| sizer.measure(scope, entry));

    assert_eq!(total.get(), Size::bytes(4096));
}

#[test]
fn a_sizer_built_from_outside_scan_logs_a_denied_subfolder_as_a_walk_skip() {
    let fake = fake_home();
    fake.add_dir(format!("{HOME}/target/locked"));
    fake.fail_on(
        format!("{HOME}/target/locked"),
        Op::ReadDir,
        std::io::ErrorKind::PermissionDenied,
    );

    let gate = gate(&fake);
    let log = Log::default();
    let entry =
        Entry::lstat(&gate, Path::new(&format!("{HOME}/target"))).expect("folder is present");

    {
        let sizer = Sizer::walking(&gate, &NoProgress, &log);
        rayon::scope(|scope| sizer.measure(scope, entry));
    }

    let (skipped, problems) = log.into_parts();
    assert_eq!(
        skipped,
        [Skipped {
            path: PathBuf::from(format!("{HOME}/target/locked")),
            reason: WalkSkip::Denied,
        }]
    );
    assert!(problems.is_empty(), "{problems:?}");
}
