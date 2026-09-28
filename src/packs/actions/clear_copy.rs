use crate::fs::Backend;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::Store;

/// A copy beside a pack that is only ever cleared, never restored. The pack and its
/// set-aside copy are not among them, so neither can be deleted in place.
#[derive(Debug, Clone, Copy)]
pub enum Discarded {
    /// `.<repo>.new`: a pull's staging folder.
    Staging,
    /// `.<repo>.removed`: a pack, or a stale set-aside copy, on its way out.
    Removed,
}

/// Deletes a discarded copy beside a pack, if there is one.
pub struct ClearCopy<'a> {
    pub source: &'a Source,
    pub copy: Discarded,
}

impl ClearCopy<'_> {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        let path = match self.copy {
            Discarded::Staging => store.staging_dir(self.source),
            Discarded::Removed => store.removed_dir(self.source),
        };
        if !store.exists(&path)? {
            return Ok(());
        }

        store.gate().delete_own(&path)?;
        Ok(())
    }
}
