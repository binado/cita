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

/// A loaded schema-1 global library registry.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    shelves: BTreeSet<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryData {
    schema: u32,
    default: String,
    shelves: BTreeSet<String>,
}

/// An exclusive advisory lock for one shelf.
#[derive(Debug)]
pub struct ShelfLock {
    _file: File,
}

/// Error produced by global library loading, validation, locking, or persistence.
#[derive(Debug, Error)]
pub enum LibraryError {
    /// The global library root is not absolute.
    #[error("cita home must be an absolute path: {0}")]
    RelativeRoot(PathBuf),
    /// The explicit global library root is empty.
    #[error("CITA_HOME cannot be empty")]
    EmptyRoot,
    /// No operating-system home directory is available.
    #[error("could not determine the home directory; set CITA_HOME to an absolute path")]
    HomeUnavailable,
    /// A managed directory could not be created.
    #[error("could not create {path}: {source}")]
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
    /// A managed file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// File that could not be read.
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
    /// A shelf path is missing, malformed, or a symlink.
    #[error("invalid shelf `{name}`: {message}")]
    InvalidShelf {
        /// Shelf name.
        name: String,
        /// Validation diagnostic.
        message: String,
    },
    /// A managed lock could not be acquired.
    #[error("could not lock {path}: {source}")]
    Lock {
        /// Lock file path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The registry could not be serialized.
    #[error("could not serialize library: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// The registry could not be written atomically.
    #[error("could not write {path}: {source}")]
    Write {
        /// Registry path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A shelf manifest is invalid.
    #[error("invalid shelf `{name}`: {source}")]
    Manifest {
        /// Shelf name.
        name: String,
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
        return Err(LibraryError::RelativeRoot(home));
    }
    Ok(home.join(".cita"))
}

impl Library {
    /// Open the global library, creating it and the default shelf when absent.
    pub fn open_or_create(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root.to_path_buf()));
        }
        create_directory(root)?;
        ensure_managed_directory(&root.join("locks"))?;
        let _lock = lock_file(&root.join("locks/library.lock"))?;
        ensure_managed_directory(&root.join("shelves"))?;
        ensure_managed_directory(&root.join("files"))?;
        let registry = root.join(LIBRARY_FILE);
        match fs::symlink_metadata(&registry) {
            Ok(_) => {
                validate_managed_file(&registry)?;
                return Self::load(root);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(LibraryError::Read {
                    path: registry,
                    source,
                });
            }
        }
        ensure_shelf_manifest(root, DEFAULT_SHELF)?;
        let shelves = BTreeSet::from([DEFAULT_SHELF.to_owned()]);
        let library = Self {
            root: root.to_path_buf(),
            shelves,
        };
        library.persist(&library.shelves)?;
        Ok(library)
    }

    /// Load an existing global library registry.
    ///
    /// Registry shape and shelf names are validated here. Per-shelf directory and
    /// `shelf.toml` checks happen in [`Self::shelf_manifest`], so one damaged shelf
    /// does not prevent loading the name list for batch operations.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_absolute() {
            return Err(LibraryError::RelativeRoot(root));
        }
        for directory in ["locks", "shelves", "files"] {
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
        for name in &data.shelves {
            validate_shelf_name(name)?;
        }
        Ok(Self {
            root,
            shelves: data.shelves,
        })
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
        self.root.join("files")
    }

    /// Return registered shelf names in stable order.
    pub fn shelves(&self) -> &BTreeSet<String> {
        &self.shelves
    }

    /// Return the deterministic manifest path for a registered shelf.
    pub fn shelf_manifest(&self, name: &str) -> Result<PathBuf, LibraryError> {
        if !self.shelves.contains(name) {
            return Err(LibraryError::UnknownShelf(name.into()));
        }
        validate_shelf_manifest(&self.root, name)
    }

    /// Create and register an empty shelf, or validate an existing registration.
    pub fn create_shelf(&mut self, name: &str) -> Result<bool, LibraryError> {
        validate_shelf_name(name)?;
        let _lock = lock_file(&self.root.join("locks/library.lock"))?;
        let current = Self::load(&self.root)?;
        if current.shelves.contains(name) {
            validate_shelf_manifest(&self.root, name)?;
            self.shelves = current.shelves;
            return Ok(false);
        }
        ensure_shelf_manifest(&self.root, name)?;
        let mut candidate = current.shelves;
        candidate.insert(name.into());
        validate_unique_shelf_paths(&self.root, &candidate)?;
        self.persist(&candidate)?;
        self.shelves = candidate;
        Ok(true)
    }

    /// Acquire the exclusive advisory lock for a shelf mutation.
    pub fn lock_shelf(&self, name: &str) -> Result<ShelfLock, LibraryError> {
        self.shelf_manifest(name)?;
        let file = lock_file(&self.root.join("locks").join(format!("{name}.lock")))?;
        Ok(ShelfLock { _file: file })
    }

    fn persist(&self, shelves: &BTreeSet<String>) -> Result<(), LibraryError> {
        let data = LibraryData {
            schema: SCHEMA,
            default: DEFAULT_SHELF.into(),
            shelves: shelves.clone(),
        };
        let mut rendered = toml::to_string_pretty(&data)?;
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        atomic_write(&self.path(), rendered.as_bytes())
    }
}

/// Validate a stable shelf name.
pub fn validate_shelf_name(name: &str) -> Result<(), LibraryError> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(LibraryError::InvalidName(name.into()));
    }
    Ok(())
}

fn shelf_directory(root: &Path, name: &str) -> PathBuf {
    root.join("shelves").join(name)
}

fn ensure_shelf_manifest(root: &Path, name: &str) -> Result<PathBuf, LibraryError> {
    validate_shelf_name(name)?;
    let directory = shelf_directory(root, name);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(LibraryError::InvalidShelf {
                name: name.into(),
                message: "shelf directory cannot be a symlink".into(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(LibraryError::InvalidShelf {
                name: name.into(),
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
                name: name.into(),
                source,
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Manifest::create(&path).map_err(|source| LibraryError::Manifest {
                name: name.into(),
                source,
            })?;
        }
        Err(source) => return Err(LibraryError::Read { path, source }),
    }
    Ok(path)
}

fn validate_shelf_manifest(root: &Path, name: &str) -> Result<PathBuf, LibraryError> {
    let directory = shelf_directory(root, name);
    let metadata = fs::symlink_metadata(&directory).map_err(|source| LibraryError::Read {
        path: directory.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LibraryError::InvalidShelf {
            name: name.into(),
            message: format!("{} must be a direct directory", directory.display()),
        });
    }
    let path = directory.join(MANIFEST_FILE);
    let metadata = fs::symlink_metadata(&path).map_err(|source| LibraryError::Read {
        path: path.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LibraryError::InvalidShelf {
            name: name.into(),
            message: format!("{} must be a direct file", path.display()),
        });
    }
    Ok(path)
}

fn validate_unique_shelf_paths(
    root: &Path,
    shelves: &BTreeSet<String>,
) -> Result<(), LibraryError> {
    let mut resolved = Vec::<(String, PathBuf)>::new();
    for name in shelves {
        let directory =
            fs::canonicalize(shelf_directory(root, name)).map_err(|source| LibraryError::Read {
                path: shelf_directory(root, name),
                source,
            })?;
        if let Some((other, _)) = resolved.iter().find(|(_, existing)| existing == &directory) {
            return Err(LibraryError::InvalidShelf {
                name: name.clone(),
                message: format!("directory aliases registered shelf `{other}`"),
            });
        }
        resolved.push((name.clone(), directory));
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

    #[test]
    fn initialization_creates_main_and_is_idempotent() {
        let directory = root();
        let mut path = directory.path().to_path_buf();
        path.push("home");
        let first = Library::open_or_create(&path).unwrap();
        assert_eq!(first.shelves(), &BTreeSet::from(["main".into()]));
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
    fn registry_and_shelves_are_sorted_and_validated() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        assert!(library.create_shelf("zeta").unwrap());
        assert!(library.create_shelf("alpha").unwrap());
        assert!(!library.create_shelf("alpha").unwrap());
        assert_eq!(
            library
                .shelves()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["alpha", "main", "zeta"]
        );
        let rendered = fs::read_to_string(path.join(LIBRARY_FILE)).unwrap();
        assert!(rendered.find("\"alpha\"").unwrap() < rendered.find("\"zeta\"").unwrap());
    }

    #[test]
    fn invalid_names_and_registry_shapes_are_rejected() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        assert!(matches!(
            library.create_shelf(".bad"),
            Err(LibraryError::InvalidName(_))
        ));
        fs::write(
            path.join(LIBRARY_FILE),
            "schema = 1\ndefault = \"other\"\nshelves = [\"main\"]\n",
        )
        .unwrap();
        assert!(matches!(
            Library::load(&path),
            Err(LibraryError::Invalid { .. })
        ));
        fs::write(
            path.join(LIBRARY_FILE),
            "schema = 1\ndefault = \"main\"\nshelves = [\"main\", \"main\"]\n",
        )
        .unwrap();
        assert!(matches!(
            Library::load(&path),
            Err(LibraryError::Invalid { .. })
        ));
    }

    #[test]
    fn load_keeps_registry_names_when_a_shelf_is_damaged() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf("paper").unwrap();
        fs::remove_dir_all(path.join("shelves/paper")).unwrap();

        let loaded = Library::load(&path).unwrap();
        assert_eq!(
            loaded
                .shelves()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["main", "paper"]
        );
        assert!(matches!(
            loaded.shelf_manifest("paper"),
            Err(LibraryError::Read { .. } | LibraryError::InvalidShelf { .. })
        ));
        assert!(loaded.shelf_manifest("main").is_ok());

        fs::write(path.join("shelves/paper"), b"not-a-directory").unwrap();
        let loaded = Library::open_or_create(&path).unwrap();
        assert!(matches!(
            loaded.shelf_manifest("paper"),
            Err(LibraryError::InvalidShelf { .. })
        ));
        assert!(loaded.shelf_manifest("main").is_ok());
    }

    #[test]
    fn case_aliases_are_rejected_on_case_insensitive_filesystems() {
        let directory = root();
        let path = directory.path().join("home");
        let mut library = Library::open_or_create(&path).unwrap();
        library.create_shelf("paper").unwrap();
        let aliases = path.join("shelves/PAPER").exists();
        let result = library.create_shelf("PAPER");
        if aliases {
            assert!(matches!(result, Err(LibraryError::InvalidShelf { .. })));
        } else {
            assert!(result.unwrap());
        }
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
            library.create_shelf("alias"),
            Err(LibraryError::InvalidShelf { .. })
        ));
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
