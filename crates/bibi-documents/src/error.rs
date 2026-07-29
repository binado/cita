use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every failure artifact retrieval can produce.
#[derive(Debug, ThisError)]
pub enum Error {
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
    /// Something is already at the destination.
    ///
    /// Retrieval never replaces a file. The caller chose the path — either
    /// explicitly or through the default naming — and overwriting it would
    /// discard whatever was there on the strength of a guess about what the
    /// user meant.
    #[error("{path} already exists")]
    DestinationExists {
        /// The occupied path.
        path: PathBuf,
    },
    /// An artifact could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
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
}
