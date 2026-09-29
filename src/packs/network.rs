//! Downloads over HTTPS.
//!
//! The network sits beside the FS layer, not inside it: the FS layer covers filesystem,
//! command, and sudo interaction (`docs/spec/safety.md#fs-interaction-layer`), and a
//! download writes nothing. Its bytes reach disk only through the gate's own-data
//! methods. Tests inject canned responses so they never touch the network.

use std::io;
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// Fetches a URL's body.
pub trait Network {
    fn get(&self, url: &str) -> io::Result<Vec<u8>>;
}

impl<N: Network + ?Sized> Network for &N {
    fn get(&self, url: &str) -> io::Result<Vec<u8>> {
        (**self).get(url)
    }
}

/// The real network, through `ureq` with native TLS and the system's trust store
/// (`docs/spec/stack.md#crates`).
pub struct Https {
    agent: Agent,
}

impl Https {
    /// The whole request, redirects and body included, fails past this. A pack tarball
    /// is well under a megabyte; a stalled server must not hang rosie.
    pub const TIMEOUT: Duration = Duration::from_secs(30);

    /// The most gzipped bytes a download may carry;
    /// [`Archive::UNPACKED_LIMIT`](super::Archive::UNPACKED_LIMIT) bounds what they
    /// unpack to.
    pub const DOWNLOAD_LIMIT: u64 = 8 * 1024 * 1024;

    pub fn new() -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        // `https_only` also refuses a redirect to plain HTTP.
        let config = Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .timeout_global(Some(Https::TIMEOUT))
            .build();

        Https {
            agent: config.into(),
        }
    }
}

impl Default for Https {
    fn default() -> Self {
        Https::new()
    }
}

impl Network for Https {
    fn get(&self, url: &str) -> io::Result<Vec<u8>> {
        let mut response = self.agent.get(url).call().map_err(ureq::Error::into_io)?;
        response
            .body_mut()
            .with_config()
            .limit(Https::DOWNLOAD_LIMIT)
            .read_to_vec()
            .map_err(ureq::Error::into_io)
    }
}
