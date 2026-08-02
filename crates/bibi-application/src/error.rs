use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every failure a strict use case can return.
#[derive(Debug, ThisError)]
pub enum Error {
    /// Persistence failed.
    #[error(transparent)]
    Persistence(#[from] bibi_manifest::Error),
    /// Domain validation failed.
    #[error(transparent)]
    Domain(#[from] bibi_core::Error),
    /// BibTeX parsing failed.
    #[error(transparent)]
    Bibtex(#[from] bibi_bibtex::Error),
    /// Provider selection or resolution failed.
    #[error(transparent)]
    Provider(#[from] bibi_provider::Error),
    /// File I/O failed.
    #[error("{path}: {source}")]
    Io {
        /// File involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// JSON projection failed.
    #[error("could not render JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Request is invalid independently of stored state.
    #[error("{0}")]
    Usage(String),
}

impl Error {
    /// Wrap file I/O.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Construct a usage failure.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}
