//! Unpacking pulled pack tarballs: only regular files and folders are accepted
//! (`docs/spec/safety.md#rosies-own-data`).

mod tarball;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rosie::packs::{Archive, Error, Refusal};
use tar::EntryType;
use tarball::{TOP, Tarball};

fn rule_names(archive: &Archive) -> Vec<&OsStr> {
    archive.rules().iter().map(|rule| rule.name()).collect()
}

fn refusal_of(result: Result<Archive, Error>) -> (PathBuf, Refusal) {
    match result {
        Err(Error::Unsafe { path, refusal }) => (path, refusal),
        other => panic!("expected an unsafe-entry refusal, got {other:?}"),
    }
}

#[test]
fn takes_the_rules_folder_files_and_the_root_config() {
    let gz = Tarball::pack(&[("node.toml", "[rules.a]\n"), ("rust.toml", "[rules.b]\n")])
        .file(&format!("{TOP}/README.md"), b"# rosie")
        .dir(&format!("{TOP}/src"))
        .file(&format!("{TOP}/src/main.rs"), b"fn main() {}")
        .file(&format!("{TOP}/rules/notes.md"), b"not a rule file")
        .dir(&format!("{TOP}/rules/nested"))
        .file(&format!("{TOP}/rules/nested/deep.toml"), b"[rules.c]\n")
        .gzip();

    let archive = Archive::parse(&gz).expect("well-formed pack");

    let mut names = rule_names(&archive);
    names.sort();
    assert_eq!(
        names,
        vec![OsStr::new("node.toml"), OsStr::new("rust.toml")]
    );
    let node = &archive.rules()[0];
    let node_body = match node.name() == "node.toml" {
        true => node.contents(),
        false => archive.rules()[1].contents(),
    };
    assert_eq!(node_body, b"[rules.a]\n");
    assert_eq!(archive.config().expect("root config"), "roots = []\n");
}

#[test]
fn refuses_an_absolute_entry() {
    let gz = Tarball::pack(&[("node.toml", "")])
        .file("/Users/me/.zshrc", b"pwned")
        .gzip();

    let (path, refusal) = refusal_of(Archive::parse(&gz));

    assert_eq!(refusal, Refusal::Absolute);
    assert_eq!(path, Path::new("/Users/me/.zshrc"));
}

#[test]
fn refuses_a_parent_folder_entry() {
    let gz = Tarball::pack(&[("node.toml", "")])
        .file(&format!("{TOP}/rules/../../../.zshrc"), b"pwned")
        .gzip();

    let (_, refusal) = refusal_of(Archive::parse(&gz));

    assert_eq!(refusal, Refusal::ParentDir);
}

#[test]
fn refuses_a_symlink_anywhere_in_the_archive() {
    let in_rules = Tarball::pack(&[("node.toml", "")])
        .symlink(&format!("{TOP}/rules/evil.toml"), "../../.zshrc")
        .gzip();
    let elsewhere = Tarball::pack(&[("node.toml", "")])
        .symlink(&format!("{TOP}/docs/link"), "target")
        .gzip();

    for gz in [in_rules, elsewhere] {
        let (_, refusal) = refusal_of(Archive::parse(&gz));
        assert_eq!(refusal, Refusal::Symlink);
    }
}

#[test]
fn refuses_a_symlink_followed_by_a_file_written_through_it() {
    let gz = Tarball::github()
        .symlink(&format!("{TOP}/rules"), "../../../../.ssh")
        .file(&format!("{TOP}/rules/authorized_keys.toml"), b"pwned")
        .gzip();

    let (path, refusal) = refusal_of(Archive::parse(&gz));

    assert_eq!(refusal, Refusal::Symlink);
    assert_eq!(path, PathBuf::from(format!("{TOP}/rules")));
}

#[test]
fn refuses_a_hardlink() {
    let gz = Tarball::pack(&[("node.toml", "")])
        .hardlink(&format!("{TOP}/rules/evil.toml"), "etc/passwd")
        .gzip();

    let (_, refusal) = refusal_of(Archive::parse(&gz));

    assert_eq!(refusal, Refusal::Hardlink);
}

#[test]
fn refuses_device_nodes_and_other_special_entries() {
    let cases = [
        (EntryType::Char, Refusal::Device),
        (EntryType::Block, Refusal::Device),
        (EntryType::Fifo, Refusal::Special),
    ];

    for (kind, expected) in cases {
        let gz = Tarball::pack(&[("node.toml", "")])
            .entry(kind, &format!("{TOP}/rules/dev.toml"), b"")
            .gzip();

        let (_, refusal) = refusal_of(Archive::parse(&gz));
        assert_eq!(refusal, expected, "entry type {kind:?}");
    }
}

#[test]
fn refuses_entries_outside_the_single_top_folder() {
    let gz = Tarball::pack(&[("node.toml", "")])
        .dir("other-top")
        .file("other-top/rules/extra.toml", b"")
        .gzip();

    let result = Archive::parse(&gz);

    assert!(
        matches!(&result, Err(Error::OutsideTopFolder { path }) if path == Path::new("other-top")),
        "got {result:?}"
    );
}

#[test]
fn refuses_a_rule_file_listed_twice() {
    let gz = Tarball::pack(&[("node.toml", "[rules.a]\n"), ("node.toml", "[rules.b]\n")]).gzip();

    let result = Archive::parse(&gz);

    assert!(
        matches!(result, Err(Error::Duplicate { .. })),
        "got {result:?}"
    );
}

#[test]
fn refuses_a_rule_file_too_large_to_be_a_rule_file() {
    let huge = vec![b'#'; Archive::FILE_LIMIT as usize + 1];
    let gz = Tarball::pack(&[("node.toml", "")])
        .file(&format!("{TOP}/rules/huge.toml"), &huge)
        .gzip();

    let result = Archive::parse(&gz);

    assert!(
        matches!(result, Err(Error::TooLarge { .. })),
        "got {result:?}"
    );
}

#[test]
fn refuses_an_archive_without_rules() {
    let gz = Tarball::github()
        .file(&format!("{TOP}/config.toml"), b"roots = []\n")
        .gzip();

    let result = Archive::parse(&gz);

    assert!(matches!(result, Err(Error::NoRules)), "got {result:?}");
}

#[test]
fn reports_a_missing_config() {
    let gz = Tarball::github()
        .dir(&format!("{TOP}/rules"))
        .file(&format!("{TOP}/rules/node.toml"), b"")
        .gzip();

    let archive = Archive::parse(&gz).expect("a pack without a config is still a pack");

    assert!(matches!(archive.config(), Err(Error::NoConfig)));
}

#[test]
fn refuses_bytes_that_are_not_a_gzipped_tarball() {
    let result = Archive::parse(b"<html>rate limited</html>");

    assert!(matches!(result, Err(Error::Archive(_))), "got {result:?}");
}
