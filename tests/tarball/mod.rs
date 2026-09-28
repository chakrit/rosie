//! Builds gzipped tarballs shaped like GitHub's repository archives, including
//! malicious entries a well-behaved archiver refuses to write.

// Each test crate that includes this module uses a different subset of it.
#![allow(dead_code)]

use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header};

/// The top folder GitHub puts every entry under.
pub const TOP: &str = "rosie-0123abc";

pub struct Tarball {
    builder: Builder<Vec<u8>>,
}

impl Tarball {
    /// An archive that starts like `git archive` output: a pax global header carrying
    /// the commit, then the top folder.
    pub fn github() -> Self {
        Tarball::bare()
            .entry(
                EntryType::XGlobalHeader,
                "pax_global_header",
                b"52 comment=0123abc0123abc0123abc0123abc0123abc0123\n",
            )
            .dir(TOP)
    }

    pub fn bare() -> Self {
        Tarball {
            builder: Builder::new(Vec::new()),
        }
    }

    /// A pack archive with the given `rules/` files and a root `config.toml`.
    pub fn pack(rules: &[(&str, &str)]) -> Self {
        let with_folders = Tarball::github().dir(&format!("{TOP}/rules"));
        let with_rules = rules.iter().fold(with_folders, |tarball, (name, body)| {
            tarball.file(&format!("{TOP}/rules/{name}"), body.as_bytes())
        });
        with_rules.file(&format!("{TOP}/config.toml"), b"roots = []\n")
    }

    pub fn dir(self, path: &str) -> Self {
        self.entry(EntryType::Directory, path, b"")
    }

    pub fn file(self, path: &str, contents: &[u8]) -> Self {
        self.entry(EntryType::Regular, path, contents)
    }

    pub fn symlink(self, path: &str, target: &str) -> Self {
        self.link(EntryType::Symlink, path, target)
    }

    pub fn hardlink(self, path: &str, target: &str) -> Self {
        self.link(EntryType::Link, path, target)
    }

    /// Writes the entry name into the header as raw bytes, bypassing the builder's path
    /// checks so absolute and `..` names can be written.
    pub fn entry(mut self, kind: EntryType, path: &str, contents: &[u8]) -> Self {
        let mut header = header(kind, path);
        header.set_size(contents.len() as u64);
        header.set_cksum();
        self.builder
            .append(&header, contents)
            .expect("append entry");
        self
    }

    pub fn gzip(self) -> Vec<u8> {
        let tar = self.builder.into_inner().expect("finish tar");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).expect("gzip tar");
        encoder.finish().expect("finish gzip")
    }

    fn link(mut self, kind: EntryType, path: &str, target: &str) -> Self {
        let mut header = header(kind, path);
        header.set_size(0);
        header
            .set_link_name(target)
            .expect("link target fits the header");
        header.set_cksum();
        self.builder.append(&header, &[][..]).expect("append link");
        self
    }
}

fn header(kind: EntryType, path: &str) -> Header {
    let mut header = Header::new_ustar();
    header.set_entry_type(kind);
    header.set_mode(0o644);

    let name = &mut header.as_old_mut().name;
    assert!(path.len() <= name.len(), "test paths fit a ustar name");
    name[..path.len()].copy_from_slice(path.as_bytes());
    header
}
