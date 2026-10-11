//! The installed packs under `<data>/packs/<owner>/<repo>/` (`docs/spec/rules.md#packs`).
//!
//! Every pack folder holds its `*.toml` rule files and a `.pulled` file with the pull
//! time in Unix seconds. Copies of a pack sit beside it under dot-prefixed names; a
//! valid pack name never starts with `.`, so they are never mistaken for packs. What
//! each copy means is set out on [`Store`].

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::error::{Error, Stray};
use super::source::Source;
use crate::fs::{Backend, FileKind, FsContext, FsOp, Rosie};
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

/// What every pack action runs in: the FS context, the home folder, and the packs
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
/// Brings one source to rest, the pack alone or nothing. It runs before anything lists
/// the installed packs (every pull, remove, rule load, and first-run check), for every
/// source with a copy beside it.
///
/// 1. `.old` without `<repo>`: the swap stopped between its renames, so `.old` is the
///    pack; rename it back to `<repo>`. This comes first, so a copy that cannot be
///    cleared never keeps the pack away.
/// 2. Clear `.new`.
/// 3. Clear `.removed`.
/// 4. `.old` beside `<repo>`: the swap finished, so `.old` is stale; rename it to
///    `.removed`, then clear `.removed`.
///
/// # Pull (`InstallPack`, after settling)
///
/// | Step                                    | A crash or failure here leaves     | The next settle      |
/// |-----------------------------------------|------------------------------------|----------------------|
/// | 1. write `.new`                         | `<repo>` if replacing, and `.new`  | clears `.new`        |
/// | 2. rename `<repo>` to `.old`, if any    | a failure: `<repo>` and `.new`;    | puts back any `.old`,|
/// |                                         | a crash after it: `.old` and       | and clears `.new`    |
/// |                                         | `.new`                             |                      |
/// | 3. rename `.new` to `<repo>`            | a failure on a first install:      | puts back any `.old`,|
/// |                                         | `.new`; one when replacing         | then clears `.new`   |
/// |                                         | settles, putting `.old` back, then |                      |
/// |                                         | clearing `.new`; if the move back  |                      |
/// |                                         | fails (`Error::Restore`), `.old`   |                      |
/// |                                         | and `.new`; if clearing fails      |                      |
/// |                                         | (`Error::Leftover`), `<repo>` and  |                      |
/// |                                         | part of `.new`; a crash before it: |                      |
/// |                                         | `.old` if replacing, and `.new`    |                      |
/// | 4. settle: rename `.old` to `.removed`  | `<repo>` and `.old`                | discards `.old`      |
/// | 5. settle: clear `.removed`             | `<repo>` and part of `.removed`    | clears `.removed`    |
///
/// A failure in steps 4 and 5 fails the pull, though the new pack is installed.
///
/// # Remove (`RemovePack`, after settling every source)
///
/// | Step                                    | A crash or failure here leaves     | The next settle      |
/// |-----------------------------------------|------------------------------------|----------------------|
/// | 1. rename `<repo>` to `.removed`        | a failure: `<repo>`; a crash after | clears any `.removed`|
/// |                                         | it: `.removed`                     |                      |
/// | 2. clear `.removed`                     | part of `.removed`                 | clears `.removed`    |
/// | 3. delete the owner folder, if empty    | an empty owner folder              | nothing: not a pack  |
///
/// # First run (`first_run`)
///
/// Settles every source with a copy beside it, then pulls the default source when no
/// pack is installed. A crash while settling leaves a state from the tables above.
/// `seed_config` settles the same way and, when no pack is installed, installs the
/// default source from the one download it also reads the config from.
pub struct Store<'g, B: Backend> {
    ctx: &'g FsContext<B>,
    dir: PathBuf,
}

/// A pack on disk and when it was pulled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub source: Source,
    pub pulled_at: SystemTime,
}

impl<'g, B: Backend> Store<'g, B> {
    /// `data_dir` is rosie's injected data folder, `~/.local/share/rosie`.
    pub fn new(ctx: &'g FsContext<B>, data_dir: &Path) -> Self {
        Store {
            ctx,
            dir: data_dir.join(PACKS_FOLDER),
        }
    }

    // layout, for the pack actions

    pub(super) fn ctx(&self) -> &FsContext<B> {
        self.ctx
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

    /// The sources of every pack in the folder, ordered, whether or not a copy beside it
    /// has been settled. Only [`super::actions::settle_packs::Settled`] lists packs for
    /// the rest of rosie, after settling.
    pub(super) fn unsettled_sources(&self) -> Result<Vec<Source>, Error> {
        let mut sources: Vec<Source> = self
            .owner_entries()?
            .into_iter()
            .filter_map(|entry| match entry {
                OwnerEntry::Pack(source) => Some(source),
                OwnerEntry::Copy(_) | OwnerEntry::File => None,
            })
            .collect();
        sources.sort();
        Ok(sources)
    }

    /// The sources with any copy beside their pack folder, ordered.
    pub(super) fn sources_with_copies(&self) -> Result<Vec<Source>, Error> {
        let sources: BTreeSet<Source> = self
            .owner_entries()?
            .into_iter()
            .filter_map(|entry| match entry {
                OwnerEntry::Copy(source) => Some(source),
                OwnerEntry::Pack(_) | OwnerEntry::File => None,
            })
            .collect();
        Ok(sources.into_iter().collect())
    }

    /// Whether anything is at `path`, without following a symlink there.
    pub(super) fn exists(&self, path: &Path) -> Result<bool, Error> {
        match Rosie::lstat(path).perform(self.ctx) {
            Ok(_) => Ok(true),
            Err(error) if error.is_not_found() => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn copy_dir(&self, source: &Source, suffix: &str) -> PathBuf {
        self.owner_dir(source)
            .join(format!("{COPY_PREFIX}{}{suffix}", source.pack()))
    }

    // listing

    /// Every entry of every owner's folder, `<packs>/<owner>/<entry>`, as what it is.
    fn owner_entries(&self) -> Result<Vec<OwnerEntry>, Error> {
        let owners = Rosie::entries(&self.dir)
            .perform(self.ctx)?
            .into_iter()
            .map(|(path, kind)| top_entry(&path, kind))
            .collect::<Result<Vec<_>, _>>()?;

        let mut held = Vec::new();
        for owner in owners {
            let TopEntry::Owner(owner) = owner else {
                continue;
            };
            for (path, kind) in Rosie::entries(&self.dir.join(owner.as_str())).perform(self.ctx)? {
                held.push(owner_entry(&owner, &path, kind)?);
            }
        }
        Ok(held)
    }

    pub(super) fn installed_from(&self, source: Source) -> Result<Installed, Error> {
        let path = pulled_file(&self.pack_dir(&source));
        let text = Rosie::read(&path).perform(self.ctx)?;
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
}

/// What an entry of the packs folder is.
enum TopEntry {
    /// An owner's folder, holding that owner's packs.
    Owner(Name),
    /// A plain file, such as `.DS_Store`, passed over.
    File,
}

/// What an entry of an owner's folder is.
enum OwnerEntry {
    Pack(Source),
    /// A copy beside a pack, named `.<repo>.new`, `.old`, or `.removed`; see [`Store`].
    Copy(Source),
    /// A plain file, such as `.DS_Store`, passed over.
    File,
}

fn top_entry(path: &Path, kind: FileKind) -> Result<TopEntry, Error> {
    match kind {
        FileKind::File => Ok(TopEntry::File),
        FileKind::Dir => entry_name(path).map(TopEntry::Owner),
        FileKind::Symlink | FileKind::Other => Err(stray(path, Stray::NotAFolder(kind))),
    }
}

/// A copy is known by its name, whatever its kind, so settling can clear it.
fn owner_entry(owner: &Name, path: &Path, kind: FileKind) -> Result<OwnerEntry, Error> {
    if let Some(repo) = copied_repo(path) {
        let repo = Name::parse(repo).map_err(|error| stray(path, Stray::BadName(error)))?;
        return source_in(owner, path, repo).map(OwnerEntry::Copy);
    }

    match kind {
        FileKind::File => Ok(OwnerEntry::File),
        FileKind::Dir => source_in(owner, path, entry_name(path)?).map(OwnerEntry::Pack),
        FileKind::Symlink | FileKind::Other => Err(stray(path, Stray::NotAFolder(kind))),
    }
}

/// The repo name inside a copy's name, `.<repo><suffix>`.
fn copied_repo(path: &Path) -> Option<&str> {
    let copy = path.file_name()?.to_str()?.strip_prefix(COPY_PREFIX)?;
    COPY_SUFFIXES
        .iter()
        .find_map(|suffix| copy.strip_suffix(suffix))
}

/// The entry's own name as a pack or owner name.
fn entry_name(path: &Path) -> Result<Name, Error> {
    let text = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| stray(path, Stray::NotUtf8))?;
    Name::parse(text).map_err(|error| stray(path, Stray::BadName(error)))
}

fn source_in(owner: &Name, path: &Path, repo: Name) -> Result<Source, Error> {
    Source::from_valid_names(owner.clone(), repo).map_err(|_| stray(path, Stray::ReservedName))
}

fn stray(path: &Path, problem: Stray) -> Error {
    Error::StrayEntry {
        path: path.into(),
        problem,
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::fs::{self, Bounds, Paths};

    const DATA: &str = "/Users/me/.local/share/rosie";
    const PACKS: &str = "/Users/me/.local/share/rosie/packs";

    fn listed(fake: &FakeBackend) -> Result<Vec<Source>, Error> {
        let bounds = Bounds {
            roots: Vec::new(),
            paths: Paths::new(Path::new("/Users/me")).expect("absolute home"),
            user_uid: USER_UID,
        };
        let ctx = FsContext::new(fake, bounds).expect("absolute bounds");
        Store::new(&ctx, Path::new(DATA)).unsettled_sources()
    }

    #[test]
    fn lists_packs_passing_over_plain_files_and_copies() {
        let fake = FakeBackend::new();
        fake.add_file(format!("{PACKS}/.DS_Store"), "");
        fake.add_file(format!("{PACKS}/chakrit/.DS_Store"), "");
        fake.add_file(format!("{PACKS}/chakrit/rosie/.pulled"), "1");
        fake.add_dir(format!("{PACKS}/chakrit/.rosie.old"));
        fake.add_dir(format!("{PACKS}/someone/.gone.removed"));

        let sources = listed(&fake).expect("packs list");

        let names: Vec<String> = sources.iter().map(Source::to_string).collect();
        assert_eq!(names, ["chakrit/rosie"]);
    }

    // Every reader of the packs folder, the rule loader included, takes its packs from
    // this one listing. So an entry that is not a pack is refused, naming it, rather
    // than left out by one reader and loaded by another.
    #[test]
    fn refuses_an_entry_of_the_packs_folder_that_is_not_a_pack() {
        let bad_owner = FakeBackend::new();
        bad_owner.add_file(format!("{PACKS}/local.dev/mine/x.toml"), "");
        let bad_pack = FakeBackend::new();
        bad_pack.add_file(format!("{PACKS}/someone/my.rules/x.toml"), "");
        let reserved = FakeBackend::new();
        reserved.add_dir(format!("{PACKS}/someone/user"));
        let linked = FakeBackend::new();
        linked.add_dir("/Users/me/dotfiles/pack");
        linked.add_symlink(format!("{PACKS}/me/pack"), "/Users/me/dotfiles/pack");

        let cases = [
            (bad_owner, "local.dev", "not a valid pack name"),
            (bad_pack, "someone/my.rules", "not a valid pack name"),
            (reserved, "someone/user", "reserved"),
            (linked, "me/pack", "symlink"),
        ];
        for (fake, entry, problem) in cases {
            let result = listed(&fake);
            let Err(Error::StrayEntry {
                path,
                problem: found,
            }) = &result
            else {
                panic!("{entry}: expected a stray entry, got {result:?}");
            };
            assert_eq!(*path, Path::new(PACKS).join(entry));
            assert!(found.to_string().contains(problem), "{entry}: {found}");
        }
    }

    #[test]
    fn refuses_a_packs_path_that_is_not_a_folder() {
        let fake = FakeBackend::new();
        fake.add_file(PACKS, "");

        let result = listed(&fake);

        assert!(
            matches!(
                &result,
                Err(Error::Fs(fs::Error::NotAFolder { found: "file", .. }))
            ),
            "{result:?}"
        );
    }
}
