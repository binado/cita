//! Retrieval: HTTP, pacing, and retry. Nothing here interprets a response.
//!
//! This half is public so that diagnostics and fixture capture can call it
//! alone — emitting INSPIRE's unabridged answer is the first step, run by
//! itself. It is also where pacing and retry live, so every path that reaches
//! INSPIRE, including a diagnostic one, is rate-limited by construction.

use crate::{
    error::{self},
    rate_limit::{Clock, RateLimiter, SystemClock},
    retry::{MAX_RETRIES, RetryEvent, RetryObserver, delay_after},
};
use bibi_core::provider::RetrievalError;
use reqwest::{StatusCode, header::RETRY_AFTER};
use std::{sync::Arc, time::Duration};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://inspirehep.net/";
const DEFAULT_USER_AGENT: &str = concat!("bibi-inspire/", env!("CARGO_PKG_VERSION"));

/// An unparsed INSPIRE JSON response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawJson(String);

/// An unparsed INSPIRE BibTeX response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawBibtex(String);

impl RawJson {
    /// Wrap a response body, for fixtures and captured diagnostics.
    pub fn new(body: impl Into<String>) -> Self {
        Self(body.into())
    }

    /// The body as received.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RawBibtex {
    /// Wrap a response body, for fixtures and captured diagnostics.
    pub fn new(body: impl Into<String>) -> Self {
        Self(body.into())
    }

    /// The body as received.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The INSPIRE fields bibi maps.
///
/// Requesting only these is most of the reason a refresh is cheap: the response
/// carries what the record model uses and nothing else. A diagnostic that wants
/// the full record makes its own unnarrowed request rather than widening this.
pub const RECORD_FIELDS: &[&str] = &[
    "control_number",
    "texkeys",
    "titles",
    "authors.full_name",
    "collaborations",
    "publication_info.year",
    "preprint_date",
    "arxiv_eprints",
    "dois",
];

/// The fields the payload join needs, and no more.
pub const JOIN_FIELDS: &[&str] = &["control_number", "texkeys"];

/// INSPIRE retrieval.
#[derive(Clone)]
pub struct Transport {
    http: reqwest::Client,
    base_url: Url,
    limiter: Arc<RateLimiter>,
    clock: Arc<dyn Clock>,
    on_retry: RetryObserver,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transport")
            .field("base_url", &self.base_url)
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

impl Transport {
    /// Begin configuring a transport.
    pub fn builder() -> TransportBuilder {
        TransportBuilder::default()
    }

    /// Run one search, returning the structured response.
    pub async fn search_json(
        &self,
        query: &str,
        size: usize,
        fields: &[&str],
    ) -> Result<RawJson, RetrievalError> {
        let mut url = self.search_url()?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", query);
            pairs.append_pair("format", "json");
            pairs.append_pair("size", &size.to_string());
            if !fields.is_empty() {
                pairs.append_pair("fields", &fields.join(","));
            }
        }
        Ok(RawJson(self.request(url, query).await?))
    }

    /// Run one search, returning concatenated BibTeX entries.
    pub async fn search_bibtex(
        &self,
        query: &str,
        size: usize,
    ) -> Result<RawBibtex, RetrievalError> {
        let mut url = self.search_url()?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", query);
            pairs.append_pair("format", "bibtex");
            pairs.append_pair("size", &size.to_string());
        }
        Ok(RawBibtex(self.request(url, query).await?))
    }

    fn search_url(&self) -> Result<Url, RetrievalError> {
        self.base_url
            .join("api/literature")
            .map_err(|_| error::configuration(format!("invalid base URL `{}`", self.base_url)))
    }

    /// Issue one request, paced, with bounded retries on 429.
    async fn request(&self, url: Url, resource: &str) -> Result<String, RetrievalError> {
        let mut attempt = 0;
        loop {
            // Pacing comes first, so a retry does not jump the queue.
            self.limiter.acquire(self.clock.as_ref()).await;
            let response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(error::transport)?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt >= MAX_RETRIES {
                    return Err(error::rate_limited(attempt));
                }
                attempt += 1;
                let delay = delay_after(
                    response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|value| value.to_str().ok()),
                );
                (self.on_retry)(&RetryEvent {
                    resource: resource.to_owned(),
                    delay,
                    attempt,
                    max_retries: MAX_RETRIES,
                });
                self.clock.sleep(delay).await;
                continue;
            }
            if !status.is_success() {
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|error| format!("(unreadable body: {error})"));
                return Err(error::status(status.as_u16(), body));
            }
            return response.text().await.map_err(error::transport);
        }
    }
}

/// Configures a [`Transport`].
pub struct TransportBuilder {
    base_url: String,
    user_agent: String,
    timeout: Duration,
    clock: Arc<dyn Clock>,
    limiter: Arc<RateLimiter>,
    on_retry: RetryObserver,
}

impl Default for TransportBuilder {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(30),
            clock: Arc::new(SystemClock),
            limiter: Arc::new(RateLimiter::default()),
            on_retry: Arc::new(|_| {}),
        }
    }
}

impl TransportBuilder {
    /// Point the client at another INSPIRE-compatible base URL.
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

    /// Supply the clock every wait goes through.
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Supply the rate limiter, for tests that need a smaller budget.
    pub fn limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = limiter;
        self
    }

    /// Register an observer called before each rate-limit retry.
    pub fn on_retry(mut self, observer: impl Fn(&RetryEvent) + Send + Sync + 'static) -> Self {
        self.on_retry = Arc::new(observer);
        self
    }

    /// Validate the configuration and build the transport.
    pub fn build(self) -> Result<Transport, RetrievalError> {
        let mut base_url = Url::parse(&self.base_url)
            .map_err(|_| error::configuration(format!("invalid base URL `{}`", self.base_url)))?;
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(error::transport)?;
        Ok(Transport {
            http,
            base_url,
            limiter: self.limiter,
            clock: self.clock,
            on_retry: self.on_retry,
        })
    }
}
