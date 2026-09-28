//! The allocated size of a planned item (`docs/spec/plan.md#stats`,
//! `docs/spec/performance.md#scan`).
//!
//! Sizes are allocated bytes, deduplicated by dev and inode across every item one
//! [`Sizer`] measures, so a hardlinked file counts once per plan. A symlink counts as
//! itself and is never followed. Sizing never crosses into another volume and never
//! opens a dataless placeholder; mounts, placeholders, and folders it may not list are
//! recorded as walk skips.

use std::collections::HashSet;
use std::path::Path;

use crate::fs::{self, Backend, FileKind, Gate, Metadata};
use crate::plan::{Size, WalkSkip, WalkSkips};

#[derive(Debug, Default)]
pub struct Sizer {
    seen: HashSet<(u64, u64)>,
}

impl Sizer {
    pub fn new() -> Self {
        Sizer::default()
    }

    /// The allocated bytes of `path`, whose own metadata is `meta`, and of everything
    /// below it that this sizer has not counted yet.
    pub fn measure<B: Backend>(
        &mut self,
        gate: &Gate<B>,
        path: &Path,
        meta: Metadata,
        skips: &mut WalkSkips,
    ) -> Result<Size, fs::Error> {
        let mut size = self.own_size(meta);
        if meta.kind != FileKind::Dir {
            return Ok(size);
        }
        if meta.is_dataless() {
            skips.record(WalkSkip::Placeholder);
            return Ok(size);
        }

        let names = match gate.read_dir(path) {
            Ok(names) => names,
            // The folder itself vanished between its `lstat` and this listing: it holds
            // nothing now, the same as a folder gone before its `lstat` ever ran.
            Err(error) if error.has_vanished() => return Ok(size),
            Err(error) if error.is_permission_denied() => {
                skips.record(WalkSkip::Denied);
                return Ok(size);
            }
            Err(error) => return Err(error),
        };
        for name in names {
            let child = path.join(name);
            let child_meta = match gate.lstat(&child) {
                Ok(meta) => meta,
                Err(error) if error.has_vanished() => continue,
                Err(error) => return Err(error),
            };
            if child_meta.dev != meta.dev {
                skips.record(WalkSkip::Mount);
                continue;
            }
            size += self.measure(gate, &child, child_meta, skips)?;
        }
        Ok(size)
    }

    /// The entry's own allocated bytes, or nothing when its inode was counted already.
    fn own_size(&mut self, meta: Metadata) -> Size {
        match self.seen.insert((meta.dev, meta.inode)) {
            true => Size::bytes(meta.allocated),
            false => Size::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::PathBuf;

    use super::*;
    use crate::fs::Bounds;
    use crate::fs::fake::{FakeBackend, USER_UID};

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![PathBuf::from("/w")],
            config_dir: PathBuf::from("/c"),
            data_dir: PathBuf::from("/d"),
            user_uid: USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn measured(fake: &FakeBackend, sizer: &mut Sizer, path: &str) -> (Size, WalkSkips) {
        let gate = gate(fake);
        let mut skips = WalkSkips::default();
        let meta = gate.lstat(Path::new(path)).expect("fixture exists");
        let size = sizer
            .measure(&gate, Path::new(path), meta, &mut skips)
            .expect("sizing succeeds");
        (size, skips)
    }

    fn dir_size(fake: &FakeBackend, path: &str) -> u64 {
        gate(fake)
            .lstat(Path::new(path))
            .expect("fixture exists")
            .allocated
    }

    #[test]
    fn sums_a_tree_counting_a_hardlinked_file_once() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/one", 8192);
        fake.add_sized_file("/w/a/sub/two", 4096);
        fake.add_hardlink("/w/a/one", "/w/a/sub/one-again");
        let folders = dir_size(&fake, "/w/a") + dir_size(&fake, "/w/a/sub");

        let (size, skips) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert_eq!(size, Size::bytes(8192 + 4096 + folders));
        assert_eq!(skips.total(), 0);
    }

    #[test]
    fn counts_an_inode_once_across_items_of_one_plan() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/one", 8192);
        fake.add_hardlink("/w/a/one", "/w/b/one");
        let mut sizer = Sizer::new();
        let folders = dir_size(&fake, "/w/a");

        let (first, _) = measured(&fake, &mut sizer, "/w/a");
        let (second, _) = measured(&fake, &mut sizer, "/w/b");

        assert_eq!(first, Size::bytes(8192 + folders));
        assert_eq!(second, Size::bytes(dir_size(&fake, "/w/b")));
    }

    #[test]
    fn counts_a_symlink_as_itself_without_following_it() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/elsewhere/big", 1 << 30);
        fake.add_sized_file("/w/a/small", 4096);
        fake.add_symlink("/w/a/link", "/w/elsewhere");
        let link = gate(&fake).lstat(Path::new("/w/a/link")).expect("link");
        let own = dir_size(&fake, "/w/a") + link.allocated;

        let (size, _) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert_eq!(size, Size::bytes(4096 + own));
    }

    #[test]
    fn skips_a_mount_a_placeholder_and_a_denied_folder() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/volume/big", 1 << 30);
        fake.mount("/w/a/volume", 7);
        fake.add_sized_file("/w/a/cloud/big", 1 << 30);
        fake.make_dataless("/w/a/cloud");
        fake.add_sized_file("/w/a/locked/big", 1 << 30);
        fake.fail_on("/w/a/locked", fs::Op::ReadDir, ErrorKind::PermissionDenied);

        let (size, skips) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert!(size < Size::bytes(1 << 30), "{size}");
        assert_eq!((skips.mounts, skips.placeholders, skips.denied), (1, 1, 1));
    }

    #[test]
    fn a_listing_error_other_than_a_denial_fails() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/broken/x", 4096);
        fake.fail_on("/w/a/broken", fs::Op::ReadDir, ErrorKind::Other);
        let gate = gate(&fake);
        let meta = gate.lstat(Path::new("/w/a")).expect("fixture");

        let result =
            Sizer::new().measure(&gate, Path::new("/w/a"), meta, &mut WalkSkips::default());

        assert!(result.is_err());
    }

    #[test]
    fn a_child_that_vanishes_after_listing_counts_nothing() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/kept", 4096);
        fake.add_sized_file("/w/a/gone", 1 << 30);
        fake.fail_on("/w/a/gone", fs::Op::Lstat, ErrorKind::NotFound);

        let (size, skips) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert_eq!(size, Size::bytes(4096 + dir_size(&fake, "/w/a")));
        assert_eq!(skips.total(), 0);
    }

    #[test]
    fn a_child_rosie_cannot_lstat_fails() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/kept", 4096);
        fake.add_sized_file("/w/a/locked", 4096);
        fake.fail_on("/w/a/locked", fs::Op::Lstat, ErrorKind::PermissionDenied);
        let gate = gate(&fake);
        let meta = gate.lstat(Path::new("/w/a")).expect("fixture");

        let result =
            Sizer::new().measure(&gate, Path::new("/w/a"), meta, &mut WalkSkips::default());

        assert!(
            matches!(
                result,
                Err(fs::Error::Io {
                    op: fs::Op::Lstat,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    /// Same vanishing as `a_child_that_vanishes_after_listing_counts_nothing`, but the
    /// parent folder was replaced by a plain file mid-scan: `lstat` then reports
    /// `NotADirectory` rather than `NotFound`. This is a skip, not a scan failure, the
    /// same as any other entry that vanished after it was listed.
    #[test]
    fn a_child_whose_parent_became_a_file_mid_scan_counts_nothing() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/kept", 4096);
        fake.add_sized_file("/w/a/gone", 1 << 30);
        fake.fail_on("/w/a/gone", fs::Op::Lstat, ErrorKind::NotADirectory);

        let (size, skips) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert_eq!(size, Size::bytes(4096 + dir_size(&fake, "/w/a")));
        assert_eq!(skips.total(), 0);
    }

    /// The folder itself vanishes between its own `lstat` (already done by the caller)
    /// and this sizer's `read_dir` of it: nothing more to count, not a scan failure.
    #[test]
    fn a_folder_that_vanishes_between_its_lstat_and_its_listing_counts_only_itself() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/w/a/kept", 4096);
        fake.fail_on("/w/a", fs::Op::ReadDir, ErrorKind::NotFound);

        let (size, skips) = measured(&fake, &mut Sizer::new(), "/w/a");

        assert_eq!(size, Size::bytes(dir_size(&fake, "/w/a")));
        assert_eq!(skips.total(), 0);
    }
}
