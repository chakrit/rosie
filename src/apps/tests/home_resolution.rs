//! `home` is resolved before it is used to build any path, so a `..`-spelled home plans
//! the same as the home it resolves to.

use super::*;

const DOTTED_HOME: &str = "/Users/me/../me";

#[test]
fn a_dotted_home_plans_the_same_app_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("Caches/com.foo.Bar/data"), "x");
    quiet(&fake);

    let plain = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        home(),
        AggressiveItems::Unticked,
    )
    .expect("plain home");
    let dotted = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        Path::new(DOTTED_HOME),
        AggressiveItems::Unticked,
    )
    .expect("dotted home");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_plans_the_same_orphan_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_file(lib("Caches/com.gone.App/data"), "x");

    let plain = scan_orphans(&gate(&fake, &ALL_ROOTS), home(), AggressiveItems::Unticked)
        .expect("plain home");
    let dotted = scan_orphans(
        &gate(&fake, &ALL_ROOTS),
        Path::new(DOTTED_HOME),
        AggressiveItems::Unticked,
    )
    .expect("dotted home");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_lists_the_same_picker_apps_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);

    let plain = picker_apps(&gate(&fake, &ALL_ROOTS), home()).expect("plain home");
    let dotted =
        picker_apps(&gate(&fake, &ALL_ROOTS), Path::new(DOTTED_HOME)).expect("dotted home");

    assert_eq!(dotted, plain);
}
