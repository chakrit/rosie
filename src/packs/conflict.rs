//! Rules from different packs that clean the same target the same way
//! (`docs/spec/rules.md#packs`).
//!
//! Two rules clash when their whole rule tables are equal: same strategy, same target,
//! same detection fields. This compares the TOML tables directly; it needs no rule model.

use std::fmt;

use toml::{Table, Value};

use super::archive::RuleFile;
use super::error::Error;
use super::source::Source;

const RULES_KEY: &str = "rules";

/// A freshly pulled rule that duplicates a rule of another installed pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The pulled rule, as `pack/rule`.
    pub pulled: String,
    /// The other pack's rule, as `pack/rule`.
    pub installed: String,
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} and {} clean the same target the same way",
            self.pulled, self.installed
        )
    }
}

/// A pack's rules as `pack/rule` names with their tables.
pub(super) struct PackRules {
    rules: Vec<(String, Value)>,
}

impl PackRules {
    /// Parses every rule file of a pack. A file that is not TOML, or whose `rules` key is
    /// not a table, is an error naming it.
    pub(super) fn parse(source: &Source, files: &[RuleFile]) -> Result<PackRules, Error> {
        let per_file = files
            .iter()
            .map(|file| rules_in(source, file))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(PackRules {
            rules: per_file.into_iter().flatten().collect(),
        })
    }

    /// The rules of `self` that duplicate a rule of `other`.
    pub(super) fn conflicts_with(&self, other: &PackRules) -> Vec<Conflict> {
        self.rules
            .iter()
            .flat_map(|(pulled, table)| {
                other
                    .rules
                    .iter()
                    .filter(move |(_, other_table)| other_table == table)
                    .map(move |(installed, _)| Conflict {
                        pulled: pulled.clone(),
                        installed: installed.clone(),
                    })
            })
            .collect()
    }
}

fn rules_in(source: &Source, file: &RuleFile) -> Result<Vec<(String, Value)>, Error> {
    let file_label = format!("{source}/{}", file.name().to_string_lossy());
    let text = std::str::from_utf8(file.contents()).map_err(|_| Error::RuleFileNotText {
        file: file_label.clone(),
    })?;

    let mut document: Table = text.parse().map_err(|error| Error::RuleFile {
        file: file_label.clone(),
        source: Box::new(error),
    })?;
    let Some(rules) = document.remove(RULES_KEY) else {
        return Ok(Vec::new());
    };
    let Value::Table(rules) = rules else {
        return Err(Error::RulesNotTable { file: file_label });
    };

    let qualified = rules
        .into_iter()
        .map(|(name, table)| (format!("{}/{name}", source.pack()), table))
        .collect();
    Ok(qualified)
}
