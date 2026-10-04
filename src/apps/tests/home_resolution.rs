//! The home in `Paths` is resolved once where it is made, so a `..`-spelled home plans the
//! same as the home it resolves to.

use super::*;

fn dotted_home() -> Paths {
    Paths::new(Path::new("/Users/me/../me")).expect("absolute paths")
}

#[test]
fn a_dotted_home_plans_the_same_app_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("Caches/com.foo.Bar/data"), "x");
    quiet(&fake);

    let plain = scan_app(
        &ctx(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &paths(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("plain paths");
    let dotted = scan_app(
        &ctx(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &dotted_home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("dotted paths");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_plans_the_same_orphan_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_file(lib("Caches/com.gone.App/data"), "x");

    let plain = scan_orphans(
        &ctx(&fake, &ALL_ROOTS),
        &paths(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("plain paths");
    let dotted = scan_orphans(
        &ctx(&fake, &ALL_ROOTS),
        &dotted_home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("dotted paths");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_lists_the_same_picker_apps_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);

    let plain = picker_apps(&ctx(&fake, &ALL_ROOTS), &paths()).expect("plain paths");
    let dotted = picker_apps(&ctx(&fake, &ALL_ROOTS), &dotted_home()).expect("dotted paths");

    assert_eq!(dotted, plain);
}
