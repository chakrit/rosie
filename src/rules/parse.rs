//! Reads one rule file into validated rule shapes. Serde reads the TOML; everything else
//! is checked here, and every problem names the rule.

use std::collections::BTreeMap;
use std::path::Path;

use mlua::Lua;
use serde::Deserialize;

use super::error::{Error, Problem};
use super::glob::Glob;
use super::lua::compile;
use super::rule::{
    Detection, FixedPath, FixedPaths, FolderName, FolderRule, Globs, LuaCode, Markers, Shape, Twin,
};
use crate::fs::ShellCommand;

/// One rule table from a file, keyed as written: a plain name or a qualified override.
pub(super) struct Parsed {
    pub key: String,
    pub shape: Shape,
}

/// Parses a rule file. `lua` compiles Lua detections to catch syntax errors at load.
pub(super) fn parse_file(bytes: &[u8], file: &Path, lua: &Lua) -> Result<Vec<Parsed>, Error> {
    let raw: RawFile = toml::from_slice(bytes).map_err(|source| Error::Toml {
        file: file.into(),
        source: Box::new(source),
    })?;

    raw.rules
        .into_iter()
        .map(|(key, value)| match parse_rule(&key, value, lua) {
            Ok(shape) => Ok(Parsed { key, shape }),
            Err(problem) => Err(Error::Rule {
                file: file.into(),
                rule: key,
                problem,
            }),
        })
        .collect()
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFile {
    rules: BTreeMap<String, toml::Value>,
}

/// Every rule field. `Option` tells an absent field from an empty one: an absent field
/// is checked against the strategy, an empty list is refused.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRule {
    strategy: String,
    target: Option<String>,
    target_aggressive: Option<String>,
    marker: Option<Vec<String>>,
    inside: Option<Vec<String>>,
    expr: Option<String>,
    script: Option<String>,
    paths: Option<Vec<String>>,
    paths_aggressive: Option<Vec<String>>,
    cmd: Option<String>,
    cmd_aggressive: Option<String>,
    shell: Option<String>,
    env: Option<BTreeMap<String, String>>,
}

impl RawRule {
    fn present_fields(&self) -> [(&'static str, bool); 12] {
        [
            ("target", self.target.is_some()),
            ("target_aggressive", self.target_aggressive.is_some()),
            ("marker", self.marker.is_some()),
            ("inside", self.inside.is_some()),
            ("expr", self.expr.is_some()),
            ("script", self.script.is_some()),
            ("paths", self.paths.is_some()),
            ("paths_aggressive", self.paths_aggressive.is_some()),
            ("cmd", self.cmd.is_some()),
            ("cmd_aggressive", self.cmd_aggressive.is_some()),
            ("shell", self.shell.is_some()),
            ("env", self.env.is_some()),
        ]
    }
}

#[derive(Clone, Copy)]
enum Strategy {
    Name,
    Marker,
    Lua,
    Path,
    Tool,
}

impl Strategy {
    fn parse(text: &str) -> Result<Strategy, Problem> {
        match text {
            "" => Err(Problem::MissingStrategy),
            "name" => Ok(Strategy::Name),
            "marker" => Ok(Strategy::Marker),
            "lua" => Ok(Strategy::Lua),
            "path" => Ok(Strategy::Path),
            "tool" => Ok(Strategy::Tool),
            other => Err(Problem::UnknownStrategy(other.into())),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Strategy::Name => "name",
            Strategy::Marker => "marker",
            Strategy::Lua => "lua",
            Strategy::Path => "path",
            Strategy::Tool => "tool",
        }
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Strategy::Name => &["target", "target_aggressive"],
            Strategy::Marker => &["target", "target_aggressive", "marker", "inside"],
            Strategy::Lua => &["target", "target_aggressive", "expr", "script"],
            Strategy::Path => &["paths", "paths_aggressive"],
            Strategy::Tool => &["cmd", "cmd_aggressive", "shell", "env"],
        }
    }
}

fn parse_rule(key: &str, value: toml::Value, lua: &Lua) -> Result<Shape, Problem> {
    let toml::Value::Table(table) = value else {
        return Err(Problem::NotATable);
    };
    let raw: RawRule = toml::Value::Table(table)
        .try_into()
        .map_err(|error| Problem::Invalid(Box::new(error)))?;
    let strategy = Strategy::parse(&raw.strategy)?;
    let foreign = raw
        .present_fields()
        .into_iter()
        .find(|&(field, present)| present && !strategy.fields().contains(&field));
    if let Some((field, _)) = foreign {
        return Err(Problem::FieldNotAllowed {
            field: field.into(),
            strategy: strategy.label(),
        });
    }

    match strategy {
        Strategy::Name => folder(&raw, Detection::Name),
        Strategy::Marker => folder(&raw, Detection::Marker(markers(&raw)?)),
        Strategy::Lua => folder(&raw, Detection::Lua(lua_code(key, &raw, lua)?)),
        Strategy::Path => paths(&raw),
        Strategy::Tool => tool(&raw),
    }
}

// strategies

fn folder(raw: &RawRule, detection: Detection) -> Result<Shape, Problem> {
    let normal = raw
        .target
        .as_deref()
        .map(|text| folder_name("target", text))
        .transpose()?;
    let aggressive = raw
        .target_aggressive
        .as_deref()
        .map(|text| folder_name("target_aggressive", text))
        .transpose()?;
    let target = Twin::from_parts(normal, aggressive)
        .ok_or(Problem::Missing("`target` or `target_aggressive`"))?;
    Ok(Shape::Folder(FolderRule { target, detection }))
}

fn markers(raw: &RawRule) -> Result<Markers, Problem> {
    let sibling = raw
        .marker
        .as_deref()
        .map(|patterns| globs("marker", patterns))
        .transpose()?;
    let inside = raw
        .inside
        .as_deref()
        .map(|patterns| globs("inside", patterns))
        .transpose()?;
    Markers::from_parts(sibling, inside).ok_or(Problem::Missing("`marker` or `inside`"))
}

fn lua_code(key: &str, raw: &RawRule, lua: &Lua) -> Result<LuaCode, Problem> {
    let code = match (&raw.expr, &raw.script) {
        (Some(expr), None) => LuaCode::Expr(expr.as_str().into()),
        (None, Some(script)) => LuaCode::Script(script.as_str().into()),
        (Some(_), Some(_)) => return Err(Problem::BothExprAndScript),
        (None, None) => return Err(Problem::Missing("one of `expr` or `script`")),
    };

    compile(lua, key, &code).map_err(|error| Problem::LuaSyntax(error.to_string()))?;
    Ok(code)
}

fn paths(raw: &RawRule) -> Result<Shape, Problem> {
    let normal = raw
        .paths
        .as_deref()
        .map(|texts| fixed_paths("paths", texts))
        .transpose()?;
    let aggressive = raw
        .paths_aggressive
        .as_deref()
        .map(|texts| fixed_paths("paths_aggressive", texts))
        .transpose()?;
    let paths = Twin::from_parts(normal, aggressive)
        .ok_or(Problem::Missing("`paths` or `paths_aggressive`"))?;
    Ok(Shape::Paths(paths))
}

fn tool(raw: &RawRule) -> Result<Shape, Problem> {
    let command = |field, text: &String| {
        let shell = raw
            .shell
            .clone()
            .unwrap_or_else(|| ShellCommand::DEFAULT_SHELL.to_owned());
        let env = raw.env.clone().unwrap_or_default();
        ShellCommand::new(shell, text.clone(), env)
            .map_err(|source| Problem::BadCommand { field, source })
    };

    let normal = raw
        .cmd
        .as_ref()
        .map(|text| command("cmd", text))
        .transpose()?;
    let aggressive = raw
        .cmd_aggressive
        .as_ref()
        .map(|text| command("cmd_aggressive", text))
        .transpose()?;
    let cmd = Twin::from_parts(normal, aggressive)
        .ok_or(Problem::Missing("`cmd` or `cmd_aggressive`"))?;
    Ok(Shape::Tool(cmd))
}

// fields

fn folder_name(field: &'static str, text: &str) -> Result<FolderName, Problem> {
    FolderName::parse(text).ok_or_else(|| Problem::BadFolderName {
        field,
        value: text.into(),
    })
}

fn globs(field: &'static str, patterns: &[String]) -> Result<Globs, Problem> {
    let parsed = patterns
        .iter()
        .map(|pattern| Glob::parse(pattern).map_err(|source| Problem::BadGlob { field, source }))
        .collect::<Result<Vec<_>, _>>()?;
    Globs::new(parsed).ok_or(Problem::EmptyList(field))
}

fn fixed_paths(field: &'static str, texts: &[String]) -> Result<FixedPaths, Problem> {
    let parsed = texts
        .iter()
        .map(|text| {
            FixedPath::parse(text).ok_or_else(|| Problem::BadPath {
                field,
                value: text.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(repeated) = parsed
        .iter()
        .enumerate()
        .find_map(|(index, path)| parsed[..index].contains(path).then(|| texts[index].clone()))
    {
        return Err(Problem::DuplicatePath {
            field,
            value: repeated,
        });
    }
    FixedPaths::new(parsed).ok_or(Problem::EmptyList(field))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::rules::rule::Tier;

    fn parse(text: &str) -> Result<Vec<Parsed>, Error> {
        let lua = Lua::new();
        parse_file(text.as_bytes(), Path::new("/packs/test.toml"), &lua)
    }

    fn only_shape(text: &str) -> Shape {
        let mut parsed = parse(text).expect("valid rule file");
        assert_eq!(parsed.len(), 1);
        parsed.remove(0).shape
    }

    /// The problem with the one rule in `text`, which must be named `r`.
    fn problem(text: &str) -> Problem {
        match parse(text) {
            Err(Error::Rule { rule, problem, .. }) => {
                assert_eq!(rule, "r");
                problem
            }
            Err(other) => panic!("expected a rule problem, got {other}"),
            Ok(_) => panic!("expected a rule problem, got a valid rule"),
        }
    }

    fn glob_texts(globs: &Globs) -> Vec<&str> {
        globs.iter().map(Glob::as_str).collect()
    }

    // strategies

    #[test]
    fn parses_a_name_rule() {
        let shape = only_shape("[rules.r]\nstrategy = \"name\"\ntarget = \"__pycache__\"");

        let Shape::Folder(folder) = shape else {
            panic!("expected a folder rule");
        };
        assert_eq!(folder.detection, Detection::Name);
        assert_eq!(
            folder.target.normal().map(FolderName::as_os_str),
            Some("__pycache__".as_ref())
        );
        assert_eq!(folder.target.aggressive(), None);
    }

    #[test]
    fn parses_a_marker_rule_with_sibling_and_inside_patterns() {
        let shape = only_shape(
            "[rules.r]\nstrategy = \"marker\"\ntarget = \".venv\"\n\
             marker = [\"pyproject.toml\", \"requirements*.txt\"]\ninside = [\"pyvenv.cfg\"]",
        );

        let Shape::Folder(FolderRule {
            detection: Detection::Marker(markers),
            ..
        }) = shape
        else {
            panic!("expected a marker rule");
        };
        assert_eq!(
            markers.sibling().map(glob_texts),
            Some(vec!["pyproject.toml", "requirements*.txt"])
        );
        assert_eq!(markers.inside().map(glob_texts), Some(vec!["pyvenv.cfg"]));
    }

    #[test]
    fn wraps_a_lua_expr_and_keeps_a_script_whole() {
        let expr = only_shape(
            "[rules.r]\nstrategy = \"lua\"\ntarget = \"build\"\nexpr = \"exists(path) -- note\"",
        );
        let script = only_shape(
            "[rules.r]\nstrategy = \"lua\"\ntarget = \"build\"\nscript = \"return true\"",
        );

        let chunk = |shape: &Shape| match shape {
            Shape::Folder(FolderRule {
                detection: Detection::Lua(code),
                ..
            }) => code.chunk().into_owned(),
            other => panic!("expected a lua rule, got {other:?}"),
        };
        assert_eq!(chunk(&expr), "return (exists(path) -- note\n)");
        assert_eq!(chunk(&script), "return true");
    }

    #[test]
    fn parses_paths_with_their_aggressive_twin() {
        let shape = only_shape(
            "[rules.r]\nstrategy = \"path\"\npaths = [\"~/.npm\", \"/Library/Caches/x\"]\n\
             paths_aggressive = [\"~/Library/Caches\"]",
        );

        let Shape::Paths(paths) = shape else {
            panic!("expected a path rule");
        };
        let home = &crate::fs::Paths::new(Path::new("/Users/me")).expect("absolute home");
        let resolved: Vec<(Tier, Vec<PathBuf>)> = paths
            .tiers()
            .map(|(tier, paths)| (tier, paths.iter().map(|p| p.resolve(home)).collect()))
            .collect();
        assert_eq!(
            resolved,
            vec![
                (
                    Tier::Normal,
                    vec![
                        PathBuf::from("/Users/me/.npm"),
                        PathBuf::from("/Library/Caches/x")
                    ]
                ),
                (
                    Tier::Aggressive,
                    vec![PathBuf::from("/Users/me/Library/Caches")]
                ),
            ]
        );
    }

    #[test]
    fn keeps_tool_commands_whole_for_the_default_shell() {
        let shape = only_shape(
            "[rules.r]\nstrategy = \"tool\"\ncmd = \"  docker system  prune --force \"\n\
             cmd_aggressive = \"docker system prune --all\"",
        );

        let Shape::Tool(cmd) = shape else {
            panic!("expected a tool rule");
        };
        let command = |text: &str| {
            ShellCommand::new("/bin/bash".into(), text.into(), BTreeMap::new()).expect("valid")
        };
        assert_eq!(
            cmd,
            Twin::Both {
                normal: command("  docker system  prune --force "),
                aggressive: command("docker system prune --all"),
            }
        );
    }

    #[test]
    fn gives_both_twins_the_rules_shell_and_environment() {
        let shape = only_shape(
            "[rules.r]\nstrategy = \"tool\"\nshell = \"/bin/zsh\"\n\
             cmd = \"brew cleanup\"\ncmd_aggressive = \"brew cleanup -s\"\n\
             env = { HOMEBREW_NO_ENV_HINTS = \"1\" }",
        );

        let Shape::Tool(Twin::Both { normal, aggressive }) = shape else {
            panic!("expected a tool rule with twins");
        };
        for command in [&normal, &aggressive] {
            assert_eq!(command.shell(), Path::new("/bin/zsh"));
            assert_eq!(
                command
                    .env()
                    .get("HOMEBREW_NO_ENV_HINTS")
                    .map(String::as_str),
                Some("1")
            );
        }
        assert_eq!(aggressive.cmd(), "brew cleanup -s");
    }

    #[test]
    fn accepts_an_aggressive_twin_alone() {
        let target =
            only_shape("[rules.r]\nstrategy = \"name\"\ntarget_aggressive = \"DerivedData\"");
        let paths = only_shape("[rules.r]\nstrategy = \"path\"\npaths_aggressive = [\"~/.m2\"]");
        let cmd = only_shape("[rules.r]\nstrategy = \"tool\"\ncmd_aggressive = \"x prune\"");
        let marker = only_shape(
            "[rules.r]\nstrategy = \"marker\"\ntarget_aggressive = \"Pods\"\nmarker = [\"Podfile\"]",
        );
        let lua = only_shape(
            "[rules.r]\nstrategy = \"lua\"\ntarget_aggressive = \"out\"\nexpr = \"true\"",
        );

        for folder in [target, marker, lua] {
            assert!(
                matches!(
                    folder,
                    Shape::Folder(FolderRule {
                        target: Twin::Aggressive(_),
                        ..
                    })
                ),
                "{folder:?}"
            );
        }
        assert!(matches!(paths, Shape::Paths(Twin::Aggressive(_))));
        assert!(matches!(cmd, Shape::Tool(Twin::Aggressive(_))));
    }

    // problems

    #[test]
    fn refuses_both_or_neither_of_expr_and_script() {
        let both = problem(
            "[rules.r]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"true\"\nscript = \"return true\"",
        );
        let neither = problem("[rules.r]\nstrategy = \"lua\"\ntarget = \"x\"");

        assert!(matches!(both, Problem::BothExprAndScript));
        assert!(matches!(neither, Problem::Missing(_)));
    }

    #[test]
    fn refuses_lua_that_does_not_compile() {
        let found = problem("[rules.r]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"exists(\"");

        let Problem::LuaSyntax(message) = found else {
            panic!("expected a Lua syntax problem, got {found}");
        };
        assert!(message.contains("error: r:"), "{message}");
        assert!(!message.contains(".rs:"), "{message}");
    }

    #[test]
    fn refuses_binary_lua_chunks() {
        let found =
            problem("[rules.r]\nstrategy = \"lua\"\ntarget = \"x\"\nscript = \"\\u001bLua\"");

        let Problem::LuaSyntax(message) = found else {
            panic!("expected a Lua syntax problem, got {found}");
        };
        assert!(
            message.contains("attempt to load a binary chunk"),
            "{message}"
        );
    }

    #[test]
    fn hands_shell_syntax_to_the_shell_unparsed() {
        let text = "sh -c 'rm $HOME/*' && é";
        let shape = only_shape(&format!("[rules.r]\nstrategy = \"tool\"\ncmd = {text:?}"));

        let Shape::Tool(Twin::Normal(command)) = shape else {
            panic!("expected a tool rule");
        };
        assert_eq!(command.cmd(), text);
    }

    #[test]
    fn refuses_a_tool_command_no_shell_can_run() {
        let cases = [
            ("shell = \"bash\"\ncmd = \"x\"", "cmd"),
            ("cmd = \"x\"\nenv = { \"-i\" = \"1\" }", "cmd"),
            ("cmd_aggressive = \"  \"", "cmd_aggressive"),
            ("cmd = \"\"", "cmd"),
        ];

        for (fields, want) in cases {
            let text = format!("[rules.r]\nstrategy = \"tool\"\n{fields}");
            let found = problem(&text);
            assert!(
                matches!(found, Problem::BadCommand { field, .. } if field == want),
                "{text}: {found}"
            );
        }
    }

    #[test]
    fn refuses_fields_of_another_strategy() {
        let cases = [
            (
                "[rules.r]\nstrategy = \"name\"\ntarget = \"x\"\nmarker = [\"a\"]",
                "marker",
                "name",
            ),
            (
                "[rules.r]\nstrategy = \"marker\"\ntarget = \"x\"\nmarker = [\"a\"]\nexpr = \"true\"",
                "expr",
                "marker",
            ),
            (
                "[rules.r]\nstrategy = \"path\"\npaths = [\"~/x\"]\ntarget = \"x\"",
                "target",
                "path",
            ),
            (
                "[rules.r]\nstrategy = \"tool\"\ncmd = \"x\"\npaths = [\"~/x\"]",
                "paths",
                "tool",
            ),
            (
                "[rules.r]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"true\"\ninside = [\"a\"]",
                "inside",
                "lua",
            ),
            (
                "[rules.r]\nstrategy = \"path\"\npaths = [\"~/x\"]\ntarget_aggressive = \"x\"",
                "target_aggressive",
                "path",
            ),
            (
                "[rules.r]\nstrategy = \"name\"\ntarget = \"x\"\nscript = \"return true\"",
                "script",
                "name",
            ),
            (
                "[rules.r]\nstrategy = \"tool\"\ncmd = \"x\"\npaths_aggressive = [\"~/x\"]",
                "paths_aggressive",
                "tool",
            ),
            (
                "[rules.r]\nstrategy = \"path\"\npaths = [\"~/x\"]\ncmd = \"x\"",
                "cmd",
                "path",
            ),
            (
                "[rules.r]\nstrategy = \"marker\"\ntarget = \"x\"\nmarker = [\"a\"]\ncmd_aggressive = \"x\"",
                "cmd_aggressive",
                "marker",
            ),
        ];

        for (text, field, strategy) in cases {
            let found = problem(text);
            assert!(
                matches!(&found, Problem::FieldNotAllowed { field: f, strategy: s } if f == field && *s == strategy),
                "{text}: got {found}"
            );
        }
    }

    #[test]
    fn refuses_missing_and_unknown_strategies_and_fields() {
        assert!(matches!(
            problem("[rules.r]\ntarget = \"x\""),
            Problem::MissingStrategy
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"regex\"\ntarget = \"x\""),
            Problem::UnknownStrategy(s) if s == "regex"
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"name\"\ntargte = \"x\""),
            Problem::Invalid(_)
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"name\"\ntarget = [\"x\"]"),
            Problem::Invalid(_)
        ));
        assert!(matches!(problem("[rules]\nr = 3"), Problem::NotATable));
    }

    #[test]
    fn refuses_rules_with_nothing_to_clean() {
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"name\""),
            Problem::Missing(_)
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"marker\"\ntarget = \"x\""),
            Problem::Missing(_)
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"path\""),
            Problem::Missing(_)
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"tool\""),
            Problem::Missing(_)
        ));
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"path\"\npaths = []"),
            Problem::EmptyList("paths")
        ));
        assert!(matches!(
            problem(
                "[rules.r]\nstrategy = \"marker\"\ntarget = \"x\"\nmarker = []\ninside = [\"a\"]"
            ),
            Problem::EmptyList("marker")
        ));
    }

    #[test]
    fn refuses_bad_targets_paths_globs_and_commands() {
        for target in ["\"\"", "\"a/b\"", "\".\"", "\"..\"", "\"a\\u0000b\""] {
            let text = format!("[rules.r]\nstrategy = \"name\"\ntarget = {target}");
            assert!(
                matches!(problem(&text), Problem::BadFolderName { .. }),
                "{target}"
            );
        }
        for path in ["relative/x", "~user/x", ""] {
            let text = format!("[rules.r]\nstrategy = \"path\"\npaths = [{path:?}]");
            assert!(matches!(problem(&text), Problem::BadPath { .. }), "{path}");
        }
        assert!(matches!(
            problem("[rules.r]\nstrategy = \"marker\"\ntarget = \"x\"\nmarker = [\"a/b\"]"),
            Problem::BadGlob {
                field: "marker",
                ..
            }
        ));
    }

    #[test]
    fn loads_an_aggressive_twin_that_repeats_the_normal_value() {
        for text in [
            "[rules.r]\nstrategy = \"name\"\ntarget = \"build\"\ntarget_aggressive = \"build\"",
            "[rules.r]\nstrategy = \"path\"\npaths = [\"~/a\", \"~/b\"]\n\
             paths_aggressive = [\"~/c\", \"~/a\"]",
            "[rules.r]\nstrategy = \"tool\"\ncmd = \"x prune\"\ncmd_aggressive = \"x prune\"",
        ] {
            let both = match only_shape(text) {
                Shape::Folder(FolderRule { target, .. }) => matches!(target, Twin::Both { .. }),
                Shape::Paths(paths) => matches!(paths, Twin::Both { .. }),
                Shape::Tool(cmd) => matches!(cmd, Twin::Both { .. }),
            };
            assert!(both, "{text}");
        }
    }

    #[test]
    fn refuses_a_path_repeated_within_one_list() {
        let text = "[rules.r]\nstrategy = \"path\"\npaths = [\"~/a\", \"~/a\"]";
        assert!(
            matches!(problem(text), Problem::DuplicatePath { field: "paths", .. }),
            "{}",
            problem(text)
        );

        let text = "[rules.r]\nstrategy = \"path\"\npaths_aggressive = [\"~/a\", \"~/b\", \"~/a\"]";
        assert!(
            matches!(
                problem(text),
                Problem::DuplicatePath {
                    field: "paths_aggressive",
                    ..
                }
            ),
            "{}",
            problem(text)
        );
    }

    #[test]
    fn names_the_file_for_malformed_toml_and_unknown_top_level_keys() {
        for text in ["[rules.r\n", "[other]\nx = 1"] {
            assert!(
                matches!(parse(text), Err(Error::Toml { file, .. }) if file == Path::new("/packs/test.toml")),
                "{text}"
            );
        }
    }
}
