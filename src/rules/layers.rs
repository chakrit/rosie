//! Loading the rule layers (`docs/spec/rules.md#layers-and-overrides`): pulled packs,
//! then the user's rules, which may replace a pack's rule by its qualified name.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use mlua::{Lua, LuaOptions, StdLib};

use super::error::{Error, Problem};
use super::name::{Name, NameError, RuleId};
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

/// Every rule, keyed by identity, with the file that defined it.
type Loaded = BTreeMap<RuleId, (Rule, PathBuf)>;

pub(super) fn load<B: Backend>(gate: &Gate<B>, dirs: &RuleDirs) -> Result<Vec<Rule>, Error> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|error| Error::LuaStart(error.to_string()))?;
    let mut loaded = Loaded::new();

    for (pack, folder) in pack_folders(gate, dirs)? {
        for file in rule_files(gate, &folder)? {
            for parsed in read_rule_file(gate, &file, &lua)? {
                let rule = pack_rule_name(&parsed, &file)?;
                let id = RuleId {
                    pack: pack.clone(),
                    rule,
                };
                insert(&mut loaded, id, Source::Pack, parsed, &file)?;
            }
        }
    }

    let mut overrides = BTreeMap::new();
    for file in rule_files(gate, &dirs.user)? {
        for parsed in read_rule_file(gate, &file, &lua)? {
            apply_user_rule(&mut loaded, &mut overrides, parsed, &file)?;
        }
    }

    Ok(loaded.into_values().map(|(rule, _)| rule).collect())
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

/// A rule name in a pack, which cannot override another pack's rule.
fn pack_rule_name(parsed: &Parsed, file: &Path) -> Result<Name, Error> {
    let problem = match Name::parse(&parsed.key) {
        Ok(name) => return Ok(name),
        Err(_) if parsed.key.contains('/') => Problem::OverrideInPack,
        Err(error) => Problem::Name(error),
    };
    Err(Error::Rule {
        file: file.into(),
        rule: parsed.key.clone(),
        problem,
    })
}

// user rules

/// Adds a user rule to the `user` pack, or replaces a pack's rule when its key is a
/// qualified name.
fn apply_user_rule(
    loaded: &mut Loaded,
    overrides: &mut BTreeMap<RuleId, PathBuf>,
    parsed: Parsed,
    file: &Path,
) -> Result<(), Error> {
    let name_problem = |error: NameError| Error::Rule {
        file: file.into(),
        rule: parsed.key.clone(),
        problem: Problem::Name(error),
    };

    if !parsed.key.contains('/') {
        let rule = Name::parse(&parsed.key).map_err(name_problem)?;
        let pack = Name::user_pack();
        return insert(loaded, RuleId { pack, rule }, Source::Pack, parsed, file);
    }

    let id = RuleId::parse_qualified(&parsed.key).map_err(name_problem)?;
    let from_a_pack = !id.pack.is_user_pack() && loaded.contains_key(&id);
    if !from_a_pack {
        return Err(Error::UnknownOverride {
            file: file.into(),
            rule: id,
        });
    }
    if let Some(first) = overrides.insert(id.clone(), file.into()) {
        return Err(Error::DuplicateRule {
            rule: id,
            first,
            second: file.into(),
        });
    }

    let rule = Rule {
        id: id.clone(),
        source: Source::UserOverride,
        shape: parsed.shape,
    };
    loaded.insert(id, (rule, file.into()));
    Ok(())
}

fn insert(
    loaded: &mut Loaded,
    id: RuleId,
    source: Source,
    parsed: Parsed,
    file: &Path,
) -> Result<(), Error> {
    if let Some((_, first)) = loaded.get(&id) {
        return Err(Error::DuplicateRule {
            rule: id,
            first: first.clone(),
            second: file.into(),
        });
    }

    let rule = Rule {
        id: id.clone(),
        source,
        shape: parsed.shape,
    };
    loaded.insert(id, (rule, file.into()));
    Ok(())
}

// files

fn read_rule_file<B: Backend>(
    gate: &Gate<B>,
    file: &Path,
    lua: &Lua,
) -> Result<Vec<Parsed>, Error> {
    let bytes = gate.read_own_file(file)?;
    parse_file(&bytes, file, lua)
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
