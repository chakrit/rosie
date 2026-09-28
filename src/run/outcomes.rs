//! What the elevated child writes to its stdout for the run that launched it: each
//! item's outcome as it finishes, so the run's end-of-run stats cover the `sudo` part
//! too. The child reports each item on stderr itself.
//!
//! The report is a sequence of records, each ended by a NUL byte, which no path or
//! command word can hold. The first record is the header `rosie-outcomes 1`; each
//! other record is a small TOML document about one item:
//!
//! ```toml
//! outcome = "done"
//! path = "/Library/Caches/x"
//! ```
//!
//! or, for a command, `command = ["/usr/sbin/pkgutil", "--forget", "com.x"]` in place
//! of `path`. Each record is written whole and flushed before the next item, so a child
//! that crashes mid-run leaves the records of the items it finished. A last record
//! without its NUL was cut short and is ignored.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::item::{ItemResult, Subject};
use crate::plan::{Outcome, SkipReason};

const HEADER: &str = "rosie-outcomes 1";
const END: char = '\0';

#[derive(Debug, Error)]
pub enum Error {
    #[error("the elevated child's report is not valid UTF-8")]
    NotUtf8,

    #[error("the elevated child reported nothing")]
    Empty,

    #[error("the elevated child's report starts with {0:?}; expected {HEADER:?}")]
    Header(String),

    #[error("the elevated child's report record {record:?} is unreadable: {source}")]
    Parse {
        record: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("the elevated child's report has an unknown outcome {0:?}")]
    UnknownOutcome(String),

    #[error("the elevated child's report record names both a path and a command, or neither")]
    Subject,

    #[error("cannot write the elevated report: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("cannot write the elevated report: {0}")]
    Write(#[from] std::io::Error),
}

/// Which item a record is about: its path, or its command's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Path(PathBuf),
    Command(Vec<String>),
}

impl Key {
    pub fn of(subject: &Subject) -> Key {
        match subject {
            Subject::Path(path) => Key::Path(path.clone()),
            Subject::Command { argv, .. } => Key::Command(
                std::iter::once(argv.program())
                    .chain(argv.args().iter().map(|arg| arg.as_os_str()))
                    .map(|word| word.to_string_lossy().into_owned())
                    .collect(),
            ),
        }
    }
}

/// One finished item as the child reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub key: Key,
    pub outcome: Outcome,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Entry {
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<String>>,
}

/// Writes the report as items finish: the header when created, then one flushed
/// record per item.
pub struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    pub fn begin(mut out: W) -> Result<Self, Error> {
        write!(out, "{HEADER}{END}")?;
        out.flush()?;
        Ok(Writer { out })
    }

    pub fn record(&mut self, result: &ItemResult) -> Result<(), Error> {
        let (path, command) = match Key::of(&result.subject) {
            Key::Path(path) => (Some(path), None),
            Key::Command(words) => (None, Some(words)),
        };
        let entry = Entry {
            outcome: outcome_label(result.outcome).to_owned(),
            path,
            command,
        };

        let record = format!("{}{END}", toml::to_string(&entry)?);
        self.out.write_all(record.as_bytes())?;
        self.out.flush()?;
        Ok(())
    }
}

/// Reads a report back: every whole record after the header. Output that does not
/// start with the header, such as the empty output of a sudo that refused, is an
/// error.
pub fn decode(bytes: &[u8]) -> Result<Vec<Record>, Error> {
    let text = str::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
    let whole = text
        .split_inclusive(END)
        .filter(|record| record.ends_with(END));

    let mut records = whole.map(|record| record.trim_end_matches(END));
    let header = records.next().ok_or(Error::Empty)?;
    if header != HEADER {
        return Err(Error::Header(header.to_owned()));
    }

    records.map(decode_record).collect()
}

fn decode_record(record: &str) -> Result<Record, Error> {
    let entry: Entry = toml::from_str(record).map_err(|source| Error::Parse {
        record: record.to_owned(),
        source,
    })?;

    let key = match (entry.path, entry.command) {
        (Some(path), None) => Key::Path(path),
        (None, Some(words)) => Key::Command(words),
        _ => return Err(Error::Subject),
    };
    let outcome = parse_outcome(&entry.outcome)?;
    Ok(Record { key, outcome })
}

const OUTCOMES: [Outcome; 6] = [
    Outcome::Done,
    Outcome::Skipped(SkipReason::Stale),
    Outcome::Skipped(SkipReason::RunningProcess),
    Outcome::Skipped(SkipReason::SudoRefused),
    Outcome::Skipped(SkipReason::UnsafeToElevate),
    Outcome::Failed,
];

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Done => "done",
        Outcome::Skipped(reason) => reason.label(),
        Outcome::Failed => "failed",
    }
}

fn parse_outcome(label: &str) -> Result<Outcome, Error> {
    OUTCOMES
        .into_iter()
        .find(|outcome| outcome_label(*outcome) == label)
        .ok_or_else(|| Error::UnknownOutcome(label.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Argv;
    use crate::plan::ItemSize;

    fn path_result(path: &str, outcome: Outcome) -> ItemResult {
        ItemResult {
            subject: Subject::Path(PathBuf::from(path)),
            size: ItemSize::Unknown,
            outcome,
            reason: String::new(),
        }
    }

    #[test]
    fn reads_back_every_outcome_for_paths_and_commands() {
        let mut results: Vec<ItemResult> = OUTCOMES
            .into_iter()
            .map(|outcome| path_result("/Library/Caches/a \"b\"\nc", outcome))
            .collect();
        results.push(ItemResult {
            subject: Subject::Command {
                argv: Argv::new("/usr/sbin/pkgutil").arg("--forget").arg("com.x"),
                output: vec!["kept on stderr only".into()],
            },
            size: ItemSize::Unknown,
            outcome: Outcome::Done,
            reason: String::new(),
        });

        let mut writer = Writer::begin(Vec::new()).expect("header");
        for result in &results {
            writer.record(result).expect("recordable");
        }
        let decoded = decode(&writer.out).expect("decodable");

        let expected: Vec<Record> = results
            .iter()
            .map(|result| Record {
                key: Key::of(&result.subject),
                outcome: result.outcome,
            })
            .collect();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn ignores_a_last_record_cut_short() {
        let text = "rosie-outcomes 1\0\
                    outcome = \"done\"\npath = \"/a\"\n\0\
                    outcome = \"fail";

        let decoded = decode(text.as_bytes()).expect("the whole records decode");

        assert_eq!(
            decoded,
            vec![Record {
                key: Key::Path(PathBuf::from("/a")),
                outcome: Outcome::Done,
            }]
        );
    }

    #[test]
    fn refuses_output_without_the_header() {
        assert!(matches!(decode(b""), Err(Error::Empty)));
        assert!(matches!(
            decode(b"rosie-outcomes 2\0"),
            Err(Error::Header(_))
        ));
    }
}
