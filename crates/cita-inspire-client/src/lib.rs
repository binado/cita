//! Direct BibTeX access to the INSPIRE literature API.

use biblatex::RawBibliography;
use cita_core::Locator;
use reqwest::{StatusCode, header::RETRY_AFTER};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://inspirehep.net/";
const DEFAULT_USER_AGENT: &str = concat!("cita-inspire-client/", env!("CARGO_PKG_VERSION"));
const MAX_BATCH_KEYS: usize = 100;
const MAX_ENCODED_QUERY: usize = 6 * 1024;
const MAX_429_RETRIES: usize = 3;

#[derive(Clone, Debug)]
pub struct BatchResponse {
    pub requested_keys: Vec<String>,
    pub bibtex: String,
}

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    retry_fallback: Duration,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        ClientBuilder::new().build()
    }
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub async fn lookup(&self, locator: &Locator) -> Result<String, Error> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::InvalidBaseUrl(self.base_url.to_string()))?;
            segments.pop_if_empty();
            segments.push("api");
            match locator {
                Locator::Inspire(id) => {
                    segments.push("literature");
                    segments.push(&id.to_string());
                }
                Locator::Arxiv(id) => {
                    segments.push("arxiv");
                    segments.push(id);
                }
                Locator::Doi(doi) => {
                    segments.push("doi");
                    segments.push(doi);
                }
            }
        }
        url.query_pairs_mut().append_pair("format", "bibtex");
        self.request(url, locator.to_string()).await
    }

    pub async fn lookup_key(&self, key: &str) -> Result<String, Error> {
        validate_key(key)?;
        let url = self.search_url(&[key.to_owned()])?;
        self.request(url, format!("texkey:{key}")).await
    }

    /// Fetch sequential direct-search batches in deterministic input order.
    pub async fn lookup_keys(&self, keys: &[String]) -> Result<Vec<BatchResponse>, Error> {
        let chunks = batch_keys(keys)?;
        let mut responses = Vec::with_capacity(chunks.len());
        for requested_keys in chunks {
            let url = self.search_url(&requested_keys)?;
            let label = requested_keys.join(", ");
            let bibtex = self.request(url, format!("texkeys {label}")).await?;
            responses.push(BatchResponse {
                requested_keys,
                bibtex,
            });
        }
        Ok(responses)
    }

    fn search_url(&self, keys: &[String]) -> Result<Url, Error> {
        let mut url = self
            .base_url
            .join("api/literature")
            .map_err(|_| Error::InvalidBaseUrl(self.base_url.to_string()))?;
        let query = keys
            .iter()
            .map(|key| format!("texkey:{key}"))
            .collect::<Vec<_>>()
            .join(" or ");
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("format", "bibtex")
            .append_pair("size", &keys.len().to_string());
        Ok(url)
    }

    async fn request(&self, url: Url, resource: String) -> Result<String, Error> {
        let mut retries = 0;
        loop {
            let response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(Error::Transport)?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS && retries < MAX_429_RETRIES {
                retries += 1;
                let delay = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(retry_after_delay)
                    .unwrap_or(self.retry_fallback);
                tokio::time::sleep(delay).await;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Err(Error::NotFound(resource));
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(Error::HttpStatus { status, body });
            }
            let body = response.text().await.map_err(Error::Transport)?;
            validate_bibtex_body(&body)?;
            return Ok(body);
        }
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new().expect("default INSPIRE configuration is valid")
    }
}

#[derive(Clone, Debug)]
pub struct ClientBuilder {
    base_url: String,
    user_agent: String,
    timeout: Duration,
    retry_fallback: Duration,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            user_agent: DEFAULT_USER_AGENT.into(),
            timeout: Duration::from_secs(30),
            retry_fallback: Duration::from_secs(5),
        }
    }
    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }
    pub fn retry_fallback(mut self, value: Duration) -> Self {
        self.retry_fallback = value;
        self
    }
    pub fn build(self) -> Result<Client, Error> {
        let mut base_url =
            Url::parse(&self.base_url).map_err(|_| Error::InvalidBaseUrl(self.base_url.clone()))?;
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(Error::Transport)?;
        Ok(Client {
            http,
            base_url,
            retry_fallback: self.retry_fallback,
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid INSPIRE base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("unsafe INSPIRE texkey `{0}`")]
    UnsafeKey(String),
    #[error("INSPIRE request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("INSPIRE did not find {0}")]
    NotFound(String),
    #[error("INSPIRE returned HTTP {status}: {body}")]
    HttpStatus { status: StatusCode, body: String },
    #[error("INSPIRE returned malformed BibTeX: {0}")]
    MalformedBibtex(String),
}

pub fn batch_keys(keys: &[String]) -> Result<Vec<Vec<String>>, Error> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    for key in keys {
        validate_key(key)?;
        let needs_new = batches.last().is_some_and(|batch| {
            batch.len() == MAX_BATCH_KEYS || {
                let mut candidate = batch.clone();
                candidate.push(key.clone());
                encoded_query_len(&candidate) > MAX_ENCODED_QUERY
            }
        });
        if needs_new || batches.is_empty() {
            batches.push(Vec::new());
        }
        batches.last_mut().expect("batch exists").push(key.clone());
    }
    Ok(batches)
}

fn encoded_query_len(keys: &[String]) -> usize {
    let query = keys
        .iter()
        .map(|key| format!("texkey:{key}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", &query);
    serializer.finish().len()
}

fn validate_key(key: &str) -> Result<(), Error> {
    if !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'+' | b'-')
        })
    {
        Ok(())
    } else {
        Err(Error::UnsafeKey(key.to_owned()))
    }
}

fn retry_after_delay(value: &str) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            httpdate::parse_http_date(value)
                .ok()
                .map(|time| time.duration_since(SystemTime::now()).unwrap_or_default())
        })
}

fn validate_bibtex_body(body: &str) -> Result<(), Error> {
    let raw =
        RawBibliography::parse(body).map_err(|error| Error::MalformedBibtex(error.to_string()))?;
    if !raw.preamble.is_empty() || !raw.abbreviations.is_empty() {
        return Err(Error::MalformedBibtex(
            "response contains unsupported BibTeX directives".into(),
        ));
    }
    let mut cursor = 0;
    for entry in raw.entries {
        if !body[cursor..entry.span.start].trim().is_empty() {
            return Err(Error::MalformedBibtex(
                "response contains non-entry content".into(),
            ));
        }
        cursor = entry
            .span
            .end
            .checked_add(1)
            .filter(|end| *end <= body.len() && body.as_bytes()[*end - 1] == b'}')
            .ok_or_else(|| Error::MalformedBibtex("entry has no closing brace".into()))?;
    }
    if !body[cursor..].trim().is_empty() {
        return Err(Error::MalformedBibtex(
            "response contains non-entry content".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn server(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            responses.into_iter().map(|(status, headers, body)| {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 16384];
            let length = stream.read(&mut request).unwrap();
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&request[..length]).lines().next().unwrap().to_owned()
        }).collect()
        });
        (format!("http://{address}/"), handle)
    }

    #[tokio::test]
    async fn single_lookup_requests_bibtex() {
        let bib = "@article{A,title={A}}";
        let (base, handle) = server(vec![("200 OK", "", bib)]);
        let client = Client::builder().base_url(base).build().unwrap();
        assert_eq!(
            client.lookup(&"1207.7214".parse().unwrap()).await.unwrap(),
            bib
        );
        assert!(handle.join().unwrap()[0].contains("GET /api/arxiv/1207.7214?format=bibtex "));
    }

    #[tokio::test]
    async fn retries_three_rate_limits() {
        let bib = "@article{A,title={A}}";
        let (base, handle) = server(vec![
            ("429 Too Many Requests", "Retry-After: 0\r\n", ""),
            ("429 Too Many Requests", "Retry-After: 0\r\n", ""),
            ("429 Too Many Requests", "Retry-After: 0\r\n", ""),
            ("200 OK", "", bib),
        ]);
        let client = Client::builder().base_url(base).build().unwrap();
        assert_eq!(client.lookup_key("A").await.unwrap(), bib);
        assert_eq!(handle.join().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn reports_status_and_malformed_body_errors() {
        let (base, handle) = server(vec![("500 Server Error", "", "broken")]);
        let client = Client::builder().base_url(base).build().unwrap();
        assert!(matches!(
            client.lookup_key("A").await,
            Err(Error::HttpStatus { status, .. }) if status == StatusCode::INTERNAL_SERVER_ERROR
        ));
        handle.join().unwrap();

        let (base, handle) = server(vec![("200 OK", "", "not BibTeX")]);
        let client = Client::builder().base_url(base).build().unwrap();
        assert!(matches!(
            client.lookup_key("A").await,
            Err(Error::MalformedBibtex(_))
        ));
        handle.join().unwrap();
    }

    #[test]
    fn splits_at_one_hundred_keys_and_encoded_limit() {
        let keys = (0..101)
            .map(|index| format!("K{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            batch_keys(&keys)
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            [100, 1]
        );
        let long = (0..3)
            .map(|index| format!("K{index}{}", "x".repeat(3000)))
            .collect::<Vec<_>>();
        assert_eq!(batch_keys(&long).unwrap().len(), 2);
    }
}
