//! The installed packs under `<data>/packs/<owner>/<repo>/` (`docs/spec/rules.md#packs`).
//!
//! Every pack folder holds its `*.toml` rule files and a `.pulled` file with the pull
//! time in Unix seconds. Copies of a pack sit beside it under dot-prefixed names; a
//! valid pack name never starts with `.`, so they are never mistaken for packs. What
//! each copy means is set out on [`Store`].

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::archive::{RULE_EXTENSION, RuleFile};
use super::error::Error;
use super::source::Source;
use crate::fs::{self, Backend, FileKind, Gate};
use crate::rules::Name;

const PACKS_FOLDER: &str = "packs";
const PULLED_FILE: &str = ".pulled";
const COPY_PREFIX: &str = ".";
const STAGING_SUFFIX: &str = ".new";
const SET_ASIDE_SUFFIX: &str = ".old";
const REMOVED_SUFFIX: &str = ".removed";
const COPY_SUFFIXES: [&str; 3] = [STAGING_SUFFIX, SET_ASIDE_SUFFIX, REMOVED_SUFFIX];

/// The file inside a pack folder that records its pull time.
pub(super) fn pulled_file(pack_dir: &Path) -> PathBuf {
    pack_dir.join(PULLED_FILE)
}

/// The pull time as stored: Unix seconds.
pub(super) fn pulled_text(pulled_at: SystemTime) -> String {
    // A clock before 1970 is recorded as 1970: the pack then only ages sooner.
    let seconds = pulled_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

/// The context every pack action runs in: the gate, the home folder, and the packs
/// folder.
///
/// # Copies beside a pack
///
/// For a source `owner/repo`, the folder `<data>/packs/<owner>/` can hold:
///
/// | Name              | What it is                                   | What becomes of it       |
/// |-------------------|----------------------------------------------|--------------------------|
/// | `<repo>`          | the pack; always complete, as it only ever   | kept                     |
/// |                   | arrives by renaming a complete folder        |                          |
/// | `.<repo>.new`     | staging: a pull writing the new pack         | cleared in place         |
/// | `.<repo>.old`     | set aside: the only copy of the old pack,    | put back as `<repo>`, or |
/// |                   | between a swap's two renames                 | renamed to `.removed`    |
/// | `.<repo>.removed` | a copy on its way out                        | cleared, never restored  |
///
/// `.<repo>.old` is the one name a pack ever comes back from, and nothing is deleted
/// under it: a copy is only ever deleted under `.new` or `.removed`. Every step below
/// is one rename or one delete, except the pull's first, which writes the staging copy
/// file by file. A crash or failure at any step leaves one of the states listed beside
/// it, and the next settle brings that state to rest.
///
/// # Settle (`SettlePack`)
///
/// Brings one source to rest, the pack alone or nothing. It runs before every pull,
/// remove, and first-run check, for every source with a copy beside it.
///
/// 1. Clear `.new`.
/// 2. Clear `.removed`.
/// 3. `.old` without `<repo>`: the swap stopped between its renames, so `.old` is the
///    pack; rename it back to `<repo>`.
/// 4. `.old` beside `<repo>`: the swap finished, so `.old` is stale; rename it to
///    `.removed`, then clear `.removed`.
///
/// # Pull (`InstallPack`, after settling)
///
/// | Step                                    | A crash or failure here leaves     | The next settle      |
/// |-----------------------------------------|------------------------------------|----------------------|
/// | 1. write `.new`                         | `<repo>` if replacing, and `.new`  | clears `.new`        |
/// | 2. clear `.new` and stop, if the volume | `<repo>` if replacing, and part of | clears `.new`        |
/// |    folds the pack name onto `user`      | `.new`                             |                      |
/// | 3. rename `<repo>` to `.old`, if any    | a failure: `<repo>` and `.new`;    | clears `.new`, and   |
/// |                                         | a crash after it: `.old` and       | puts back any `.old` |
/// |                                         | `.new`                             |                      |
/// | 4. rename `.new` to `<repo>`            | a failure renames `.old` back; if  | puts `.old` back     |
/// |                                         | that fails too (`Error::Restore`), |                      |
/// |                                         | `.old` and `.new`                  |                      |
/// | 5. settle: rename `.old` to `.removed`  | `<repo>` and `.old`                | discards `.old`      |
/// | 6. settle: clear `.removed`             | `<repo>` and part of `.removed`    | clears `.removed`    |
///
/// A failure in steps 5 and 6 fails the pull, though the new pack is installed.
///
/// # Remove (`RemovePack`, after settling every source)
///
/// | Step                                    | A crash or failure here leaves     | The next settle      |
/// |-----------------------------------------|------------------------------------|----------------------|
/// | 1. rename `<repo>` to `.removed`        | `.removed`                         | clears `.removed`    |
/// | 2. clear `.removed`                     | part of `.removed`                 | clears `.removed`    |
/// | 3. delete the owner folder, if empty    | an empty owner folder              | nothing: not a pack  |
///
/// # First run (`first_run`)
///
/// Settles every source with a copy beside it, then pulls the default source when no
/// pack is installed. A crash while settling leaves a state from the tables above.
pub struct Store<'g, B: Backend> {
    gate: &'g Gate<B>,
    home: PathBuf,
    dir: PathBuf,
}

/// A pack on disk and when it was pulled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub source: Source,
    pub pulled_at: SystemTime,
}

impl<'g, B: Backend> Store<'g, B> {
    /// `home` is the injected home folder, which a rule's `~` stands for; `data_dir` is
    /// rosie's injected data folder, `~/.local/share/rosie`.
    pub fn new(gate: &'g Gate<B>, home: &Path, data_dir: &Path) -> Self {
        Store {
            gate,
            home: home.into(),
            dir: data_dir.join(PACKS_FOLDER),
        }
    }

    /// Every installed pack, ordered by source. Entries whose names are not valid pack
    /// names, such as staging copies, are not packs and are left out.
    pub fn installed(&self) -> Result<Vec<Installed>, Error> {
        self.sources()?
            .into_iter()
            .map(|source| self.installed_from(source))
            .collect()
    }

    /// The sources of every installed pack, ordered, without reading their pull times.
    pub fn sources(&self) -> Result<Vec<Source>, Error> {
        let owners = self.folders_named_validly(&self.dir)?;
        let per_owner = owners
            .iter()
            .map(|owner| self.sources_of(owner))
            .collect::<Result<Vec<_>, _>>()?;

        let mut sources: Vec<Source> = per_owner.into_iter().flatten().collect();
        sources.sort();
        Ok(sources)
    }

    /// The rule files of an installed pack, sorted by name so a duplicate rule's error
    /// names the same "first" and "second" file on every run, regardless of `read_dir`
    /// order.
    pub fn rule_files(&self, source: &Source) -> Result<Vec<RuleFile>, Error> {
        let pack = self.pack_dir(source);
        let names = sorted(self.gate.read_dir(&pack)?);
        names
            .into_iter()
            .filter(|name| Path::new(name).extension() == Some(OsStr::new(RULE_EXTENSION)))
            .map(|name| {
                let contents = self.gate.read_own_file(&pack.join(&name))?;
                Ok(RuleFile::new(name, contents))
            })
            .collect()
    }

    // layout, for the pack actions

    pub(super) fn gate(&self) -> &Gate<B> {
        self.gate
    }

    pub(super) fn home(&self) -> &Path {
        &self.home
    }

    pub(super) fn owner_dir(&self, source: &Source) -> PathBuf {
        self.dir.join(source.owner())
    }

    pub(super) fn pack_dir(&self, source: &Source) -> PathBuf {
        self.owner_dir(source).join(source.pack().as_str())
    }

    pub(super) fn staging_dir(&self, source: &Source) -> PathBuf {
        self.copy_dir(source, STAGING_SUFFIX)
    }

    pub(super) fn set_aside_dir(&self, source: &Source) -> PathBuf {
        self.copy_dir(source, SET_ASIDE_SUFFIX)
    }

    pub(super) fn removed_dir(&self, source: &Source) -> PathBuf {
        self.copy_dir(source, REMOVED_SUFFIX)
    }

    /// `source` with its owner spelled as the volume stores the owner's folder, when it
    /// finds one. GitHub owner names are case-insensitive, so the folder the volume folds
    /// the owner onto is that owner's, and its packs keep that spelling.
    pub(super) fn resolve(&self, source: &Source) -> Result<Source, Error> {
        let owner = OsStr::new(source.owner());
        let stored = match self.gate.stored_name(&self.dir, owner) {
            Ok(stored) => stored,
            Err(fs::Error::Io { source: error, .. }) if error.kind() == ErrorKind::NotFound => {
                return Ok(source.clone());
            }
            Err(error) => return Err(error.into()),
        };

        let stray = || Error::StrayOwnerFolder {
            owner: source.owner().to_owned(),
            path: self.dir.join(&stored),
        };
        stored
            .to_str()
            .and_then(|stored| Source::from_names(stored, source.pack().as_str()))
            .ok_or_else(stray)
    }

    /// The sources with any copy beside their pack folder, ordered.
    pub(super) fn sources_with_copies(&self) -> Result<Vec<Source>, Error> {
        let owners = self.folders_named_validly(&self.dir)?;
        let per_owner = owners
            .iter()
            .map(|owner| self.copies_of(owner))
            .collect::<Result<Vec<_>, _>>()?;

        let sources: BTreeSet<Source> = per_owner.into_iter().flatten().collect();
        Ok(sources.into_iter().collect())
    }

    /// The installed sources whose pack folder the volume finds under the pack name
    /// `pack` in any owner's folder: the pack spelled exactly, or, on a volume that folds
    /// case or Unicode form, the pack `pack` folds onto, as its folder is spelled.
    pub(super) fn sources_named(&self, pack: &str) -> Result<Vec<Source>, Error> {
        let installed = self.sources()?;
        let owners: BTreeSet<&str> = installed.iter().map(Source::owner).collect();

        let mut named = Vec::new();
        for owner in owners {
            let stored = match self
                .gate
                .stored_name(&self.dir.join(owner), OsStr::new(pack))
            {
                Ok(stored) => stored,
                Err(fs::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let found = stored
                .to_str()
                .and_then(|repo| Source::from_names(owner, repo));
            named.extend(found.filter(|source| installed.contains(source)));
        }
        Ok(named)
    }

    /// Whether the volume folds `source`'s pack name onto the reserved pack name
    /// `user`: a lookup of the reserved name's staging folder finds `source`'s staging
    /// folder, which must exist.
    pub(super) fn stages_as_reserved(&self, source: &Source) -> Result<bool, Error> {
        let reserved = format!("{COPY_PREFIX}{}{STAGING_SUFFIX}", Name::user_pack());
        let staging = self.staging_dir(source);

        let stored = match self
            .gate
            .stored_name(&self.owner_dir(source), OsStr::new(&reserved))
        {
            Ok(stored) => stored,
            Err(fs::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        Ok(staging.file_name() == Some(stored.as_os_str()))
    }

    /// Whether anything is at `path`, without following a symlink there.
    pub(super) fn exists(&self, path: &Path) -> Result<bool, Error> {
        match self.gate.lstat(path) {
            Ok(_) => Ok(true),
            Err(fs::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn copy_dir(&self, source: &Source, suffix: &str) -> PathBuf {
        self.owner_dir(source)
            .join(format!("{COPY_PREFIX}{}{suffix}", source.pack()))
    }

    // listing

    fn sources_of(&self, owner: &str) -> Result<Vec<Source>, Error> {
        let repos = self.folders_named_validly(&self.dir.join(owner))?;
        let sources = repos
            .iter()
            .filter_map(|repo| Source::from_names(owner, repo))
            .collect();
        Ok(sources)
    }

    fn copies_of(&self, owner: &str) -> Result<Vec<Source>, Error> {
        let names = self.gate.read_dir(&self.dir.join(owner))?;
        let repos = names
            .iter()
            .filter_map(|name| name.to_str()?.strip_prefix(COPY_PREFIX))
            .filter_map(|copy| {
                COPY_SUFFIXES
                    .iter()
                    .find_map(|suffix| copy.strip_suffix(suffix))
            });
        Ok(repos
            .filter_map(|repo| Source::from_names(owner, repo))
            .collect())
    }

    fn installed_from(&self, source: Source) -> Result<Installed, Error> {
        let path = pulled_file(&self.pack_dir(&source));
        let text = self.gate.read_own_file(&path)?;
        let bad_pull_time = || Error::BadPullTime { path: path.clone() };

        let seconds = std::str::from_utf8(&text)
            .map_err(|_| bad_pull_time())?
            .trim()
            .parse::<u64>()
            .map_err(|_| bad_pull_time())?;

        Ok(Installed {
            source,
            pulled_at: UNIX_EPOCH + Duration::from_secs(seconds),
        })
    }

    /// Folders directly inside `dir` whose names are valid pack names. A missing `dir`
    /// has none.
    fn folders_named_validly(&self, dir: &Path) -> Result<Vec<String>, Error> {
        let names = match self.gate.read_dir(dir) {
            Ok(names) => names,
            Err(fs::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error.into()),
        };

        let valid_names = names
            .into_iter()
            .filter_map(|name| name.into_string().ok())
            .filter(|entry| Name::parse(entry).is_ok());
        valid_names
            .filter_map(|name| match self.gate.lstat(&dir.join(&name)) {
                Ok(meta) if meta.kind == FileKind::Dir => Some(Ok(name)),
                Ok(_) => None,
                Err(error) => Some(Err(error.into())),
            })
            .collect()
    }
}

/// `names` in a fixed order, independent of the backend's own `read_dir` order.
fn sorted(mut names: Vec<OsString>) -> Vec<OsString> {
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::sorted;

    #[test]
    fn sorts_names_regardless_of_read_dir_order() {
        let names: Vec<OsString> = ["c.toml", "a.toml", "b.toml"]
            .into_iter()
            .map(OsString::from)
            .collect();

        let sorted_names = sorted(names);

        assert_eq!(
            sorted_names,
            vec![
                OsString::from("a.toml"),
                OsString::from("b.toml"),
                OsString::from("c.toml"),
            ]
        );
    }
}
