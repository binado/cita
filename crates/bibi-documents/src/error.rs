use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every failure the document cache can produce.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The cache root is unusable or unsafe to own.
    #[error("unusable document cache root {path}: {reason}")]
    UnsafeRoot {
        /// The rejected root.
        path: PathBuf,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// A download did not complete.
    #[error("could not download {url}: {source}")]
    Download {
        /// What was being fetched.
        url: String,
        /// The underlying failure.
        #[source]
        source: reqwest::Error,
    },
    /// The server answered, unsuccessfully.
    #[error("{url} returned HTTP {status}")]
    Status {
        /// What was being fetched.
        url: String,
        /// The status code.
        status: u16,
    },
    /// arXiv has no such artifact.
    #[error("arXiv has no {kind} for {id}")]
    NotFound {
        /// The identifier.
        id: String,
        /// Which artifact.
        kind: &'static str,
    },
    /// A response exceeded the configured download bound.
    #[error("arXiv {kind} for {id} exceeds the {limit}-byte download limit")]
    ArtifactTooLarge {
        /// The identifier being fetched.
        id: String,
        /// Which artifact exceeded its bound.
        kind: &'static str,
        /// The maximum accepted response size.
        limit: usize,
    },
    /// The response was not the artifact it claimed to be.
    #[error("{id}: {reason}")]
    InvalidArtifact {
        /// The identifier.
        id: String,
        /// What was wrong.
        reason: String,
    },
    /// A source archive is malformed, oversized, or unsafe to extract.
    #[error("{id}: unsafe or malformed source archive: {reason}")]
    UnsafeArchive {
        /// The identifier.
        id: String,
        /// What was wrong.
        reason: String,
    },
    /// A cache entry could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// A cached artifact exists but is not usable.
    #[error("cached artifact at {path} is unusable; re-fetch it with `--force`")]
    InvalidCacheEntry {
        /// The path involved.
        path: PathBuf,
    },
    /// The client could not be built.
    #[error("could not build the arXiv client: {0}")]
    Client(#[source] reqwest::Error),
    /// The base URL is not a URL.
    #[error("invalid arXiv base URL `{0}`")]
    InvalidBaseUrl(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn unsafe_archive(id: &str, reason: impl std::fmt::Display) -> Self {
        Self::UnsafeArchive {
            id: id.to_owned(),
            reason: reason.to_string(),
        }
    }
}
