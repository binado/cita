//! The `.bib` file as the authoritative store.
//!
//! Bibliographic content is the BibTeX itself. Tool-owned bookkeeping lives in
//! [`FIELD_PREFIX`]-namespaced fields on the entries, so there is no second file
//! to keep in step and nothing that can drift.
//!
//! The file belongs to the user, who is expected to edit it by hand. Every
//! mutation therefore rewrites only the entries it touches: see [`Bibfile`].
#![warn(missing_docs)]

mod entry;
mod store;

pub use entry::{
    ARXIV_FIELD, DOI_FIELD, Entry, FIELD_PREFIX, FROZEN_FIELD, INSPIRE_ID_FIELD,
    INSPIRE_UPDATED_FIELD,
};
pub use store::{
    AddOutcome, Bibfile, ConflictPolicy, Diagnostic, KeyRequest, PendingReference,
    ProjectedReference, atomic_write,
};

use std::path::PathBuf;
use thiserror::Error;

/// Default bibliography file name, used when no path is given.
pub const BIBLIOGRAPHY_FILE: &str = "references.bib";

/// Environment variable naming the bibliography to operate on.
pub const PATH_ENV: &str = "BIBI_BIB";

/// Error produced while reading, validating, or rewriting a bibliography.
#[derive(Debug, Error)]
pub enum Error {
    /// The bibliography does not exist. Never created implicitly: a mistyped
    /// directory must not silently become a new, empty bibliography.
    #[error(
        "no {BIBLIOGRAPHY_FILE} at {0}; create one with `touch {BIBLIOGRAPHY_FILE}`, or pass --path"
    )]
    Missing(PathBuf),
    /// The bibliography could not be read.
    #[error("could not read {path}")]
    Read {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The bibliography could not be written.
    #[error("could not write {path}")]
    Write {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid BibTeX, or an entry is unusable.
    #[error("invalid bibliography: {0}")]
    Invalid(#[from] bibi_bibliography::Error),
    /// One entry could not be projected into a reference.
    #[error("could not read `{key}`: {message}")]
    InvalidEntry {
        /// Local citation key of the offending entry.
        key: String,
        /// What was wrong with it.
        message: String,
    },
    /// Two entries claim the same normalized DOI or arXiv identifier.
    #[error("`{key}` and `{other}` share the identity {identity}")]
    DuplicateIdentity {
        /// One local key holding the identity.
        key: String,
        /// The other local key holding it.
        other: String,
        /// The normalized identity itself.
        identity: String,
    },
    /// A selector matched nothing.
    #[error("no reference matches `{0}`")]
    NoMatch(String),
    /// An exact key was requested for a record already stored under another key.
    #[error("INSPIRE record already stored as `{existing}`; requested `{requested}`")]
    CannotRename {
        /// Key the record is already stored under.
        existing: String,
        /// Key the caller asked for.
        requested: String,
    },
    /// A local key is already taken by a different reference.
    #[error("local key `{0}` is already in use")]
    KeyInUse(String),
    /// Two entries claim the same INSPIRE record.
    #[error("`{key}` and `{other}` both claim INSPIRE record {record_id}")]
    DuplicateRecord {
        /// One local key claiming the record.
        key: String,
        /// The other local key claiming it.
        other: String,
        /// The contested record id.
        record_id: u64,
    },
    /// A managed entry got no record back from the provider.
    #[error("INSPIRE returned no record {record_id} for `{key}`")]
    MissingRecord {
        /// Local key that asked for the record.
        key: String,
        /// Record id that went unanswered.
        record_id: u64,
    },
    /// The provider returned a record nothing asked for.
    #[error("INSPIRE returned record {0}, which no entry requested")]
    UnexpectedRecord(u64),
}
