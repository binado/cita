//! Listing and its JSON projection.

use crate::error::Error;
use bibi_core::{Record, RecordFilter};
use bibi_manifest::Manifest;
use serde::Serialize;

/// Which records to list.
#[derive(Clone, Debug, Default)]
pub struct ListRequest {
    /// The ordinary filters, already complete (including any `--local` set).
    pub filter: RecordFilter,
}

/// Filter a loaded manifest, returning records in local-key order.
///
/// Pure, like [`render_manifest`](crate::render_manifest): the caller loads the
/// manifest, so listing has no I/O of its own and the same bytes always yield
/// the same records.
///
/// Filtering by provider is a pure manifest query — an uninstalled provider
/// simply matches nothing here, because listing never has to *call* it. Whether
/// naming such a provider is *useful* is the adapter's question, and the binary
/// answers it before building the filter by checking the name against the
/// providers it carries plus those this manifest already names. `--local` is
/// structural: it selects records with no provider id.
pub fn list(manifest: &Manifest, request: &ListRequest) -> Vec<Record> {
    manifest.filter(&request.filter).cloned().collect()
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
