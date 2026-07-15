//! INSPIRE metadata provider for PaperDB, built on a reusable async INSPIRE
//! literature API client.
//!
//! The client deliberately covers only direct literature lookup. It retains
//! unknown JSON fields, making typed consumers forward-compatible with API
//! additions without coupling them to the full INSPIRE schema. On top of it,
//! [`InspireProvider`] implements `paperdb_core::MetadataProvider`, mapping
//! raw INSPIRE records into provider-neutral resolved papers.
//!
//! ```no_run
//! # async fn example() -> Result<(), paperdb_inspire_client::Error> {
//! use paperdb_inspire_client::{Client, LiteratureId};
//!
//! let client = Client::new()?;
//! let record = client
//!     .literature(LiteratureId::arxiv("1207.7214")?)
//!     .await?;
//! println!("{}", record.metadata.titles[0].title);
//! # Ok(())
//! # }
//! ```

mod model;
mod provider;

pub use model::*;
pub use provider::InspireProvider;
use reqwest::StatusCode;
use std::time::Duration;
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://inspirehep.net/";
const DEFAULT_USER_AGENT: &str = concat!("paperdb-inspire-client/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiteratureId {
    Record(u64),
    Arxiv(String),
    Doi(String),
}

impl LiteratureId {
    pub fn record(id: u64) -> Result<Self, Error> {
        if id == 0 {
            return Err(Error::InvalidIdentifier(
                "record id must be positive".into(),
            ));
        }
        Ok(Self::Record(id))
    }

    pub fn arxiv(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        validate_arxiv(&id)?;
        Ok(Self::Arxiv(id))
    }

    pub fn doi(doi: impl Into<String>) -> Result<Self, Error> {
        let doi = doi.into();
        validate_doi(&doi)?;
        Ok(Self::Doi(doi))
    }
}

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        ClientBuilder::new().build()
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub async fn literature(&self, id: LiteratureId) -> Result<LiteratureRecord, Error> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::InvalidBaseUrl(self.base_url.to_string()))?;
            segments.pop_if_empty();
            segments.push("api");
            match &id {
                LiteratureId::Record(value) => {
                    if *value == 0 {
                        return Err(Error::InvalidIdentifier(
                            "record id must be positive".into(),
                        ));
                    }
                    segments.push("literature");
                    segments.push(&value.to_string());
                }
                LiteratureId::Arxiv(value) => {
                    validate_arxiv(value)?;
                    segments.push("arxiv");
                    segments.push(value);
                }
                LiteratureId::Doi(value) => {
                    validate_doi(value)?;
                    segments.push("doi");
                    segments.push(value);
                }
            }
        }

        let response = self.http.get(url).send().await.map_err(Error::Transport)?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(Error::NotFound(id));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::HttpStatus { status, body });
        }
        response.json().await.map_err(Error::MalformedResponse)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new().expect("the default INSPIRE client configuration is valid")
    }
}

#[derive(Clone, Debug)]
pub struct ClientBuilder {
    base_url: String,
    user_agent: String,
    timeout: Duration,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            user_agent: DEFAULT_USER_AGENT.into(),
            timeout: Duration::from_secs(30),
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

    pub fn build(self) -> Result<Client, Error> {
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
        Ok(Client { http, base_url })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid literature identifier: {0}")]
    InvalidIdentifier(String),
    #[error("invalid INSPIRE base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("request to INSPIRE failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("INSPIRE record not found: {0:?}")]
    NotFound(LiteratureId),
    #[error("INSPIRE returned HTTP {status}: {body}")]
    HttpStatus { status: StatusCode, body: String },
    #[error("INSPIRE returned a malformed response: {0}")]
    MalformedResponse(#[source] reqwest::Error),
}

fn validate_arxiv(id: &str) -> Result<(), Error> {
    let without_version = paperdb_core::strip_arxiv_version(id);
    let modern = {
        let mut parts = without_version.split('.');
        matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None)
            if a.len() == 4 && a.bytes().all(|c| c.is_ascii_digit())
            && (b.len() == 4 || b.len() == 5) && b.bytes().all(|c| c.is_ascii_digit()))
    };
    let legacy = without_version
        .split_once('/')
        .is_some_and(|(archive, number)| {
            !archive.is_empty()
                && archive
                    .bytes()
                    .all(|c| c.is_ascii_alphabetic() || matches!(c, b'.' | b'-'))
                && number.len() == 7
                && number.bytes().all(|c| c.is_ascii_digit())
        });
    if modern || legacy {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier(format!("invalid arXiv id `{id}`")))
    }
}

fn validate_doi(doi: &str) -> Result<(), Error> {
    let valid = doi.starts_with("10.")
        && doi.split_once('/').is_some_and(|(registrant, suffix)| {
            registrant.len() > 3
                && registrant[3..].bytes().all(|c| c.is_ascii_digit())
                && !suffix.trim().is_empty()
                && !doi.bytes().any(|c| c.is_ascii_whitespace())
        });
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier(format!("invalid DOI `{doi}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn server(status: &str, body: &str, delay: Duration) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).unwrap();
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            String::from_utf8_lossy(&request[..length])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        (format!("http://{address}/"), handle)
    }

    fn fixture() -> &'static str {
        r#"{"id":"42","metadata":{"titles":[{"title":"Example","new_title_field":true}],"new_metadata_field":{"kept":true}},"new_record_field":7}"#
    }

    #[tokio::test]
    async fn constructs_record_arxiv_legacy_and_encoded_doi_urls() {
        let cases = [
            (LiteratureId::record(42).unwrap(), "/api/literature/42"),
            (
                LiteratureId::arxiv("2401.00001").unwrap(),
                "/api/arxiv/2401.00001",
            ),
            (
                LiteratureId::arxiv("hep-th/9901001").unwrap(),
                "/api/arxiv/hep-th%2F9901001",
            ),
            (
                LiteratureId::doi("10.1000/a/b").unwrap(),
                "/api/doi/10.1000%2Fa%2Fb",
            ),
        ];
        for (id, expected_path) in cases {
            let (base_url, handle) = server("200 OK", fixture(), Duration::ZERO);
            let client = Client::builder().base_url(base_url).build().unwrap();
            let record = client.literature(id).await.unwrap();
            assert_eq!(record.id.as_deref(), Some("42"));
            assert!(record.extra.contains_key("new_record_field"));
            assert!(record.metadata.extra.contains_key("new_metadata_field"));
            assert_eq!(record.metadata.titles[0].extra["new_title_field"], true);
            let request_line = handle.join().unwrap();
            assert!(request_line.contains(expected_path), "{request_line}");
        }
    }

    #[tokio::test]
    async fn distinguishes_not_found_http_and_malformed_responses() {
        let (base_url, not_found_server) = server("404 Not Found", "{}", Duration::ZERO);
        let client = Client::builder().base_url(base_url).build().unwrap();
        assert!(matches!(
            client.literature(LiteratureId::Record(1)).await,
            Err(Error::NotFound(LiteratureId::Record(1)))
        ));
        not_found_server.join().unwrap();

        let (base_url, error_server) = server("503 Service Unavailable", "down", Duration::ZERO);
        let client = Client::builder().base_url(base_url).build().unwrap();
        assert!(matches!(
            client.literature(LiteratureId::Record(1)).await,
            Err(Error::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE,
                ..
            })
        ));
        error_server.join().unwrap();

        let (base_url, malformed_server) = server("200 OK", "not-json", Duration::ZERO);
        let client = Client::builder().base_url(base_url).build().unwrap();
        assert!(matches!(
            client.literature(LiteratureId::Record(1)).await,
            Err(Error::MalformedResponse(_))
        ));
        malformed_server.join().unwrap();
    }

    #[tokio::test]
    async fn applies_configured_timeout() {
        let (base_url, handle) = server("200 OK", fixture(), Duration::from_millis(200));
        let client = Client::builder()
            .base_url(base_url)
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        match client.literature(LiteratureId::Record(1)).await {
            Err(Error::Transport(error)) => assert!(error.is_timeout()),
            result => panic!("expected timeout, got {result:?}"),
        }
        handle.join().unwrap();
    }

    #[test]
    fn validates_identifiers_and_base_urls() {
        assert!(LiteratureId::record(0).is_err());
        assert!(LiteratureId::arxiv("bad").is_err());
        assert!(LiteratureId::doi("not-a-doi").is_err());
        assert!(Client::builder().base_url("not a URL").build().is_err());
    }
}
