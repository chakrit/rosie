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

/// Which step of a settle failed.
#[derive(Debug)]
pub enum Unsettled {
    /// Checking for the set-aside copy or the pack folder, or moving the copy back to
    /// the absent pack folder, failed; the copy is kept where it was.
    Restore(Error),
    /// Any set-aside copy is already back in place or stale, but clearing or discarding
    /// a copy beside the pack failed.
    Clear(Error),
}

impl From<Unsettled> for Error {
    fn from(unsettled: Unsettled) -> Self {
        match unsettled {
            Unsettled::Restore(error) | Unsettled::Clear(error) => error,
        }
    }
}

/// What the set-aside copy is once the pack folder has been settled.
enum SetAside {
    /// There is none, or it was just moved back as the pack.
    Gone,
    /// It sits beside a newer pack.
    Stale,
}

impl SettlePack<'_> {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<(), Unsettled> {
        let set_aside = self.restore(store).map_err(Unsettled::Restore)?;
        self.clear(store, set_aside).map_err(Unsettled::Clear)
    }

    /// Moves the set-aside copy back when the pack folder is absent: the swap stopped
    /// between its renames, so that copy is the pack. It runs before anything is
    /// cleared, so no failure to clear another copy keeps the pack away.
    fn restore<B: Backend>(&self, store: &Store<B>) -> Result<SetAside, Error> {
        let set_aside = store.set_aside_dir(self.source);
        if !store.exists(&set_aside)? {
            return Ok(SetAside::Gone);
        }

        let pack = store.pack_dir(self.source);
        if store.exists(&pack)? {
            return Ok(SetAside::Stale);
        }

        store.gate().rename_own(&set_aside, &pack)?;
        Ok(SetAside::Gone)
    }

    /// Clears the staging and removed copies, then discards a stale set-aside copy
    /// through the never-restored name.
    fn clear<B: Backend>(&self, store: &Store<B>, set_aside: SetAside) -> Result<(), Error> {
        self.clear_copy(store, Discarded::Staging)?;
        self.clear_copy(store, Discarded::Removed)?;

        match set_aside {
            SetAside::Gone => Ok(()),
            SetAside::Stale => {
                store.gate().rename_own(
                    &store.set_aside_dir(self.source),
                    &store.removed_dir(self.source),
                )?;
                self.clear_copy(store, Discarded::Removed)
            }
        }
    }

    fn clear_copy<B: Backend>(&self, store: &Store<B>, copy: Discarded) -> Result<(), Error> {
        ClearCopy {
            source: self.source,
            copy,
        }
        .run(store)
    }
}
