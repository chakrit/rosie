//! Rule packs: pull, remove, first-run pull, config seeding, and age
//! (`docs/spec/rules.md#packs`).
//!
//! A pack is pulled from a GitHub `owner/repo` as a tarball, checked entry by entry in
//! memory, and installed through [`crate::fs::Rosie`] ops
//! (`docs/spec/safety.md#rosies-own-data`). The network is injected beside the context.

mod actions;
mod age;
mod archive;
mod error;
mod network;
mod source;
mod store;

use std::time::SystemTime;

use actions::download_archive::DownloadArchive;
use actions::install_pack::InstallPack;

use crate::fs::Backend;

pub use actions::remove_pack::RemovePack;
pub use actions::settle_packs::{SettlePacks, Settled};
pub use age::{AgeWarning, MONTH, age_warning};
pub use archive::{Archive, RuleFile};
pub use error::{Error, Refusal, Stray};
pub use network::{Https, Network};
pub use source::Source;
pub use store::{Installed, Store};

/// A pull that installed its pack.
#[derive(Debug)]
pub struct Pulled {
    pub source: Source,
}

/// What the first-run check did.
#[derive(Debug)]
pub enum FirstRun {
    /// A pack is installed; nothing was pulled.
    Ready,
    /// No pack was installed, so the default source was pulled.
    Pulled(Pulled),
}

/// What [`seed_config`] downloaded.
#[derive(Debug)]
pub struct Seed {
    /// The default source's root `config.toml`.
    pub config: String,
    pub first_run: FirstRun,
}

/// `rosie rules pull [<source>]`: downloads the source's pack and installs it, replacing
/// any installed copy wholesale. A source already installed under another case is
/// pulled under its installed spelling.
pub fn pull<B: Backend, N: Network>(
    store: &Store<B>,
    network: &N,
    requested: &Source,
    now: SystemTime,
) -> Result<Pulled, Error> {
    let settled = SettlePacks.run(store)?;
    let installed = settled.sources()?;
    let source = installed_spelling(&installed, requested);
    refuse_name_taken(&installed, source)?;

    let archive = DownloadArchive { source }.run(network)?;
    install(&settled, source, &archive, now)
}

/// Pulls the default source when no pack is installed. An offline failure is returned
/// with its reason, and rosie stops.
pub fn first_run<B: Backend, N: Network>(
    store: &Store<B>,
    network: &N,
    now: SystemTime,
) -> Result<FirstRun, Error> {
    if !SettlePacks.run(store)?.sources()?.is_empty() {
        return Ok(FirstRun::Ready);
    }

    pull(store, network, &Source::default(), now).map(FirstRun::Pulled)
}

/// Downloads the default source once for a fresh user config: returns its root
/// `config.toml`, and installs its pack from the same download when no pack is
/// installed, as [`first_run`] would.
pub fn seed_config<B: Backend, N: Network>(
    store: &Store<B>,
    network: &N,
    now: SystemTime,
) -> Result<Seed, Error> {
    let settled = SettlePacks.run(store)?;
    let first_run_due = settled.sources()?.is_empty();
    let source = &Source::default();

    let archive = DownloadArchive { source }.run(network)?;
    let config = archive.config()?.to_owned();
    let first_run = match first_run_due {
        true => FirstRun::Pulled(install(&settled, source, &archive, now)?),
        false => FirstRun::Ready,
    };

    Ok(Seed { config, first_run })
}

/// Checks a downloaded pack's rules load, then installs it.
fn install<B: Backend>(
    settled: &Settled<B>,
    source: &Source,
    archive: &Archive,
    now: SystemTime,
) -> Result<Pulled, Error> {
    archive.check_rules(source)?;

    InstallPack {
        source,
        rules: archive.rules(),
        pulled_at: now,
    }
    .run(settled.store())?;
    Ok(Pulled {
        source: source.clone(),
    })
}

/// Two packs with one name would give their rules the same `pack/rule` identities, so a
/// source is refused when another installed source has its pack name.
fn refuse_name_taken(installed: &[Source], source: &Source) -> Result<(), Error> {
    let taken = installed.iter().find(|other| {
        let same_name = other.pack().matches_ignoring_case(source.pack().as_str());
        same_name && *other != source
    });

    match taken {
        Some(other) => Err(Error::PackNameTaken {
            pack: source.pack().to_string(),
            installed: other.clone(),
        }),
        None => Ok(()),
    }
}

/// The installed source naming the same repository as `requested`, or `requested` when
/// none does.
fn installed_spelling<'a>(installed: &'a [Source], requested: &'a Source) -> &'a Source {
    let same_repo = installed.iter().find(|other| other.is_same_repo(requested));
    match same_repo {
        Some(installed) => installed,
        None => requested,
    }
}
