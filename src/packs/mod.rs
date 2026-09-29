//! Rule packs: pull, remove, first-run pull, config seeding, and age
//! (`docs/spec/rules.md#packs`).
//!
//! A pack is pulled from a GitHub `owner/repo` as a tarball, checked entry by entry in
//! memory, and installed through the gate's own-data methods
//! (`docs/spec/safety.md#rosies-own-data`). The network is injected beside the gate.

mod actions;
mod age;
mod archive;
mod conflict;
mod error;
mod network;
mod source;
mod store;

use std::time::SystemTime;

use actions::download_archive::DownloadArchive;
use actions::install_pack::InstallPack;
use actions::settle_packs::SettlePacks;
use conflict::PackRules;

use crate::fs::Backend;

pub use actions::remove_pack::RemovePack;
pub use age::{AgeWarning, MONTH, age_warning};
pub use archive::{Archive, RuleFile};
pub use conflict::Conflict;
pub use error::{Error, Refusal};
pub use network::{Https, Network};
pub use source::Source;
pub use store::{Installed, Store};

/// A pull that installed its pack.
#[derive(Debug)]
pub struct Pulled {
    pub source: Source,
    /// Pulled rules that duplicate rules of other installed packs, to warn about.
    pub conflicts: Vec<Conflict>,
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
/// any installed copy wholesale.
///
/// GitHub owner names are case-insensitive, so when the volume already stores the
/// requested owner's folder under another spelling, the pull downloads, installs, and
/// reports the source under that stored spelling.
pub fn pull<B: Backend, N: Network>(
    store: &Store<B>,
    network: &N,
    requested: &Source,
    now: SystemTime,
) -> Result<Pulled, Error> {
    SettlePacks.run(store)?;
    let source = &store.resolve(requested)?;
    refuse_name_taken(store, source)?;

    let archive = DownloadArchive { source }.run(network)?;
    install(store, source, &archive, now)
}

/// Pulls the default source when no pack is installed. An offline failure is returned
/// with its reason, and rosie stops.
pub fn first_run<B: Backend, N: Network>(
    store: &Store<B>,
    network: &N,
    now: SystemTime,
) -> Result<FirstRun, Error> {
    SettlePacks.run(store)?;
    if !store.sources()?.is_empty() {
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
    SettlePacks.run(store)?;
    let first_run_due = store.sources()?.is_empty();
    let source = &match first_run_due {
        true => store.resolve(&Source::default())?,
        false => Source::default(),
    };
    if first_run_due {
        refuse_name_taken(store, source)?;
    }

    let archive = DownloadArchive { source }.run(network)?;
    let config = archive.config()?.to_owned();
    let first_run = match first_run_due {
        true => FirstRun::Pulled(install(store, source, &archive, now)?),
        false => FirstRun::Ready,
    };

    Ok(Seed { config, first_run })
}

/// Checks a downloaded pack against the other installed packs, then installs it.
fn install<B: Backend>(
    store: &Store<B>,
    source: &Source,
    archive: &Archive,
    now: SystemTime,
) -> Result<Pulled, Error> {
    let installed = store.sources()?;
    let pulled_rules = PackRules::load(source, archive.rules())?;
    let conflicts = conflicts_with_others(store, source, &pulled_rules, &installed)?;

    InstallPack {
        source,
        rules: archive.rules(),
        pulled_at: now,
    }
    .run(store)?;
    Ok(Pulled {
        source: source.clone(),
        conflicts,
    })
}

/// Two packs with one name would give their rules the same `pack/rule` identities, and a
/// source the volume folds onto an installed pack would respell that pack and its rules.
/// So a source is refused when the volume finds another installed source under its pack
/// name; the refusal names that pack as it is spelled on disk.
fn refuse_name_taken<B: Backend>(store: &Store<B>, source: &Source) -> Result<(), Error> {
    let named = store.sources_named(source.pack().as_str())?;
    let taken = named.into_iter().find(|other| other != source);

    match taken {
        Some(other) => Err(Error::PackNameTaken {
            pack: source.pack().to_string(),
            installed: other,
        }),
        None => Ok(()),
    }
}

fn conflicts_with_others<B: Backend>(
    store: &Store<B>,
    source: &Source,
    pulled: &PackRules,
    installed: &[Source],
) -> Result<Vec<Conflict>, Error> {
    let others = installed.iter().filter(|other| *other != source);
    let per_pack = others
        .map(|other| {
            let rules = PackRules::load(other, &store.rule_files(other)?)?;
            Ok(pulled.conflicts_with(&rules, store.home()))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(per_pack.into_iter().flatten().collect())
}
