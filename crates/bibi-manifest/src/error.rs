use std::path::PathBuf;
use thiserror::Error as ThisError;

/// Every failure this crate can produce.
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
    #[error("{path} is not a valid manifest: {source}")]
    Decode {
        /// The file involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: toml::de::Error,
    },
    /// A candidate could not be serialized.
    #[error("could not serialize the manifest: {0}")]
    Encode(#[from] toml::ser::Error),
    /// The manifest states no schema version.
    #[error("{path} states no `schema` version; it was not written by bibi")]
    MissingSchema {
        /// The file involved.
        path: PathBuf,
    },
    /// The manifest states a schema this build does not implement.
    #[error(
        "{path} uses manifest schema {found}, but this bibi implements schema {supported}; upgrade bibi to read it"
    )]
    UnsupportedSchema {
        /// The file involved.
        path: PathBuf,
        /// The schema the file states.
        found: u64,
        /// The schema this build implements.
        supported: u64,
    },
    /// Two records claim one identity.
    #[error("two records share the same {kind} `{value}`: `{first}` and `{second}`")]
    Duplicate {
        /// Which index rejected them.
        kind: &'static str,
        /// The shared value.
        value: String,
        /// The local key of the first record.
        first: String,
        /// The local key of the second record.
        second: String,
    },
    /// A record failed domain validation.
    #[error(transparent)]
    Record(#[from] bibi_core::Error),
    /// A stored value is not a valid domain value.
    #[error("record `{key}` has an invalid `{field}` field: {source}")]
    InvalidField {
        /// The record's local key, or its id when the key itself is invalid.
        key: String,
        /// Which field was rejected.
        field: &'static str,
        /// The underlying failure.
        #[source]
        source: bibi_core::Error,
    },
    /// A stored payload is not one valid standalone entry.
    #[error("record `{key}` has an invalid payload: {source}")]
    InvalidPayload {
        /// The record's local key.
        key: String,
        /// The underlying failure.
        #[source]
        source: bibi_bibtex::Error,
    },
    /// The manifest changed on disk after it was read.
    #[error("{path} changed since it was read; re-run the command")]
    StaleManifest {
        /// The file involved.
        path: PathBuf,
    },
    /// A manifest was required but does not exist.
    #[error("no manifest at {path}")]
    NoManifest {
        /// Where one was expected.
        path: PathBuf,
    },
    /// A manifest already exists where one was to be created.
    #[error("{path} already exists")]
    AlreadyExists {
        /// The file involved.
        path: PathBuf,
    },
    /// A mutation named a record the candidate does not hold.
    #[error("no record with id `{id}`")]
    UnknownRecord {
        /// The requested id.
        id: String,
    },
    /// A citation key is already taken by a different record.
    #[error("citation key `{key}` already belongs to another record")]
    KeyInUse {
        /// The requested key.
        key: String,
    },
    /// A selector matched no record.
    #[error("no record matches `{selector}`")]
    NoMatch {
        /// The selector as written.
        selector: String,
    },
}

impl Error {
    /// Wrap an I/O failure with the path it concerns.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
