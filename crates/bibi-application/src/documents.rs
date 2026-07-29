//! Retrieving a record's documents, and evicting the cache.

use crate::{error::Error, services::Services};
use bibi_core::Selector;
use bibi_documents::{ArtifactKind, CleanMode, CleanReport, FetchOutcome, FetchPolicy};
use bibi_manifest::ManifestStore;

/// Options for `fetch`.
#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
    /// The record to fetch for.
    pub selector: String,
    /// Fetch the source package instead of the PDF.
    pub source: bool,
    /// Report the public URL instead of downloading anything.
    pub url: bool,
    /// Download again, replacing the cached artifact.
    pub force: bool,
}

/// What `fetch` produced.
#[derive(Clone, Debug)]
pub enum FetchTarget {
    /// A cached artifact, already present.
    Cached(std::path::PathBuf),
    /// A cached artifact, downloaded now.
    Downloaded(std::path::PathBuf),
    /// A public URL, with nothing downloaded.
    Url(String),
}

impl FetchTarget {
    /// The single line the command writes to stdout.
    ///
    /// One value, so that `open $(bibi fetch <selector>)` works.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Cached(path) | Self::Downloaded(path) => path.to_string_lossy(),
            Self::Url(url) => std::borrow::Cow::Borrowed(url),
        }
    }
}

/// Resolve a record and produce a path or URL for its document.
///
/// The cache key is computed from the record's arXiv identifier every time.
/// Nothing in the manifest refers to a cache path, so renaming, removing, or
/// migrating a record needs no document bookkeeping at all.
pub async fn fetch(
    services: &Services,
    store: &ManifestStore,
    request: &FetchRequest,
) -> Result<FetchTarget, Error> {
    if request.source && request.url {
        return Err(Error::usage(
            "`--url` names the published PDF, so it cannot be combined with `--source`",
        ));
    }
    let manifest = store.load()?.manifest;
    let record = manifest.resolve(&Selector::parse(&request.selector)?)?;
    let arxiv = record.identifiers.arxiv.clone().ok_or_else(|| {
        Error::usage(format!(
            "`{}` has no arXiv identifier, and arXiv is the only document source in this version",
            record.key
        ))
    })?;
    let documents = services.documents()?;

    if request.url {
        return Ok(FetchTarget::Url(documents.pdf_url(&arxiv)?.to_string()));
    }
    let kind = if request.source {
        ArtifactKind::Source
    } else {
        ArtifactKind::Pdf
    };
    let policy = if request.force {
        FetchPolicy::Force
    } else {
        FetchPolicy::UseCache
    };
    Ok(match documents.fetch(&arxiv, kind, policy).await? {
        FetchOutcome::Cached(path) => FetchTarget::Cached(path),
        FetchOutcome::Downloaded(path) => FetchTarget::Downloaded(path),
    })
}

/// Report or evict the global document cache.
///
/// This resolves no manifest: the cache belongs to the machine rather than to
/// any project, and removing a record never evicted anything from it.
pub fn clean_cache(services: &Services, mode: CleanMode) -> Result<CleanReport, Error> {
    Ok(services.documents()?.clean(mode)?)
}
