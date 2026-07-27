use crate::{
    DATABASE_FILE, DEFAULT_SHELF, KeyRequest, PendingReference, SourceSnapshot,
    source::{HepIdentifiers, InspireEntry},
};
use cita_bibliography::{BibtexSnapshot, validate_key};
use cita_core::{Identifiers, Locator, Reference, normalize_arxiv, normalize_doi};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

const DATABASE_SCHEMA: i64 = 1;
const FILES_DIR: &str = "files";

const CREATE_SCHEMA: &str = r#"
CREATE TABLE bibliography_references (
    id          INTEGER PRIMARY KEY,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('import', 'inspire')),
    bibtex      TEXT NOT NULL,
    title       TEXT NOT NULL CHECK (length(trim(title)) > 0),
    year        INTEGER
);
CREATE TABLE contributors (
    reference_id INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('author', 'collaboration')),
    position     INTEGER NOT NULL CHECK (position >= 0),
    name         TEXT NOT NULL,
    PRIMARY KEY (reference_id, kind, position)
);
CREATE TABLE identities (
    reference_id INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('doi', 'arxiv')),
    value        TEXT NOT NULL,
    canonical    INTEGER NOT NULL CHECK (canonical IN (0, 1)),
    PRIMARY KEY (reference_id, kind, value),
    UNIQUE (kind, value)
);
CREATE UNIQUE INDEX one_canonical_identity
    ON identities(reference_id, kind) WHERE canonical = 1;
CREATE TABLE inspire_records (
    reference_id INTEGER PRIMARY KEY REFERENCES bibliography_references(id) ON DELETE CASCADE,
    record_id    INTEGER NOT NULL UNIQUE CHECK (record_id > 0),
    updated      TEXT NOT NULL
);
CREATE TABLE shelves (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE shelf_references (
    shelf_id      INTEGER NOT NULL REFERENCES shelves(id) ON DELETE CASCADE,
    reference_id  INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    citation_key  TEXT NOT NULL,
    PRIMARY KEY (shelf_id, reference_id),
    UNIQUE (shelf_id, citation_key)
);
PRAGMA user_version = 1;
"#;

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
    /// SQLite operation failed.
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
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
        create_directory(root)?;
        create_directory(&root.join(FILES_DIR))?;
        let library = Self { root: root.into() };
        let mut connection = library.connection()?;
        initialize_schema(&mut connection)?;
        ensure_main(&mut connection)?;
        Ok(library)
    }

    /// Open an existing database without creating it.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root.into()));
        }
        let library = Self { root: root.into() };
        validate_database_file(&library.path())?;
        let mut connection = library.connection()?;
        validate_schema(&connection)?;
        ensure_main(&mut connection)?;
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
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT name FROM shelves ORDER BY name")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|name| ShelfName::try_from(name?))
            .collect()
    }

    /// Create a shelf, returning whether it was new.
    pub fn create_shelf(&self, name: &ShelfName) -> Result<bool, LibraryError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT name FROM shelves WHERE name = ?1 COLLATE NOCASE",
                [name.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing) = &existing
            && existing != name.as_str()
        {
            return Err(LibraryError::ShelfAlias {
                requested: name.to_string(),
                existing: existing.clone(),
            });
        }
        if existing.is_none() {
            transaction.execute("INSERT INTO shelves(name) VALUES (?1)", [name.as_str()])?;
        }
        transaction.commit()?;
        Ok(existing.is_none())
    }

    /// Confirm that a shelf exists.
    pub fn validate_shelf(&self, name: &ShelfName) -> Result<(), LibraryError> {
        let connection = self.connection()?;
        shelf_id(&connection, name).map(|_| ())
    }

    /// Return all entries in a shelf ordered by citation key.
    pub fn entries(&self, shelf: &ShelfName) -> Result<Vec<ShelfEntry>, LibraryError> {
        let connection = self.connection()?;
        let shelf_id = shelf_id(&connection, shelf)?;
        entries_for_shelf(&connection, shelf_id)
    }

    /// Return semantic references in shelf-key order.
    pub fn projected(&self, shelf: &ShelfName) -> Result<Vec<ProjectedReference>, LibraryError> {
        Ok(self
            .entries(shelf)?
            .into_iter()
            .map(|entry| ProjectedReference {
                key: entry.key,
                reference: entry.reference,
            })
            .collect())
    }

    /// Find within one shelf by exact key, INSPIRE ID, DOI, or arXiv ID.
    pub fn find(
        &self,
        shelf: &ShelfName,
        selector: &str,
    ) -> Result<Option<ProjectedReference>, LibraryError> {
        let connection = self.connection()?;
        let shelf_id = shelf_id(&connection, shelf)?;
        let exact = connection
            .query_row(
                "SELECT reference_id, citation_key FROM shelf_references
                 WHERE shelf_id = ?1 AND citation_key = ?2",
                params![shelf_id, selector],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((id, key)) = exact {
            return Ok(Some(ProjectedReference {
                key,
                reference: load_reference(&connection, id)?,
            }));
        }
        let locator = match selector.parse::<Locator>() {
            Ok(locator) => locator,
            Err(_) => return Ok(None),
        };
        let id = match locator {
            Locator::Inspire(value) => inspire_record_in_shelf(&connection, shelf_id, value)?,
            Locator::Doi(value) => {
                identity_in_shelf(&connection, shelf_id, "doi", &normalize_doi(&value))?
            }
            Locator::Arxiv(value) => {
                identity_in_shelf(&connection, shelf_id, "arxiv", &normalize_arxiv(&value))?
            }
        };
        let Some(id) = id else {
            return Ok(None);
        };
        let key = connection.query_row(
            "SELECT citation_key FROM shelf_references
             WHERE shelf_id = ?1 AND reference_id = ?2",
            params![shelf_id, id],
            |row| row.get(0),
        )?;
        Ok(Some(ProjectedReference {
            key,
            reference: load_reference(&connection, id)?,
        }))
    }

    /// Add a validated batch atomically.
    pub fn add_batch(
        &self,
        shelf: &ShelfName,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<Vec<AddOutcome>, LibraryError> {
        for item in &pending {
            validate_key(item.key.as_str())?;
            item.source
                .project_checked()
                .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let shelf_id = shelf_id(&transaction, shelf)?;
        let mut outcomes = Vec::with_capacity(pending.len());
        for item in pending {
            outcomes.push(add_one(&transaction, shelf_id, item, policy)?);
        }
        transaction.commit()?;
        Ok(outcomes)
    }

    /// Add every valid item in one transaction and report isolatable failures.
    pub fn add_batch_skipping_errors(
        &self,
        shelf: &ShelfName,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<(Vec<AddOutcome>, Vec<SkippedReference>), LibraryError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let shelf_id = shelf_id(&transaction, shelf)?;
        let mut outcomes = Vec::new();
        let mut skipped = Vec::new();
        for item in pending {
            let key = item.key.as_str().to_owned();
            let validation = validate_key(item.key.as_str())
                .map_err(LibraryError::from)
                .and_then(|_| {
                    item.source
                        .project_checked()
                        .map_err(|error| LibraryError::InvalidSource(error.to_string()))
                        .map(|_| ())
                });
            if let Err(error) = validation {
                skipped.push(SkippedReference {
                    key,
                    message: error.to_string(),
                });
                continue;
            }
            transaction.execute_batch("SAVEPOINT import_entry")?;
            match add_one(&transaction, shelf_id, item, policy) {
                Ok(outcome) => {
                    transaction.execute_batch("RELEASE import_entry")?;
                    outcomes.push(outcome);
                }
                Err(error) => {
                    transaction.execute_batch("ROLLBACK TO import_entry; RELEASE import_entry")?;
                    skipped.push(SkippedReference {
                        key,
                        message: error.to_string(),
                    });
                }
            }
        }
        transaction.commit()?;
        Ok((outcomes, skipped))
    }

    /// Remove all selected memberships atomically and delete resulting orphans.
    pub fn remove_batch(
        &self,
        shelf: &ShelfName,
        selectors: &[String],
    ) -> Result<Vec<ProjectedReference>, LibraryError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let shelf_id = shelf_id(&transaction, shelf)?;
        let mut selected = Vec::new();
        for selector in selectors {
            let item = find_in_connection(&transaction, shelf_id, selector)?
                .ok_or_else(|| LibraryError::ReferenceNotFound(selector.clone()))?;
            if !selected
                .iter()
                .any(|(id, _): &(i64, ProjectedReference)| id == &item.0)
            {
                selected.push(item);
            }
        }
        for (id, _) in &selected {
            transaction.execute(
                "DELETE FROM shelf_references WHERE shelf_id = ?1 AND reference_id = ?2",
                params![shelf_id, id],
            )?;
            delete_if_orphan(&transaction, *id)?;
        }
        transaction.commit()?;
        Ok(selected.into_iter().map(|(_, item)| item).collect())
    }

    /// Select unique references for a shelf or the complete library.
    pub fn sync_candidates(
        &self,
        shelf: Option<&ShelfName>,
    ) -> Result<Vec<SyncCandidate>, LibraryError> {
        let connection = self.connection()?;
        let ids = if let Some(shelf) = shelf {
            let shelf_id = shelf_id(&connection, shelf)?;
            let mut statement = connection.prepare(
                "SELECT reference_id FROM shelf_references
                 WHERE shelf_id = ?1 ORDER BY reference_id",
            )?;
            statement
                .query_map([shelf_id], |row| row.get(0))?
                .collect::<Result<Vec<i64>, _>>()?
        } else {
            let mut statement =
                connection.prepare("SELECT id FROM bibliography_references ORDER BY id")?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<i64>, _>>()?
        };
        ids.into_iter()
            .map(|id| {
                Ok(SyncCandidate {
                    id,
                    source: load_source(&connection, id)?,
                })
            })
            .collect()
    }

    /// Apply a complete INSPIRE result set atomically.
    pub fn apply_sync(&self, updates: Vec<SyncUpdate>) -> Result<usize, LibraryError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut changed = 0;
        for update in updates {
            let current = load_source(&transaction, update.id)?;
            if current != update.expected {
                return Err(LibraryError::ConcurrentChange);
            }
            if current == update.replacement {
                continue;
            }
            ensure_source_identities_available(&transaction, &update.replacement, Some(update.id))?;
            replace_reference(&transaction, update.id, &update.replacement)?;
            changed += 1;
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub(crate) fn connection(&self) -> Result<Connection, LibraryError> {
        if self.path().exists() {
            validate_database_file(&self.path())?;
        }
        let connection = Connection::open(self.path())?;
        configure(&connection)?;
        Ok(connection)
    }
}

fn create_directory(path: &Path) -> Result<(), LibraryError> {
    fs::create_dir_all(path).map_err(|source| LibraryError::Filesystem {
        path: path.into(),
        source,
    })
}

fn validate_database_file(path: &Path) -> Result<(), LibraryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| LibraryError::Filesystem {
        path: path.into(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LibraryError::InvalidDatabasePath(path.into()));
    }
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), LibraryError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;",
    )?;
    Ok(())
}

fn initialize_schema(connection: &mut Connection) -> Result<(), LibraryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(CREATE_SCHEMA)?;
        transaction.commit()?;
    } else if version != DATABASE_SCHEMA {
        return Err(LibraryError::UnsupportedSchema { found: version });
    }
    Ok(())
}

fn validate_schema(connection: &Connection) -> Result<(), LibraryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != DATABASE_SCHEMA {
        return Err(LibraryError::UnsupportedSchema { found: version });
    }
    Ok(())
}

fn ensure_main(connection: &mut Connection) -> Result<(), LibraryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO shelves(name) VALUES (?1)",
        [DEFAULT_SHELF],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn shelf_id(connection: &Connection, shelf: &ShelfName) -> Result<i64, LibraryError> {
    connection
        .query_row(
            "SELECT id FROM shelves WHERE name = ?1",
            [shelf.as_str()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::UnknownShelf(shelf.to_string()))
}

fn identity_in_shelf(
    connection: &Connection,
    shelf_id: i64,
    kind: &str,
    value: &str,
) -> Result<Option<i64>, LibraryError> {
    Ok(connection
        .query_row(
            "SELECT sr.reference_id FROM shelf_references sr
             JOIN identities i ON i.reference_id = sr.reference_id
             WHERE sr.shelf_id = ?1 AND i.kind = ?2 AND i.value = ?3",
            params![shelf_id, kind, value],
            |row| row.get(0),
        )
        .optional()?)
}

fn inspire_record_in_shelf(
    connection: &Connection,
    shelf_id: i64,
    record_id: u64,
) -> Result<Option<i64>, LibraryError> {
    let Ok(record_id) = i64::try_from(record_id) else {
        return Ok(None);
    };
    Ok(connection
        .query_row(
            "SELECT sr.reference_id FROM shelf_references sr
             JOIN inspire_records ir ON ir.reference_id = sr.reference_id
             WHERE sr.shelf_id = ?1 AND ir.record_id = ?2",
            params![shelf_id, record_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn source_identities(
    source: &SourceSnapshot,
) -> Result<Vec<(String, String)>, LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    let mut identities = reference
        .identifiers
        .arxiv
        .into_iter()
        .map(|value| ("arxiv".into(), normalize_arxiv(&value)))
        .chain(
            reference
                .identifiers
                .dois
                .into_iter()
                .map(|value| ("doi".into(), normalize_doi(&value))),
        )
        .collect::<Vec<_>>();
    identities.sort();
    identities.dedup();
    Ok(identities)
}

fn sql_record_id(record_id: u64) -> Result<i64, LibraryError> {
    if record_id == 0 {
        return Err(LibraryError::InvalidSource(
            "INSPIRE record id is zero".into(),
        ));
    }
    i64::try_from(record_id).map_err(|_| {
        LibraryError::InvalidSource(format!(
            "INSPIRE record id {record_id} exceeds SQLite's integer range"
        ))
    })
}

fn source_record_id(record_id: i64) -> Result<u64, LibraryError> {
    u64::try_from(record_id)
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            LibraryError::InvalidSource(format!(
                "stored INSPIRE record id {record_id} is not positive"
            ))
        })
}

fn matching_reference_ids(
    connection: &Connection,
    source: &SourceSnapshot,
    exclude: Option<i64>,
) -> Result<BTreeSet<i64>, LibraryError> {
    let mut matches = BTreeSet::new();
    for (kind, value) in source_identities(source)? {
        if let Some(id) = connection
            .query_row(
                "SELECT reference_id FROM identities WHERE kind = ?1 AND value = ?2",
                params![kind, value],
                |row| row.get(0),
            )
            .optional()?
            && Some(id) != exclude
        {
            matches.insert(id);
        }
    }
    if let Some(entry) = source.inspire_entry()
        && let Some(id) = connection
            .query_row(
                "SELECT reference_id FROM inspire_records WHERE record_id = ?1",
                [sql_record_id(entry.record_id)?],
                |row| row.get(0),
            )
            .optional()?
        && Some(id) != exclude
    {
        matches.insert(id);
    }
    Ok(matches)
}

fn ensure_source_identities_available(
    connection: &Connection,
    source: &SourceSnapshot,
    exclude: Option<i64>,
) -> Result<(), LibraryError> {
    let matches = matching_reference_ids(connection, source, exclude)?;
    if matches.is_empty() {
        Ok(())
    } else {
        Err(LibraryError::IdentityConflict)
    }
}

pub(crate) fn insert_reference(
    transaction: &Transaction<'_>,
    source: &SourceSnapshot,
) -> Result<i64, LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    transaction.execute(
        "INSERT INTO bibliography_references(source_kind, bibtex, title, year)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            if source.inspire_entry().is_some() {
                "inspire"
            } else {
                "import"
            },
            source.raw_bibtex(),
            reference.title,
            reference.year
        ],
    )?;
    let id = transaction.last_insert_rowid();
    insert_details(transaction, id, source, &reference)?;
    Ok(id)
}

fn insert_details(
    transaction: &Transaction<'_>,
    id: i64,
    source: &SourceSnapshot,
    reference: &Reference,
) -> Result<(), LibraryError> {
    for (kind, values) in [
        ("author", &reference.authors),
        ("collaboration", &reference.collaborations),
    ] {
        for (position, name) in values.iter().enumerate() {
            transaction.execute(
                "INSERT INTO contributors(reference_id, kind, position, name)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, kind, position as i64, name],
            )?;
        }
    }
    let canonical = source.inspire_entry().map(|entry| &entry.identifiers);
    for (kind, value) in source_identities(source)? {
        let is_canonical = canonical.is_some_and(|ids| match kind.as_str() {
            "arxiv" => ids.arxiv.as_deref() == Some(value.as_str()),
            "doi" => ids.doi.as_deref() == Some(value.as_str()),
            _ => false,
        });
        transaction.execute(
            "INSERT INTO identities(reference_id, kind, value, canonical)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, kind, value, is_canonical],
        )?;
    }
    if let Some(entry) = source.inspire_entry() {
        transaction.execute(
            "INSERT INTO inspire_records(reference_id, record_id, updated)
             VALUES (?1, ?2, ?3)",
            params![id, sql_record_id(entry.record_id)?, entry.updated],
        )?;
    }
    Ok(())
}

fn replace_reference(
    transaction: &Transaction<'_>,
    id: i64,
    source: &SourceSnapshot,
) -> Result<(), LibraryError> {
    let reference = source
        .project_checked()
        .map_err(|error| LibraryError::InvalidSource(error.to_string()))?;
    transaction.execute("DELETE FROM contributors WHERE reference_id = ?1", [id])?;
    transaction.execute("DELETE FROM identities WHERE reference_id = ?1", [id])?;
    transaction.execute("DELETE FROM inspire_records WHERE reference_id = ?1", [id])?;
    transaction.execute(
        "UPDATE bibliography_references
         SET source_kind = ?2, bibtex = ?3, title = ?4, year = ?5 WHERE id = ?1",
        params![
            id,
            if source.inspire_entry().is_some() {
                "inspire"
            } else {
                "import"
            },
            source.raw_bibtex(),
            reference.title,
            reference.year
        ],
    )?;
    insert_details(transaction, id, source, &reference)
}

fn add_one(
    transaction: &Transaction<'_>,
    shelf_id: i64,
    item: PendingReference,
    policy: ConflictPolicy,
) -> Result<AddOutcome, LibraryError> {
    let matching = matching_reference_ids(transaction, &item.source, None)?;
    if matching.len() > 1 {
        return Err(LibraryError::IdentityConflict);
    }
    let existing_global = matching.first().copied();
    let inserted = existing_global.is_none();
    let reference_id = match existing_global {
        Some(id) => id,
        None => insert_reference(transaction, &item.source)?,
    };
    let existing_key = transaction
        .query_row(
            "SELECT citation_key FROM shelf_references
             WHERE shelf_id = ?1 AND reference_id = ?2",
            params![shelf_id, reference_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let suggested = matches!(&item.key, KeyRequest::Suggested(_));
    let requested = item.key.into_string();
    let key_owner = transaction
        .query_row(
            "SELECT reference_id FROM shelf_references
             WHERE shelf_id = ?1 AND citation_key = ?2",
            params![shelf_id, requested],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    if let Some(existing_key) = existing_key {
        if suggested {
            promote_if_needed(transaction, reference_id, &item.source)?;
            return Ok(AddOutcome::Existing(existing_key));
        }
        if existing_key == requested {
            promote_if_needed(transaction, reference_id, &item.source)?;
            return Ok(AddOutcome::Existing(existing_key));
        }
        if matches!(policy, ConflictPolicy::Skip) {
            if inserted {
                delete_if_orphan(transaction, reference_id)?;
            }
            return Ok(AddOutcome::Skipped {
                key: requested,
                conflicting: existing_key,
            });
        }
        let mut replaced = vec![existing_key.clone()];
        if let Some(owner) = key_owner
            && owner != reference_id
        {
            transaction.execute(
                "DELETE FROM shelf_references
                 WHERE shelf_id = ?1 AND reference_id = ?2",
                params![shelf_id, owner],
            )?;
            delete_if_orphan(transaction, owner)?;
            replaced.push(requested.clone());
        }
        transaction.execute(
            "UPDATE shelf_references SET citation_key = ?3
             WHERE shelf_id = ?1 AND reference_id = ?2",
            params![shelf_id, reference_id, requested],
        )?;
        promote_if_needed(transaction, reference_id, &item.source)?;
        return Ok(AddOutcome::Overwritten {
            key: requested,
            replaced,
        });
    }

    if let Some(owner) = key_owner {
        if matches!(policy, ConflictPolicy::Skip) {
            if inserted {
                delete_if_orphan(transaction, reference_id)?;
            }
            return Ok(AddOutcome::Skipped {
                key: requested.clone(),
                conflicting: requested,
            });
        }
        transaction.execute(
            "DELETE FROM shelf_references WHERE shelf_id = ?1 AND reference_id = ?2",
            params![shelf_id, owner],
        )?;
        delete_if_orphan(transaction, owner)?;
        transaction.execute(
            "INSERT INTO shelf_references(shelf_id, reference_id, citation_key)
             VALUES (?1, ?2, ?3)",
            params![shelf_id, reference_id, requested],
        )?;
        promote_if_needed(transaction, reference_id, &item.source)?;
        return Ok(AddOutcome::Overwritten {
            key: requested.clone(),
            replaced: vec![requested],
        });
    }

    transaction.execute(
        "INSERT INTO shelf_references(shelf_id, reference_id, citation_key)
         VALUES (?1, ?2, ?3)",
        params![shelf_id, reference_id, requested],
    )?;
    promote_if_needed(transaction, reference_id, &item.source)?;
    Ok(AddOutcome::Added(requested))
}

fn promote_if_needed(
    transaction: &Transaction<'_>,
    id: i64,
    incoming: &SourceSnapshot,
) -> Result<(), LibraryError> {
    if incoming.inspire_entry().is_some() && load_source(transaction, id)?.inspire_entry().is_none()
    {
        ensure_source_identities_available(transaction, incoming, Some(id))?;
        replace_reference(transaction, id, incoming)?;
    }
    Ok(())
}

fn delete_if_orphan(transaction: &Transaction<'_>, id: i64) -> Result<(), LibraryError> {
    transaction.execute(
        "DELETE FROM bibliography_references
         WHERE id = ?1 AND NOT EXISTS (
             SELECT 1 FROM shelf_references WHERE reference_id = ?1
         )",
        [id],
    )?;
    Ok(())
}

fn entries_for_shelf(
    connection: &Connection,
    shelf_id: i64,
) -> Result<Vec<ShelfEntry>, LibraryError> {
    let mut statement = connection.prepare(
        "SELECT reference_id, citation_key FROM shelf_references
         WHERE shelf_id = ?1 ORDER BY citation_key",
    )?;
    let rows = statement
        .query_map([shelf_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, key)| {
            Ok(ShelfEntry {
                id,
                key,
                source: load_source(connection, id)?,
                reference: load_reference(connection, id)?,
            })
        })
        .collect()
}

fn load_source(connection: &Connection, id: i64) -> Result<SourceSnapshot, LibraryError> {
    let (kind, bibtex) = connection.query_row(
        "SELECT source_kind, bibtex FROM bibliography_references WHERE id = ?1",
        [id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match kind.as_str() {
        "import" => Ok(SourceSnapshot::Import(BibtexSnapshot::new(bibtex)?)),
        "inspire" => {
            let (record_id, updated) = connection.query_row(
                "SELECT record_id, updated FROM inspire_records
                 WHERE reference_id = ?1",
                [id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )?;
            let record_id = source_record_id(record_id)?;
            let mut statement = connection.prepare(
                "SELECT kind, value FROM identities
                 WHERE reference_id = ?1 AND canonical = 1",
            )?;
            let mut identifiers = HepIdentifiers::default();
            for row in statement.query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (kind, value) = row?;
                match kind.as_str() {
                    "arxiv" => identifiers.arxiv = Some(value),
                    "doi" => identifiers.doi = Some(value),
                    _ => unreachable!("database CHECK limits identity kind"),
                }
            }
            Ok(SourceSnapshot::Inspire(InspireEntry {
                record_id,
                updated,
                bibtex,
                identifiers,
            }))
        }
        _ => Err(LibraryError::InvalidSource(format!(
            "unknown source kind `{kind}`"
        ))),
    }
}

fn load_reference(connection: &Connection, id: i64) -> Result<Reference, LibraryError> {
    let (title, year) = connection.query_row(
        "SELECT title, year FROM bibliography_references WHERE id = ?1",
        [id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i32>>(1)?)),
    )?;
    let mut contributors = connection.prepare(
        "SELECT kind, name FROM contributors
         WHERE reference_id = ?1 ORDER BY kind, position",
    )?;
    let mut authors = Vec::new();
    let mut collaborations = Vec::new();
    for row in contributors.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (kind, name) = row?;
        if kind == "author" {
            authors.push(name);
        } else {
            collaborations.push(name);
        }
    }
    let mut identity_statement = connection.prepare(
        "SELECT kind, value FROM identities
         WHERE reference_id = ?1 ORDER BY kind, value",
    )?;
    let mut dois = Vec::new();
    let mut arxiv = Vec::new();
    for row in identity_statement.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (kind, value) = row?;
        if kind == "doi" {
            dois.push(value);
        } else {
            arxiv.push(value);
        }
    }
    Ok(Reference {
        title,
        authors,
        collaborations,
        year,
        identifiers: Identifiers { dois, arxiv },
    })
}

fn find_in_connection(
    connection: &Connection,
    shelf_id: i64,
    selector: &str,
) -> Result<Option<(i64, ProjectedReference)>, LibraryError> {
    let exact = connection
        .query_row(
            "SELECT reference_id, citation_key FROM shelf_references
             WHERE shelf_id = ?1 AND citation_key = ?2",
            params![shelf_id, selector],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let matched = if exact.is_some() {
        exact
    } else {
        let locator = match selector.parse::<Locator>() {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let id = match locator {
            Locator::Inspire(value) => inspire_record_in_shelf(connection, shelf_id, value)?,
            Locator::Doi(value) => {
                identity_in_shelf(connection, shelf_id, "doi", &normalize_doi(&value))?
            }
            Locator::Arxiv(value) => {
                identity_in_shelf(connection, shelf_id, "arxiv", &normalize_arxiv(&value))?
            }
        };
        match id {
            Some(id) => Some((
                id,
                connection.query_row(
                    "SELECT citation_key FROM shelf_references
                     WHERE shelf_id = ?1 AND reference_id = ?2",
                    params![shelf_id, id],
                    |row| row.get(0),
                )?,
            )),
            None => None,
        }
    };
    let Some((id, key)) = matched else {
        return Ok(None);
    };
    Ok(Some((
        id,
        ProjectedReference {
            key,
            reference: load_reference(connection, id)?,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyRequest;

    fn imported(key: &str, title: &str, extra: &str) -> PendingReference {
        PendingReference {
            key: KeyRequest::Exact(key.into()),
            source: SourceSnapshot::Import(
                BibtexSnapshot::new(format!(
                    "@article{{Upstream,\n  title = {{{title}}}{extra}\n}}"
                ))
                .unwrap(),
            ),
        }
    }

    #[test]
    fn one_reference_can_have_different_keys_in_two_shelves() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let other = ShelfName::try_from("paper").unwrap();
        library.create_shelf(&other).unwrap();
        let main = ShelfName::default_shelf();
        library
            .add_batch(
                &main,
                vec![imported("One", "Shared", ",\n  doi = {10.1/shared}")],
                ConflictPolicy::Skip,
            )
            .unwrap();
        library
            .add_batch(
                &other,
                vec![imported("Two", "Different raw", ",\n  doi = {10.1/shared}")],
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(library.entries(&main).unwrap()[0].key, "One");
        assert_eq!(library.entries(&other).unwrap()[0].key, "Two");
        assert_eq!(
            library.entries(&main).unwrap()[0].source,
            library.entries(&other).unwrap()[0].source
        );
    }

    #[test]
    fn overwrite_is_local_and_removal_collects_orphans() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        library
            .add_batch(
                &main,
                vec![imported("K", "First", "")],
                ConflictPolicy::Skip,
            )
            .unwrap();
        library
            .add_batch(
                &main,
                vec![imported("K", "Second", "")],
                ConflictPolicy::Overwrite,
            )
            .unwrap();
        assert_eq!(library.entries(&main).unwrap()[0].reference.title, "Second");
        library.remove_batch(&main, &["K".into()]).unwrap();
        let connection = library.connection().unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM bibliography_references", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn lenient_batch_uses_savepoints_and_commits_valid_entries() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        let (outcomes, skipped) = library
            .add_batch_skipping_errors(
                &main,
                vec![imported("bad key", "Bad", ""), imported("Good", "Good", "")],
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(outcomes, [AddOutcome::Added("Good".into())]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(library.entries(&main).unwrap().len(), 1);
    }

    #[test]
    fn identities_cannot_implicitly_merge_two_global_references() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        library
            .add_batch(
                &main,
                vec![
                    imported("Doi", "DOI", ",\n  doi = {10.1/one}"),
                    imported(
                        "Arxiv",
                        "arXiv",
                        ",\n  eprint = {2001.00001},\n  archivePrefix = {arXiv}",
                    ),
                ],
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(
            library.find(&main, "doi:10.1/ONE").unwrap().unwrap().key,
            "Doi"
        );
        assert_eq!(
            library
                .find(&main, "arxiv:2001.00001v2")
                .unwrap()
                .unwrap()
                .key,
            "Arxiv"
        );
        let error = library
            .add_batch(
                &main,
                vec![imported(
                    "Bridge",
                    "Bridge",
                    ",\n  doi = {10.1/one},\n  eprint = {2001.00001},\n  archivePrefix = {arXiv}",
                )],
                ConflictPolicy::Overwrite,
            )
            .unwrap_err();
        assert!(matches!(error, LibraryError::IdentityConflict));
        assert_eq!(library.entries(&main).unwrap().len(), 2);
    }

    #[test]
    fn inspire_records_use_integer_ids_and_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        let source = SourceSnapshot::Inspire(InspireEntry {
            record_id: 42,
            updated: "2026-01-01".into(),
            bibtex: "@article{Provider,\n title={Managed},\n doi={10.1/managed}\n}".into(),
            identifiers: HepIdentifiers::new(None, Some("10.1/managed".into())),
        });
        library
            .add_batch(
                &main,
                vec![PendingReference {
                    key: KeyRequest::Suggested("Managed".into()),
                    source: source.clone(),
                }],
                ConflictPolicy::Skip,
            )
            .unwrap();

        let connection = library.connection().unwrap();
        let (record_id, storage_class): (i64, String) = connection
            .query_row(
                "SELECT record_id, typeof(record_id) FROM inspire_records",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(record_id, 42);
        assert_eq!(storage_class, "integer");
        assert_eq!(library.entries(&main).unwrap()[0].source, source);
        assert_eq!(
            library.find(&main, "inspire:42").unwrap().unwrap().key,
            "Managed"
        );
    }

    #[test]
    fn inspire_record_ids_outside_sqlite_range_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let source = SourceSnapshot::Inspire(InspireEntry {
            record_id: u64::MAX,
            updated: "2026-01-01".into(),
            bibtex: "@article{Provider,\n title={Managed}\n}".into(),
            identifiers: HepIdentifiers::default(),
        });
        let error = library
            .add_batch(
                &ShelfName::default_shelf(),
                vec![PendingReference {
                    key: KeyRequest::Suggested("Managed".into()),
                    source,
                }],
                ConflictPolicy::Skip,
            )
            .unwrap_err();
        assert!(
            matches!(error, LibraryError::InvalidSource(message) if message.contains("SQLite"))
        );
    }

    #[test]
    fn stale_sync_results_are_rejected_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let main = ShelfName::default_shelf();
        library
            .add_batch(
                &main,
                vec![imported("Local", "Imported", ",\n  doi = {10.1/shared}")],
                ConflictPolicy::Skip,
            )
            .unwrap();
        let candidate = library.sync_candidates(None).unwrap().pop().unwrap();
        let managed = SourceSnapshot::Inspire(InspireEntry {
            record_id: 42,
            updated: "2026-01-01".into(),
            bibtex: "@article{Provider,\n title={Managed},\n doi={10.1/shared}\n}".into(),
            identifiers: HepIdentifiers::new(None, Some("10.1/shared".into())),
        });
        library
            .add_batch(
                &main,
                vec![PendingReference {
                    key: KeyRequest::Suggested("Local".into()),
                    source: managed.clone(),
                }],
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(
            library.find(&main, "inspire:42").unwrap().unwrap().key,
            "Local"
        );
        let error = library
            .apply_sync(vec![SyncUpdate {
                id: candidate.id,
                expected: candidate.source,
                replacement: managed,
            }])
            .unwrap_err();
        assert!(matches!(error, LibraryError::ConcurrentChange));
        assert_eq!(
            library.entries(&main).unwrap()[0].reference.title,
            "Managed"
        );
    }

    #[test]
    fn connections_enable_required_sqlite_guarantees() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        let connection = library.connection().unwrap();
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        let journal: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let synchronous: i64 = connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        let busy_timeout: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal, "wal");
        assert_eq!(synchronous, 2);
        assert_eq!(busy_timeout, 5000);
    }

    #[test]
    fn shelf_case_aliases_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open_or_create(root.path().canonicalize().unwrap()).unwrap();
        library
            .create_shelf(&ShelfName::try_from("Paper").unwrap())
            .unwrap();
        let error = library
            .create_shelf(&ShelfName::try_from("paper").unwrap())
            .unwrap_err();
        assert!(matches!(error, LibraryError::ShelfAlias { .. }));
    }
}
