//! Global cache eviction.

use crate::{error::Error, path::documents_root};
use std::path::{Path, PathBuf};

/// Whether to report or to delete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanMode {
    /// Report what would be removed, and remove nothing.
    DryRun,
    /// Remove the document cache.
    All,
}

/// What a clean found, and whether it acted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanReport {
    /// The directory that was walked.
    pub root: PathBuf,
    /// Regular files counted.
    pub files: usize,
    /// Directories counted, excluding the root.
    pub directories: usize,
    /// Total bytes of the files counted.
    pub bytes: u64,
    /// Whether anything was actually deleted.
    pub removed: bool,
}

/// Walk, and on [`CleanMode::All`] remove, the document cache.
///
/// Cleaning is global and never consults a manifest: the cache is shared
/// between projects and has no reference counts, so "is anything still using
/// this?" is a question with no answer and eviction is explicit instead.
pub(crate) fn clean(cache_root: &Path, mode: CleanMode) -> Result<CleanReport, Error> {
    let root = documents_root(cache_root);
    let mut report = CleanReport {
        root: root.clone(),
        ..CleanReport::default()
    };
    if !root.exists() {
        return Ok(report);
    }
    walk(&root, &mut report)?;
    if mode == CleanMode::All {
        // Only the subtree bibi created and named is removed, never the cache
        // root itself, which the platform may share with other tools.
        std::fs::remove_dir_all(&root).map_err(|source| Error::io(&root, source))?;
        report.removed = true;
    }
    Ok(report)
}

fn walk(directory: &Path, report: &mut CleanReport) -> Result<(), Error> {
    for entry in std::fs::read_dir(directory).map_err(|source| Error::io(directory, source))? {
        let entry = entry.map_err(|source| Error::io(directory, source))?;
        let path = entry.path();
        // `symlink_metadata` so a symlink is counted as itself rather than
        // followed out of the tree being measured.
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| Error::io(&path, source))?;
        if metadata.is_dir() {
            report.directories += 1;
            walk(&path, report)?;
        } else {
            report.files += 1;
            report.bytes = report.bytes.saturating_add(metadata.len());
        }
    }
    Ok(())
}
