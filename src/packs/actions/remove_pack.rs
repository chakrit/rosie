use super::clear_copy::{ClearCopy, Discarded};
use super::settle_packs::{SettlePacks, Settled};
use crate::fs::{Backend, FsOp, Rosie};
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::Store;
use crate::rules::PackName;

/// Removes an installed pack by its pack name, including `rosie`
/// (`docs/spec/rules.md#packs`).
///
/// Copies beside the packs are settled first, so a pack an interrupted swap set aside
/// counts as installed and a stale set-aside copy is gone. The pack is then renamed to
/// its removed copy, which is only ever cleared, never restored, so it disappears whole
/// even if deleting its files stops partway. An owner folder left empty is removed too.
/// Every crash point is listed on [`Store`].
pub struct RemovePack {
    pub pack: String,
}

impl RemovePack {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<Source, Error> {
        let source = self.find(&SettlePacks.run(store)?)?;
        let owner_dir = store.owner_dir(&source);

        Rosie::rename(&store.pack_dir(&source), &store.removed_dir(&source))
            .perform(store.ctx())?;
        ClearCopy {
            source: &source,
            copy: Discarded::Removed,
        }
        .run(store)?;

        if Rosie::read_dir(&owner_dir).perform(store.ctx())?.is_empty() {
            Rosie::delete(&owner_dir).perform(store.ctx())?;
        }
        Ok(source)
    }

    fn find<B: Backend>(&self, settled: &Settled<B>) -> Result<Source, Error> {
        let not_installed = || Error::NotInstalled {
            pack: self.pack.clone(),
        };
        // Text that is not a valid pack name names no installed pack.
        let wanted = PackName::parse(&self.pack).map_err(|_| not_installed())?;

        let matching: Vec<Source> = settled
            .sources()?
            .into_iter()
            .filter(|source| *source.pack() == wanted)
            .collect();

        match matching.as_slice() {
            [] => Err(not_installed()),
            [source] => Ok(source.clone()),
            _ => Err(Error::Ambiguous {
                pack: self.pack.clone(),
                sources: matching,
            }),
        }
    }
}
