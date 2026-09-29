//! Loading the rule layers (`docs/spec/rules.md#layers-and-overrides`): pulled packs,
//! then the user's rules, which may replace a pack's rule by its qualified name.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::error::{Error, Problem};
use super::lua::new_parser_state;
use super::name::{Name, NameError, RuleId};
use super::pack::load_pack;
use super::parse::{Parsed, parse_file};
use super::rule::{Rule, Source};
use crate::fs::{self, Backend, Bounds, FileKind, Gate};

/// Where the rule layers live, below rosie's own folders.
#[derive(Debug, Clone)]
pub struct RuleDirs {
    /// `<data>/packs`, holding `<owner>/<repo>/*.toml`.
    pub packs: PathBuf,
    /// `<config>/rules`, holding `*.toml`.
    pub user: PathBuf,
}

impl RuleDirs {
    pub fn new(bounds: &Bounds) -> RuleDirs {
        RuleDirs {
            packs: bounds.data_dir.join("packs"),
            user: bounds.config_dir.join("rules"),
        }
    }
}

pub(super) fn load<B: Backend>(gate: &Gate<B>, dirs: &RuleDirs) -> Result<Vec<Rule>, Error> {
    let mut loaded: BTreeMap<RuleId, Rule> = BTreeMap::new();
    for (pack, folder) in pack_folders(gate, dirs)? {
        let files = read_rule_files(gate, &folder)?;
        let labeled = files
            .iter()
            .map(|(file, bytes)| (file.as_path(), bytes.as_slice()));
        let rules = load_pack(&pack, labeled)?;
        loaded.extend(rules.into_iter().map(|rule| (rule.id.clone(), rule)));
    }

    let lua = new_parser_state()?;
    let mut user_layer = UserLayer::new();
    for (file, bytes) in read_rule_files(gate, &dirs.user)? {
        for parsed in parse_file(&bytes, &file, &lua)? {
            apply_user_rule(&mut loaded, &mut user_layer, parsed, &file)?;
        }
    }

    Ok(loaded.into_values().collect())
}

// packs

/// Each pulled pack's name and folder, from `<packs>/<owner>/<repo>`, in name order.
fn pack_folders<B: Backend>(
    gate: &Gate<B>,
    dirs: &RuleDirs,
) -> Result<Vec<(Name, PathBuf)>, Error> {
    let mut packs: BTreeMap<Name, PathBuf> = BTreeMap::new();

    for owner in subfolders(gate, &dirs.packs)? {
        for repo in subfolders(gate, &owner)? {
            if is_copy_beside_pack(&repo) {
                continue;
            }
            let name = repo_name(&repo)?;
            let first = match name.is_user_pack() {
                true => Some(dirs.user.clone()),
                false => packs.get(&name).cloned(),
            };
            if let Some(first) = first {
                return Err(Error::DuplicatePack {
                    pack: name.to_string(),
                    first,
                    second: repo,
                });
            }
            packs.insert(name, repo);
        }
    }

    Ok(packs.into_iter().collect())
}

/// Whether `repo` is a dot-prefixed staging or aside copy beside a pack (the copies
/// documented on `Store` in `src/packs/store.rs`), rather than a pack folder: a valid
/// pack name never starts with `.`, so those copies are never mistaken for one.
fn is_copy_beside_pack(repo: &Path) -> bool {
    repo.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// The repo folder's pack name.
fn repo_name(repo: &Path) -> Result<Name, Error> {
    let text = repo
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();

    Name::parse(&text).map_err(|source| Error::PackName {
        folder: repo.into(),
        source,
    })
}

// user rules

/// The file that defined each rule of the user layer, its own rules and its overrides.
type UserLayer = BTreeMap<RuleId, PathBuf>;

/// Adds a user rule to the `user` pack, or replaces a pack's rule when its key is a
/// qualified name.
fn apply_user_rule(
    loaded: &mut BTreeMap<RuleId, Rule>,
    user_layer: &mut UserLayer,
    parsed: Parsed,
    file: &Path,
) -> Result<(), Error> {
    let (id, source) = user_rule_identity(loaded, &parsed.key, file)?;
    if let Some(first) = user_layer.insert(id.clone(), file.into()) {
        return Err(Error::DuplicateRule {
            rule: id,
            first,
            second: file.into(),
        });
    }

    let rule = Rule {
        id: id.clone(),
        source,
        shape: parsed.shape,
    };
    loaded.insert(id, rule);
    Ok(())
}

/// The identity a user rule's key names: a rule of the `user` pack, or the pack rule a
/// qualified key overrides.
fn user_rule_identity(
    loaded: &BTreeMap<RuleId, Rule>,
    key: &str,
    file: &Path,
) -> Result<(RuleId, Source), Error> {
    let name_problem = |error: NameError| Error::Rule {
        file: file.into(),
        rule: key.to_owned(),
        problem: Problem::Name(error),
    };

    if !key.contains('/') {
        let rule = Name::parse(key).map_err(name_problem)?;
        let pack = Name::user_pack();
        return Ok((RuleId { pack, rule }, Source::Pack));
    }

    let id = RuleId::parse_qualified(key).map_err(name_problem)?;
    let from_a_pack = !id.pack.is_user_pack() && loaded.contains_key(&id);
    match from_a_pack {
        true => Ok((id, Source::UserOverride)),
        false => Err(Error::UnknownOverride {
            file: file.into(),
            rule: id,
        }),
    }
}

// files

/// Every `*.toml` file of `dir` with its contents, in name order.
fn read_rule_files<B: Backend>(
    gate: &Gate<B>,
    dir: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>, Error> {
    rule_files(gate, dir)?
        .into_iter()
        .map(|file| {
            let bytes = gate.read_own_file(&file)?;
            Ok((file, bytes))
        })
        .collect()
}

/// The folders in `dir`, in name order; none when `dir` is missing. Plain files such as
/// `.DS_Store` are passed over; a symlink or special file is refused.
fn subfolders<B: Backend>(gate: &Gate<B>, dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = entries(gate, dir)?;
    let mut folders = Vec::new();
    for (path, kind) in entries {
        match kind {
            FileKind::Dir => folders.push(path),
            FileKind::File => {}
            FileKind::Symlink | FileKind::Other => {
                return Err(Error::WrongKind {
                    path,
                    found: kind.label(),
                    expected: "folder",
                });
            }
        }
    }
    Ok(folders)
}

/// The `*.toml` files in `dir`, in name order; none when `dir` is missing. Other entries
/// are passed over; a `*.toml` entry that is not a regular file is refused.
fn rule_files<B: Backend>(gate: &Gate<B>, dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = entries(gate, dir)?;
    let mut files = Vec::new();
    for (path, kind) in entries {
        let is_toml = path
            .extension()
            .is_some_and(|extension| extension == "toml");
        match (is_toml, kind) {
            (false, _) => {}
            (true, FileKind::File) => files.push(path),
            (true, _) => {
                return Err(Error::WrongKind {
                    path,
                    found: kind.label(),
                    expected: "file",
                });
            }
        }
    }
    Ok(files)
}

/// Every entry of a folder with its kind, in name order. The folder is `lstat`ed, with
/// no symlink anywhere on its path, before it is listed.
fn entries<B: Backend>(gate: &Gate<B>, dir: &Path) -> Result<Vec<(PathBuf, FileKind)>, Error> {
    match gate.lstat_link_free(dir) {
        Ok(meta) if meta.kind == FileKind::Dir => {}
        Ok(meta) => {
            return Err(Error::WrongKind {
                path: dir.into(),
                found: meta.kind.label(),
                expected: "folder",
            });
        }
        Err(fs::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    }

    let mut names = gate.read_dir(dir)?;
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            let meta = gate.lstat(&path)?;
            Ok((path, meta.kind))
        })
        .collect()
}
