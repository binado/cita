//! Choosing which manifest a command acts on.

use bibi_manifest::ManifestStore;
use std::path::{Path, PathBuf};

/// The manifest name bibi looks for in the working directory.
pub const MANIFEST_NAME: &str = "bibi.toml";

/// Resolves a selection to exactly one manifest store.
#[derive(Clone, Debug)]
pub struct TargetResolver {
    working_directory: PathBuf,
}

impl TargetResolver {
    /// Build a resolver for one working directory.
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
        }
    }

    /// Resolve the target: an explicit path, or `./bibi.toml`.
    ///
    /// There is no upward search and no user-level manifest. The target is
    /// always evident from where the command was run, which removes an entire
    /// class of questions — where a walk stops, which ancestor wins, whether a
    /// command run deep inside an unrelated repository quietly targets that
    /// repository's root, whether this project or the user's library was meant.
    ///
    /// Either way the parent directory must already exist, so a typo produces a
    /// diagnostic rather than a directory tree.
    pub fn resolve(&self, path: Option<&Path>) -> ManifestStore {
        match path {
            Some(path) => ManifestStore::new(self.absolute(path)),
            None => ManifestStore::new(self.working_directory.join(MANIFEST_NAME)),
        }
    }

    /// Resolve an input path the user's shell just completed.
    ///
    /// Inputs are working-directory-relative while outputs are
    /// manifest-relative. Resolving a tab-completed input against some other
    /// directory is surprising in a way the output rule is not.
    pub fn input(&self, path: &Path) -> PathBuf {
        self.absolute(path)
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.working_directory.join(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(working: &Path) -> TargetResolver {
        TargetResolver::new(working)
    }

    #[test]
    fn the_working_directory_manifest_is_the_default() {
        let resolver = resolver(Path::new("/work/project"));
        let store = resolver.resolve(None);
        assert_eq!(store.path(), Path::new("/work/project/bibi.toml"));
    }

    #[test]
    fn an_explicit_path_wins_and_stays_where_it_was_written() {
        let resolver = resolver(Path::new("/work/project"));
        let store = resolver.resolve(Some(Path::new("other/bibi.toml")));
        assert_eq!(store.path(), Path::new("/work/project/other/bibi.toml"));
        assert_eq!(store.directory(), Path::new("/work/project/other"));
    }

    #[test]
    fn there_is_no_upward_search() {
        // Standing in a subdirectory of a project targets the subdirectory.
        let resolver = resolver(Path::new("/work/project/chapters"));
        let store = resolver.resolve(None);
        assert_eq!(store.path(), Path::new("/work/project/chapters/bibi.toml"));
    }

    #[test]
    fn inputs_resolve_against_the_working_directory() {
        let resolver = resolver(Path::new("/work/project"));
        assert_eq!(
            resolver.input(Path::new("colleague.bib")),
            Path::new("/work/project/colleague.bib")
        );
        assert_eq!(
            resolver.input(Path::new("/tmp/colleague.bib")),
            Path::new("/tmp/colleague.bib")
        );
    }
}
