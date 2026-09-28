use super::settle_pack::SettlePack;
use crate::fs::Backend;
use crate::packs::error::Error;
use crate::packs::store::Store;

/// Settles every pack that has a copy beside it, as the first-run check and a remove
/// need before they read which packs are installed.
pub struct SettlePacks;

impl SettlePacks {
    pub fn run<B: Backend>(&self, store: &Store<B>) -> Result<(), Error> {
        store
            .sources_with_copies()?
            .iter()
            .try_for_each(|source| SettlePack { source }.run(store))
    }
}
