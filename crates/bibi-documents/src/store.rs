//! Downloading, publishing, and reading back cached artifacts.

use crate::{
    clean::{CleanMode, CleanReport, clean},
    error::Error,
    path::{ArtifactKind, artifact_path, validate_root},
    source::{ArchiveLimits, extract},
};
use bibi_core::ArxivId;
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://arxiv.org/";
const DEFAULT_USER_AGENT: &str = concat!("bibi-documents/", env!("CARGO_PKG_VERSION"));
const DEFAULT_MAX_PDF_BYTES: usize = 256 * 1024 * 1024;
const PDF_SIGNATURE: &[u8] = b"%PDF-";

/// Whether a cached copy may be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchPolicy {
    /// Use the cache when it holds a usable artifact.
    UseCache,
    /// Download again and replace the cached artifact.
    Force,
}

/// Where the artifact is, and whether this call put it there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchOutcome {
    /// It was already cached.
    Cached(PathBuf),
    /// It was downloaded now.
    Downloaded(PathBuf),
}

impl FetchOutcome {
    /// The absolute path to the artifact.
    pub fn path(&self) -> &Path {
        match self {
            Self::Cached(path) | Self::Downloaded(path) => path,
        }
    }
}

/// The global, derived, disposable document cache.
#[derive(Clone, Debug)]
pub struct DocumentStore {
    cache_root: PathBuf,
    base_url: Url,
    http: reqwest::Client,
    limits: ArchiveLimits,
    max_pdf_bytes: usize,
}

impl DocumentStore {
    /// Open a store over a cache root, refusing a root unsafe to own.
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::builder(cache_root).build()
    }

    /// Begin configuring a store.
    pub fn builder(cache_root: impl Into<PathBuf>) -> DocumentStoreBuilder {
        DocumentStoreBuilder {
            cache_root: cache_root.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(60),
            limits: ArchiveLimits::default(),
            max_pdf_bytes: DEFAULT_MAX_PDF_BYTES,
        }
    }

    /// Where this store keeps its artifacts.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Where one artifact would be cached, whether or not it is.
    pub fn path_for(&self, id: &ArxivId, kind: ArtifactKind) -> PathBuf {
        artifact_path(&self.cache_root, id, kind)
    }

    /// The public URL of a work's PDF, for opening in a browser.
    pub fn pdf_url(&self, id: &ArxivId) -> Result<Url, Error> {
        artifact_url(&self.base_url, "pdf", id)
    }

    /// Fetch an artifact, using the cache unless told to replace it.
    ///
    /// Nothing else in bibi calls this: sync never touches the document cache,
    /// including `sync --force`, because a metadata refresh and a download are
    /// different operations with different costs.
    pub async fn fetch(
        &self,
        id: &ArxivId,
        kind: ArtifactKind,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error> {
        let path = self.path_for(id, kind);
        if policy == FetchPolicy::UseCache
            && let Some(cached) = self.usable(&path, kind)?
        {
            return Ok(FetchOutcome::Cached(cached));
        }
        let endpoint = match kind {
            ArtifactKind::Pdf => "pdf",
            ArtifactKind::Source => "e-print",
        };
        let url = artifact_url(&self.base_url, endpoint, id)?;
        let bytes = self.download(&url, id, kind).await?;
        match kind {
            ArtifactKind::Pdf => self.publish_pdf(&path, &bytes, id)?,
            ArtifactKind::Source => self.publish_source(&path, &bytes, id)?,
        }
        Ok(FetchOutcome::Downloaded(path))
    }

    /// Report or evict the whole document cache.
    pub fn clean(&self, mode: CleanMode) -> Result<CleanReport, Error> {
        clean(&self.cache_root, mode)
    }

    /// A cached artifact, if there is a usable one.
    fn usable(&self, path: &Path, kind: ArtifactKind) -> Result<Option<PathBuf>, Error> {
        if !path.exists() {
            return Ok(None);
        }
        match kind {
            ArtifactKind::Pdf => {
                let mut signature = [0u8; PDF_SIGNATURE.len()];
                let read = {
                    use std::io::Read;
                    let mut file =
                        std::fs::File::open(path).map_err(|source| Error::io(path, source))?;
                    file.read(&mut signature)
                        .map_err(|source| Error::io(path, source))?
                };
                if read != PDF_SIGNATURE.len() || signature != PDF_SIGNATURE {
                    return Err(Error::InvalidCacheEntry {
                        path: path.to_path_buf(),
                    });
                }
            }
            ArtifactKind::Source => {
                if !path.is_dir() || std::fs::read_dir(path).is_err() {
                    return Err(Error::InvalidCacheEntry {
                        path: path.to_path_buf(),
                    });
                }
            }
        }
        Ok(Some(path.to_path_buf()))
    }

    async fn download(
        &self,
        url: &Url,
        id: &ArxivId,
        kind: ArtifactKind,
    ) -> Result<Vec<u8>, Error> {
        let mut response =
            self.http
                .get(url.clone())
                .send()
                .await
                .map_err(|source| Error::Download {
                    url: url.to_string(),
                    source,
                })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound {
                id: id.to_string(),
                kind: kind.label(),
            });
        }
        if !status.is_success() {
            return Err(Error::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        let limit = match kind {
            ArtifactKind::Pdf => self.max_pdf_bytes,
            ArtifactKind::Source => self.limits.max_compressed_bytes,
        };
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(Error::ArtifactTooLarge {
                id: id.to_string(),
                kind: kind.label(),
                limit,
            });
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|source| Error::Download {
            url: url.to_string(),
            source,
        })? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > limit)
            {
                return Err(Error::ArtifactTooLarge {
                    id: id.to_string(),
                    kind: kind.label(),
                    limit,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Publish a PDF: validate, write beside the destination, then rename.
    fn publish_pdf(&self, path: &Path, bytes: &[u8], id: &ArxivId) -> Result<(), Error> {
        if !bytes.starts_with(PDF_SIGNATURE) {
            // arXiv answers with an HTML holding page for withdrawn or
            // not-yet-processed works, and that is not a PDF.
            return Err(Error::InvalidArtifact {
                id: id.to_string(),
                reason: "the response is not a PDF".into(),
            });
        }
        let parent = parent_of(path)?;
        std::fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|source| Error::io(parent, source))?;
        temporary
            .write_all(bytes)
            .map_err(|source| Error::io(path, source))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| Error::io(path, source))?;
        temporary
            .persist(path)
            .map_err(|error| Error::io(path, error.error))?;
        Ok(())
    }

    /// Publish a source tree: extract fully beside the destination, then swap.
    ///
    /// Only a complete tree is ever published. Directory replacement is not one
    /// atomic step, so a crash mid-swap can leave the artifact absent — which
    /// for a derived, disposable cache means re-fetching, not corruption.
    fn publish_source(&self, path: &Path, bytes: &[u8], id: &ArxivId) -> Result<(), Error> {
        let parent = parent_of(path)?;
        std::fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        let staging = tempfile::tempdir_in(parent).map_err(|source| Error::io(parent, source))?;
        extract(bytes, staging.path(), id.as_str(), self.limits)?;

        let previous = path.with_extension("replaced");
        let had_previous = path.exists();
        if had_previous {
            let _ = std::fs::remove_dir_all(&previous);
            std::fs::rename(path, &previous).map_err(|source| Error::io(path, source))?;
        }
        let staged = staging.keep();
        match std::fs::rename(&staged, path) {
            Ok(()) => {
                if had_previous {
                    let _ = std::fs::remove_dir_all(&previous);
                }
                Ok(())
            }
            Err(source) => {
                // Put the previous tree back rather than leaving nothing.
                let _ = std::fs::remove_dir_all(&staged);
                if had_previous {
                    let _ = std::fs::rename(&previous, path);
                }
                Err(Error::io(path, source))
            }
        }
    }
}

fn parent_of(path: &Path) -> Result<&Path, Error> {
    path.parent().ok_or_else(|| Error::UnsafeRoot {
        path: path.to_path_buf(),
        reason: "an artifact path always has a parent",
    })
}

/// `<base>/<endpoint>/<id>`, with the identifier's own components appended.
fn artifact_url(base_url: &Url, endpoint: &str, id: &ArxivId) -> Result<Url, Error> {
    let mut url = base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::InvalidBaseUrl(base_url.to_string()))?;
        segments.pop_if_empty();
        segments.push(endpoint);
        for segment in id.as_str().split('/') {
            segments.push(segment);
        }
    }
    Ok(url)
}

/// Configures a [`DocumentStore`].
#[derive(Clone, Debug)]
pub struct DocumentStoreBuilder {
    cache_root: PathBuf,
    base_url: String,
    user_agent: String,
    timeout: Duration,
    limits: ArchiveLimits,
    max_pdf_bytes: usize,
}

impl DocumentStoreBuilder {
    /// Point the store at another arXiv-compatible base URL.
    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    /// Override the user agent.
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }

    /// Override the request timeout.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }

    /// Override the archive bounds.
    pub fn limits(mut self, limits: ArchiveLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Override the largest PDF response accepted.
    pub fn max_pdf_bytes(mut self, max_pdf_bytes: usize) -> Self {
        self.max_pdf_bytes = max_pdf_bytes;
        self
    }

    /// Validate the configuration and build the store.
    pub fn build(self) -> Result<DocumentStore, Error> {
        validate_root(&self.cache_root)?;
        let mut base_url =
            Url::parse(&self.base_url).map_err(|_| Error::InvalidBaseUrl(self.base_url.clone()))?;
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(Error::Client)?;
        Ok(DocumentStore {
            cache_root: self.cache_root,
            base_url,
            http,
            limits: self.limits,
            max_pdf_bytes: self.max_pdf_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_public_urls_a_browser_can_open() {
        let store = DocumentStore::new("/tmp/bibi-cache").unwrap();
        assert_eq!(
            store
                .pdf_url(&ArxivId::new("1207.7214v2").unwrap())
                .unwrap()
                .as_str(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            store
                .pdf_url(&ArxivId::new("hep-th/9901001").unwrap())
                .unwrap()
                .as_str(),
            "https://arxiv.org/pdf/hep-th/9901001"
        );
    }

    #[test]
    fn refuses_a_cache_root_it_must_not_be_allowed_to_delete() {
        assert!(DocumentStore::new("/").is_err());
        assert!(DocumentStore::new("").is_err());
    }
}
