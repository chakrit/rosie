//! The sandboxed Lua detection strategy (`docs/spec/rules.md#lua`).
//!
//! A [`LuaSandbox`] owns one Lua 5.4 state. It is not `Send`, so a parallel scan holds
//! one per worker thread, and it compiles each rule's chunk once. Each check runs in a
//! fresh environment holding only an allowlist of pure standard functions, copied so no
//! check can alter another's, the candidate folder as `path`, and the read-only API
//! below. Nothing in it can reach `os`, `io`, `require`, `load`, `debug`, metatables,
//! or the random generator's seed, and no global one check sets is seen by the next.
//! The API borrows the gate through `Lua::scope`, so every read goes through it and
//! refuses symlinks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, LuaString, Scope, StdLib, Table, Value};

use super::error::Error;
use super::glob::Glob;
use super::name::RuleId;
use super::rule::LuaCode;
use crate::fs::{self, Backend, FileKind, Gate, resolve_dots};

/// Standard functions a check may call: pure, with no access to code loading,
/// metatables, the garbage collector, or output.
const BASE_FUNCTIONS: [&str; 14] = [
    "assert", "error", "ipairs", "next", "pairs", "pcall", "rawequal", "rawget", "rawlen",
    "select", "tonumber", "tostring", "type", "xpcall",
];

/// The library functions a check may call, each library given to a check as its own
/// table. `math.random` and `math.randomseed` are left out: their generator state is
/// shared by every check on the thread.
const LIBRARIES: [(&str, &[&str]); 4] = [
    (
        "string",
        &[
            "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match", "pack",
            "packsize", "rep", "reverse", "sub", "unpack", "upper",
        ],
    ),
    (
        "table",
        &[
            "concat", "insert", "move", "pack", "remove", "sort", "unpack",
        ],
    ),
    (
        "math",
        &[
            "abs",
            "acos",
            "asin",
            "atan",
            "ceil",
            "cos",
            "exp",
            "floor",
            "fmod",
            "huge",
            "log",
            "max",
            "maxinteger",
            "min",
            "mininteger",
            "modf",
            "pi",
            "sin",
            "sqrt",
            "tan",
            "tointeger",
            "type",
            "ult",
        ],
    ),
    (
        "utf8",
        &["char", "charpattern", "codepoint", "codes", "len", "offset"],
    ),
];

/// A fresh Lua state with no standard library loaded, for parsing and validating rules
/// (never for running a check, which needs [`LuaSandbox`]'s allowlisted environment).
pub(super) fn new_parser_state() -> Result<Lua, Error> {
    Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|error| Error::LuaStart(error.to_string()))
}

/// Compiles a rule's Lua as text, never as binary bytecode, into a function named after
/// the rule so errors point at it. The function's environment is empty until a check
/// gives it one.
pub(super) fn compile(lua: &Lua, rule: &str, code: &LuaCode) -> mlua::Result<Function> {
    lua.load(&*code.chunk())
        .set_name(format!("={rule}"))
        .set_mode(ChunkMode::Text)
        .set_environment(lua.create_table()?)
        .into_function()
}

pub struct LuaSandbox {
    lua: Lua,
    compiled: RefCell<HashMap<RuleId, Compiled>>,
}

/// A rule's chunk as compiled in this sandbox's state.
struct Compiled {
    code: LuaCode,
    function: Function,
}

impl LuaSandbox {
    pub fn new() -> Result<LuaSandbox, Error> {
        let libraries = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8;
        let lua = Lua::new_with(libraries, LuaOptions::default())
            .map_err(|error| Error::LuaStart(error.to_string()))?;
        // String values index this state's own `string` table through their shared
        // metatable, not the per-check copy, so `('').dump` would still reach `dump`.
        lua.globals()
            .raw_get::<Table>("string")
            .and_then(|string| string.raw_set("dump", Value::Nil))
            .map_err(|error| Error::LuaStart(error.to_string()))?;

        Ok(LuaSandbox {
            lua,
            compiled: RefCell::default(),
        })
    }

    /// Runs a rule's Lua against a candidate folder. A truthy result matches; a Lua
    /// error is returned against the rule, and the candidate does not match.
    pub fn accepts<B: Backend>(
        &self,
        gate: &Gate<B>,
        rule: &RuleId,
        code: &LuaCode,
        candidate: &Path,
    ) -> Result<bool, Error> {
        let result = self.function(rule, code).and_then(|function| {
            self.lua.scope(|scope| {
                let env = self.environment(scope, gate, candidate)?;
                if !function.set_environment(env)? {
                    return Err(mlua::Error::runtime("the chunk takes no environment"));
                }
                function.call::<Value>(())
            })
        });

        match result {
            Ok(value) => Ok(!matches!(value, Value::Nil | Value::Boolean(false))),
            Err(error) => Err(Error::Lua {
                rule: rule.clone(),
                path: candidate.into(),
                message: error_line(&error),
            }),
        }
    }

    /// The rule's compiled chunk, compiled on its first check in this sandbox.
    fn function(&self, rule: &RuleId, code: &LuaCode) -> mlua::Result<Function> {
        let cached = self
            .compiled
            .borrow()
            .get(rule)
            .filter(|compiled| compiled.code == *code)
            .map(|compiled| compiled.function.clone());
        if let Some(function) = cached {
            return Ok(function);
        }

        let function = compile(&self.lua, &rule.to_string(), code)?;

        let compiled = Compiled {
            code: code.clone(),
            function: function.clone(),
        };
        self.compiled.borrow_mut().insert(rule.clone(), compiled);
        Ok(function)
    }

    fn environment<'scope, B: Backend>(
        &self,
        scope: &'scope Scope<'scope, '_>,
        gate: &'scope Gate<B>,
        candidate: &Path,
    ) -> mlua::Result<Table> {
        let lua = &self.lua;
        let globals = lua.globals();
        let env = lua.create_table()?;

        for name in BASE_FUNCTIONS {
            env.raw_set(name, globals.raw_get::<Value>(name)?)?;
        }
        for (name, members) in LIBRARIES {
            let library: Table = globals.raw_get(name)?;
            env.raw_set(name, library_copy(lua, &library, members)?)?;
        }
        env.raw_set("path", path_string(lua, candidate)?)?;

        let exists = scope.create_function(|_, path: LuaString| exists(gate, &lua_path(&path)))?;
        let glob = scope.create_function(|lua, pattern: LuaString| {
            let found = glob(gate, &lua_path(&pattern))?;
            let strings = found
                .iter()
                .map(|path| path_string(lua, path))
                .collect::<mlua::Result<Vec<_>>>()?;
            lua.create_sequence_from(strings)
        })?;
        let read_json = scope.create_function(|lua, path: LuaString| {
            let bytes = read(gate, &lua_path(&path))?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(mlua::Error::runtime)?;
            json_value(lua, value)
        })?;
        let read_toml = scope.create_function(|lua, path: LuaString| {
            let bytes = read(gate, &lua_path(&path))?;
            let table: toml::Table = toml::from_slice(&bytes).map_err(mlua::Error::runtime)?;
            toml_value(lua, toml::Value::Table(table))
        })?;
        let parent = scope.create_function(|lua, path: LuaString| {
            let path = lua_path(&path);
            if !path.is_absolute() {
                return Err(mlua::Error::runtime(fs::Error::Relative { path }));
            }
            path.parent()
                .map(|parent| path_string(lua, parent))
                .transpose()
        })?;

        env.raw_set("exists", exists)?;
        env.raw_set("glob", glob)?;
        env.raw_set("read_json", read_json)?;
        env.raw_set("read_toml", read_toml)?;
        env.raw_set("parent", parent)?;
        Ok(env)
    }
}

/// A Lua error's own message line, without the stack traceback `mlua` appends: that
/// traceback names Lua's own frames, not anything scan output should show.
fn error_line(error: &mlua::Error) -> String {
    let text = error.to_string();
    match text.split_once("\nstack traceback:") {
        Some((line, _)) => line.to_string(),
        None => text,
    }
}

// the API

/// Whether a path exists; a path through or at a symlink is an error.
fn exists<B: Backend>(gate: &Gate<B>, path: &Path) -> mlua::Result<bool> {
    match gate.lstat_link_free(path) {
        Ok(_) => Ok(true),
        Err(error) if error.has_vanished() => Ok(false),
        Err(error) => Err(mlua::Error::runtime(error)),
    }
}

/// Expands an absolute pattern whose components may hold `*` and `?`, sorted. Symlinks
/// are never matched or descended.
fn glob<B: Backend>(gate: &Gate<B>, pattern: &Path) -> mlua::Result<Vec<PathBuf>> {
    let pattern = resolve_dots(pattern).map_err(mlua::Error::runtime)?;
    let parts = pattern
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(Part::parse(name)),
            _ => None,
        })
        .collect::<mlua::Result<Vec<_>>>()?;
    let literal_len = parts
        .iter()
        .position(|part| matches!(part, Part::Wildcard(_)))
        .unwrap_or(parts.len());

    let base: PathBuf = std::iter::once(OsStr::new("/"))
        .chain(parts[..literal_len].iter().filter_map(Part::literal))
        .collect();
    let base_kind = match gate.lstat_link_free(&base) {
        Ok(meta) => meta.kind,
        Err(error) if error.has_vanished() => return Ok(Vec::new()),
        Err(error) => return Err(mlua::Error::runtime(error)),
    };

    let mut found = vec![(base, base_kind)];
    for part in &parts[literal_len..] {
        let folders = found
            .into_iter()
            .filter(|&(_, kind)| kind == FileKind::Dir)
            .map(|(folder, _)| folder);
        found = folders
            .map(|folder| children_matching(gate, &folder, part))
            .collect::<mlua::Result<Vec<_>>>()?
            .concat();
    }

    let mut paths: Vec<PathBuf> = found.into_iter().map(|(path, _)| path).collect();
    paths.sort();
    Ok(paths)
}

/// One component of a glob pattern.
enum Part<'a> {
    Literal(&'a OsStr),
    Wildcard(Glob),
}

impl<'a> Part<'a> {
    fn parse(name: &'a OsStr) -> mlua::Result<Part<'a>> {
        let has_wildcard = name
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'*' | b'?'));
        if !has_wildcard {
            return Ok(Part::Literal(name));
        }

        let Some(text) = name.to_str() else {
            return Err(mlua::Error::runtime("a glob pattern must be UTF-8"));
        };
        let glob = Glob::parse(text).map_err(mlua::Error::runtime)?;
        Ok(Part::Wildcard(glob))
    }

    fn literal(&self) -> Option<&'a OsStr> {
        match self {
            Part::Literal(name) => Some(name),
            Part::Wildcard(_) => None,
        }
    }
}

/// The entries of a link-free folder that one pattern component names, without
/// symlinks.
fn children_matching<B: Backend>(
    gate: &Gate<B>,
    folder: &Path,
    part: &Part,
) -> mlua::Result<Vec<(PathBuf, FileKind)>> {
    let candidates = match part {
        Part::Literal(name) => vec![folder.join(name)],
        Part::Wildcard(glob) => {
            let listing = gate.read_dir(folder).map_err(mlua::Error::runtime)?;
            listing
                .into_iter()
                .filter(|entry| entry.to_str().is_some_and(|entry| glob.matches(entry)))
                .map(|entry| folder.join(entry))
                .collect()
        }
    };

    let mut children = Vec::new();
    for child in candidates {
        match gate.lstat(&child) {
            Ok(meta) if meta.kind == FileKind::Symlink => {}
            Ok(meta) => children.push((child, meta.kind)),
            Err(error) if error.has_vanished() => {}
            Err(error) => return Err(mlua::Error::runtime(error)),
        }
    }
    Ok(children)
}

fn read<B: Backend>(gate: &Gate<B>, path: &Path) -> mlua::Result<Vec<u8>> {
    gate.read_file(path).map_err(mlua::Error::runtime)
}

// values

fn lua_path(text: &LuaString) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&text.as_bytes()))
}

fn path_string(lua: &Lua, path: &Path) -> mlua::Result<LuaString> {
    lua.create_string(path.as_os_str().as_bytes())
}

/// A new table holding only the named members of a library.
fn library_copy(lua: &Lua, library: &Table, members: &[&str]) -> mlua::Result<Table> {
    let copy = lua.create_table()?;
    for &member in members {
        copy.raw_set(member, library.raw_get::<Value>(member)?)?;
    }
    Ok(copy)
}

fn json_value(lua: &Lua, value: serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as Json;

    let converted = match value {
        Json::Null => Value::Nil,
        Json::Bool(flag) => Value::Boolean(flag),
        Json::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(integer), _) => Value::Integer(integer),
            (None, Some(float)) => Value::Number(float),
            (None, None) => {
                return Err(mlua::Error::runtime(format!("unsupported number {number}")));
            }
        },
        Json::String(text) => Value::String(lua.create_string(text)?),
        Json::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| json_value(lua, item))
                .collect::<mlua::Result<Vec<_>>>()?;
            Value::Table(lua.create_sequence_from(items)?)
        }
        Json::Object(fields) => {
            let table = lua.create_table()?;
            for (key, field) in fields {
                table.raw_set(key, json_value(lua, field)?)?;
            }
            Value::Table(table)
        }
    };
    Ok(converted)
}

/// TOML values as Lua values; datetimes become their TOML text.
fn toml_value(lua: &Lua, value: toml::Value) -> mlua::Result<Value> {
    use toml::Value as Toml;

    let converted = match value {
        Toml::String(text) => Value::String(lua.create_string(text)?),
        Toml::Integer(integer) => Value::Integer(integer),
        Toml::Float(float) => Value::Number(float),
        Toml::Boolean(flag) => Value::Boolean(flag),
        Toml::Datetime(datetime) => Value::String(lua.create_string(datetime.to_string())?),
        Toml::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| toml_value(lua, item))
                .collect::<mlua::Result<Vec<_>>>()?;
            Value::Table(lua.create_sequence_from(items)?)
        }
        Toml::Table(fields) => {
            let table = lua.create_table()?;
            for (key, field) in fields {
                table.raw_set(key, toml_value(lua, field)?)?;
            }
            Value::Table(table)
        }
    };
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Bounds;
    use crate::fs::Op;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::rules::name::Name;
    use std::io::ErrorKind;

    const APP: &str = "/Users/me/code/app/build";

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![PathBuf::from("/Users/me/code")],
            config_dir: PathBuf::from("/Users/me/.config/rosie"),
            data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
            user_uid: USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn rule() -> RuleId {
        RuleId {
            pack: Name::parse("test").expect("name"),
            rule: Name::parse("build").expect("name"),
        }
    }

    fn run(fake: &FakeBackend, code: LuaCode) -> Result<bool, Error> {
        let sandbox = LuaSandbox::new().expect("lua starts");
        sandbox.accepts(&gate(fake), &rule(), &code, Path::new(APP))
    }

    fn expr(fake: &FakeBackend, expr: &str) -> Result<bool, Error> {
        run(fake, LuaCode::Expr(expr.into()))
    }

    fn script(fake: &FakeBackend, script: &str) -> Result<bool, Error> {
        run(fake, LuaCode::Script(script.into()))
    }

    fn fixture() -> FakeBackend {
        let fake = FakeBackend::new();
        fake.add_dir(APP);
        fake.add_file(
            "/Users/me/code/app/package.json",
            r#"{"name": "app", "version": 3, "ratio": 1.5, "private": false, "license": null,
                "scripts": {"build": "vite build"}, "tags": ["a", "b"]}"#,
        );
        fake.add_file(
            "/Users/me/code/app/Cargo.toml",
            "[package]\nname = \"app\"\nedition = \"2024\"\nversion = 3\nratio = 1.5\n\
             publish = false\nreleased = 1979-05-27\nkeywords = [\"a\", \"b\"]\n",
        );
        fake.add_file("/Users/me/code/app/main.tf", "");
        fake.add_file("/Users/me/code/app/vars.tf", "");
        fake.add_file("/Users/me/code/app/build/out.o", "");
        fake
    }

    #[test]
    fn sees_the_candidate_as_path_and_matches_on_truthy_results() {
        let fake = fixture();

        let cases = [
            (format!("path == {APP:?}"), true),
            ("path ~= nil and 0".into(), true),
            ("''".into(), true),
            ("false".into(), false),
            ("nil".into(), false),
        ];
        for (code, want) in cases {
            assert_eq!(expr(&fake, &code).expect("runs"), want, "{code}");
        }
        assert!(!script(&fake, "local x = 1").expect("runs"));
    }

    #[test]
    fn runs_a_script_as_written_without_an_implicit_return() {
        let fake = fixture();

        let bare = script(&fake, "exists(parent(path) .. '/Cargo.toml')");

        assert!(!bare.expect("runs"));
    }

    #[test]
    fn refuses_binary_chunks() {
        let fake = fixture();

        let result = script(&fake, "\u{1b}Lua");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. })
                if message.contains("attempt to load a binary chunk")),
            "{result:?}"
        );
    }

    #[test]
    fn has_no_random_state_to_share_between_checks() {
        let fake = fixture();

        let found = expr(
            &fake,
            "math.random == nil and math.randomseed == nil and math.floor(1.5) == 1",
        );

        assert!(found.expect("runs"));
    }

    #[test]
    fn exists_tests_paths_through_the_gate() {
        let fake = fixture();

        assert!(expr(&fake, "exists(parent(path) .. '/Cargo.toml')").expect("runs"));
        assert!(!expr(&fake, "exists(parent(path) .. '/go.mod')").expect("runs"));
        assert!(!expr(&fake, "exists(parent(path) .. '/Cargo.toml/x')").expect("runs"));
    }

    #[test]
    fn exists_refuses_a_relative_path() {
        let fake = fixture();

        let result = expr(&fake, "exists('rel/x')");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. })
                if message.contains("rel/x is not an absolute path")),
            "{result:?}"
        );
    }

    #[test]
    fn parent_walks_up_and_ends_at_the_root() {
        let fake = fixture();

        assert!(expr(&fake, "parent(path) == '/Users/me/code/app'").expect("runs"));
        assert!(expr(&fake, "parent('/') == nil").expect("runs"));
    }

    #[test]
    fn parent_refuses_a_relative_path() {
        let fake = fixture();

        let result = expr(&fake, "parent('rel/x')");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. })
                if message.contains("rel/x is not an absolute path")),
            "{result:?}"
        );
    }

    /// The fake lists a folder in name order, so this cannot tell whether `glob` sorts;
    /// the real `read_dir` order is arbitrary.
    #[test]
    fn glob_expands_wildcards_in_any_component() {
        let fake = fixture();
        fake.add_file("/Users/me/code/other/main.tf", "");

        let found = script(
            &fake,
            "local found = glob('/Users/me/code/*/*.tf')\n\
             return table.concat(found, ',') == \
             '/Users/me/code/app/main.tf,/Users/me/code/app/vars.tf,/Users/me/code/other/main.tf'",
        );
        let none = expr(&fake, "#glob('/Users/me/nowhere/*.tf') == 0");

        assert!(found.expect("runs"));
        assert!(none.expect("runs"));
    }

    #[test]
    fn glob_matches_a_question_mark_as_one_character() {
        let fake = fixture();

        let found = script(
            &fake,
            "return table.concat(glob('/Users/me/code/app/ma?n.tf'), ',') == \
             '/Users/me/code/app/main.tf'",
        );

        assert!(found.expect("runs"));
    }

    #[test]
    fn glob_refuses_a_component_outside_the_glob_syntax() {
        let fake = fixture();

        let not_utf8 = expr(&fake, "glob('/Users/me/code/\\xff*')");
        let braces = expr(&fake, "glob('/Users/me/code/*.{tf,toml}')");

        assert!(
            matches!(&not_utf8, Err(Error::Lua { message, .. }) if message.contains("UTF-8")),
            "{not_utf8:?}"
        );
        assert!(
            matches!(&braces, Err(Error::Lua { message, .. }) if message.contains("only `*` and `?`")),
            "{braces:?}"
        );
    }

    #[test]
    fn glob_reports_a_failed_listing_or_inspection() {
        let unlistable = fixture();
        unlistable.fail_on(
            "/Users/me/code/app",
            Op::ReadDir,
            ErrorKind::PermissionDenied,
        );
        let uninspectable = fixture();
        uninspectable.fail_on(
            "/Users/me/code/app/main.tf",
            Op::Lstat,
            ErrorKind::PermissionDenied,
        );

        for fake in [unlistable, uninspectable] {
            let result = expr(&fake, "glob('/Users/me/code/app/*.tf')");
            assert!(
                matches!(&result, Err(Error::Lua { message, .. }) if message.contains("denied")),
                "{result:?}"
            );
        }
    }

    #[test]
    fn glob_refuses_a_relative_pattern() {
        let fake = fixture();

        let result = expr(&fake, "glob('rel/*.tf')");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. })
                if message.contains("rel/*.tf is not an absolute path")),
            "{result:?}"
        );
    }

    #[test]
    fn glob_resolves_dot_dot_before_matching() {
        let fake = fixture();

        let found = script(
            &fake,
            "return table.concat(glob('/Users/me/code/app/build/../*.tf'), ',') == \
             '/Users/me/code/app/main.tf,/Users/me/code/app/vars.tf'",
        );

        assert!(found.expect("runs"));
    }

    #[test]
    fn glob_skips_folders_that_lack_a_later_literal_component() {
        let fake = fixture();
        fake.add_dir("/Users/me/code/empty");

        let found = script(
            &fake,
            "return table.concat(glob('/Users/me/code/*/Cargo.toml'), ',') == \
             '/Users/me/code/app/Cargo.toml'",
        );

        assert!(found.expect("runs"));
    }

    #[test]
    fn glob_passes_over_files_where_a_folder_is_needed() {
        let fake = fixture();
        fake.add_file("/Users/me/code/notes.txt", "");

        let found = script(
            &fake,
            "return table.concat(glob('/Users/me/code/*/*.toml'), ',') == \
             '/Users/me/code/app/Cargo.toml'",
        );

        assert!(found.expect("runs"));
    }

    #[test]
    fn glob_neither_matches_nor_descends_symlinks() {
        let fake = fixture();
        fake.add_file("/Users/me/elsewhere/deep/x.tf", "");
        fake.add_symlink(
            "/Users/me/code/app/linked.tf",
            "/Users/me/elsewhere/deep/x.tf",
        );
        fake.add_symlink("/Users/me/code/link", "/Users/me/elsewhere");

        let found = script(
            &fake,
            "return table.concat(glob('/Users/me/code/*/*.tf'), ',') == \
             '/Users/me/code/app/main.tf,/Users/me/code/app/vars.tf'",
        );
        let through = expr(&fake, "glob('/Users/me/code/link/*/x.tf')");
        let literal = expr(&fake, "glob('/Users/me/code/link/deep/x.tf')");

        assert!(found.expect("runs"));
        assert!(matches!(through, Err(Error::Lua { .. })));
        assert!(matches!(literal, Err(Error::Lua { .. })), "{literal:?}");
    }

    #[test]
    fn reads_json_and_toml_into_tables() {
        let fake = fixture();

        let json = expr(
            &fake,
            "(function(p) return p.name == 'app' and math.type(p.version) == 'integer' \
             and p.version == 3 and p.ratio == 1.5 and p.private == false \
             and rawequal(p.license, nil) \
             and p.scripts.build == 'vite build' and p.tags[2] == 'b' end)\
             (read_json(parent(path) .. '/package.json'))",
        );
        let toml = expr(
            &fake,
            "(function(p) return p.edition == '2024' and math.type(p.version) == 'integer' \
             and p.version == 3 and p.ratio == 1.5 and p.publish == false \
             and p.released == '1979-05-27' and p.keywords[2] == 'b' end)\
             (read_toml(parent(path) .. '/Cargo.toml').package)",
        );

        assert!(json.expect("runs"));
        assert!(toml.expect("runs"));
    }

    #[test]
    fn refuses_to_read_through_a_symlink() {
        let fake = fixture();
        fake.add_file("/Users/me/secret.json", "{}");
        fake.add_symlink("/Users/me/code/app/linked.json", "/Users/me/secret.json");

        let read = expr(&fake, "read_json('/Users/me/code/app/linked.json')");
        let exists = expr(&fake, "exists('/Users/me/code/app/linked.json')");

        assert!(matches!(read, Err(Error::Lua { .. })));
        assert!(matches!(exists, Err(Error::Lua { .. })));
    }

    #[test]
    fn has_no_os_io_require_load_or_metatables() {
        let fake = fixture();
        let absent = [
            "os",
            "io",
            "require",
            "load",
            "loadfile",
            "dofile",
            "loadstring",
            "package",
            "debug",
            "collectgarbage",
            "setmetatable",
            "getmetatable",
            "print",
            "coroutine",
            "string.dump",
        ];

        for name in absent {
            assert!(
                expr(&fake, &format!("{name} == nil")).expect("runs"),
                "{name}"
            );
        }
        assert!(matches!(
            expr(&fake, "os.execute('touch /tmp/pwned')"),
            Err(Error::Lua { .. })
        ));
        assert!(matches!(
            expr(&fake, "string.dump(exists) and load"),
            Err(Error::Lua { .. }) | Ok(false)
        ));
    }

    #[test]
    fn string_methods_do_not_reach_dump() {
        let fake = fixture();

        let dump = expr(&fake, "type(('').dump) == 'nil'");
        let methods = expr(
            &fake,
            "('a'):upper() == 'A' and ('%d'):format(3) == '3' and ('x'):rep(2) == 'xx'",
        );

        assert!(dump.expect("runs"), "('').dump must be nil");
        assert!(methods.expect("runs"), "ordinary string methods must work");
    }

    #[test]
    fn keeps_no_state_between_checks() {
        let fake = fixture();
        let sandbox = LuaSandbox::new().expect("lua starts");
        let gate = gate(&fake);
        let check = |code: &str| {
            sandbox.accepts(
                &gate,
                &rule(),
                &LuaCode::Script(code.into()),
                Path::new(APP),
            )
        };

        check("leaked = true; string.upper = nil; return true").expect("runs");

        assert!(check("return leaked == nil and string.upper('a') == 'A'").expect("runs"));
    }

    #[test]
    fn recompiles_when_a_rules_code_changes_under_the_same_id() {
        let fake = fixture();
        let sandbox = LuaSandbox::new().expect("lua starts");
        let gate = gate(&fake);
        let check = |code: &str| {
            sandbox.accepts(&gate, &rule(), &LuaCode::Expr(code.into()), Path::new(APP))
        };

        assert!(!check("false").expect("runs"));
        assert!(
            check("true").expect("runs"),
            "the changed rule must not run its stale chunk"
        );
    }

    #[test]
    fn reports_a_lua_error_against_the_rule_and_candidate() {
        let fake = fixture();

        let result = expr(&fake, "error('boom')");

        let Err(Error::Lua {
            rule: failed,
            path,
            message,
        }) = result
        else {
            panic!("expected a Lua error, got {result:?}");
        };
        assert_eq!(failed, rule());
        assert_eq!(path, PathBuf::from(APP));
        assert!(message.contains("boom"), "{message}");
    }

    #[test]
    fn reports_a_lua_error_without_the_stack_traceback() {
        let fake = fixture();

        let result = expr(&fake, "error('boom')");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. })
                if message == "runtime error: test/build:1: boom"),
            "{result:?}"
        );
    }

    #[test]
    fn reports_a_failed_read_as_a_lua_error() {
        let fake = fixture();
        fake.fail_on(
            "/Users/me/code/app/package.json",
            Op::ReadFile,
            ErrorKind::PermissionDenied,
        );

        let result = expr(&fake, "read_json(parent(path) .. '/package.json')");

        assert!(
            matches!(&result, Err(Error::Lua { message, .. }) if message.contains("package.json")),
            "{result:?}"
        );
    }
}
