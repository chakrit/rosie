use super::settle_pack::SettlePack;
use crate::fs::Backend;
use crate::packs::error::Error;
use crate::packs::source::Source;
use crate::packs::store::{Installed, Store};
use crate::rules::PackFolder;

/// Settles every pack that has a copy beside it, and hands back the settled packs
/// folder: the only way to list which packs are installed.
pub struct SettlePacks;

impl SettlePacks {
    pub fn run<'s, 'g, B: Backend>(
        &self,
        store: &'s Store<'g, B>,
    ) -> Result<Settled<'s, 'g, B>, Error> {
        store
            .sources_with_copies()?
            .iter()
            .try_for_each(|source| SettlePack { source }.run(store))?;
        Ok(Settled { store })
    }
}

/// The packs folder after [`SettlePacks`], so a pack an interrupted pull set aside is
/// back in place and counts as installed. Every pack action ends settled or fails, so
/// the listings stay true while rosie runs.
///
/// These are the listings of the packs folder: the rule loader, the age warning, pulls,
/// `rules remove`, and the first-run check all read them. Plain files, such as
/// `.DS_Store`, are passed over, and copies beside a pack are not packs; any other entry
/// that is not a pack is refused, naming it.
pub struct Settled<'s, 'g, B: Backend> {
    store: &'s Store<'g, B>,
}

impl<B: Backend> Settled<'_, '_, B> {
    /// The sources of every installed pack, ordered, without reading their pull times.
    pub fn sources(&self) -> Result<Vec<Source>, Error> {
        self.store.unsettled_sources()
    }

    /// Every installed pack and when it was pulled, ordered by source.
    pub fn installed(&self) -> Result<Vec<Installed>, Error> {
        self.sources()?
            .into_iter()
            .map(|source| self.store.installed_from(source))
            .collect()
    }

    /// Each installed pack's name and folder, for the rule loader.
    pub fn pack_folders(&self) -> Result<Vec<PackFolder>, Error> {
        let sources = self.sources()?;
        let folders = sources.iter().map(|source| PackFolder {
            name: source.pack().clone(),
            folder: self.store.pack_dir(source),
        });
        Ok(folders.collect())
    }

    /// The store the packs were settled in, for the actions that change it.
    pub(in crate::packs) fn store(&self) -> &Store<'_, B> {
        self.store
    }

    /// The installed sources whose pack folder the volume finds under the pack name
    /// `pack`; see [`Store::sources_named`].
    pub(in crate::packs) fn sources_named(&self, pack: &str) -> Result<Vec<Source>, Error> {
        self.store.sources_named(&self.sources()?, pack)
    }
}
