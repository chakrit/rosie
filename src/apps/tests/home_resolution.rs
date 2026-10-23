//! The home in `Paths` is resolved once where it is made, so a `..`-spelled home plans the
//! same as the home it resolves to.

use super::*;

const DOTTED_HOME: &str = "/Users/me/../me";

#[test]
fn a_dotted_home_plans_the_same_app_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("Caches/com.foo.Bar/data"), "x");
    quiet(&fake);

    let plain = AppScanner::new(&ctx(&fake, &ALL_ROOTS))
        .scan(Path::new(BAR), &settings(AggressiveItems::Skip))
        .expect("plain paths");
    let dotted = AppScanner::new(&ctx_at_home(&fake, &ALL_ROOTS, DOTTED_HOME))
        .scan(Path::new(BAR), &settings(AggressiveItems::Skip))
        .expect("dotted paths");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_plans_the_same_orphan_scan_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_file(lib("Caches/com.gone.App/data"), "x");

    let plain = scan_orphans(&ctx(&fake, &ALL_ROOTS), &settings(AggressiveItems::Skip))
        .expect("plain paths");
    let dotted = scan_orphans(
        &ctx_at_home(&fake, &ALL_ROOTS, DOTTED_HOME),
        &settings(AggressiveItems::Skip),
    )
    .expect("dotted paths");

    assert_eq!(deletes(&dotted), deletes(&plain));
}

#[test]
fn a_dotted_home_lists_the_same_picker_apps_as_the_plain_home() {
    let fake = FakeBackend::new();
    installed_suite(&fake);

    let plain = picker_apps(&ctx(&fake, &ALL_ROOTS)).expect("plain paths");
    let dotted = picker_apps(&ctx_at_home(&fake, &ALL_ROOTS, DOTTED_HOME)).expect("dotted paths");

    assert_eq!(dotted, plain);
}
