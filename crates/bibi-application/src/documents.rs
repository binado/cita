//! Acquiring a record's document.

use crate::{error::Error, services::Services};
use bibi_core::Selector;
use bibi_documents::{ArtifactKind, default_filename};
use bibi_manifest::ManifestStore;
use std::path::{Path, PathBuf};

/// Options for `fetch`.
#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
    /// The record to fetch for.
    pub selector: String,
    /// Fetch the source archive instead of the PDF.
    pub source: bool,
    /// Report the public URL instead of downloading anything.
    pub url: bool,
    /// Download here instead of to the default name. Never overwritten.
    pub output: Option<PathBuf>,
    /// The directory a default name resolves against.
    pub working_directory: PathBuf,
}

/// What `fetch` produced.
#[derive(Clone, Debug)]
pub enum FetchTarget {
    /// An artifact downloaded now.
    Downloaded(PathBuf),
    /// An artifact already at the default destination, which was not replaced.
    Present(PathBuf),
    /// A public URL, with nothing downloaded.
    Url(String),
}

impl FetchTarget {
    /// The single line the command writes to stdout.
    ///
    /// One value, so that `open $(bibi fetch <selector>)` works.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Downloaded(path) | Self::Present(path) => path.to_string_lossy(),
            Self::Url(url) => std::borrow::Cow::Borrowed(url),
        }
    }
}

/// Resolve a record and acquire its document, or report where to find it.
///
/// `fetch` retrieves arXiv artifacts and nothing else. A record carrying no
/// arXiv identifier fails and says so: bibi does not follow a DOI to a
/// publisher. Keeping the command pointed at one artifact service is what stops
/// it from becoming a general document acquisition layer.
///
/// `keep_existing` reports rather than fails when the default destination is
/// already occupied, which is what lets `--open` open a file a previous fetch
/// downloaded. An explicit `--output` is never treated this way: the caller
/// named that path, so silently accepting whatever is already there would be a
/// guess about what they meant.
pub async fn fetch(
    services: &Services,
    store: &ManifestStore,
    request: &FetchRequest,
    keep_existing: bool,
) -> Result<FetchTarget, Error> {
    let manifest = store.load()?.manifest;
    let record = manifest.resolve(&Selector::parse(&request.selector)?)?;
    let arxiv = record.identifiers.arxiv.clone().ok_or_else(|| {
        Error::usage(format!(
            "`{}` has no arXiv identifier, and arXiv is the only document source",
            record.key
        ))
    })?;
    let kind = if request.source {
        ArtifactKind::Source
    } else {
        ArtifactKind::Pdf
    };

    // A URL is a function of the identifier and arXiv's public address alone,
    // so reporting one must not require building a client.
    if request.url {
        return Ok(FetchTarget::Url(
            bibi_documents::public_url(&arxiv, kind)?.to_string(),
        ));
    }

    let (destination, named) = match &request.output {
        Some(path) => (absolute(&request.working_directory, path), true),
        None => (
            request
                .working_directory
                .join(default_filename(&arxiv, kind)),
            false,
        ),
    };
    if !named && keep_existing && destination.exists() {
        return Ok(FetchTarget::Present(destination));
    }
    services
        .documents()?
        .download(&arxiv, kind, &destination)
        .await?;
    Ok(FetchTarget::Downloaded(destination))
}

fn absolute(working_directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    }
}
