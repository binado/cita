//! Listing and its JSON projection.

use crate::error::Error;
use bibi_core::{Record, RecordFilter};
use bibi_manifest::ManifestStore;
use serde::Serialize;

/// Which records to list.
#[derive(Clone, Debug, Default)]
pub struct ListRequest {
    /// The ordinary filters, already complete (including any `--local` set).
    pub filter: RecordFilter,
}

/// Load and filter, returning records in local-key order.
///
/// `--provider <name>` is a pure manifest query: the named provider need not be
/// installed, and an uninstalled one yields an empty listing rather than an
/// error, because listing never has to *call* it. `--local` is answered by the
/// caller filling [`RecordFilter::unrefreshable_providers`] from the registry's
/// capabilities, never by comparing a stored provider name with a literal.
pub fn list(store: &ManifestStore, request: &ListRequest) -> Result<Vec<Record>, Error> {
    let manifest = store.load()?.manifest;
    Ok(manifest.filter(&request.filter).cloned().collect())
}

/// The stable schema-1 JSON shape of one record.
///
/// The payload is deliberately absent: `show`, `list --format bibtex`, and
/// `export` are the BibTeX views, and this one exists for metadata filtering
/// and shell tooling. Opaque provider values are strings rather than numbers,
/// because the next provider's handles will not be numeric.
#[derive(Debug, Serialize)]
pub struct ListJsonRecord {
    id: String,
    key: String,
    provider: String,
    provider_id: Option<String>,
    revision: Option<String>,
    doi: Option<String>,
    arxiv: Option<String>,
    title: String,
    authors: Vec<String>,
    collaborations: Vec<String>,
    year: Option<i32>,
}

impl ListJsonRecord {
    fn from_record(record: &Record) -> Self {
        Self {
            id: record.id.to_string(),
            key: record.key.to_string(),
            provider: record.provenance.provider.to_string(),
            provider_id: record
                .provenance
                .provider_id
                .as_ref()
                .map(ToString::to_string),
            revision: record.provenance.revision.as_ref().map(ToString::to_string),
            doi: record.identifiers.doi.as_ref().map(ToString::to_string),
            arxiv: record.identifiers.arxiv.as_ref().map(ToString::to_string),
            title: record.description.title.clone(),
            authors: record.description.authors.clone(),
            collaborations: record.description.collaborations.clone(),
            year: record.description.year,
        }
    }
}

/// Project records to the schema-1 JSON array, ending in one newline.
///
/// Generated fresh on every invocation and never stored, cached, or read back.
/// A user who wants a file redirects stdout, and that file is ordinary output
/// bibi does not maintain.
pub fn to_json(records: &[Record]) -> Result<String, Error> {
    let projected = records
        .iter()
        .map(ListJsonRecord::from_record)
        .collect::<Vec<_>>();
    let mut rendered = serde_json::to_string_pretty(&projected)?;
    rendered.push('\n');
    Ok(rendered)
}

/// One local key per line.
pub fn to_keys(records: &[Record]) -> String {
    records
        .iter()
        .map(|record| record.key.to_string())
        .map(|key| key + "\n")
        .collect()
}
