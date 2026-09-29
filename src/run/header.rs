//! The header a run writes to the elevated child's stdin, ahead of the items
//! (`docs/spec/safety.md#internal-elevated-entry`):
//!
//! ```text
//! rosie-elevated 1
//! nonce 3f9a…
//! home /Users/me
//!
//! <the sudo part of the plan, as plan TOML>
//! ```

use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::fs::Home;

/// The header format this rosie writes and reads.
pub const HEADER_VERSION: u32 = 1;

const MAGIC: &str = "rosie-elevated";
const NONCE_HEX_DIGITS: usize = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("stdin does not start with a rosie elevation header")]
    Missing,

    #[error(
        "elevation header version {found} is not supported; this rosie reads version {HEADER_VERSION}"
    )]
    Version { found: String },

    #[error("elevation header line {line} is not `{expected}`")]
    Malformed { line: usize, expected: &'static str },

    #[error("the elevation nonce is not {NONCE_HEX_DIGITS} lowercase hex digits")]
    InvalidNonce,

    #[error("home {home:?} is not an absolute UTF-8 path on one line")]
    InvalidHome { home: PathBuf },

    #[error("the elevated items are not valid UTF-8")]
    NotUtf8,
}

/// A random token the run passes both as the child's argument and in its stdin header,
/// proving the piped items come from the run that launched the child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nonce(String);

impl Nonce {
    /// 128 bits from std's per-process random hash keys: no FFI, no extra crate.
    pub fn generate() -> Self {
        let halves = [0u8, 1].map(|salt| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u8(salt);
            hasher.write_u32(std::process::id());
            hasher.write_u128(nanos_since_epoch());
            hasher.finish()
        });
        Nonce(format!("{:016x}{:016x}", halves[0], halves[1]))
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let hex = text.len() == NONCE_HEX_DIGITS
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        match hex {
            true => Ok(Nonce(text.to_owned())),
            false => Err(Error::InvalidNonce),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn nanos_since_epoch() -> u128 {
    // A clock before 1970 still leaves the random hash keys to make the nonce unique.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub nonce: Nonce,
    /// The invoking user's home, where the child loads their config and roots from.
    pub home: Home,
}

impl Header {
    /// The header followed by `body`.
    pub fn encode(&self, body: &str) -> Result<Vec<u8>, Error> {
        let home = home_text(&self.home)?;
        let text = format!(
            "{MAGIC} {HEADER_VERSION}\nnonce {}\nhome {home}\n\n{body}",
            self.nonce.as_str()
        );
        Ok(text.into_bytes())
    }

    /// Splits stdin into its header and the body after it.
    pub fn decode(bytes: &[u8]) -> Result<(Header, &str), Error> {
        let text = str::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
        let mut lines = text.splitn(5, '\n');
        let mut next = || lines.next().unwrap_or_default();

        let version = next().strip_prefix(MAGIC).ok_or(Error::Missing)?;
        let version = version.strip_prefix(' ').ok_or(Error::Missing)?;
        if version != HEADER_VERSION.to_string() {
            return Err(Error::Version {
                found: version.to_owned(),
            });
        }
        let nonce = field(next(), 2, "nonce <hex>", "nonce ")?;
        let home = field(next(), 3, "home <path>", "home ")?;
        if !next().is_empty() {
            return Err(Error::Malformed {
                line: 4,
                expected: "",
            });
        }
        let body = next();

        let home = PathBuf::from(home);
        home_text(&home)?;
        let home = Home::new(&home).map_err(|_| Error::InvalidHome { home: home.clone() })?;
        let header = Header {
            nonce: Nonce::parse(nonce)?,
            home,
        };
        Ok((header, body))
    }
}

fn field<'a>(
    line: &'a str,
    number: usize,
    expected: &'static str,
    prefix: &str,
) -> Result<&'a str, Error> {
    line.strip_prefix(prefix).ok_or(Error::Malformed {
        line: number,
        expected,
    })
}

fn home_text(home: &Path) -> Result<&str, Error> {
    let invalid = || Error::InvalidHome {
        home: home.to_path_buf(),
    };
    let text = home.to_str().ok_or_else(invalid)?;
    match home.is_absolute() && !text.contains('\n') {
        true => Ok(text),
        false => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            nonce: Nonce::parse("0123456789abcdef0123456789abcdef").expect("valid nonce"),
            home: Home::new(Path::new("/Users/me")).expect("absolute home"),
        }
    }

    #[test]
    fn reads_back_what_it_writes() {
        let bytes = header().encode("version = 1\n").expect("encodable");

        let (decoded, body) = Header::decode(&bytes).expect("decodable");

        assert_eq!(decoded, header());
        assert_eq!(body, "version = 1\n");
    }

    #[test]
    fn generates_distinct_well_formed_nonces() {
        let first = Nonce::generate();
        let second = Nonce::generate();

        assert_ne!(first, second);
        assert_eq!(Nonce::parse(first.as_str()), Ok(first));
    }

    #[test]
    fn refuses_another_header_version() {
        let text = "rosie-elevated 2\nnonce 0123456789abcdef0123456789abcdef\nhome /Users/me\n\n";

        let decoded = Header::decode(text.as_bytes());

        assert_eq!(
            decoded.map(|(header, _)| header),
            Err(Error::Version { found: "2".into() })
        );
    }

    #[test]
    fn refuses_stdin_without_a_header() {
        let decoded = Header::decode(b"version = 1\n[[delete]]\n");

        assert_eq!(decoded.map(|(header, _)| header), Err(Error::Missing));
    }

    #[test]
    fn refuses_a_malformed_nonce() {
        let text = "rosie-elevated 1\nnonce 0123\nhome /Users/me\n\n";

        let decoded = Header::decode(text.as_bytes());

        assert_eq!(decoded.map(|(header, _)| header), Err(Error::InvalidNonce));
    }

    #[test]
    fn refuses_a_relative_home() {
        let text = "rosie-elevated 1\nnonce 0123456789abcdef0123456789abcdef\nhome me\n\n";

        let decoded = Header::decode(text.as_bytes());

        assert!(matches!(decoded, Err(Error::InvalidHome { .. })));
    }

    #[test]
    fn refuses_to_write_a_home_that_would_break_the_header() {
        let header = Header {
            home: Home::new(Path::new("/Users/me\nnonce x")).expect("absolute home"),
            ..header()
        };

        assert!(matches!(header.encode(""), Err(Error::InvalidHome { .. })));
    }
}
