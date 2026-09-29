//! Smoke tier: the real `rosie` binary, run on temp folders with `env_clear()` and a
//! sandboxed `HOME` and `PATH`, never the network or the real home
//! (`docs/spec/testing.md#fakes-and-sandboxing`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// A local port nothing listens on (the discard port), for every proxy variable HTTP
/// clients read.
const DEAD_PROXY: &str = "http://127.0.0.1:9";
const DEAD_PROXIES: [&str; 3] = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"];

const NODE_RULE: &str = "[rules.node-modules]\n\
                          strategy = \"marker\"\n\
                          target = \"node_modules\"\n\
                          marker = [\"package.json\"]\n";

/// A sandboxed `$HOME` with the `rosie` pack already pulled, so a smoke test never
/// reaches the network. Its own temp folder is dropped, and so removed, with it.
pub struct SmokeHome {
    _home: TempDir,
    home: PathBuf,
}

impl SmokeHome {
    /// A home whose `roots` allowlist holds exactly these folders under it, real paths
    /// with no symlink component the way a seeded `config.toml` always holds them
    /// (`docs/spec/safety.md#roots`). Each folder is created.
    pub fn with_roots(roots: &[&str]) -> Self {
        let home = tempfile::tempdir().expect("create a sandboxed home");
        // macOS temp folders sit under the `/var` -> `/private/var` link.
        let home_path = home.path().canonicalize().expect("canonicalize the home");

        let pack_dir = home_path.join(".local/share/rosie/packs/chakrit/rosie");
        std::fs::create_dir_all(&pack_dir).expect("create the pack folder");
        std::fs::write(pack_dir.join("node.toml"), NODE_RULE).expect("write the rule pack");
        std::fs::write(pack_dir.join(".pulled"), pulled_now()).expect("write the pulled marker");

        let config_dir = home_path.join(".config/rosie");
        std::fs::create_dir_all(&config_dir).expect("create the config folder");
        let quoted: Vec<String> = roots
            .iter()
            .map(|root| {
                let path = home_path.join(root);
                std::fs::create_dir_all(&path).expect("create a root folder");
                format!("{:?}", path.display().to_string())
            })
            .collect();
        std::fs::write(
            config_dir.join("config.toml"),
            format!("roots = [{}]\n", quoted.join(", ")),
        )
        .expect("write config.toml");

        SmokeHome {
            _home: home,
            home: home_path,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// A path under the sandboxed home, real and symlink-free like every path under it.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.home.join(relative)
    }

    /// Runs the real `rosie` binary with a clean environment: only `HOME`, `PATH`, and
    /// the proxy variables are set, and stdin is the given text (or closed). The proxies
    /// point at a local port nothing listens on, so a download the seeded pack should
    /// have made unnecessary fails loudly instead of reaching the network.
    pub fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rosie"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .envs(DEAD_PROXIES.map(|name| (name, DEAD_PROXY)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run the rosie binary");

        let mut input = child.stdin.take().expect("stdin");
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).expect("write stdin");
        }
        drop(input);

        child.wait_with_output().expect("wait for rosie")
    }
}

/// The pull time a fresh pack is seeded with: now, so age checks never trigger.
fn pulled_now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs()
        .to_string()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 stdout")
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf-8 stderr")
}

/// A `node_modules` project inside `dir`, matched by the seeded pack's rule.
pub fn node_project(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create project folder");
    std::fs::write(dir.join("package.json"), "{}").expect("write package.json");
    let node_modules = dir.join("node_modules");
    std::fs::create_dir_all(&node_modules).expect("create node_modules");
    std::fs::write(node_modules.join("left-pad.js"), "x").expect("write a package file");
}
