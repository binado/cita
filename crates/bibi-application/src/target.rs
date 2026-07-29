//! Choosing which manifest a command acts on.

use crate::error::Error;
use bibi_manifest::ManifestStore;
use std::path::{Path, PathBuf};

/// The platform locations bibi uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformPaths {
    global_manifest: PathBuf,
    cache_root: PathBuf,
}

/// The environment variable that overrides the global manifest's location.
pub const GLOBAL_MANIFEST_ENV: &str = "BIBI_GLOBAL_MANIFEST";

/// The environment variable that overrides the document cache's location.
pub const CACHE_ROOT_ENV: &str = "BIBI_CACHE_ROOT";

impl PlatformPaths {
    /// Discover the platform's directories.
    ///
    /// The global manifest lives beside *configuration* rather than in a data
    /// directory: it is authored, reviewable, hand-editable content that users
    /// will want to symlink into dotfiles, not derived state. The document
    /// cache is derived and disposable, so it lives in the cache directory.
    pub fn discover() -> Result<Self, Error> {
        let directories = directories::ProjectDirs::from("", "", "bibi")
            .ok_or(Error::NoPlatformDirectory { what: "config" })?;
        let global_manifest = match std::env::var_os(GLOBAL_MANIFEST_ENV) {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => directories.config_dir().join("bibi.toml"),
        };
        let cache_root = match std::env::var_os(CACHE_ROOT_ENV) {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => directories.cache_dir().to_path_buf(),
        };
        Ok(Self {
            global_manifest,
            cache_root,
        })
    }

    /// Build fixed paths, for tests and for callers that already know them.
    pub fn new(global_manifest: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            global_manifest: global_manifest.into(),
            cache_root: cache_root.into(),
        }
    }

    /// Where the user-level manifest lives.
    pub fn global_manifest(&self) -> &Path {
        &self.global_manifest
    }

    /// Where downloaded documents are cached, for every project alike.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }
}

/// Which manifest the user asked for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetSelection {
    /// An explicit manifest path.
    pub path: Option<PathBuf>,
    /// The user-level manifest.
    pub global: bool,
}

/// The manifest name bibi looks for in the working directory.
pub const MANIFEST_NAME: &str = "bibi.toml";

/// Resolves a selection to exactly one manifest store.
#[derive(Clone, Debug)]
pub struct TargetResolver {
    paths: PlatformPaths,
    working_directory: PathBuf,
}

impl TargetResolver {
    /// Build a resolver for one working directory.
    pub fn new(paths: PlatformPaths, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            working_directory: working_directory.into(),
        }
    }

    /// Resolve the target, by the first rule that applies.
    ///
    /// There is no upward search. The target is always evident from where the
    /// command was run, which removes an entire class of questions — where a
    /// walk stops, which ancestor wins, whether a command run deep inside an
    /// unrelated repository quietly targets that repository's root.
    pub fn resolve(&self, selection: &TargetSelection) -> Result<ManifestStore, Error> {
        match (&selection.path, selection.global) {
            (Some(_), true) => Err(Error::usage(
                "`--path` and `--global` both select a target; give only one",
            )),
            // An explicit path is the user's own; its parent must exist, so a
            // typo produces a diagnostic rather than a directory tree.
            (Some(path), false) => Ok(ManifestStore::new(self.absolute(path))),
            // The fixed platform location is bibi's own, so it may be created.
            (None, true) => Ok(ManifestStore::creating_parents(
                self.paths.global_manifest(),
            )),
            (None, false) => Ok(ManifestStore::new(
                self.working_directory.join(MANIFEST_NAME),
            )),
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

    /// The platform locations this resolver was built with.
    pub fn paths(&self) -> &PlatformPaths {
        &self.paths
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.working_directory.join(path)
        }
    }
}

/// Resolve an output path against the manifest's own directory.
///
/// With `-p other/bibi.toml`, a rendered bibliography is written beside that
/// manifest rather than beside the shell's working directory.
pub fn output(store: &ManifestStore, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        store.directory().join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(working: &Path) -> TargetResolver {
        TargetResolver::new(
            PlatformPaths::new("/platform/config/bibi/bibi.toml", "/platform/cache/bibi"),
            working,
        )
    }

    #[test]
    fn the_working_directory_manifest_is_the_default() {
        let resolver = resolver(Path::new("/work/project"));
        let store = resolver.resolve(&TargetSelection::default()).unwrap();
        assert_eq!(store.path(), Path::new("/work/project/bibi.toml"));
    }

    #[test]
    fn an_explicit_path_wins_and_stays_where_it_was_written() {
        let resolver = resolver(Path::new("/work/project"));
        let store = resolver
            .resolve(&TargetSelection {
                path: Some(PathBuf::from("other/bibi.toml")),
                global: false,
            })
            .unwrap();
        assert_eq!(store.path(), Path::new("/work/project/other/bibi.toml"));
        assert_eq!(store.directory(), Path::new("/work/project/other"));
        // Outputs follow the manifest, not the shell.
        assert_eq!(
            output(&store, Path::new("references.bib")),
            Path::new("/work/project/other/references.bib")
        );
    }

    #[test]
    fn the_global_flag_selects_the_platform_manifest() {
        let resolver = resolver(Path::new("/work/project"));
        let store = resolver
            .resolve(&TargetSelection {
                path: None,
                global: true,
            })
            .unwrap();
        assert_eq!(store.path(), Path::new("/platform/config/bibi/bibi.toml"));
    }

    #[test]
    fn path_and_global_are_two_spellings_of_one_thing() {
        let resolver = resolver(Path::new("/work/project"));
        assert!(matches!(
            resolver.resolve(&TargetSelection {
                path: Some(PathBuf::from("bibi.toml")),
                global: true,
            }),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn there_is_no_upward_search() {
        // Standing in a subdirectory of a project targets the subdirectory.
        let resolver = resolver(Path::new("/work/project/chapters"));
        let store = resolver.resolve(&TargetSelection::default()).unwrap();
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
