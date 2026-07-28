//! Document store: download and cache arXiv PDFs and source packages.

use std::{fs, io::Write, path::PathBuf, time::Duration};

use reqwest::StatusCode;
use tempfile::NamedTempFile;
use url::Url;

use crate::endpoint::{DEFAULT_BASE_URL, artifact_url, validated_arxiv_id};
use crate::pdf::{PDF_SIGNATURE, has_pdf_signature, pdf_cache_path, validate_cached_pdf};
use crate::source::{
    DEFAULT_SOURCE_LIMITS, SourceArchiveLimits, cache_entry_exists,
    extract_source_archive_with_limits, publish_source, source_cache_path, validate_cached_source,
};
use crate::{ArtifactKind, Error, FetchOutcome, FetchPolicy};

const DEFAULT_USER_AGENT: &str = concat!("cita-documents/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
/// Downloads validated arXiv PDFs and source packages into an atomic local cache.
pub struct DocumentStore {
    cache_root: PathBuf,
    http: reqwest::Client,
    base_url: Url,
    pub(crate) source_limits: SourceArchiveLimits,
}

impl DocumentStore {
    /// Create a store with the default arXiv endpoint and HTTP settings.
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::builder(cache_root).build()
    }

    /// Begin configuring a document store.
    pub fn builder(cache_root: impl Into<PathBuf>) -> DocumentStoreBuilder {
        DocumentStoreBuilder::new(cache_root)
    }

    /// Fetch an arXiv PDF according to the requested cache policy.
    pub async fn fetch(&self, arxiv_id: &str, policy: FetchPolicy) -> Result<FetchOutcome, Error> {
        self.fetch_artifact(arxiv_id, ArtifactKind::Pdf, policy)
            .await
    }

    /// Fetch an arXiv artifact according to the requested cache policy.
    pub async fn fetch_artifact(
        &self,
        arxiv_id: &str,
        kind: ArtifactKind,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error> {
        let arxiv_id = validated_arxiv_id(arxiv_id)?;
        match kind {
            ArtifactKind::Pdf => self.fetch_pdf(arxiv_id, policy).await,
            ArtifactKind::Source => self.fetch_source(arxiv_id, policy).await,
        }
    }

    async fn fetch_pdf(
        &self,
        arxiv_id: String,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error> {
        let destination = pdf_cache_path(&self.cache_root, &arxiv_id);

        if policy != FetchPolicy::Force && destination.exists() {
            validate_cached_pdf(&destination)?;
            return Ok(FetchOutcome::Cached(destination));
        }
        if policy == FetchPolicy::CacheOnly {
            return Err(Error::NotCached(destination));
        }

        let parent = destination
            .parent()
            .expect("a cache destination always has a parent");
        fs::create_dir_all(parent).map_err(|source| Error::CreateCacheDirectory {
            path: parent.to_owned(),
            source,
        })?;

        let url = artifact_url(&self.base_url, ArtifactKind::Pdf, &arxiv_id)?;
        let mut response = self.http.get(url).send().await.map_err(Error::Transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::HttpStatus { arxiv_id, status });
        }

        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::Write {
            path: destination.clone(),
            source,
        })?;
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            temporary.write_all(&chunk).map_err(|source| Error::Write {
                path: destination.clone(),
                source,
            })?;
        }
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| Error::Write {
                path: destination.clone(),
                source,
            })?;
        if !has_pdf_signature(temporary.as_file_mut()).map_err(|source| Error::Write {
            path: destination.clone(),
            source,
        })? {
            return Err(Error::InvalidDownloadedPdf(arxiv_id));
        }
        temporary
            .persist(&destination)
            .map_err(|error| Error::Write {
                path: destination.clone(),
                source: error.error,
            })?;
        Ok(FetchOutcome::Downloaded(destination))
    }

    async fn fetch_source(
        &self,
        arxiv_id: String,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error> {
        let destination = source_cache_path(&self.cache_root, &arxiv_id);
        if policy != FetchPolicy::Force && cache_entry_exists(&destination)? {
            validate_cached_source(&destination)?;
            return Ok(FetchOutcome::Cached(destination));
        }
        if policy == FetchPolicy::CacheOnly {
            return Err(Error::SourceNotCached(destination));
        }

        let parent = destination
            .parent()
            .expect("a source cache destination always has a parent");
        fs::create_dir_all(parent).map_err(|source| Error::CreateCacheDirectory {
            path: parent.to_owned(),
            source,
        })?;

        let url = artifact_url(&self.base_url, ArtifactKind::Source, &arxiv_id)?;
        let mut response = self.http.get(url).send().await.map_err(Error::Transport)?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(Error::SourceUnavailable(arxiv_id));
        }
        if !status.is_success() {
            return Err(Error::HttpStatus { arxiv_id, status });
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            if bytes.len().saturating_add(chunk.len()) > self.source_limits.max_compressed_bytes {
                return Err(crate::source::invalid_source_archive(
                    &arxiv_id,
                    "compressed source exceeds size limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.starts_with(PDF_SIGNATURE) {
            return Err(Error::SourceUnavailable(arxiv_id));
        }

        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(parent)
            .map_err(|source| Error::ExtractSource {
                path: destination.clone(),
                source,
            })?;
        extract_source_archive_with_limits(&bytes, staging.path(), &arxiv_id, self.source_limits)?;
        publish_source(staging, &destination)?;
        Ok(FetchOutcome::Downloaded(destination))
    }
}

/// Build the public arXiv PDF URL for a validated identifier.
pub fn arxiv_pdf_url(arxiv_id: &str) -> Result<Url, Error> {
    let arxiv_id = validated_arxiv_id(arxiv_id)?;
    let base_url =
        Url::parse(DEFAULT_BASE_URL).expect("the built-in arXiv base URL must always be valid");
    artifact_url(&base_url, ArtifactKind::Pdf, &arxiv_id)
}

#[derive(Clone, Debug)]
/// Configures a [`DocumentStore`].
pub struct DocumentStoreBuilder {
    cache_root: PathBuf,
    base_url: String,
    user_agent: String,
    timeout: Duration,
}

impl DocumentStoreBuilder {
    /// Create a builder rooted at the supplied cache directory.
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            base_url: DEFAULT_BASE_URL.into(),
            user_agent: DEFAULT_USER_AGENT.into(),
            timeout: Duration::from_secs(60),
        }
    }

    /// Override the arXiv-compatible base URL.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the HTTP user-agent header.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Override the request timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Validate the configuration and construct the store.
    pub fn build(self) -> Result<DocumentStore, Error> {
        let mut base_url =
            Url::parse(&self.base_url).map_err(|_| Error::InvalidBaseUrl(self.base_url.clone()))?;
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(Error::Transport)?;
        Ok(DocumentStore {
            cache_root: self.cache_root,
            http,
            base_url,
            source_limits: DEFAULT_SOURCE_LIMITS,
        })
    }
}
