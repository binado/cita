use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every persistence failure.
#[derive(Debug, ThisError)]
pub enum Error {
    /// A manifest file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// The manifest is not valid TOML, or a value has the wrong type.
    #[error("{path} is not a valid bibliography: {source}")]
    Decode {
        /// The file involved.
        path: PathBuf,
        /// The TOML failure.
        #[source]
        source: toml::de::Error,
    },
    /// A bibliography could not be serialized.
    #[error("could not serialize the bibliography: {0}")]
    Encode(#[from] toml::ser::Error),
    /// The manifest states no schema version.
    #[error("{path} states no `schema` version; it was not written by bibi")]
    MissingSchema {
        /// The file involved.
        path: PathBuf,
    },
    /// The manifest states a schema this build does not implement.
    #[error("{path} uses schema {found}, but this bibi implements schema {supported}")]
    UnsupportedSchema {
        /// The file involved.
        path: PathBuf,
        /// The schema found.
        found: u64,
        /// The supported schema.
        supported: u64,
    },
    /// A stored field is not a valid domain value.
    #[error("record `{record}` has an invalid `{field}` field: {source}")]
    InvalidField {
        /// Record label used for the diagnostic.
        record: String,
        /// Rejected field.
        field: &'static str,
        /// Domain validation failure.
        #[source]
        source: bibi_core::Error,
    },
    /// Source and provider id do not form a legal source.
    #[error("record `{record}` has an invalid source: {message}")]
    InvalidSource {
        /// Record label used for the diagnostic.
        record: String,
        /// Shape problem.
        message: &'static str,
    },
    /// Stored BibTeX is not one valid entry.
    #[error("record `{record}` has invalid BibTeX: {source}")]
    InvalidPayload {
        /// Record label used for the diagnostic.
        record: String,
        /// BibTeX parse failure.
        #[source]
        source: bibi_bibtex::Error,
    },
    /// The complete restored bibliography violates a domain invariant.
    #[error(transparent)]
    Bibliography(#[from] bibi_core::Error),
    /// The manifest changed after it was read.
    #[error("{path} changed since it was read; re-run the command")]
    StaleBibliography {
        /// The file involved.
        path: PathBuf,
    },
    /// A bibliography was required but does not exist.
    #[error("no bibliography at {path}")]
    NoBibliography {
        /// Where one was expected.
        path: PathBuf,
    },
    /// A bibliography already exists where one was to be created.
    #[error("{path} already exists")]
    AlreadyExists {
        /// The file involved.
        path: PathBuf,
    },
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
