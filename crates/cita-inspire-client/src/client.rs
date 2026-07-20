use crate::{InspireRecord, snapshot::SelectedRecord, wire::SearchResponse};
use cita_bibliography::{parse as parse_bibtex, project_bibtex};
use cita_core::{Locator, MetadataProvider, ProviderError, Reference, ReferenceSource};
use reqwest::{StatusCode, header::RETRY_AFTER};
use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://inspirehep.net/";
const DEFAULT_USER_AGENT: &str = concat!("cita-inspire-client/", env!("CARGO_PKG_VERSION"));
const MAX_BATCH_RECORDS: usize = 100;
const MAX_ENCODED_QUERY: usize = 6 * 1024;
const MAX_429_RETRIES: usize = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Eq, PartialEq)]
/// Notification emitted immediately before retrying a rate-limited request.
pub struct RetryEvent {
    /// Human-readable resource being requested.
    pub resource: String,
    /// Delay before the next attempt.
    pub delay: Duration,
    /// One-based retry attempt number.
    pub attempt: usize,
    /// Maximum number of retries after the initial request.
    pub max_retries: usize,
}

type RetryObserver = Arc<dyn Fn(&RetryEvent) + Send + Sync>;

#[derive(Clone)]
/// HTTP client for resolving and refreshing INSPIRE literature records.
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    retry_fallback: Duration,
    on_retry: RetryObserver,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("retry_fallback", &self.retry_fallback)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Create a client with the default INSPIRE endpoint and retry settings.
    pub fn new() -> Result<Self, Error> {
        ClientBuilder::new().build()
    }

    /// Begin configuring an INSPIRE client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Resolve only source-neutral JSON metadata without fetching BibTeX.
    pub async fn resolve_reference(&self, locator: &Locator) -> Result<Reference, Error> {
        self.lookup_json(locator)
            .await?
            .project()
            .map_err(|error| Error::Malformed(error.to_string()))
    }

    /// Resolve JSON and authoritative BibTeX into a durable record.
    pub async fn resolve_snapshot(&self, locator: &Locator) -> Result<InspireRecord, Error> {
        let record = self.lookup_json(locator).await?;
        let bibtex = self.lookup_bibtex(locator).await?;
        let mut entries =
            parse_bibtex(&bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
        if entries.len() != 1 {
            return Err(Error::Malformed(format!(
                "INSPIRE returned {} BibTeX entries for one record",
                entries.len()
            )));
        }
        let (key, snapshot) = entries.pop_first().expect("length checked");
        cross_check(record, key, snapshot.bibtex)
    }

    /// Refresh records by stable INSPIRE ID in bounded search batches.
    pub async fn refresh_records(&self, ids: &[u64]) -> Result<Vec<InspireRecord>, Error> {
        let mut output = Vec::with_capacity(ids.len());
        for batch in batch_ids(ids) {
            let (json_url, bib_url) = self.search_urls(&batch)?;
            let json = self
                .request(json_url, format!("INSPIRE records {batch:?}"))
                .await?;
            let response: SearchResponse =
                serde_json::from_str(&json).map_err(|error| Error::Malformed(error.to_string()))?;
            let mut records = response
                .hits
                .hits
                .into_iter()
                .map(|record| record.into_selected())
                .collect::<Result<Vec<_>, _>>()?;
            let bibtex = self
                .request(bib_url, format!("INSPIRE records {batch:?}"))
                .await?;
            let mut bib_entries =
                parse_bibtex(&bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
            for id in batch {
                let position = records
                    .iter()
                    .position(|record| record.record_id() == id)
                    .ok_or(Error::NotFound(format!("inspire:{id}")))?;
                let record = records.remove(position);
                let matching = bib_entries
                    .keys()
                    .filter(|key| record.texkeys().contains(key))
                    .cloned()
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    return Err(Error::Malformed(format!(
                        "record {id} matched {} BibTeX entries",
                        matching.len()
                    )));
                }
                let key = matching.into_iter().next().expect("length checked");
                let bibtex = bib_entries
                    .remove(&key)
                    .expect("matching key exists")
                    .bibtex;
                output.push(cross_check(record, key, bibtex)?);
            }
            if !records.is_empty() || !bib_entries.is_empty() {
                return Err(Error::Malformed(
                    "INSPIRE returned unexplained refresh results".into(),
                ));
            }
        }
        Ok(output)
    }

    async fn lookup_json(&self, locator: &Locator) -> Result<SelectedRecord, Error> {
        let url = self.record_url(locator, "json")?;
        let body = self.request(url, locator.to_string()).await?;
        let record = serde_json::from_str::<crate::wire::LiteratureRecord>(&body)
            .map_err(|error| Error::Malformed(error.to_string()))?;
        record.into_selected()
    }

    async fn lookup_bibtex(&self, locator: &Locator) -> Result<String, Error> {
        let url = self.record_url(locator, "bibtex")?;
        self.request(url, locator.to_string()).await
    }

    fn record_url(&self, locator: &Locator, format: &str) -> Result<Url, Error> {
        let mut url = self.base_url.clone();
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
        drop(segments);
        url.query_pairs_mut().append_pair("format", format);
        Ok(url)
    }

    fn search_urls(&self, ids: &[u64]) -> Result<(Url, Url), Error> {
        let query = ids
            .iter()
            .map(|id| format!("control_number:{id}"))
            .collect::<Vec<_>>()
            .join(" or ");
        let base = self
            .base_url
            .join("api/literature")
            .map_err(|_| Error::InvalidBaseUrl(self.base_url.to_string()))?;
        let mut json = base.clone();
        json.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("format", "json")
            .append_pair("size", &ids.len().to_string());
        let mut bibtex = base;
        bibtex
            .query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("format", "bibtex")
            .append_pair("size", &ids.len().to_string());
        Ok((json, bibtex))
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
                    .unwrap_or(self.retry_fallback)
                    .min(MAX_RETRY_AFTER);
                let event = RetryEvent {
                    resource: resource.clone(),
                    delay,
                    attempt: retries,
                    max_retries: MAX_429_RETRIES,
                };
                (self.on_retry)(&event);
                tokio::time::sleep(delay).await;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Err(Error::NotFound(resource));
            }
            if !status.is_success() {
                return Err(Error::HttpStatus {
                    status,
                    body: response
                        .text()
                        .await
                        .unwrap_or_else(|error| format!("(unreadable response body: {error})")),
                });
            }
            return response.text().await.map_err(Error::Transport);
        }
    }
}

impl MetadataProvider for Client {
    type Snapshot = InspireRecord;

    async fn resolve(&self, locator: &Locator) -> Result<Self::Snapshot, ProviderError> {
        self.resolve_snapshot(locator).await.map_err(provider_error)
    }

    async fn refresh(&self, provider_ids: &[String]) -> Result<Vec<Self::Snapshot>, ProviderError> {
        let ids = provider_ids
            .iter()
            .map(|id| {
                id.parse::<u64>()
                    .map_err(|_| ProviderError::InvalidLocator(format!("inspire:{id}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.refresh_records(&ids).await.map_err(provider_error)
    }
}

fn provider_error(error: Error) -> ProviderError {
    match error {
        Error::NotFound(value) => ProviderError::NotFound(value),
        Error::Malformed(value) => ProviderError::Malformed(value),
        error => ProviderError::Request(error.to_string()),
    }
}

/// Confirm that an authoritative BibTeX entry describes the selected record,
/// then fuse the two into a durable record keyed by the canonical texkey.
fn cross_check(
    record: SelectedRecord,
    bibtex_key: String,
    bibtex: String,
) -> Result<InspireRecord, Error> {
    if !record.texkeys().contains(&bibtex_key) {
        return Err(Error::Malformed(format!(
            "BibTeX key `{bibtex_key}` is not one of the INSPIRE texkeys"
        )));
    }
    let bib_reference =
        project_bibtex(&bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
    if !record.identity_matches(&bib_reference) {
        return Err(Error::Malformed(
            "INSPIRE JSON and BibTeX do not identify the same record".into(),
        ));
    }
    Ok(record.into_record(bibtex_key, bibtex, &bib_reference))
}

fn batch_ids(ids: &[u64]) -> Vec<Vec<u64>> {
    let mut batches: Vec<Vec<u64>> = Vec::new();
    for id in ids {
        let needs_new = batches.last().is_some_and(|batch| {
            batch.len() == MAX_BATCH_RECORDS || {
                let mut candidate = batch.clone();
                candidate.push(*id);
                encoded_query_len(&candidate) > MAX_ENCODED_QUERY
            }
        });
        if needs_new || batches.is_empty() {
            batches.push(Vec::new());
        }
        batches.last_mut().expect("batch exists").push(*id);
    }
    batches
}

fn encoded_query_len(ids: &[u64]) -> usize {
    let query = ids
        .iter()
        .map(|id| format!("control_number:{id}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", &query);
    serializer.finish().len()
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

#[derive(Clone)]
/// Configures an INSPIRE [`Client`].
pub struct ClientBuilder {
    base_url: String,
    user_agent: String,
    timeout: Duration,
    retry_fallback: Duration,
    on_retry: RetryObserver,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .field("retry_fallback", &self.retry_fallback)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// Create a builder with production defaults.
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            user_agent: DEFAULT_USER_AGENT.into(),
            timeout: Duration::from_secs(30),
            retry_fallback: Duration::from_secs(5),
            on_retry: Arc::new(|_| {}),
        }
    }

    /// Override the INSPIRE-compatible base URL.
    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    /// Override the HTTP user-agent header.
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }

    /// Override the request timeout.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }

    /// Set the delay used when `Retry-After` is absent or invalid.
    pub fn retry_fallback(mut self, value: Duration) -> Self {
        self.retry_fallback = value;
        self
    }

    /// Register an observer called before each rate-limit retry.
    pub fn on_retry(mut self, observer: impl Fn(&RetryEvent) + Send + Sync + 'static) -> Self {
        self.on_retry = Arc::new(observer);
        self
    }

    /// Validate the configuration and construct the client.
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
            on_retry: self.on_retry,
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new().expect("default INSPIRE configuration is valid")
    }
}

#[derive(Debug, Error)]
/// Error produced by INSPIRE configuration, requests, or response validation.
pub enum Error {
    /// The configured base URL is invalid.
    #[error("invalid INSPIRE base URL: {0}")]
    InvalidBaseUrl(String),
    /// An HTTP request failed before a response was received.
    #[error("INSPIRE request failed: {0}")]
    Transport(#[source] reqwest::Error),
    /// No record matched the requested locator or stable ID.
    #[error("INSPIRE did not find {0}")]
    NotFound(String),
    /// INSPIRE returned an unsuccessful response.
    #[error("INSPIRE returned HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP response status.
        status: StatusCode,
        /// Response body retained for diagnostics.
        body: String,
    },
    /// INSPIRE returned inconsistent or malformed data.
    #[error("INSPIRE returned malformed data: {0}")]
    Malformed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batching_splits_at_the_record_limit_and_respects_the_query_limit() {
        let ids = (1..=205).collect::<Vec<u64>>();
        let batches = batch_ids(&ids);
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [100, 100, 5]
        );
        assert_eq!(batches.concat(), ids);
        assert!(
            batches
                .iter()
                .all(|batch| encoded_query_len(batch) <= MAX_ENCODED_QUERY)
        );
        assert!(batch_ids(&[]).is_empty());
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_delay("7"), Some(Duration::from_secs(7)));
        let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(300));
        let delay = retry_after_delay(&future).unwrap();
        assert!(delay > Duration::from_secs(250) && delay <= Duration::from_secs(300));
        let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(300));
        assert_eq!(retry_after_delay(&past), Some(Duration::ZERO));
        assert_eq!(retry_after_delay("soon"), None);
    }
}
