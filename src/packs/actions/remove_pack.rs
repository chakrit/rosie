use super::clear_copy::{ClearCopy, Discarded};
use super::settle_packs::{SettlePacks, Settled};
use crate::fs::Backend;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::Store;

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

        store
            .gate()
            .rename_own(&store.pack_dir(&source), &store.removed_dir(&source))?;
        ClearCopy {
            source: &source,
            copy: Discarded::Removed,
        }
        .run(store)?;

        if store.gate().read_dir(&owner_dir)?.is_empty() {
            store.gate().delete_own(&owner_dir)?;
        }
        Ok(source)
    }

    fn find<B: Backend>(&self, settled: &Settled<B>) -> Result<Source, Error> {
        let matching: Vec<Source> = settled
            .sources()?
            .into_iter()
            .filter(|source| source.pack().as_str() == self.pack)
            .collect();

        match matching.as_slice() {
            [] => Err(Error::NotInstalled {
                pack: self.pack.clone(),
            }),
            [source] => Ok(source.clone()),
            _ => Err(Error::Ambiguous {
                pack: self.pack.clone(),
                sources: matching,
            }),
        }
    }
}
