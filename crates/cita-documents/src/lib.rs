//! arXiv PDF download and local cache management for Cita.

use cita_core::{Locator, PaperRecord};
use reqwest::StatusCode;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::NamedTempFile;
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://arxiv.org/";
const DEFAULT_USER_AGENT: &str = concat!("cita-documents/", env!("CARGO_PKG_VERSION"));
const PDF_SIGNATURE: &[u8] = b"%PDF-";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FetchPolicy {
    #[default]
    UseCache,
    CacheOnly,
    Force,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    Downloaded(PathBuf),
    Cached(PathBuf),
}

impl FetchOutcome {
    pub fn path(&self) -> &Path {
        match self {
            Self::Downloaded(path) | Self::Cached(path) => path,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DocumentStore {
    cache_root: PathBuf,
    http: reqwest::Client,
    base_url: Url,
}

impl DocumentStore {
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::builder(cache_root).build()
    }

    pub fn builder(cache_root: impl Into<PathBuf>) -> DocumentStoreBuilder {
        DocumentStoreBuilder::new(cache_root)
    }

    pub async fn fetch(
        &self,
        paper: &PaperRecord,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error> {
        let arxiv_id = paper
            .arxiv_ids
            .first()
            .ok_or(Error::MissingArxivIdentifier)?;
        let arxiv_id = validated_arxiv_id(arxiv_id)?;
        let destination = cache_path(&self.cache_root, &arxiv_id);

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
}

#[derive(Clone, Debug)]
pub struct DocumentStoreBuilder {
    cache_root: PathBuf,
    base_url: String,
    user_agent: String,
    timeout: Duration,
}

impl DocumentStoreBuilder {
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            base_url: DEFAULT_BASE_URL.into(),
            user_agent: DEFAULT_USER_AGENT.into(),
            timeout: Duration::from_secs(60),
        }
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

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
        })
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("paper has no arXiv identifier, so no PDF can be fetched")]
    MissingArxivIdentifier,
    #[error("paper contains an invalid arXiv identifier: `{0}`")]
    InvalidArxivIdentifier(String),
    #[error("invalid arXiv base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("could not create document cache directory {path}: {source}")]
    CreateCacheDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not inspect cached PDF {path}: {source}")]
    InspectCachedPdf {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cached file {0} is not a valid PDF")]
    InvalidCachedPdf(PathBuf),
    #[error("PDF is not cached at {0}")]
    NotCached(PathBuf),
    #[error("arXiv request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("arXiv returned HTTP {status} for `{arxiv_id}`")]
    HttpStatus {
        arxiv_id: String,
        status: StatusCode,
    },
    #[error("arXiv returned non-PDF content for `{0}`")]
    InvalidDownloadedPdf(String),
    #[error("could not write PDF cache file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn validated_arxiv_id(value: &str) -> Result<String, Error> {
    match format!("arxiv:{value}").parse::<Locator>() {
        Ok(Locator::Arxiv(id)) => Ok(id),
        _ => Err(Error::InvalidArxivIdentifier(value.to_owned())),
    }
}

pub fn arxiv_pdf_url(paper: &PaperRecord) -> Result<Url, Error> {
    let arxiv_id = paper
        .arxiv_ids
        .first()
        .ok_or(Error::MissingArxivIdentifier)?;
    let arxiv_id = validated_arxiv_id(arxiv_id)?;
    let base_url =
        Url::parse(DEFAULT_BASE_URL).expect("the built-in arXiv base URL must always be valid");
    pdf_url(&base_url, &arxiv_id)
}

fn cache_path(cache_root: &Path, arxiv_id: &str) -> PathBuf {
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
    let mut url = base_url.clone();
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| Error::InvalidBaseUrl(base_url.to_string()))?;
    segments.pop_if_empty();
    segments.push("pdf");
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
    use cita_core::INSPIRE_SOURCE;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn paper(arxiv_ids: &[&str]) -> PaperRecord {
        PaperRecord {
            title: "Example".into(),
            source: INSPIRE_SOURCE.into(),
            arxiv_ids: arxiv_ids.iter().map(|id| (*id).to_owned()).collect(),
            ..PaperRecord::default()
        }
    }

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
            .fetch(&paper(&["1207.7214"]), FetchPolicy::UseCache)
            .await
            .unwrap();
        let expected = directory.path().join("arxiv/1207.7214.pdf");
        assert_eq!(outcome, FetchOutcome::Downloaded(expected.clone()));
        assert_eq!(fs::read(&expected).unwrap(), b"%PDF-example");
        assert!(handle.join().unwrap().contains("GET /pdf/1207.7214 "));

        assert_eq!(
            store
                .fetch(&paper(&["1207.7214"]), FetchPolicy::UseCache)
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
            .fetch(&paper(&["hep-th/9901001"]), FetchPolicy::UseCache)
            .await
            .unwrap();
        assert_eq!(
            outcome.path(),
            directory.path().join("arxiv/hep-th/9901001.pdf")
        );
        assert!(handle.join().unwrap().contains("GET /pdf/hep-th/9901001 "));
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
            store
                .fetch(&paper(&["1207.7214"]), FetchPolicy::Force)
                .await
                .unwrap(),
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
                .fetch(&paper(&["1207.7214"]), FetchPolicy::CacheOnly)
                .await
                .unwrap(),
            FetchOutcome::Cached(destination)
        );

        let missing = directory.path().join("arxiv/2101.00001.pdf");
        assert!(matches!(
            store
                .fetch(&paper(&["2101.00001"]), FetchPolicy::CacheOnly)
                .await,
            Err(Error::NotCached(path)) if path == missing
        ));
    }

    #[test]
    fn builds_public_arxiv_pdf_urls_for_browser_opening() {
        assert_eq!(
            arxiv_pdf_url(&paper(&["1207.7214"])).unwrap().as_str(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            arxiv_pdf_url(&paper(&["hep-th/9901001"])).unwrap().as_str(),
            "https://arxiv.org/pdf/hep-th/9901001"
        );
        assert!(matches!(
            arxiv_pdf_url(&paper(&[])),
            Err(Error::MissingArxivIdentifier)
        ));
    }

    #[tokio::test]
    async fn rejects_missing_invalid_and_non_pdf_documents() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();
        assert!(matches!(
            store.fetch(&paper(&[]), FetchPolicy::UseCache).await,
            Err(Error::MissingArxivIdentifier)
        ));
        assert!(matches!(
            store
                .fetch(&paper(&["not-an-id"]), FetchPolicy::UseCache)
                .await,
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
            store
                .fetch(&paper(&["1207.7214"]), FetchPolicy::Force)
                .await,
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
            store
                .fetch(&paper(&["1207.7214"]), FetchPolicy::UseCache)
                .await,
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
                .fetch(&paper(&["1207.7214"]), FetchPolicy::UseCache)
                .await,
            Err(Error::InvalidCachedPdf(path)) if path == destination
        ));
    }

    #[test]
    fn cache_errors_are_policy_neutral() {
        let path = PathBuf::from("cache/paper.pdf");

        assert_eq!(
            Error::InvalidCachedPdf(path.clone()).to_string(),
            "cached file cache/paper.pdf is not a valid PDF"
        );
        assert_eq!(
            Error::NotCached(path).to_string(),
            "PDF is not cached at cache/paper.pdf"
        );
    }
}
