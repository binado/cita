mod connection;
mod models;
mod reads;
mod schema;
mod writes;

use crate::{DATABASE_FILE, DEFAULT_SHELF, PendingReference, SourceSnapshot};
use cita_core::Reference;
use diesel::sqlite::SqliteConnection;
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub(crate) use reads::shelf_id;
pub(crate) use writes::{
    clear_library, insert_membership, insert_reference, insert_shelf, source_identities,
};

const FILES_DIR: &str = "files";

/// Resolve the global cita root from `CITA_HOME` or the operating-system home.
pub fn global_library_root() -> Result<PathBuf, LibraryError> {
    if let Some(configured) = env::var_os("CITA_HOME") {
        if configured.is_empty() {
            return Err(LibraryError::EmptyRoot);
        }
        let root = PathBuf::from(configured);
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root));
        }
        return Ok(root);
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .ok_or(LibraryError::HomeUnavailable)?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(LibraryError::RelativeHome(home));
    }
    Ok(home.join(".cita"))
}

/// A validated shelf name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ShelfName(String);

impl ShelfName {
    /// Return the fixed default shelf name.
    pub fn default_shelf() -> Self {
        Self(DEFAULT_SHELF.into())
    }

    /// Return the validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ShelfName {
    type Error = LibraryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut bytes = value.bytes();
        if !bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(LibraryError::InvalidName(value.into()));
        }
        Ok(Self(value.into()))
    }
}

impl TryFrom<String> for ShelfName {
    type Error = LibraryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl std::str::FromStr for ShelfName {
    type Err = LibraryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl std::fmt::Display for ShelfName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for ShelfName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// SQLite storage failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// `CITA_HOME` is empty.
    #[error("CITA_HOME cannot be empty")]
    EmptyRoot,
    /// The configured root is relative.
    #[error("cita home must be an absolute path: {0}")]
    RelativeRoot(PathBuf),
    /// The operating-system home is relative.
    #[error("home directory must be absolute: {0}; set CITA_HOME to override")]
    RelativeHome(PathBuf),
    /// No home directory was available.
    #[error("could not determine the home directory; set CITA_HOME")]
    HomeUnavailable,
    /// A managed filesystem operation failed.
    #[error("could not access {path}")]
    Filesystem {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A managed database path is not a direct regular file.
    #[error("invalid database path {0}: expected a direct regular file")]
    InvalidDatabasePath(PathBuf),
    /// A managed database path could not be represented as a SQLite file URL.
    #[error("could not represent database path as a SQLite URL: {0}")]
    InvalidDatabaseUrl(PathBuf),
    /// The database schema is unsupported.
    #[error("unsupported SQLite schema {found}; this version supports schema 1")]
    UnsupportedSchema {
        /// Schema version found.
        found: i64,
    },
    /// A shelf name is malformed.
    #[error("invalid shelf name `{0}`; names must match [A-Za-z0-9][A-Za-z0-9._-]*")]
    InvalidName(String),
    /// A requested shelf does not exist.
    #[error("unknown shelf `{0}`")]
    UnknownShelf(String),
    /// A shelf differs from an existing name only by ASCII case.
    #[error("shelf `{requested}` aliases existing shelf `{existing}` by case")]
    ShelfAlias {
        /// Requested spelling.
        requested: String,
        /// Existing spelling.
        existing: String,
    },
    /// A reference selector did not match.
    #[error("reference `{0}` was not found")]
    ReferenceNotFound(String),
    /// More than one global reference matched incoming identities.
    #[error("incoming identities refer to different global references")]
    IdentityConflict,
    /// A global identity is already owned by another reference.
    #[error("{kind} identity `{value}` is already owned by another reference")]
    DuplicateIdentity {
        /// Identity kind.
        kind: String,
        /// Normalized value.
        value: String,
    },
    /// A sync result raced another mutation.
    #[error("reference changed while INSPIRE data was being fetched")]
    ConcurrentChange,
    /// Source data is malformed.
    #[error("invalid source metadata: {0}")]
    InvalidSource(String),
    /// A lossless interchange document is inconsistent.
    #[error("invalid interchange document: {0}")]
    InvalidInterchange(String),
    /// A SQLite connection could not be established.
    #[error(transparent)]
    Connection(#[from] diesel::ConnectionError),
    /// A database operation failed.
    #[error(transparent)]
    Database(#[from] diesel::result::Error),
    /// BibTeX operation failed.
    #[error(transparent)]
    Bibtex(#[from] cita_bibliography::Error),
}

/// Whether local citation-key collisions are skipped or replaced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConflictPolicy {
    /// Keep the existing shelf membership.
    #[default]
    Skip,
    /// Replace only colliding shelf memberships.
    Overwrite,
}

/// Result of adding one reference to a shelf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddOutcome {
    /// A membership was added.
    Added(String),
    /// The reference already belonged to the shelf.
    Existing(String),
    /// A local collision was retained.
    Skipped {
        /// Requested key.
        key: String,
        /// Existing conflicting key.
        conflicting: String,
    },
    /// Local memberships were replaced.
    Overwritten {
        /// New key.
        key: String,
        /// Removed or renamed local keys.
        replaced: Vec<String>,
    },
}

/// One entry omitted by a lenient transactional batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedReference {
    /// Requested shelf-local key.
    pub key: String,
    /// Validation or storage diagnostic.
    pub message: String,
}

/// A shelf-local key and its semantic reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    /// Shelf-local citation key.
    pub key: String,
    /// Stored semantic projection.
    pub reference: Reference,
}

/// Complete data needed to render a shelf entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfEntry {
    /// Private database reference ID.
    pub(crate) id: i64,
    /// Shelf-local citation key.
    pub key: String,
    /// Authoritative source snapshot.
    pub source: SourceSnapshot,
    /// Stored semantic projection.
    pub reference: Reference,
}

/// A reference selected for INSPIRE synchronization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncCandidate {
    /// Private database reference ID.
    pub id: i64,
    /// Snapshot observed before network access.
    pub source: SourceSnapshot,
}

/// An INSPIRE result guarded by its previously observed snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncUpdate {
    /// Private database reference ID.
    pub id: i64,
    /// Snapshot that must still be stored when applying the update.
    pub expected: SourceSnapshot,
    /// Replacement snapshot.
    pub replacement: SourceSnapshot,
}

/// Handle for the one user-global SQLite library.
#[derive(Clone, Debug)]
pub struct Library {
    pub(crate) root: PathBuf,
}

impl Library {
    /// Open the library, creating the database and `main` shelf when absent.
    pub fn open_or_create(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root.into()));
        }
        connection::create_directory(root)?;
        connection::create_directory(&root.join(FILES_DIR))?;
        let library = Self { root: root.into() };
        let mut connection = library.connection()?;
        connection::initialize_schema(&mut connection)?;
        connection::ensure_main(&mut connection)?;
        Ok(library)
    }

    /// Open an existing database without creating it.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root.into()));
        }
        let library = Self { root: root.into() };
        connection::validate_database_file(&library.path())?;
        let mut connection = library.connection()?;
        connection::validate_schema(&mut connection)?;
        connection::ensure_main(&mut connection)?;
        Ok(library)
    }

    /// Return the global store root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the authoritative database path.
    pub fn path(&self) -> PathBuf {
        self.root.join(DATABASE_FILE)
    }

    /// Return the shared document-cache root.
    pub fn files_root(&self) -> PathBuf {
        self.root.join(FILES_DIR)
    }

    /// Return shelf names in deterministic order.
    pub fn shelves(&self) -> Result<BTreeSet<ShelfName>, LibraryError> {
        reads::shelves(&mut self.connection()?)
    }

    /// Create a shelf, returning whether it was new.
    pub fn create_shelf(&self, name: &ShelfName) -> Result<bool, LibraryError> {
        writes::create_shelf(&mut self.connection()?, name)
    }

    /// Confirm that a shelf exists.
    pub fn validate_shelf(&self, name: &ShelfName) -> Result<(), LibraryError> {
        reads::shelf_id(&mut self.connection()?, name).map(|_| ())
    }

    /// Return all entries in a shelf ordered by citation key.
    pub fn entries(&self, shelf: &ShelfName) -> Result<Vec<ShelfEntry>, LibraryError> {
        reads::entries(&mut self.connection()?, shelf)
    }

    /// Return semantic references in shelf-key order.
    pub fn projected(&self, shelf: &ShelfName) -> Result<Vec<ProjectedReference>, LibraryError> {
        reads::projected(&mut self.connection()?, shelf)
    }

    /// Find within one shelf by exact key, INSPIRE ID, DOI, or arXiv ID.
    pub fn find(
        &self,
        shelf: &ShelfName,
        selector: &str,
    ) -> Result<Option<ProjectedReference>, LibraryError> {
        reads::find(&mut self.connection()?, shelf, selector)
    }

    /// Add a validated batch atomically.
    pub fn add_batch(
        &self,
        shelf: &ShelfName,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<Vec<AddOutcome>, LibraryError> {
        writes::add_batch(&mut self.connection()?, shelf, pending, policy)
    }

    /// Add every valid item in one transaction and report isolatable failures.
    pub fn add_batch_skipping_errors(
        &self,
        shelf: &ShelfName,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<(Vec<AddOutcome>, Vec<SkippedReference>), LibraryError> {
        writes::add_batch_skipping_errors(&mut self.connection()?, shelf, pending, policy)
    }

    /// Remove all selected memberships atomically and delete resulting orphans.
    pub fn remove_batch(
        &self,
        shelf: &ShelfName,
        selectors: &[String],
    ) -> Result<Vec<ProjectedReference>, LibraryError> {
        writes::remove_batch(&mut self.connection()?, shelf, selectors)
    }

    /// Select unique references for a shelf or the complete library.
    pub fn sync_candidates(
        &self,
        shelf: Option<&ShelfName>,
    ) -> Result<Vec<SyncCandidate>, LibraryError> {
        reads::sync_candidates(&mut self.connection()?, shelf)
    }

    /// Apply a complete INSPIRE result set atomically.
    pub fn apply_sync(&self, updates: Vec<SyncUpdate>) -> Result<usize, LibraryError> {
        writes::apply_sync(&mut self.connection()?, updates)
    }

    pub(crate) fn connection(&self) -> Result<SqliteConnection, LibraryError> {
        connection::establish(&self.path())
    }
}

#[cfg(test)]
mod tests;
