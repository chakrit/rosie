use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;

use super::settle_pack::{SettlePack, Unsettled};
use crate::fs::{self, Backend, FsContext, FsOp, Rosie};
use crate::packs::archive::RuleFile;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::{self, Store};

/// Installs a pack's rule files, replacing any installed copy wholesale.
///
/// The files are written to a staging folder beside the pack, then swapped in by two
/// renames: the old pack aside, the new pack in. The pack folder never holds a mix of
/// old and new files; between the renames it is briefly absent, and when the second
/// rename fails after the first, settling the pack moves the old pack back before it
/// clears the staging folder. Every crash point is listed on [`Store`].
pub struct InstallPack<'a> {
    pub source: &'a Source,
    pub rules: &'a [RuleFile],
    pub pulled_at: SystemTime,
}

impl InstallPack<'_> {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        let settle = SettlePack {
            source: self.source,
        };
        settle.run(store)?;

        let staging = store.staging_dir(self.source);
        self.stage(store.ctx(), &staging)?;
        self.swap_in(store, &settle, &staging)?;

        // The swap leaves the old pack set aside beside the new one; settling discards
        // it through the never-restored name.
        settle.run(store)?;
        Ok(())
    }

    /// Writes the rule files and the pull time into the fresh `staging` folder, each as
    /// a new file. A failure leaves the staging folder behind; it is never read as a
    /// pack and the next settle clears it.
    fn stage<B: Backend>(&self, ctx: &FsContext<B>, staging: &Path) -> Result<(), Error> {
        Rosie::create_dir_all(staging).perform(ctx)?;
        for rule in self.rules {
            Rosie::create(&staging.join(rule.name()), rule.contents())
                .perform(ctx)
                .map_err(|error| named_twice(error, rule))?;
        }

        let pulled_at = store::pulled_text(self.pulled_at);
        Rosie::create(&store::pulled_file(staging), pulled_at.as_bytes()).perform(ctx)?;
        Ok(())
    }

    fn swap_in<B: Backend>(
        &self,
        store: &Store<B>,
        settle: &SettlePack,
        staging: &Path,
    ) -> Result<(), Error> {
        let ctx = store.ctx();
        let pack = store.pack_dir(self.source);
        let set_aside = store.set_aside_dir(self.source);
        let replacing = store.exists(&pack)?;
        if replacing {
            Rosie::rename(&pack, &set_aside).perform(ctx)?;
        }

        let installed = Rosie::rename(staging, &pack).perform(ctx);
        match (installed, replacing) {
            (Ok(()), _) => Ok(()),
            (Err(error), false) => Err(error.into()),
            // With the pack folder absent, settling puts `.old` back, then clears `.new`.
            (Err(error), true) => match settle.run(store) {
                Ok(()) => Err(error.into()),
                Err(Unsettled::Restore(restore)) => Err(Error::Restore {
                    error: Box::new(error),
                    restore: Box::new(restore),
                    set_aside,
                }),
                Err(Unsettled::Clear(clear)) => Err(Error::Leftover {
                    error: Box::new(error),
                    clear: Box::new(clear),
                }),
            },
        }
    }
}

/// Distinct archive names can name one file on the volume, which folds case and
/// Unicode form; the later file is then a duplicate of the earlier one. The FS layer
/// reports it as a misspelling of the name already written, or the volume as a name
/// that already exists.
fn named_twice(error: fs::Error, rule: &RuleFile) -> Error {
    match &error {
        fs::Error::Misspelled { .. } => Error::Duplicate {
            path: rule.archive_path(),
        },
        fs::Error::Io { source, .. } if source.kind() == ErrorKind::AlreadyExists => {
            Error::Duplicate {
                path: rule.archive_path(),
            }
        }
        _ => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::ErrorKind;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::fs::{Bounds, Op, Paths};

    const HOME: &str = "/Users/me";
    const DATA: &str = "/Users/me/.local/share/rosie";
    const PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";
    const STAGING: &str = "/Users/me/.local/share/rosie/packs/chakrit/.rosie.new";

    fn ctx(fake: &FakeBackend) -> FsContext<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![],
            paths: Paths::new(Path::new(HOME)).expect("absolute home"),
            user_uid: USER_UID,
        };
        FsContext::new(fake, bounds).expect("absolute bounds")
    }

    fn rule(name: &str, body: &str) -> RuleFile {
        RuleFile::new(OsString::from(name), body.as_bytes().to_vec())
    }

    #[test]
    fn a_failed_swap_puts_the_old_pack_back() {
        let fake = FakeBackend::new();
        fake.add_file(format!("{PACK}/old.toml"), "old");
        fake.add_file(format!("{PACK}/.pulled"), "1");
        fake.fail_on(STAGING, Op::Rename, ErrorKind::PermissionDenied);
        let ctx = ctx(&fake);
        let store = Store::new(
            &ctx,
            &Paths::new(Path::new(HOME)).expect("absolute home"),
            Path::new(DATA),
        );
        let source = Source::default();
        let rules = [rule("new.toml", "new")];

        let result = InstallPack {
            source: &source,
            rules: &rules,
            pulled_at: UNIX_EPOCH + Duration::from_secs(2),
        }
        .run(&store);

        assert!(
            matches!(
                &result,
                Err(Error::Fs(crate::fs::Error::Io { op: Op::Rename, .. }))
            ),
            "got {result:?}"
        );
        let old = fake
            .read_file(Path::new(&format!("{PACK}/old.toml")))
            .expect("old pack back in place");
        assert_eq!(old, b"old");
        assert!(!fake.exists(format!("{PACK}/new.toml")));
    }

    #[test]
    fn clears_a_staging_folder_an_interrupted_pull_left_behind() {
        let fake = FakeBackend::new();
        fake.add_file(format!("{STAGING}/stale.toml"), "stale");
        let ctx = ctx(&fake);
        let store = Store::new(
            &ctx,
            &Paths::new(Path::new(HOME)).expect("absolute home"),
            Path::new(DATA),
        );
        let source = Source::default();
        let rules = [rule("new.toml", "new")];

        InstallPack {
            source: &source,
            rules: &rules,
            pulled_at: UNIX_EPOCH,
        }
        .run(&store)
        .expect("install");

        assert!(!fake.exists(format!("{PACK}/stale.toml")));
        assert!(fake.exists(format!("{PACK}/new.toml")));
        assert!(!fake.exists(STAGING));
    }
}
