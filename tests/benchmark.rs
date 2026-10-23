//! Benchmark tier: the speed budgets on a real temp-folder tree, run by hand on gz44
//! (`docs/spec/testing.md#benchmark`):
//!
//! ```sh
//! nice -n 19 cargo test --release --test benchmark -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The 1M-entry tree is built once under `tmp/benchmark/` and reused by later runs. Each
//! case runs the release `rosie` binary once to warm the cache, then reports the best
//! of three timed runs.

mod smoke_fixture;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use smoke_fixture::{SmokeHome, stderr, stdout};

const WALK_BUDGET: Duration = Duration::from_secs(1);
const SIZING_BUDGET: Duration = Duration::from_millis(1500);

/// Folders per level, levels of folders, and files per leaf folder: 10⁴ leaf folders of
/// 99 files, plus 11,110 folders, is 1,001,110 entries under `node_modules`.
const FAN_OUT: usize = 10;
const DEPTH: u32 = 4;
const LEAF_FILES: usize = 99;

const TIMED_RUNS: usize = 3;

#[test]
#[ignore = "benchmark: run by hand on gz44"]
fn walk_of_1m_entries() {
    let home = fixture();
    let node_modules = home.path("project/node_modules");

    // Started inside `node_modules`, the walk claims nothing: every entry is walked.
    let best = best_of(&home, &["scan", "tree", &display(&node_modules)]);

    println!("walk, 1M entries: {best:?}");
    assert!(
        best < WALK_BUDGET,
        "walk took {best:?}, budget {WALK_BUDGET:?}"
    );
}

#[test]
#[ignore = "benchmark: run by hand on gz44"]
fn sizing_of_1m_entries() {
    let home = fixture();
    let project = home.path("project");

    // The walk claims `node_modules` at once; the time is the sizing of its 1M entries.
    let best = best_of(&home, &["scan", "tree", &display(&project)]);

    println!("sizing, 1M entries: {best:?}");
    assert!(
        best < SIZING_BUDGET,
        "sizing took {best:?}, budget {SIZING_BUDGET:?}"
    );
}

#[test]
#[ignore = "benchmark: run by hand on gz44"]
fn delete_of_100k_entries() {
    let home = fixture();
    let source = home.path("project/node_modules/d0");
    let app = home.path("deletes/app");

    let mut times = Vec::with_capacity(TIMED_RUNS);
    for _ in 0..TIMED_RUNS {
        clone_project(&source, &app);
        let plan = stdout(&succeeded(
            home.run(&["scan", "tree", &display(&home.path("deletes"))], None),
        ));

        let started = Instant::now();
        succeeded(home.run(&["run", "-"], Some(&plan)));
        times.push(started.elapsed());

        assert!(!app.join("node_modules").exists(), "the planned folder");
    }

    let best = times.into_iter().min().expect("timed runs");
    println!("delete, 100k entries: {best:?}");
}

/// The kept benchmark home, with its 1M-entry `node_modules` built on first use.
fn fixture() -> SmokeHome {
    let home = SmokeHome::kept_at(&kept_home(), &["project", "deletes"]);

    let project = home.path("project");
    let built = project.join(".built");
    if !built.exists() {
        build_tree(&project);
        std::fs::write(&built, "").expect("write the built marker");
    }
    home
}

/// A `node_modules` project of `FAN_OUT^DEPTH` leaf folders of `LEAF_FILES` empty files.
fn build_tree(project: &Path) {
    std::fs::write(project.join("package.json"), "{}").expect("write package.json");
    let node_modules = project.join("node_modules");

    let leaves = FAN_OUT.pow(DEPTH);
    (0..leaves).into_par_iter().for_each(|leaf| {
        let folder = leaf_folder(&node_modules, leaf);
        std::fs::create_dir_all(&folder).expect("create a leaf folder");
        for file in 0..LEAF_FILES {
            std::fs::write(folder.join(format!("f{file:02}")), "").expect("write a file");
        }
    });
}

/// The leaf folder numbered `leaf`, one `d<digit>` level per base-`FAN_OUT` digit.
fn leaf_folder(node_modules: &Path, leaf: usize) -> PathBuf {
    (0..DEPTH)
        .rev()
        .fold(node_modules.to_path_buf(), |folder, level| {
            let digit = leaf / FAN_OUT.pow(level) % FAN_OUT;
            folder.join(format!("d{digit}"))
        })
}

/// A fresh `node_modules` project at `app` whose modules are an APFS clone of `source`.
fn clone_project(source: &Path, app: &Path) {
    std::fs::create_dir_all(app).expect("create the app folder");
    std::fs::write(app.join("package.json"), "{}").expect("write package.json");

    let cloned = std::process::Command::new("/bin/cp")
        .args(["-c", "-R"])
        .arg(source)
        .arg(app.join("node_modules"))
        .status()
        .expect("run cp");
    assert!(cloned.success(), "cp -c -R {}", source.display());
}

/// The best wall time of `TIMED_RUNS` runs of `args`, after one warming run.
fn best_of(home: &SmokeHome, args: &[&str]) -> Duration {
    succeeded(home.run(args, None));

    (0..TIMED_RUNS)
        .map(|_| {
            let started = Instant::now();
            succeeded(home.run(args, None));
            started.elapsed()
        })
        .min()
        .expect("timed runs")
}

fn succeeded(output: Output) -> Output {
    assert!(output.status.success(), "{}", stderr(&output));
    output
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// `tmp/benchmark/home` in the repository, created if absent, as a real path.
fn kept_home() -> PathBuf {
    let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/benchmark/home");
    std::fs::create_dir_all(&home).expect("create the benchmark home");
    home.canonicalize()
        .expect("canonicalize the benchmark home")
}
