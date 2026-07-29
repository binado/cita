//! Where an artifact lives, and which roots bibi is willing to own.

use crate::error::Error;
use bibi_core::ArxivId;
use std::path::{Path, PathBuf};

/// Everything bibi caches lives under this subtree of the cache root.
///
/// Naming it explicitly is what makes `cache clean` safe: bibi removes a
/// directory it created and named, never whatever the root happens to contain.
pub const DOCUMENTS: &str = "documents";
const ARXIV: &str = "arxiv";
const PDF_NAME: &str = "paper.pdf";
const SOURCE_NAME: &str = "source";

/// Which artifact of a work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactKind {
    /// The typeset PDF.
    Pdf,
    /// The extracted source package.
    Source,
}

impl ArtifactKind {
    /// A word for diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Source => "source",
        }
    }
}

/// The document root inside a cache root.
pub(crate) fn documents_root(cache_root: &Path) -> PathBuf {
    cache_root.join(DOCUMENTS)
}

/// Where one artifact is cached.
///
/// Addressed by identifier and kind, never by record identity: two projects
/// citing one paper then share a copy, and a cached file is meaningful on its
/// own rather than only in the company of the manifest that asked for it.
///
/// A legacy identifier contributes two path components. That is safe only
/// because [`ArxivId`] has already validated both — no caller-supplied string
/// is ever joined directly.
pub(crate) fn artifact_path(cache_root: &Path, id: &ArxivId, kind: ArtifactKind) -> PathBuf {
    let mut path = documents_root(cache_root).join(ARXIV);
    match id.legacy_parts() {
        Some((archive, number)) => {
            path.push(archive);
            path.push(number);
        }
        None => path.push(id.as_str()),
    }
    path.push(match kind {
        ArtifactKind::Pdf => PDF_NAME,
        ArtifactKind::Source => SOURCE_NAME,
    });
    path
}

/// Refuse a cache root bibi must not be allowed to delete.
///
/// `cache clean --all` removes the document subtree of this root, so a root
/// that is empty, the filesystem root, or a home directory would turn a cache
/// eviction into something else entirely.
pub(crate) fn validate_root(cache_root: &Path) -> Result<(), Error> {
    let reject = |reason| {
        Err(Error::UnsafeRoot {
            path: cache_root.to_path_buf(),
            reason,
        })
    };
    if cache_root.as_os_str().is_empty() {
        return reject("the path is empty");
    }
    if cache_root.parent().is_none() {
        return reject("it is a filesystem root");
    }
    // At least two named components, so a cache root is somewhere like
    // `~/.cache/bibi` rather than a shared top-level directory.
    if cache_root
        .components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count()
        < 2
    {
        return reject("it is too close to the filesystem root");
    }
    for variable in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(variable)
            && !home.is_empty()
            && Path::new(&home) == cache_root
        {
            return reject("it is a home directory");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arxiv(value: &str) -> ArxivId {
        ArxivId::new(value).unwrap()
    }

    #[test]
    fn a_modern_identifier_is_one_directory() {
        assert_eq!(
            artifact_path(
                Path::new("/cache"),
                &arxiv("1207.7214v2"),
                ArtifactKind::Pdf
            ),
            Path::new("/cache/documents/arxiv/1207.7214/paper.pdf")
        );
        assert_eq!(
            artifact_path(
                Path::new("/cache"),
                &arxiv("1207.7214"),
                ArtifactKind::Source
            ),
            Path::new("/cache/documents/arxiv/1207.7214/source")
        );
    }

    #[test]
    fn a_legacy_identifier_keeps_its_archive_directory() {
        assert_eq!(
            artifact_path(
                Path::new("/cache"),
                &arxiv("hep-th/9901001"),
                ArtifactKind::Pdf
            ),
            Path::new("/cache/documents/arxiv/hep-th/9901001/paper.pdf")
        );
        assert_eq!(
            artifact_path(
                Path::new("/cache"),
                &arxiv("HEP-TH/9901001v3"),
                ArtifactKind::Source
            ),
            Path::new("/cache/documents/arxiv/hep-th/9901001/source")
        );
    }

    #[test]
    fn dangerous_roots_are_refused() {
        assert!(validate_root(Path::new("")).is_err());
        assert!(validate_root(Path::new("/")).is_err());
        assert!(validate_root(Path::new("/tmp")).is_err());
        assert!(validate_root(Path::new("/tmp/bibi-cache")).is_ok());
    }

    #[test]
    fn a_home_directory_is_refused() {
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
        let Some(home) = home else {
            return;
        };
        assert!(validate_root(Path::new(&home)).is_err());
        assert!(validate_root(&Path::new(&home).join("Library/Caches/bibi")).is_ok());
    }
}
