//! Loading one pack's rule files into rules (`docs/spec/rules.md#packs`). This is the one
//! definition of a valid pack: the rule layers load every pulled pack through it, and a
//! pull runs it over a downloaded pack before installing it.

use std::collections::BTreeMap;
use std::path::Path;

use super::error::{Error, Problem};
use super::lua::new_parser_state;
use super::name::{Name, RuleId};
use super::parse::{Parsed, parse_file};
use super::rule::{Rule, Source};

/// The rules of the pack `pack`, from its `files`, each given as its label (for error
/// messages) and contents, in rule name order. Every file must parse, every rule key
/// must be a plain rule name, and no name may be defined twice across the files.
pub fn load_pack<'a>(
    pack: &Name,
    files: impl IntoIterator<Item = (&'a Path, &'a [u8])>,
) -> Result<Vec<Rule>, Error> {
    let lua = new_parser_state()?;
    let mut defined: BTreeMap<RuleId, (Rule, &Path)> = BTreeMap::new();

    for (file, bytes) in files {
        for parsed in parse_file(bytes, file, &lua)? {
            let rule = pack_rule(pack, parsed, file)?;
            define(&mut defined, rule, file)?;
        }
    }

    Ok(defined.into_values().map(|(rule, _)| rule).collect())
}

/// A rule of a pack, whose key must be a plain rule name: a pack cannot override
/// another pack's rule.
fn pack_rule(pack: &Name, parsed: Parsed, file: &Path) -> Result<Rule, Error> {
    let rule = Name::parse(&parsed.key).map_err(|error| {
        let problem = match parsed.key.contains('/') {
            true => Problem::OverrideInPack,
            false => Problem::Name(error),
        };
        Error::Rule {
            file: file.into(),
            rule: parsed.key.clone(),
            problem,
        }
    })?;

    Ok(Rule {
        id: RuleId {
            pack: pack.clone(),
            rule,
        },
        source: Source::Pack,
        shape: parsed.shape,
    })
}

fn define<'a>(
    defined: &mut BTreeMap<RuleId, (Rule, &'a Path)>,
    rule: Rule,
    file: &'a Path,
) -> Result<(), Error> {
    if let Some((_, first)) = defined.get(&rule.id) {
        return Err(Error::DuplicateRule {
            rule: rule.id,
            first: first.into(),
            second: file.into(),
        });
    }

    defined.insert(rule.id.clone(), (rule, file));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Name {
        Name::parse("rosie").expect("valid pack name")
    }

    #[test]
    fn loads_every_rule_across_files_under_the_pack() {
        let a = (
            Path::new("a.toml"),
            "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"".as_bytes(),
        );
        let b = (
            Path::new("b.toml"),
            "[rules.y]\nstrategy = \"path\"\npaths = [\"~/.y\"]".as_bytes(),
        );

        let rules = load_pack(&pack(), [a, b]).expect("valid pack");

        let ids: Vec<String> = rules.iter().map(|rule| rule.id().to_string()).collect();
        assert_eq!(ids, vec!["rosie/x", "rosie/y"]);
    }

    #[test]
    fn names_the_file_of_a_rule_that_fails_to_parse() {
        let bad = (
            Path::new("pack/bad.toml"),
            "[rules.x]\nstrategy = \"regex\"".as_bytes(),
        );

        let error = load_pack(&pack(), [bad]).expect_err("invalid strategy refused");

        assert!(
            matches!(&error, Error::Rule { file, .. } if file == Path::new("pack/bad.toml")),
            "{error}"
        );
    }

    #[test]
    fn names_both_files_of_a_rule_defined_twice() {
        let rule = "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"".as_bytes();

        let error = load_pack(
            &pack(),
            [(Path::new("a.toml"), rule), (Path::new("b.toml"), rule)],
        )
        .expect_err("duplicate refused");

        assert!(
            matches!(&error, Error::DuplicateRule { first, second, .. }
                if first == Path::new("a.toml") && second == Path::new("b.toml")),
            "{error}"
        );
    }
}
