use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every failure a use case can return.
///
/// A partial batch is not one of them. Resolving fifty of two hundred entries
/// and then hitting a network error returns a successful report carrying typed
/// failures, so the forty-nine that worked are committed and each failure keeps
/// its own diagnostic.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The manifest could not be read, validated, or written.
    #[error(transparent)]
    Manifest(#[from] bibi_manifest::Error),
    /// A domain value was rejected.
    #[error(transparent)]
    Domain(#[from] bibi_core::Error),
    /// BibTeX could not be parsed or rendered.
    #[error(transparent)]
    Bibtex(#[from] bibi_bibtex::Error),
    /// Provider selection, construction, or qualifier validation failed.
    #[error(transparent)]
    Provider(#[from] bibi_provider::Error),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// An artifact could not be retrieved.
    #[error(transparent)]
    Documents(#[from] bibi_documents::Error),
    /// JSON output could not be produced.
    #[error("could not render JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The request itself is invalid, independently of any stored state.
    #[error("{0}")]
    Usage(String),
}

impl Error {
    /// Wrap an I/O failure with the path it concerns.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// A usage error, phrased for the person who typed the command.
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}
