//! The in-memory home, gate, and canned network the pack tests run against.

// Each test crate that includes this module uses a different subset of it.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rosie::fs::fake::{FakeBackend, USER_UID};
use rosie::fs::{Backend, Bounds, Gate};
use rosie::packs::{Network, Source};

pub const HOME: &str = "/Users/me";
pub const CONFIG: &str = "/Users/me/.config/rosie";
pub const DATA: &str = "/Users/me/.local/share/rosie";
pub const OWNER: &str = "/Users/me/.local/share/rosie/packs/chakrit";
pub const ROSIE_PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";
pub const ROSIE_STAGING: &str = "/Users/me/.local/share/rosie/packs/chakrit/.rosie.new";
pub const ROSIE_SET_ASIDE: &str = "/Users/me/.local/share/rosie/packs/chakrit/.rosie.old";
pub const ROSIE_REMOVED: &str = "/Users/me/.local/share/rosie/packs/chakrit/.rosie.removed";
pub const ROSIE_URL: &str = "https://github.com/chakrit/rosie/archive/HEAD.tar.gz";

pub const NODE_RULE: &str = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\nmarker = [\"package.json\"]\n";

/// Serves canned bodies by URL and records every request; unknown URLs fail as offline.
#[derive(Default)]
pub struct Canned {
    bodies: HashMap<String, Vec<u8>>,
    requested: RefCell<Vec<String>>,
}

impl Canned {
    pub fn offline() -> Self {
        Canned::default()
    }

    pub fn serving(url: &str, body: Vec<u8>) -> Self {
        Canned::offline().and(url, body)
    }

    pub fn and(mut self, url: &str, body: Vec<u8>) -> Self {
        self.bodies.insert(url.to_owned(), body);
        self
    }

    pub fn requested(&self) -> Vec<String> {
        self.requested.borrow().clone()
    }
}

impl Network for Canned {
    fn get(&self, url: &str) -> io::Result<Vec<u8>> {
        self.requested.borrow_mut().push(url.to_owned());
        self.bodies.get(url).cloned().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "the network is unreachable")
        })
    }
}

pub fn fake_home() -> FakeBackend {
    let fake = FakeBackend::new();
    fake.add_dir(HOME);
    fake
}

pub fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
    let bounds = Bounds {
        roots: vec![],
        config_dir: PathBuf::from(CONFIG),
        data_dir: PathBuf::from(DATA),
        user_uid: USER_UID,
    };
    Gate::new(fake, bounds).expect("absolute bounds")
}

pub fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

pub fn source(text: &str) -> Source {
    text.parse().expect("valid source")
}

pub fn url_of(text: &str) -> String {
    format!("https://github.com/{text}/archive/HEAD.tar.gz")
}

pub fn contents(fake: &FakeBackend, path: impl AsRef<Path>) -> String {
    let bytes = fake.read_file(path.as_ref()).expect("file exists");
    String::from_utf8(bytes).expect("utf-8 file")
}

pub fn listing(fake: &FakeBackend, path: &str) -> Vec<OsString> {
    let mut names = fake.read_dir(Path::new(path)).expect("folder exists");
    names.sort();
    names
}
