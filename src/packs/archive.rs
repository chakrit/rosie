//! A downloaded pack tarball, checked and read into memory before anything is written
//! (`docs/spec/safety.md#rosies-own-data`).
//!
//! Only regular files and folders are accepted; absolute paths, `..`, symlinks,
//! hardlinks, devices, and every other entry type refuse the whole archive. The pax
//! global header `git archive` writes first carries no file and is passed over.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::{Entry, EntryType};

use super::error::{Error, Refusal};

const RULES_FOLDER: &str = "rules";
pub(super) const RULE_EXTENSION: &str = "toml";
const CONFIG_FILE: &str = "config.toml";

/// The unpacked archive: the files of its `rules/` folder and its root `config.toml`.
#[derive(Debug)]
pub struct Archive {
    rules: Vec<RuleFile>,
    config: Option<Vec<u8>>,
}

/// One `*.toml` file of a pack, named without its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFile {
    name: OsString,
    contents: Vec<u8>,
}

impl Archive {
    /// The largest rule or config file accepted, far above any real one.
    pub const FILE_LIMIT: u64 = 1024 * 1024;

    /// The most bytes an archive may unpack to, every entry counted, kept or not. It
    /// bounds a gzip bomb spread over many files below [`Archive::FILE_LIMIT`].
    pub const UNPACKED_LIMIT: u64 = 32 * 1024 * 1024;

    /// Reads a gzipped tarball whose entries all sit in one top folder, as GitHub's
    /// repository archives do.
    pub fn parse(gzipped: &[u8]) -> Result<Archive, Error> {
        Archive::parse_within(gzipped, Archive::UNPACKED_LIMIT)
    }

    /// `parse`, bounded by an injected limit so the boundary can be exercised on
    /// KiB-sized fixtures instead of inflating a real-sized archive in every test run.
    fn parse_within(gzipped: &[u8], limit: u64) -> Result<Archive, Error> {
        // Reading one byte past the limit marks an archive over it. The check runs even
        // when the read succeeded: tar reads a stream cut at an entry boundary as a
        // shorter, valid archive.
        let unpacked = GzDecoder::new(gzipped).take(limit + 1);
        let mut tar = tar::Archive::new(unpacked);

        let read = read_entries(&mut tar);

        let over_limit = tar.into_inner().limit() == 0;
        match over_limit {
            true => Err(Error::UnpackedTooLarge { limit }),
            false => read,
        }
    }

    pub fn rules(&self) -> &[RuleFile] {
        &self.rules
    }

    /// The root `config.toml`, used to seed a fresh user config.
    pub fn config(&self) -> Result<&str, Error> {
        let bytes = self.config.as_deref().ok_or(Error::NoConfig)?;
        std::str::from_utf8(bytes).map_err(|_| Error::ConfigNotText)
    }
}

impl RuleFile {
    pub fn new(name: OsString, contents: Vec<u8>) -> Self {
        RuleFile { name, contents }
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    /// Where the file sat in the archive, below its top folder.
    pub fn archive_path(&self) -> PathBuf {
        Path::new(RULES_FOLDER).join(&self.name)
    }
}

/// The archive read so far.
#[derive(Default)]
struct Unpacking {
    top: Option<OsString>,
    rules: Vec<RuleFile>,
    config: Option<Vec<u8>>,
}

/// Where an accepted entry lands, by its path below the top folder.
enum Place {
    Config,
    Rule(OsString),
    Unused,
}

impl Unpacking {
    fn take<R: Read>(&mut self, mut entry: Entry<'_, R>) -> Result<(), Error> {
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            return Ok(());
        }

        let path = entry.path().map_err(Error::Archive)?.into_owned();
        if let Some(refusal) = refusal_for(kind) {
            return Err(Error::Unsafe { path, refusal });
        }
        let parts = plain_parts(&path)?;
        let Some((top, inside)) = parts.split_first() else {
            return Ok(());
        };
        self.check_top(top)?;

        let place = match kind {
            EntryType::Regular => place_of(inside),
            _ => Place::Unused,
        };
        match place {
            Place::Config => self.take_config(&mut entry, path),
            Place::Rule(name) => self.take_rule(&mut entry, path, name),
            Place::Unused => Ok(()),
        }
    }

    fn finish(self) -> Result<Archive, Error> {
        if self.rules.is_empty() {
            return Err(Error::NoRules);
        }

        Ok(Archive {
            rules: self.rules,
            config: self.config,
        })
    }

    fn check_top(&mut self, top: &OsStr) -> Result<(), Error> {
        match &self.top {
            None => {
                self.top = Some(top.to_owned());
                Ok(())
            }
            Some(known) if known == top => Ok(()),
            Some(_) => Err(Error::OutsideTopFolder {
                path: PathBuf::from(top),
            }),
        }
    }

    fn take_config<R: Read>(
        &mut self,
        entry: &mut Entry<'_, R>,
        path: PathBuf,
    ) -> Result<(), Error> {
        if self.config.is_some() {
            return Err(Error::Duplicate { path });
        }

        self.config = Some(read_limited(entry, path)?);
        Ok(())
    }

    fn take_rule<R: Read>(
        &mut self,
        entry: &mut Entry<'_, R>,
        path: PathBuf,
        name: OsString,
    ) -> Result<(), Error> {
        if self.rules.iter().any(|rule| rule.name == name) {
            return Err(Error::Duplicate { path });
        }

        let contents = read_limited(entry, path)?;
        self.rules.push(RuleFile { name, contents });
        Ok(())
    }
}

fn read_entries<R: Read>(tar: &mut tar::Archive<R>) -> Result<Archive, Error> {
    let entries = tar.entries().map_err(Error::Archive)?;

    let mut unpacking = Unpacking::default();
    for entry in entries {
        unpacking.take(entry.map_err(Error::Archive)?)?;
    }

    unpacking.finish()
}

/// The refusal for every entry type but regular files and folders.
fn refusal_for(kind: EntryType) -> Option<Refusal> {
    match kind {
        EntryType::Regular | EntryType::Directory => None,
        EntryType::Symlink => Some(Refusal::Symlink),
        EntryType::Link => Some(Refusal::Hardlink),
        EntryType::Char | EntryType::Block => Some(Refusal::Device),
        _ => Some(Refusal::Special),
    }
}

/// The path's plain components; `.` is dropped, and a root or `..` refuses the entry.
fn plain_parts(path: &Path) -> Result<Vec<&OsStr>, Error> {
    let refuse = |refusal| Error::Unsafe {
        path: path.to_path_buf(),
        refusal,
    };

    path.components()
        .filter(|component| *component != Component::CurDir)
        .map(|component| match component {
            Component::Normal(part) => Ok(part),
            Component::ParentDir => Err(refuse(Refusal::ParentDir)),
            _ => Err(refuse(Refusal::Absolute)),
        })
        .collect()
}

fn place_of(inside: &[&OsStr]) -> Place {
    match inside {
        [file] if *file == CONFIG_FILE => Place::Config,
        [folder, file]
            if *folder == RULES_FOLDER
                && Path::new(file).extension() == Some(OsStr::new(RULE_EXTENSION)) =>
        {
            Place::Rule(file.to_os_string())
        }
        _ => Place::Unused,
    }
}

fn read_limited<R: Read>(entry: &mut Entry<'_, R>, path: PathBuf) -> Result<Vec<u8>, Error> {
    if entry.size() > Archive::FILE_LIMIT {
        return Err(Error::TooLarge {
            path,
            limit: Archive::FILE_LIMIT,
        });
    }

    let mut contents = Vec::new();
    entry.read_to_end(&mut contents).map_err(Error::Archive)?;
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, EntryType, Header};

    use super::*;

    const TOP: &str = "top";

    /// A gzipped tarball holding one `rules/filler-N.toml` entry per size in `sizes`,
    /// each exactly that many content bytes.
    fn gzip_with_rule_sizes(sizes: &[u64]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        append_dir(&mut builder, TOP);
        append_dir(&mut builder, &format!("{TOP}/rules"));
        for (index, size) in sizes.iter().enumerate() {
            let body = vec![b'#'; *size as usize];
            let path = format!("{TOP}/rules/filler-{index}.toml");
            append_file(&mut builder, &path, &body);
        }

        let tar = builder.into_inner().expect("finish tar");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).expect("gzip tar");
        encoder.finish().expect("finish gzip")
    }

    fn append_dir(builder: &mut Builder<Vec<u8>>, path: &str) {
        let mut header = Header::new_ustar();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_path(path).expect("path fits a ustar header");
        header.set_cksum();
        builder.append(&header, &[][..]).expect("append dir");
    }

    fn append_file(builder: &mut Builder<Vec<u8>>, path: &str, body: &[u8]) {
        let mut header = Header::new_ustar();
        header.set_entry_type(EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_path(path).expect("path fits a ustar header");
        header.set_cksum();
        builder.append(&header, body).expect("append file");
    }

    #[test]
    fn refuses_a_stream_cut_mid_entry_past_the_limit() {
        let limit = 2048;
        // One entry twice the limit: the cut lands inside its data, not on a boundary.
        let gz = gzip_with_rule_sizes(&[limit * 2]);

        let result = Archive::parse_within(&gz, limit);

        assert!(
            matches!(result, Err(Error::UnpackedTooLarge { limit: got }) if got == limit),
            "got {result:?}"
        );
    }

    #[test]
    fn refuses_a_stream_cut_exactly_at_an_entry_boundary_past_the_limit() {
        // The two folder entries are one 512-byte header each, and each file entry one
        // header plus one 512-byte body block, so the limit falls exactly after the first
        // file entry.
        let entry_size = 512;
        let block = 512 + entry_size;
        let limit = block * 2;
        let gz = gzip_with_rule_sizes(&[entry_size, entry_size, entry_size]);

        let result = Archive::parse_within(&gz, limit);

        assert!(
            matches!(result, Err(Error::UnpackedTooLarge { limit: got }) if got == limit),
            "got {result:?}"
        );
    }

    #[test]
    fn accepts_a_stream_within_the_limit() {
        let gz = gzip_with_rule_sizes(&[256, 256]);

        let result = Archive::parse_within(&gz, 4096);

        assert!(result.is_ok(), "got {result:?}");
    }
}
