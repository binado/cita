//! Error type for document fetch and cache operations.

use std::path::PathBuf;

use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
/// Error produced while locating, downloading, validating, or caching an artifact.
pub enum Error {
    /// The supplied arXiv identifier is invalid.
    #[error("reference contains an invalid arXiv identifier: `{0}`")]
    InvalidArxivIdentifier(String),
    /// The configured base URL is invalid.
    #[error("invalid arXiv base URL: {0}")]
    InvalidBaseUrl(String),
    /// A cache directory could not be created.
    #[error("could not create document cache directory {path}")]
    CreateCacheDirectory {
        /// Directory that could not be created.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// An existing cached file could not be inspected.
    #[error("could not inspect cached PDF {path}")]
    InspectCachedPdf {
        /// Cached file that could not be inspected.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// An existing cache entry does not have a PDF signature.
    #[error("cached file {0} is not a valid PDF")]
    InvalidCachedPdf(PathBuf),
    /// Cache-only mode found no file at the expected path.
    #[error("PDF is not cached at {0}")]
    NotCached(PathBuf),
    /// The source service has no TeX source package for this identifier.
    #[error("no TeX source is available for `{0}`")]
    SourceUnavailable(String),
    /// A downloaded source response is not a valid, safe gzip-compressed tar archive.
    #[error("invalid or unsafe source archive for `{arxiv_id}`: {reason}")]
    InvalidSourceArchive {
        /// Normalized arXiv identifier requested.
        arxiv_id: String,
        /// Reason the archive was rejected.
        reason: String,
    },
    /// An existing source cache entry is not a valid non-empty directory tree.
    #[error("cached source directory {0} is invalid")]
    InvalidCachedSource(PathBuf),
    /// Cache-only mode found no source directory at the expected path.
    #[error("source is not cached at {0}")]
    SourceNotCached(PathBuf),
    /// A source package could not be extracted into its temporary directory.
    #[error("could not extract source package for {path}")]
    ExtractSource {
        /// Path being written during extraction (a staging path, or the final
        /// cache path when staging setup fails).
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A complete extracted source tree could not be published to the cache.
    #[error("could not publish source cache directory {path}")]
    PublishSource {
        /// Intended final source cache path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The HTTP request failed before a response was received.
    #[error("arXiv request failed: {0}")]
    Transport(#[source] reqwest::Error),
    /// arXiv returned an unsuccessful HTTP status.
    #[error("arXiv returned HTTP {status} for `{arxiv_id}`")]
    HttpStatus {
        /// Normalized arXiv identifier requested.
        arxiv_id: String,
        /// HTTP response status.
        status: StatusCode,
    },
    /// The downloaded response does not have a PDF signature.
    #[error("arXiv returned non-PDF content for `{0}`")]
    InvalidDownloadedPdf(String),
    /// A temporary or final cache file could not be written.
    #[error("could not write PDF cache file {path}")]
    Write {
        /// Intended final cache path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
}
