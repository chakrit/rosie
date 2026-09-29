//! The sandbox tier: rosie's CLI run in-process through `App` over the in-memory fake,
//! a canned network, and a scripted console (`docs/spec/testing.md#tiers`).

// Each test crate that includes this module uses a different subset of it.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt::Display;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rosie::cli::{App, Console, Env, ExitStatus, PromptError, Terminals};
use rosie::fs::fake::{Call, FakeBackend};
use rosie::fs::{Argv, CommandOutput, Exit, Home};
use rosie::process::ProcessTable;
use rosie::run::PlainReporter;
use rosie::run::elevated::Stdin;
use rosie::scan::NoProgress;

use crate::pack_fixture::Canned;

pub const HOME: &str = "/Users/me";
pub const CWD: &str = "/Users/me/src";
pub const CONFIG_FILE: &str = "/Users/me/.config/rosie/config.toml";
pub const PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";
pub const ROSIE_URL: &str = "https://github.com/chakrit/rosie/archive/HEAD.tar.gz";
pub const EXE: &str = "/usr/local/bin/rosie";
pub const PID: u32 = 4242;

/// The time every run happens at, and the pack's pull time by default.
pub const NOW: u64 = 1_800_000_000;

pub const NODE_RULE: &str = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\nmarker = [\"package.json\"]\n";
pub const CARGO_RULE: &str =
    "[rules.cargo-target]\nstrategy = \"marker\"\ntarget = \"target\"\nmarker = [\"Cargo.toml\"]\n";

pub const TERMINAL: Terminals = Terminals {
    stdin: true,
    stdout: true,
    stderr: true,
};
pub const PIPED: Terminals = Terminals {
    stdin: false,
    stdout: false,
    stderr: false,
};

/// A fake machine: a home with rosie's `node-modules` rule pulled, a config whose only
/// root is the home, an empty process table, and rosie running as the user.
pub struct Sandbox {
    pub fake: FakeBackend,
    pub net: Canned,
    pub now: SystemTime,
}

impl Sandbox {
    pub fn new() -> Self {
        Sandbox::with_roots(&[HOME])
    }

    /// Like [`Sandbox::new`], with these roots instead of the home.
    pub fn with_roots(roots: &[&str]) -> Self {
        let sandbox = Sandbox::bare();
        sandbox.install_pack(&[("node.toml", NODE_RULE)], NOW);
        let quoted: Vec<String> = roots.iter().map(|root| format!("\"{root}\"")).collect();
        sandbox.write_config(&format!("roots = [{}]\n", quoted.join(", ")));
        sandbox
    }

    /// A home with no pack and no config, as on a first run.
    pub fn bare() -> Self {
        let fake = FakeBackend::new();
        fake.add_dir(CWD);
        let sandbox = Sandbox {
            fake,
            net: Canned::offline(),
            now: at(NOW),
        };
        sandbox.running_as("501");
        sandbox.processes("");
        sandbox
    }

    pub fn with_net(mut self, net: Canned) -> Self {
        self.net = net;
        self
    }

    pub fn at_time(mut self, now: SystemTime) -> Self {
        self.now = now;
        self
    }

    // fixtures

    /// Writes rule files into the `rosie` pack, pulled at `pulled` Unix seconds.
    pub fn install_pack(&self, rules: &[(&str, &str)], pulled: u64) {
        for (name, text) in rules {
            self.fake.add_file(format!("{PACK}/{name}"), *text);
        }
        self.fake
            .add_file(format!("{PACK}/.pulled"), pulled.to_string());
    }

    /// Adds a rule file to the installed `rosie` pack.
    pub fn add_rules(&self, name: &str, text: &str) {
        self.fake.add_file(format!("{PACK}/{name}"), text);
    }

    pub fn write_config(&self, text: &str) {
        self.fake.add_file(CONFIG_FILE, text);
    }

    /// A node project at `dir`: its manifest and a `node_modules` holding one file.
    pub fn node_project(&self, dir: &str) {
        self.fake.add_file(format!("{dir}/package.json"), "{}");
        self.fake
            .add_file(format!("{dir}/node_modules/left-pad/index.js"), "x");
    }

    /// What `id -u` prints.
    pub fn running_as(&self, uid: &str) {
        self.respond(Argv::new("/usr/bin/id").arg("-u"), &format!("{uid}\n"));
    }

    /// What `ps -axo pid=,ppid=,uid=,comm=` prints.
    pub fn processes(&self, ps: &str) {
        self.respond(ProcessTable::argv(), ps);
    }

    pub fn respond(&self, argv: impl Into<Argv>, stdout: &str) {
        let output = CommandOutput {
            exit: Exit::Code(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        self.fake.respond(argv, output);
    }

    // running

    /// Runs `rosie <args>` with every stream a terminal and no scripted answers.
    pub fn run(&self, args: &[&str]) -> Ran {
        self.run_with(Script::terminal(), args)
    }

    pub fn run_with(&self, script: Script, args: &[&str]) -> Ran {
        let env = Env {
            home: Home::new(Path::new(HOME)).expect("absolute home"),
            cwd: PathBuf::from(CWD),
            exe: PathBuf::from(EXE),
            pid: PID,
            now: self.now,
        };
        let console = ScriptedConsole::new(script);
        let out = console.out.clone();
        let err = console.err.clone();
        let asked = Arc::clone(&console.asked);

        let argv = std::iter::once("rosie")
            .chain(args.iter().copied())
            .map(OsString::from);
        let status = App::new(&self.fake, &self.net, console, env).run(argv);

        Ran {
            status,
            stdout: out.text(),
            stderr: err.text(),
            asked: asked.lock().expect("prompt log").clone(),
        }
    }

    // inspecting

    pub fn exists(&self, path: &str) -> bool {
        self.fake.exists(path)
    }

    /// Every removal the fake was asked for.
    pub fn removals(&self) -> Vec<PathBuf> {
        self.fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::RemoveFile(path) | Call::RemoveEmptyDir(path) => Some(path),
                _ => None,
            })
            .collect()
    }

    pub fn file(&self, path: &str) -> String {
        use rosie::fs::Backend;
        let bytes = self.fake.read_file(Path::new(path)).expect("file exists");
        String::from_utf8(bytes).expect("utf-8 file")
    }
}

pub fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// One run's exit status, output, and the prompts it showed.
#[derive(Debug)]
pub struct Ran {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub asked: Vec<String>,
}

impl Ran {
    pub fn assert_status(&self, expected: ExitStatus) -> &Self {
        assert_eq!(self.status, expected, "{self:#?}");
        self
    }
}

/// What the scripted console answers, in order, and what its streams are.
#[derive(Debug, Clone)]
pub struct Script {
    pub terminals: Terminals,
    pub stdin: Vec<u8>,
    pub stdin_is_pipe: bool,
    pub answers: VecDeque<String>,
    pub picks: VecDeque<usize>,
    pub checklists: VecDeque<Vec<usize>>,
}

impl Script {
    pub fn terminal() -> Self {
        Script {
            terminals: TERMINAL,
            stdin: Vec::new(),
            stdin_is_pipe: false,
            answers: VecDeque::new(),
            picks: VecDeque::new(),
            checklists: VecDeque::new(),
        }
    }

    pub fn piped() -> Self {
        Script {
            terminals: PIPED,
            ..Script::terminal()
        }
    }

    pub fn answer(mut self, text: &str) -> Self {
        self.answers.push_back(text.to_owned());
        self
    }

    pub fn pick(mut self, index: usize) -> Self {
        self.picks.push_back(index);
        self
    }

    pub fn keep(mut self, kept: &[usize]) -> Self {
        self.checklists.push_back(kept.to_vec());
        self
    }

    pub fn stdin(mut self, bytes: &str) -> Self {
        self.stdin = bytes.as_bytes().to_vec();
        self.stdin_is_pipe = true;
        self
    }
}

/// A stream the test reads back after the run.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    pub fn text(&self) -> String {
        let bytes = self.0.lock().expect("stream buffer").clone();
        String::from_utf8(bytes).expect("utf-8 output")
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("stream buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ScriptedConsole {
    script: Script,
    out: Shared,
    err: Shared,
    /// Every prompt, picker, and checklist shown, in order.
    asked: Arc<Mutex<Vec<String>>>,
}

impl ScriptedConsole {
    fn new(script: Script) -> Self {
        ScriptedConsole {
            script,
            out: Shared::default(),
            err: Shared::default(),
            asked: Arc::default(),
        }
    }

    fn record(&self, prompt: &str) {
        self.asked
            .lock()
            .expect("prompt log")
            .push(prompt.to_owned());
    }
}

impl Console for ScriptedConsole {
    type Out = Shared;
    type Err = Shared;
    type Progress = NoProgress;
    type Reporter = PlainReporter<Shared>;

    fn terminals(&self) -> Terminals {
        self.script.terminals
    }

    fn stdout(&mut self) -> &mut Shared {
        &mut self.out
    }

    fn stderr(&mut self) -> &mut Shared {
        &mut self.err
    }

    fn read_stdin(&mut self) -> io::Result<Vec<u8>> {
        Ok(std::mem::take(&mut self.script.stdin))
    }

    fn capture_stdin(&mut self) -> io::Result<Stdin> {
        match self.script.stdin_is_pipe {
            true => Ok(Stdin::Pipe(std::mem::take(&mut self.script.stdin))),
            false => Ok(Stdin::NotPipe),
        }
    }

    fn ask(&mut self, prompt: &str) -> Result<String, PromptError> {
        self.record(prompt);
        self.script
            .answers
            .pop_front()
            .ok_or(PromptError::Cancelled)
    }

    fn pick(&mut self, prompt: &str, options: &[String]) -> Result<usize, PromptError> {
        self.record(&format!("{prompt} {}", options.join(" | ")));
        self.script.picks.pop_front().ok_or(PromptError::Cancelled)
    }

    /// Leaves ticked the items at the scripted positions.
    fn checklist<T: Display>(
        &mut self,
        prompt: &str,
        items: Vec<T>,
    ) -> Result<Vec<T>, PromptError> {
        let labels: Vec<String> = items.iter().map(ToString::to_string).collect();
        self.record(&format!("{prompt} {}", labels.join(" | ")));
        let positions = self
            .script
            .checklists
            .pop_front()
            .ok_or(PromptError::Cancelled)?;

        let kept = items
            .into_iter()
            .enumerate()
            .filter(|(position, _)| positions.contains(position))
            .map(|(_, item)| item)
            .collect();
        Ok(kept)
    }

    fn progress(&self) -> NoProgress {
        NoProgress
    }

    fn reporter(&self) -> PlainReporter<Shared> {
        PlainReporter::new(self.err.clone())
    }
}
