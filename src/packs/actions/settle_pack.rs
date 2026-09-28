use super::clear_copy::{ClearCopy, Discarded};
use crate::fs::Backend;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::Store;

/// Brings the copies beside one pack to rest: the pack alone, or nothing. This is the
/// only place that decides what becomes of a set-aside copy; see [`Store`] for the
/// states it settles.
pub struct SettlePack<'a> {
    pub source: &'a Source,
}

impl SettlePack<'_> {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        let set_aside = store.set_aside_dir(self.source);
        let pack = store.pack_dir(self.source);
        ClearCopy {
            source: self.source,
            copy: Discarded::Staging,
        }
        .run(store)?;
        self.clear_removed(store)?;
        if !store.exists(&set_aside)? {
            return Ok(());
        }

        match store.exists(&pack)? {
            // The swap finished: the pack is newer, and the set-aside copy is stale.
            true => {
                store
                    .gate()
                    .rename_own(&set_aside, &store.removed_dir(self.source))?;
                self.clear_removed(store)
            }
            // The swap stopped between its renames: the set-aside copy is the pack.
            false => {
                store.gate().rename_own(&set_aside, &pack)?;
                Ok(())
            }
        }
    }

    fn clear_removed<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        ClearCopy {
            source: self.source,
            copy: Discarded::Removed,
        }
        .run(store)
    }
}
