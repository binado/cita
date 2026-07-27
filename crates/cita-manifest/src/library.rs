//! Schema-1 global library registry and shelf locking.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{MANIFEST_FILE, Manifest, SCHEMA};

/// Name of the global library registry.
pub const LIBRARY_FILE: &str = "library.toml";
/// Fixed default shelf name.
pub const DEFAULT_SHELF: &str = "main";

/// Directory holding every advisory lock file.
const LOCKS_DIR: &str = "locks";
/// Directory holding every shelf directory.
const SHELVES_DIR: &str = "shelves";
/// Directory holding the shared document cache.
const FILES_DIR: &str = "files";
/// Lock file guarding the registry itself.
///
/// Shelf locks are `shelf-<name>.lock`, and a shelf name can never begin with
/// `-`, so the two namespaces cannot collide and no shelf name is reserved.
const REGISTRY_LOCK: &str = "registry.lock";

/// A validated shelf name.
///
/// The leading-alphanumeric rule is what makes `shelves/<name>` traversal-safe:
/// `.`, `..`, absolute paths, and any name containing a path separator are all
/// rejected, so every path built from a `ShelfName` stays inside the store.
/// Construct one with `TryFrom`/`parse`; there is no way to build an unchecked
/// value, so holding a `ShelfName` is proof the name was validated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShelfName(String);

impl ShelfName {
    /// Return the fixed default shelf name.
    pub fn default_shelf() -> Self {
        Self(DEFAULT_SHELF.to_owned())
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
        Ok(Self(value.to_owned()))
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

impl std::ops::Deref for ShelfName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ShelfName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lets a `BTreeSet<ShelfName>` be probed with a plain `&str`.
impl std::borrow::Borrow<str> for ShelfName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShelfName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A loaded schema-1 global library registry.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    shelves: BTreeSet<ShelfName>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryData {
    schema: u32,
    default: String,
    shelves: BTreeSet<String>,
}

/// An exclusive advisory lock for one shelf, released when dropped.
///
/// "Advisory" means only cooperating cita processes are excluded; an editor or
/// script writing `shelf.toml` directly is not blocked by this lock.
///
/// Locking uses `flock`, which is per open-file-description and therefore **not
/// reentrant**: acquiring the same lock twice within one process deadlocks. No
/// current code path nests acquisitions, but `ShelfLock` is `Send` and is held
/// across `.await` points, so a concurrent batch loop would hang rather than error.
#[derive(Debug)]
#[must_use = "the shelf lock is released when dropped; bind it for the whole critical section"]
pub struct ShelfLock {
    name: ShelfName,
    manifest_path: PathBuf,
    _file: File,
}

impl ShelfLock {
    /// Return the shelf this lock covers.
    pub fn name(&self) -> &ShelfName {
        &self.name
    }

    /// Load the manifest this lock protects.
    ///
    /// Going through the lock is what pairs a mutation with the lock covering it,
    /// so "lock one shelf, mutate another" cannot be written by accident.
    pub fn manifest(&self) -> Result<Manifest, crate::Error> {
        Manifest::load(&self.manifest_path)
    }
}

/// An exclusive advisory lock over the registry, released when dropped.
///
/// Carries the same `flock` non-reentrancy caveat as [`ShelfLock`].
#[derive(Debug)]
#[must_use = "the registry lock is released when dropped; bind it for the whole critical section"]
pub struct LibraryLock {
    _file: File,
}

/// Error produced by global library loading, validation, locking, or persistence.
///
/// Every variant whose message ends in a path deliberately omits its underlying
/// cause: the cause is reachable through [`std::error::Error::source`], and
/// repeating it in `Display` makes `anyhow`'s `{:#}` print it twice.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// The global library root is not absolute.
    #[error("cita home must be an absolute path: {0}")]
    RelativeRoot(PathBuf),
    /// The home directory used to derive the default root is not absolute.
    #[error("home directory must be an absolute path: {0}; set CITA_HOME to override")]
    RelativeHome(PathBuf),
    /// The explicit global library root is empty.
    #[error("CITA_HOME cannot be empty")]
    EmptyRoot,
    /// No operating-system home directory is available.
    #[error("could not determine the home directory; set CITA_HOME to an absolute path")]
    HomeUnavailable,
    /// A managed directory could not be created.
    #[error("could not create {path}")]
    CreateDirectory {
        /// Directory that could not be created.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A managed directory is not a direct directory.
    #[error("invalid managed directory {path}: {message}")]
    InvalidDirectory {
        /// Invalid directory.
        path: PathBuf,
        /// Validation diagnostic.
        message: String,
    },
    /// A managed file is not a direct regular file.
    #[error("invalid managed file {path}: {message}")]
    InvalidFile {
        /// Invalid file.
        path: PathBuf,
        /// Validation diagnostic.
        message: String,
    },
    /// A managed path could not be read.
    ///
    /// Covers directories as well as files; a registered shelf whose directory is
    /// gone reports [`LibraryError::ShelfMissing`] instead, so a damaged shelf is
    /// distinguishable from a genuine I/O failure.
    #[error("could not read {path}")]
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A registry failed syntax or semantic validation.
    #[error("invalid library {path}: {message}")]
    Invalid {
        /// Invalid registry path.
        path: PathBuf,
        /// Validation diagnostic.
        message: String,
    },
    /// The registry uses an unsupported schema.
    #[error(
        "unsupported library.toml schema {found}; this version supports schema 1 and provides no legacy migration"
    )]
    UnsupportedSchema {
        /// Schema value found in the file.
        found: i64,
    },
    /// A shelf name is malformed.
    #[error("invalid shelf name `{0}`; names must match [A-Za-z0-9][A-Za-z0-9._-]*")]
    InvalidName(String),
    /// A requested shelf is not registered.
    #[error("unknown shelf `{0}`")]
    UnknownShelf(String),
    /// A shelf path is malformed or a symlink.
    #[error("invalid shelf `{name}`: {message}")]
    InvalidShelf {
        /// Shelf name.
        name: ShelfName,
        /// Validation diagnostic.
        message: String,
    },
    /// A registered shelf's directory or manifest is gone.
    ///
    /// The registry deliberately tolerates this so batch operations can report one
    /// damaged shelf and keep going; `cita init` repairs the default shelf.
    #[error("shelf `{name}` is registered but {path} is missing; run `cita init` to repair it")]
    ShelfMissing {
        /// Shelf name.
        name: ShelfName,
        /// Path that should exist.
        path: PathBuf,
    },
    /// A managed lock could not be acquired.
    #[error("could not lock {path}")]
    Lock {
        /// Lock file path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The registry could not be serialized.
    #[error("could not serialize library")]
    Serialize(#[from] toml::ser::Error),
    /// The registry could not be written atomically.
    #[error("could not write {path}")]
    Write {
        /// Registry path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A shelf manifest is invalid.
    #[error("invalid shelf `{name}`")]
    Manifest {
        /// Shelf name.
        name: ShelfName,
        /// Manifest error.
        #[source]
        source: crate::Error,
    },
}

/// Resolve the user-global library root from `CITA_HOME` or the home directory.
pub fn global_library_root() -> Result<PathBuf, LibraryError> {
    if let Some(configured) = env::var_os("CITA_HOME") {
        if configured.is_empty() {
            return Err(LibraryError::EmptyRoot);
        }
        let path = PathBuf::from(configured);
        if !path.is_absolute() {
            return Err(LibraryError::RelativeRoot(path));
        }
        return Ok(path);
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

fn locks_dir(root: &Path) -> PathBuf {
    root.join(LOCKS_DIR)
}

fn registry_lock_path(root: &Path) -> PathBuf {
    locks_dir(root).join(REGISTRY_LOCK)
}

fn shelf_lock_path(root: &Path, name: &ShelfName) -> PathBuf {
    locks_dir(root).join(format!("shelf-{name}.lock"))
}

impl Library {
    /// Open the global library, creating or repairing it as needed.
    ///
    /// Requires an absolute root and blocks on the registry lock. Missing managed
    /// directories and a missing default shelf are recreated on every call, so a
    /// partially deleted store heals instead of wedging. A *corrupt* shelf manifest
    /// is deliberately left alone: parsing it here would make one damaged shelf fail
    /// every command and destroy the tolerance [`Self::load`] promises. Shelves other
    /// than the default are never repaired; those surface from [`Self::shelf_manifest`].
    pub fn open_or_create(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root.to_path_buf()));
        }
        create_directory(root)?;
        ensure_managed_directory(&locks_dir(root))?;
        let _lock = lock_file(&registry_lock_path(root))?;
        ensure_managed_directory(&root.join(SHELVES_DIR))?;
        ensure_managed_directory(&root.join(FILES_DIR))?;
        let registry = root.join(LIBRARY_FILE);
        let library = match fs::symlink_metadata(&registry) {
            Ok(_) => {
                validate_managed_file(&registry)?;
                Self::load(root)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let library = Self {
                    root: root.to_path_buf(),
                    shelves: BTreeSet::from([ShelfName::default_shelf()]),
                };
                library.persist(&library.shelves)?;
                library
            }
            Err(source) => {
                return Err(LibraryError::Read {
                    path: registry,
                    source,
                });
            }
        };
        // Runs on both branches: without it, deleting `shelves/main` leaves a store
        // that every command fails on and no command can repair.
        repair_default_shelf(root)?;
        Ok(library)
    }

    /// Load an existing global library registry.
    ///
    /// Registry shape and shelf names are validated here. Per-shelf directory and
    /// `shelf.toml` checks happen in [`Self::shelf_manifest`], so one damaged shelf
    /// does not prevent loading the name list for batch operations. That tolerance
    /// does not extend to the managed `locks`, `shelves`, and `files` directories,
    /// which must all exist as direct directories.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root));
        }
        for directory in [LOCKS_DIR, SHELVES_DIR, FILES_DIR] {
            validate_managed_directory(&root.join(directory))?;
        }
        let path = root.join(LIBRARY_FILE);
        validate_managed_file(&path)?;
        let source = fs::read_to_string(&path).map_err(|source| LibraryError::Read {
            path: path.clone(),
            source,
        })?;
        let value: toml::Value =
            toml::from_str(&source).map_err(|error| LibraryError::Invalid {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let schema = value
            .get("schema")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| LibraryError::Invalid {
                path: path.clone(),
                message: "missing integer schema".into(),
            })?;
        if schema != i64::from(SCHEMA) {
            return Err(LibraryError::UnsupportedSchema { found: schema });
        }
        let data: LibraryData = toml::from_str(&source).map_err(|error| LibraryError::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if value
            .get("shelves")
            .and_then(toml::Value::as_array)
            .is_some_and(|values| values.len() != data.shelves.len())
        {
            return Err(LibraryError::Invalid {
                path,
                message: "registered shelf names must be unique".into(),
            });
        }
        if data.default != DEFAULT_SHELF {
            return Err(LibraryError::Invalid {
                path,
                message: format!("default shelf must be `{DEFAULT_SHELF}`"),
            });
        }
        if !data.shelves.contains(DEFAULT_SHELF) {
            return Err(LibraryError::Invalid {
                path,
                message: format!("registered shelves must contain `{DEFAULT_SHELF}`"),
            });
        }
        let shelves = data
            .shelves
            .iter()
            .map(|name| ShelfName::try_from(name.as_str()))
            .collect::<Result<_, _>>()?;
        Ok(Self { root, shelves })
    }

    /// Return the global store root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the registry path.
    pub fn path(&self) -> PathBuf {
        self.root.join(LIBRARY_FILE)
    }

    /// Return the shared document-cache root.
    pub fn files_root(&self) -> PathBuf {
        self.root.join(FILES_DIR)
    }

    /// Return registered shelf names in stable order, as of the last load.
    ///
    /// This is the registry snapshot taken when this handle was opened, not a live
    /// read; another process may have registered a shelf since.
    pub fn shelves(&self) -> &BTreeSet<ShelfName> {
        &self.shelves
    }

    /// Validate a registered shelf on disk and return its manifest path.
    ///
    /// This is where the per-shelf validation deferred by [`Self::load`] happens, so
    /// it touches the filesystem rather than merely computing a path. The directory
    /// and `shelf.toml` must both exist as direct, non-symlink entries. Fails with
    /// [`LibraryError::UnknownShelf`] when the name is not registered,
    /// [`LibraryError::ShelfMissing`] when it is registered but absent from disk, and
    /// [`LibraryError::InvalidShelf`] when either entry is the wrong kind.
    pub fn shelf_manifest(&self, name: &ShelfName) -> Result<PathBuf, LibraryError> {
        if !self.shelves.contains(name) {
            return Err(LibraryError::UnknownShelf(name.to_string()));
        }
        validate_shelf_manifest(&self.root, name)
    }

    /// Create and register an empty shelf, or validate an existing registration.
    ///
    /// Returns `true` when the shelf was newly created and `false` when it was
    /// already registered. Takes the registry lock and refreshes this handle's shelf
    /// set from disk, so a concurrent registration by another process is picked up.
    ///
    /// Validation runs *before* anything is written: a failure therefore leaves no
    /// half-created shelf directory behind, and a damaged unrelated shelf cannot
    /// block creating a new one.
    pub fn create_shelf(&mut self, name: &ShelfName) -> Result<bool, LibraryError> {
        let _lock = lock_file(&registry_lock_path(&self.root))?;
        let current = Self::load(&self.root)?;
        if current.shelves.contains(name) {
            validate_shelf_manifest(&self.root, name)?;
            self.shelves = current.shelves;
            return Ok(false);
        }
        validate_new_shelf_alias(&self.root, name, &current.shelves)?;
        ensure_shelf_manifest(&self.root, name)?;
        let mut candidate = current.shelves;
        candidate.insert(name.clone());
        self.persist(&candidate)?;
        self.shelves = candidate;
        Ok(true)
    }

    /// Acquire the exclusive advisory lock guarding one shelf's mutations.
    ///
    /// **Blocks** until the lock is available; there is no timeout. The shelf is
    /// validated first, so an unknown or damaged shelf fails before any locking.
    /// The lock is released when the returned [`ShelfLock`] is dropped.
    pub fn lock_shelf(&self, name: &ShelfName) -> Result<ShelfLock, LibraryError> {
        let manifest_path = self.shelf_manifest(name)?;
        let file = lock_file(&shelf_lock_path(&self.root, name))?;
        Ok(ShelfLock {
            name: name.clone(),
            manifest_path,
            _file: file,
        })
    }

    /// Acquire the exclusive advisory lock guarding the registry.
    ///
    /// **Blocks** until the lock is available; there is no timeout.
    pub fn lock_registry(&self) -> Result<LibraryLock, LibraryError> {
        Ok(LibraryLock {
            _file: lock_file(&registry_lock_path(&self.root))?,
        })
    }

    fn persist(&self, shelves: &BTreeSet<ShelfName>) -> Result<(), LibraryError> {
        let data = LibraryData {
            schema: SCHEMA,
            default: DEFAULT_SHELF.into(),
            shelves: shelves.iter().map(ShelfName::to_string).collect(),
        };
        let mut rendered = toml::to_string_pretty(&data)?;
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        atomic_write(&self.path(), rendered.as_bytes())
    }
}

fn shelf_directory(root: &Path, name: &ShelfName) -> PathBuf {
    root.join(SHELVES_DIR).join(name.as_str())
}

/// Create the shelf directory and manifest when absent, and fully validate them.
///
/// Unlike [`repair_default_shelf`], an existing manifest is parsed here, so a
/// corrupt shelf is rejected at creation time.
fn ensure_shelf_manifest(root: &Path, name: &ShelfName) -> Result<PathBuf, LibraryError> {
    let directory = shelf_directory(root, name);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(LibraryError::InvalidShelf {
                name: name.clone(),
                message: "shelf directory cannot be a symlink".into(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(LibraryError::InvalidShelf {
                name: name.clone(),
                message: format!("{} is not a directory", directory.display()),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_directory(&directory)?;
        }
        Err(source) => {
            return Err(LibraryError::Read {
                path: directory,
                source,
            });
        }
    }
    let path = directory.join(MANIFEST_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_managed_file(&path)?;
            Manifest::load(&path).map_err(|source| LibraryError::Manifest {
                name: name.clone(),
                source,
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Manifest::create(&path).map_err(|source| LibraryError::Manifest {
                name: name.clone(),
                source,
            })?;
        }
        Err(source) => return Err(LibraryError::Read { path, source }),
    }
    Ok(path)
}

/// Recreate the default shelf's directory and manifest when they are missing.
///
/// Deliberately does **not** parse an existing manifest. This runs on every
/// `open_or_create`, so parsing here would make one corrupt `main/shelf.toml` fail
/// every command — including the batch operations that are supposed to survive a
/// damaged shelf. A corrupt manifest still surfaces on first use.
fn repair_default_shelf(root: &Path) -> Result<(), LibraryError> {
    let name = ShelfName::default_shelf();
    let directory = shelf_directory(root, &name);
    if !matches!(fs::symlink_metadata(&directory), Ok(metadata) if metadata.is_dir()) {
        ensure_managed_directory(&directory)?;
    }
    let path = directory.join(MANIFEST_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Manifest::create(&path).map_err(|source| LibraryError::Manifest { name, source })?;
            Ok(())
        }
        Err(source) => Err(LibraryError::Read { path, source }),
    }
}

/// Validate that a registered shelf exists on disk as direct, non-symlink entries.
///
/// Delegates the entry-kind checks to the shared managed-path validators so the
/// same on-disk state cannot yield a different variant depending on entry point,
/// then relabels them with the shelf's name for a message the user can act on.
fn validate_shelf_manifest(root: &Path, name: &ShelfName) -> Result<PathBuf, LibraryError> {
    let directory = shelf_directory(root, name);
    validate_managed_directory(&directory).map_err(|error| relabel_shelf(error, name))?;
    let path = directory.join(MANIFEST_FILE);
    validate_managed_file(&path).map_err(|error| relabel_shelf(error, name))?;
    Ok(path)
}

/// Rewrite a managed-path error as a shelf error, naming the shelf.
fn relabel_shelf(error: LibraryError, name: &ShelfName) -> LibraryError {
    match error {
        LibraryError::Read { path, source } if source.kind() == std::io::ErrorKind::NotFound => {
            LibraryError::ShelfMissing {
                name: name.clone(),
                path,
            }
        }
        LibraryError::InvalidDirectory { path, .. } => LibraryError::InvalidShelf {
            name: name.clone(),
            message: format!("{} must be a direct directory", path.display()),
        },
        LibraryError::InvalidFile { path, .. } => LibraryError::InvalidShelf {
            name: name.clone(),
            message: format!("{} must be a direct file", path.display()),
        },
        other => other,
    }
}

/// Reject a new shelf whose directory would alias an already-registered one.
///
/// Only the *new* name is canonicalized against the registered set. A sibling that
/// cannot be canonicalized is damaged, not aliasing, so it is skipped — otherwise
/// one broken shelf would block creating every other shelf. Running before anything
/// is written also means a rejection leaves no half-created directory behind.
fn validate_new_shelf_alias(
    root: &Path,
    name: &ShelfName,
    registered: &BTreeSet<ShelfName>,
) -> Result<(), LibraryError> {
    // Not resolvable yet means nothing to alias. A case-insensitive filesystem is
    // the case that matters: there `shelves/PAPER` already resolves to `paper`.
    let Ok(candidate) = fs::canonicalize(shelf_directory(root, name)) else {
        return Ok(());
    };
    for other in registered.iter().filter(|other| *other != name) {
        if fs::canonicalize(shelf_directory(root, other)).is_ok_and(|path| path == candidate) {
            return Err(LibraryError::InvalidShelf {
                name: name.clone(),
                message: format!("directory aliases registered shelf `{other}`"),
            });
        }
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<(), LibraryError> {
    fs::create_dir_all(path).map_err(|source| LibraryError::CreateDirectory {
        path: path.to_path_buf(),
        source,
    })
}

fn ensure_managed_directory(path: &Path) -> Result<(), LibraryError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_managed_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_directory(path),
        Err(source) => Err(LibraryError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_managed_directory(path: &Path) -> Result<(), LibraryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| LibraryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LibraryError::InvalidDirectory {
            path: path.to_path_buf(),
            message: "must be a direct directory".into(),
        });
    }
    Ok(())
}

fn validate_managed_file(path: &Path) -> Result<(), LibraryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| LibraryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LibraryError::InvalidFile {
            path: path.to_path_buf(),
            message: "must be a direct regular file".into(),
        });
    }
    Ok(())
}

fn lock_file(path: &Path) -> Result<File, LibraryError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_managed_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(LibraryError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| LibraryError::Lock {
            path: path.to_path_buf(),
            source,
        })?;
    file.lock_exclusive().map_err(|source| LibraryError::Lock {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(file)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), LibraryError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| LibraryError::Write {
        path: path.into(),
        source,
    })?;
    temporary
        .write_all(bytes)
        .map_err(|source| LibraryError::Write {
            path: path.into(),
            source,
        })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| LibraryError::Write {
            path: path.into(),
            source,
        })?;
    temporary
        .persist(path)
        .map_err(|error| LibraryError::Write {
            path: path.into(),
            source: error.error,
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn shelf(name: &str) -> ShelfName {
        ShelfName::try_from(name).unwrap()
    }

    fn names(library: &Library) -> Vec<&str> {
        library.shelves().iter().map(ShelfName::as_str).collect()
    }

    #[test]
    fn initialization_creates_main_and_is_idempotent() {
        let directory = root();
        let mut path = directory.path().to_path_buf();
        path.push("home");
        let first = Library::open_or_create(&path).unwrap();
        assert_eq!(first.shelves(), &BTreeSet::from([shelf("main")]));
        assert!(path.join("shelves/main/shelf.toml").is_file());
        assert!(path.join("files").is_dir());
        assert_eq!(
            Library::open_or_create(&path).unwrap().shelves(),
            first.shelves()
        );
        fs::remove_dir(path.join("files")).unwrap();
        Library::open_or_create(&path).unwrap();
        assert!(path.join("files").is_dir());
    }

    #[test]
    fn opening_repairs_a_deleted_default_shelf() {
        let directory = root();
        let path = directory.path().join("home");
        let library = Library::open_or_create(&path).unwrap();
        library.shelf_manifest(&ShelfName::default_shelf()).unwrap();

        fs::remove_dir_all(path.join("shelves/main")).unwrap();
        let repaired = Library::open_or_create(&path).unwrap();
        assert!(path.join("shelves/main/shelf.toml").is_file());
        repaired
            .shelf_manifest(&ShelfName::default_shelf())
            .unwrap();
    }

    #[test]
    fn repairing_the_default_shelf_never_rewrites_an_existing_manifest() {
        // A corrupt `main` must stay corrupt rather than be silently replaced, and
        // must not stop the library from opening for other shelves.
        let directory = root();
        let path = directory.path().join("home");
        Library::open_or_create(&path).unwrap();
        let manifest = path.join("shelves/main/shelf.toml");
        fs::write(&manifest, "schema = 1\nunknown = true\n").unwrap();

        let library = Library::open_or_create(&path).unwrap();
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            "schema = 1\nunknown = true\n"
        );
        assert_eq!(names(&library), ["main"]);
    }

    #[test]
    fn registry_and_shelves_are_sorted_and_validated() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        assert!(library.create_shelf(&shelf("zeta")).unwrap());
        assert!(library.create_shelf(&shelf("alpha")).unwrap());
        assert!(!library.create_shelf(&shelf("alpha")).unwrap());
        assert_eq!(names(&library), ["alpha", "main", "zeta"]);
        let rendered = fs::read_to_string(path.join(LIBRARY_FILE)).unwrap();
        assert!(rendered.find("\"alpha\"").unwrap() < rendered.find("\"zeta\"").unwrap());
    }

    #[test]
    fn invalid_names_are_rejected_before_any_filesystem_work() {
        for name in [".bad", "..", "/etc", "a/b", "", "-lead"] {
            assert!(matches!(
                ShelfName::try_from(name),
                Err(LibraryError::InvalidName(_))
            ));
        }
        assert_eq!(shelf("ok.name_1-2").as_str(), "ok.name_1-2");
    }

    #[test]
    fn invalid_registry_shapes_are_rejected() {
        let directory = root();
        let path = directory.path().join("home");
        Library::open_or_create(&path).unwrap();
        for registry in [
            "schema = 1\ndefault = \"other\"\nshelves = [\"main\"]\n",
            "schema = 1\ndefault = \"main\"\nshelves = [\"main\", \"main\"]\n",
            "schema = 1\ndefault = \"main\"\nshelves = [\"paper\"]\n",
            "schema = 1\ndefault = \"main\"\nshelves = [\"main\"]\nextra = 1\n",
            "default = \"main\"\nshelves = [\"main\"]\n",
        ] {
            fs::write(path.join(LIBRARY_FILE), registry).unwrap();
            assert!(
                matches!(Library::load(&path), Err(LibraryError::Invalid { .. })),
                "{registry}"
            );
        }
        fs::write(
            path.join(LIBRARY_FILE),
            "schema = 1\ndefault = \"main\"\nshelves = [\"main\", \".bad\"]\n",
        )
        .unwrap();
        assert!(matches!(
            Library::load(&path),
            Err(LibraryError::InvalidName(_))
        ));
    }

    #[test]
    fn unsupported_schema_is_rejected_without_rewriting() {
        let directory = root();
        let path = directory.path().join("home");
        Library::open_or_create(&path).unwrap();
        let registry = "schema = 2\ndefault = \"main\"\nshelves = [\"main\"]\n";
        fs::write(path.join(LIBRARY_FILE), registry).unwrap();
        assert!(matches!(
            Library::load(&path),
            Err(LibraryError::UnsupportedSchema { found: 2 })
        ));
        assert_eq!(
            fs::read_to_string(path.join(LIBRARY_FILE)).unwrap(),
            registry
        );
    }

    #[test]
    fn load_keeps_registry_names_when_a_shelf_is_damaged() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("paper")).unwrap();
        fs::remove_dir_all(path.join("shelves/paper")).unwrap();

        let loaded = Library::load(&path).unwrap();
        assert_eq!(names(&loaded), ["main", "paper"]);
        assert!(matches!(
            loaded.shelf_manifest(&shelf("paper")),
            Err(LibraryError::ShelfMissing { .. })
        ));
        assert!(loaded.shelf_manifest(&shelf("main")).is_ok());

        fs::write(path.join("shelves/paper"), b"not-a-directory").unwrap();
        let loaded = Library::open_or_create(&path).unwrap();
        assert!(matches!(
            loaded.shelf_manifest(&shelf("paper")),
            Err(LibraryError::InvalidShelf { .. })
        ));
        assert!(loaded.shelf_manifest(&shelf("main")).is_ok());
    }

    #[test]
    fn a_damaged_shelf_does_not_block_creating_another() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("bar")).unwrap();
        fs::remove_dir_all(path.join("shelves/bar")).unwrap();

        assert!(library.create_shelf(&shelf("foo")).unwrap());
        assert_eq!(names(&library), ["bar", "foo", "main"]);
        assert!(path.join("shelves/foo/shelf.toml").is_file());
    }

    #[test]
    fn a_rejected_creation_leaves_no_orphan_directory() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("paper")).unwrap();

        // A regular file where the new shelf directory would go is rejected, and
        // nothing about the store changes.
        fs::write(path.join("shelves/blocked"), b"in the way").unwrap();
        assert!(library.create_shelf(&shelf("blocked")).is_err());
        assert_eq!(names(&library), ["main", "paper"]);
        let rendered = fs::read_to_string(path.join(LIBRARY_FILE)).unwrap();
        assert!(!rendered.contains("blocked"), "{rendered}");
    }

    #[test]
    fn shelf_and_registry_locks_use_separate_namespaces() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        // `library` is only special as a filename; as a shelf it must behave like
        // any other and must not reuse the registry's lock.
        let name = shelf("library");
        library.create_shelf(&name).unwrap();
        drop(library.lock_shelf(&name).unwrap());
        assert!(path.join("locks/shelf-library.lock").is_file());
        assert!(path.join("locks/registry.lock").is_file());
        assert_ne!(
            registry_lock_path(&path),
            shelf_lock_path(&path, &name),
            "a shelf must never share the registry lock"
        );
    }

    #[test]
    fn case_aliases_are_rejected_on_case_insensitive_filesystems() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("paper")).unwrap();
        // On a case-sensitive filesystem `PAPER` is a genuinely distinct shelf, so
        // succeeding there is correct. `aliased_directories_are_rejected` covers the
        // rejection branch on every platform.
        let aliases = path.join("shelves/PAPER").exists();
        let result = library.create_shelf(&shelf("PAPER"));
        if aliases {
            assert!(matches!(result, Err(LibraryError::InvalidShelf { .. })));
        } else {
            assert!(result.unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn aliased_directories_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("paper")).unwrap();
        symlink(path.join("shelves/paper"), path.join("shelves/alias")).unwrap();
        assert!(matches!(
            library.create_shelf(&shelf("alias")),
            Err(LibraryError::InvalidShelf { .. })
        ));
        assert_eq!(names(&library), ["main", "paper"]);
    }

    #[cfg(unix)]
    #[test]
    fn direct_shelf_directories_cannot_be_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = root();
        let path = directory.path().join("home");
        let outside = root();
        let mut library = Library::open_or_create(&path).unwrap();
        symlink(outside.path(), path.join("shelves/alias")).unwrap();
        assert!(matches!(
            library.create_shelf(&shelf("alias")),
            Err(LibraryError::InvalidShelf { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn managed_files_cannot_be_symlinks() {
        use std::os::unix::fs::symlink;

        // `shelf.toml` and `library.toml` are the files atomic writes replace, so a
        // symlink here would let a write escape the store.
        let directory = root();
        let path = directory.path().join("home");
        let outside = root();
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf(&shelf("paper")).unwrap();

        let manifest = path.join("shelves/paper/shelf.toml");
        fs::remove_file(&manifest).unwrap();
        symlink(outside.path().join("escaped.toml"), &manifest).unwrap();
        assert!(matches!(
            library.shelf_manifest(&shelf("paper")),
            Err(LibraryError::InvalidShelf { .. })
        ));

        let registry = path.join(LIBRARY_FILE);
        fs::remove_file(&registry).unwrap();
        symlink(outside.path().join("escaped-registry.toml"), &registry).unwrap();
        assert!(matches!(
            Library::open_or_create(&path),
            Err(LibraryError::InvalidFile { .. })
        ));
        assert!(!outside.path().join("escaped-registry.toml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_parent_directories_cannot_be_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = root();
        let path = directory.path().join("home");
        let outside = root();
        fs::create_dir(&path).unwrap();
        symlink(outside.path(), path.join("shelves")).unwrap();
        assert!(matches!(
            Library::open_or_create(&path),
            Err(LibraryError::InvalidDirectory { .. })
        ));
        assert!(!outside.path().join("main").exists());
    }
}
