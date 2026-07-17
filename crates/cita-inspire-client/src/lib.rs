//! INSPIRE JSON metadata provider with authoritative BibTeX snapshots.

use cita_bibliography::parse as parse_bibtex;
use cita_core::{
    Identifiers, Locator, MetadataProvider, ProjectionError, ProviderError, Publication, Reference,
    ReferenceSource, normalize_arxiv, normalize_doi,
};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime},
};
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://inspirehep.net/";
const DEFAULT_USER_AGENT: &str = concat!("cita-inspire-client/", env!("CARGO_PKG_VERSION"));
const MAX_BATCH_RECORDS: usize = 100;
const MAX_ENCODED_QUERY: usize = 6 * 1024;
const MAX_429_RETRIES: usize = 3;
// The base URL is configurable, so a server-supplied Retry-After is honored
// only up to this ceiling to keep a misbehaving endpoint from hanging the CLI.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireSnapshot {
    pub record_id: u64,
    pub updated: String,
    pub texkeys: Vec<String>,
    pub bibtex: String,
    pub titles: Vec<Title>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<Collaboration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publication_info: Vec<PublicationInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arxiv_eprints: Vec<ArxivEprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dois: Vec<Doi>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<UrlValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprint_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_date: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub full_name: String,
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub role: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collaboration {
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub curated_relation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArxivEprint {
    pub value: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doi {
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrlValue {
    pub value: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        One(String),
        Many(Vec<String>),
    }
    Option::<Value>::deserialize(deserializer).map(|value| match value {
        None => Vec::new(),
        Some(Value::One(value)) => vec![value],
        Some(Value::Many(values)) => values,
    })
}

impl InspireSnapshot {
    pub fn validate(&self) -> Result<(), ProjectionError> {
        if self.record_id == 0 {
            return Err(ProjectionError::Invalid("INSPIRE record id is zero".into()));
        }
        let reference = self.project()?;
        if self.texkeys.is_empty() {
            return Err(ProjectionError::Invalid(
                "INSPIRE snapshot has no texkeys".into(),
            ));
        }
        let entries = parse_bibtex(&self.bibtex)
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        if entries.len() != 1 {
            return Err(ProjectionError::Invalid(format!(
                "INSPIRE snapshot has {} BibTeX entries",
                entries.len()
            )));
        }
        let key = entries.keys().next().expect("length checked");
        if !self.texkeys.contains(key) {
            return Err(ProjectionError::Invalid(format!(
                "BibTeX key `{key}` is not one of the INSPIRE texkeys"
            )));
        }
        let bib_reference = entries.values().next().expect("length checked").project()?;
        if !same_record(&reference, &bib_reference) {
            return Err(ProjectionError::Invalid(
                "INSPIRE JSON and BibTeX do not identify the same record".into(),
            ));
        }
        Ok(())
    }
}

fn same_record(json: &Reference, bib: &Reference) -> bool {
    let shared_doi = json
        .identifiers
        .dois
        .iter()
        .any(|id| bib.identifiers.dois.contains(id));
    let shared_arxiv = json
        .identifiers
        .arxiv
        .iter()
        .any(|id| bib.identifiers.arxiv.contains(id));
    shared_doi
        || shared_arxiv
        || (json.identifiers.dois.is_empty() && json.identifiers.arxiv.is_empty())
}

impl ReferenceSource for InspireSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let title = self
            .titles
            .first()
            .map(|item| item.title.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or(ProjectionError::MissingTitle)?;
        let publication_info = select_publication(self);
        let publication = publication_info.map(to_publication);
        let arxiv = unique(
            self.arxiv_eprints
                .iter()
                .map(|item| normalize_arxiv(&item.value)),
        );
        let dois = unique(self.dois.iter().map(|item| normalize_doi(&item.value)));
        let year = publication
            .as_ref()
            .and_then(|item| item.year)
            .or_else(|| date_year(self.preprint_date.as_deref()))
            .or_else(|| date_year(self.earliest_date.as_deref()))
            .or_else(|| arxiv.first().and_then(|id| arxiv_year(id)));
        let primary_category = self
            .arxiv_eprints
            .first()
            .and_then(|item| item.categories.first())
            .cloned();
        let mut providers = BTreeMap::new();
        providers.insert("inspire".into(), vec![self.record_id.to_string()]);
        Ok(Reference {
            title,
            authors: self
                .authors
                .iter()
                .filter(|author| {
                    author.role.is_empty()
                        || author
                            .role
                            .iter()
                            .any(|role| role.eq_ignore_ascii_case("author"))
                })
                .map(|author| author.full_name.clone())
                .collect(),
            collaborations: self
                .collaborations
                .iter()
                .map(|item| item.value.clone())
                .collect(),
            year,
            publication,
            url: self.urls.first().map(|url| url.value.clone()).or_else(|| {
                Some(format!(
                    "https://inspirehep.net/literature/{}",
                    self.record_id
                ))
            }),
            primary_category,
            identifiers: Identifiers {
                dois,
                arxiv,
                providers,
            },
        })
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn select_publication(snapshot: &InspireSnapshot) -> Option<&PublicationInfo> {
    let visible = |item: &&PublicationInfo| !item.hidden && item.journal_title.is_some();
    snapshot
        .publication_info
        .iter()
        .filter(visible)
        .find(|item| item.curated_relation && (item.page_start.is_some() || item.artid.is_some()))
        .or_else(|| {
            snapshot
                .publication_info
                .iter()
                .filter(visible)
                .find(|item| {
                    item.year.is_some()
                        && (item.journal_volume.is_some()
                            || item.page_start.is_some()
                            || item.artid.is_some())
                })
        })
}

fn to_publication(item: &PublicationInfo) -> Publication {
    let pages = match (&item.page_start, &item.page_end, &item.artid) {
        (Some(start), Some(end), _) if start != end => Some(format!("{start}-{end}")),
        (Some(start), _, _) => Some(start.clone()),
        (_, _, Some(article)) => Some(article.clone()),
        _ => None,
    };
    Publication {
        journal: item.journal_title.clone(),
        volume: item.journal_volume.clone(),
        issue: item.journal_issue.clone(),
        pages,
        year: item.year,
    }
}

fn date_year(date: Option<&str>) -> Option<i32> {
    date?.get(..4)?.parse().ok()
}
fn arxiv_year(id: &str) -> Option<i32> {
    if id.as_bytes().get(4) == Some(&b'.') {
        return Some(2000 + id.get(..2)?.parse::<i32>().ok()?);
    }
    let year = id.split_once('/')?.1.get(..2)?.parse::<i32>().ok()?;
    Some(if year >= 91 { 1900 + year } else { 2000 + year })
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

    pub async fn resolve_reference(&self, locator: &Locator) -> Result<Reference, Error> {
        let record = self.lookup_json(locator).await?;
        snapshot_from_record(record, String::new())?
            .project()
            .map_err(|error| Error::Malformed(error.to_string()))
    }

    pub async fn resolve_snapshot(&self, locator: &Locator) -> Result<InspireSnapshot, Error> {
        let record = self.lookup_json(locator).await?;
        let bibtex = self.lookup_bibtex(locator).await?;
        let snapshot = snapshot_from_record(record, bibtex)?;
        snapshot
            .validate()
            .map_err(|error| Error::Malformed(error.to_string()))?;
        Ok(snapshot)
    }

    pub async fn refresh_records(&self, ids: &[u64]) -> Result<Vec<InspireSnapshot>, Error> {
        let mut output = Vec::with_capacity(ids.len());
        for batch in batch_ids(ids) {
            let (json_url, bib_url) = self.search_urls(&batch)?;
            let json = self
                .request(json_url, format!("INSPIRE records {batch:?}"))
                .await?;
            let response: SearchResponse =
                serde_json::from_str(&json).map_err(|error| Error::Malformed(error.to_string()))?;
            let mut records = response.hits.hits;
            let bibtex = self
                .request(bib_url, format!("INSPIRE records {batch:?}"))
                .await?;
            let mut bib_entries =
                parse_bibtex(&bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
            for id in batch {
                let position = records
                    .iter()
                    .position(|record| record.record_id() == Some(id))
                    .ok_or(Error::NotFound(format!("inspire:{id}")))?;
                let record = records.remove(position);
                let metadata = &record.metadata;
                let matching = bib_entries
                    .keys()
                    .filter(|key| metadata.texkeys.contains(key))
                    .cloned()
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    return Err(Error::Malformed(format!(
                        "record {id} matched {} BibTeX entries",
                        matching.len()
                    )));
                }
                let bib = bib_entries
                    .remove(&matching[0])
                    .expect("matching key exists")
                    .bibtex;
                let snapshot = snapshot_from_record(record, bib)?;
                snapshot
                    .validate()
                    .map_err(|error| Error::Malformed(error.to_string()))?;
                output.push(snapshot);
            }
            if !records.is_empty() || !bib_entries.is_empty() {
                return Err(Error::Malformed(
                    "INSPIRE returned unexplained refresh results".into(),
                ));
            }
        }
        Ok(output)
    }

    async fn lookup_json(&self, locator: &Locator) -> Result<LiteratureRecord, Error> {
        let url = self.record_url(locator, "json")?;
        let body = self.request(url, locator.to_string()).await?;
        serde_json::from_str(&body).map_err(|error| Error::Malformed(error.to_string()))
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
        let mut bib = base;
        bib.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("format", "bibtex")
            .append_pair("size", &ids.len().to_string());
        Ok((json, bib))
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
                eprintln!(
                    "INSPIRE rate limited the request for {resource}; \
                     retrying in {delay:?} (attempt {retries} of {MAX_429_RETRIES})"
                );
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
    type Snapshot = InspireSnapshot;
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

#[derive(Clone, Debug, Deserialize)]
struct LiteratureRecord {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    updated: Option<String>,
    metadata: LiteratureMetadata,
}
impl LiteratureRecord {
    fn record_id(&self) -> Option<u64> {
        self.id
            .as_u64()
            .or_else(|| self.id.as_str()?.parse().ok())
            .or(self.metadata.control_number)
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
struct LiteratureMetadata {
    #[serde(default)]
    control_number: Option<u64>,
    #[serde(default)]
    titles: Vec<ApiTitle>,
    #[serde(default)]
    authors: Vec<ApiAuthor>,
    #[serde(default)]
    collaborations: Vec<ApiCollaboration>,
    #[serde(default)]
    texkeys: Vec<String>,
    #[serde(default)]
    publication_info: Vec<ApiPublicationInfo>,
    #[serde(default)]
    arxiv_eprints: Vec<ApiArxivEprint>,
    #[serde(default)]
    dois: Vec<ApiDoi>,
    #[serde(default)]
    urls: Vec<ApiUrlValue>,
    #[serde(default, alias = "document_type")]
    document_types: Vec<String>,
    #[serde(default)]
    preprint_date: Option<String>,
    #[serde(default)]
    earliest_date: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiTitle {
    title: String,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiAuthor {
    full_name: String,
    #[serde(default, deserialize_with = "one_or_many")]
    role: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiCollaboration {
    value: String,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiPublicationInfo {
    #[serde(default)]
    journal_title: Option<String>,
    #[serde(default)]
    journal_volume: Option<String>,
    #[serde(default)]
    journal_issue: Option<String>,
    #[serde(default)]
    page_start: Option<String>,
    #[serde(default)]
    page_end: Option<String>,
    #[serde(default)]
    artid: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    curated_relation: bool,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiArxivEprint {
    value: String,
    #[serde(default)]
    categories: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiDoi {
    value: String,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiUrlValue {
    value: String,
}

fn snapshot_from_record(
    record: LiteratureRecord,
    bibtex: String,
) -> Result<InspireSnapshot, Error> {
    let record_id = record
        .record_id()
        .ok_or_else(|| Error::Malformed("record has no numeric id".into()))?;
    let m = record.metadata;
    Ok(InspireSnapshot {
        record_id,
        updated: record
            .updated
            .ok_or_else(|| Error::Malformed("record has no update timestamp".into()))?,
        texkeys: m.texkeys,
        bibtex,
        titles: m
            .titles
            .into_iter()
            .map(|v| Title { title: v.title })
            .collect(),
        authors: m
            .authors
            .into_iter()
            .map(|v| Author {
                full_name: v.full_name,
                role: v.role,
            })
            .collect(),
        collaborations: m
            .collaborations
            .into_iter()
            .map(|v| Collaboration { value: v.value })
            .collect(),
        publication_info: m
            .publication_info
            .into_iter()
            .map(|v| PublicationInfo {
                journal_title: v.journal_title,
                journal_volume: v.journal_volume,
                journal_issue: v.journal_issue,
                page_start: v.page_start,
                page_end: v.page_end,
                artid: v.artid,
                year: v.year,
                hidden: v.hidden,
                curated_relation: v.curated_relation,
            })
            .collect(),
        arxiv_eprints: m
            .arxiv_eprints
            .into_iter()
            .map(|v| ArxivEprint {
                value: v.value,
                categories: v.categories,
            })
            .collect(),
        dois: m.dois.into_iter().map(|v| Doi { value: v.value }).collect(),
        urls: m
            .urls
            .into_iter()
            .map(|v| UrlValue { value: v.value })
            .collect(),
        document_types: m.document_types,
        preprint_date: m.preprint_date,
        earliest_date: m.earliest_date,
    })
}

#[derive(Deserialize)]
struct SearchResponse {
    hits: SearchHits,
}
#[derive(Deserialize)]
struct SearchHits {
    #[serde(default)]
    hits: Vec<LiteratureRecord>,
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
impl Default for Client {
    fn default() -> Self {
        Self::new().expect("default INSPIRE configuration is valid")
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid INSPIRE base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("INSPIRE request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("INSPIRE did not find {0}")]
    NotFound(String),
    #[error("INSPIRE returned HTTP {status}: {body}")]
    HttpStatus { status: StatusCode, body: String },
    #[error("INSPIRE returned malformed data: {0}")]
    Malformed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_selects_roles_publication_and_fallbacks() {
        let record: LiteratureRecord = serde_json::from_value(serde_json::json!({
            "id":"1124337", "updated":"2025-01-01", "metadata": {
                "titles":[{"title":"First"}], "authors":[{"full_name":"Aad, G."},{"full_name":"Editor", "role":"editor"}],
                "texkeys":["Aad:2012tfa"], "arxiv_eprints":[{"value":"1207.7214v2", "categories":["hep-ex"]}],
                "dois":[{"value":"10.1/ABC"}], "publication_info":[{"journal_title":"JHEP", "year":2012, "artid":"1", "curated_relation":true}]
            }})).unwrap();
        let snapshot = snapshot_from_record(
            record,
            "@article{Aad:2012tfa,title={First},doi={10.1/ABC},eprint={1207.7214}}".into(),
        )
        .unwrap();
        let projected = snapshot.project().unwrap();
        assert_eq!(projected.year, Some(2012));
        assert_eq!(projected.authors, ["Aad, G."]);
        assert_eq!(projected.identifiers.arxiv, ["1207.7214"]);
        snapshot.validate().unwrap();
    }

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
