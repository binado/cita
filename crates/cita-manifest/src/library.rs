//! Schema-1 library registry storage and shelf-path validation.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{BIBLIOGRAPHY_FILE, MANIFEST_FILE, SCHEMA};

/// Name of the library registry.
pub const LIBRARY_FILE: &str = "cita-library.toml";

/// A registered, independently managed cita project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shelf {
    path: PathBuf,
}

impl Shelf {
    /// Return the shelf path relative to the library root.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A loaded schema-1 library registry.
#[derive(Clone, Debug)]
pub struct Library {
    path: PathBuf,
    shelves: BTreeMap<String, Shelf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryData {
    schema: u32,
    #[serde(default)]
    shelves: BTreeMap<String, ShelfData>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShelfData {
    path: String,
}

/// Error produced by library discovery, validation, or persistence.
#[derive(Debug, Error)]
pub enum LibraryError {
    /// No registry was found during ancestor discovery.
    #[error("no cita-library.toml found in {start} or its parents; run `cita library init`")]
    NotFound {
        /// Directory where discovery began.
        start: PathBuf,
    },
    /// The requested library root is not an existing directory.
    #[error("library path {0} is not an existing directory")]
    NotDirectory(PathBuf),
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
        "unsupported cita-library.toml schema {found}; this version supports schema 1 and provides no legacy migration"
    )]
    UnsupportedSchema {
        /// Schema value found in the file.
        found: i64,
    },
    /// A library root also contains shelf-level managed artifacts.
    #[error("library root cannot contain shelf artifact {0}")]
    RootShelf(PathBuf),
    /// A shelf name is malformed.
    #[error("invalid shelf name `{0}`; names must match [A-Za-z0-9][A-Za-z0-9._-]*")]
    InvalidName(String),
    /// A shelf path is malformed, unsafe, missing, escapes the library root,
    /// or overlaps another shelf.
    #[error("invalid path for shelf `{name}`: {message}")]
    InvalidPath {
        /// Shelf whose path is invalid.
        name: String,
        /// Validation diagnostic.
        message: String,
    },
    /// A requested shelf is not registered.
    #[error("unknown shelf `{0}`")]
    UnknownShelf(String),
    /// A shelf name is already registered with another path.
    #[error("shelf `{name}` is already registered at {path}")]
    AlreadyRegistered {
        /// Stable shelf name.
        name: String,
        /// Existing relative path.
        path: PathBuf,
    },
    /// A loaded registry's shelves failed validation.
    #[error("invalid library {path}: {source}")]
    InvalidShelf {
        /// Invalid registry path.
        path: PathBuf,
        /// The specific validation failure.
        #[source]
        source: Box<LibraryError>,
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
}

impl Library {
    /// Create an empty registry, or load an existing valid registry.
    pub fn create(directory: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = directory.as_ref();
        if !root.is_dir() {
            return Err(LibraryError::NotDirectory(root.to_path_buf()));
        }
        reject_root_shelf(root)?;
        let path = root.join(LIBRARY_FILE);
        if path.exists() {
            return Self::load(path);
        }
        let library = Self {
            path,
            shelves: BTreeMap::new(),
        };
        library.persist(&library.shelves)?;
        Ok(library)
    }

    /// Search a directory and its ancestors for a registry, then load it.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let start = start.as_ref();
        for directory in start.ancestors() {
            let candidate = directory.join(LIBRARY_FILE);
            if candidate.is_file() {
                return Self::load(candidate);
            }
        }
        Err(LibraryError::NotFound {
            start: start.to_path_buf(),
        })
    }

    /// Load and validate a registry and all registered paths.
    ///
    /// Shelf manifests and generated bibliographies are deliberately not read.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let path = path.as_ref().to_path_buf();
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
        let root = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        reject_root_shelf(&root)?;
        let shelves = data
            .shelves
            .into_iter()
            .map(|(name, shelf)| {
                (
                    name,
                    Shelf {
                        path: shelf.path.into(),
                    },
                )
            })
            .collect();
        let library = Self { path, shelves };
        library
            .validate_shelves(false)
            .map_err(|error| LibraryError::InvalidShelf {
                path: library.path.clone(),
                source: Box::new(error),
            })?;
        Ok(library)
    }

    /// Return the registry path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the library root.
    pub fn root(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }

    /// Return registered shelves in stable name order.
    pub fn shelves(&self) -> &BTreeMap<String, Shelf> {
        &self.shelves
    }

    /// Return a shelf by its stable name.
    pub fn shelf(&self, name: &str) -> Result<&Shelf, LibraryError> {
        self.shelves
            .get(name)
            .ok_or_else(|| LibraryError::UnknownShelf(name.into()))
    }

    /// Resolve a registered shelf to its library-root-relative directory.
    pub fn shelf_directory(&self, name: &str) -> Result<PathBuf, LibraryError> {
        Ok(self.root().join(self.shelf(name)?.path()))
    }

    /// Validate a proposed registration before creating its directory.
    pub fn validate_registration(
        &self,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), LibraryError> {
        validate_name(name)?;
        match self.candidate_shelves(name, path)? {
            Some(candidate) => validate_shelf_set(self.root(), &candidate, false),
            None => Ok(()),
        }
    }

    /// Atomically register an initialized shelf.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Result<(), LibraryError> {
        let name = name.into();
        match self.candidate_shelves(&name, path)? {
            Some(candidate) => {
                validate_shelf_set(self.root(), &candidate, true)?;
                self.persist(&candidate)?;
                self.shelves = candidate;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Return the shelf map with `name`/`path` inserted, or `None` if that
    /// exact name/path pair is already registered (a no-op registration).
    fn candidate_shelves(
        &self,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<Option<BTreeMap<String, Shelf>>, LibraryError> {
        if let Some(existing) = self.shelves.get(name) {
            if existing.path == path.as_ref() {
                return Ok(None);
            }
            return Err(LibraryError::AlreadyRegistered {
                name: name.into(),
                path: existing.path.clone(),
            });
        }
        let mut candidate = self.shelves.clone();
        candidate.insert(
            name.into(),
            Shelf {
                path: path.as_ref().to_path_buf(),
            },
        );
        Ok(Some(candidate))
    }

    fn validate_shelves(&self, require_exists: bool) -> Result<(), LibraryError> {
        validate_shelf_set(self.root(), &self.shelves, require_exists)
    }

    fn persist(&self, shelves: &BTreeMap<String, Shelf>) -> Result<(), LibraryError> {
        let data = LibraryData {
            schema: SCHEMA,
            shelves: shelves
                .iter()
                .map(|(name, shelf)| {
                    let path = shelf
                        .path
                        .to_str()
                        .ok_or_else(|| LibraryError::InvalidPath {
                            name: name.clone(),
                            message: "path is not valid UTF-8".into(),
                        })?;
                    Ok((name.clone(), ShelfData { path: path.into() }))
                })
                .collect::<Result<_, LibraryError>>()?,
        };
        let mut rendered = toml::to_string_pretty(&data)?;
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        atomic_write(&self.path, rendered.as_bytes())
    }
}

fn validate_name(name: &str) -> Result<(), LibraryError> {
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

fn validate_shelf_set(
    root: &Path,
    shelves: &BTreeMap<String, Shelf>,
    require_exists: bool,
) -> Result<(), LibraryError> {
    let canonical_root = fs::canonicalize(root).map_err(|source| LibraryError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    let mut resolved = Vec::<(&str, PathBuf)>::new();
    for (name, shelf) in shelves {
        validate_name(name)?;
        validate_relative_path(name, &shelf.path)?;
        let target = root.join(&shelf.path);
        if require_exists && !target.is_dir() {
            return Err(LibraryError::InvalidPath {
                name: name.clone(),
                message: format!("{} is not an existing directory", shelf.path.display()),
            });
        }
        let canonical_target = resolve_with_existing_ancestor(&target).map_err(|source| {
            LibraryError::InvalidPath {
                name: name.clone(),
                message: format!("could not resolve {}: {source}", shelf.path.display()),
            }
        })?;
        if canonical_target == canonical_root || !canonical_target.starts_with(&canonical_root) {
            return Err(LibraryError::InvalidPath {
                name: name.clone(),
                message: "path resolves to or outside the library root".into(),
            });
        }
        for (other_name, other_path) in &resolved {
            if canonical_target == *other_path
                || canonical_target.starts_with(other_path)
                || other_path.starts_with(&canonical_target)
            {
                return Err(LibraryError::InvalidPath {
                    name: name.clone(),
                    message: format!(
                        "path is equal to, nested within, contains, or aliases shelf `{other_name}`"
                    ),
                });
            }
        }
        resolved.push((name, canonical_target));
    }
    Ok(())
}

fn validate_relative_path(name: &str, path: &Path) -> Result<(), LibraryError> {
    if path.as_os_str().is_empty() || path == Path::new(".") || path.is_absolute() {
        return Err(LibraryError::InvalidPath {
            name: name.into(),
            message: "path must be a non-empty relative path other than `.`".into(),
        });
    }
    if path.to_str().is_none() {
        return Err(LibraryError::InvalidPath {
            name: name.into(),
            message: "path is not valid UTF-8".into(),
        });
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(LibraryError::InvalidPath {
            name: name.into(),
            message: "path cannot contain `..` components".into(),
        });
    }
    Ok(())
}

fn resolve_with_existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path has no existing ancestor",
                    )
                })?;
                suffix.push(name.to_owned());
                ancestor = ancestor.parent().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn reject_root_shelf(root: &Path) -> Result<(), LibraryError> {
    for file in [MANIFEST_FILE, BIBLIOGRAPHY_FILE] {
        let path = root.join(file);
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(LibraryError::RootShelf(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(LibraryError::Read { path, source }),
        }
    }
    Ok(())
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

    #[test]
    fn registry_is_sorted_and_load_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let mut library = Library::create(directory.path()).unwrap();
        fs::create_dir(directory.path().join("z")).unwrap();
        fs::create_dir(directory.path().join("a")).unwrap();
        library.register("z-shelf", "z").unwrap();
        library.register("a-shelf", "a").unwrap();

        let rendered = fs::read_to_string(directory.path().join(LIBRARY_FILE)).unwrap();
        assert!(
            rendered.find("[shelves.a-shelf]").unwrap()
                < rendered.find("[shelves.z-shelf]").unwrap()
        );
        assert_eq!(
            Library::create(directory.path()).unwrap().shelves(),
            library.shelves()
        );
    }

    #[test]
    fn rejects_names_escapes_duplicates_nesting_and_unknown_data() {
        let directory = tempfile::tempdir().unwrap();
        let mut library = Library::create(directory.path()).unwrap();
        fs::create_dir(directory.path().join("one")).unwrap();
        fs::create_dir(directory.path().join("one/nested")).unwrap();
        assert!(matches!(
            library.validate_registration(".bad", "other"),
            Err(LibraryError::InvalidName(_))
        ));
        assert!(
            library
                .validate_registration("escape", "../escape")
                .is_err()
        );
        library.register("one", "one").unwrap();
        assert!(library.validate_registration("duplicate", "one").is_err());
        assert!(
            library
                .validate_registration("nested", "one/nested")
                .is_err()
        );

        fs::write(
            directory.path().join(LIBRARY_FILE),
            "schema = 1\nunknown = true\n",
        )
        .unwrap();
        assert!(matches!(
            Library::load(directory.path().join(LIBRARY_FILE)),
            Err(LibraryError::Invalid { .. })
        ));
    }

    #[test]
    fn rejects_unsupported_schema_and_root_shelves() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(LIBRARY_FILE), "schema = 2\n").unwrap();
        assert!(matches!(
            Library::load(directory.path().join(LIBRARY_FILE)),
            Err(LibraryError::UnsupportedSchema { found: 2 })
        ));
        fs::write(directory.path().join(LIBRARY_FILE), "schema = 1\n").unwrap();
        fs::write(directory.path().join(MANIFEST_FILE), "schema = 1\n").unwrap();
        assert!(matches!(
            Library::load(directory.path().join(LIBRARY_FILE)),
            Err(LibraryError::RootShelf(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_aliases_and_escapes() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mut library = Library::create(directory.path()).unwrap();
        fs::create_dir(directory.path().join("real")).unwrap();
        symlink("real", directory.path().join("alias")).unwrap();
        symlink(outside.path(), directory.path().join("outside")).unwrap();
        library.register("real", "real").unwrap();
        assert!(library.validate_registration("alias", "alias").is_err());
        assert!(library.validate_registration("outside", "outside").is_err());
    }

    #[test]
    fn discovery_is_independent_and_walks_ancestors() {
        let directory = tempfile::tempdir().unwrap();
        Library::create(directory.path()).unwrap();
        let nested = directory.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(Library::discover(&nested).unwrap().root(), directory.path());
    }
}
