use crate::packs::archive::Archive;
use crate::packs::error::Error;
use crate::packs::network::Network;
use crate::packs::source::Source;

/// Downloads a source's tarball and checks it into an [`Archive`].
pub struct DownloadArchive<'a> {
    pub source: &'a Source,
}

impl DownloadArchive<'_> {
    pub fn run<N: Network>(&self, network: &N) -> Result<Archive, Error> {
        let url = self.source.tarball_url();

        let gzipped = network
            .get(&url)
            .map_err(|source| Error::Download { url, source })?;

        Archive::parse(&gzipped)
    }
}
