//! The in-memory home, rules, and scanner the scan tests run against.

// Each test crate that includes this module uses a different subset of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use rosie::config::Walk;
use rosie::fs::fake::{FakeBackend, USER_UID};
use rosie::fs::{Bounds, CommandOutput, Exit, Gate};
use rosie::plan::{AggressiveItems, Status};
use rosie::process::ProcessTable;
use rosie::rules::{Name, RuleDirs, RuleSet};
use rosie::scan::{Progress, Scan, Scanner, Settings};

pub const HOME: &str = "/Users/me";
pub const CONFIG: &str = "/Users/me/.config/rosie";
pub const DATA: &str = "/Users/me/.local/share/rosie";
pub const PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";

/// A fake home with a canned, empty process table.
pub fn fake_home() -> FakeBackend {
    let fake = FakeBackend::new();
    fake.add_dir(HOME);
    running(&fake, "");
    fake
}

pub const CODE: &str = "/Users/me/code";

pub const NODE: &str = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\nmarker = [\"package.json\"]\n";

/// A fake home whose `rosie` pack holds the `node-modules` marker rule.
pub fn node_home() -> FakeBackend {
    let fake = fake_home();
    add_rules(&fake, "node.toml", NODE);
    fake
}

/// A node project at `dir`: its manifest and a `node_modules` holding one file.
pub fn node_project(fake: &FakeBackend, dir: &str) {
    fake.add_file(format!("{dir}/package.json"), "{}");
    fake.add_file(format!("{dir}/node_modules/left-pad/index.js"), "x");
}

/// Installs a rule file in the `rosie` pack.
pub fn add_rules(fake: &FakeBackend, file: &str, text: &str) {
    fake.add_file(format!("{PACK}/{file}"), text);
}

/// Sets what `ps -axo pid=,ppid=,uid=,comm=` prints.
pub fn running(fake: &FakeBackend, ps: &str) {
    let output = CommandOutput {
        exit: Exit::Code(0),
        stdout: ps.as_bytes().to_vec(),
        stderr: Vec::new(),
    };
    fake.respond(ProcessTable::argv(), output);
}

/// How a test scans: the roots, the walk flags, `--aggressive`, and `--only`.
#[derive(Clone)]
pub struct Options {
    pub home: PathBuf,
    pub roots: Vec<PathBuf>,
    pub walk: Walk,
    pub aggressive: AggressiveItems,
    /// The `--only` rule names; empty scans with every rule, as without the flag.
    pub only: Vec<&'static str>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            home: PathBuf::from(HOME),
            roots: vec![PathBuf::from(HOME)],
            walk: Walk::default(),
            aggressive: AggressiveItems::Unticked,
            only: Vec::new(),
        }
    }
}

/// The fixture's bounds: `roots`, with the fixture's config and data dirs.
pub fn bounds_with_roots(roots: Vec<PathBuf>) -> Bounds {
    Bounds {
        roots,
        config_dir: PathBuf::from(CONFIG),
        data_dir: PathBuf::from(DATA),
        user_uid: USER_UID,
    }
}

/// A gate over `fake` bounded to `roots`, with the fixture's config and data dirs.
pub fn gate_with_roots(fake: &FakeBackend, roots: Vec<PathBuf>) -> Gate<&FakeBackend> {
    Gate::new(fake, bounds_with_roots(roots)).expect("absolute bounds")
}

/// A gate over `fake` bounded to home, with the fixture's config and data dirs.
pub fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
    gate_with_roots(fake, vec![PathBuf::from(HOME)])
}

pub fn scanner(fake: &FakeBackend, options: Options) -> Scanner<&FakeBackend> {
    let bounds = bounds_with_roots(options.roots);
    let gate = Gate::new(fake, bounds.clone()).expect("absolute bounds");
    let rules = RuleSet::load(&gate, &RuleDirs::new(&bounds)).expect("rules load");
    let rules = match options.only.is_empty() {
        true => rules,
        false => rules.only(&names(&options.only)).expect("known rule names"),
    };
    let settings = Settings {
        home: options.home,
        walk: options.walk,
        aggressive: options.aggressive,
    };
    Scanner::new(gate, rules, settings)
}

pub fn names(texts: &[&str]) -> Vec<Name> {
    texts
        .iter()
        .map(|text| Name::parse(text).expect("valid rule name"))
        .collect()
}

pub fn tree(fake: &FakeBackend, dir: &str) -> Scan {
    tree_with(fake, dir, Options::default())
}

pub fn tree_with(fake: &FakeBackend, dir: &str, options: Options) -> Scan {
    let progress = Recording::default();
    scanner(fake, options)
        .tree(Path::new(dir), &progress)
        .expect("tree scan runs")
}

pub fn caches(fake: &FakeBackend) -> Scan {
    caches_with(fake, Options::default())
}

pub fn caches_with(fake: &FakeBackend, options: Options) -> Scan {
    let progress = Recording::default();
    scanner(fake, options)
        .caches(&progress)
        .expect("caches scan runs")
}

/// Counts what a scan reports as it goes.
#[derive(Default)]
pub struct Recording {
    pub found: AtomicUsize,
    pub bytes: AtomicU64,
    /// How many times sizing reported bytes.
    pub sized_calls: AtomicUsize,
}

impl Progress for Recording {
    fn found(&self) {
        self.found.fetch_add(1, Ordering::Relaxed);
    }

    fn sized(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.sized_calls.fetch_add(1, Ordering::Relaxed);
    }
}

// reading a scan

/// The planned delete paths, in plan order.
pub fn paths(scan: &Scan) -> Vec<&str> {
    scan.plan
        .deletes()
        .iter()
        .map(|delete| delete.path.to_str().expect("utf-8"))
        .collect()
}

/// The planned deletes as (path, rules, status).
pub fn entries(scan: &Scan) -> Vec<(&str, Vec<&str>, Status)> {
    scan.plan
        .deletes()
        .iter()
        .map(|delete| {
            let path = delete.path.to_str().expect("utf-8");
            let rules = delete.rules.iter().map(String::as_str).collect();
            (path, rules, delete.status)
        })
        .collect()
}

/// The size of the planned delete at `path`, in bytes.
pub fn size_of(scan: &Scan, path: &str) -> u64 {
    let delete = scan
        .plan
        .deletes()
        .iter()
        .find(|delete| delete.path == Path::new(path));
    delete.expect("path is planned").size.get()
}

/// The skipped paths as (path, reason label).
pub fn skipped(scan: &Scan) -> Vec<(&str, rosie::plan::WalkSkip)> {
    scan.skipped
        .iter()
        .map(|skip| (skip.path.to_str().expect("utf-8"), skip.reason))
        .collect()
}

pub fn problems(scan: &Scan) -> Vec<String> {
    scan.problems.iter().map(ToString::to_string).collect()
}
