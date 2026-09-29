use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;

use super::clear_copy::{ClearCopy, Discarded};
use super::settle_pack::SettlePack;
use crate::fs::{self, Backend, Gate};
use crate::packs::archive::RuleFile;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::{self, Store};

/// Installs a pack's rule files, replacing any installed copy wholesale.
///
/// The files are written to a staging folder beside the pack, then swapped in by two
/// renames: the old pack aside, the new pack in. The pack folder never holds a mix of
/// old and new files; between the renames it is briefly absent, and a failed second
/// rename moves the old pack back. Every crash point is listed on [`Store`].
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
        self.stage(store.gate(), &staging)?;
        self.refuse_reserved(store)?;
        self.swap_in(store, &staging)?;

        // The swap leaves the old pack set aside beside the new one; settling discards
        // it through the never-restored name.
        settle.run(store)
    }

    /// Writes the rule files and the pull time into the fresh `staging` folder, each as
    /// a new file. A failure leaves the staging folder behind; it is never read as a
    /// pack and the next settle clears it.
    fn stage<B: Backend>(&self, gate: &Gate<B>, staging: &Path) -> Result<(), Error> {
        gate.create_own_dir_all(staging)?;
        for rule in self.rules {
            gate.create_own_file(&staging.join(rule.name()), rule.contents())
                .map_err(|error| named_twice(error, rule))?;
        }

        let pulled_at = store::pulled_text(self.pulled_at);
        gate.create_own_file(&store::pulled_file(staging), pulled_at.as_bytes())?;
        Ok(())
    }

    /// Refuses a pack name the volume folds onto the reserved pack name `user`, clearing
    /// the staging folder. Only the volume knows which names it folds together, and the
    /// staging folder is the first entry spelled with the pack name, so it is asked here.
    fn refuse_reserved<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        if !store.stages_as_reserved(self.source)? {
            return Ok(());
        }

        ClearCopy {
            source: self.source,
            copy: Discarded::Staging,
        }
        .run(store)?;
        Err(Error::ReservedPack {
            text: self.source.to_string(),
        })
    }

    fn swap_in<B: Backend>(&self, store: &Store<B>, staging: &Path) -> Result<(), Error> {
        let gate = store.gate();
        let pack = store.pack_dir(self.source);
        let set_aside = store.set_aside_dir(self.source);
        let replacing = store.exists(&pack)?;
        if replacing {
            gate.rename_own(&pack, &set_aside)?;
        }

        let installed = gate.rename_own(staging, &pack);
        match (installed, replacing) {
            (Ok(()), _) => Ok(()),
            (Err(error), false) => Err(error.into()),
            (Err(error), true) => match gate.rename_own(&set_aside, &pack) {
                Ok(()) => Err(error.into()),
                Err(restore) => Err(Error::Restore {
                    error: Box::new(error),
                    restore: Box::new(restore),
                    set_aside,
                }),
            },
        }
    }
}

/// Distinct archive names can name one file on the volume, which folds case and
/// Unicode form; the later file is then a duplicate of the earlier one.
fn named_twice(error: fs::Error, rule: &RuleFile) -> Error {
    match &error {
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
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::fs::{Bounds, Op};

    const HOME: &str = "/Users/me";
    const DATA: &str = "/Users/me/.local/share/rosie";
    const PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";
    const STAGING: &str = "/Users/me/.local/share/rosie/packs/chakrit/.rosie.new";

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![],
            config_dir: PathBuf::from("/Users/me/.config/rosie"),
            data_dir: PathBuf::from(DATA),
            user_uid: USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
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
        let gate = gate(&fake);
        let store = Store::new(&gate, Path::new(HOME), Path::new(DATA));
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
        let gate = gate(&fake);
        let store = Store::new(&gate, Path::new(HOME), Path::new(DATA));
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
