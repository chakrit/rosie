//! The contract-tier harness: every case runs once against the in-memory fake and once
//! against `RealBackend` in a temp folder (`docs/spec/testing.md#fakes-and-sandboxing`).

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rosie::fs::fake::{FakeBackend, USER_UID};
use rosie::fs::{Backend, Bounds, RealBackend};
use tempfile::TempDir;

/// Declares each case as a test against both backends.
macro_rules! contract {
    ($($case:ident),* $(,)?) => {
        mod fake {
            $( #[test] fn $case() { super::$case(&$crate::common::FakeHarness::new()); } )*
        }
        mod real {
            $( #[test] fn $case() { super::$case(&$crate::common::RealHarness::new()); } )*
        }
    };
}
pub(crate) use contract;

/// A backend plus the fixtures rosie itself never creates (symlinks, hardlinks).
pub trait Harness {
    type B: Backend + Sync;

    fn backend(&self) -> &Self::B;
    /// An empty, symlink-free folder to work in.
    fn root(&self) -> &Path;
    fn user_uid(&self) -> u32;
    fn symlink(&self, link: &Path, target: &Path);
    fn hardlink(&self, existing: &Path, new: &Path);
}

pub struct FakeHarness {
    backend: FakeBackend,
    root: PathBuf,
}

impl FakeHarness {
    pub fn new() -> Self {
        let backend = FakeBackend::new();
        let root = PathBuf::from("/Users/me/sandbox");
        backend.add_dir(&root);
        FakeHarness { backend, root }
    }
}

impl Harness for FakeHarness {
    type B = FakeBackend;

    fn backend(&self) -> &FakeBackend {
        &self.backend
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn user_uid(&self) -> u32 {
        USER_UID
    }

    fn symlink(&self, link: &Path, target: &Path) {
        self.backend.add_symlink(link, target);
    }

    fn hardlink(&self, existing: &Path, new: &Path) {
        self.backend.add_hardlink(existing, new);
    }
}

pub struct RealHarness {
    _temp: TempDir,
    root: PathBuf,
    user_uid: u32,
}

impl RealHarness {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("create temp folder");
        // macOS temp folders sit under the `/var` → `/private/var` link.
        let root = temp
            .path()
            .canonicalize()
            .expect("canonicalize temp folder");
        let user_uid = RealBackend.lstat(&root).expect("lstat temp folder").uid;
        RealHarness {
            _temp: temp,
            root,
            user_uid,
        }
    }
}

impl Harness for RealHarness {
    type B = RealBackend;

    fn backend(&self) -> &RealBackend {
        &RealBackend
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn user_uid(&self) -> u32 {
        self.user_uid
    }

    fn symlink(&self, link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create symlink fixture");
    }

    fn hardlink(&self, existing: &Path, new: &Path) {
        std::fs::hard_link(existing, new).expect("create hardlink fixture");
    }
}

/// Tests leave read-only folders behind; open them again so the temp folder can be
/// removed.
impl Drop for RealHarness {
    fn drop(&mut self) {
        make_folders_writable(&self.root);
    }
}

fn make_folders_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    for entry in std::fs::read_dir(dir).expect("list a test folder") {
        let path = entry.expect("read a test folder entry").path();
        let meta = std::fs::symlink_metadata(&path).expect("lstat a test entry");
        if meta.is_dir() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("reopen a test folder for cleanup");
            make_folders_writable(&path);
        }
    }
}

pub fn sorted(mut names: Vec<OsString>) -> Vec<OsString> {
    names.sort();
    names
}

pub fn error_kind<T: std::fmt::Debug>(result: std::io::Result<T>) -> ErrorKind {
    result.expect_err("operation should fail").kind()
}

pub fn empty_bounds(h: &impl Harness) -> Bounds {
    Bounds {
        roots: Vec::new(),
        config_dir: h.root().join("config/rosie"),
        data_dir: h.root().join("data/rosie"),
        user_uid: h.user_uid(),
    }
}
