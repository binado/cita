//! Reading and publishing `bibi.toml`.

use crate::{
    atomic::atomic_replace,
    candidate::{Manifest, ManifestCandidate},
    error::Error,
    schema,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// What the manifest looked like when it was read.
///
/// The generation retains the exact bytes rather than a hash: the comparison
/// happens once per commit, the file is small, and keeping the bytes avoids
/// both a hashing dependency and any question about collisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generation(State);

#[derive(Clone, Debug, Eq, PartialEq)]
enum State {
    /// No manifest existed when the command started.
    Missing,
    /// These were the bytes on disk.
    Loaded(String),
}

impl Generation {
    /// True when no manifest existed at load time.
    pub fn is_missing(&self) -> bool {
        matches!(self.0, State::Missing)
    }
}

/// One manifest read from disk, with the generation it was read at.
#[derive(Debug)]
pub struct LoadedManifest {
    /// The validated manifest.
    pub manifest: Manifest,
    generation: Generation,
}

impl LoadedManifest {
    /// The generation this manifest was read at.
    pub fn generation(&self) -> &Generation {
        &self.generation
    }

    /// Split into the manifest and its generation.
    pub fn into_parts(self) -> (Manifest, Generation) {
        (self.manifest, self.generation)
    }

    /// Split into a mutable candidate and the generation to commit against.
    pub fn into_candidate(self) -> (ManifestCandidate, Generation) {
        (self.manifest.into_candidate(), self.generation)
    }
}

/// A manifest file at one path.
#[derive(Clone, Debug)]
pub struct ManifestStore {
    path: PathBuf,
}

impl ManifestStore {
    /// A store over one manifest path, whose parent must already exist.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The manifest path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The directory outputs resolve against.
    pub fn directory(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }

    /// True when a manifest file exists at this path.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Load the manifest, failing when there is none.
    ///
    /// Read-only commands use this: conjuring an empty manifest would hide the
    /// actual answer, which is that this directory holds no project.
    pub fn load(&self) -> Result<LoadedManifest, Error> {
        match self.read()? {
            None => Err(Error::NoManifest {
                path: self.path.clone(),
            }),
            Some(source) => self.decode(source),
        }
    }

    /// Load the manifest, or an empty one when the file does not exist.
    ///
    /// Only commands that write records use this, and the returned generation
    /// distinguishes "no manifest, create one" from "manifest exists".
    pub fn load_or_empty(&self) -> Result<LoadedManifest, Error> {
        match self.read()? {
            None => Ok(LoadedManifest {
                manifest: ManifestCandidate::empty().validate()?,
                generation: Generation(State::Missing),
            }),
            Some(source) => self.decode(source),
        }
    }

    /// Create an empty manifest, refusing to overwrite anything.
    ///
    /// The refusal covers an invalid or newer-schema file too: `init` must not
    /// be a way to discard a manifest this build cannot read.
    pub fn create_empty(&self) -> Result<(), Error> {
        if self.path.exists() {
            return Err(Error::AlreadyExists {
                path: self.path.clone(),
            });
        }
        let rendered = ManifestCandidate::empty().validate()?.to_toml()?;
        atomic_replace(&self.path, rendered.as_bytes())
    }

    /// Validate a candidate and publish it, if the file has not changed.
    ///
    /// The comparison is best-effort staleness detection, not mutual exclusion:
    /// a window remains between reading and renaming. It is kept because it
    /// costs one read and catches the case that actually happens — a long sync
    /// holding a manifest in memory across seconds of network work while the
    /// user edits the file in another terminal.
    pub fn commit(
        &self,
        expected: &Generation,
        candidate: ManifestCandidate,
    ) -> Result<Generation, Error> {
        let manifest = candidate.validate()?;
        let rendered = manifest.to_toml()?;
        let current = self.read()?;
        let unchanged = match (&expected.0, &current) {
            (State::Missing, None) => true,
            (State::Loaded(expected), Some(current)) => expected == current,
            _ => false,
        };
        if !unchanged {
            return Err(Error::StaleManifest {
                path: self.path.clone(),
            });
        }
        atomic_replace(&self.path, rendered.as_bytes())?;
        Ok(Generation(State::Loaded(rendered)))
    }

    fn read(&self) -> Result<Option<String>, Error> {
        match fs::read_to_string(&self.path) {
            Ok(source) => Ok(Some(source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::io(&self.path, source)),
        }
    }

    fn decode(&self, source: String) -> Result<LoadedManifest, Error> {
        let records = schema::parse(&self.path, &source)?;
        Ok(LoadedManifest {
            manifest: Manifest::validate(records)?,
            generation: Generation(State::Loaded(source)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::record;

    fn store() -> (tempfile::TempDir, ManifestStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = ManifestStore::new(directory.path().join("bibi.toml"));
        (directory, store)
    }

    #[test]
    fn a_missing_manifest_is_an_error_to_read_and_empty_to_write() {
        let (_directory, store) = store();
        assert!(matches!(store.load(), Err(Error::NoManifest { .. })));
        let loaded = store.load_or_empty().unwrap();
        assert!(loaded.manifest.is_empty());
        assert!(loaded.generation().is_missing());
    }

    #[test]
    fn a_first_commit_creates_the_file_and_later_ones_replace_it() {
        let (_directory, store) = store();
        let (mut candidate, generation) = store.load_or_empty().unwrap().into_candidate();
        candidate.insert(record("Alpha", "inspire", "1")).unwrap();
        let generation = store.commit(&generation, candidate).unwrap();
        assert!(store.exists());
        let (mut candidate, reloaded) = store.load().unwrap().into_candidate();
        assert_eq!(&generation, &reloaded);
        candidate.insert(record("Zed", "inspire", "2")).unwrap();
        store.commit(&reloaded, candidate).unwrap();
        assert_eq!(store.load().unwrap().manifest.len(), 2);
    }

    #[test]
    fn init_refuses_to_overwrite_anything_at_all() {
        let (_directory, store) = store();
        store.create_empty().unwrap();
        assert!(matches!(
            store.create_empty(),
            Err(Error::AlreadyExists { .. })
        ));
        // Including a manifest this build cannot even read.
        fs::write(store.path(), "schema = 99\n").unwrap();
        assert!(matches!(
            store.create_empty(),
            Err(Error::AlreadyExists { .. })
        ));
    }

    #[test]
    fn a_commit_is_refused_after_an_out_of_band_edit() {
        let (_directory, store) = store();
        store.create_empty().unwrap();
        let (mut candidate, generation) = store.load().unwrap().into_candidate();
        candidate.insert(record("Alpha", "inspire", "1")).unwrap();
        // Somebody else writes while this command was working.
        let (mut other, other_generation) = store.load().unwrap().into_candidate();
        other.insert(record("Zed", "inspire", "2")).unwrap();
        store.commit(&other_generation, other).unwrap();

        assert!(matches!(
            store.commit(&generation, candidate),
            Err(Error::StaleManifest { .. })
        ));
        // The other command's write survives intact.
        let manifest = store.load().unwrap().manifest;
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest.records()[0].key.as_str(), "Zed");
    }

    #[test]
    fn a_first_commit_is_refused_when_a_manifest_appeared_meanwhile() {
        let (_directory, store) = store();
        let (candidate, generation) = store.load_or_empty().unwrap().into_candidate();
        store.create_empty().unwrap();
        assert!(matches!(
            store.commit(&generation, candidate),
            Err(Error::StaleManifest { .. })
        ));
    }

    #[test]
    fn a_candidate_that_fails_validation_leaves_the_file_untouched() {
        let (_directory, store) = store();
        let (mut candidate, generation) = store.load_or_empty().unwrap().into_candidate();
        candidate.insert(record("Alpha", "inspire", "1")).unwrap();
        let generation = store.commit(&generation, candidate).unwrap();
        let before = fs::read_to_string(store.path()).unwrap();

        // Two records claiming one provider identity: caught by validation,
        // before any byte is written.
        let (mut candidate, _) = store.load().unwrap().into_candidate();
        candidate.records_mut().push(record("Zed", "inspire", "1"));
        assert!(matches!(
            store.commit(&generation, candidate),
            Err(Error::Duplicate { .. })
        ));
        assert_eq!(fs::read_to_string(store.path()).unwrap(), before);
    }
}
