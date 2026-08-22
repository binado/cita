use crate::{
    Error,
    source::{
        DEFAULT_SOURCE_LIMITS, SourceArchiveLimits, cache_entry_exists,
        extract_source_archive_with_limits, invalid_source_archive, publish_source,
        source_cache_path, validate_cached_source,
    },
};
use cita_core::Locator;
use reqwest::StatusCode;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::NamedTempFile;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://arxiv.org/";
const DEFAULT_USER_AGENT: &str = concat!("cita-documents/", env!("CARGO_PKG_VERSION"));
const PDF_SIGNATURE: &[u8] = b"%PDF-";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Controls whether a document fetch may use or update the cache.
pub enum FetchPolicy {
    /// Return a valid cached artifact, otherwise download it.
    #[default]
    UseCache,
    /// Return only a valid cached artifact and never make a network request.
    CacheOnly,
    /// Download the artifact even when a valid cached copy exists.
    Force,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Selects the kind of arXiv artifact to fetch.
pub enum ArtifactKind {
    /// The rendered PDF document.
    #[default]
    Pdf,
    /// The latest gzip-compressed TeX source package, extracted into a directory.
    Source,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Describes whether an artifact was downloaded or found in the cache.
pub enum FetchOutcome {
    /// An artifact was downloaded and atomically stored at this path.
    Downloaded(PathBuf),
    /// A valid artifact was already cached at this path.
    Cached(PathBuf),
}

impl FetchOutcome {
    /// Return the absolute or caller-supplied-root-relative cached path.
    pub fn path(&self) -> &Path {
        match self {
            Self::Downloaded(path) | Self::Cached(path) => path,
        }
    }
}

#[derive(Clone, Debug)]
/// Downloads validated arXiv PDFs and source packages into an atomic local cache.
pub struct DocumentStore {
    cache_root: PathBuf,
    http: reqwest::Client,
    base_url: Url,
    source_limits: SourceArchiveLimits,
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

        let url = pdf_url(&self.base_url, &arxiv_id)?;
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

        let url = source_url(&self.base_url, &arxiv_id)?;
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
                return Err(invalid_source_archive(
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

fn validated_arxiv_id(value: &str) -> Result<String, Error> {
    match format!("arxiv:{value}").parse::<Locator>() {
        Ok(Locator::Arxiv(id)) => Ok(id),
        _ => Err(Error::InvalidArxivIdentifier(value.to_owned())),
    }
}

/// Build the public arXiv PDF URL for a validated identifier.
pub fn arxiv_pdf_url(arxiv_id: &str) -> Result<Url, Error> {
    let arxiv_id = validated_arxiv_id(arxiv_id)?;
    let base_url =
        Url::parse(DEFAULT_BASE_URL).expect("the built-in arXiv base URL must always be valid");
    pdf_url(&base_url, &arxiv_id)
}

fn pdf_cache_path(cache_root: &Path, arxiv_id: &str) -> PathBuf {
    let mut path = cache_root.join("arxiv");
    if let Some((archive, number)) = arxiv_id.split_once('/') {
        path.push(archive);
        path.push(format!("{number}.pdf"));
    } else {
        path.push(format!("{arxiv_id}.pdf"));
    }
    path
}

fn pdf_url(base_url: &Url, arxiv_id: &str) -> Result<Url, Error> {
    artifact_url(base_url, "pdf", arxiv_id)
}

fn source_url(base_url: &Url, arxiv_id: &str) -> Result<Url, Error> {
    artifact_url(base_url, "src", arxiv_id)
}

fn artifact_url(base_url: &Url, endpoint: &str, arxiv_id: &str) -> Result<Url, Error> {
    let mut url = base_url.clone();
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| Error::InvalidBaseUrl(base_url.to_string()))?;
    segments.pop_if_empty();
    segments.push(endpoint);
    for segment in arxiv_id.split('/') {
        segments.push(segment);
    }
    drop(segments);
    Ok(url)
}

fn validate_cached_pdf(path: &Path) -> Result<(), Error> {
    let mut file = fs::File::open(path).map_err(|source| Error::InspectCachedPdf {
        path: path.to_owned(),
        source,
    })?;
    if has_pdf_signature(&mut file).map_err(|source| Error::InspectCachedPdf {
        path: path.to_owned(),
        source,
    })? {
        Ok(())
    } else {
        Err(Error::InvalidCachedPdf(path.to_owned()))
    }
}

fn has_pdf_signature(file: &mut fs::File) -> Result<bool, std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut signature = [0_u8; PDF_SIGNATURE.len()];
    let length = file.read(&mut signature)?;
    Ok(length == PDF_SIGNATURE.len() && signature == PDF_SIGNATURE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{
        MAX_DECOMPRESSED_SOURCE_BYTES, MAX_SOURCE_ARCHIVE_FILES, gzip_raw_tar_entry, gzip_tar,
    };
    use flate2::{Compression, write::GzEncoder};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn server(status: &str, body: &[u8]) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            String::from_utf8_lossy(&request[..length])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        (format!("http://{address}/"), handle)
    }

    #[tokio::test]
    async fn downloads_modern_arxiv_pdf_and_reuses_cache() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-example");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch("1207.7214", FetchPolicy::UseCache)
            .await
            .unwrap();
        let expected = directory.path().join("arxiv/1207.7214.pdf");
        assert_eq!(outcome, FetchOutcome::Downloaded(expected.clone()));
        assert_eq!(fs::read(&expected).unwrap(), b"%PDF-example");
        assert!(handle.join().unwrap().contains("GET /pdf/1207.7214 "));

        assert_eq!(
            store
                .fetch("1207.7214", FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Cached(expected)
        );
    }

    #[tokio::test]
    async fn stores_legacy_arxiv_ids_in_nested_paths() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-legacy");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch("hep-th/9901001", FetchPolicy::UseCache)
            .await
            .unwrap();
        assert_eq!(
            outcome.path(),
            directory.path().join("arxiv/hep-th/9901001.pdf")
        );
        assert!(handle.join().unwrap().contains("GET /pdf/hep-th/9901001 "));
    }

    #[tokio::test]
    async fn extracts_modern_source_packages_and_reuses_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip_tar(&[("main.tex", b"source"), ("figures/plot.dat", b"figure")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let expected = directory.path().join("arxiv/1207.7214/source");
        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Downloaded(expected.clone())
        );
        assert_eq!(fs::read(expected.join("main.tex")).unwrap(), b"source");
        assert_eq!(
            fs::read(expected.join("figures/plot.dat")).unwrap(),
            b"figure"
        );
        assert!(handle.join().unwrap().contains("GET /src/1207.7214 "));

        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Cached(expected)
        );
    }

    #[tokio::test]
    async fn extracts_legacy_source_packages_at_the_legacy_cache_path() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip_tar(&[("paper.tex", b"legacy")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch_artifact(
                "hep-th/9901001",
                ArtifactKind::Source,
                FetchPolicy::UseCache,
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.path(),
            directory.path().join("arxiv/hep-th/9901001/source")
        );
        assert_eq!(
            fs::read(outcome.path().join("paper.tex")).unwrap(),
            b"legacy"
        );
        assert!(handle.join().unwrap().contains("GET /src/hep-th/9901001 "));
    }

    #[tokio::test]
    async fn force_replaces_source_only_after_a_valid_download() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214/source");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("old.tex"), b"old").unwrap();

        let archive = gzip_tar(&[("new.tex", b"new")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await
                .unwrap(),
            FetchOutcome::Downloaded(_)
        ));
        handle.join().unwrap();
        assert!(!destination.join("old.tex").exists());
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");

        let (base_url, handle) = server("200 OK", b"corrupt");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await,
            Err(Error::InvalidSourceArchive { .. })
        ));
        handle.join().unwrap();
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");

        let (base_url, handle) = server("404 Not Found", b"missing");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await,
            Err(Error::SourceUnavailable(_))
        ));
        handle.join().unwrap();
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");
    }

    #[tokio::test]
    async fn source_cache_only_handles_hits_misses_and_invalid_directories() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214/source");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("main.tex"), b"cached").unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();

        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::CacheOnly)
                .await
                .unwrap(),
            FetchOutcome::Cached(destination.clone())
        );
        let missing = directory.path().join("arxiv/2101.00001/source");
        assert!(matches!(
            store
                .fetch_artifact(
                    "2101.00001",
                    ArtifactKind::Source,
                    FetchPolicy::CacheOnly
                )
                .await,
            Err(Error::SourceNotCached(path)) if path == missing
        ));
        fs::remove_file(destination.join("main.tex")).unwrap();
        assert!(matches!(
            store
                .fetch_artifact(
                    "1207.7214",
                    ArtifactKind::Source,
                    FetchPolicy::CacheOnly
                )
                .await,
            Err(Error::InvalidCachedSource(path)) if path == destination
        ));
    }

    #[tokio::test]
    async fn rejects_unavailable_and_malformed_source_responses() {
        type ErrorPredicate = fn(&Error) -> bool;
        let cases: Vec<(&str, Vec<u8>, ErrorPredicate)> = vec![
            ("404 Not Found", b"missing".to_vec(), |error| {
                matches!(error, Error::SourceUnavailable(_))
            }),
            ("200 OK", b"%PDF-only".to_vec(), |error| {
                matches!(error, Error::SourceUnavailable(_))
            }),
            ("200 OK", b"not gzip".to_vec(), |error| {
                matches!(error, Error::InvalidSourceArchive { .. })
            }),
            (
                "200 OK",
                {
                    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                    encoder.write_all(b"not a tar").unwrap();
                    encoder.finish().unwrap()
                },
                |error| matches!(error, Error::InvalidSourceArchive { .. }),
            ),
            ("200 OK", gzip_tar(&[]), |error| {
                matches!(error, Error::InvalidSourceArchive { .. })
            }),
            ("500 Internal Server Error", b"failed".to_vec(), |error| {
                matches!(
                    error,
                    Error::HttpStatus {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        ..
                    }
                )
            }),
        ];

        for (status, body, predicate) in cases {
            let directory = tempfile::tempdir().unwrap();
            let (base_url, handle) = server(status, &body);
            let store = DocumentStore::builder(directory.path())
                .base_url(base_url)
                .build()
                .unwrap();
            let error = store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap_err();
            assert!(predicate(&error), "{error}");
            handle.join().unwrap();
            assert!(!directory.path().join("arxiv/1207.7214/source").exists());
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_source_entries_without_escaping_staging() {
        let cases = [
            gzip_raw_tar_entry(b"../escape.tex", tar::EntryType::Regular, b"escape"),
            gzip_raw_tar_entry(b"/absolute.tex", tar::EntryType::Regular, b"absolute"),
            gzip_raw_tar_entry(b"link", tar::EntryType::Symlink, b""),
            gzip_raw_tar_entry(b"hardlink", tar::EntryType::Link, b""),
            gzip_raw_tar_entry(b"device", tar::EntryType::Char, b""),
            gzip_raw_tar_entry(b"block", tar::EntryType::Block, b""),
            gzip_raw_tar_entry(b"fifo", tar::EntryType::Fifo, b""),
        ];

        for archive in cases {
            let directory = tempfile::tempdir().unwrap();
            let (base_url, handle) = server("200 OK", &archive);
            let store = DocumentStore::builder(directory.path())
                .base_url(base_url)
                .build()
                .unwrap();
            assert!(matches!(
                store
                    .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                    .await,
                Err(Error::InvalidSourceArchive { .. })
            ));
            handle.join().unwrap();
            assert!(!directory.path().join("escape.tex").exists());
            assert!(!directory.path().join("arxiv/1207.7214/source").exists());
        }
    }

    #[tokio::test]
    async fn rejects_oversized_compressed_source_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let body = vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff]
            .into_iter()
            .chain(std::iter::repeat_n(0_u8, 64))
            .collect::<Vec<_>>();
        let (base_url, handle) = server("200 OK", &body);
        let mut store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        store.source_limits = SourceArchiveLimits {
            max_compressed_bytes: 32,
            max_decompressed_bytes: MAX_DECOMPRESSED_SOURCE_BYTES,
            max_files: MAX_SOURCE_ARCHIVE_FILES,
        };
        let error = store
            .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
            .await
            .unwrap_err();
        handle.join().unwrap();
        assert!(matches!(
            error,
            Error::InvalidSourceArchive { reason, .. } if reason.contains("compressed source exceeds size limit")
        ));
        assert!(!directory.path().join("arxiv/1207.7214/source").exists());
    }

    #[tokio::test]
    async fn force_atomically_replaces_cached_pdf() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-old").unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-new");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::Force).await.unwrap(),
            FetchOutcome::Downloaded(_)
        ));
        assert_eq!(fs::read(destination).unwrap(), b"%PDF-new");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn cache_only_reuses_a_valid_pdf_and_rejects_a_cache_miss() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-cached").unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();

        assert_eq!(
            store
                .fetch("1207.7214", FetchPolicy::CacheOnly)
                .await
                .unwrap(),
            FetchOutcome::Cached(destination)
        );

        let missing = directory.path().join("arxiv/2101.00001.pdf");
        assert!(matches!(
            store
                .fetch("2101.00001", FetchPolicy::CacheOnly)
                .await,
            Err(Error::NotCached(path)) if path == missing
        ));
    }

    #[test]
    fn builds_public_arxiv_pdf_urls_for_browser_opening() {
        assert_eq!(
            arxiv_pdf_url("1207.7214").unwrap().as_str(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            arxiv_pdf_url("hep-th/9901001").unwrap().as_str(),
            "https://arxiv.org/pdf/hep-th/9901001"
        );
        assert!(matches!(
            arxiv_pdf_url(""),
            Err(Error::InvalidArxivIdentifier(_))
        ));
    }

    #[tokio::test]
    async fn rejects_missing_invalid_and_non_pdf_documents() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();
        assert!(matches!(
            store.fetch("", FetchPolicy::UseCache).await,
            Err(Error::InvalidArxivIdentifier(_))
        ));
        assert!(matches!(
            store.fetch("not-an-id", FetchPolicy::UseCache).await,
            Err(Error::InvalidArxivIdentifier(_))
        ));

        let (base_url, handle) = server("200 OK", b"not a PDF");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-existing").unwrap();
        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::Force).await,
            Err(Error::InvalidDownloadedPdf(_))
        ));
        assert_eq!(fs::read(destination).unwrap(), b"%PDF-existing");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn reports_http_and_invalid_cached_file_errors() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("404 Not Found", b"missing");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::UseCache).await,
            Err(Error::HttpStatus {
                status: StatusCode::NOT_FOUND,
                ..
            })
        ));
        handle.join().unwrap();

        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"broken").unwrap();
        assert!(matches!(
            store
                .fetch("1207.7214", FetchPolicy::UseCache)
                .await,
            Err(Error::InvalidCachedPdf(path)) if path == destination
        ));
    }
}
