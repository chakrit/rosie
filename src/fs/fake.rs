//! In-memory `Backend` for tests.
//!
//! Public (not `cfg(test)`) so both in-crate tests and the CLI sandbox tests under
//! `tests/` can inject it. It simulates a case-insensitive, normalization-preserving
//! volume run as a non-root user: folders, files with any allocated size, symlinks,
//! hardlinks sharing an inode, other owners, other volumes, dataless placeholders,
//! permission denials, injected errors, canned command output, and a record of every
//! call.
//!
//! Fixture methods (`add_*`, `chown`, `chmod`, `mount`, …) live in `fixture`: they bypass
//! permissions, are not recorded, and panic on any fixture a real volume could not hold.
//! Backend methods walk paths as the kernel does: symlinks before the final
//! component are followed, `..` applies to the folder reached, and nothing can sit
//! below a non-folder.

mod fixture;
mod rename;
mod walk;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::backend::{
    Argv, Backend, CommandOutput, Exit, FileKind, Metadata, ROOT_UID, SF_DATALESS,
};
use super::error::Op;
use walk::{Creatable, FinalLink, Location, Tail};

/// The uid the fake runs as, and owns fixtures by default.
pub const USER_UID: u32 = 501;

const ROOT_DEV: u64 = 1;
const BLOCK_SIZE: u64 = 4096;
const DIR_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;
const SYMLINK_MODE: u32 = 0o755;

/// One backend call, recorded in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    ReadDir(PathBuf),
    Lstat(PathBuf),
    ReadLink(PathBuf),
    ReadFile(PathBuf),
    WriteFile(PathBuf),
    CreateDirAll(PathBuf),
    RemoveFile(PathBuf),
    RemoveEmptyDir(PathBuf),
    SetMode(PathBuf, u32),
    Rename(PathBuf, PathBuf),
    Run(Argv),
    /// `sudo <argv>`, with what was piped to its stdin.
    Sudo(Argv, Vec<u8>),
}

pub struct FakeBackend {
    state: Mutex<State>,
}

struct State {
    user_uid: u32,
    entries: BTreeMap<PathBuf, u64>,
    inodes: HashMap<u64, Node>,
    next_inode: u64,
    failures: HashMap<(PathBuf, Op), ErrorKind>,
    responses: Vec<(Argv, CommandOutput)>,
    sudo_response: Option<CommandOutput>,
    calls: Vec<Call>,
}

#[derive(Debug, Clone)]
struct Node {
    body: Body,
    allocated: u64,
    dev: u64,
    uid: u32,
    mode: u32,
    flags: u32,
}

#[derive(Debug, Clone)]
enum Body {
    File(Vec<u8>),
    Dir,
    Symlink(PathBuf),
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBackend {
    /// An empty volume holding only `/`, owned by root, run as [`USER_UID`].
    pub fn new() -> Self {
        Self::running_as(USER_UID)
    }

    /// An empty volume run as the given uid; [`ROOT_UID`] bypasses permissions.
    pub fn running_as(user_uid: u32) -> Self {
        let mut state = State {
            user_uid,
            entries: BTreeMap::new(),
            inodes: HashMap::new(),
            next_inode: 2,
            failures: HashMap::new(),
            responses: Vec::new(),
            sudo_response: None,
            calls: Vec::new(),
        };
        let root = Node::new(Body::Dir, ROOT_DEV, ROOT_UID, DIR_MODE);
        state.link_new(PathBuf::from("/"), root);
        FakeBackend {
            state: Mutex::new(state),
        }
    }

    // injected behavior

    /// Makes one operation on one path fail with the given error.
    pub fn fail_on(&self, path: impl AsRef<Path>, op: Op, kind: ErrorKind) {
        let key = (path.as_ref().to_path_buf(), op);
        self.lock().failures.insert(key, kind);
    }

    /// Sets the output a command returns; unknown commands fail as not found.
    pub fn respond(&self, argv: Argv, output: CommandOutput) {
        let mut state = self.lock();
        state.responses.retain(|(known, _)| *known != argv);
        state.responses.push((argv, output));
    }

    /// Sets what `sudo` returns, whatever it is asked to run; without it, sudo is not
    /// found.
    pub fn respond_to_sudo(&self, output: CommandOutput) {
        self.lock().sudo_response = Some(output);
    }

    // inspection

    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// Whether an entry exists at exactly this path, without recording a call.
    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.lock().entries.contains_key(path.as_ref())
    }

    // internals

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("fake backend state poisoned by a panicking test")
    }

    /// Records the call, then fails it if a failure was injected for this path and op.
    fn begin(&self, call: Call, path: &Path, op: Op) -> io::Result<MutexGuard<'_, State>> {
        let mut state = self.lock();
        state.calls.push(call);
        match state.failures.get(&(path.to_path_buf(), op)) {
            Some(kind) => Err(io::Error::from(*kind)),
            None => Ok(state),
        }
    }
}

impl Backend for FakeBackend {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let state = self.begin(Call::ReadDir(path.into()), path, Op::ReadDir)?;
        let key = state.follow(path)?;
        let node = state.node_at(&key);
        if !matches!(node.body, Body::Dir) {
            return Err(ErrorKind::NotADirectory.into());
        }
        if !state.can_list(node) {
            return Err(ErrorKind::PermissionDenied.into());
        }

        let names = state
            .children(&key)
            .filter_map(|(child, _)| child.file_name().map(OsString::from))
            .collect();
        Ok(names)
    }

    fn lstat(&self, path: &Path) -> io::Result<Metadata> {
        let state = self.begin(Call::Lstat(path.into()), path, Op::Lstat)?;
        let key = state.resolve(path)?;
        let inode = state.entries[&key];
        let node = state.node_at(&key);
        Ok(Metadata {
            kind: node.body.kind(),
            allocated: node.allocated,
            dev: node.dev,
            inode,
            uid: node.uid,
            mode: node.mode,
            flags: node.flags,
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let state = self.begin(Call::ReadLink(path.into()), path, Op::ReadLink)?;
        let key = state.resolve(path)?;
        match &state.node_at(&key).body {
            Body::Symlink(target) => Ok(target.clone()),
            _ => Err(ErrorKind::InvalidInput.into()),
        }
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        let state = self.begin(Call::ReadFile(path.into()), path, Op::ReadFile)?;
        let key = state.follow(path)?;
        match &state.node_at(&key).body {
            Body::File(contents) => Ok(contents.clone()),
            Body::Dir => Err(ErrorKind::IsADirectory.into()),
            Body::Symlink(_) => Err(io::Error::other("symlink chain did not end")),
        }
    }

    fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut state = self.begin(Call::WriteFile(path.into()), path, Op::WriteFile)?;
        // `open(O_CREAT)` follows a final symlink, dangling or not.
        let key = match state.locate(path, FinalLink::Follow)? {
            Location::Found(key) => return state.overwrite(&key, contents),
            Location::Absent(_, Creatable::FolderOnly) => return Err(ErrorKind::NotFound.into()),
            Location::Absent(key, Creatable::Anything) => key,
        };

        let mut node = Node::new(Body::File(contents.to_vec()), 0, state.user_uid, FILE_MODE);
        node.allocated = allocation_for(contents.len());
        state.create_entry(key, node)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let mut state = self.begin(Call::CreateDirAll(path.into()), path, Op::CreateDir)?;
        state.make_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let mut state = self.begin(Call::RemoveFile(path.into()), path, Op::RemoveFile)?;
        let key = state.resolve(path)?;
        if state.is_dir(&key) {
            return Err(ErrorKind::PermissionDenied.into());
        }
        state.require_writable_dir(parent_of(&key)?)?;
        state.entries.remove(&key);
        Ok(())
    }

    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        let mut state = self.begin(Call::RemoveEmptyDir(path.into()), path, Op::RemoveDir)?;
        let key = state.resolve(path)?;
        if !state.is_dir(&key) {
            return Err(ErrorKind::NotADirectory.into());
        }
        if state.children(&key).next().is_some() {
            return Err(ErrorKind::DirectoryNotEmpty.into());
        }
        state.require_writable_dir(parent_of(&key)?)?;
        state.entries.remove(&key);
        Ok(())
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        let mut state = self.begin(Call::SetMode(path.into(), mode), path, Op::SetMode)?;
        let key = state.follow(path)?;
        let user_uid = state.user_uid;
        let node = state.node_mut(&key);
        if user_uid != ROOT_UID && node.uid != user_uid {
            return Err(ErrorKind::PermissionDenied.into());
        }
        node.mode = mode & 0o7777;
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let call = Call::Rename(from.into(), to.into());
        let mut state = self.begin(call, from, Op::Rename)?;
        state.rename(from, to)
    }

    fn run(&self, argv: &Argv) -> io::Result<CommandOutput> {
        let mut state = self.lock();
        state.calls.push(Call::Run(argv.clone()));
        state
            .responses
            .iter()
            .find(|(known, _)| known == argv)
            .map(|(_, output)| output.clone())
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))
    }

    /// Hands over the canned stdout lines, then the stderr lines.
    fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, mut on_line: F) -> io::Result<Exit> {
        let output = self.run(argv)?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stdout.lines().chain(stderr.lines()) {
            on_line(line);
        }
        Ok(output.exit)
    }

    fn sudo(&self, argv: &Argv, stdin: &[u8]) -> io::Result<CommandOutput> {
        let mut state = self.lock();
        state.calls.push(Call::Sudo(argv.clone(), stdin.to_vec()));
        state
            .sudo_response
            .clone()
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))
    }
}

impl Node {
    fn new(body: Body, dev: u64, uid: u32, mode: u32) -> Self {
        Node {
            body,
            allocated: 0,
            dev,
            uid,
            mode,
            flags: 0,
        }
    }
}

impl Body {
    fn kind(&self) -> FileKind {
        match self {
            Body::File(_) => FileKind::File,
            Body::Dir => FileKind::Dir,
            Body::Symlink(_) => FileKind::Symlink,
        }
    }
}

impl State {
    fn children<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = (PathBuf, u64)> + 'a {
        self.subtree(dir)
            .filter(move |(path, _)| path.parent() == Some(dir))
    }

    /// The entry at `root` and every entry below it.
    fn subtree<'a>(&'a self, root: &'a Path) -> impl Iterator<Item = (PathBuf, u64)> + 'a {
        self.entries
            .range::<Path, _>((Bound::Included(root), Bound::Unbounded))
            .take_while(move |(path, _)| path.starts_with(root))
            .map(|(path, inode)| (path.clone(), *inode))
    }

    fn node_at(&self, key: &Path) -> &Node {
        &self.inodes[&self.entries[key]]
    }

    fn is_dir(&self, key: &Path) -> bool {
        matches!(self.node_at(key).body, Body::Dir)
    }

    fn node_mut(&mut self, key: &Path) -> &mut Node {
        let inode = self.entries[key];
        self.inodes
            .get_mut(&inode)
            .expect("every entry has an inode")
    }

    // permissions

    fn can_list(&self, node: &Node) -> bool {
        self.grants(node, 0o500)
    }

    /// Adding or removing names in a folder: write and search permission.
    fn require_writable_dir(&self, dir: &Path) -> io::Result<()> {
        self.require(dir, 0o300)
    }

    /// Write permission alone, without search: what a rename checks on a folder it
    /// moves or replaces.
    fn require_write_bit(&self, dir: &Path) -> io::Result<()> {
        self.require(dir, 0o200)
    }

    fn require(&self, key: &Path, owner_bits: u32) -> io::Result<()> {
        match self.grants(self.node_at(key), owner_bits) {
            true => Ok(()),
            false => Err(ErrorKind::PermissionDenied.into()),
        }
    }

    /// Whether the user holds every permission in `owner_bits`, written in the owner
    /// position, through the owner class or the other class.
    fn grants(&self, node: &Node, owner_bits: u32) -> bool {
        let other_bits = owner_bits >> 6;
        match (self.user_uid, node.uid) {
            (ROOT_UID, _) => true,
            (user, owner) if user == owner => node.mode & owner_bits == owner_bits,
            _ => node.mode & other_bits == other_bits,
        }
    }

    // mutation

    /// Creates an entry where a path walk found none, as `open(O_CREAT)` and `mkdir` do:
    /// the parent folder must be writable, and the entry joins the parent's volume.
    fn create_entry(&mut self, key: PathBuf, mut node: Node) -> io::Result<()> {
        let parent = parent_of(&key)?;
        self.require_writable_dir(parent)?;

        node.dev = self.node_at(parent).dev;
        self.link_new(key, node);
        Ok(())
    }

    /// `mkdir -p` as std's `create_dir_all` does it: `mkdir`, creating missing parents
    /// first, and accepting a path that already leads to a folder.
    fn make_dir_all(&mut self, path: &Path) -> io::Result<()> {
        match self.make_dir(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) if self.leads_to_dir(path) => return Ok(()),
            Err(error) => return Err(error),
        }

        self.make_dir_all(parent_of(path)?)?;
        match self.make_dir(path) {
            Ok(()) => Ok(()),
            Err(_) if self.leads_to_dir(path) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// `mkdir(2)`: never follows a final symlink, so an existing link is `EEXIST`.
    fn make_dir(&mut self, path: &Path) -> io::Result<()> {
        let key = match self.locate(path, FinalLink::Keep)? {
            Location::Found(_) => return Err(ErrorKind::AlreadyExists.into()),
            Location::Absent(key, _) => key,
        };
        let node = Node::new(Body::Dir, 0, self.user_uid, DIR_MODE);
        self.create_entry(key, node)
    }

    /// std's `Path::is_dir`: follows every link, and any failure means no.
    fn leads_to_dir(&self, path: &Path) -> bool {
        let followed = self.follow(path);
        matches!(
            followed.map(|key| self.node_at(&key).body.kind()),
            Ok(FileKind::Dir)
        )
    }

    /// Stores a new inode at `path`. Every entry sits in a folder: the path walk and the
    /// fixtures only ever produce such paths, and this holds them to it.
    fn link_new(&mut self, path: PathBuf, node: Node) {
        if let Some(parent) = path.parent() {
            assert!(
                self.is_dir(parent),
                "fake entry {} would sit below a non-folder",
                path.display()
            );
        }

        let inode = self.next_inode;
        self.next_inode += 1;
        self.inodes.insert(inode, node);
        self.entries.insert(path, inode);
    }

    fn overwrite(&mut self, key: &Path, contents: &[u8]) -> io::Result<()> {
        let user_uid = self.user_uid;
        let node = self.node_mut(key);
        if matches!(node.body, Body::Dir) {
            return Err(ErrorKind::IsADirectory.into());
        }
        let writable = user_uid == ROOT_UID || (node.uid == user_uid && node.mode & 0o200 != 0);
        if !writable {
            return Err(ErrorKind::PermissionDenied.into());
        }
        node.body = Body::File(contents.to_vec());
        node.allocated = allocation_for(contents.len());
        Ok(())
    }
}

fn allocation_for(len: usize) -> u64 {
    (len as u64).div_ceil(BLOCK_SIZE) * BLOCK_SIZE
}

fn parent_of(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_case_insensitively_like_a_default_volume() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/Code/app.txt", "x");

        let meta = fake
            .lstat(Path::new("/users/ME/code/APP.txt"))
            .expect("folded lookup");
        let listing = fake
            .read_dir(Path::new("/users/me"))
            .expect("list folded folder");

        assert_eq!(meta.kind, FileKind::File);
        assert_eq!(listing, vec![OsString::from("Code")]);
    }

    #[test]
    fn hardlinks_share_an_inode_and_survive_each_other() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "data");
        fake.add_hardlink("/a/one", "/b/two");

        let one = fake.lstat(Path::new("/a/one")).expect("lstat one");
        let two = fake.lstat(Path::new("/b/two")).expect("lstat two");
        fake.remove_file(Path::new("/a/one"))
            .expect("remove one name");

        assert_eq!(one.inode, two.inode);
        assert_eq!(
            fake.read_file(Path::new("/b/two"))
                .expect("read other name"),
            b"data"
        );
    }

    #[test]
    fn mounted_folders_report_their_volume_and_pass_it_on() {
        let fake = FakeBackend::new();
        fake.add_dir("/Volumes/ext");
        fake.mount("/Volumes/ext", 7);
        fake.add_file("/Volumes/ext/later.txt", "x");

        let root_dev = fake.lstat(Path::new("/Volumes")).expect("lstat").dev;
        let later_dev = fake
            .lstat(Path::new("/Volumes/ext/later.txt"))
            .expect("lstat")
            .dev;

        assert_ne!(root_dev, 7);
        assert_eq!(later_dev, 7);
    }

    #[test]
    fn root_owned_items_refuse_the_user() {
        let fake = FakeBackend::new();
        fake.add_file("/Library/Thing/file", "x");
        fake.chown("/Library/Thing", ROOT_UID);

        let removed = fake.remove_file(Path::new("/Library/Thing/file"));
        let chmodded = fake.set_mode(Path::new("/Library/Thing"), 0o777);

        assert_eq!(
            removed.map_err(|e| e.kind()),
            Err(ErrorKind::PermissionDenied)
        );
        assert_eq!(
            chmodded.map_err(|e| e.kind()),
            Err(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn injected_failures_hit_only_their_path_and_op() {
        let fake = FakeBackend::new();
        fake.add_dir("/p/locked");
        fake.fail_on("/p/locked", Op::ReadDir, ErrorKind::PermissionDenied);

        let listed = fake.read_dir(Path::new("/p/locked"));
        let inspected = fake.lstat(Path::new("/p/locked"));

        assert_eq!(
            listed.map_err(|e| e.kind()),
            Err(ErrorKind::PermissionDenied)
        );
        assert!(inspected.is_ok());
    }

    #[test]
    fn commands_return_canned_output_and_are_recorded() {
        let fake = FakeBackend::new();
        let ps = Argv::new("ps").arg("-axo").arg("pid=,ppid=,uid=,comm=");
        let output = CommandOutput {
            exit: Exit::Code(0),
            stdout: b"  42     1   501 /Applications/Foo.app/Contents/MacOS/Foo\n".to_vec(),
            stderr: Vec::new(),
        };
        fake.respond(ps.clone(), output.clone());

        let ran = fake.run(&ps).expect("canned ps");
        let unknown = fake.run(&Argv::new("gradle"));

        assert_eq!(ran, output);
        assert_eq!(unknown.map_err(|e| e.kind()), Err(ErrorKind::NotFound));
        assert_eq!(
            fake.calls(),
            vec![Call::Run(ps), Call::Run(Argv::new("gradle"))]
        );
    }

    #[test]
    fn huge_and_dataless_files_report_their_metadata() {
        let fake = FakeBackend::new();
        fake.add_sized_file("/big.img", 5 << 40);
        fake.add_sized_file("/cloud.pdf", 0);
        fake.make_dataless("/cloud.pdf");

        let big = fake.lstat(Path::new("/big.img")).expect("lstat big");
        let cloud = fake.lstat(Path::new("/cloud.pdf")).expect("lstat cloud");

        assert_eq!(big.allocated, 5 << 40);
        assert!(!big.is_dataless());
        assert!(cloud.is_dataless());
    }
}
